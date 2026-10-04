use std::ffi::CStr;
#[cfg(target_os = "android")]
use std::ffi::CString;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::mem::MaybeUninit;
use std::os::fd::AsRawFd;
use std::os::raw::{c_char, c_long};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use crate::ffi;

pub const MODULE_DIR_DEFAULT: &str = "/data/adb/modules/NovaSched_Zen_Edition";
pub const STATE_DIR: &str = "/data/adb/novasched";
pub const RUNTIME_DIR: &str = STATE_DIR;
pub const CONFIG_PATH: &str = "/data/adb/novasched/config.json";
pub const MODE_PATH: &str = "/data/adb/novasched/mode.txt";
pub const SCENE_REQUEST_PATH: &str = "/data/adb/novasched/scene-request.txt";
pub const APP_MODES_PATH: &str = "/data/adb/novasched/app_modes.txt";
pub const OPTIONS_PATH: &str = "/data/adb/novasched/options.txt";

pub type Result<T> = std::result::Result<T, String>;
/// The constant is a last-resort fallback, not a root-framework path gate.
pub fn default_module_dir() -> PathBuf {
    if let Ok(executable) = std::env::current_exe() {
        if let Some(module) = executable
            .parent()
            .filter(|p| p.file_name().is_some_and(|name| name == "bin"))
            .and_then(Path::parent)
        {
            match fs::read_to_string(module.join("module.prop")) {
                Ok(text) if text.lines().any(|line| line == "id=NovaSched_Zen_Edition") => {
                    return module.into()
                }
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => eprintln!("novasched: 读取可执行文件所属模块信息失败: {error}"),
            }
        }
    }
    PathBuf::from(MODULE_DIR_DEFAULT)
}
static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

pub fn create_dir(path: &Path) -> Result<()> {
    fs::create_dir_all(path).map_err(|e| format!("创建目录 {} 失败: {e}", path.display()))
}

pub fn read_bytes(path: &Path) -> Result<Vec<u8>> {
    fs::read(path).map_err(|e| format!("读取 {} 失败: {e}", path.display()))
}

pub fn read_text(path: &Path) -> Result<String> {
    fs::read_to_string(path).map_err(|e| format!("读取 {} 失败: {e}", path.display()))
}

pub fn read_trimmed(path: impl AsRef<Path>) -> Result<String> {
    Ok(read_text(path.as_ref())?.trim().to_string())
}

pub fn atomic_write(path: &Path, data: &[u8], mode: u32) -> Result<()> {
    atomic_inner(path, data, mode, None)
}

pub fn atomic_replace_preserving(path: &Path, data: &[u8], expected: &[u8]) -> Result<()> {
    let old = File::open(path).map_err(|e| e.to_string())?;
    let mode = old.metadata().map_err(|e| e.to_string())?.mode() & 0o7777;
    atomic_inner(path, data, mode, Some((&old, expected)))
}

fn atomic_inner(
    path: &Path,
    data: &[u8],
    mode: u32,
    preserve: Option<(&File, &[u8])>,
) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("路径没有父目录: {}", path.display()))?;
    create_dir(parent)?;
    let temporary = parent.join(format!(
        ".novasched-{}-{}.tmp",
        current_pid(),
        TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    let mut options = OpenOptions::new();
    options.create_new(true).write(true).mode(mode);
    let mut file = options
        .open(&temporary)
        .map_err(|e| format!("创建临时文件 {} 失败: {e}", temporary.display()))?;
    if let Err(error) = file.write_all(data).and_then(|_| file.sync_all()) {
        cleanup(&temporary);
        return Err(format!(
            "写入临时文件 {} 失败: {error}",
            temporary.display()
        ));
    }
    if let Err(error) = fs::set_permissions(&temporary, fs::Permissions::from_mode(mode)) {
        cleanup(&temporary);
        return Err(format!("设置权限 {} 失败: {error}", temporary.display()));
    }
    if let Some((old, expected)) = preserve {
        let result = (|| {
            let source = old.metadata().map_err(|e| e.to_string())?;
            let target = file.metadata().map_err(|e| e.to_string())?;
            if (source.uid() != target.uid() || source.gid() != target.gid())
                && unsafe { ffi::fchown(file.as_raw_fd(), source.uid(), source.gid()) } != 0
            {
                return Err(ffi::errno_message("保留文件所有者失败"));
            }
            #[cfg(target_os = "android")]
            {
                let name = b"security.selinux\0";
                let mut label = [0u8; 512];
                let len = unsafe {
                    ffi::fgetxattr(
                        old.as_raw_fd(),
                        name.as_ptr().cast(),
                        label.as_mut_ptr().cast(),
                        label.len(),
                    )
                };
                if len < 0 {
                    return Err(ffi::errno_message("读取 SELinux 标签失败"));
                }
                if unsafe {
                    ffi::fsetxattr(
                        file.as_raw_fd(),
                        name.as_ptr().cast(),
                        label.as_ptr().cast(),
                        len as usize,
                        0,
                    )
                } != 0
                {
                    return Err(ffi::errno_message("保留 SELinux 标签失败"));
                }
            }
            if read_bytes(path)? != expected {
                return Err("Scene 文件已并发更新，取消覆盖，请重试".into());
            }
            Ok(())
        })();
        if let Err(error) = result {
            cleanup(&temporary);
            return Err(error);
        }
    }
    if let Err(error) = fs::rename(&temporary, path) {
        cleanup(&temporary);
        return Err(format!("原子替换 {} 失败: {error}", path.display()));
    }
    File::open(parent)
        .and_then(|file| file.sync_all())
        .map_err(|e| format!("同步文件目录 {} 失败: {e}", parent.display()))
}

fn cleanup(path: &Path) {
    if let Err(e) = remove_if_exists(path) {
        eprintln!("novasched: {e}");
    }
}
pub fn remove_if_exists(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("删除 {} 失败: {e}", path.display())),
    }
}

fn open_write_value(path: &Path, value: &str) -> Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .open(path)
        .map_err(|e| format!("打开节点 {} 失败: {e}", path.display()))?;
    file.write_all(value.as_bytes())
        .map_err(|e| format!("写入节点 {}={} 失败: {e}", path.display(), value))
}

pub fn write_node(path: &Path, value: &str) -> Result<()> {
    let original_mode = fs::metadata(path).ok().map(|m| m.mode() & 0o7777);
    match open_write_value(path, value) {
        Ok(()) => Ok(()),
        Err(first_error) => {
            // Retrying a semantic sysfs rejection after chmod produces a
            // duplicate error and cannot make the node supported. Only retry
            // genuine access-denied failures.
            if !permission_retry_warranted(&first_error) {
                return Err(first_error);
            }
            let Some(mode) = original_mode else {
                return Err(first_error);
            };
            fs::set_permissions(path, fs::Permissions::from_mode(mode | 0o600))
                .map_err(|e| format!("节点 {} 不可写且修改权限失败: {e}", path.display()))?;
            let result = open_write_value(path, value);
            let restore = fs::set_permissions(path, fs::Permissions::from_mode(mode));
            match (result, restore) {
                (Ok(()), Ok(())) => Ok(()),
                (Err(e), _) => Err(e),
                (Ok(()), Err(e)) => Err(format!(
                    "写入成功但恢复节点权限 {} 失败: {e}",
                    path.display()
                )),
            }
        }
    }
}

fn permission_retry_warranted(error: &str) -> bool {
    error.contains("Permission denied")
        || error.contains("Operation not permitted")
        || error.contains("os error 13)")
        || error.contains("os error 1)")
}

pub fn getprop(name: &str) -> String {
    #[cfg(target_os = "android")]
    {
        let Ok(c_name) = CString::new(name) else {
            return String::new();
        };
        let mut buffer = [0 as c_char; 128];
        let length = unsafe { ffi::__system_property_get(c_name.as_ptr(), buffer.as_mut_ptr()) };
        if length <= 0 {
            return String::new();
        }
        return unsafe { CStr::from_ptr(buffer.as_ptr()) }
            .to_string_lossy()
            .into_owned();
    }
    #[cfg(not(target_os = "android"))]
    {
        Command::new("getprop")
            .arg(name)
            .output()
            .ok()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_default()
    }
}

pub fn run_command(program: &str, args: &[&str]) -> Result<String> {
    let output = Command::new(program)
        .args(args)
        .output()
        .map_err(|e| format!("执行 {program} 失败: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "命令 {program} 退出码 {:?}: {}",
            output.status.code(),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Small platform queries must not block the scheduling heartbeat indefinitely.
/// A file avoids pipe backpressure and bounds the amount of captured output.
pub fn query_command(program: &str, args: &[&str], timeout: Duration) -> Result<String> {
    let path = std::env::temp_dir().join(format!(
        "novasched-query-{}-{}",
        current_pid(),
        TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    // Android's generic temp directory may not be writable in a root domain.
    let path = if cfg!(target_os = "android") {
        Path::new(STATE_DIR).join(path.file_name().ok_or("查询临时路径无效")?)
    } else {
        path
    };
    let output = OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(&path)
        .map_err(|e| format!("创建查询输出失败: {e}"))?;
    let result = (|| {
        let stderr = output
            .try_clone()
            .map_err(|e| format!("创建查询错误输出失败: {e}"))?;
        let mut child = Command::new(program)
            .args(args)
            .env_remove("LD_LIBRARY_PATH")
            .env_remove("LD_PRELOAD")
            .env_remove("CLASSPATH")
            .stdout(Stdio::from(output))
            .stderr(Stdio::from(stderr))
            .spawn()
            .map_err(|e| format!("执行 {program} 失败: {e}"))?;
        let deadline = Instant::now() + timeout;
        loop {
            match child.try_wait() {
                Ok(Some(status)) => {
                    let mut text = String::new();
                    File::open(&path)
                        .and_then(|f| f.take(65_537).read_to_string(&mut text))
                        .map_err(|e| format!("读取查询输出失败: {e}"))?;
                    if text.len() > 65_536 {
                        return Err("平台查询输出超出限制".into());
                    }
                    if !status.success() {
                        return Err(format!(
                            "查询 {program} 退出码 {:?}: {}",
                            status.code(),
                            text.trim()
                        ));
                    }
                    return Ok(text.trim().to_string());
                }
                Ok(None) if Instant::now() < deadline => sleep(Duration::from_millis(20)),
                outcome => {
                    let killed = child.kill();
                    let waited = child.wait();
                    if let Err(e) = killed {
                        eprintln!("novasched: 终止查询失败: {e}");
                    }
                    if let Err(e) = waited {
                        eprintln!("novasched: 回收查询失败: {e}");
                    }
                    return Err(match outcome {
                        Err(e) => format!("检查平台查询失败: {e}"),
                        _ => format!("查询 {program} 超时"),
                    });
                }
            }
        }
    })();
    if let Err(error) = fs::remove_file(&path) {
        eprintln!("novasched: 清理查询输出失败: {error}");
    }
    result
}

pub fn local_timestamp() -> String {
    let mut raw: c_long = 0;
    unsafe {
        ffi::time(&mut raw as *mut c_long);
    }
    let mut tm = MaybeUninit::<ffi::Tm>::uninit();
    let result = unsafe { ffi::localtime_r(&raw as *const c_long, tm.as_mut_ptr()) };
    if result.is_null() {
        return "1970-01-01 00:00:00".to_string();
    }
    let format = b"%Y-%m-%d %H:%M:%S\0";
    let mut buffer = [0 as c_char; 32];
    let length = unsafe {
        ffi::strftime(
            buffer.as_mut_ptr(),
            buffer.len(),
            format.as_ptr() as *const c_char,
            tm.as_ptr(),
        )
    };
    if length == 0 {
        return "1970-01-01 00:00:00".to_string();
    }
    unsafe { CStr::from_ptr(buffer.as_ptr()) }
        .to_string_lossy()
        .into_owned()
}

pub fn hex_encode(input: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(input.len() * 2);
    for byte in input {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

pub fn hex_decode(input: &str) -> Result<Vec<u8>> {
    if input.len() % 2 != 0 {
        return Err("十六进制长度无效".to_string());
    }
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len() / 2);
    let value = |b: u8| -> Option<u8> {
        match b {
            b'0'..=b'9' => Some(b - b'0'),
            b'a'..=b'f' => Some(b - b'a' + 10),
            b'A'..=b'F' => Some(b - b'A' + 10),
            _ => None,
        }
    };
    for pair in bytes.chunks_exact(2) {
        let hi = value(pair[0]).ok_or_else(|| "十六进制字符无效".to_string())?;
        let lo = value(pair[1]).ok_or_else(|| "十六进制字符无效".to_string())?;
        out.push((hi << 4) | lo);
    }
    Ok(out)
}

pub fn file_stamp(path: &Path) -> Option<(i64, i64, u64)> {
    let metadata = fs::metadata(path).ok()?;
    Some((metadata.mtime(), metadata.mtime_nsec(), metadata.len()))
}

pub fn sleep(duration: Duration) {
    std::thread::sleep(duration);
}

pub fn base_package(process: &str) -> &str {
    process
        .split_once(':')
        .map(|(base, _)| base)
        .unwrap_or(process)
}

pub fn valid_package(package: &str) -> bool {
    if package == "*" {
        return true;
    }
    !package.is_empty()
        && package.len() <= 255
        && package.contains('.')
        && package
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b':'))
        && package.matches(':').count() <= 1
        && !package
            .bytes()
            .any(|b| b.is_ascii_whitespace() || b == b'/' || b == b'\\')
        && package
            .as_bytes()
            .first()
            .map(|b| b.is_ascii_alphabetic())
            .unwrap_or(false)
}

pub fn current_pid() -> i32 {
    unsafe { ffi::getpid() }
}

pub fn acquire_flock(path: &Path) -> Result<File> {
    if let Some(parent) = path.parent() {
        create_dir(parent)?;
    }
    let file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .mode(0o600)
        .open(path)
        .map_err(|e| format!("打开锁文件 {} 失败: {e}", path.display()))?;
    let result = unsafe { ffi::flock(file.as_raw_fd(), ffi::LOCK_EX | ffi::LOCK_NB) };
    if result != 0 {
        return Err(format!(
            "另一个 NovaSched 实例正在运行: {}",
            std::io::Error::last_os_error()
        ));
    }
    Ok(file)
}

#[cfg(test)]
mod regression_tests {
    use super::*;
    #[test]
    fn concurrent_atomic_writers_never_share_temp_files() {
        let dir = std::env::temp_dir().join(format!("cts-atomic-{}", current_pid()));
        create_dir(&dir).unwrap();
        let path = dir.join("value");
        let threads: Vec<_> = (0..16)
            .map(|i| {
                let path = path.clone();
                std::thread::spawn(move || {
                    for _ in 0..8 {
                        atomic_write(&path, format!("record-{i:02}").as_bytes(), 0o600).unwrap();
                    }
                })
            })
            .collect();
        for t in threads {
            t.join().unwrap();
        }
        let data = read_text(&path).unwrap();
        assert!(data.starts_with("record-") && data.len() == 9);
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn scene_replace_checks_concurrent_edits_and_preserves_mode() {
        let path = std::env::temp_dir().join(format!("cts-preserve-{}", current_pid()));
        atomic_write(&path, b"old", 0o640).unwrap();
        assert!(atomic_replace_preserving(&path, b"new", b"stale").is_err());
        assert_eq!(read_bytes(&path).unwrap(), b"old");
        atomic_replace_preserving(&path, b"new", b"old").unwrap();
        assert_eq!(read_bytes(&path).unwrap(), b"new");
        assert_eq!(fs::metadata(&path).unwrap().mode() & 0o777, 0o640);
        fs::remove_file(path).unwrap();
    }
    #[test]
    fn package_and_errno_validation_reject_protocol_injection() {
        assert!(valid_package("com.example.game:worker"));
        for p in [
            "com.a,mode:fast",
            "com.a\tfast",
            "com.a\n* fast",
            "com.a/../../data",
            "com.a::x",
        ] {
            assert!(!valid_package(p));
        }
        assert!(!permission_retry_warranted("io (os error 110)"));
        assert!(!permission_retry_warranted("io (os error 95)"));
        assert!(permission_retry_warranted("io (os error 13)"));
    }
}
