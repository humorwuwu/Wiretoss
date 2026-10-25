use std::fs;
use std::path::{Path, PathBuf};

// argon2 0.6 是一次破坏性重写（password-hash 0.6 / phc 0.6）：
// `hash_password` 不再接收盐参数、自己生成随机盐，`PasswordHash` 也搬到了 `phc` 下。
// 网上绝大多数 0.5 时代的示例在这里都编译不过。
use argon2::password_hash::phc::PasswordHash;
use argon2::password_hash::{PasswordHasher, PasswordVerifier};
use argon2::Argon2;
use directories::{BaseDirs, UserDirs};
use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult};

/// 配置格式用 JSON 而不是 TOML：serde_json 本来就是 API 层的依赖，
/// 少一个 crate 就少一处版本风险；配置项本身也就这么几个，
/// 真要改共享目录有 `--root` 参数，不需要靠手写注释。
const CONFIG_VERSION: u32 = 1;

/// 原生客户端模式的持久化设置。密码**不**存在这里。
///
/// 整个结构加 `#[serde(default)]`：老版本 config.json 里没有 `client` 字段，
/// 必须能继续加载，而不是让升级后的程序打不开配置。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ClientSettings {
    /// 稳定设备标识。服务端用它区分多台客户端；首次运行生成后不再变化。
    pub client_id: String,
    /// 展示给服务端用户的设备名，默认主机名，可由用户修改。
    pub display_name: String,
    /// 上次连接的服务器地址（`host:port` 或 `http://host:port`）。
    pub server_url: String,
    pub username: String,
    /// 接收目录。连接时由用户确认，之后每次连接默认用它。
    pub receive_dir: Option<PathBuf>,
}

impl ClientSettings {
    pub fn new() -> Self {
        Self {
            client_id: uuid::Uuid::new_v4().to_string(),
            display_name: crate::net::hostname(),
            server_url: String::new(),
            username: String::new(),
            receive_dir: Some(default_receive_dir()),
        }
    }

    /// 补齐旧配置里缺失/非法的标识与设备名。返回是否发生了修改。
    pub fn ensure_identity(&mut self) -> bool {
        let mut changed = false;
        if !valid_client_id(&self.client_id) {
            self.client_id = uuid::Uuid::new_v4().to_string();
            changed = true;
        }
        if self.display_name.trim().is_empty() {
            self.display_name = crate::net::hostname();
            changed = true;
        }
        changed
    }
}

/// 和服务端 `clients::parse_client_id` 保持同一套规则。
/// 配置被手动改坏时直接换一个新的，避免客户端一连接就吃 400。
fn valid_client_id(id: &str) -> bool {
    id.len() >= 8
        && id.len() <= 64
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// 程序启动时的界面模式。持久化后，客户端专用的机器下次启动不会误开服务。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum StartMode {
    #[default]
    Server,
    Client,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub version: u32,
    pub username: String,
    /// Argon2id 的 PHC 字符串。明文密码只存在于首启那一刻的终端输出里。
    pub password_hash: String,
    pub root: PathBuf,
    pub port: u16,
    pub session_hours: u64,
    /// 允许没有会话的访客只读浏览/下载。上传、删除、投送等写操作仍需登录。
    /// 旧配置文件没有这个字段时默认关闭。
    #[serde(default)]
    pub allow_guest: bool,
    /// 是否同时开一个 HTTPS 监听。桌面浏览器要调用 File System Access API
    /// 写本机目录，必须处于安全上下文；HTTP 监听仍然保留给手机。
    #[serde(default)]
    pub https: bool,
    #[serde(default = "default_https_port")]
    pub https_port: u16,
    /// 客户端模式设置。字段级 default 保证旧配置可读。
    #[serde(default)]
    pub client: ClientSettings,
    #[serde(default)]
    pub start_mode: StartMode,
}

/// 默认用户名。首次运行没指定就用它。
pub const DEFAULT_USERNAME: &str = "wiretoss";
pub const DEFAULT_HTTPS_PORT: u16 = 8443;

fn default_https_port() -> u16 {
    DEFAULT_HTTPS_PORT
}

impl Config {
    /// 配置是否已存在。用来区分"首次运行"和"改设置"两条完全不同的路径：
    /// 前者要引导用户设账号，后者不该再打扰他。
    pub fn exists() -> bool {
        config_path().map(|p| p.exists()).unwrap_or(false)
    }

    /// 读取已有配置。
    pub fn load() -> AppResult<Self> {
        let path = config_path()?;
        let raw = fs::read_to_string(&path)
            .map_err(|e| AppError::Internal(format!("读取配置失败（{}）：{e}", path.display())))?;
        let mut cfg: Self = serde_json::from_str(&raw)
            .map_err(|e| AppError::Internal(format!("配置文件损坏（{}）：{e}", path.display())))?;
        // 从旧版本升级时 client 字段是空的。补一个稳定标识并尽量写回，
        // 这样第一次连接客户端模式时服务端就能看到一致的设备身份。
        if cfg.client.ensure_identity() {
            if let Err(e) = cfg.save() {
                tracing::warn!("补齐客户端标识后保存配置失败（不影响本次运行）：{e}");
            }
        }
        Ok(cfg)
    }

    /// 首次运行时按用户给的账号密码创建配置。
    pub fn create(username: &str, password: &str) -> AppResult<Self> {
        let cfg = Config {
            version: CONFIG_VERSION,
            username: username.to_string(),
            password_hash: hash_password(password)?,
            root: default_root()?,
            port: 8080,
            session_hours: 12,
            client: ClientSettings::new(),
            start_mode: StartMode::Server,
            allow_guest: false,
            https: false,
            https_port: DEFAULT_HTTPS_PORT,
        };
        cfg.save()?;
        Ok(cfg)
    }

    /// 建一份**还没有密码**的配置，交给图形界面去引导用户设置。
    ///
    /// 刻意不生成随机密码：那串密码用户从头到尾没见过，却会让界面认为
    /// 「已经设过了」从而自动开服 —— 结果是开了个谁也登不进去的服务。
    /// 空的 password_hash 解析不出合法 PHC 字符串，`verify_password` 一律返回
    /// false，所以在用户设置密码之前，登录窗口是关死的。
    pub fn create_without_password(username: &str) -> AppResult<Self> {
        let cfg = Config {
            version: CONFIG_VERSION,
            username: username.to_string(),
            password_hash: String::new(),
            root: default_root()?,
            port: 8080,
            session_hours: 12,
            client: ClientSettings::new(),
            start_mode: StartMode::Server,
            allow_guest: false,
            https: false,
            https_port: DEFAULT_HTTPS_PORT,
        };
        cfg.save()?;
        Ok(cfg)
    }

    pub fn save(&self) -> AppResult<()> {
        let path = config_path()?;
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        let body = serde_json::to_string_pretty(self)?;
        fs::write(&path, body)?;
        restrict_permissions(&path)?;
        Ok(())
    }

    /// 改密码：先写配置再返回，避免内存与磁盘不一致。
    pub fn set_password(&mut self, plaintext: &str) -> AppResult<()> {
        self.password_hash = hash_password(plaintext)?;
        self.save()
    }

    pub fn verify_password(&self, candidate: &str) -> bool {
        let Ok(parsed) = PasswordHash::new(&self.password_hash) else {
            tracing::error!("配置里的 password_hash 不是合法的 PHC 字符串");
            return false;
        };
        Argon2::default()
            .verify_password(candidate.as_bytes(), &parsed)
            .is_ok()
    }
}

pub fn hash_password(plaintext: &str) -> AppResult<String> {
    // 0.6 起由 argon2 自己生成随机盐（走 getrandom），不再需要外部传入 OsRng——
    // 也就顺带消除了 0.5 时代 rand_core 版本对不齐的经典编译报错。
    let hash = Argon2::default()
        .hash_password(plaintext.as_bytes())
        .map_err(AppError::internal)?;
    Ok(hash.to_string())
}

/// 随机密码的默认长度。
///
/// 8 位看着短，但字母表是 32 个字符（去掉 0/O、1/l/I 这些抄错率高的），
/// 也就是 40 bit 熵。配合登录限流（连错 5 次就开始指数退避），
/// 在线暴力破解完全没有可行性；而它足够短，可以从一台机器的终端
/// 手敲到另一台机器上——这恰恰是这个工具最高频的使用方式。
pub const DEFAULT_PASSWORD_LEN: usize = 8;

/// 生成人类可读的随机密码。字母表剔除了易混淆字符，
/// 因为最常见的用法是"看着这台机器的终端，往另一台机器上敲"。
pub fn generate_password(len: usize) -> String {
    const ALPHABET: &[u8] = b"abcdefghjkmnpqrstuvwxyz23456789";
    let mut buf = vec![0u8; len];
    getrandom::fill(&mut buf).expect("操作系统随机数不可用");
    buf.iter()
        .map(|b| ALPHABET[*b as usize % ALPHABET.len()] as char)
        .collect()
}

/// 配置目录：macOS `~/Library/Application Support/Wiretoss`、
/// Windows `%APPDATA%\Wiretoss`、Linux `~/.config/Wiretoss`。
pub fn config_dir() -> AppResult<PathBuf> {
    let base = BaseDirs::new().ok_or_else(|| AppError::Internal("无法定位用户主目录".into()))?;
    Ok(base.config_dir().join("Wiretoss"))
}

pub fn config_path() -> AppResult<PathBuf> {
    Ok(config_dir()?.join("config.json"))
}

/// HTTPS 的 CA / 服务器证书目录。证书和私钥属于敏感文件，不要提交，
/// 也不要在阶段清理时删除（见 AGENTS.md 的红线）。
pub fn certs_dir() -> AppResult<PathBuf> {
    Ok(config_dir()?.join("certs"))
}

/// 默认共享目录：`~/Wiretoss`。两端保持同名，用户心智负担最小。
pub fn default_root() -> AppResult<PathBuf> {
    let base = BaseDirs::new().ok_or_else(|| AppError::Internal("无法定位用户主目录".into()))?;
    Ok(base.home_dir().join("Wiretoss"))
}

/// 客户端默认接收目录：下载目录下的 `Wiretoss`。
/// 与共享目录 `~/Wiretoss` 分开，避免同一台机器同时跑两种模式时互相覆盖。
pub fn default_receive_dir() -> PathBuf {
    if let Some(dirs) = UserDirs::new() {
        if let Some(downloads) = dirs.download_dir() {
            return downloads.join("Wiretoss");
        }
    }
    if let Some(base) = BaseDirs::new() {
        return base.home_dir().join("Downloads").join("Wiretoss");
    }
    std::env::temp_dir().join("Wiretoss")
}

/// 配置文件里有口令哈希，权限必须收紧到仅本人可读。
/// Windows 没有等价的 unix mode，依赖 `%APPDATA%` 本身的用户隔离。
#[cfg(unix)]
fn restrict_permissions(path: &Path) -> AppResult<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    Ok(())
}

#[cfg(not(unix))]
fn restrict_permissions(_path: &Path) -> AppResult<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn password_roundtrip() {
        let hash = hash_password("correct horse battery staple").unwrap();
        let cfg = Config {
            version: CONFIG_VERSION,
            username: "u".into(),
            password_hash: hash,
            root: PathBuf::from("/tmp"),
            port: 8080,
            session_hours: 12,
            client: ClientSettings::default(),
            start_mode: StartMode::Server,
            allow_guest: false,
            https: false,
            https_port: DEFAULT_HTTPS_PORT,
        };
        assert!(cfg.verify_password("correct horse battery staple"));
        assert!(!cfg.verify_password("wrong"));
        assert!(!cfg.verify_password(""));
    }

    #[test]
    fn same_password_gets_different_hash() {
        // 盐必须每次不同，否则两个相同口令会产生相同哈希。
        let a = hash_password("same").unwrap();
        let b = hash_password("same").unwrap();
        assert_ne!(a, b);
    }

    #[test]
    fn empty_password_hash_rejects_every_login() {
        // 「首次运行、还没设密码」这个状态的兜底：空 password_hash 解析不出
        // 合法 PHC 串，任何口令都必须验不过。要是不成立，程序就会开出一个
        // 谁都能进的服务。
        let cfg = Config {
            version: CONFIG_VERSION,
            username: "u".into(),
            password_hash: String::new(),
            root: PathBuf::from("/tmp"),
            port: 8080,
            session_hours: 12,
            client: ClientSettings::default(),
            start_mode: StartMode::Server,
            allow_guest: false,
            https: false,
            https_port: DEFAULT_HTTPS_PORT,
        };
        assert!(!cfg.verify_password(""));
        assert!(!cfg.verify_password("anything"));
        assert!(!cfg.verify_password("u"));
    }

    #[test]
    fn generated_passwords_avoid_ambiguous_chars() {
        let pw = generate_password(64);
        assert_eq!(pw.len(), 64);
        for c in pw.chars() {
            assert!(!"01lIO".contains(c), "密码里不该出现易混淆字符 {c}");
        }
    }

    #[test]
    fn generated_passwords_differ() {
        assert_ne!(generate_password(16), generate_password(16));
    }

    #[test]
    fn old_config_without_client_fields_still_loads() {
        // 升级前写下的 config.json 没有 client/start_mode。缺字段必须走默认值，
        // 否则老用户升级后程序会直接报「配置文件损坏」。
        let json = r#"{
            "version": 1,
            "username": "u",
            "password_hash": "",
            "root": "/tmp",
            "port": 8080,
            "session_hours": 12
        }"#;
        let cfg: Config = serde_json::from_str(json).unwrap();
        assert!(cfg.client.client_id.is_empty());
        assert_eq!(cfg.start_mode, StartMode::Server);
    }

    #[test]
    fn client_identity_is_generated_once() {
        let mut client = ClientSettings::default();
        assert!(client.ensure_identity());
        let id = client.client_id.clone();
        let name = client.display_name.clone();
        assert!(valid_client_id(&id) && !name.is_empty());
        // 第二次调用不应再改任何东西。
        assert!(!client.ensure_identity());
        assert_eq!(client.client_id, id);
        assert_eq!(client.display_name, name);
    }

    #[test]
    fn invalid_client_id_is_replaced() {
        let mut client = ClientSettings {
            client_id: "bad id".into(),
            ..ClientSettings::default()
        };
        assert!(client.ensure_identity());
        assert!(valid_client_id(&client.client_id));
    }

    #[test]
    fn default_receive_dir_is_absolute_and_not_shared_root() {
        let dir = default_receive_dir();
        assert!(
            dir.is_absolute(),
            "接收目录必须是绝对路径：{}",
            dir.display()
        );
        assert!(dir.ends_with("Wiretoss"));
    }
}
