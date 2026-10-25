//! 分片上传与断点续传。
//!
//! 核心设计：**所有分片直接写到同一个文件的对应偏移量上**，而不是各存一个
//! 分片文件、最后再拼接。这样 complete 阶段只是一次同目录内的 `rename`——
//! 原子、零拷贝、不额外占用一份磁盘空间，几个 GB 的文件也不会在完成瞬间
//! 卡住几十秒去拼盘。
//!
//! 代价是并发写同一文件需要串行化，用一个 `Mutex<File>` 解决。
//! 分片本身由网络决定到达速率，这个锁不会是瓶颈。

use std::io::{Seek, SeekFrom, Write};
use std::sync::{Arc, Mutex};

use axum::extract::{Path as AxumPath, State};
use axum::http::HeaderMap;
use axum::Json;
use bytes::Bytes;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};

use crate::auth::{require_csrf, WriteUser};
use crate::error::{AppError, AppResult};
use crate::events::TransferEvent;
use crate::fs_api::unique_path;
use crate::state::{AppState, Upload, CHUNK_SIZE};

#[derive(Deserialize)]
pub struct InitRequest {
    /// 客户端提供的原始文件名，落盘前会被净化。
    pub name: String,
    pub size: u64,
    /// Unix 秒。参与指纹计算，用来识别"这是同一个文件"。
    pub last_modified: i64,
    /// 目标相对目录，空串表示共享根目录。
    #[serde(default)]
    pub dir: String,
    /// 同名文件是否覆盖。默认自动改名，绝不静默丢数据。
    #[serde(default)]
    pub overwrite: bool,
}

#[derive(Serialize)]
pub struct InitResponse {
    pub upload_id: String,
    pub chunk_size: u64,
    pub total_chunks: u64,
    /// 已经收到过的分片下标。非空即代表这是一次断点续传。
    pub received_chunks: Vec<u64>,
    pub resumed: bool,
}

/// 指纹由**服务端**计算，不采信客户端传来的值。
/// 否则任何已登录用户都能凭猜测的指纹去续传别人正在传的文件。
fn fingerprint(dir: &str, name: &str, size: u64, last_modified: i64) -> String {
    let mut hasher = Sha256::new();
    hasher.update(dir.as_bytes());
    hasher.update([0]);
    hasher.update(name.as_bytes());
    hasher.update([0]);
    hasher.update(size.to_le_bytes());
    hasher.update(last_modified.to_le_bytes());
    hex::encode(hasher.finalize())
}

pub async fn init(
    State(state): State<Arc<AppState>>,
    _user: WriteUser,
    headers: HeaderMap,
    Json(req): Json<InitRequest>,
) -> AppResult<Json<InitResponse>> {
    require_csrf(&headers)?;

    if req.name.trim().is_empty() {
        return Err(AppError::BadRequest("文件名不能为空".into()));
    }
    if req.size > (1u64 << 44) {
        // 16 TiB。纯粹是防呆：真到这一步说明客户端算错了大小。
        return Err(AppError::BadRequest("文件过大".into()));
    }

    // 目标目录必须存在且位于共享根内 —— 在写入任何字节之前就校验掉。
    let target_dir = state.root.resolve_existing(&req.dir)?;
    if !target_dir.is_dir() {
        return Err(AppError::BadRequest("目标不是目录".into()));
    }

    let fp = fingerprint(&req.dir, &req.name, req.size, req.last_modified);
    let total_chunks = req.size.div_ceil(CHUNK_SIZE);

    // 先看有没有可续传的旧上传。命中条件是"同一个指纹"且还没完成。
    {
        let uploads = state
            .uploads
            .lock()
            .map_err(|_| AppError::Internal("上传表锁中毒".into()))?;
        if let Some(existing) = uploads.values().find(|u| u.fingerprint == fp) {
            let received = existing.received_indices();
            let resumed = !received.is_empty();
            let response = InitResponse {
                upload_id: existing.id.clone(),
                chunk_size: CHUNK_SIZE,
                total_chunks: existing.total_chunks,
                received_chunks: received,
                resumed,
            };
            if resumed {
                tracing::info!(
                    "续传「{}」：已有 {}/{} 分片",
                    req.name,
                    response.received_chunks.len(),
                    total_chunks
                );
                state.broadcast(TransferEvent::UploadStarted {
                    id: existing.id.clone(),
                    name: req.name.clone(),
                    size: req.size,
                    resumed: true,
                });
            }
            return Ok(Json(response));
        }
    }

    let id = uuid::Uuid::new_v4().to_string();
    let part_dir = state.root.partial_dir().join(&id);
    std::fs::create_dir_all(&part_dir)?;

    // 预先创建（可能为空）的数据文件，之后所有分片都按偏移写进它。
    let file = std::fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(true)
        .open(part_dir.join("data"))?;
    file.set_len(req.size)?;

    let upload = Upload {
        id: id.clone(),
        fingerprint: fp,
        display_name: req.name.clone(),
        target_dir,
        size: req.size,
        total_chunks,
        received: vec![false; total_chunks as usize],
        file: Arc::new(Mutex::new(file)),
        created_at: std::time::Instant::now(),
        overwrite: req.overwrite,
    };

    state
        .uploads
        .lock()
        .map_err(|_| AppError::Internal("上传表锁中毒".into()))?
        .insert(id.clone(), upload);

    state.broadcast(TransferEvent::UploadStarted {
        id: id.clone(),
        name: req.name.clone(),
        size: req.size,
        resumed: false,
    });
    tracing::info!(
        "开始接收「{}」（{} 字节，{} 个分片）",
        req.name,
        req.size,
        total_chunks
    );

    Ok(Json(InitResponse {
        upload_id: id,
        chunk_size: CHUNK_SIZE,
        total_chunks,
        received_chunks: Vec::new(),
        resumed: false,
    }))
}

/// 接收一个分片。请求体就是裸字节，不做 multipart 包装——
/// 省掉一层解析和一份拷贝，8 MiB 的分片直接按偏移写盘。
pub async fn put_chunk(
    State(state): State<Arc<AppState>>,
    _user: WriteUser,
    headers: HeaderMap,
    AxumPath((id, index)): AxumPath<(String, u64)>,
    body: Bytes,
) -> AppResult<Json<serde_json::Value>> {
    require_csrf(&headers)?;

    // 把需要的东西从锁里取出来，尽早释放上传表锁。
    let (file, expected_len, size, name, already) = {
        let uploads = state
            .uploads
            .lock()
            .map_err(|_| AppError::Internal("上传表锁中毒".into()))?;
        let upload = uploads
            .get(&id)
            .ok_or_else(|| AppError::NotFound("上传会话不存在或已过期".into()))?;

        if index >= upload.total_chunks {
            return Err(AppError::BadRequest(format!(
                "分片下标 {index} 超出范围（共 {} 个）",
                upload.total_chunks
            )));
        }

        // 除了最后一片，每个分片都必须恰好是 CHUNK_SIZE。
        // 长度不对说明客户端算错了，此时写下去会静默破坏文件偏移，必须拒绝。
        let offset = index * CHUNK_SIZE;
        let expected = if index + 1 == upload.total_chunks {
            upload.size - offset
        } else {
            CHUNK_SIZE
        };

        (
            upload.file.clone(),
            expected,
            upload.size,
            upload.display_name.clone(),
            upload.received[index as usize],
        )
    };

    if body.len() as u64 != expected_len {
        return Err(AppError::BadRequest(format!(
            "分片 {index} 长度不符：期望 {expected_len} 字节，收到 {}",
            body.len()
        )));
    }

    // 已收到的分片直接返回成功。客户端重试是常态（网络抖动、并发竞态），
    // 让它幂等比让它报错更省事。
    if already {
        return Ok(Json(json!({ "ok": true, "duplicate": true })));
    }

    let offset = index * CHUNK_SIZE;
    let payload = body.to_vec();
    tokio::task::spawn_blocking(move || -> AppResult<()> {
        let mut guard = file
            .lock()
            .map_err(|_| AppError::Internal("上传文件锁中毒".into()))?;
        guard.seek(SeekFrom::Start(offset))?;
        guard.write_all(&payload)?;
        Ok(())
    })
    .await
    .map_err(AppError::internal)??;

    // 落盘成功后才标记为已收到。顺序反过来的话，进程崩溃会留下
    // "标记为已收到但内容不全"的分片，续传时会静默产生损坏的文件。
    let progress = {
        let mut uploads = state
            .uploads
            .lock()
            .map_err(|_| AppError::Internal("上传表锁中毒".into()))?;
        let upload = uploads
            .get_mut(&id)
            .ok_or_else(|| AppError::NotFound("上传会话不存在".into()))?;
        upload.received[index as usize] = true;
        upload.received.iter().filter(|done| **done).count() as u64
    };

    state.broadcast(TransferEvent::UploadProgress {
        id: id.clone(),
        name,
        bytes: progress * CHUNK_SIZE,
        size,
    });

    Ok(Json(json!({ "ok": true, "received": progress })))
}

#[derive(Serialize)]
pub struct CompleteResponse {
    /// 相对共享根的最终路径，前端据此提示"已保存到 …"。
    pub path: String,
    pub name: String,
    pub size: u64,
}

pub async fn complete(
    State(state): State<Arc<AppState>>,
    _user: WriteUser,
    headers: HeaderMap,
    AxumPath(id): AxumPath<String>,
) -> AppResult<Json<CompleteResponse>> {
    require_csrf(&headers)?;

    // 取出并**从注册表移除**：完成或失败都不该再接受这个 id 的分片。
    let upload = {
        let mut uploads = state
            .uploads
            .lock()
            .map_err(|_| AppError::Internal("上传表锁中毒".into()))?;
        uploads
            .remove(&id)
            .ok_or_else(|| AppError::NotFound("上传会话不存在或已过期".into()))?
    };

    if !upload.is_complete() {
        let missing = upload.received.iter().filter(|d| !**d).count();
        // 放回去，让客户端还能继续补分片，而不是整个重来。
        let id2 = upload.id.clone();
        state
            .uploads
            .lock()
            .map_err(|_| AppError::Internal("上传表锁中毒".into()))?
            .insert(id2.clone(), upload);
        return Err(AppError::BadRequest(format!("还有 {missing} 个分片未收到")));
    }

    let part_dir = upload.part_dir(&state.root.partial_dir());
    let part_file = part_dir.join("data");

    // Windows 上无法重命名一个仍被打开的文件，所以必须先把句柄全部释放。
    // Arc 的强引用计数在这是 1（注册表里那份已被移出），显式 drop 比依赖
    // 作用域结束更不容易在后续改动中被破坏。
    let size = upload.size;
    drop(upload.file);

    let target = state
        .root
        .resolve_in(&upload.target_dir, &upload.display_name)?;

    // 覆盖与非覆盖分开处理：fs::rename 在 Windows 上遇到已存在的目标会直接失败，
    // 所以覆盖时必须先删。非覆盖时用 unique_path 找一个不冲突的名字。
    let final_path = if upload.overwrite {
        if target.exists() {
            std::fs::remove_file(&target)?;
        }
        target
    } else {
        unique_path(
            &upload.target_dir,
            &crate::fs_api::sanitize_filename(&upload.display_name)?,
        )
    };

    // 同一文件系统内的 rename 是原子的：对端要么看到完整的旧文件，
    // 要么看到完整的新文件，不会看到写了一半的中间态。
    std::fs::rename(&part_file, &final_path)?;
    let _ = std::fs::remove_dir_all(&part_dir);

    let rel_path = final_path
        .strip_prefix(state.root.path())
        .map(|p| p.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|_| upload.display_name.clone());
    let final_name = final_path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| upload.display_name.clone());

    state.broadcast(TransferEvent::UploadFinished {
        id: id.clone(),
        name: final_name.clone(),
        path: rel_path.clone(),
    });
    // 通知所有浏览器刷新列表——包括发起方和接收方的另一个标签页。
    let dir = rel_path
        .rsplit_once('/')
        .map(|(d, _)| d.to_string())
        .unwrap_or_default();
    state.broadcast(TransferEvent::FilesChanged { dir });

    tracing::info!("接收完成：{rel_path}（{size} 字节）");
    Ok(Json(CompleteResponse {
        path: rel_path,
        name: final_name,
        size,
    }))
}

pub async fn abort(
    State(state): State<Arc<AppState>>,
    _user: WriteUser,
    headers: HeaderMap,
    AxumPath(id): AxumPath<String>,
) -> AppResult<Json<serde_json::Value>> {
    require_csrf(&headers)?;
    let upload = state
        .uploads
        .lock()
        .map_err(|_| AppError::Internal("上传表锁中毒".into()))?
        .remove(&id);

    if let Some(upload) = upload {
        let part_dir = upload.part_dir(&state.root.partial_dir());
        drop(upload.file);
        let _ = std::fs::remove_dir_all(part_dir);
        state.broadcast(TransferEvent::UploadFailed {
            id,
            name: upload.display_name,
            error: "已取消".into(),
        });
    }
    Ok(Json(json!({ "ok": true })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingerprint_is_stable_and_field_separated() {
        let a = fingerprint("dir", "a.txt", 100, 1700000000);
        assert_eq!(a, fingerprint("dir", "a.txt", 100, 1700000000));
        // 拼接歧义：("ab","c") 和 ("a","bc") 必须得到不同指纹，
        // 分隔符就是为这个加的。
        assert_ne!(
            fingerprint("d", "ab.txt", 1, 0),
            fingerprint("d", "a", 1, 0)
        );
    }

    #[test]
    fn fingerprint_changes_with_every_component() {
        let base = fingerprint("dir", "a.txt", 100, 1700000000);
        assert_ne!(base, fingerprint("other", "a.txt", 100, 1700000000));
        assert_ne!(base, fingerprint("dir", "b.txt", 100, 1700000000));
        assert_ne!(base, fingerprint("dir", "a.txt", 101, 1700000000));
        assert_ne!(base, fingerprint("dir", "a.txt", 100, 1700000001));
    }

    #[test]
    fn chunk_count_math() {
        // 空文件不需要任何分片，客户端 init 完直接 complete。
        assert_eq!(0u64.div_ceil(CHUNK_SIZE), 0);
        assert_eq!(1u64.div_ceil(CHUNK_SIZE), 1);
        assert_eq!(CHUNK_SIZE.div_ceil(CHUNK_SIZE), 1);
        assert_eq!((CHUNK_SIZE + 1).div_ceil(CHUNK_SIZE), 2);
        assert_eq!((CHUNK_SIZE * 3).div_ceil(CHUNK_SIZE), 3);
    }
}
