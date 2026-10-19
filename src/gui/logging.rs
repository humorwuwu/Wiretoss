//! 把服务端的 tracing 日志接到窗口的日志面板，同时落一份到文件。
//!
//! GUI 模式下程序没有终端可写（Windows 上连控制台都没有），所以日志必须进窗口。
//! 但只放在内存里也不行：出问题时用户重启一下就什么都没了，没法发给你看。
//! 所以同一个 writer 同时往两个地方写。

use std::io::{self, Write};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};

use tracing_subscriber::fmt::MakeWriter;

/// 日志文件超过这个大小就在启动时清空。
/// 这是个长期驻留的后台服务，不设上限的话日志会一直涨。
const MAX_LOG_BYTES: u64 = 1024 * 1024;

pub struct GuiWriter {
    tx: Sender<String>,
    notify: Arc<dyn Fn() + Send + Sync>,
    file: Option<Arc<Mutex<std::fs::File>>>,
    /// tracing 不保证一次 write 就是一整行，得自己攒。
    buf: Vec<u8>,
}

#[derive(Clone)]
pub struct GuiMakeWriter {
    tx: Sender<String>,
    notify: Arc<dyn Fn() + Send + Sync>,
    file: Option<Arc<Mutex<std::fs::File>>>,
}

pub fn init(tx: Sender<String>, notify: Arc<dyn Fn() + Send + Sync>) {
    let writer = GuiMakeWriter {
        tx,
        notify,
        file: open_log_file(),
    };

    // 已经设过全局 subscriber 就算了（例如测试里重复调用）。
    let _ = tracing_subscriber::fmt()
        .with_writer(writer)
        .with_target(false)
        .with_ansi(false)
        .with_timer(crate::logging::timer())
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "wiretoss=info,tower_http=warn".into()),
        )
        .try_init();
}

fn open_log_file() -> Option<Arc<Mutex<std::fs::File>>> {
    let path = crate::config::config_dir().ok()?.join("wiretoss.log");
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).ok()?;
    }

    let too_big = std::fs::metadata(&path)
        .map(|m| m.len() > MAX_LOG_BYTES)
        .unwrap_or(false);
    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(!too_big)
        .write(true)
        .truncate(too_big)
        .open(&path)
        .ok()?;

    Some(Arc::new(Mutex::new(file)))
}

impl GuiWriter {
    fn take_line(&mut self) -> Option<String> {
        let pos = self.buf.iter().position(|b| *b == b'\n')?;
        let line: Vec<u8> = self.buf.drain(..=pos).collect();
        Some(String::from_utf8_lossy(&line).trim().to_string())
    }

    fn emit(&mut self, text: String) {
        if text.is_empty() {
            return;
        }
        if let Some(file) = &self.file {
            if let Ok(mut file) = file.lock() {
                let _ = writeln!(file, "{text}");
            }
        }
        if self.tx.send(text).is_ok() {
            (self.notify)();
        }
    }
}

impl Write for GuiWriter {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        self.buf.extend_from_slice(data);
        while let Some(line) = self.take_line() {
            self.emit(line);
        }
        Ok(data.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        let rest = String::from_utf8_lossy(&self.buf).trim().to_string();
        self.buf.clear();
        self.emit(rest);
        Ok(())
    }
}

impl<'a> MakeWriter<'a> for GuiMakeWriter {
    type Writer = GuiWriter;

    fn make_writer(&'a self) -> Self::Writer {
        GuiWriter {
            tx: self.tx.clone(),
            notify: self.notify.clone(),
            file: self.file.clone(),
            buf: Vec::new(),
        }
    }
}
