//! 服务端发起的「投送」：把本地任意路径登记成一个批次，由指定客户端拉取。
//!
//! 安全边界和上传/下载相反：这里的源路径是**本地 GUI 拖进来的**，可以是共享
//! 目录之外的文件；HTTP 请求永远只能凭 batch UUID 和一个数组下标取文件，
//! 不能自己提交路径。因此 `SafeRoot` 仍然是浏览器可触达的唯一文件边界。

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use axum::extract::{Path as AxumPath, Request, State};
use axum::http::header::CACHE_CONTROL;
use axum::http::{HeaderMap, HeaderValue};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tower::ServiceExt;
use tower_http::services::ServeFile;
use unicode_normalization::UnicodeNormalization;

use crate::auth::{require_csrf, WriteUser};
use crate::clients::{parse_client_id, send_notice, PushNotice, SendError};
use crate::error::{AppError, AppResult};
use crate::events::TransferEvent;
use crate::fs_api::{is_junk, sanitize_filename, WINDOWS_RESERVED};
use crate::state::AppState;

pub const MAX_ENTRIES: usize = 20_000;
pub const MAX_DEPTH: usize = 64;
pub const MAX_COMPONENT_CHARS: usize = 200;
pub const MAX_TOTAL_BYTES: u64 = 1 << 40; // 1 TiB

#[derive(Debug, Clone)]
pub struct PushEntry {
    /// 接收端相对路径，`/` 分隔、NFC、逐组件净化过。
    pub dest: String,
    /// 服务端绝对路径。只有本地 GUI 能创建批次，所以它可以位于共享根之外。
    pub source: PathBuf,
    pub is_dir: bool,
    pub size: u64,
}

#[derive(Debug)]
pub struct PushBatch {
    pub id: String,
    pub client_id: String,
    pub client_name: String,
    /// 展示名：`照片` / `照片 等 3 项`。
    pub name: String,
    pub entries: Vec<PushEntry>,
    pub total_files: u64,
    pub total_bytes: u64,
    pub done: u64,
    pub created_at: Instant,
    pub last_activity: Instant,
}

#[derive(Debug, Clone, Copy)]
pub struct PushLimits {
    pub max_entries: usize,
    pub max_depth: usize,
    pub max_total_bytes: u64,
}

impl Default for PushLimits {
    fn default() -> Self {
        Self {
            max_entries: MAX_ENTRIES,
            max_depth: MAX_DEPTH,
            max_total_bytes: MAX_TOTAL_BYTES,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestItem {
    /// 文件接口用的下标。客户端**必须**用它，不能自己数文件。
    pub index: usize,
    pub path: String,
    pub is_dir: bool,
    pub size: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestResponse {
    pub batch_id: String,
    pub name: String,
    pub total_files: u64,
    pub total_bytes: u64,
    pub items: Vec<ManifestItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingBatch {
    pub batch_id: String,
    pub name: String,
    pub total_files: u64,
    pub total_bytes: u64,
    pub age_secs: u64,
}

/// 把任意文件系统名字变成可以安全落到 macOS/Windows 的单个路径组件。
pub fn sanitize_push_component(raw: &str) -> String {
    let nfc: String = raw.nfc().collect();
    let cleaned = sanitize_filename(&nfc).unwrap_or_else(|_| "_".to_string());
    if cleaned.chars().count() <= MAX_COMPONENT_CHARS {
        cleaned
    } else {
        cleaned.chars().take(MAX_COMPONENT_CHARS).collect()
    }
}

/// 第 n 个候选名。目录保留完整名字（`照片 (1)`），文件保留扩展名
/// （`a.txt` -> `a (1).txt`）。
pub fn numbered_name(name: &str, is_dir: bool, n: u32) -> String {
    if is_dir {
        return format!("{name} ({n})");
    }
    let path = Path::new(name);
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    match path.extension() {
        Some(ext) => format!("{stem} ({n}).{}", ext.to_string_lossy()),
        None => format!("{stem} ({n})"),
    }
}

/// 同一目录下的去重。key 用 NFC 小写：Windows/macOS 目标文件系统大小写不敏感，
/// 只按字节比较会造出 `A.txt` 和 `a.txt` 两个落盘后互相覆盖的条目。
pub fn unique_sibling_name(
    desired: &str,
    is_dir: bool,
    used: &mut HashSet<String>,
) -> Result<String, String> {
    if used.insert(desired.nfc().collect::<String>().to_lowercase()) {
        return Ok(desired.to_string());
    }
    for n in 1..1000u32 {
        let candidate = numbered_name(desired, is_dir, n);
        if used.insert(candidate.nfc().collect::<String>().to_lowercase()) {
            return Ok(candidate);
        }
    }
    Err(format!("同一目录下同名项过多：{desired}"))
}

/// 严格校验服务器给出的相对路径。客户端只用它判断路径是否安全，
/// **不会把不同路径净化成同一个**（那会造成静默丢文件）。
pub fn validate_relative_dest(rel: &str) -> Result<Vec<String>, String> {
    if rel.is_empty() {
        return Err("路径为空".into());
    }
    if rel.len() > 4096 {
        return Err("路径过长".into());
    }
    if rel.contains('\0') {
        return Err("路径含空字节".into());
    }

    let mut parts = Vec::new();
    for part in rel.split('/') {
        if part.is_empty() || part == "." || part == ".." {
            return Err(format!("路径组件非法：{part}"));
        }
        if part.contains('\\') || part.contains(':') {
            return Err(format!("路径组件非法：{part}"));
        }
        if part
            .chars()
            .any(|c| c.is_control() || matches!(c, '<' | '>' | '"' | '|' | '?' | '*'))
        {
            return Err(format!("路径组件含非法字符：{part}"));
        }
        if part.ends_with(' ') || part.ends_with('.') {
            return Err(format!("路径组件以空格或点结尾：{part}"));
        }
        if part.len() > 255 {
            return Err("路径组件过长".into());
        }
        let stem = part.split('.').next().unwrap_or("").to_ascii_uppercase();
        if WINDOWS_RESERVED.contains(&stem.as_str()) {
            return Err(format!("Windows 保留名：{part}"));
        }
        parts.push(part.to_string());
    }
    Ok(parts)
}

/// 把本地路径展开成投送条目列表。目录会递归展开，并保留空目录。
pub fn build_entries(paths: &[PathBuf], limits: &PushLimits) -> Result<Vec<PushEntry>, String> {
    let mut entries = Vec::new();
    let mut seen_roots = HashSet::new();
    let mut top_used = HashSet::new();
    let mut total_bytes = 0u64;

    for path in paths {
        let canonical = path
            .canonicalize()
            .map_err(|e| format!("无法读取 {}：{e}", path.display()))?;
        // 同一个目录拖两次（或软链和真身同时拖入）只发一次。
        if !seen_roots.insert(canonical.clone()) {
            continue;
        }

        let meta = std::fs::metadata(&canonical)
            .map_err(|e| format!("无法读取 {}：{e}", path.display()))?;
        let display_name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .ok_or_else(|| format!("无法投送：{}", path.display()))?;
        let safe_name = sanitize_push_component(&display_name);
        let dest_name = unique_sibling_name(&safe_name, meta.is_dir(), &mut top_used)?;

        if meta.is_file() {
            entries.push(PushEntry {
                dest: dest_name,
                source: canonical,
                is_dir: false,
                size: meta.len(),
            });
            total_bytes = total_bytes.saturating_add(meta.len());
        } else if meta.is_dir() {
            entries.push(PushEntry {
                dest: dest_name.clone(),
                source: canonical.clone(),
                is_dir: true,
                size: 0,
            });
            walk_push_dir(
                &canonical,
                &dest_name,
                0,
                limits,
                &mut entries,
                &mut total_bytes,
            )?;
        } else {
            return Err(format!("不支持的文件类型：{}", path.display()));
        }

        if entries.len() > limits.max_entries {
            return Err(format!("条目超过 {} 个，请分批发送", limits.max_entries));
        }
        if total_bytes > limits.max_total_bytes {
            return Err("总大小超过上限，请分批发送".into());
        }
    }

    if entries.is_empty() {
        return Err("没有可发送的文件".into());
    }
    Ok(entries)
}

fn walk_push_dir(
    dir: &Path,
    dest_prefix: &str,
    depth: usize,
    limits: &PushLimits,
    entries: &mut Vec<PushEntry>,
    total_bytes: &mut u64,
) -> Result<(), String> {
    if depth > limits.max_depth {
        tracing::warn!("目录层级过深，跳过：{}", dir.display());
        return Ok(());
    }

    let mut children: Vec<_> = std::fs::read_dir(dir)
        .map_err(|e| format!("读取目录失败 {}：{e}", dir.display()))?
        .filter_map(Result::ok)
        .collect();
    children.sort_by_key(|e| e.file_name());

    let mut used = HashSet::new();
    for child in children {
        let raw_name = child.file_name().to_string_lossy().to_string();
        if is_junk(&raw_name) || raw_name == ".wiretoss" {
            continue;
        }

        let path = child.path();
        let link_meta = match std::fs::symlink_metadata(&path) {
            Ok(m) => m,
            Err(_) => continue,
        };
        // 目录内的软链一律不跟随：一个指回上层的链接就能让遍历无限循环，
        // 也可能把共享根之外的文件夹带出去。
        if link_meta.file_type().is_symlink() {
            continue;
        }
        if !link_meta.is_dir() && !link_meta.is_file() {
            continue;
        }

        let safe_name = sanitize_push_component(&raw_name);
        let dest_name = unique_sibling_name(&safe_name, link_meta.is_dir(), &mut used)?;
        let dest = format!("{dest_prefix}/{dest_name}");

        if link_meta.is_dir() {
            entries.push(PushEntry {
                dest: dest.clone(),
                source: path.clone(),
                is_dir: true,
                size: 0,
            });
            if entries.len() > limits.max_entries {
                return Err(format!("条目超过 {} 个，请分批发送", limits.max_entries));
            }
            walk_push_dir(&path, &dest, depth + 1, limits, entries, total_bytes)?;
        } else {
            *total_bytes = total_bytes.saturating_add(link_meta.len());
            entries.push(PushEntry {
                dest,
                source: path,
                is_dir: false,
                size: link_meta.len(),
            });
            if entries.len() > limits.max_entries {
                return Err(format!("条目超过 {} 个，请分批发送", limits.max_entries));
            }
            if *total_bytes > limits.max_total_bytes {
                return Err("总大小超过上限，请分批发送".into());
            }
        }
    }
    Ok(())
}

fn batch_display_name(entries: &[PushEntry]) -> String {
    let mut tops: Vec<String> = Vec::new();
    for entry in entries {
        if let Some(first) = entry.dest.split('/').next() {
            if !first.is_empty() && !tops.iter().any(|t| t == first) {
                tops.push(first.to_string());
            }
        }
    }
    match tops.len() {
        0 => "投送".to_string(),
        1 => tops[0].clone(),
        n => format!("{} 等 {} 项", tops[0], n),
    }
}

/// 本地 GUI 调用：遍历路径、登记批次，并通过 SSE 通知目标客户端来取。
pub async fn create_batch(
    state: &Arc<AppState>,
    client_id: &str,
    paths: Vec<PathBuf>,
) -> Result<String, String> {
    if paths.is_empty() {
        return Err("没有选中任何文件".into());
    }

    // 先确认在线，省得白遍历一个大目录。
    let client_name = {
        let clients = state
            .clients
            .lock()
            .map_err(|_| "客户端表锁中毒".to_string())?;
        clients
            .get(client_id)
            .map(|c| c.info.name.clone())
            .ok_or_else(|| "客户端已离线".to_string())?
    };

    let limits = PushLimits::default();
    let entries = tokio::task::spawn_blocking(move || build_entries(&paths, &limits))
        .await
        .map_err(|e| format!("遍历文件失败：{e}"))??;

    let total_files = entries.iter().filter(|e| !e.is_dir).count() as u64;
    let total_bytes = entries
        .iter()
        .map(|e| if e.is_dir { 0 } else { e.size })
        .sum();
    let name = batch_display_name(&entries);
    let batch_id = uuid::Uuid::new_v4().to_string();

    {
        let mut pushes = state
            .pushes
            .lock()
            .map_err(|_| "投送表锁中毒".to_string())?;
        pushes.insert(
            batch_id.clone(),
            PushBatch {
                id: batch_id.clone(),
                client_id: client_id.to_string(),
                client_name: client_name.clone(),
                name: name.clone(),
                entries,
                total_files,
                total_bytes,
                done: 0,
                created_at: Instant::now(),
                last_activity: Instant::now(),
            },
        );
    }

    if let Err(e) = send_notice(
        state,
        client_id,
        PushNotice::PushOffer {
            batch_id: batch_id.clone(),
            name: name.clone(),
            total_files,
            total_bytes,
        },
    ) {
        if let Ok(mut pushes) = state.pushes.lock() {
            pushes.remove(&batch_id);
        }
        return Err(match e {
            SendError::Offline => "客户端已离线".into(),
            SendError::Busy => "客户端繁忙，请稍后重试".into(),
        });
    }

    state.broadcast(TransferEvent::PushStarted {
        batch_id: batch_id.clone(),
        name,
        client_name,
        file_count: total_files,
        total_bytes,
    });
    Ok(batch_id)
}

// ---------------------------------------------------------------- HTTP API

fn take_batch_for_client<'a>(
    pushes: &'a mut std::collections::HashMap<String, PushBatch>,
    batch: &str,
    client_id: &str,
) -> AppResult<&'a mut PushBatch> {
    let found = pushes
        .get_mut(batch)
        .ok_or_else(|| AppError::NotFound("投送已过期或被取消".into()))?;
    if found.client_id != client_id {
        return Err(AppError::Forbidden);
    }
    found.last_activity = Instant::now();
    Ok(found)
}

pub async fn manifest(
    State(state): State<Arc<AppState>>,
    _user: WriteUser,
    headers: HeaderMap,
    AxumPath(batch): AxumPath<String>,
) -> AppResult<Response> {
    let client_id = parse_client_id(&headers)?;
    let (name, total_files, total_bytes, items) = {
        let mut pushes = state
            .pushes
            .lock()
            .map_err(|_| AppError::Internal("投送表锁中毒".into()))?;
        let found = take_batch_for_client(&mut pushes, &batch, &client_id)?;
        let items = found
            .entries
            .iter()
            .enumerate()
            .map(|(index, entry)| ManifestItem {
                index,
                path: entry.dest.clone(),
                is_dir: entry.is_dir,
                size: entry.size,
            })
            .collect();
        (
            found.name.clone(),
            found.total_files,
            found.total_bytes,
            items,
        )
    };

    let mut response = Json(ManifestResponse {
        batch_id: batch,
        name,
        total_files,
        total_bytes,
        items,
    })
    .into_response();
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(response)
}

pub async fn file(
    State(state): State<Arc<AppState>>,
    _user: WriteUser,
    headers: HeaderMap,
    AxumPath((batch, index)): AxumPath<(String, usize)>,
    req: Request,
) -> AppResult<Response> {
    let client_id = parse_client_id(&headers)?;
    let source = {
        let mut pushes = state
            .pushes
            .lock()
            .map_err(|_| AppError::Internal("投送表锁中毒".into()))?;
        let found = take_batch_for_client(&mut pushes, &batch, &client_id)?;
        let entry = found
            .entries
            .get(index)
            .ok_or_else(|| AppError::NotFound("文件不存在".into()))?;
        if entry.is_dir {
            return Err(AppError::BadRequest("该条目是目录".into()));
        }
        entry.source.clone()
    };

    if !source.is_file() {
        return Err(AppError::NotFound("源文件已不存在".into()));
    }

    // Range / If-Range / ETag 全部交给 ServeFile，和共享目录下载保持一致。
    ServeFile::new(source)
        .oneshot(req)
        .await
        .map_err(AppError::internal)
        .map(IntoResponse::into_response)
}

#[derive(Deserialize)]
pub struct ProgressRequest {
    #[serde(default)]
    pub done: u64,
}

pub async fn progress(
    State(state): State<Arc<AppState>>,
    _user: WriteUser,
    headers: HeaderMap,
    AxumPath(batch): AxumPath<String>,
    Json(req): Json<ProgressRequest>,
) -> AppResult<Json<Value>> {
    require_csrf(&headers)?;
    let client_id = parse_client_id(&headers)?;

    let (done, total_bytes) = {
        let mut pushes = state
            .pushes
            .lock()
            .map_err(|_| AppError::Internal("投送表锁中毒".into()))?;
        let found = take_batch_for_client(&mut pushes, &batch, &client_id)?;
        found.done = req.done.min(found.total_bytes);
        (found.done, found.total_bytes)
    };

    state.broadcast(TransferEvent::PushProgress {
        batch_id: batch,
        done,
        total_bytes,
    });
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
pub struct CompleteRequest {
    #[serde(default)]
    pub ok: bool,
    #[serde(default)]
    pub error: Option<String>,
}

pub async fn complete(
    State(state): State<Arc<AppState>>,
    _user: WriteUser,
    headers: HeaderMap,
    AxumPath(batch): AxumPath<String>,
    Json(req): Json<CompleteRequest>,
) -> AppResult<Json<Value>> {
    require_csrf(&headers)?;
    let client_id = parse_client_id(&headers)?;

    // 无论成功失败都移除批次。保留失败批次会让客户端重连后重新下载，
    // 而接收目录里已经有上一次的半成品，结果是一堆「文件 (1)」。
    let removed = {
        let mut pushes = state
            .pushes
            .lock()
            .map_err(|_| AppError::Internal("投送表锁中毒".into()))?;
        let Some(found) = pushes.get(&batch) else {
            return Err(AppError::NotFound("投送已过期或被取消".into()));
        };
        if found.client_id != client_id {
            return Err(AppError::Forbidden);
        }
        pushes.remove(&batch).expect("刚刚确认存在")
    };

    let error = req
        .error
        .unwrap_or_default()
        .chars()
        .take(500)
        .collect::<String>();
    if req.ok {
        state.broadcast(TransferEvent::PushFinished {
            batch_id: batch,
            bytes: removed.done,
        });
    } else {
        state.broadcast(TransferEvent::PushFailed {
            batch_id: batch,
            error: if error.is_empty() {
                "客户端接收失败".into()
            } else {
                error
            },
            done: removed.done,
        });
    }
    Ok(Json(json!({ "ok": true })))
}

pub async fn pending(
    State(state): State<Arc<AppState>>,
    _user: WriteUser,
    headers: HeaderMap,
) -> AppResult<Json<Value>> {
    let client_id = parse_client_id(&headers)?;
    let now = Instant::now();
    let mut batches: Vec<PendingBatch> = {
        let pushes = state
            .pushes
            .lock()
            .map_err(|_| AppError::Internal("投送表锁中毒".into()))?;
        pushes
            .values()
            .filter(|b| b.client_id == client_id)
            .map(|b| PendingBatch {
                batch_id: b.id.clone(),
                name: b.name.clone(),
                total_files: b.total_files,
                total_bytes: b.total_bytes,
                age_secs: now.duration_since(b.created_at).as_secs(),
            })
            .collect()
    };
    batches.sort_by_key(|b| b.age_secs);
    Ok(Json(json!({ "batches": batches })))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn limits() -> PushLimits {
        PushLimits::default()
    }

    #[test]
    fn sanitize_component_normalizes_and_protects() {
        // NFD 的 é（e + 组合重音）要转成 NFC。
        assert_eq!(sanitize_push_component("cafe\u{301}.txt"), "café.txt");
        assert_eq!(sanitize_push_component("a:b?.txt"), "ab.txt");
        assert_eq!(sanitize_push_component("  name.  "), "name");
        assert_eq!(sanitize_push_component("CON.txt"), "_CON.txt");
        assert_eq!(sanitize_push_component("..."), "_");
        assert_eq!(
            sanitize_push_component(&"x".repeat(300)).chars().count(),
            MAX_COMPONENT_CHARS
        );
    }

    #[test]
    fn sibling_names_are_case_insensitively_unique() {
        let mut used = HashSet::new();
        assert_eq!(
            unique_sibling_name("A.txt", false, &mut used).unwrap(),
            "A.txt"
        );
        assert_eq!(
            unique_sibling_name("a.txt", false, &mut used).unwrap(),
            "a (1).txt"
        );
        let mut used = HashSet::new();
        assert_eq!(
            unique_sibling_name("照片", true, &mut used).unwrap(),
            "照片"
        );
        assert_eq!(
            unique_sibling_name("照片", true, &mut used).unwrap(),
            "照片 (1)"
        );
    }

    #[test]
    fn relative_dest_rejects_traversal_and_platform_hazards() {
        assert_eq!(
            validate_relative_dest("照片/2024/a.jpg").unwrap(),
            vec!["照片", "2024", "a.jpg"]
        );
        for evil in [
            "../etc/passwd",
            "a/../../etc/passwd",
            "a//b",
            "a\\b",
            "C:/Windows",
            "CON.txt",
            "a.",
            "a ",
            ".",
            "..",
        ] {
            assert!(validate_relative_dest(evil).is_err(), "{evil} 应被拒绝");
        }
    }

    #[test]
    fn builds_tree_with_empty_dirs_and_skips_junk() {
        let tmp = tempfile::tempdir().unwrap();
        let photos = tmp.path().join("Photos");
        fs::create_dir_all(photos.join("sub")).unwrap();
        fs::create_dir_all(photos.join("empty")).unwrap();
        fs::write(photos.join("a.txt"), b"hello").unwrap();
        fs::write(photos.join("sub/b.bin"), b"xyz").unwrap();
        fs::write(photos.join(".DS_Store"), b"junk").unwrap();
        fs::write(photos.join("._a.txt"), b"junk").unwrap();

        let entries = build_entries(&[photos], &limits()).unwrap();
        assert!(entries.iter().any(|e| e.dest == "Photos" && e.is_dir));
        assert!(entries.iter().any(|e| e.dest == "Photos/empty" && e.is_dir));
        assert!(entries
            .iter()
            .any(|e| e.dest == "Photos/a.txt" && e.size == 5 && !e.is_dir));
        assert!(entries.iter().any(|e| e.dest == "Photos/sub/b.bin"));
        assert!(!entries.iter().any(|e| e.dest.contains(".DS_Store")));
        assert!(!entries.iter().any(|e| e.dest.contains("._a.txt")));
    }

    #[test]
    fn top_level_names_are_deduplicated() {
        let tmp = tempfile::tempdir().unwrap();
        let a = tmp.path().join("a");
        let b = tmp.path().join("b");
        fs::create_dir_all(&a).unwrap();
        fs::create_dir_all(&b).unwrap();
        fs::write(a.join("same.txt"), b"1").unwrap();
        fs::write(b.join("same.txt"), b"2").unwrap();

        let entries = build_entries(&[a.join("same.txt"), b.join("same.txt")], &limits()).unwrap();
        let dests: Vec<_> = entries.iter().map(|e| e.dest.as_str()).collect();
        assert!(dests.contains(&"same.txt"));
        assert!(dests.contains(&"same (1).txt"));
    }

    #[test]
    fn entry_limit_is_enforced() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("a.txt"), b"a").unwrap();
        fs::write(tmp.path().join("b.txt"), b"b").unwrap();
        let small = PushLimits {
            max_entries: 1,
            ..limits()
        };
        assert!(build_entries(&[tmp.path().to_path_buf()], &small).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn nested_symlinks_are_skipped() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("dir");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("real.txt"), b"ok").unwrap();
        std::os::unix::fs::symlink("/etc/passwd", dir.join("passwd")).unwrap();

        let entries = build_entries(&[dir], &limits()).unwrap();
        assert!(entries.iter().any(|e| e.dest == "dir/real.txt"));
        assert!(!entries.iter().any(|e| e.dest == "dir/passwd"));
    }
}
