//! 原生客户端的协议与落盘逻辑。
//!
//! 这一层不碰 GUI：登录、SSE 解析、manifest 校验、逐文件下载/重试/上报都在
//! 这里完成，集成测试可以直接驱动它。GUI 运行时在 `gui::client`。

use std::collections::{HashMap, HashSet};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use futures_util::{Stream, StreamExt};
use reqwest::cookie::Jar;
use serde::Deserialize;
use serde_json::json;
use tokio::io::AsyncWriteExt;
use unicode_normalization::UnicodeNormalization;

use crate::auth::CSRF_HEADER;
use crate::clients::{PushNotice, CLIENT_ID_HEADER};
use crate::push::{
    numbered_name, validate_relative_dest, ManifestItem, ManifestResponse, PendingBatch,
    MAX_ENTRIES, MAX_TOTAL_BYTES,
};

const MAX_SSE_BUFFER: usize = 1024 * 1024;
const DOWNLOAD_ATTEMPTS: usize = 3;

#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error("登录失败：用户名或密码错误")]
    Auth,
    #[error("连接失败：{0}")]
    Network(String),
    #[error("服务器返回 {0}：{1}")]
    Http(u16, String),
    #[error("路径不安全：{0}")]
    UnsafePath(String),
    #[error("写入失败：{0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Other(String),
}

impl From<reqwest::Error> for ClientError {
    fn from(e: reqwest::Error) -> Self {
        ClientError::Network(e.to_string())
    }
}

impl ClientError {
    /// 网络抖动、服务端临时错误、磁盘忙可以重试；鉴权/路径/404 重试也没用。
    pub fn retryable(&self) -> bool {
        match self {
            ClientError::Network(_) | ClientError::Io(_) => true,
            ClientError::Http(status, _) => *status == 408 || *status == 429 || *status >= 500,
            _ => false,
        }
    }

    pub fn is_auth(&self) -> bool {
        matches!(self, ClientError::Auth)
    }
}

#[derive(Clone)]
pub struct ClientSession {
    http: reqwest::Client,
    base: reqwest::Url,
    client_id: String,
    display_name: String,
}

impl ClientSession {
    pub fn new(
        base_url: &str,
        client_id: String,
        display_name: String,
    ) -> Result<Self, ClientError> {
        let base = normalize_server_url(base_url).map_err(ClientError::Other)?;
        // 不设整体 timeout：SSE 是长连接，只有建连超时才有意义。
        let http = reqwest::Client::builder()
            .cookie_provider(Arc::new(Jar::default()))
            .connect_timeout(Duration::from_secs(5))
            .user_agent(format!("wiretoss/{}", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| ClientError::Other(format!("创建 HTTP 客户端失败：{e}")))?;
        Ok(Self {
            http,
            base,
            client_id,
            display_name,
        })
    }

    pub fn client_id(&self) -> &str {
        &self.client_id
    }

    fn url(&self, path: &str) -> reqwest::Url {
        self.base.join(path).unwrap_or_else(|_| self.base.clone())
    }

    pub async fn login(&self, username: &str, password: &str) -> Result<(), ClientError> {
        let resp = self
            .http
            .post(self.url("/api/login"))
            .json(&json!({ "username": username, "password": password }))
            .send()
            .await?;
        if resp.status().is_success() {
            Ok(())
        } else {
            Err(status_error(resp).await)
        }
    }

    /// 打开定向事件流。返回的字节流由调用方用 [`SseParser`] 增量解析。
    pub async fn events(&self) -> Result<impl Stream<Item = reqwest::Result<Bytes>>, ClientError> {
        let mut url = self.url("/api/client/events");
        url.query_pairs_mut()
            .append_pair("name", &self.display_name);
        let resp = self
            .http
            .get(url)
            .header(CLIENT_ID_HEADER, self.client_id.as_str())
            .send()
            .await?;
        if !resp.status().is_success() {
            return Err(status_error(resp).await);
        }
        Ok(resp.bytes_stream())
    }

    pub async fn fetch_manifest(&self, batch_id: &str) -> Result<ManifestResponse, ClientError> {
        validate_batch_id(batch_id)?;
        let resp = self
            .http
            .get(self.url(&format!("/api/push/{batch_id}/manifest")))
            .header(CLIENT_ID_HEADER, self.client_id.as_str())
            .send()
            .await?;
        if !resp.status().is_success() {
            return Err(status_error(resp).await);
        }
        Ok(resp.json::<ManifestResponse>().await?)
    }

    pub async fn pending(&self) -> Result<Vec<PendingBatch>, ClientError> {
        #[derive(Deserialize)]
        struct PendingResponse {
            #[serde(default)]
            batches: Vec<PendingBatch>,
        }
        let resp = self
            .http
            .get(self.url("/api/push/pending"))
            .header(CLIENT_ID_HEADER, self.client_id.as_str())
            .send()
            .await?;
        if !resp.status().is_success() {
            return Err(status_error(resp).await);
        }
        Ok(resp.json::<PendingResponse>().await?.batches)
    }

    pub async fn report_progress(&self, batch_id: &str, done: u64) -> Result<(), ClientError> {
        validate_batch_id(batch_id)?;
        let resp = self
            .http
            .post(self.url(&format!("/api/push/{batch_id}/progress")))
            .header(CLIENT_ID_HEADER, self.client_id.as_str())
            .header(CSRF_HEADER, "1")
            .json(&json!({ "done": done }))
            .send()
            .await?;
        if resp.status().is_success() {
            Ok(())
        } else {
            Err(status_error(resp).await)
        }
    }

    pub async fn complete(
        &self,
        batch_id: &str,
        ok: bool,
        error: Option<&str>,
    ) -> Result<(), ClientError> {
        validate_batch_id(batch_id)?;
        let resp = self
            .http
            .post(self.url(&format!("/api/push/{batch_id}/complete")))
            .header(CLIENT_ID_HEADER, self.client_id.as_str())
            .header(CSRF_HEADER, "1")
            .json(&json!({ "ok": ok, "error": error }))
            .send()
            .await?;
        if resp.status().is_success() {
            Ok(())
        } else {
            Err(status_error(resp).await)
        }
    }
}

async fn status_error(resp: reqwest::Response) -> ClientError {
    let status = resp.status().as_u16();
    let text = resp.text().await.unwrap_or_default();
    let message = serde_json::from_str::<serde_json::Value>(&text)
        .ok()
        .and_then(|v| v.get("error").and_then(|e| e.as_str()).map(str::to_string))
        .unwrap_or(text);
    if status == 401 {
        ClientError::Auth
    } else {
        ClientError::Http(status, message)
    }
}

/// 接受 `host:port` 或 `http://host:port`。当前构建没有 TLS，所以明确拒绝 https。
pub fn normalize_server_url(input: &str) -> Result<reqwest::Url, String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err("服务器地址不能为空".into());
    }
    let with_scheme = if trimmed.contains("://") {
        trimmed.to_string()
    } else {
        format!("http://{trimmed}")
    };
    let url = reqwest::Url::parse(&with_scheme).map_err(|e| format!("服务器地址无效：{e}"))?;
    match url.scheme() {
        "http" => {}
        "https" => return Err("当前版本不支持 HTTPS，请使用 http:// 地址".into()),
        other => return Err(format!("不支持的协议：{other}")),
    }
    if url.host().is_none() {
        return Err("服务器地址缺少主机名".into());
    }
    if url.path() != "/" && !url.path().is_empty() {
        return Err("服务器地址不要带路径".into());
    }
    if url.query().is_some() || url.fragment().is_some() {
        return Err("服务器地址不要带查询参数".into());
    }
    Ok(url)
}

fn validate_batch_id(batch_id: &str) -> Result<(), ClientError> {
    if batch_id.len() >= 8
        && batch_id.len() <= 64
        && batch_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        Ok(())
    } else {
        Err(ClientError::Other("批次标识无效".into()))
    }
}

// ---------------------------------------------------------------- SSE

/// 增量 SSE 解析。服务端事件都是单行 JSON；这里仍按规范处理多行 data。
#[derive(Default)]
pub struct SseParser {
    buf: Vec<u8>,
}

impl SseParser {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, chunk: &[u8]) -> Vec<PushNotice> {
        self.buf.extend_from_slice(chunk);
        let mut out = Vec::new();

        while let Some((frame_end, sep_end)) = frame_boundary(&self.buf) {
            let frame: Vec<u8> = self.buf.drain(..sep_end).collect();
            let text = String::from_utf8_lossy(&frame[..frame_end]);
            let mut data = String::new();
            for line in text.split('\n') {
                let line = line.strip_suffix('\r').unwrap_or(line);
                if let Some(rest) = line.strip_prefix("data:") {
                    if !data.is_empty() {
                        data.push('\n');
                    }
                    data.push_str(rest.strip_prefix(' ').unwrap_or(rest));
                }
            }
            if !data.is_empty() {
                match serde_json::from_str::<PushNotice>(&data) {
                    Ok(PushNotice::Skip) => {}
                    Ok(notice) => out.push(notice),
                    Err(e) => tracing::debug!("忽略无法解析的 SSE 事件：{e}"),
                }
            }
        }

        if self.buf.len() > MAX_SSE_BUFFER {
            tracing::warn!("SSE 缓冲区超过 1 MiB，丢弃");
            self.buf.clear();
        }
        out
    }
}

fn frame_boundary(buf: &[u8]) -> Option<(usize, usize)> {
    let mut i = 0;
    while i + 1 < buf.len() {
        if buf[i] == b'\n' && buf[i + 1] == b'\n' {
            return Some((i, i + 2));
        }
        if i + 3 < buf.len() && &buf[i..i + 4] == b"\r\n\r\n" {
            return Some((i, i + 4));
        }
        i += 1;
    }
    None
}

// ---------------------------------------------------------------- 落盘

#[derive(Debug, Clone)]
pub struct PlannedItem {
    pub index: usize,
    pub path: String,
    pub dest: PathBuf,
    pub is_dir: bool,
    pub size: u64,
}

/// 在写任何文件前完成全部校验与重名规划。
pub fn plan_destinations(
    receive: &Path,
    items: &[ManifestItem],
) -> Result<Vec<PlannedItem>, String> {
    if items.len() > MAX_ENTRIES {
        return Err(format!("条目超过 {} 个", MAX_ENTRIES));
    }
    let receive = receive
        .canonicalize()
        .map_err(|e| format!("接收目录无效（{}）：{e}", receive.display()))?;

    let mut top_names: HashMap<String, String> = HashMap::new();
    let mut used_top: HashSet<String> = HashSet::new();
    let mut seen_indexes = HashSet::new();
    let mut seen_dests = HashSet::new();
    let mut total_bytes = 0u64;
    let mut out = Vec::with_capacity(items.len());

    for item in items {
        if !seen_indexes.insert(item.index) {
            return Err("manifest 里的文件下标重复".into());
        }
        let parts = validate_relative_dest(&item.path)?;
        let first = parts[0].clone();
        let key: String = first.nfc().collect::<String>().to_lowercase();
        let mapped = if let Some(existing) = top_names.get(&key) {
            existing.clone()
        } else {
            // 只有顶层条目本身能告诉我们它是目录；子项的首组件一定是目录。
            let is_dir = item.is_dir && parts.len() == 1;
            let chosen = unique_top_level_name(&receive, &first, is_dir, &mut used_top)?;
            top_names.insert(key, chosen.clone());
            chosen
        };

        let mut dest = receive.clone();
        dest.push(&mapped);
        for part in &parts[1..] {
            dest.push(part);
        }
        if !dest.starts_with(&receive) {
            return Err("目标路径越界".into());
        }
        if !seen_dests.insert(dest.clone()) {
            return Err(format!("manifest 里有重复目标路径：{}", item.path));
        }
        if !item.is_dir {
            total_bytes = total_bytes.saturating_add(item.size);
        }
        out.push(PlannedItem {
            index: item.index,
            path: item.path.clone(),
            dest,
            is_dir: item.is_dir,
            size: item.size,
        });
    }

    if total_bytes > MAX_TOTAL_BYTES {
        return Err("总大小超过上限".into());
    }
    Ok(out)
}

fn unique_top_level_name(
    receive: &Path,
    desired: &str,
    is_dir: bool,
    used: &mut HashSet<String>,
) -> Result<String, String> {
    for n in 0..1000u32 {
        let name = if n == 0 {
            desired.to_string()
        } else {
            numbered_name(desired, is_dir, n)
        };
        let key = name.nfc().collect::<String>().to_lowercase();
        // 用 symlink_metadata 而不是 exists：断链的软链 exists() 返回 false，
        // 但落盘时它仍会占住这个名字。
        match std::fs::symlink_metadata(receive.join(&name)) {
            Ok(_) => continue,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => continue,
        }
        if used.insert(key) {
            return Ok(name);
        }
    }
    Err(format!("接收目录里同名项过多：{desired}"))
}

/// 逐级创建目录，且**不盲穿软链**：接收目录里预先存在的软链可能指向别处。
async fn ensure_dir(root: &Path, dir: &Path) -> Result<(), ClientError> {
    let rel = dir
        .strip_prefix(root)
        .map_err(|_| ClientError::UnsafePath(format!("{} 不在接收目录内", dir.display())))?;
    let mut cur = root.to_path_buf();
    for comp in rel.components() {
        let Component::Normal(name) = comp else {
            return Err(ClientError::UnsafePath("路径组件非法".into()));
        };
        cur.push(name);
        match tokio::fs::symlink_metadata(&cur).await {
            Ok(meta) => {
                if meta.file_type().is_symlink() {
                    return Err(ClientError::UnsafePath(format!(
                        "{} 是软链接",
                        cur.display()
                    )));
                }
                if !meta.is_dir() {
                    return Err(ClientError::UnsafePath(format!(
                        "{} 不是目录",
                        cur.display()
                    )));
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                match tokio::fs::create_dir(&cur).await {
                    Ok(()) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                        let meta = tokio::fs::symlink_metadata(&cur).await?;
                        if meta.file_type().is_symlink() || !meta.is_dir() {
                            return Err(ClientError::UnsafePath(format!(
                                "{} 不是普通目录",
                                cur.display()
                            )));
                        }
                    }
                    Err(e) => return Err(ClientError::Io(e)),
                }
            }
            Err(e) => return Err(ClientError::Io(e)),
        }
    }
    Ok(())
}

struct TempFileGuard {
    path: PathBuf,
    armed: bool,
}

impl Drop for TempFileGuard {
    fn drop(&mut self) {
        if self.armed {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

#[derive(Debug)]
pub struct BatchOutcome {
    pub files: u64,
    pub bytes: u64,
    pub errors: Vec<String>,
}

/// 下载整个批次。单个文件失败会继续尝试其余文件，最后统一 complete。
pub async fn run_batch(
    session: &ClientSession,
    manifest: &ManifestResponse,
    receive: &Path,
    on_progress: &(dyn Fn(u64) + Send + Sync),
) -> Result<BatchOutcome, ClientError> {
    validate_batch_id(&manifest.batch_id)?;
    let receive = receive.canonicalize().map_err(ClientError::Io)?;

    let planned = match plan_destinations(&receive, &manifest.items) {
        Ok(planned) => planned,
        Err(e) => {
            let _ = session.complete(&manifest.batch_id, false, Some(&e)).await;
            return Err(ClientError::UnsafePath(e));
        }
    };

    for item in &planned {
        if item.is_dir {
            if let Err(e) = ensure_dir(&receive, &item.dest).await {
                // 目录建不出来时也不会再有文件能落盘；直接 complete 失败，
                // 避免批次留在服务端被反复重试。
                let message = e.to_string();
                let _ = session
                    .complete(&manifest.batch_id, false, Some(&message))
                    .await;
                return Err(e);
            }
        }
    }

    let mut completed_bytes = 0u64;
    let mut files = 0u64;
    let mut errors = Vec::new();

    for item in &planned {
        if item.is_dir {
            continue;
        }
        let parent = item
            .dest
            .parent()
            .ok_or_else(|| ClientError::UnsafePath("目标没有父目录".into()))?;
        if let Err(e) = ensure_dir(&receive, parent).await {
            errors.push(format!("{}：{e}", item.path));
            continue;
        }

        match download_one(
            session,
            &manifest.batch_id,
            item,
            completed_bytes,
            on_progress,
        )
        .await
        {
            Ok(size) => {
                completed_bytes = completed_bytes.saturating_add(size);
                files += 1;
            }
            Err(e) => {
                let auth = e.is_auth();
                errors.push(format!("{}：{e}", item.path));
                if auth {
                    break;
                }
            }
        }

        // 进度上报失败不影响接收；下一次上报/complete 会覆盖。
        let _ = session
            .report_progress(&manifest.batch_id, completed_bytes)
            .await;
    }

    let ok = errors.is_empty();
    let summary = if ok {
        None
    } else {
        Some(
            errors
                .iter()
                .take(5)
                .cloned()
                .collect::<Vec<_>>()
                .join("；"),
        )
    };
    let _ = session
        .complete(&manifest.batch_id, ok, summary.as_deref())
        .await;

    Ok(BatchOutcome {
        files,
        bytes: completed_bytes,
        errors,
    })
}

async fn download_one(
    session: &ClientSession,
    batch_id: &str,
    item: &PlannedItem,
    completed_bytes: u64,
    on_progress: &(dyn Fn(u64) + Send + Sync),
) -> Result<u64, ClientError> {
    let mut last_error: Option<ClientError> = None;
    for attempt in 0..DOWNLOAD_ATTEMPTS {
        match download_attempt(session, batch_id, item, completed_bytes, on_progress).await {
            Ok(size) => return Ok(size),
            Err(e) => {
                if !e.retryable() || attempt + 1 >= DOWNLOAD_ATTEMPTS {
                    return Err(e);
                }
                last_error = Some(e);
                tokio::time::sleep(Duration::from_millis(300 * (1u64 << attempt))).await;
            }
        }
    }
    Err(last_error.unwrap_or_else(|| ClientError::Other("下载失败".into())))
}

async fn download_attempt(
    session: &ClientSession,
    batch_id: &str,
    item: &PlannedItem,
    completed_bytes: u64,
    on_progress: &(dyn Fn(u64) + Send + Sync),
) -> Result<u64, ClientError> {
    let resp = session
        .http
        .get(session.url(&format!("/api/push/{batch_id}/file/{}", item.index)))
        .header(CLIENT_ID_HEADER, session.client_id.as_str())
        .send()
        .await?;
    if !resp.status().is_success() {
        return Err(status_error(resp).await);
    }

    let parent = item
        .dest
        .parent()
        .ok_or_else(|| ClientError::UnsafePath("目标没有父目录".into()))?;
    let tmp_name = format!(
        ".wiretoss-{}-{}-{}.part",
        &batch_id[..batch_id.len().min(8)],
        item.index,
        uuid::Uuid::new_v4()
    );
    let tmp = parent.join(tmp_name);

    // guard 先声明、后打开文件：变量逆序 Drop，文件句柄会先释放，
    // Windows 上才能删除/重命名临时文件。
    let mut guard = TempFileGuard {
        path: tmp.clone(),
        armed: true,
    };
    let mut file = tokio::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&tmp)
        .await?;

    let mut stream = resp.bytes_stream();
    let mut size = 0u64;
    let mut last_callback = Instant::now();
    let mut last_report = Instant::now();

    loop {
        let next = tokio::time::timeout(Duration::from_secs(30), stream.next())
            .await
            .map_err(|_| ClientError::Network("下载超时".into()))?;
        let Some(chunk) = next else {
            break;
        };
        let chunk = chunk.map_err(ClientError::from)?;
        file.write_all(&chunk).await?;
        size = size.saturating_add(chunk.len() as u64);

        if last_callback.elapsed() >= Duration::from_millis(100) {
            on_progress(completed_bytes.saturating_add(size));
            last_callback = Instant::now();
        }
        if last_report.elapsed() >= Duration::from_millis(250) {
            let _ = session
                .report_progress(batch_id, completed_bytes.saturating_add(size))
                .await;
            last_report = Instant::now();
        }
    }

    file.flush().await?;
    drop(file);

    if size != item.size {
        // 网络截断或源文件在传输中变化都可能走到这里；归为可重试，
        // 给连接抖动一次机会，三次都不一致才真正失败。
        return Err(ClientError::Network(format!(
            "大小不一致（期望 {}，收到 {}）",
            item.size, size
        )));
    }
    on_progress(completed_bytes.saturating_add(size));

    // 规划时已避开磁盘上已有的名字；这里再查一次是为了挡住规划之后
    // 新出现的文件/目录，避免静默覆盖用户数据。
    let dest = if tokio::fs::symlink_metadata(&item.dest).await.is_ok() {
        unique_file_path(&item.dest).await?
    } else {
        item.dest.clone()
    };
    tokio::fs::rename(&tmp, &dest).await?;
    guard.armed = false;
    Ok(size)
}

async fn unique_file_path(path: &Path) -> Result<PathBuf, ClientError> {
    let parent = path
        .parent()
        .ok_or_else(|| ClientError::UnsafePath("目标没有父目录".into()))?;
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .ok_or_else(|| ClientError::UnsafePath("目标没有文件名".into()))?;
    for n in 1..1000u32 {
        let candidate = parent.join(numbered_name(&name, false, n));
        match tokio::fs::symlink_metadata(&candidate).await {
            Ok(_) => continue,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(candidate),
            Err(e) => return Err(ClientError::Io(e)),
        }
    }
    Err(ClientError::Other("无法找到可用的文件名".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_url_normalization() {
        assert_eq!(
            normalize_server_url("192.168.1.5:8080").unwrap().as_str(),
            "http://192.168.1.5:8080/"
        );
        assert_eq!(
            normalize_server_url("http://host:1234/").unwrap().as_str(),
            "http://host:1234/"
        );
        assert!(normalize_server_url("https://host:1234").is_err());
        assert!(normalize_server_url("http://host:1234/path").is_err());
        assert!(normalize_server_url("").is_err());
    }

    #[test]
    fn sse_parser_handles_chunk_boundaries_and_crlf() {
        let mut parser = SseParser::new();
        let payload = concat!(
            "data: {\"kind\":\"push_offer\",\"batch_id\":\"abc-123-xyz\",",
            "\"name\":\"照片\",\"total_files\":2,\"total_bytes\":10}\r\n\r\n"
        );
        let bytes = payload.as_bytes();
        let mut notices = parser.push(&bytes[..20]);
        notices.extend(parser.push(&bytes[20..]));
        assert_eq!(notices.len(), 1);
        assert!(matches!(notices[0], PushNotice::PushOffer { .. }));
    }

    #[test]
    fn sse_parser_ignores_comments_and_unknown_kinds() {
        let mut parser = SseParser::new();
        let out = parser.push(
            b": keep-alive\n\n\
              event: lagged\n\n\
              data: {\"kind\":\"future_thing\"}\n\n\
              data: {\"kind\":\"push_offer\",\"batch_id\":\"abc-123-xyz\",\"name\":\"x\",\"total_files\":0,\"total_bytes\":0}\n\n",
        );
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn plan_destinations_renames_existing_top_level() {
        let receive = tempfile::tempdir().unwrap();
        std::fs::create_dir(receive.path().join("Photos")).unwrap();
        let items = vec![
            ManifestItem {
                index: 0,
                path: "Photos".into(),
                is_dir: true,
                size: 0,
            },
            ManifestItem {
                index: 1,
                path: "Photos/a.txt".into(),
                is_dir: false,
                size: 3,
            },
        ];
        let planned = plan_destinations(receive.path(), &items).unwrap();
        assert!(planned[0].dest.ends_with("Photos (1)"));
        assert!(planned[1].dest.ends_with("Photos (1)/a.txt"));
    }

    #[test]
    fn plan_destinations_rejects_traversal() {
        let receive = tempfile::tempdir().unwrap();
        let items = vec![ManifestItem {
            index: 0,
            path: "../evil.txt".into(),
            is_dir: false,
            size: 1,
        }];
        assert!(plan_destinations(receive.path(), &items).is_err());
    }
}
