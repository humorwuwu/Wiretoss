use std::collections::HashMap;
use std::net::IpAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};
use std::time::Instant;

use tokio::sync::{broadcast, watch};

use crate::clients::{ClientInfo, ConnectedClient};
use crate::config::Config;
use crate::error::AppResult;
use crate::events::TransferEvent;
use crate::fs_api::SafeRoot;
use crate::push::PushBatch;

/// 分片大小。8 MiB 是个折中：分片太小会让局域网上的请求数爆炸
/// （每个分片都是一次完整的 HTTP 往返），太大则断点续传的粒度变粗、
/// 单个分片失败要重传的代价变高。
pub const CHUNK_SIZE: u64 = 8 * 1024 * 1024;

/// 未完成上传的保留时长。超过这个时长的上传会话会被后台任务清掉。
/// 定得比较长是故意的：用户可能明天才想起来继续传那个没传完的文件夹。
pub const UPLOAD_TTL_SECS: u64 = 24 * 60 * 60;

/// 打包下载券的有效期。券是一次性的，这个时长只是兜住"建了券但浏览器没去下载"的情况。
pub const ZIP_TICKET_TTL_SECS: u64 = 300;

/// 投送批次多久没有活动就回收。单个大文件可能传很久，所以不能只看创建时间。
pub const PUSH_IDLE_TTL_SECS: u64 = 2 * 60 * 60;

/// 投送批次的硬上限，防止长时间挂着的批次引用早已变化的源文件。
pub const PUSH_MAX_AGE_SECS: u64 = 24 * 60 * 60;

pub struct Session {
    pub username: String,
    pub expires_at: Instant,
}

/// 一次进行中的分片上传。
pub struct Upload {
    pub id: String,
    /// `sha256(文件名 + 大小 + mtime)`。刷新页面后前端拿同一个指纹来 init，
    /// 命中就能拿回 `received` 继续传，这是断点续传的全部机关。
    pub fingerprint: String,
    /// 前端传来的原始文件名（用于展示和最终命名）。
    pub display_name: String,
    /// 已解析并校验过的目标目录绝对路径。
    pub target_dir: PathBuf,
    pub size: u64,
    pub total_chunks: u64,
    /// 每个分片是否已落盘。用 Vec 而不是 HashSet，因为要按索引直接查。
    pub received: Vec<bool>,
    /// 所有分片共用的同一个文件句柄，按偏移量写入。
    /// 之所以要锁：多个分片并发到达时，`seek` + `write` 不是原子操作，
    /// 不加锁会互相踩掉对方的文件位置。
    pub file: Arc<Mutex<std::fs::File>>,
    pub overwrite: bool,
    pub created_at: Instant,
}

impl Upload {
    pub fn received_indices(&self) -> Vec<u64> {
        self.received
            .iter()
            .enumerate()
            .filter(|(_, done)| **done)
            .map(|(i, _)| i as u64)
            .collect()
    }

    pub fn is_complete(&self) -> bool {
        self.received.iter().all(|done| *done)
    }

    /// 分片目录：`{root}/.wiretoss/partial/{upload_id}`
    pub fn part_dir(&self, partial_root: &std::path::Path) -> PathBuf {
        partial_root.join(&self.id)
    }
}

/// 登录失败计数。局域网内暴力破解的带宽成本极低，所以必须限流。
#[derive(Default)]
pub struct LoginAttempts {
    pub failures: u32,
    pub blocked_until: Option<Instant>,
}

impl LoginAttempts {
    /// 递增退避：失败 5 次后开始锁，锁定时长随失败次数翻倍，上限 5 分钟。
    /// 用退避而不是永久封禁，是因为局域网里 IP 可能被 DHCP 复用，
    /// 永久封禁会误伤无辜的下一个租户。
    pub fn register_failure(&mut self) {
        self.failures = self.failures.saturating_add(1);
        if self.failures >= 5 {
            let backoff = 1u64 << (self.failures - 5).min(8);
            let secs = backoff.min(300);
            self.blocked_until = Some(Instant::now() + std::time::Duration::from_secs(secs));
        }
    }

    pub fn blocked_for(&self) -> Option<u64> {
        let until = self.blocked_until?;
        let now = Instant::now();
        if until > now {
            Some((until - now).as_secs() + 1)
        } else {
            None
        }
    }
}

/// 一次打包下载的凭据。存的是**已解析好的**绝对路径列表，
/// 这样流式发送开始后就再也不需要碰文件系统之外的任何校验了。
pub struct ZipTicket {
    pub entries: Vec<(String, PathBuf)>,
    /// 下载文件名，建券时算好。流式发送时再算的话，只能从展开后的条目名反推，
    /// 得到的是「包内第一个文件.zip」而不是用户选的那个目录名。
    pub name: String,
    pub created_at: Instant,
}

pub struct AppState {
    /// 密码可能被改，所以放 RwLock。读多写极少。
    pub config: RwLock<Config>,
    pub root: SafeRoot,
    /// 服务端会话表。比签名 Cookie 多花一个 HashMap，换来的是登出即失效、
    /// 无需管理签名密钥。
    pub sessions: Mutex<HashMap<String, Session>>,
    pub uploads: Mutex<HashMap<String, Upload>>,
    pub zip_tickets: Mutex<HashMap<String, ZipTicket>>,
    pub login_guard: Mutex<HashMap<IpAddr, LoginAttempts>>,
    /// SSE 广播。没有订阅者时 send 会失败，属正常情况，忽略即可。
    pub events: broadcast::Sender<TransferEvent>,
    /// 已连接的原生客户端（设备 id -> 连接）。浏览器不会出现在这里。
    pub clients: Mutex<HashMap<String, ConnectedClient>>,
    /// 在线客户端快照，只给 GUI 用。刻意不走浏览器 SSE：
    /// 设备名和 IP 没必要暴露给所有网页。
    pub client_list: watch::Sender<Vec<ClientInfo>>,
    /// 待客户端拉取的投送批次。
    pub pushes: Mutex<HashMap<String, PushBatch>>,
}

impl AppState {
    pub fn new(config: Config) -> AppResult<Arc<Self>> {
        let root = SafeRoot::new(&config.root)?;
        std::fs::create_dir_all(root.partial_dir())?;
        let (events, _) = broadcast::channel(256);
        let (client_list, _) = watch::channel(Vec::new());

        let state = Arc::new(Self {
            config: RwLock::new(config),
            root,
            sessions: Mutex::new(HashMap::new()),
            uploads: Mutex::new(HashMap::new()),
            zip_tickets: Mutex::new(HashMap::new()),
            login_guard: Mutex::new(HashMap::new()),
            events,
            clients: Mutex::new(HashMap::new()),
            client_list,
            pushes: Mutex::new(HashMap::new()),
        });

        state.cleanup_stale_partials();
        Ok(state)
    }

    /// 清理上次运行留下的中转分片。
    ///
    /// 刻意在启动时做而不是在运行中做：运行中删除会打断正在进行的上传，
    /// 而中转目录在共享根之下，启动时它是空的或全是上一次的残骸。
    fn cleanup_stale_partials(&self) {
        let dir = self.root.partial_dir();
        let Ok(read) = std::fs::read_dir(&dir) else {
            return;
        };
        let mut removed = 0usize;
        for entry in read.flatten() {
            if std::fs::remove_dir_all(entry.path()).is_ok() {
                removed += 1;
            }
        }
        if removed > 0 {
            tracing::info!("清理了 {removed} 个上次遗留的未完成上传");
        }
    }

    pub fn broadcast(&self, event: TransferEvent) {
        // 没有浏览器订阅时返回 Err，这不是问题。
        let _ = self.events.send(event);
    }

    /// 登录成功后清理过期会话，避免长跑进程里 sessions 无限增长。
    pub fn purge_expired_sessions(&self) {
        let now = Instant::now();
        if let Ok(mut sessions) = self.sessions.lock() {
            sessions.retain(|_, s| s.expires_at > now);
        }
    }

    /// 定期清理三张会无限增长的表。
    ///
    /// 上传表尤其重要：客户端传了一半就关掉浏览器的话，那个会话会永远留在表里，
    /// 连带它占用的中转目录也永远不释放。上传是**长期驻留**的场景
    /// （用户可能开着好几天），所以这个清理必须在运行中做，不能只在启动时做。
    pub fn purge_stale(&self) {
        let now = Instant::now();

        if let Ok(mut sessions) = self.sessions.lock() {
            sessions.retain(|_, s| s.expires_at > now);
        }

        if let Ok(mut tickets) = self.zip_tickets.lock() {
            tickets.retain(|_, t| t.created_at.elapsed().as_secs() < ZIP_TICKET_TTL_SECS);
        }

        // 投送批次：先回收，再广播。锁内只做 HashMap 操作，广播放到锁外。
        let expired_pushes: Vec<PushBatch> = match self.pushes.lock() {
            Ok(mut pushes) => {
                let expired: Vec<String> = pushes
                    .iter()
                    .filter(|(_, b)| {
                        b.created_at.elapsed().as_secs() > PUSH_MAX_AGE_SECS
                            || b.last_activity.elapsed().as_secs() > PUSH_IDLE_TTL_SECS
                    })
                    .map(|(id, _)| id.clone())
                    .collect();
                expired
                    .into_iter()
                    .filter_map(|id| pushes.remove(&id))
                    .collect()
            }
            Err(_) => Vec::new(),
        };
        for batch in expired_pushes {
            tracing::info!("回收超时未完成的投送：{}", batch.name);
            self.broadcast(TransferEvent::PushFailed {
                batch_id: batch.id,
                error: "超时未完成".into(),
                done: batch.done,
            });
        }

        let Ok(mut uploads) = self.uploads.lock() else {
            return;
        };
        let stale: Vec<String> = uploads
            .iter()
            .filter(|(_, u)| u.created_at.elapsed().as_secs() > UPLOAD_TTL_SECS)
            .map(|(id, _)| id.clone())
            .collect();

        for id in stale {
            let Some(upload) = uploads.remove(&id) else {
                continue;
            };
            // 先放掉文件句柄再删目录：Windows 上打开着的文件删不掉。
            let name = upload.display_name.clone();
            drop(upload.file);
            let _ = std::fs::remove_dir_all(self.root.partial_dir().join(&id));
            tracing::info!("清理超过 24 小时未完成的上传：{name}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_starts_after_five_failures() {
        let mut a = LoginAttempts::default();
        for _ in 0..4 {
            a.register_failure();
        }
        assert!(a.blocked_for().is_none(), "4 次失败还不该锁");

        a.register_failure();
        let wait = a.blocked_for().expect("第 5 次失败应该触发封锁");
        assert!(wait > 0 && wait <= 2, "首次封锁应为 1 秒左右，实际 {wait}");
    }

    #[test]
    fn backoff_is_capped_at_five_minutes() {
        let mut a = LoginAttempts::default();
        for _ in 0..40 {
            a.register_failure();
        }
        let wait = a.blocked_for().unwrap();
        assert!(wait <= 301, "封锁时长必须有上限，实际 {wait}");
    }
}
