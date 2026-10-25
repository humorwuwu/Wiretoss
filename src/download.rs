use std::io;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use async_zip::base::write::ZipFileWriter;
use async_zip::{Compression, ZipDateTime, ZipDateTimeBuilder, ZipEntryBuilder};
use axum::body::Body;
use axum::extract::{Path as AxumPath, Query, Request, State};
use axum::http::header::{CACHE_CONTROL, CONTENT_DISPOSITION, CONTENT_TYPE};
use axum::http::{HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use bytes::Bytes;
// async_zip 走的是 futures 那套 io trait（futures_io::AsyncWrite），不是 tokio 的。
// 这两套 trait 名字一样但互不相通，是这一块最容易踩的坑。
use futures::channel::mpsc;
use futures_util::io::{copy, AsyncWrite, AsyncWriteExt};
use futures_util::{ready, Sink};
use serde::{Deserialize, Serialize};
use tokio_util::compat::TokioAsyncReadCompatExt;
use tower::ServiceExt;
use tower_http::services::ServeFile;
use unicode_normalization::UnicodeNormalization;

use crate::auth::AuthUser;
use crate::error::{AppError, AppResult};
use crate::fs_api::{is_junk, SafeRoot};
use crate::state::{AppState, ZipTicket, ZIP_TICKET_TTL_SECS};

/// 每次从 ZipWriter 往 channel 送的字节量。
/// 太小会让分片数量爆炸、每个分片都过一次 channel 同步；太大则首字节延迟变高。
const ZIP_FLUSH_THRESHOLD: usize = 64 * 1024;

/// 超过这个大小的文件在 zip 里不压缩。
/// 用户打包的通常是视频/图片/压缩包，这些格式本身已压缩过，
/// 再跑一遍 deflate 只会白烧 CPU 换不到体积。
const ZIP_COMPRESS_LIMIT: u64 = 8 * 1024 * 1024;

#[derive(Deserialize)]
pub struct PathQuery {
    pub path: String,
}

// ---------------------------------------------------------------- 单文件下载

/// 下载单个文件。Range 支持交给 `ServeFile`——它已经正确处理了
/// Range / If-Range / ETag / 多段请求这些容易写错的细节，
/// 浏览器自带的下载器因此天然获得断点续传能力。
pub async fn download_file(
    State(state): State<Arc<AppState>>,
    _user: AuthUser,
    Query(q): Query<PathQuery>,
    req: Request,
) -> AppResult<Response> {
    let path = state.root.resolve_existing(&q.path)?;
    let meta = std::fs::metadata(&path)?;
    if meta.is_dir() {
        return Err(AppError::BadRequest("目录请使用打包下载".into()));
    }

    let response = ServeFile::new(&path)
        .oneshot(req)
        .await
        .map_err(AppError::internal)?
        .into_response();

    // 只有成功响应才附加下载头；404/416 的响应体是 ServeFile 的错误信息，
    // 给它加 Content-Disposition 会让浏览器把错误页当文件存下来。
    if response.status().is_success() || response.status() == StatusCode::PARTIAL_CONTENT {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "download".into());
        let mut response = response;
        response.headers_mut().insert(
            CONTENT_DISPOSITION,
            HeaderValue::from_str(&content_disposition(&name)).map_err(AppError::internal)?,
        );
        // 文件可能在服务端被外部程序替换；浏览器缓存会让“保存到本机”
        // 读到旧内容，所以下载响应不缓存。
        response
            .headers_mut()
            .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
        Ok(response)
    } else {
        Ok(response)
    }
}

// ---------------------------------------------------------------- 打包下载

#[derive(Deserialize)]
pub struct ZipRequest {
    /// 相对共享根的路径列表，可以混合文件和目录。
    pub paths: Vec<String>,
}

#[derive(Serialize)]
pub struct ZipTicketResponse {
    pub ticket: String,
    pub name: String,
    pub file_count: usize,
}

/// 第一步：登记要打包的路径，换一张一次性的下载券。
///
/// 之所以不直接 `POST` 返回 zip 流，是因为浏览器的原生下载（以及它的进度条、
/// 断点续传、落盘流式写入）只对**导航式 GET** 生效。走 fetch 拿 zip 就得
/// 整个塞进内存，几个 GB 的文件夹会直接把标签页撑爆。
/// 所以拆成两步：POST 建券（路径列表可以很长，不受 URL 长度限制），
/// 然后让浏览器去 GET 那个短 URL。
pub async fn create_zip_ticket(
    State(state): State<Arc<AppState>>,
    _user: AuthUser,
    Json(req): Json<ZipRequest>,
) -> AppResult<Json<ZipTicketResponse>> {
    if req.paths.is_empty() {
        return Err(AppError::BadRequest("没有选中任何文件".into()));
    }

    // 建券时就把所有路径解析一遍：与其在流已经开始发送之后才发现某个路径非法
    // （那时已经没法返回错误码了），不如在这里一次性拒绝。
    let mut entries = Vec::new();
    for rel in &req.paths {
        collect_entries(&state.root, rel, &mut entries)?;
    }
    if entries.is_empty() {
        return Err(AppError::NotFound("选中的内容为空".into()));
    }

    let ticket = uuid::Uuid::new_v4().to_string();
    let name = zip_name(&req.paths);
    let file_count = entries.len();

    let mut tickets = state
        .zip_tickets
        .lock()
        .map_err(|_| AppError::Internal("下载券表锁中毒".into()))?;
    // 顺手清理过期券，避免长跑进程里无限堆积。
    tickets.retain(|_, t| t.created_at.elapsed().as_secs() < ZIP_TICKET_TTL_SECS);
    tickets.insert(
        ticket.clone(),
        ZipTicket {
            entries,
            name: name.clone(),
            created_at: std::time::Instant::now(),
        },
    );

    Ok(Json(ZipTicketResponse {
        ticket,
        name,
        file_count,
    }))
}

/// 第二步：流式打包。边压边发，不落临时文件，内存占用与文件数无关。
pub async fn download_zip(
    State(state): State<Arc<AppState>>,
    _user: AuthUser,
    AxumPath(ticket): AxumPath<String>,
) -> AppResult<Response> {
    // 券是一次性的：取走即删。这样即使 URL 泄漏到日志里也无法重放。
    let entry = {
        let mut tickets = state
            .zip_tickets
            .lock()
            .map_err(|_| AppError::Internal("下载券表锁中毒".into()))?;
        tickets
            .remove(&ticket)
            .ok_or_else(|| AppError::NotFound("下载券已失效，请重新发起打包下载".into()))?
    };

    let (tx, rx) = mpsc::channel::<Result<Bytes, io::Error>>(8);
    let error_tx = tx.clone();
    // 用建券时算好的名字。这里从展开后的条目反推的话，下载一个叫「照片」的
    // 目录会得到「照片里的第一个文件.zip」，和建券时返回给前端的名字对不上。
    let name = entry.name.clone();

    // 打包是 IO 密集的异步活，丢给独立任务跑，让响应体立刻开始流动。
    tokio::spawn(async move {
        if let Err(e) = write_zip(entry.entries, tx).await {
            // 客户端主动断开是常态（用户点了取消），不该刷成错误日志。
            if e.kind() != io::ErrorKind::BrokenPipe {
                tracing::warn!("打包下载中断：{e}");
            }
            let mut error_tx = error_tx;
            let _ = error_tx.try_send(Err(e));
        }
    });

    let body = Body::from_stream(rx);

    let mut response = Response::new(body);
    response
        .headers_mut()
        .insert(CONTENT_TYPE, HeaderValue::from_static("application/zip"));
    response.headers_mut().insert(
        CONTENT_DISPOSITION,
        HeaderValue::from_str(&content_disposition(&name)).map_err(AppError::internal)?,
    );
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(response)
}

/// 把一批文件写进 zip。`tx` 是唯一的出口——所有字节都经由 channel 流向 HTTP 响应体。
async fn write_zip(
    entries: Vec<(String, PathBuf)>,
    tx: mpsc::Sender<Result<Bytes, io::Error>>,
) -> io::Result<()> {
    let mut writer = ZipFileWriter::new(ChannelWriter::new(tx));

    for (name, path) in entries {
        let meta = tokio::fs::metadata(&path).await?;

        // 已经压过的格式（视频、图片、压缩包）再跑 deflate 只是白烧 CPU，
        // 换不到体积——局域网里 CPU 才是稀缺资源。
        let compression = if meta.len() > ZIP_COMPRESS_LIMIT {
            Compression::Stored
        } else {
            Compression::Deflate
        };

        let mut builder = ZipEntryBuilder::new(name.into(), compression)
            .unix_permissions(entry_permissions(&meta));
        if let Some(dt) = zip_datetime(&meta) {
            builder = builder.last_modification_date(dt);
        }

        // 流式写：先写本地文件头，数据边读边压边发，最后补一个数据描述符。
        // 全程不需要回头改文件，这正是 async_zip 相对 zip crate 的价值所在。
        let mut entry = writer
            .write_entry_stream(builder.build())
            .await
            .map_err(zip_err)?;

        // tokio 的 File 要 `.compat()` 成 futures 的 AsyncRead 才能喂给 entry。
        let mut file = tokio::fs::File::open(&path).await?.compat();
        copy(&mut file, &mut entry).await?;

        // 必须显式 close：没有 Drop 兜底，漏掉这一步这个条目就不会进中央目录，
        // 解压时表现为"文件凭空消失"。
        entry.close().await.map_err(zip_err)?;
    }

    // `close()` 只写中央目录和结束记录，**不会 flush 内层 writer**
    // （它的实现最后是 `Ok(self.writer.into_inner())`）。少了这一步，
    // 缓冲在 ChannelWriter 里的最后一段——正好包含中央目录结束记录——
    // 会随 drop 一起消失，产出的 zip 看起来完整却打不开。
    let mut inner = writer.close().await.map_err(zip_err)?;
    inner.flush().await?;
    Ok(())
}

fn zip_err(e: async_zip::error::ZipError) -> io::Error {
    io::Error::other(e.to_string())
}

/// zip 条目的权限位。不设的话 `external_file_attribute` 默认是 0，
/// 解压出来的文件在 macOS/Linux 上是 `----------`（000），直接打不开。
#[cfg(unix)]
fn entry_permissions(meta: &std::fs::Metadata) -> u16 {
    use std::os::unix::fs::PermissionsExt;
    let mode = (meta.permissions().mode() & 0o777) as u16;
    // 源文件权限本来就是 0 的话不要原样带过去，否则又造出打不开的文件。
    if mode == 0 {
        0o644
    } else {
        mode
    }
}

#[cfg(not(unix))]
fn entry_permissions(_meta: &std::fs::Metadata) -> u16 {
    0o644
}

/// 一个把写入攒够阈值再推给 async 侧的 `AsyncWrite`。
///
/// 存在的意义是给 async_zip 提供背压：channel 满的时候返回 `Pending`，
/// 让 zip 任务挂起，而不是把整个压缩结果堆在内存里等 HTTP 慢慢发。
struct ChannelWriter {
    tx: mpsc::Sender<Result<Bytes, io::Error>>,
    buf: Vec<u8>,
}

impl ChannelWriter {
    fn new(tx: mpsc::Sender<Result<Bytes, io::Error>>) -> Self {
        Self {
            tx,
            buf: Vec::with_capacity(ZIP_FLUSH_THRESHOLD * 2),
        }
    }

    /// 把缓冲区推空。channel 满就注册 waker 后返回 `Pending`。
    fn poll_drain(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.as_mut().get_mut();
        while !this.buf.is_empty() {
            match Pin::new(&mut this.tx).poll_ready(cx) {
                Poll::Pending => return Poll::Pending,
                // 接收端（HTTP 响应体）被丢弃 = 浏览器取消了下载。
                // 这个错误会一路冒泡，让打包任务停下来，
                // 否则用户取消后我们还会把整个目录读完、压完。
                Poll::Ready(Err(_)) => return Poll::Ready(Err(broken_pipe())),
                Poll::Ready(Ok(())) => {
                    let chunk = Bytes::from(std::mem::take(&mut this.buf));
                    if Pin::new(&mut this.tx).start_send(Ok(chunk)).is_err() {
                        return Poll::Ready(Err(broken_pipe()));
                    }
                }
            }
        }
        Poll::Ready(Ok(()))
    }
}

fn broken_pipe() -> io::Error {
    io::Error::new(io::ErrorKind::BrokenPipe, "客户端已断开")
}

impl AsyncWrite for ChannelWriter {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        data: &[u8],
    ) -> Poll<io::Result<usize>> {
        // 攒够阈值就先推出去。推不动（Pending）时直接返回，**不接收这批新数据**——
        // 接收了再返回 Pending 会让调用方重发同样的字节，造成内容重复。
        // 因此缓冲区的大小上限是阈值加单次写入量，不会无限涨。
        if self.buf.len() >= ZIP_FLUSH_THRESHOLD {
            ready!(self.as_mut().poll_drain(cx))?;
        }
        self.buf.extend_from_slice(data);
        Poll::Ready(Ok(data.len()))
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.poll_drain(cx)
    }

    fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.poll_drain(cx)
    }
}

// ---------------------------------------------------------------- 路径收集

/// 把相对路径展开成 `(zip 内条目名, 磁盘绝对路径)` 列表。目录会被递归展开。
fn collect_entries(root: &SafeRoot, rel: &str, out: &mut Vec<(String, PathBuf)>) -> AppResult<()> {
    let abs = root.resolve_existing(rel)?;
    let meta = std::fs::metadata(&abs)?;
    let base = rel
        .rsplit('/')
        .find(|s| !s.is_empty())
        .unwrap_or("download");

    if meta.is_file() {
        out.push((zip_entry_name(base), abs));
    } else {
        walk(root, &abs, base, out, 0)?;
    }
    Ok(())
}

/// 递归展开目录。深度上限 64 是防止病态嵌套把栈打爆——
/// 正常情况下没人会嵌套这么深，触发了说明目录结构本身有问题。
fn walk(
    root: &SafeRoot,
    dir: &Path,
    prefix: &str,
    out: &mut Vec<(String, PathBuf)>,
    depth: usize,
) -> AppResult<()> {
    if depth > 64 {
        tracing::warn!("目录层级过深，跳过：{}", dir.display());
        return Ok(());
    }

    let mut children: Vec<_> = std::fs::read_dir(dir)?.filter_map(Result::ok).collect();
    children.sort_by_key(|e| e.file_name());

    for child in children {
        let name = child.file_name().to_string_lossy().to_string();
        if is_junk(&name) || name == ".wiretoss" {
            continue;
        }

        let path = child.path();
        let link_meta = match std::fs::symlink_metadata(&path) {
            Ok(m) => m,
            Err(_) => continue,
        };

        // 软链必须逐个校验，不能因为"顶层路径已经校验过"就放过。
        // 顶层校验只管到入口，目录里任何一个指向外部的链接都可能是逃逸口。
        if link_meta.file_type().is_symlink() {
            let Ok(real) = path.canonicalize() else {
                continue;
            };
            // 目录软链一律不跟随：一个指回上层的链接就能让打包无限循环。
            // 文件软链只允许指向共享目录内部 —— 否则 `ln -s /etc/passwd x`
            // 就能让人把整台机器的文件打包下载走。
            if real.is_dir() || !real.starts_with(root.path()) {
                continue;
            }
        }

        let entry_name = format!("{}/{}", prefix, zip_entry_name(&name));

        if link_meta.is_dir() {
            walk(root, &path, &entry_name, out, depth + 1)?;
        } else {
            out.push((entry_name, path));
        }
    }
    Ok(())
}

/// zip 条目名统一用 `/` 分隔（zip 规范如此），并转成 NFC。
///
/// NFC 这步是给 Mac -> Windows 用的：macOS 磁盘上存的是 NFD（`é` = `e` + 组合符），
/// 原样写进 zip 再在 Windows 上解开，文件名会显示成两个字符。归档是为了跨机传递，
/// 所以按接收方的习惯归一化。
fn zip_entry_name(name: &str) -> String {
    name.nfc().collect::<String>().replace('\\', "/")
}

fn zip_name(paths: &[String]) -> String {
    let first = paths
        .first()
        .and_then(|p| p.rsplit('/').find(|s| !s.is_empty()))
        .unwrap_or("Wiretoss");
    if paths.len() == 1 {
        format!("{first}.zip")
    } else {
        format!("{first} 等 {} 项.zip", paths.len())
    }
}

fn zip_datetime(meta: &std::fs::Metadata) -> Option<ZipDateTime> {
    let secs = meta
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_secs() as i64;
    let dt = time::OffsetDateTime::from_unix_timestamp(secs).ok()?;

    // zip 用的是 MS-DOS 时间格式，起点是 1980 年，更早的日期根本表示不了。
    // 遇到就直接不设时间戳，而不是塞一个回绕成 2107 年的乱码。
    if dt.year() < 1980 {
        return None;
    }

    Some(
        ZipDateTimeBuilder::new()
            .year(dt.year())
            .month(dt.month() as u32)
            .day(dt.day() as u32)
            .hour(dt.hour() as u32)
            .minute(dt.minute() as u32)
            .second(dt.second() as u32)
            .build(),
    )
}

// ---------------------------------------------------------------- 下载头

/// 构造 Content-Disposition。
///
/// 同时给 `filename=`（ASCII 兜底）和 `filename*=UTF-8''`（RFC 5987）：
/// 老浏览器只认前者，而中文/emoji 文件名只有后者能正确传递。
pub fn content_disposition(filename: &str) -> String {
    let ascii: String = filename
        .chars()
        .map(|c| {
            if c.is_ascii() && c != '"' && c != '\\' && !c.is_control() {
                c
            } else {
                '_'
            }
        })
        .collect();
    let encoded: String = filename
        .bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect();
    format!("attachment; filename=\"{ascii}\"; filename*=UTF-8''{encoded}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_disposition_covers_both_forms() {
        let cd = content_disposition("报告 2024.pdf");
        // ASCII 兜底里中文被替换掉，但结构仍然合法
        assert!(cd.starts_with("attachment; filename=\""));
        assert!(cd.contains("filename*=UTF-8''"));
        // RFC 5987 段必须是百分号编码，不能出现裸的非 ASCII 字节
        let encoded = cd.split("UTF-8''").nth(1).unwrap();
        assert!(encoded.is_ascii());
        assert!(encoded.contains("%E6%8A%A5")); // "报"
    }

    #[test]
    fn content_disposition_escapes_quotes() {
        // 文件名里的引号会截断 filename="..." 参数，必须替换掉
        let cd = content_disposition("we\"ird.txt");
        assert!(cd.contains("filename=\"we_ird.txt\""));
    }

    #[test]
    fn zip_entry_name_uses_forward_slash() {
        assert_eq!(zip_entry_name("a\\b"), "a/b");
    }

    #[test]
    fn zip_entry_name_normalizes_to_nfc() {
        let nfd: String = "cafe\u{301}.txt".to_string(); // e + 组合重音
        let nfc = zip_entry_name(&nfd);
        assert_eq!(nfc, "café.txt");
    }

    #[test]
    fn zip_name_single_vs_multiple() {
        assert_eq!(zip_name(&["docs/a.txt".into()]), "a.txt.zip");
        assert_eq!(
            zip_name(&["docs".into(), "b.txt".into()]),
            "docs 等 2 项.zip"
        );
    }
}
