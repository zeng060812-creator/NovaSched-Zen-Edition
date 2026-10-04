use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex};

use crate::util;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Debug = 0,
    Info = 1,
    Warn = 2,
    Error = 3,
}

impl Level {
    pub fn parse(value: &str) -> Self {
        match value.trim().to_ascii_uppercase().as_str() {
            "DEBUG" => Self::Debug,
            "WARN" | "WARNING" | "WARNNING" => Self::Warn,
            "ERROR" => Self::Error,
            _ => Self::Info,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Debug => "调试 ->",
            Self::Info => "信息 ->",
            Self::Warn => "警告 ->",
            Self::Error => "错误 ->",
        }
    }
}

struct Inner {
    mirror: PathBuf,
    persistent: PathBuf,
    level: AtomicU8,
    lock: Mutex<BTreeMap<PathBuf, String>>,
}

#[derive(Clone)]
pub struct Logger {
    inner: Arc<Inner>,
}

impl Logger {
    pub fn new(state_dir: &Path) -> Self {
        Self {
            inner: Arc::new(Inner {
                mirror: PathBuf::from("/sdcard/Android/NovaSched/log.txt"),
                persistent: state_dir.join("novasched.log"),
                level: AtomicU8::new(Level::Info as u8),
                lock: Mutex::new(BTreeMap::new()),
            }),
        }
    }

    pub fn clear(&self) {
        self.info("开始新的运行会话；保留上次退出记录");
    }
    pub fn persistent_path(&self) -> &Path {
        &self.inner.persistent
    }

    pub fn set_level(&self, level: &str) {
        self.inner
            .level
            .store(Level::parse(level) as u8, Ordering::Relaxed);
    }

    pub fn debug(&self, message: impl AsRef<str>) {
        self.log(Level::Debug, message.as_ref());
    }

    pub fn info(&self, message: impl AsRef<str>) {
        self.log(Level::Info, message.as_ref());
    }

    pub fn warn(&self, message: impl AsRef<str>) {
        self.log(Level::Warn, message.as_ref());
    }

    pub fn error(&self, message: impl AsRef<str>) {
        self.log(Level::Error, message.as_ref());
    }

    pub fn log(&self, level: Level, message: &str) {
        if (level as u8) < self.inner.level.load(Ordering::Relaxed) {
            return;
        }
        let line = format!(
            "{} {} {}\n",
            util::local_timestamp(),
            level.label(),
            message
        );
        let Ok(mut failures) = self.inner.lock.lock() else {
            eprintln!("novasched: 日志互斥锁损坏: {message}");
            return;
        };
        let mut wrote = false;
        for path in [&self.inner.persistent, &self.inner.mirror] {
            // Shared storage is only a convenience mirror.  It may not be
            // mounted yet at boot, and should never make the daemon noisy or
            // unhealthy; the protected /data/adb log is authoritative.
            let result = if path == &self.inner.mirror && !Path::new("/sdcard/Android").is_dir() {
                continue;
            } else {
                append_bounded(path, &line)
            };
            match result {
                Ok(()) => {
                    wrote = true;
                    failures.remove(path);
                }
                Err(error) => {
                    let detail = error.to_string();
                    if path == &self.inner.mirror {
                        // Android scoped-storage and delayed emulated storage
                        // are normal. Silently keep only the internal log.
                        failures.insert(path.clone(), detail);
                    } else if failures.get(path) != Some(&detail) {
                        eprintln!("novasched: 日志 {} 写入失败: {detail}", path.display());
                        failures.insert(path.clone(), detail);
                    }
                }
            }
        }
        if !wrote {
            eprint!("{line}");
        }
    }
}

fn append_bounded(path: &Path, line: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    match fs::metadata(path) {
        Ok(m) if m.len() >= 1024 * 1024 => fs::rename(path, path.with_extension("log.1"))?,
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?
        .write_all(line.as_bytes())
}

#[cfg(test)]
mod regression_tests {
    use super::*;
    #[test]
    fn logs_recreate_parent_and_rotate_instead_of_erasing_history() {
        let dir = std::env::temp_dir().join(format!("novasched-logs-{}", std::process::id()));
        let path = dir.join("nested/novasched.log");
        append_bounded(&path, "previous\n").unwrap();
        append_bounded(&path, "current\n").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "previous\ncurrent\n");
        fs::write(&path, vec![b'x'; 1024 * 1024]).unwrap();
        append_bounded(&path, "after-rotation\n").unwrap();
        assert_eq!(
            fs::metadata(path.with_extension("log.1")).unwrap().len(),
            1024 * 1024
        );
        assert_eq!(fs::read_to_string(&path).unwrap(), "after-rotation\n");
        fs::remove_dir_all(dir).unwrap();
    }
}
