use std::ffi::CStr;
#[cfg(target_os = "android")]
use std::ffi::CString;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::mem::MaybeUninit;
use std::os::fd::AsRawFd;
use std::os::raw::{c_char, c_long};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use crate::ffi;

pub const MODULE_DIR_DEFAULT: &str = "/data/adb/modules/NovaSched_Zen_Edition";
pub const STATE_DIR: &str = "/data/adb/novasched";
pub const RUNTIME_DIR: &str = STATE_DIR;
pub const CONFIG_PATH: &str = "/data/adb/novasched/config/config.json";
pub const MODE_PATH: &str = "/data/adb/novasched/run/mode.txt";
pub const SCENE_REQUEST_PATH: &str = "/data/adb/novasched/run/scene-request.txt";
pub const APP_MODES_PATH: &str = "/data/adb/novasched/config/app_modes.txt";
pub const AUTO_POLICY_PATH: &str = "/data/adb/novasched/config/automatic.json";

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

pub fn current_android_user() -> Result<u32> {
    use std::sync::{Mutex, OnceLock};
    static USER: OnceLock<Mutex<Option<(Instant, Result<u32>)>>> = OnceLock::new();
    let cache = USER.get_or_init(|| Mutex::new(None));
    if let Ok(guard) = cache.lock() {
        if let Some((at, user)) = guard.as_ref() {
            if at.elapsed() < Duration::from_secs(10) {
                return user.clone();
            }
        }
    }
    let result =
        query_current_user(|program, args| query_command(program, args, Duration::from_secs(2)));
    if let Ok(mut guard) = cache.lock() {
        *guard = Some((Instant::now(), result.clone()));
    }
    result
}

fn query_current_user(mut query: impl FnMut(&str, &[&str]) -> Result<String>) -> Result<u32> {
    let mut failures = Vec::new();
    for (program, args) in [
        ("/system/bin/cmd", vec!["activity", "get-current-user"]),
        ("/system/bin/am", vec!["get-current-user"]),
        ("/system/bin/dumpsys", vec!["activity", "users"]),
        ("/system/bin/dumpsys", vec!["user"]),
        ("/system/bin/dumpsys", vec!["window", "policy"]),
        ("/system/bin/dumpsys", vec!["activity", "activities"]),
    ] {
        let command = format!("{program} {}", args.join(" "));
        match query(program, &args) {
            Ok(text) => {
                if let Some(user) = parse_current_user(&text) { return Ok(user); }
                failures.push(format!("{command}: 未识别当前用户"));
            }
            Err(error) => failures.push(format!("{command}: {}", error.chars().take(320).collect::<String>())),
        }
    }
    Err(format!("无法查询 Android 当前用户；{}", failures.join("；")))
}

fn parse_current_user(text: &str) -> Option<u32> {
    fn id(text: &str) -> Option<u32> {
        let text = text.trim();
        (!text.is_empty() && text.bytes().all(|b| b.is_ascii_digit()))
            .then(|| text.parse().ok()).flatten()
    }
    if let Some(user) = id(text) { return Some(user); }
    let mut found = None;
    for line in text.lines() {
        let line = line.trim();
        for prefix in ["mCurrentUser=", "mCurrentUserId=", "mCurrentUser:", "mCurrentUserId:", "Current user:"] {
            if let Some(value) = line.strip_prefix(prefix) {
                let value = value.split_whitespace().next()?.trim_end_matches(',');
                let user = id(value)?;
                if found.is_some_and(|previous| previous != user) { return None; }
                found = Some(user);
            }
        }
    }
    found
}

#[cfg(test)]
mod user_tests {
    use super::*;
    #[test]
    fn current_user_query_is_explicit_and_never_defaults_to_zero() {
        assert_eq!(query_current_user(|_, _| Ok("10\n".into())), Ok(10));
        assert!(query_current_user(|_, _| Ok("Error: activity unavailable".into())).is_err());
        let mut calls = 0;
        assert_eq!(
            query_current_user(|_, _| {
                calls += 1;
                if calls == 1 {
                    Err("denied".into())
                } else {
                    Ok("0".into())
                }
            }),
            Ok(0)
        );
        assert_eq!(calls, 2);
    }

    #[test]
    fn activity_summary_recovers_user_when_short_commands_fail() {
        assert_eq!(query_current_user(|program, _| {
            if program.ends_with("dumpsys") {
                Ok("ACTIVITY MANAGER ACTIVITIES\n  mCurrentUser=10\n".into())
            } else {
                Err("unsupported command".into())
            }
        }), Ok(10));
        assert_eq!(parse_current_user("Current user: 0\n"), Some(0));
        assert_eq!(parse_current_user("unrelated: 0\n"), None);
    }
}

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
    reject_symlink(path)?;
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

/// A planted symlink at a destination means the private layout was tampered
/// with. rename(2) would only replace the link itself, but the unexpected
/// layout must be reported instead of silently accepted.
fn reject_symlink(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => {
            Err(format!("拒绝覆盖符号链接: {}", path.display()))
        }
        Ok(_) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("检查 {} 失败: {e}", path.display())),
    }
}
pub fn remove_if_exists(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("删除 {} 失败: {e}", path.display())),
    }
}

fn open_write_value(path: &Path, value: &str) -> std::io::Result<()> {
    let mut file = OpenOptions::new().write(true).open(path)?;
    file.write_all(value.as_bytes())
}

pub fn write_node(path: &Path, value: &str) -> Result<()> {
    let original_mode = fs::metadata(path).ok().map(|m| m.mode() & 0o7777);
    let describe = |error: &std::io::Error| {
        format!("写入节点 {}={} 失败: {error}", path.display(), value)
    };
    match open_write_value(path, value) {
        Ok(()) => Ok(()),
        Err(first_error) => {
            // Retrying a semantic sysfs rejection after chmod produces a
            // duplicate error and cannot make the node supported. Only retry
            // genuine access-denied failures, identified by errno rather than
            // by matching a localized message string.
            if !permission_retry_warranted(&first_error) {
                return Err(describe(&first_error));
            }
            let Some(mode) = original_mode else {
                return Err(describe(&first_error));
            };
            fs::set_permissions(path, fs::Permissions::from_mode(mode | 0o600))
                .map_err(|e| format!("节点 {} 不可写且修改权限失败: {e}", path.display()))?;
            let result = open_write_value(path, value);
            let restore = fs::set_permissions(path, fs::Permissions::from_mode(mode));
            match (result, restore) {
                (Ok(()), Ok(())) => Ok(()),
                (Err(e), _) => Err(describe(&e)),
                (Ok(()), Err(e)) => Err(format!(
                    "写入成功但恢复节点权限 {} 失败: {e}",
                    path.display()
                )),
            }
        }
    }
}

fn permission_retry_warranted(error: &std::io::Error) -> bool {
    // EACCES (13) / EPERM (1): a chmod may grant write access. Anything else
    // (EINVAL, EIO, ENOSPC, …) is semantic and retrying cannot help.
    matches!(error.raw_os_error(), Some(13) | Some(1))
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

/// Android forwards these descriptors to Binder services. Pipes keep queries
/// independent of private runtime-file labels and inherited terminal access.
pub fn query_command(program: &str, args: &[&str], timeout: Duration) -> Result<String> {
    query_command_limited(program, args, timeout, 65_536)
}

pub fn query_command_limited(program: &str, args: &[&str], timeout: Duration, limit: u64) -> Result<String> {
    if limit > 1_048_576 { return Err("平台查询限额不能超过 1 MiB".into()); }
    let mut command = Command::new(program);
    command.args(args).env_remove("LD_LIBRARY_PATH").env_remove("LD_PRELOAD").env_remove("CLASSPATH")
        .stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    unsafe {
        command.pre_exec(|| {
            if ffi::setsid() < 0 { return Err(std::io::Error::last_os_error()); }
            Ok(())
        });
    }
    let mut child = command.spawn().map_err(|e| format!("执行 {program} 失败: {e}"))?;
    let result = (|| {
        let mut stdout = child.stdout.take().ok_or("未取得查询输出管道")?;
        let mut stderr = child.stderr.take().ok_or("未取得查询错误管道")?;
        for fd in [stdout.as_raw_fd(), stderr.as_raw_fd()] {
            let flags = unsafe { ffi::fcntl(fd, ffi::F_GETFL) };
            if flags < 0 || unsafe { ffi::fcntl(fd, ffi::F_SETFL, flags | ffi::O_NONBLOCK) } < 0 {
                return Err(ffi::errno_message("配置查询管道失败"));
            }
        }
        let mut output = QueryOutput::new(limit as usize);
        let mut errors = QueryOutput::new(16_384);
        let deadline = Instant::now() + timeout;
        loop {
            output.drain(&mut stdout)?;
            errors.drain(&mut stderr)?;
            if output.truncated { return Err(format!("查询 {program} 输出超出限制")); }
            if let Some(status) = child.try_wait().map_err(|e| format!("检查平台查询失败: {e}"))? {
                while !output.drain(&mut stdout)? {
                    if output.truncated { return Err(format!("查询 {program} 输出超出限制")); }
                }
                errors.drain(&mut stderr)?;
                if output.truncated { return Err(format!("查询 {program} 输出超出限制")); }
                let text = String::from_utf8_lossy(&output.bytes).trim().to_string();
                if !status.success() {
                    return Err(format!("查询 {program} 退出码 {:?}: {} {}", status.code(), text,
                        String::from_utf8_lossy(&errors.bytes).trim()));
                }
                return Ok(text);
            }
            if Instant::now() >= deadline {
                return Err(format!("查询 {program} 超时: {}", String::from_utf8_lossy(&errors.bytes).trim()));
            }
            let mut fds = [
                ffi::PollFd { fd: if output.eof { -1 } else { stdout.as_raw_fd() }, events: ffi::POLLIN, revents: 0 },
                ffi::PollFd { fd: if errors.eof { -1 } else { stderr.as_raw_fd() }, events: ffi::POLLIN, revents: 0 },
            ];
            if unsafe { ffi::poll(fds.as_mut_ptr(), fds.len(), 20) } < 0 {
                let error = std::io::Error::last_os_error();
                if error.kind() != std::io::ErrorKind::Interrupted { return Err(format!("等待查询管道失败: {error}")); }
            }
        }
    })();
    if result.is_err() && child.try_wait().ok().flatten().is_none() {
        // The unreaped child owns the process group established by setsid.
        unsafe { ffi::kill(-(child.id() as i32), ffi::SIGKILL); }
        let _ = child.kill();
        let _ = child.wait();
    }
    result
}

struct QueryOutput { bytes: Vec<u8>, limit: usize, truncated: bool, eof: bool }
impl QueryOutput {
    fn new(limit: usize) -> Self { Self { bytes: Vec::new(), limit, truncated: false, eof: false } }
    fn drain(&mut self, reader: &mut impl Read) -> Result<bool> {
        if self.eof { return Ok(true); }
        let mut buffer = [0u8; 8192];
        for _ in 0..8 {
            match reader.read(&mut buffer) {
                Ok(0) => { self.eof = true; return Ok(true); }
                Ok(count) => {
                    let keep = count.min(self.limit.saturating_sub(self.bytes.len()));
                    self.bytes.extend_from_slice(&buffer[..keep]);
                    self.truncated |= keep < count;
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => return Ok(true),
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(format!("读取查询管道失败: {error}")),
            }
        }
        Ok(false)
    }
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
    reject_symlink(path)?;
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
        assert!(!permission_retry_warranted(&std::io::Error::from_raw_os_error(110)));
        assert!(!permission_retry_warranted(&std::io::Error::from_raw_os_error(95)));
        assert!(permission_retry_warranted(&std::io::Error::from_raw_os_error(13)));
        assert!(permission_retry_warranted(&std::io::Error::from_raw_os_error(1)));
        assert!(!permission_retry_warranted(&std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "denied"
        )));
    }
}

#[cfg(test)]
mod query_output_regressions {
    use super::*;
    #[test]
    fn successful_android_queries_ignore_stderr_warnings() {
        let reply = query_command("/bin/sh", &["-c", "printf '10\\n'; printf 'vendor warning\\n' >&2"], Duration::from_secs(2)).unwrap();
        assert_eq!(reply, "10");
        let error = query_command("/bin/sh", &["-c", "printf 'binder denied' >&2; exit 1"], Duration::from_secs(2)).unwrap_err();
        assert!(error.contains("binder denied"));
    }
}

#[cfg(test)]
mod android_startup_regressions {
    use super::*;
    #[test]
    fn aosp_colon_and_vendor_equals_user_fields_are_both_supported() {
        for text in ["mCurrentUserId:10", "mCurrentUserId: 10", "mCurrentUserId=10", "mCurrentUser=10,", "Current user: 10"] {
            assert_eq!(parse_current_user(text), Some(10), "{text}");
        }
        for text in ["mTargetUserId:0", "mCurrentUserId:-1", "mCurrentUserId:10\nmCurrentUserId:0", "Error: 0", "0\n10"] {
            assert_eq!(parse_current_user(text), None, "{text}");
        }
        assert_eq!(query_current_user(|_, args| {
            if args == ["activity", "users"] { Ok("mStartedUsers:\n User #10: state=RUNNING_UNLOCKED\nmCurrentUserId:10\n".into()) }
            else { Err("Binder stdout denied".into()) }
        }), Ok(10));
    }
    #[test]
    fn failed_user_lookup_preserves_the_command_and_os_error() {
        let error = query_current_user(|_, _| Err("Permission denied (os error 13)".into())).unwrap_err();
        assert!(error.contains("activity get-current-user"));
        assert!(error.contains("dumpsys activity users")); assert!(error.contains("Permission denied"));
    }
    #[test]
    fn binder_queries_use_pipes_and_eof_stdin_without_private_output_files() {
        let reply = query_command("python3", &["-c", "import os,stat; assert stat.S_ISFIFO(os.fstat(1).st_mode); assert stat.S_ISFIFO(os.fstat(2).st_mode); assert os.read(0,1)==b''; print(10)"], Duration::from_secs(2)).unwrap();
        assert_eq!(reply, "10");
    }
    #[test]
    fn full_pipes_and_stderr_floods_do_not_block_queries() {
        let reply = query_command_limited("python3", &["-c", "import os; os.write(2,b'w'*200000); os.write(1,b'a'*200000)"], Duration::from_secs(2), 250_000).unwrap();
        assert_eq!(reply.len(), 200_000); assert!(reply.bytes().all(|byte| byte == b'a'));
        let error = query_command_limited("python3", &["-c", "import os; os.write(1,b'a'*200000)"], Duration::from_secs(2), 1024).unwrap_err();
        assert!(error.contains("输出超出限制"));
    }
    #[test]
    fn query_timeout_terminates_a_waiting_wrapper_and_its_child() {
        let started = Instant::now();
        let error = query_command("/bin/sh", &["-c", "sleep 5 & wait"], Duration::from_millis(120)).unwrap_err();
        assert!(error.contains("超时")); assert!(started.elapsed() < Duration::from_secs(1));
    }
}
