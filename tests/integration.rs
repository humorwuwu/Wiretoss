//! 端到端集成测试。
//!
//! 在真实 HTTP 栈上起一个服务实例，走完整链路。单元测试能覆盖路径解析和
//! 指纹算法，但覆盖不了"分片乱序到达后拼出来的文件是否完好"、
//! "Cookie 是否真的生效"这类只有整条链路跑起来才暴露的问题。

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use reqwest::cookie::Jar;
use reqwest::Client;
use serde_json::json;
use sha2::{Digest, Sha256};
use wiretoss::client::{self, ClientSession};
use wiretoss::clients::{ClientInfo, ConnectedClient, PushNotice};
use wiretoss::config::{hash_password, Config};
use wiretoss::push;
use wiretoss::router::build_router;
use wiretoss::state::{AppState, CHUNK_SIZE, PUSH_IDLE_TTL_SECS};

const USER: &str = "tester";
const PASS: &str = "correct horse battery staple";

const CLIENT_ID: &str = "test-client-0001";
const CLIENT_ID_HEADER: &str = "x-wiretoss-client-id";

struct TestServer {
    addr: SocketAddr,
    client: Client,
    root: tempfile::TempDir,
    state: Arc<AppState>,
}

impl TestServer {
    fn url(&self, path: &str) -> String {
        format!("http://{}{path}", self.addr)
    }

    fn path(&self, rel: &str) -> std::path::PathBuf {
        self.root.path().join(rel)
    }

    async fn login(&self) -> reqwest::StatusCode {
        self.client
            .post(self.url("/api/login"))
            .json(&json!({ "username": USER, "password": PASS }))
            .send()
            .await
            .unwrap()
            .status()
    }
}

async fn start() -> TestServer {
    let root = tempfile::tempdir().unwrap();
    let cfg = Config {
        version: 1,
        username: USER.to_string(),
        password_hash: hash_password(PASS).unwrap(),
        root: root.path().to_path_buf(),
        port: 0,
        session_hours: 12,
        client: wiretoss::config::ClientSettings::default(),
        start_mode: wiretoss::config::StartMode::Server,
    };
    let state = AppState::new(cfg).unwrap();

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = build_router(state.clone());

    tokio::spawn(async move {
        // 必须用 with_connect_info：登录限流依赖客户端 IP。
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .unwrap();
    });

    // 给监听一个落地的时间，避免首个请求偶发连接被拒。
    tokio::time::sleep(Duration::from_millis(50)).await;

    let client = Client::builder()
        .cookie_provider(Arc::new(Jar::default()))
        .build()
        .unwrap();

    TestServer {
        addr,
        client,
        root,
        state,
    }
}

/// 确定性伪随机数据。用 xorshift 而不是引入 rand：
/// 测试要的是"内容不会碰巧相等"，不是密码学强度。
fn pseudo_random(len: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(len + 8);
    let mut x: u64 = 0x2545_F491_4F6C_DD1D;
    while out.len() < len {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        out.extend_from_slice(&x.to_le_bytes());
    }
    out.truncate(len);
    out
}

fn sha256(data: &[u8]) -> String {
    hex::encode(Sha256::digest(data))
}

// ─────────────────────────────── 认证 ───────────────────────────────

// 用多线程运行时：默认的 current-thread 运行时下，19 个测试各自在一条线程上
// 跑一个单线程 runtime，服务端任务和测试任务抢同一个执行器，
// 偶发会出现连接被接受却没有响应（客户端报 IncompleteMessage）。
// 生产环境跑的就是多线程运行时，测试也应该对齐。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unauthenticated_requests_are_rejected() {
    let s = start().await;
    for path in ["/api/list", "/api/me", "/api/events"] {
        let res = s.client.get(s.url(path)).send().await.unwrap();
        assert_eq!(res.status(), 401, "{path} 未登录时应该返回 401");
    }
}

// 用多线程运行时：默认的 current-thread 运行时下，19 个测试各自在一条线程上
// 跑一个单线程 runtime，服务端任务和测试任务抢同一个执行器，
// 偶发会出现连接被接受却没有响应（客户端报 IncompleteMessage）。
// 生产环境跑的就是多线程运行时，测试也应该对齐。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn login_rejects_wrong_password() {
    let s = start().await;
    let res = s
        .client
        .post(s.url("/api/login"))
        .json(&json!({ "username": USER, "password": "wrong" }))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 401);

    // 用户名对但密码错、密码对但用户名错，对外表现必须一致，
    // 否则等于告诉攻击者哪个用户名存在。
    let res2 = s
        .client
        .post(s.url("/api/login"))
        .json(&json!({ "username": "nosuchuser", "password": PASS }))
        .send()
        .await
        .unwrap();
    assert_eq!(res2.status(), 401);
}

// 用多线程运行时：默认的 current-thread 运行时下，19 个测试各自在一条线程上
// 跑一个单线程 runtime，服务端任务和测试任务抢同一个执行器，
// 偶发会出现连接被接受却没有响应（客户端报 IncompleteMessage）。
// 生产环境跑的就是多线程运行时，测试也应该对齐。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn login_then_list_and_logout() {
    let s = start().await;
    std::fs::write(s.path("hello.txt"), b"hi").unwrap();

    assert_eq!(s.login().await, 200);

    let entries: serde_json::Value = s
        .client
        .get(s.url("/api/list"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(entries["entries"][0]["name"], "hello.txt");
    assert_eq!(entries["entries"][0]["size"], 2);

    // 登出后会话必须立刻失效（这正是用服务端会话表而不是签名 Cookie 的好处）。
    s.client
        .post(s.url("/api/logout"))
        .header("X-Wiretoss", "1")
        .send()
        .await
        .unwrap();
    assert_eq!(
        s.client
            .get(s.url("/api/list"))
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
}

// 用多线程运行时：默认的 current-thread 运行时下，19 个测试各自在一条线程上
// 跑一个单线程 runtime，服务端任务和测试任务抢同一个执行器，
// 偶发会出现连接被接受却没有响应（客户端报 IncompleteMessage）。
// 生产环境跑的就是多线程运行时，测试也应该对齐。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn login_is_rate_limited_after_repeated_failures() {
    let s = start().await;
    for _ in 0..5 {
        s.client
            .post(s.url("/api/login"))
            .json(&json!({ "username": USER, "password": "wrong" }))
            .send()
            .await
            .unwrap();
    }
    let res = s
        .client
        .post(s.url("/api/login"))
        .json(&json!({ "username": USER, "password": PASS }))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 429, "连续失败后应该触发限流");
}

// 用多线程运行时：默认的 current-thread 运行时下，19 个测试各自在一条线程上
// 跑一个单线程 runtime，服务端任务和测试任务抢同一个执行器，
// 偶发会出现连接被接受却没有响应（客户端报 IncompleteMessage）。
// 生产环境跑的就是多线程运行时，测试也应该对齐。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mutating_requests_require_csrf_header() {
    let s = start().await;
    s.login().await;

    let res = s
        .client
        .post(s.url("/api/mkdir"))
        .json(&json!({ "dir": "", "name": "x" }))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 400, "缺少 X-Wiretoss 头的写操作必须被拒绝");
}

// ─────────────────────────────── 上传 / 下载 ───────────────────────────────

// 用多线程运行时：默认的 current-thread 运行时下，19 个测试各自在一条线程上
// 跑一个单线程 runtime，服务端任务和测试任务抢同一个执行器，
// 偶发会出现连接被接受却没有响应（客户端报 IncompleteMessage）。
// 生产环境跑的就是多线程运行时，测试也应该对齐。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn chunked_upload_then_download_roundtrip() {
    let s = start().await;
    assert_eq!(s.login().await, 200);

    // 刻意跨多个分片，且最后一片不满 —— 边界正是最容易写错的地方。
    let data = pseudo_random(3 * CHUNK_SIZE as usize + 12345);
    let expected = sha256(&data);

    let init: serde_json::Value = s
        .client
        .post(s.url("/api/upload/init"))
        .header("X-Wiretoss", "1")
        .json(&json!({
            "name": "big.bin",
            "size": data.len(),
            "last_modified": 1_700_000_000,
            "dir": "",
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    let upload_id = init["upload_id"].as_str().unwrap().to_string();
    let total = init["total_chunks"].as_u64().unwrap();
    assert_eq!(total, 4);
    assert!(init["received_chunks"].as_array().unwrap().is_empty());

    // 倒序上传：如果服务端是按到达顺序追加而不是按偏移写入，
    // 拼出来的文件就是错的，这个用例会立刻抓到。
    for idx in (0..total).rev() {
        let start = idx * CHUNK_SIZE;
        let end = ((idx + 1) * CHUNK_SIZE).min(data.len() as u64);
        let chunk = data[start as usize..end as usize].to_vec();

        let res = s
            .client
            .put(s.url(&format!("/api/upload/chunk/{upload_id}/{idx}")))
            .header("X-Wiretoss", "1")
            .body(chunk)
            .send()
            .await
            .unwrap();
        assert!(
            res.status().is_success(),
            "分片 {idx} 上传失败：{}",
            res.status()
        );
    }

    let done = s
        .client
        .post(s.url(&format!("/api/upload/{upload_id}/complete")))
        .header("X-Wiretoss", "1")
        .send()
        .await
        .unwrap();
    assert!(done.status().is_success());

    let body = s
        .client
        .get(s.url("/api/download"))
        .query(&[("path", "big.bin")])
        .send()
        .await
        .unwrap()
        .bytes()
        .await
        .unwrap();

    assert_eq!(body.len(), data.len());
    assert_eq!(sha256(&body), expected, "下载回来的内容与上传的不一致");

    // 中转目录必须被清干净，不能留下几个 GB 的残骸。
    let partial = s.path(".wiretoss/partial");
    assert_eq!(
        std::fs::read_dir(&partial).map(|d| d.count()).unwrap_or(0),
        0,
        "上传完成后中转目录应该为空"
    );
}

// 用多线程运行时：默认的 current-thread 运行时下，19 个测试各自在一条线程上
// 跑一个单线程 runtime，服务端任务和测试任务抢同一个执行器，
// 偶发会出现连接被接受却没有响应（客户端报 IncompleteMessage）。
// 生产环境跑的就是多线程运行时，测试也应该对齐。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn upload_resumes_from_existing_chunks() {
    let s = start().await;
    s.login().await;

    let data = pseudo_random(3 * CHUNK_SIZE as usize);
    let init_body = json!({
        "name": "resume.bin",
        "size": data.len(),
        "last_modified": 1_700_000_000,
        "dir": "",
    });

    let first: serde_json::Value = s
        .client
        .post(s.url("/api/upload/init"))
        .header("X-Wiretoss", "1")
        .json(&init_body)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let upload_id = first["upload_id"].as_str().unwrap().to_string();

    // 只传前两片，模拟"传一半断网/关页面"。
    for idx in 0..2u64 {
        let start = idx * CHUNK_SIZE;
        let chunk = data[start as usize..(start + CHUNK_SIZE) as usize].to_vec();
        s.client
            .put(s.url(&format!("/api/upload/chunk/{upload_id}/{idx}")))
            .header("X-Wiretoss", "1")
            .body(chunk)
            .send()
            .await
            .unwrap();
    }

    // 重新 init 同样的文件：必须拿回同一个会话和已收到的分片列表。
    let second: serde_json::Value = s
        .client
        .post(s.url("/api/upload/init"))
        .header("X-Wiretoss", "1")
        .json(&init_body)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    assert_eq!(second["upload_id"], upload_id, "应该复用同一个上传会话");
    assert_eq!(second["resumed"], true);
    assert_eq!(second["received_chunks"].as_array().unwrap().len(), 2);

    // 补上最后一片后必须能正常完成。
    let start = 2 * CHUNK_SIZE;
    s.client
        .put(s.url(&format!("/api/upload/chunk/{upload_id}/2")))
        .header("X-Wiretoss", "1")
        .body(data[start as usize..].to_vec())
        .send()
        .await
        .unwrap();

    let done = s
        .client
        .post(s.url(&format!("/api/upload/{upload_id}/complete")))
        .header("X-Wiretoss", "1")
        .send()
        .await
        .unwrap();
    assert!(done.status().is_success());

    let body = s
        .client
        .get(s.url("/api/download"))
        .query(&[("path", "resume.bin")])
        .send()
        .await
        .unwrap()
        .bytes()
        .await
        .unwrap();
    assert_eq!(sha256(&body), sha256(&data), "续传后的文件内容必须完好");
}

// 用多线程运行时：默认的 current-thread 运行时下，19 个测试各自在一条线程上
// 跑一个单线程 runtime，服务端任务和测试任务抢同一个执行器，
// 偶发会出现连接被接受却没有响应（客户端报 IncompleteMessage）。
// 生产环境跑的就是多线程运行时，测试也应该对齐。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn complete_fails_while_chunks_are_missing() {
    let s = start().await;
    s.login().await;

    let init: serde_json::Value = s
        .client
        .post(s.url("/api/upload/init"))
        .header("X-Wiretoss", "1")
        .json(&json!({
            "name": "partial.bin",
            "size": 2 * CHUNK_SIZE,
            "last_modified": 1,
            "dir": "",
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let id = init["upload_id"].as_str().unwrap();

    let res = s
        .client
        .post(s.url(&format!("/api/upload/{id}/complete")))
        .header("X-Wiretoss", "1")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 400);
    // 且不能留下半截文件在共享目录里
    assert!(!s.path("partial.bin").exists());
}

// 用多线程运行时：默认的 current-thread 运行时下，19 个测试各自在一条线程上
// 跑一个单线程 runtime，服务端任务和测试任务抢同一个执行器，
// 偶发会出现连接被接受却没有响应（客户端报 IncompleteMessage）。
// 生产环境跑的就是多线程运行时，测试也应该对齐。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wrong_chunk_size_is_rejected() {
    let s = start().await;
    s.login().await;

    let init: serde_json::Value = s
        .client
        .post(s.url("/api/upload/init"))
        .header("X-Wiretoss", "1")
        .json(&json!({
            "name": "x.bin",
            "size": 2 * CHUNK_SIZE,
            "last_modified": 1,
            "dir": "",
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let id = init["upload_id"].as_str().unwrap();

    // 第 0 片不是最后一片，必须恰好 CHUNK_SIZE。短一点就说明客户端算错了，
    // 放过去会在文件中间留下空洞。
    let res = s
        .client
        .put(s.url(&format!("/api/upload/chunk/{id}/0")))
        .header("X-Wiretoss", "1")
        .body(vec![0u8; 1024])
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 400);
}

// 用多线程运行时：默认的 current-thread 运行时下，19 个测试各自在一条线程上
// 跑一个单线程 runtime，服务端任务和测试任务抢同一个执行器，
// 偶发会出现连接被接受却没有响应（客户端报 IncompleteMessage）。
// 生产环境跑的就是多线程运行时，测试也应该对齐。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn range_request_returns_partial_content() {
    let s = start().await;
    s.login().await;
    std::fs::write(s.path("r.txt"), b"0123456789").unwrap();

    let res = s
        .client
        .get(s.url("/api/download"))
        .query(&[("path", "r.txt")])
        .header("Range", "bytes=2-5")
        .send()
        .await
        .unwrap();

    assert_eq!(res.status(), 206);
    assert_eq!(res.headers()["content-range"], "bytes 2-5/10");
    assert_eq!(res.bytes().await.unwrap().as_ref(), b"2345");
}

// ─────────────────────────────── 安全边界 ───────────────────────────────

// 用多线程运行时：默认的 current-thread 运行时下，19 个测试各自在一条线程上
// 跑一个单线程 runtime，服务端任务和测试任务抢同一个执行器，
// 偶发会出现连接被接受却没有响应（客户端报 IncompleteMessage）。
// 生产环境跑的就是多线程运行时，测试也应该对齐。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn path_traversal_is_rejected() {
    let s = start().await;
    s.login().await;

    for evil in [
        "../etc/passwd",
        "a/../../etc/passwd",
        "..",
        "sub/../../outside.txt",
    ] {
        let res = s
            .client
            .get(s.url("/api/download"))
            .query(&[("path", evil)])
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 400, "{evil} 应该被拒绝");
    }
}

// 用多线程运行时：默认的 current-thread 运行时下，19 个测试各自在一条线程上
// 跑一个单线程 runtime，服务端任务和测试任务抢同一个执行器，
// 偶发会出现连接被接受却没有响应（客户端报 IncompleteMessage）。
// 生产环境跑的就是多线程运行时，测试也应该对齐。
// Windows 建符号链接要管理员权限，所以这几个用例整块只在 unix 上跑。
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn symlink_escaping_shared_root_is_blocked() {
    let s = start().await;
    s.login().await;

    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("secret.txt"), b"top secret").unwrap();
    std::os::unix::fs::symlink(outside.path(), s.path("escape")).unwrap();

    let res = s
        .client
        .get(s.url("/api/download"))
        .query(&[("path", "escape/secret.txt")])
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 403, "软链逃出共享目录必须被拒绝");

    // 列表里应该能看到它，但标记为受限 —— 静默隐藏会让用户困惑。
    let entries: serde_json::Value = s
        .client
        .get(s.url("/api/list"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let entry = entries["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["name"] == "escape")
        .expect("受限软链仍应出现在列表里");
    assert_eq!(entry["restricted"], true);
}

// 用多线程运行时：默认的 current-thread 运行时下，19 个测试各自在一条线程上
// 跑一个单线程 runtime，服务端任务和测试任务抢同一个执行器，
// 偶发会出现连接被接受却没有响应（客户端报 IncompleteMessage）。
// 生产环境跑的就是多线程运行时，测试也应该对齐。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cannot_delete_shared_root() {
    let s = start().await;
    s.login().await;
    std::fs::write(s.path("keep.txt"), b"keep me").unwrap();

    let res = s
        .client
        .post(s.url("/api/delete"))
        .header("X-Wiretoss", "1")
        .json(&json!({ "paths": [""] }))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 400);
    assert!(s.path("keep.txt").exists(), "共享根目录不能被删掉");
}

// 用多线程运行时：默认的 current-thread 运行时下，19 个测试各自在一条线程上
// 跑一个单线程 runtime，服务端任务和测试任务抢同一个执行器，
// 偶发会出现连接被接受却没有响应（客户端报 IncompleteMessage）。
// 生产环境跑的就是多线程运行时，测试也应该对齐。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn windows_reserved_names_are_rewritten_on_upload() {
    let s = start().await;
    s.login().await;

    let init: serde_json::Value = s
        .client
        .post(s.url("/api/upload/init"))
        .header("X-Wiretoss", "1")
        .json(&json!({
            "name": "CON.txt",
            "size": 3,
            "last_modified": 1,
            "dir": "",
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let id = init["upload_id"].as_str().unwrap();

    s.client
        .put(s.url(&format!("/api/upload/chunk/{id}/0")))
        .header("X-Wiretoss", "1")
        .body(b"abc".to_vec())
        .send()
        .await
        .unwrap();

    let done: serde_json::Value = s
        .client
        .post(s.url(&format!("/api/upload/{id}/complete")))
        .header("X-Wiretoss", "1")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    // 落盘名被加了前缀，且响应如实告知真实文件名。
    assert_eq!(done["name"], "_CON.txt");
    assert!(s.path("_CON.txt").exists());
}

// ─────────────────────────────── 打包下载 ───────────────────────────────

// 用多线程运行时：默认的 current-thread 运行时下，19 个测试各自在一条线程上
// 跑一个单线程 runtime，服务端任务和测试任务抢同一个执行器，
// 偶发会出现连接被接受却没有响应（客户端报 IncompleteMessage）。
// 生产环境跑的就是多线程运行时，测试也应该对齐。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn zip_download_produces_valid_archive() {
    let s = start().await;
    s.login().await;

    std::fs::write(s.path("a.txt"), b"hello").unwrap();
    std::fs::create_dir(s.path("sub")).unwrap();
    std::fs::write(s.path("sub/b.txt"), b"world").unwrap();
    std::fs::write(s.path("sub/.DS_Store"), b"junk").unwrap();

    let ticket: serde_json::Value = s
        .client
        .post(s.url("/api/zip"))
        .header("X-Wiretoss", "1")
        .json(&json!({ "paths": ["a.txt", "sub"] }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    assert_eq!(ticket["file_count"], 2, "系统垃圾文件不该被打进包里");

    let bytes = s
        .client
        .get(s.url(&format!("/api/zip/{}", ticket["ticket"].as_str().unwrap())))
        .send()
        .await
        .unwrap()
        .bytes()
        .await
        .unwrap();

    assert!(bytes.len() > 22, "zip 至少要有中央目录结束记录");
    assert_eq!(&bytes[0..4], b"PK\x03\x04", "不是合法的 zip 文件");
    // 结尾必须是中央目录结束标记。少了它说明 writer 没被 flush/close，
    // 这种包看起来完整但所有解压工具都会报错。
    assert_eq!(&bytes[bytes.len() - 22..bytes.len() - 18], b"PK\x05\x06");

    // 用另一个 zip 实现读回来验证。自己写自己读会一起错，
    // 独立实现能真正证明产出的格式是合规的。
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes.to_vec())).unwrap();
    assert_eq!(archive.len(), 2);

    let mut names: Vec<String> = (0..archive.len())
        .map(|i| {
            archive
                .by_index(i)
                .unwrap()
                .name()
                .expect("条目名应该是合法 UTF-8")
                .to_string()
        })
        .collect();
    names.sort();
    // 目录条目用 `/` 分隔，Windows 解压才能还原层级。
    assert_eq!(names, vec!["a.txt", "sub/b.txt"]);

    for i in 0..archive.len() {
        let entry = archive.by_index(i).unwrap();
        let mode = entry.unix_mode().expect("应该带有 unix 权限位");
        // 权限位为 0 的话解压出来是 ----------，文件直接打不开。
        assert!(
            mode & 0o400 != 0,
            "条目 {} 缺少读权限位：{mode:o}",
            entry.name().unwrap_or_default()
        );
    }

    let mut content = String::new();
    use std::io::Read;
    archive
        .by_name("sub/b.txt")
        .unwrap()
        .read_to_string(&mut content)
        .unwrap();
    assert_eq!(content, "world", "解压出来的内容必须与原始文件一致");
}

// 用多线程运行时：默认的 current-thread 运行时下，19 个测试各自在一条线程上
// 跑一个单线程 runtime，服务端任务和测试任务抢同一个执行器，
// 偶发会出现连接被接受却没有响应（客户端报 IncompleteMessage）。
// 生产环境跑的就是多线程运行时，测试也应该对齐。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn zip_ticket_is_single_use() {
    let s = start().await;
    s.login().await;
    std::fs::write(s.path("a.txt"), b"hello").unwrap();

    let ticket: serde_json::Value = s
        .client
        .post(s.url("/api/zip"))
        .header("X-Wiretoss", "1")
        .json(&json!({ "paths": ["a.txt"] }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let url = s.url(&format!("/api/zip/{}", ticket["ticket"].as_str().unwrap()));

    assert!(s
        .client
        .get(&url)
        .send()
        .await
        .unwrap()
        .status()
        .is_success());
    // 券是一次性的：即使 URL 泄漏到日志里也无法重放。
    assert_eq!(s.client.get(&url).send().await.unwrap().status(), 404);
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn zip_excludes_symlinks_pointing_outside_root() {
    let s = start().await;
    s.login().await;

    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("secret.txt"), b"TOP SECRET").unwrap();

    std::fs::create_dir(s.path("folder")).unwrap();
    std::fs::write(s.path("folder/ok.txt"), b"fine").unwrap();

    // 打包走目录递归，和单文件下载是两条独立路径。
    // 顶层路径校验挡不住目录**内部**的软链，所以这里必须单独覆盖。
    std::os::unix::fs::symlink(outside.path().join("secret.txt"), s.path("folder/leak.txt"))
        .unwrap();
    std::os::unix::fs::symlink(outside.path(), s.path("folder/leakdir")).unwrap();

    {
        let ticket: serde_json::Value = s
            .client
            .post(s.url("/api/zip"))
            .header("X-Wiretoss", "1")
            .json(&json!({ "paths": ["folder"] }))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();

        let bytes = s
            .client
            .get(s.url(&format!("/api/zip/{}", ticket["ticket"].as_str().unwrap())))
            .send()
            .await
            .unwrap()
            .bytes()
            .await
            .unwrap();

        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes.to_vec())).unwrap();
        let names: Vec<String> = (0..archive.len())
            .map(|i| archive.by_index(i).unwrap().name().unwrap().to_string())
            .collect();

        assert_eq!(
            names,
            vec!["folder/ok.txt"],
            "指向共享目录外的软链不该被打进包里"
        );
    }
}

// ─────────────────────────────── 目录操作 ───────────────────────────────

// 用多线程运行时：默认的 current-thread 运行时下，19 个测试各自在一条线程上
// 跑一个单线程 runtime，服务端任务和测试任务抢同一个执行器，
// 偶发会出现连接被接受却没有响应（客户端报 IncompleteMessage）。
// 生产环境跑的就是多线程运行时，测试也应该对齐。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mkdir_list_and_delete() {
    let s = start().await;
    s.login().await;

    let res = s
        .client
        .post(s.url("/api/mkdir"))
        .header("X-Wiretoss", "1")
        .json(&json!({ "dir": "", "name": "新建目录" }))
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success());
    assert!(s.path("新建目录").is_dir());

    // 重名必须报冲突，不能静默覆盖。
    let dup = s
        .client
        .post(s.url("/api/mkdir"))
        .header("X-Wiretoss", "1")
        .json(&json!({ "dir": "", "name": "新建目录" }))
        .send()
        .await
        .unwrap();
    assert_eq!(dup.status(), 409);

    std::fs::write(s.path("新建目录/inside.txt"), b"x").unwrap();
    let del = s
        .client
        .post(s.url("/api/delete"))
        .header("X-Wiretoss", "1")
        .json(&json!({ "paths": ["新建目录"] }))
        .send()
        .await
        .unwrap();
    assert!(del.status().is_success());
    assert!(!s.path("新建目录").exists(), "目录应被递归删除");
}

// 用多线程运行时：默认的 current-thread 运行时下，19 个测试各自在一条线程上
// 跑一个单线程 runtime，服务端任务和测试任务抢同一个执行器，
// 偶发会出现连接被接受却没有响应（客户端报 IncompleteMessage）。
// 生产环境跑的就是多线程运行时，测试也应该对齐。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn upload_into_nonexistent_dir_is_rejected() {
    let s = start().await;
    s.login().await;

    let res = s
        .client
        .post(s.url("/api/upload/init"))
        .header("X-Wiretoss", "1")
        .json(&json!({
            "name": "a.bin",
            "size": 10,
            "last_modified": 1,
            "dir": "no/such/dir",
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 404);
}

// 用多线程运行时：默认的 current-thread 运行时下，19 个测试各自在一条线程上
// 跑一个单线程 runtime，服务端任务和测试任务抢同一个执行器，
// 偶发会出现连接被接受却没有响应（客户端报 IncompleteMessage）。
// 生产环境跑的就是多线程运行时，测试也应该对齐。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unknown_api_paths_return_json_not_html() {
    let s = start().await;
    let res = s.client.get(s.url("/api/nope")).send().await.unwrap();
    assert_eq!(res.status(), 404);
    // 掉进前端 fallback 的话这里会是一个 200 的 HTML，
    // 前端 fetch 会抛 "Unexpected token '<'"，非常难排查。
    let ct = res.headers()["content-type"].to_str().unwrap().to_string();
    assert!(ct.contains("json"), "实际 content-type：{ct}");
}

// ─────────────────────────────── 投送 ───────────────────────────────

fn register_client(s: &TestServer, id: &str) -> tokio::sync::mpsc::Receiver<PushNotice> {
    let (tx, rx) = tokio::sync::mpsc::channel(8);
    s.state.clients.lock().unwrap().insert(
        id.to_string(),
        ConnectedClient {
            info: ClientInfo {
                id: id.to_string(),
                name: "测试客户端".into(),
                ip: "127.0.0.1".parse().unwrap(),
                connected_at: std::time::Instant::now(),
            },
            conn_id: format!("conn-{id}"),
            tx,
        },
    );
    rx
}

fn write_file(path: &std::path::Path, data: &[u8]) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, data).unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn push_offer_manifest_and_file_roundtrip() {
    let s = start().await;
    assert_eq!(s.login().await, reqwest::StatusCode::OK);

    let src = tempfile::tempdir().unwrap();
    let photos = src.path().join("Photos");
    std::fs::create_dir_all(photos.join("sub")).unwrap();
    std::fs::create_dir_all(photos.join("empty")).unwrap();
    let a_data = pseudo_random(4096);
    write_file(&photos.join("a.bin"), &a_data);
    write_file(&photos.join("sub/b.txt"), b"hello");
    let top = src.path().join("note.txt");
    write_file(&top, b"top");

    let mut offers = register_client(&s, CLIENT_ID);
    let batch_id = push::create_batch(&s.state, CLIENT_ID, vec![top, photos])
        .await
        .unwrap();

    match offers.recv().await.unwrap() {
        PushNotice::PushOffer {
            batch_id: offered,
            total_files,
            ..
        } => {
            assert_eq!(offered, batch_id);
            assert_eq!(total_files, 3);
        }
        other => panic!("unexpected offer: {other:?}"),
    }

    let manifest: serde_json::Value = s
        .client
        .get(s.url(&format!("/api/push/{batch_id}/manifest")))
        .header(CLIENT_ID_HEADER, CLIENT_ID)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let items = manifest["items"].as_array().unwrap();
    assert!(items
        .iter()
        .any(|i| i["path"] == "Photos" && i["is_dir"] == true));
    assert!(items
        .iter()
        .any(|i| i["path"] == "Photos/empty" && i["is_dir"] == true));

    let index = items.iter().find(|i| i["path"] == "Photos/a.bin").unwrap()["index"]
        .as_u64()
        .unwrap();
    let file_url = s.url(&format!("/api/push/{batch_id}/file/{index}"));

    let bytes = s
        .client
        .get(&file_url)
        .header(CLIENT_ID_HEADER, CLIENT_ID)
        .send()
        .await
        .unwrap()
        .bytes()
        .await
        .unwrap();
    assert_eq!(sha256(&bytes), sha256(&a_data));

    let range = s
        .client
        .get(&file_url)
        .header(CLIENT_ID_HEADER, CLIENT_ID)
        .header(reqwest::header::RANGE, "bytes=0-3")
        .send()
        .await
        .unwrap();
    assert_eq!(range.status(), reqwest::StatusCode::PARTIAL_CONTENT);

    let progress = s
        .client
        .post(s.url(&format!("/api/push/{batch_id}/progress")))
        .header(CLIENT_ID_HEADER, CLIENT_ID)
        .header("x-wiretoss", "1")
        .json(&json!({ "done": 1000 }))
        .send()
        .await
        .unwrap();
    assert_eq!(progress.status(), reqwest::StatusCode::OK);

    let complete = s
        .client
        .post(s.url(&format!("/api/push/{batch_id}/complete")))
        .header(CLIENT_ID_HEADER, CLIENT_ID)
        .header("x-wiretoss", "1")
        .json(&json!({ "ok": true }))
        .send()
        .await
        .unwrap();
    assert_eq!(complete.status(), reqwest::StatusCode::OK);
    assert!(s.state.pushes.lock().unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn push_requires_matching_client_id() {
    let s = start().await;
    s.login().await;
    let _offers = register_client(&s, CLIENT_ID);

    let src = tempfile::tempdir().unwrap();
    let file = src.path().join("a.txt");
    write_file(&file, b"x");
    let batch_id = push::create_batch(&s.state, CLIENT_ID, vec![file])
        .await
        .unwrap();

    let res = s
        .client
        .get(s.url(&format!("/api/push/{batch_id}/manifest")))
        .header(CLIENT_ID_HEADER, "other-client-9999")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), reqwest::StatusCode::FORBIDDEN);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn push_mutations_require_csrf_and_auth() {
    let s = start().await;

    // 未登录：AuthUser 提取器会先拦住。
    let anon = Client::new();
    let res = anon
        .get(s.url("/api/push/whatever/manifest"))
        .header(CLIENT_ID_HEADER, CLIENT_ID)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), reqwest::StatusCode::UNAUTHORIZED);

    s.login().await;

    let res = s
        .client
        .post(s.url("/api/push/whatever/progress"))
        .header(CLIENT_ID_HEADER, CLIENT_ID)
        .json(&json!({ "done": 1 }))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), reqwest::StatusCode::BAD_REQUEST);

    let res = s
        .client
        .post(s.url("/api/push/whatever/complete"))
        .header(CLIENT_ID_HEADER, CLIENT_ID)
        .json(&json!({ "ok": true }))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), reqwest::StatusCode::BAD_REQUEST);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn native_client_receives_file_tree() {
    let s = start().await;
    s.login().await;
    let _offers = register_client(&s, CLIENT_ID);

    let src = tempfile::tempdir().unwrap();
    let photos = src.path().join("Photos");
    std::fs::create_dir_all(photos.join("sub")).unwrap();
    std::fs::create_dir_all(photos.join("empty")).unwrap();
    let a_data = pseudo_random(2048);
    write_file(&photos.join("a.bin"), &a_data);
    write_file(&photos.join("sub/b.txt"), b"hello");
    let top = src.path().join("note.txt");
    write_file(&top, b"top");

    let batch_id = push::create_batch(&s.state, CLIENT_ID, vec![top, photos])
        .await
        .unwrap();

    let session = ClientSession::new(
        &format!("http://{}", s.addr),
        CLIENT_ID.to_string(),
        "测试客户端".into(),
    )
    .unwrap();
    session.login(USER, PASS).await.unwrap();
    let manifest = session.fetch_manifest(&batch_id).await.unwrap();

    let receive = tempfile::tempdir().unwrap();
    // 预先放一个同名目录，验证不会覆盖而是变成 "Photos (1)"。
    std::fs::create_dir(receive.path().join("Photos")).unwrap();

    let outcome = client::run_batch(&session, &manifest, receive.path(), &|_| {})
        .await
        .unwrap();
    assert!(outcome.errors.is_empty(), "{:?}", outcome.errors);

    let got = std::fs::read(receive.path().join("Photos (1)/a.bin")).unwrap();
    assert_eq!(sha256(&got), sha256(&a_data));
    assert_eq!(
        std::fs::read(receive.path().join("Photos (1)/sub/b.txt")).unwrap(),
        b"hello"
    );
    assert!(receive.path().join("Photos (1)/empty").is_dir());
    assert_eq!(
        std::fs::read(receive.path().join("note.txt")).unwrap(),
        b"top"
    );
    assert!(s.state.pushes.lock().unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn push_batch_expires_after_idle() {
    let s = start().await;
    s.login().await;
    let _offers = register_client(&s, CLIENT_ID);

    let src = tempfile::tempdir().unwrap();
    let file = src.path().join("a.txt");
    write_file(&file, b"x");
    let batch_id = push::create_batch(&s.state, CLIENT_ID, vec![file])
        .await
        .unwrap();

    {
        let mut pushes = s.state.pushes.lock().unwrap();
        let batch = pushes.get_mut(&batch_id).unwrap();
        batch.last_activity =
            std::time::Instant::now() - Duration::from_secs(PUSH_IDLE_TTL_SECS + 1);
    }
    s.state.purge_stale();
    assert!(s.state.pushes.lock().unwrap().is_empty());

    let res = s
        .client
        .get(s.url(&format!("/api/push/{batch_id}/manifest")))
        .header(CLIENT_ID_HEADER, CLIENT_ID)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), reqwest::StatusCode::NOT_FOUND);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn push_to_offline_client_fails() {
    let s = start().await;
    let src = tempfile::tempdir().unwrap();
    let file = src.path().join("a.txt");
    write_file(&file, b"x");

    let result = push::create_batch(&s.state, "offline-client-0001", vec![file]).await;
    assert!(result.is_err());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pending_lists_unfinished_batches() {
    let s = start().await;
    s.login().await;
    let _offers = register_client(&s, CLIENT_ID);

    let src = tempfile::tempdir().unwrap();
    let file = src.path().join("a.txt");
    write_file(&file, b"x");
    let batch_id = push::create_batch(&s.state, CLIENT_ID, vec![file])
        .await
        .unwrap();

    let pending: serde_json::Value = s
        .client
        .get(s.url("/api/push/pending"))
        .header(CLIENT_ID_HEADER, CLIENT_ID)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(pending["batches"]
        .as_array()
        .unwrap()
        .iter()
        .any(|b| b["batch_id"].as_str() == Some(batch_id.as_str())));

    let res = s
        .client
        .post(s.url(&format!("/api/push/{batch_id}/complete")))
        .header(CLIENT_ID_HEADER, CLIENT_ID)
        .header("x-wiretoss", "1")
        .json(&json!({ "ok": true }))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), reqwest::StatusCode::OK);

    let pending: serde_json::Value = s
        .client
        .get(s.url("/api/push/pending"))
        .header(CLIENT_ID_HEADER, CLIENT_ID)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(pending["batches"].as_array().unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn push_file_endpoint_rejects_directories() {
    let s = start().await;
    s.login().await;
    let _offers = register_client(&s, CLIENT_ID);

    let src = tempfile::tempdir().unwrap();
    let dir = src.path().join("empty");
    std::fs::create_dir(&dir).unwrap();
    let batch_id = push::create_batch(&s.state, CLIENT_ID, vec![dir])
        .await
        .unwrap();

    let manifest: serde_json::Value = s
        .client
        .get(s.url(&format!("/api/push/{batch_id}/manifest")))
        .header(CLIENT_ID_HEADER, CLIENT_ID)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let index = manifest["items"].as_array().unwrap()[0]["index"]
        .as_u64()
        .unwrap();

    let res = s
        .client
        .get(s.url(&format!("/api/push/{batch_id}/file/{index}")))
        .header(CLIENT_ID_HEADER, CLIENT_ID)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), reqwest::StatusCode::BAD_REQUEST);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn client_events_registers_and_unregisters() {
    let s = start().await;
    s.login().await;
    let id = "sse-client-0001";

    let res = s
        .client
        .get(s.url("/api/client/events"))
        .query(&[("name", "测试客户端")])
        .header(CLIENT_ID_HEADER, id)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), reqwest::StatusCode::OK);
    {
        let clients = s.state.clients.lock().unwrap();
        let client = clients.get(id).expect("SSE 连接应已登记");
        assert_eq!(client.info.name, "测试客户端");
    }

    drop(res);

    // 断开的 TCP 连接往往要等到下一次写才会被发现；反复投一条 notice
    // 给它制造写机会，避免测试干等 15 秒的 keep-alive。
    let mut gone = false;
    for _ in 0..400 {
        let _ = wiretoss::clients::send_notice(
            &s.state,
            id,
            PushNotice::PushOffer {
                batch_id: "batch-abc-123".into(),
                name: "x".into(),
                total_files: 0,
                total_bytes: 0,
            },
        );
        if !s.state.clients.lock().unwrap().contains_key(id) {
            gone = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(gone, "SSE 断开后客户端应被注销");
}
