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
                persistent: crate::storage::path(&state_dir, "novasched.log"),
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
        let path=&self.inner.persistent;
        match append_bounded(path,&line){
            Ok(())=>{failures.remove(path);},
            Err(error)=>{let detail=error.to_string();if failures.get(path)!=Some(&detail){eprintln!("novasched: 日志 {} 写入失败: {detail}",path.display());failures.insert(path.clone(),detail);}eprint!("{line}");}
        }
    }
}

pub fn append_bounded(path:&Path,line:&str)->std::io::Result<()> {
    use std::io::{Read,Seek,SeekFrom};
    use std::os::unix::fs::OpenOptionsExt;
    const LIMIT:usize=1024*1024;
    if let Some(parent)=path.parent(){fs::create_dir_all(parent)?;}
    if fs::symlink_metadata(path).is_ok_and(|m|!m.is_file()||m.file_type().is_symlink()){return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput,"日志路径不是普通文件"));}
    let mut bytes=line.as_bytes();
    if bytes.len()>LIMIT/2{let mut start=bytes.len()-LIMIT/2;while !line.is_char_boundary(start){start+=1;}bytes=&bytes[start..];}
    // O_APPEND keeps concurrent writers (daemon, installer, uninstaller,
    // legacy cleanup) from overwriting each other's lines: without it the
    // seek-to-end + write pair is not atomic across processes.
    let mut file=OpenOptions::new().create(true).read(true).append(true).mode(0o600).open(path)?;
    let len=file.metadata()?.len() as usize;
    if len.saturating_add(bytes.len())>LIMIT{
        let keep=(LIMIT/2).min(len);file.seek(SeekFrom::End(-(keep as i64)))?;
        let mut tail=Vec::with_capacity(keep);file.read_to_end(&mut tail)?;
        let start=tail.iter().position(|c|*c==b'\n').map_or(tail.len(),|i|i+1);
        file.set_len(0)?;file.write_all(&tail[start..])?;
    }
    file.write_all(bytes)
}
#[cfg(test)]mod regression_tests{use super::*;#[test]fn log_keeps_recent_lines_bounded_without_backups(){let dir=std::env::temp_dir().join(format!("nova-log-{}",std::process::id()));let p=dir.join("nested/novasched.log");append_bounded(&p,"old\n").unwrap();append_bounded(&p,"new\n").unwrap();assert_eq!(fs::read_to_string(&p).unwrap(),"old\nnew\n");fs::write(&p,"line\n".repeat(210000)).unwrap();append_bounded(&p,"latest\n").unwrap();assert!(fs::read_to_string(&p).unwrap().ends_with("latest\n"));assert!(fs::metadata(&p).unwrap().len()<=1048576);assert!(!p.with_extension("log.1").exists());append_bounded(&p,&"长".repeat(600000)).unwrap();assert!(fs::read_to_string(&p).unwrap().len()<=1048576);fs::remove_dir_all(dir).unwrap();}}
