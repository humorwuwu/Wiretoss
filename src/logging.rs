//! 日志时间戳的本地时区处理。
//!
//! `time` crate 只有在**单线程**阶段才拿得到本地时区偏移
//! （多线程下读时区环境变量是不安全的，它会直接返回 Err）。
//! 而我们的程序一起来就有一堆线程：tokio 运行时、GUI 事件循环、服务线程……
//!
//! 所以做法是：在 `main()` 的第一行、任何线程起来之前就把偏移量抓下来存住，
//! 后面 tracing 初始化时直接用这个值。不这么做的话日志时间会全是 UTC，
//! 对一个给用户看的日志来说，差 8 小时等于没法看。

use std::sync::OnceLock;

use time::format_description::well_known::Rfc3339;
use time::UtcOffset;
use tracing_subscriber::fmt::time::OffsetTime;

static LOCAL_OFFSET: OnceLock<UtcOffset> = OnceLock::new();

/// 必须在 `main()` 最开头调用，早于任何线程的创建。
pub fn capture_local_offset() {
    // 拿不到就退回 UTC —— 总比 panic 强，只是时间戳不准而已。
    let offset = UtcOffset::current_local_offset().unwrap_or(UtcOffset::UTC);
    let _ = LOCAL_OFFSET.set(offset);
}

/// 给 tracing 用的定时器，输出形如 `2026-10-18T14:09:17+08:00`。
pub fn timer() -> OffsetTime<Rfc3339> {
    OffsetTime::new(*LOCAL_OFFSET.get().unwrap_or(&UtcOffset::UTC), Rfc3339)
}
