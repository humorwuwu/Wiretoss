//! 文件系统访问层，同时也是**整个程序唯一的安全边界**。
//!
//! 所有来自 URL/JSON 的相对路径都必须穿过 [`SafeRoot`]，没有例外。
//! 这里做错的后果是把用户整个磁盘暴露给局域网，所以解析逻辑配了较厚的单元测试。

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::UNIX_EPOCH;

use axum::extract::{Query, State};
use axum::http::HeaderMap;
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use unicode_normalization::UnicodeNormalization;

use crate::auth::{require_csrf, AuthUser, WriteUser};
use crate::error::{AppError, AppResult};
use crate::events::TransferEvent;
use crate::state::AppState;

/// 系统生成的垃圾文件，默认不出现在列表里。
pub fn is_junk(name: &str) -> bool {
    name.starts_with("._")
        || matches!(
            name,
            ".DS_Store" | "Thumbs.db" | "desktop.ini" | ".localized" | "ehthumbs.db"
        )
}

/// Windows 保留设备名。这些名字在 macOS/Linux 上能正常创建，
/// 传到 Windows 侧会失败或产生难以排查的诡异行为，所以落盘前就要改掉。
pub(crate) const WINDOWS_RESERVED: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// 已确认存在的共享根目录。持有它的 canonical 形式，后续所有比较都基于它。
#[derive(Debug, Clone)]
pub struct SafeRoot {
    root: PathBuf,
}

impl SafeRoot {
    pub fn new(root: impl AsRef<Path>) -> AppResult<Self> {
        let root = root.as_ref();
        std::fs::create_dir_all(root)?;
        // canonicalize 会解析掉符号链接并转成绝对路径；不这样做的话，
        // 后面拿它和候选路径做前缀比较时会被相对路径/软链骗过去。
        let root = root.canonicalize()?;
        Ok(Self { root })
    }

    pub fn path(&self) -> &Path {
        &self.root
    }

    /// 上传中转目录（`.wiretoss/partial`）。放在共享根下而不是系统临时目录，
    /// 是为了保证 `complete` 时的 rename 是同一文件系统内的原子操作。
    pub fn partial_dir(&self) -> PathBuf {
        self.root.join(".wiretoss").join("partial")
    }

    /// 解析一个**必须已经存在**的路径（列举、下载、删除）。
    pub fn resolve_existing(&self, rel: &str) -> AppResult<PathBuf> {
        let candidate = self.join_relative(rel)?;
        let real = candidate.canonicalize().map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => AppError::NotFound(rel.to_string()),
            _ => AppError::from(e),
        })?;
        self.ensure_inside(&real)?;
        Ok(real)
    }

    /// 解析一个**可以尚不存在**的目标（新建目录、上传落盘）。
    /// 做法是只对父目录做 canonicalize 校验，再拼上净化过的文件名——
    /// 这样既能挡住 `../` 和软链逃逸，又不需要目标文件提前存在。
    pub fn resolve_new(&self, rel_dir: &str, file_name: &str) -> AppResult<PathBuf> {
        let dir = self.join_relative(rel_dir)?;
        let real_dir = dir.canonicalize().map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => AppError::NotFound(rel_dir.to_string()),
            _ => AppError::from(e),
        })?;
        self.ensure_inside(&real_dir)?;
        Ok(real_dir.join(sanitize_filename(file_name)?))
    }

    /// 在一个**已经解析并校验过**的目录下拼一个净化后的文件名。
    /// 上传落盘走这条路径：目标目录在 init 时校验过一次，之后每写一个分片
    /// 再解析一遍相对路径是浪费。
    pub fn resolve_in(&self, dir: &Path, file_name: &str) -> AppResult<PathBuf> {
        self.ensure_inside(dir)?;
        Ok(dir.join(sanitize_filename(file_name)?))
    }

    /// 把相对路径拆成组件后逐级 push。**不直接 `root.join(rel)`**，
    /// 因为那样 `..` 会被静默地拼进去。
    fn join_relative(&self, rel: &str) -> AppResult<PathBuf> {
        let mut path = self.root.clone();
        for part in normalize_components(rel)? {
            path.push(part);
        }
        Ok(path)
    }

    fn ensure_inside(&self, candidate: &Path) -> AppResult<()> {
        // Path::starts_with 是按组件比较的，所以 `/root/foo` 不会被 `/rootbar` 误判为前缀。
        if candidate.starts_with(&self.root) {
            Ok(())
        } else {
            tracing::warn!(
                "拒绝越界访问：{} 不在 {} 内",
                candidate.display(),
                self.root.display()
            );
            Err(AppError::Forbidden)
        }
    }
}

/// 把浏览器传来的相对路径拆成安全的组件列表。
///
/// 直接拒绝 `..` 而不是"解析掉"它：前端从不生成这种路径，
/// 出现即意味着有人在手工构造请求，没必要替它做算术。
fn normalize_components(rel: &str) -> AppResult<Vec<String>> {
    if rel.contains('\0') {
        return Err(AppError::BadRequest("路径含空字节".into()));
    }
    // Windows 客户端可能用反斜杠拼路径，统一成 `/` 再拆。
    let unified = rel.replace('\\', "/");
    let mut parts = Vec::new();
    for part in unified.split('/') {
        match part {
            "" | "." => continue,
            ".." => return Err(AppError::BadRequest("路径中不允许出现 ..".into())),
            p => {
                // 挡掉 `C:` 这类盘符前缀，也顺手挡掉 ADS（`file.txt:stream`）。
                if p.contains(':') {
                    return Err(AppError::BadRequest(format!("路径组件非法：{p}")));
                }
                parts.push(p.to_string());
            }
        }
    }
    // `.wiretoss` 是上传中转目录；列表里已经隐藏，但直接构造 path 不能绕过。
    if parts.first().is_some_and(|part| part == ".wiretoss") {
        return Err(AppError::Forbidden);
    }
    Ok(parts)
}

/// 净化用户提供的文件名，使其在 macOS 和 Windows 上都能安全落盘。
///
/// 非法字符直接**剥掉**、保留名**加下划线前缀**而不是报错：
/// 传一个文件时因为名字里有 `:` 就整单失败，对用户来说是糟糕的体验。
/// 最终落盘路径会在响应里返回，前端据此显示真实文件名。
pub fn sanitize_filename(name: &str) -> AppResult<String> {
    // 只取最后一段，杜绝 `../../etc/passwd` 或 `dir/file` 这类夹带。
    let last = name.rsplit(['/', '\\']).next().unwrap_or("");
    let cleaned: String = last
        .chars()
        .filter(|c| !c.is_control() && !matches!(c, '<' | '>' | ':' | '"' | '|' | '?' | '*'))
        .collect();
    // 结尾的点和空格在 Windows 上会被静默丢弃，导致实际落盘名与预期不符。
    let cleaned = cleaned.trim().trim_end_matches([' ', '.']).to_string();

    if cleaned.is_empty() || cleaned == "." || cleaned == ".." {
        return Err(AppError::BadRequest("文件名无效".into()));
    }

    // 带扩展名同样算保留名：Windows 上 `CON.txt` 一样打不开。
    let stem = cleaned.split('.').next().unwrap_or("").to_ascii_uppercase();
    if WINDOWS_RESERVED.contains(&stem.as_str()) {
        return Ok(format!("_{cleaned}"));
    }
    Ok(cleaned)
}

/// 在同一目录内找一个不冲突的名字：`a.txt` -> `a (1).txt` -> `a (2).txt`。
/// 最多尝试 1000 次，避免病态的无限循环。
pub fn unique_path(dir: &Path, file_name: &str) -> PathBuf {
    let candidate = dir.join(file_name);
    if !candidate.exists() {
        return candidate;
    }
    let path = Path::new(file_name);
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let ext = path.extension().map(|s| s.to_string_lossy().to_string());
    for n in 1..1000 {
        let name = match &ext {
            Some(ext) => format!("{stem} ({n}).{ext}"),
            None => format!("{stem} ({n})"),
        };
        let candidate = dir.join(name);
        if !candidate.exists() {
            return candidate;
        }
    }
    // 极端情况下退回原名，让 rename 去报错，而不是在这里死循环。
    dir.join(file_name)
}

#[derive(Debug, Serialize)]
pub struct Entry {
    /// 磁盘上的真实名字。客户端后续所有请求都必须用它，不能用 `display_name`。
    pub name: String,
    /// 仅供渲染。macOS 返回的文件名是 NFD（`é` 拆成 `e` + 组合符），
    /// 直接显示会和对端 Windows 上的样子对不上，所以展示时统一成 NFC。
    pub display_name: String,
    pub is_dir: bool,
    pub size: u64,
    /// Unix 秒。前端自己按本地时区格式化。
    pub mtime: i64,
    pub mime: Option<String>,
    /// 软链接指向了共享目录之外 —— 列出来但标灰，点开会拿到 403。
    /// 比直接隐藏更诚实：用户会知道那个链接为什么点不动。
    pub restricted: bool,
}

/// 列出目录内容。`rel` 为空串时表示共享根目录。
pub fn list_dir(root: &SafeRoot, rel: &str) -> AppResult<Vec<Entry>> {
    let dir = root.resolve_existing(rel)?;
    let read = std::fs::read_dir(&dir)?;

    let mut entries = Vec::new();
    for item in read {
        let item = item?;
        let raw_name = item.file_name().to_string_lossy().to_string();
        if is_junk(&raw_name) || raw_name == ".wiretoss" {
            continue;
        }

        let link_meta = match item.metadata() {
            Ok(m) => m,
            // 读不到元数据（权限、竞态删除）就跳过这一条，不该让整个列表失败。
            Err(e) => {
                tracing::debug!("跳过 {raw_name}：{e}");
                continue;
            }
        };
        let is_symlink = link_meta.file_type().is_symlink();

        // 软链要跟到真实目标才知道是文件还是目录、有多大。
        let meta = if is_symlink {
            match std::fs::metadata(item.path()) {
                Ok(m) => m,
                Err(_) => continue, // 断链，忽略
            }
        } else {
            link_meta
        };

        // 软链是否逃出了共享根。逃出去的不隐藏，但标记为受限。
        let restricted = if is_symlink {
            match item.path().canonicalize() {
                Ok(real) => !real.starts_with(root.path()),
                Err(_) => true,
            }
        } else {
            false
        };

        let is_dir = meta.is_dir();
        entries.push(Entry {
            display_name: raw_name.nfc().collect(),
            name: raw_name,
            is_dir,
            size: if is_dir { 0 } else { meta.len() },
            mtime: meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0),
            mime: if is_dir {
                None
            } else {
                mime_guess::from_path(item.path())
                    .first()
                    .map(|m| m.essence_str().to_string())
            },
            restricted,
        });
    }

    // 目录在前，然后按名字大小写不敏感排序 —— 两端文件系统的大小写敏感性不同，
    // 用字节序排会让 Mac 和 Windows 上的顺序看起来不一样。
    entries.sort_by(|a, b| {
        b.is_dir.cmp(&a.is_dir).then_with(|| {
            a.display_name
                .to_lowercase()
                .cmp(&b.display_name.to_lowercase())
        })
    });
    Ok(entries)
}

// ---------------------------------------------------------------- HTTP 处理

#[derive(Deserialize)]
pub struct ListQuery {
    #[serde(default)]
    pub path: String,
}

#[derive(Debug, Serialize)]
pub struct TreeNode {
    /// 共享根相对的**真实**路径，下载时必须用它。
    pub path: String,
    pub name: String,
    /// 仅供显示；本地保存时用它作为文件名/目录名。
    pub display_name: String,
    pub is_dir: bool,
    pub size: u64,
    pub children: Vec<TreeNode>,
}

const MAX_TREE_NODES: usize = 50_000;
const MAX_TREE_DEPTH: usize = 64;

/// 返回一个目录的递归树。浏览器保存到本机目录时用它来保留目录结构；
/// 真正下载每个文件仍走 `/api/download`，树里不包含内容。
pub async fn tree(
    State(state): State<Arc<AppState>>,
    _user: AuthUser,
    Query(q): Query<ListQuery>,
) -> AppResult<Json<Value>> {
    let dir = state.root.resolve_existing(&q.path)?;
    if !std::fs::symlink_metadata(&dir)?.is_dir() {
        return Err(AppError::BadRequest("只能列出文件夹".into()));
    }

    let mut count = 0usize;
    let entries = build_tree_children(&state.root, &q.path, 0, &mut count)?;
    Ok(Json(json!({ "path": q.path, "entries": entries })))
}

fn build_tree_children(
    root: &SafeRoot,
    rel: &str,
    depth: usize,
    count: &mut usize,
) -> AppResult<Vec<TreeNode>> {
    if depth > MAX_TREE_DEPTH {
        return Err(AppError::BadRequest("目录层级过深，无法保存到本机".into()));
    }

    let mut out = Vec::new();
    for entry in list_dir(root, rel)? {
        if entry.restricted {
            continue;
        }
        let child_rel = if rel.is_empty() {
            entry.name.clone()
        } else {
            format!("{rel}/{}", entry.name)
        };

        // 软链接不展开：可能指向目录外，也可能形成环。下载逻辑仍可单独处理文件。
        let raw_path = root.path().join(&child_rel);
        if std::fs::symlink_metadata(&raw_path)
            .map(|meta| meta.file_type().is_symlink())
            .unwrap_or(true)
        {
            continue;
        }

        *count += 1;
        if *count > MAX_TREE_NODES {
            return Err(AppError::BadRequest(
                "目录内文件太多，无法一次保存到本机".into(),
            ));
        }

        let children = if entry.is_dir {
            build_tree_children(root, &child_rel, depth + 1, count)?
        } else {
            Vec::new()
        };
        out.push(TreeNode {
            path: child_rel,
            name: entry.name,
            display_name: entry.display_name,
            is_dir: entry.is_dir,
            size: entry.size,
            children,
        });
    }
    Ok(out)
}

pub async fn list(
    State(state): State<Arc<AppState>>,
    _user: AuthUser,
    Query(q): Query<ListQuery>,
) -> AppResult<Json<Value>> {
    let entries = list_dir(&state.root, &q.path)?;
    Ok(Json(json!({ "path": q.path, "entries": entries })))
}

#[derive(Deserialize)]
pub struct MkdirRequest {
    #[serde(default)]
    pub dir: String,
    pub name: String,
}

pub async fn mkdir(
    State(state): State<Arc<AppState>>,
    _user: WriteUser,
    headers: HeaderMap,
    Json(req): Json<MkdirRequest>,
) -> AppResult<Json<Value>> {
    require_csrf(&headers)?;

    let target = state.root.resolve_new(&req.dir, &req.name)?;
    if target.exists() {
        return Err(AppError::Conflict("同名文件或目录已存在".into()));
    }
    std::fs::create_dir(&target)?;

    state.broadcast(TransferEvent::FilesChanged {
        dir: req.dir.clone(),
    });
    Ok(Json(json!({
        "ok": true,
        "name": target.file_name().map(|n| n.to_string_lossy().to_string()),
    })))
}

#[derive(Deserialize)]
pub struct DeleteRequest {
    pub paths: Vec<String>,
}

pub async fn delete(
    State(state): State<Arc<AppState>>,
    _user: WriteUser,
    headers: HeaderMap,
    Json(req): Json<DeleteRequest>,
) -> AppResult<Json<Value>> {
    require_csrf(&headers)?;

    if req.paths.is_empty() {
        return Err(AppError::BadRequest("没有选中任何内容".into()));
    }

    // 先把所有路径都解析校验一遍再动手删。
    // 边解析边删的话，列表中间有一个非法路径就会留下"删了一半"的状态，
    // 用户无从判断哪些真的没了。
    let mut targets = Vec::with_capacity(req.paths.len());
    for rel in &req.paths {
        let target = state.root.resolve_existing(rel)?;
        // 空串会解析成共享根本身。删掉它等于清空整个共享目录，
        // 而这个操作在 UI 上看起来和"删除一个文件夹"没区别，必须挡住。
        if target == state.root.path() {
            return Err(AppError::BadRequest("不能删除共享根目录".into()));
        }
        targets.push(target);
    }

    let mut dirs = std::collections::BTreeSet::new();
    for target in &targets {
        if target.is_dir() {
            std::fs::remove_dir_all(target)?;
        } else {
            std::fs::remove_file(target)?;
        }
        // 通知前端刷新被删对象所在的目录。
        if let Ok(rel) = target.strip_prefix(state.root.path()) {
            let parent = rel.parent().map(|p| p.to_string_lossy().replace('\\', "/"));
            dirs.insert(parent.unwrap_or_default());
        }
    }

    for dir in dirs {
        state.broadcast(TransferEvent::FilesChanged { dir });
    }
    Ok(Json(json!({ "ok": true, "deleted": targets.len() })))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch() -> (tempfile::TempDir, SafeRoot) {
        let tmp = tempfile::tempdir().unwrap();
        let root = SafeRoot::new(tmp.path()).unwrap();
        (tmp, root)
    }

    #[test]
    fn rejects_parent_traversal() {
        let (_tmp, root) = scratch();
        for evil in [
            "../etc/passwd",
            "a/../../etc/passwd",
            "..",
            "a/..",
            "..\\..\\windows\\system32",
        ] {
            assert!(root.resolve_existing(evil).is_err(), "{evil} 应该被拒绝");
        }
    }

    #[test]
    fn rejects_absolute_and_drive_paths() {
        let (_tmp, root) = scratch();
        // 绝对路径会被拆成组件后拼到 root 下，所以这里验证的是
        // "最终仍落在 root 内"，而不是报错——但含盘符的必须直接拒绝。
        assert!(root.resolve_existing("C:/Windows/System32").is_err());
        assert!(root.resolve_existing("c:foo").is_err());
    }

    #[test]
    fn rejects_null_byte() {
        let (_tmp, root) = scratch();
        assert!(root.resolve_existing("a\0b").is_err());
    }

    #[test]
    fn accepts_normal_relative_paths() {
        let (_tmp, root) = scratch();
        std::fs::create_dir_all(root.path().join("docs/2024")).unwrap();
        std::fs::write(root.path().join("docs/2024/note.txt"), b"hi").unwrap();

        let p = root.resolve_existing("docs/2024/note.txt").unwrap();
        assert_eq!(p, root.path().join("docs/2024/note.txt"));

        // 空串和 "." 都表示根目录本身。
        assert_eq!(root.resolve_existing("").unwrap(), root.path());
        assert_eq!(root.resolve_existing(".").unwrap(), root.path());
    }

    // 这两个用例只在 unix 上跑：Windows 建符号链接需要管理员权限，
    // 而整个测试是整块 cfg 掉的，不是把断言藏在 cfg 块里——
    // 后者在 Windows 上会留下"变量建了但没人用"的警告，CI 开了 -D warnings 会直接失败。
    #[cfg(unix)]
    #[test]
    fn rejects_symlink_escaping_root() {
        let (_tmp, root) = scratch();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("secret.txt"), b"top secret").unwrap();

        std::os::unix::fs::symlink(outside.path(), root.path().join("link")).unwrap();
        // 关键用例：纯字符串检查挡不住这个，只有 canonicalize 后比对前缀才行。
        assert!(root.resolve_existing("link/secret.txt").is_err());
        assert!(root.resolve_existing("link").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn symlink_inside_root_is_allowed() {
        let (_tmp, root) = scratch();
        std::fs::create_dir_all(root.path().join("real")).unwrap();
        std::fs::write(root.path().join("real/f.txt"), b"x").unwrap();

        std::os::unix::fs::symlink(root.path().join("real"), root.path().join("alias")).unwrap();
        assert!(root.resolve_existing("alias/f.txt").is_ok());
    }

    #[test]
    fn resolve_new_requires_existing_parent() {
        let (_tmp, root) = scratch();
        std::fs::create_dir_all(root.path().join("sub")).unwrap();
        assert!(root.resolve_new("sub", "new.txt").is_ok());
        assert!(root.resolve_new("nope", "new.txt").is_err());
        // 父目录不允许穿越
        assert!(root.resolve_new("../..", "new.txt").is_err());
    }

    #[test]
    fn sanitize_strips_path_components() {
        assert_eq!(sanitize_filename("../../etc/passwd").unwrap(), "passwd");
        assert_eq!(sanitize_filename("dir\\file.txt").unwrap(), "file.txt");
        assert_eq!(sanitize_filename("a/b/c.bin").unwrap(), "c.bin");
    }

    #[test]
    fn sanitize_handles_windows_reserved_names() {
        assert_eq!(sanitize_filename("CON").unwrap(), "_CON");
        assert_eq!(sanitize_filename("nul").unwrap(), "_nul");
        assert_eq!(sanitize_filename("COM1").unwrap(), "_COM1");
        // 带扩展名同样是保留名
        assert_eq!(sanitize_filename("con.txt").unwrap(), "_con.txt");
        // 不是保留名的正常通过
        assert_eq!(sanitize_filename("console.log").unwrap(), "console.log");
        assert_eq!(sanitize_filename("COM10").unwrap(), "COM10");
    }

    #[test]
    fn sanitize_trims_trailing_dots_and_spaces() {
        // Windows 会静默丢掉结尾的点和空格
        assert_eq!(sanitize_filename("report.txt...").unwrap(), "report.txt");
        assert_eq!(sanitize_filename("name   ").unwrap(), "name");
    }

    #[test]
    fn sanitize_removes_illegal_chars() {
        assert_eq!(
            sanitize_filename("a<b>c:d\"e|f?g*h.txt").unwrap(),
            "abcdefgh.txt"
        );
    }

    #[test]
    fn sanitize_rejects_empty_and_dots() {
        for bad in ["", ".", "..", "/", "///", "  ", "..."] {
            assert!(sanitize_filename(bad).is_err(), "{bad:?} 应该被拒绝");
        }
    }

    #[test]
    fn unique_path_appends_counter() {
        let (_tmp, root) = scratch();
        let dir = root.path();
        std::fs::write(dir.join("a.txt"), b"1").unwrap();
        assert_eq!(unique_path(dir, "a.txt"), dir.join("a (1).txt"));
        std::fs::write(dir.join("a (1).txt"), b"2").unwrap();
        assert_eq!(unique_path(dir, "a.txt"), dir.join("a (2).txt"));
        assert_eq!(unique_path(dir, "b.txt"), dir.join("b.txt"));
    }

    #[test]
    fn list_dir_hides_junk_and_sorts_dirs_first() {
        let (_tmp, root) = scratch();
        std::fs::write(root.path().join(".DS_Store"), b"junk").unwrap();
        std::fs::write(root.path().join("zeta.txt"), b"z").unwrap();
        std::fs::create_dir(root.path().join("alpha")).unwrap();

        let entries = list_dir(&root, "").unwrap();
        let names: Vec<_> = entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["alpha", "zeta.txt"]);
        assert!(entries[0].is_dir);
        assert_eq!(entries[1].size, 1);
    }
}
