//! Never send a signal using a saved PID alone.
use crate::{
    ffi,
    util::{self, Result},
};
use std::fs::{self, File, OpenOptions};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::fs::MetadataExt;
use std::path::Path;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Identity {
    pub pid: i32,
    pub start: u64,
    pub boot: String,
    pub device: u64,
    pub inode: u64,
}
impl Identity {
    pub fn capture(pid: i32) -> Result<Self> {
        if pid <= 1 {
            return Err("拒绝无效或系统 PID".into());
        }
        let start = parse_start(&util::read_text(Path::new(&format!("/proc/{pid}/stat")))?)?;
        let meta = fs::metadata(format!("/proc/{pid}/exe")).map_err(|e| e.to_string())?;
        Ok(Self {
            pid,
            start,
            boot: util::read_trimmed("/proc/sys/kernel/random/boot_id")?,
            device: meta.dev(),
            inode: meta.ino(),
        })
    }
    pub fn encode(&self) -> String {
        format!(
            "CTS_PROCESS_V1\n{}\n{}\n{}\n{}\n{}\n",
            self.pid, self.start, self.boot, self.device, self.inode
        )
    }
    pub fn parse(text: &str) -> Result<Self> {
        let f: Vec<_> = text.lines().collect();
        if f.len() != 6 || f[0] != "CTS_PROCESS_V1" {
            return Err("进程身份记录无效；旧版升级需重启手机".into());
        }
        let id = Self {
            pid: f[1].parse().map_err(|_| "PID 无效")?,
            start: f[2].parse().map_err(|_| "starttime 无效")?,
            boot: f[3].into(),
            device: f[4].parse().map_err(|_| "设备号无效")?,
            inode: f[5].parse().map_err(|_| "inode 无效")?,
        };
        if id.pid <= 1 || id.start == 0 || id.boot.is_empty() {
            return Err("进程身份不安全".into());
        }
        Ok(id)
    }
}
fn parse_start(stat: &str) -> Result<u64> {
    let tail = stat.rsplit_once(") ").ok_or("stat 格式错误")?.1;
    let values: Vec<_> = tail.split_whitespace().collect();
    if matches!(values.first(), Some(&"Z") | Some(&"X")) {
        return Err("守护进程已退出".into());
    }
    values
        .get(19)
        .ok_or("stat 缺少 starttime")?
        .parse()
        .map_err(|_| "starttime 无效".into())
}
pub struct VerifiedProcess {
    pub identity: Identity,
    fd: File,
}
impl VerifiedProcess {
    pub fn signal(&self, signal: i32) -> Result<()> {
        if unsafe {
            ffi::syscall(
                424,
                self.fd.as_raw_fd(),
                signal,
                std::ptr::null::<u8>(),
                0u32,
            )
        } < 0
        {
            return Err(ffi::errno_message("pidfd 信号失败；未使用不安全 PID 回退"));
        }
        Ok(())
    }
}
pub fn verify(state: &Path, module: &Path) -> Result<VerifiedProcess> {
    let saved = Identity::parse(&util::read_text(&state.join("daemon.identity"))?)?;
    let raw = unsafe { ffi::syscall(434, saved.pid, 0u32) };
    if raw < 0 {
        return Err(ffi::errno_message("获取进程 pidfd 失败"));
    }
    let fd = unsafe { File::from_raw_fd(raw as i32) };
    if saved != Identity::capture(saved.pid)? {
        return Err(format!("过期 PID {} 已复用或来自上次开机", saved.pid));
    }
    let actual = fs::read_link(format!("/proc/{}/exe", saved.pid)).map_err(|e| e.to_string())?;
    let text = actual.to_string_lossy();
    let actual = text.strip_suffix(" (deleted)").unwrap_or(&text);
    let expected = fs::canonicalize(module.join("bin/novasched")).map_err(|e| e.to_string())?;
    if Path::new(actual) != expected {
        return Err("PID 指向其它 ELF，拒绝控制".into());
    }
    let cmd = fs::read(format!("/proc/{}/cmdline", saved.pid)).map_err(|e| e.to_string())?;
    if cmd.split(|b| *b == 0).nth(1) != Some(b"daemon".as_slice()) {
        return Err("PID 不是调度守护".into());
    }
    if !lock_held(&state.join("daemon.lock"))? {
        return Err("守护没有持有排他锁".into());
    }
    let process = VerifiedProcess {
        identity: saved,
        fd,
    };
    process.signal(0)?;
    Ok(process)
}
pub fn lock_held(path: &Path) -> Result<bool> {
    let file = match OpenOptions::new().read(true).open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(e.to_string()),
    };
    if unsafe { ffi::flock(file.as_raw_fd(), ffi::LOCK_EX | ffi::LOCK_NB) } == 0 {
        return Ok(false);
    }
    let error = std::io::Error::last_os_error();
    if error.kind() == std::io::ErrorKind::WouldBlock {
        Ok(true)
    } else {
        Err(error.to_string())
    }
}
pub fn self_test() -> Result<()> {
    let identity = Identity::capture(util::current_pid())?;
    let raw = unsafe { ffi::syscall(434, identity.pid, 0u32) };
    if raw < 0 {
        return Err(ffi::errno_message("内核/权限不支持 pidfd"));
    }
    VerifiedProcess {
        identity,
        fd: unsafe { File::from_raw_fd(raw as i32) },
    }
    .signal(0)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_reused_and_legacy_pids() {
        let a = Identity {
            pid: 8980,
            start: 123,
            boot: "boot".into(),
            device: 1,
            inode: 10,
        };
        assert_eq!(Identity::parse(&a.encode()).unwrap(), a);
        let mut b = a.clone();
        b.start += 1;
        assert_ne!(a, b);
        assert!(Identity::parse("8980\n").is_err());
        assert!(Identity::parse(&a.encode().replace("8980", "0")).is_err());
    }
    #[test]
    fn parses_complex_stat_names() {
        let mut f = vec!["S"; 20];
        f[19] = "98765";
        assert_eq!(
            parse_start(&format!("42 (odd ) name) {}", f.join(" "))).unwrap(),
            98765
        );
        f[0] = "Z";
        assert!(parse_start(&format!("42 (dead) {}", f.join(" "))).is_err());
    }
    #[test]
    fn lock_observed_without_deleting_it() {
        let p = std::env::temp_dir().join(format!("cts-lock-{}", std::process::id()));
        let lock = util::acquire_flock(&p).unwrap();
        assert!(lock_held(&p).unwrap());
        assert!(util::acquire_flock(&p).is_err());
        drop(lock);
        assert!(!lock_held(&p).unwrap());
        fs::remove_file(p).unwrap();
    }
}

#[cfg(test)]
mod regression_tests {
    use super::*;
    #[test]
    fn live_unrelated_process_is_not_accepted_as_daemon() {
        let dir = std::env::temp_dir().join(format!("cts-identity-{}", util::current_pid()));
        util::create_dir(&dir.join("bin")).unwrap();
        fs::write(dir.join("bin/novasched"), b"not this executable").unwrap();
        let id = Identity::capture(util::current_pid()).unwrap();
        util::atomic_write(&dir.join("daemon.identity"), id.encode().as_bytes(), 0o600).unwrap();
        let lock = util::acquire_flock(&dir.join("daemon.lock")).unwrap();
        assert!(verify(&dir, &dir).is_err());
        assert_eq!(Identity::capture(id.pid).unwrap(), id);
        drop(lock);
        fs::remove_dir_all(dir).unwrap();
    }
}
