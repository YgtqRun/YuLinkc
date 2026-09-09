//! 极简轮转文件日志。
//!
//! 写入 `<config>/logs/yulink.log`，单文件超过 `MAX_FILE_BYTES` 后滚动为
//! `yulink.log.1 / .2 / .3`（最多保留 `MAX_FILES` 份）。日志仅作运行记录，
//! 不写入任何明文凭据。

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use log::{LevelFilter, Log, Metadata, Record};
use time::OffsetDateTime;

const LOG_FILE: &str = "yulink.log";
const MAX_FILE_BYTES: u64 = 512 * 1024;
const MAX_FILES: usize = 3;

struct Inner {
    dir: PathBuf,
    file: Option<File>,
}

pub struct FileLogger {
    inner: Mutex<Inner>,
    level: LevelFilter,
}

impl FileLogger {
    /// 初始化日志。目录不存在会自动创建；已超限的当日文件先滚动再写入。
    pub fn init(dir: &Path, level: LevelFilter) -> Result<(), String> {
        fs::create_dir_all(dir).map_err(|e| format!("创建日志目录失败: {e}"))?;
        rotate_if_full(dir);

        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join(LOG_FILE))
            .map_err(|e| format!("打开日志文件失败: {e}"))?;

        let logger = Box::new(FileLogger {
            inner: Mutex::new(Inner {
                dir: dir.to_path_buf(),
                file: Some(file),
            }),
            level,
        });
        log::set_boxed_logger(logger).map_err(|e| format!("日志初始化失败: {e}"))?;
        log::set_max_level(level);
        Ok(())
    }
}

impl Log for FileLogger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        metadata.level() <= self.level
    }

    fn log(&self, record: &Record) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let Ok(mut inner) = self.inner.lock() else {
            return;
        };

        // 当前文件超限时：先关闭旧句柄，再滚动，最后重开新文件
        let over_limit = inner
            .file
            .as_ref()
            .and_then(|f| f.metadata().ok())
            .map(|m| m.len() > MAX_FILE_BYTES)
            .unwrap_or(false);
        if over_limit {
            inner.file.take();
            rotate_if_full(&inner.dir);
            inner.file = OpenOptions::new()
                .create(true)
                .append(true)
                .open(inner.dir.join(LOG_FILE))
                .ok();
        }

        let Some(file) = inner.file.as_mut() else {
            return;
        };
        // 使用本地时区时间（获取失败时回退 UTC，避免日志中断）。
        let ts = OffsetDateTime::now_local()
            .unwrap_or_else(|_| OffsetDateTime::now_utc())
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap_or_else(|_| "unknown-time".into());
        let line = format!(
            "[{:<5} {}] {}\r\n",
            record.level().as_str(),
            ts,
            record.args()
        );
        let _ = file.write_all(line.as_bytes());
    }

    fn flush(&self) {
        if let Ok(mut inner) = self.inner.lock() {
            if let Some(f) = inner.file.as_mut() {
                let _ = f.flush();
            }
        }
    }
}

/// 若当前日志已超限，则依次滚动 .2 -> .3、.1 -> .2、当前 -> .1。
fn rotate_if_full(dir: &Path) {
    let current = dir.join(LOG_FILE);
    let is_full = fs::metadata(&current)
        .map(|m| m.len() > MAX_FILE_BYTES)
        .unwrap_or(false);
    if !is_full {
        return;
    }

    // 删除最老的一份
    let _ = fs::remove_file(dir.join(format!("{LOG_FILE}.{MAX_FILES}")));
    for i in (1..MAX_FILES).rev() {
        let from = dir.join(format!("{LOG_FILE}.{i}"));
        let to = dir.join(format!("{LOG_FILE}.{}", i + 1));
        if from.exists() {
            let _ = fs::rename(from, to);
        }
    }
    let _ = fs::rename(&current, dir.join(format!("{LOG_FILE}.1")));
}
