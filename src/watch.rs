//! 共享目录的外部变化 -> SSE 刷新事件。
//!
//! 通过 API 上传、删除、新建时各 handler 会主动广播 `FilesChanged`；但用户也可能
//! 在 Finder / 资源管理器里直接往共享目录拖文件，或由别的程序写入。没有监视器时，
//! 已经打开的网页只能手动刷新才能看到这些变化。
//!
//! 这里用 notify 递归监视共享根，并把 350ms 内的事件合并：一次复制会产生大量
//! 底层事件，不合并会把 SSE 广播塞满，浏览器也会疯狂重绘。

use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use notify::{Event, EventKind, RecursiveMode, Watcher};
use tokio::task::JoinHandle;

use crate::events::TransferEvent;
use crate::state::AppState;

/// 事件合并窗口。太短会把一次复制拆成几十次刷新，太长会让界面显得迟钝。
const DEBOUNCE: Duration = Duration::from_millis(350);

/// 上传中转目录，位于共享根下；它的变化不应该触发网页刷新。
const STAGING_DIR: &str = ".wiretoss";

/// 启动监视器。返回的任务句柄给 GUI 停止服务时 abort 用。
///
/// 初始化失败只记日志：手动刷新仍然可用，不应该因此拒绝启动服务。
pub fn spawn(state: Arc<AppState>) -> Option<JoinHandle<()>> {
    let root = state.root.path().to_path_buf();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Event>();

    let mut watcher = match notify::recommended_watcher(move |result| {
        if let Ok(event) = result {
            let _ = tx.send(event);
        }
    }) {
        Ok(watcher) => watcher,
        Err(error) => {
            tracing::warn!("文件监视器初始化失败，网页将不会自动刷新：{error}");
            return None;
        }
    };

    if let Err(error) = watcher.watch(&root, RecursiveMode::Recursive) {
        tracing::warn!(
            "无法监视共享目录 {}，网页将不会自动刷新：{error}",
            root.display()
        );
        return None;
    }

    tracing::info!("开始监视共享目录：{}", root.display());
    let task = tokio::spawn(async move {
        // watcher 必须活到任务结束，否则监视会立刻停止。
        let _watcher = watcher;
        let mut pending: HashSet<String> = HashSet::new();
        let mut ticker = tokio::time::interval(DEBOUNCE);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        // interval 的第一次 tick 立刻返回，先吃掉，避免启动时一次空刷新。
        ticker.tick().await;

        loop {
            tokio::select! {
                maybe_event = rx.recv() => {
                    match maybe_event {
                        Some(event) => collect_dirs(&root, &event, &mut pending),
                        None => break,
                    }
                }
                _ = ticker.tick() => {
                    for dir in pending.drain() {
                        state.broadcast(TransferEvent::FilesChanged { dir });
                    }
                }
            }
        }

        for dir in pending.drain() {
            state.broadcast(TransferEvent::FilesChanged { dir });
        }
    });

    Some(task)
}

fn collect_dirs(root: &Path, event: &Event, out: &mut HashSet<String>) {
    // 打开/读取文件也会产生 Access 事件；它们不改变目录列表，不能触发刷新。
    if matches!(event.kind, EventKind::Access(_)) {
        return;
    }

    for path in &event.paths {
        let Ok(relative) = path.strip_prefix(root) else {
            continue;
        };
        if relative.as_os_str().is_empty() {
            continue;
        }

        // 上传分片的临时写入不能让网页刷新。
        if relative
            .components()
            .next()
            .is_some_and(|part| part.as_os_str() == STAGING_DIR)
        {
            continue;
        }

        // 父目录一定受影响：新增、删除、改名都在这里体现。
        if let Some(parent) = relative.parent() {
            out.insert(relative_to_string(parent));
        }

        // macOS 的 FSEvents 有时只报“这个目录变化了”，路径本身就是目录。
        // 用户正停在这个目录里时，也要刷新目录自身。
        if path.is_dir() {
            out.insert(relative_to_string(relative));
        }
    }
}

fn relative_to_string(path: &Path) -> String {
    // 服务端 API 的相对路径统一用 '/'；Windows 的 Path 默认是 '\'。
    path.to_string_lossy().replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use super::*;
    use notify::event::CreateKind;

    fn dirs_for(root: &Path, path: &Path) -> Vec<String> {
        let event = Event {
            kind: EventKind::Create(CreateKind::File),
            paths: vec![path.to_path_buf()],
            attrs: Default::default(),
        };
        let mut out = HashSet::new();
        collect_dirs(root, &event, &mut out);
        let mut dirs: Vec<String> = out.into_iter().collect();
        dirs.sort();
        dirs
    }

    #[test]
    fn maps_paths_to_parent_directories() {
        let root = Path::new("/tmp/wiretoss-watch-root");
        assert_eq!(
            dirs_for(root, Path::new("/tmp/wiretoss-watch-root/a.txt")),
            vec![""]
        );
        assert_eq!(
            dirs_for(root, Path::new("/tmp/wiretoss-watch-root/照片/2024/a.jpg")),
            vec!["照片/2024"]
        );
    }

    #[test]
    fn ignores_staging_and_outside_paths() {
        let root = Path::new("/tmp/wiretoss-watch-root");
        assert!(dirs_for(
            root,
            Path::new("/tmp/wiretoss-watch-root/.wiretoss/partial/chunk")
        )
        .is_empty());
        assert!(dirs_for(root, Path::new("/tmp/other/a.txt")).is_empty());
    }
}
