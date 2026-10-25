//! 可选的 HTTPS 监听和本地 CA。
//!
//! 为什么需要它：浏览器的 File System Access API（`showDirectoryPicker`）
//! 只在安全上下文里可用。局域网内用 `http://IP:8080` 打开页面时，Chrome
//! 不会提供这个 API；改成 `https://IP:8443` 并信任本地 CA 之后才可以。
//!
//! 这里生成一个长期 CA（客户端只需信任一次）和一年期的服务器证书。
//! 服务器证书每次启动时按当前局域网 IP 重新签发，换 IP 时不需要重新信任 CA。

use std::fs;
use std::path::{Path, PathBuf};

use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use rcgen::{
    BasicConstraints, CertificateParams, DnType, ExtendedKeyUsagePurpose, IsCa, Issuer, KeyPair,
    KeyUsagePurpose,
};
use time::{Duration, OffsetDateTime};

use crate::config;
use crate::error::{AppError, AppResult};

const CA_FILE: &str = "ca.pem";
const CA_KEY_FILE: &str = "ca-key.pem";
const CERT_FILE: &str = "server.pem";
const KEY_FILE: &str = "server-key.pem";
const CA_CN: &str = "Wiretoss Local CA";

pub struct TlsPaths {
    pub cert: PathBuf,
    pub key: PathBuf,
}

/// 生成/读取 CA，并按当前局域网地址签发新的服务器证书。
pub fn ensure() -> AppResult<TlsPaths> {
    let dir = config::certs_dir()?;
    ensure_in(&dir)
}

/// 公开的 CA 证书路径。客户端安装这个文件并在系统里设为信任。
pub fn ca_path() -> AppResult<PathBuf> {
    Ok(config::certs_dir()?.join(CA_FILE))
}

/// axum-server 使用 rustls 但不自带加密后端；启动 HTTPS 前装一次 ring。
pub fn install_crypto_provider() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}

/// 浏览器下载 CA 证书的公开路由（不需要登录）。
pub async fn ca_cert() -> Response {
    let Ok(path) = ca_path() else {
        return (StatusCode::NOT_FOUND, "CA certificate not found").into_response();
    };
    match fs::read(&path) {
        Ok(pem) => (
            [
                (header::CONTENT_TYPE, "application/x-pem-file"),
                (
                    header::CONTENT_DISPOSITION,
                    "attachment; filename=\"wiretoss-ca.pem\"",
                ),
            ],
            pem,
        )
            .into_response(),
        Err(_) => (StatusCode::NOT_FOUND, "CA certificate not found").into_response(),
    }
}

fn ensure_in(dir: &Path) -> AppResult<TlsPaths> {
    fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(dir, fs::Permissions::from_mode(0o700));
    }

    let ca_path = dir.join(CA_FILE);
    let ca_key_path = dir.join(CA_KEY_FILE);
    let cert_path = dir.join(CERT_FILE);
    let key_path = dir.join(KEY_FILE);

    let ca_key = if ca_path.exists() && ca_key_path.exists() {
        let pem = fs::read_to_string(&ca_key_path)
            .map_err(|e| AppError::Internal(format!("读取 HTTPS CA 私钥失败：{e}")))?;
        KeyPair::from_pem(&pem)
            .map_err(|e| AppError::Internal(format!("解析 HTTPS CA 私钥失败：{e}")))?
    } else {
        let key = KeyPair::generate()
            .map_err(|e| AppError::Internal(format!("生成 HTTPS CA 私钥失败：{e}")))?;
        let params = ca_params()?;
        let cert = params
            .self_signed(&key)
            .map_err(|e| AppError::Internal(format!("生成 HTTPS CA 失败：{e}")))?;
        write_private(&ca_key_path, key.serialize_pem().as_bytes())?;
        fs::write(&ca_path, cert.pem())?;
        key
    };

    // 用 CA 每次重新签发服务器证书：IP 变化或旧证书临近过期时，
    // 客户端不用重新信任 CA。
    let issuer = Issuer::new(ca_params()?, ca_key);
    let leaf_key = KeyPair::generate()
        .map_err(|e| AppError::Internal(format!("生成 HTTPS 服务器私钥失败：{e}")))?;
    let params = leaf_params()?;
    let cert = params
        .signed_by(&leaf_key, &issuer)
        .map_err(|e| AppError::Internal(format!("签发 HTTPS 服务器证书失败：{e}")))?;

    fs::write(&cert_path, cert.pem())?;
    write_private(&key_path, leaf_key.serialize_pem().as_bytes())?;

    Ok(TlsPaths {
        cert: cert_path,
        key: key_path,
    })
}

fn ca_params() -> AppResult<CertificateParams> {
    let mut params = CertificateParams::new(Vec::<String>::new())
        .map_err(|e| AppError::Internal(format!("构造 HTTPS CA 参数失败：{e}")))?;
    let now = OffsetDateTime::now_utc();
    params.not_before = now - Duration::days(1);
    params.not_after = now + Duration::days(3650);
    params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    params.distinguished_name.push(DnType::CommonName, CA_CN);
    params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
    Ok(params)
}

fn leaf_params() -> AppResult<CertificateParams> {
    let mut params = CertificateParams::new(sans())
        .map_err(|e| AppError::Internal(format!("构造 HTTPS 证书参数失败：{e}")))?;
    let now = OffsetDateTime::now_utc();
    params.not_before = now - Duration::days(1);
    params.not_after = now + Duration::days(365);
    params.is_ca = IsCa::NoCa;
    params
        .distinguished_name
        .push(DnType::CommonName, "Wiretoss");
    params.key_usages = vec![
        KeyUsagePurpose::DigitalSignature,
        KeyUsagePurpose::KeyEncipherment,
    ];
    params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
    params.use_authority_key_identifier_extension = true;
    Ok(params)
}

fn sans() -> Vec<String> {
    let mut names = vec!["localhost".to_string(), "127.0.0.1".to_string()];
    let host = crate::net::hostname();
    names.push(host.clone());
    names.push(format!("{host}.local"));
    for ip in crate::net::lan_ipv4_addresses() {
        names.push(ip.to_string());
    }
    names.sort();
    names.dedup();
    names
}

fn write_private(path: &Path, bytes: &[u8]) -> AppResult<()> {
    fs::write(path, bytes)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_ca_and_server_certificates() {
        let dir = tempfile::tempdir().unwrap();
        let paths = ensure_in(dir.path()).unwrap();
        assert!(paths.cert.exists());
        assert!(paths.key.exists());
        assert!(dir.path().join(CA_FILE).exists());
        assert!(dir.path().join(CA_KEY_FILE).exists());
        let cert = fs::read_to_string(&paths.cert).unwrap();
        assert!(cert.contains("BEGIN CERTIFICATE"));
        let key = fs::read_to_string(&paths.key).unwrap();
        assert!(key.contains("BEGIN PRIVATE KEY"));
        // 第二次启动复用 CA，但仍会重新签发服务器证书。
        let again = ensure_in(dir.path()).unwrap();
        assert!(again.cert.exists());
    }
}
