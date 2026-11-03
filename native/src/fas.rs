//! ZenJooo · Read-only FAS discovery. Only the documented fas-rs mode interface is writable.
use std::fs;
use std::io::{ErrorKind, Read, Write};
use std::path::Path;

pub const MODE_NODE: &str = "dev/fas_rs/mode";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Status {
    pub state: &'static str,
    pub mode: String,
    pub sync: &'static str,
    pub error: String,
}

impl Status {
    pub fn available(&self) -> bool {
        matches!(self.state, "ready" | "running")
    }
    pub fn report(&self) -> String {
        format!("fas_state={}\nfas_mode={}\nfas_sync={}\nfas_node=/dev/fas_rs/mode\nfas_error={}\n",
            self.state, self.mode, self.sync, self.error.replace(['\n', '\r'], " "))
    }
}

pub fn inspect(root: &Path) -> Status {
    let mut status = Status { state: "unknown", mode: String::new(), sync: "unavailable", error: String::new() };
    let path = root.join(MODE_NODE);
    match fs::metadata(&path) {
        Ok(meta) if meta.is_file() => {
            let mode = fs::File::open(&path).and_then(|file| {
                let mut value = String::new();
                file.take(128).read_to_string(&mut value)?;
                Ok(value.trim().to_string())
            });
            match mode {
                Ok(mode) if crate::app_modes::supported(&mode) => {
                    status.state = "ready";
                    status.mode = mode;
                    status.sync = "pending";
                }
                Ok(_) => { status.state = "invalid"; status.error = "模式接口没有返回有效四档模式".into(); }
                Err(error) => { status.state = "unreadable"; status.error = error.to_string(); }
            }
        }
        Ok(_) => { status.state = "invalid"; status.error = "模式接口不是可读取的普通文件".into(); }
        Err(error) if error.kind() == ErrorKind::NotFound => {
            match process_running(&root.join("proc")) {
                Some(true) => status.state = "running",
                Some(false) => status.state = "absent",
                None => status.error = "模式接口不存在，进程检测暂不可用".into(),
            }
        }
        Err(error) => { status.state = "unreadable"; status.error = error.to_string(); }
    }
    status
}

pub fn write_mode(path: &Path, mode: &str) -> crate::util::Result<()> {
    if !crate::app_modes::supported(mode) { return Err("FAS 档位无效".into()); }
    // This is an ordinary file, unlike sysfs. Shorter names must truncate the
    // old value; writing "fast" over "performance" otherwise leaves "fastormance".
    // Do not create a node: its lifecycle belongs to the FAS process.
    let mut file = fs::OpenOptions::new().write(true).open(path).map_err(|error| error.to_string())?;
    if !file.metadata().map_err(|error| error.to_string())?.is_file() { return Err("FAS 模式接口类型无效".into()); }
    file.set_len(0).map_err(|error| error.to_string())?;
    file.write_all(mode.as_bytes()).map_err(|error| error.to_string())
}

fn process_running(proc: &Path) -> Option<bool> {
    match running_pids_in(proc) {
        Some(pids) => Some(!pids.is_empty()),
        None => None,
    }
}

/// 所有正在运行的 fas-rs 进程 pid。NovaFAS 接管时按配置对其 SIGSTOP，
/// 释放时 SIGCONT；读取失败的目录不影响其余进程的检测。
pub fn running_pids() -> Vec<i32> {
    running_pids_in(Path::new("/proc")).unwrap_or_default()
}

/// 校验单个 PID 当前是否是 fas-rs 进程（判定标准与 `running_pids` 一致：
/// cmdline 首段的文件名）。配合 pidfd 使用时它只负责减少无谓的 fd 打开；
/// PID 复用防护由 pidfd 本身保证，不依赖本函数的时效。
pub fn looks_like_fas_pid(pid: i32) -> bool {
    let bytes = fs::read(format!("/proc/{pid}/cmdline")).unwrap_or_default();
    let executable = bytes.split(|b| *b == 0).next().unwrap_or_default();
    Path::new(std::str::from_utf8(executable).unwrap_or(""))
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| matches!(name, "fas-rs" | "fas_rs"))
}

fn running_pids_in(proc: &Path) -> Option<Vec<i32>> {
    let entries = fs::read_dir(proc).ok()?;
    let mut pids = Vec::new();
    let mut unreadable = false;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(digits) = name.to_string_lossy().bytes().all(|c| c.is_ascii_digit())
            .then(|| name.to_string_lossy().to_string())
        else {
            continue;
        };
        let bytes = match fs::read(entry.path().join("cmdline")) {
            Ok(bytes) => bytes,
            Err(error) => { unreadable |= error.kind() != ErrorKind::NotFound; continue; }
        };
        let executable = bytes.split(|b| *b == 0).next().unwrap_or_default();
        let executable = Path::new(std::str::from_utf8(executable).unwrap_or(""))
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("");
        if matches!(executable, "fas-rs" | "fas_rs") {
            if let Ok(pid) = digits.parse::<i32>() {
                pids.push(pid);
            }
        }
    }
    if unreadable && pids.is_empty() { None } else { Some(pids) }
}

pub fn diagnose() -> crate::util::Result<()> {
    if unsafe { crate::ffi::geteuid() } != 0 { return Err("必须以 root 身份运行".into()); }
    print!("{}", inspect(Path::new("/")).report());
    println!("读取接口或进程只能证明已检测到 FAS，不能证明游戏正在使用帧感知调度。");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture(name: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!("nova-fas-{name}-{}", std::process::id()));
        fs::create_dir_all(path.join("proc/123")).unwrap();
        path
    }
    #[test]
    fn enabling_after_start_and_recreating_node_refreshes_readiness() {
        let root = fixture("late");
        assert_eq!(inspect(&root).state, "absent");
        fs::create_dir_all(root.join("dev/fas_rs")).unwrap();
        fs::write(root.join(MODE_NODE), "balance\n").unwrap();
        assert_eq!(inspect(&root).mode, "balance");
        fs::remove_file(root.join(MODE_NODE)).unwrap();
        assert_eq!(inspect(&root).state, "absent");
        fs::write(root.join(MODE_NODE), "fast").unwrap();
        assert_eq!(inspect(&root).mode, "fast");
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn process_detection_is_exact_and_does_not_claim_mode_sync() {
        let root = fixture("process");
        fs::write(root.join("proc/123/cmdline"), b"/data/fake-fas-rs\0fas-rs\0").unwrap();
        assert_eq!(inspect(&root).state, "absent");
        fs::write(root.join("proc/123/cmdline"), b"/data/scene/fas-rs\0run\0").unwrap();
        let status = inspect(&root);
        assert!(status.available());
        assert_eq!(status.state, "running");
        assert_eq!(status.sync, "unavailable");
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn invalid_or_non_file_nodes_are_never_ready() {
        let root = fixture("invalid");
        fs::create_dir_all(root.join("dev/fas_rs")).unwrap();
        fs::write(root.join(MODE_NODE), "\n").unwrap();
        assert_eq!(inspect(&root).state, "invalid");
        fs::remove_file(root.join(MODE_NODE)).unwrap();
        fs::create_dir(root.join(MODE_NODE)).unwrap();
        assert_eq!(inspect(&root).state, "invalid");
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn every_mode_transition_truncates_old_text_and_never_creates_a_node() {
        let root = fixture("transitions");
        let path = root.join(MODE_NODE);
        assert!(write_mode(&path, "fast").is_err());
        assert!(!path.exists());
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        for from in crate::config::MODES {
            for to in crate::config::MODES {
                fs::write(&path, from).unwrap();
                write_mode(&path, to).unwrap();
                assert_eq!(fs::read_to_string(&path).unwrap(), to);
                assert_eq!(inspect(&root).mode, to);
            }
        }
        fs::remove_dir_all(root).unwrap();
    }

}
