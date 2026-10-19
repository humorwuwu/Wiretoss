//! Wiretoss —— Mac <-> Windows 局域网文件快传。
//!
//! 拆成 lib + bin 是为了让 `tests/` 下的集成测试能拿到 [`router::build_router`]，
//! 从而在真实 HTTP 栈上跑完整链路（登录 -> 分片上传 -> 下载校验），
//! 而不是只测各个函数。二进制入口只负责解析参数和打印启动信息。

pub mod assets;
pub mod auth;
pub mod client;
pub mod clients;
pub mod config;
pub mod download;
pub mod error;
pub mod events;
pub mod fs_api;
pub mod gui;
pub mod logging;
pub mod net;
pub mod push;
pub mod router;
pub mod state;
pub mod upload;
