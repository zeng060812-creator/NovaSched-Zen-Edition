//! Delay kernel controls until credential storage is ready, and remember
//! an unconfirmed startup across a kernel reset.
use crate::util::{self, Result};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const STABLE_TIME: Duration = Duration::from_secs(180);

pub fn ensure_ready() -> Result<()> {
    if !cfg!(target_os = "android") {
        return Ok(());
    }
    if util::getprop("sys.boot_completed") != "1" {
        return Err("系统尚未完成启动，调度保持暂停".into());
    }
    let user = util::current_android_user()?;
    let ce = util::getprop(&format!("sys.user.{user}.ce_available"));
    check_user_unlocked(user, &ce, |program, args| util::query_command(program, args, Duration::from_secs(2)))
}
fn check_user_unlocked(user: u32, ce: &str, mut query: impl FnMut(&str, &[&str]) -> Result<String>) -> Result<()> {
    if matches!(ce.trim(), "1" | "true") { return Ok(()); }
    let id = user.to_string();
    let mut failures = Vec::new();
    for (program, args) in [
        ("/system/bin/cmd", vec!["activity", "get-started-user-state", &id]),
        ("/system/bin/am", vec!["get-started-user-state", &id]),
        ("/system/bin/dumpsys", vec!["activity", "users"]),
    ] {
        match query(program, &args) {
            Ok(text) => {
                let state = if program.ends_with("/dumpsys") { summary_user_state(&text, user) }
                    else { parsed_user_state(&text, user) };
                match state {
                    Some(true) => return Ok(()),
                    Some(false) => return Err(format!("等待用户 {user} 首次解锁，系统状态尚未就绪")),
                    None => failures.push(format!("{program}: 解锁状态无法识别")),
                }
            }
            Err(error) => failures.push(error.chars().take(320).collect()),
        }
    }
    Err(format!("无法确认用户 {user} 已解锁；{}", failures.join("；")))
}
fn named_user_state(state: &str) -> Option<bool> {
    match state.trim() {
        "RUNNING_UNLOCKED" | "3" => Some(true),
        "BOOTING" | "RUNNING_LOCKED" | "RUNNING_UNLOCKING" | "STOPPING" | "SHUTDOWN"
            | "0" | "1" | "2" | "4" | "5" => Some(false),
        _ => None,
    }
}
fn parsed_user_state(text: &str, user: u32) -> Option<bool> {
    if let Some(state) = named_user_state(text) { return Some(state); }
    // Parse the current state; lastState may describe an earlier unlock.
    let record = text.trim().strip_prefix("[UserState: ")?.strip_suffix(']')?;
    let mut id = None;
    let mut state = None;
    for field in record.split(',').map(str::trim) {
        if let Some(value) = field.strip_prefix("id=") {
            if id.is_some() { return None; }
            id = Some(value.parse::<u32>().ok()?);
        }
        if let Some(value) = field.strip_prefix("state=") {
            if state.is_some() { return None; }
            state = named_user_state(value);
        }
        if field == "switching=true" { return Some(false); }
    }
    if id == Some(user) { state } else { None }
}
fn summary_user_state(text: &str, user: u32) -> Option<bool> {
    let prefix = format!("User #{user}:");
    let mut state = None;
    for line in text.lines().map(str::trim) {
        let Some(record) = line.strip_prefix(&prefix) else { continue; };
        let value = record.trim().strip_prefix("state=")?.split_whitespace().next()?;
        let found = named_user_state(value)?;
        if state.is_some_and(|previous| previous != found) { return None; }
        state = Some(found && !record.split_whitespace().any(|part| part == "SWITCHING"));
    }
    state
}
pub fn diagnostic() -> String {
    let user = util::current_android_user();
    let mut report = format!("query_transport=pipe\nquery_stdin=null\nboot_completed={}\n", util::getprop("sys.boot_completed"));
    match user {
        Ok(user) => report.push_str(&format!("current_user={user}\nce_available={}\n", util::getprop(&format!("sys.user.{user}.ce_available")))),
        Err(error) => report.push_str(&format!("current_user_error={error}\n")),
    }
    match ensure_ready() {
        Ok(()) => report.push_str("boot_ready=true\n"),
        Err(error) => report.push_str(&format!("boot_ready=false\nboot_ready_error={error}\n")),
    }
    report
}

pub struct BootGuard {
    path: PathBuf,
    boot: String,
    stable_since: Option<Instant>,
    confirmed: bool,
}

impl BootGuard {
    pub fn begin(module: &Path, state: &Path) -> Result<Self> {
        let boot = util::read_trimmed("/proc/sys/kernel/random/boot_id")?;
        Self::begin_for_boot(module, state, &boot)
    }

    fn begin_for_boot(module: &Path, state: &Path, boot: &str) -> Result<Self> {
        if boot.is_empty() || !boot.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') {
            return Err("无法确认启动身份，未写入调度节点".into());
        }
        let path = crate::storage::path(&state, "boot-guard");
        let previous = match fs::read_to_string(&path) {
            Ok(text) => {
                let (boot, phase) = parse_record(&text)?;
                Some((boot.to_owned(), phase.to_owned()))
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(format!("无法读取启动保护记录: {error}")),
        };
        if let Some((old_boot, phase)) = previous {
            if old_boot != boot && phase == "pending" {
                let reason = "上次调度启动未确认稳定就发生重启，模块已自动停用。请保留 boot-guard、boot-entry.log 和内核崩溃记录，确认后再从管理器启用。";
                util::atomic_write(&module.join("disable"), reason.as_bytes(), 0o644)?;
                write_record(&path, boot, "blocked")?;
                return Err(reason.into());
            }
        }
        write_record(&path, boot, "pending")?;
        Ok(Self { path, boot: boot.into(), stable_since: None, confirmed: false })
    }

    pub fn confirm_if_stable(&mut self, healthy: bool) -> Result<bool> {
        if self.confirmed { return Ok(false); }
        if !healthy {
            self.stable_since = None;
            return Ok(false);
        }
        let since = self.stable_since.get_or_insert_with(Instant::now);
        if since.elapsed() < STABLE_TIME {
            return Ok(false);
        }
        write_record(&self.path, &self.boot, "healthy")?;
        self.confirmed = true;
        Ok(true)
    }

    pub fn finish_cleanly(&self) -> Result<()> {
        write_record(&self.path, &self.boot, "stopped")
    }
}

fn write_record(path: &Path, boot: &str, phase: &str) -> Result<()> {
    util::atomic_write(path, format!("boot={boot}\nstate={phase}\nversion={}\n", env!("CARGO_PKG_VERSION")).as_bytes(), 0o600)
}

fn parse_record(text: &str) -> Result<(&str, &str)> {
    let boot = text.lines().find_map(|line| line.strip_prefix("boot="));
    let phase = text.lines().find_map(|line| line.strip_prefix("state="));
    match (boot, phase) {
        (Some(boot), Some(phase)) if !boot.is_empty() && matches!(phase, "pending" | "healthy" | "stopped" | "blocked") => Ok((boot, phase)),
        _ => Err("启动保护记录损坏，未写入调度节点；请保留 boot-guard 供诊断".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locked_unlocking_and_unknown_users_never_pass_the_gate() {
        for state in ["0", "1", "2", "RUNNING_LOCKED", "RUNNING_UNLOCKING", "", "User is not started: 0", "RUNNING_UNLOCKED extra"] {
            assert!(!parsed_user_state(state, 0).unwrap_or(false), "{state}");
        }
        assert!(parsed_user_state("RUNNING_UNLOCKED\n", 0) == Some(true));
        assert!(parsed_user_state("3", 0) == Some(true));
    }

    #[test]
    fn a_kernel_reset_during_trial_disables_before_a_second_dispatch() {
        let root = std::env::temp_dir().join(format!("nova-guard-reset-{}", std::process::id()));
        let module = root.join("module");
        let state = root.join("state");
        fs::create_dir_all(&module).unwrap();
        fs::create_dir_all(&state).unwrap();
        BootGuard::begin_for_boot(&module, &state, "boot-a").unwrap();
        assert!(BootGuard::begin_for_boot(&module, &state, "boot-a").is_ok());
        assert!(!module.join("disable").exists());
        assert!(BootGuard::begin_for_boot(&module, &state, "boot-b").is_err());
        assert!(module.join("disable").exists());
        assert_eq!(parse_record(&fs::read_to_string(crate::storage::path(&state, "boot-guard")).unwrap()).unwrap().1, "blocked");
        // The manager's explicit re-enable permits one fresh trial.
        fs::remove_file(module.join("disable")).unwrap();
        let mut guard = BootGuard::begin_for_boot(&module, &state, "boot-c").unwrap();
        assert!(!guard.confirm_if_stable(true).unwrap());
        guard.stable_since = Some(Instant::now() - STABLE_TIME);
        assert!(!guard.confirm_if_stable(false).unwrap());
        guard.stable_since = Some(Instant::now() - STABLE_TIME);
        assert!(guard.confirm_if_stable(true).unwrap());
        assert!(!guard.confirm_if_stable(true).unwrap());
        let guard = BootGuard::begin_for_boot(&module, &state, "boot-d").unwrap();
        guard.finish_cleanly().unwrap();
        assert!(BootGuard::begin_for_boot(&module, &state, "boot-e").is_ok());
        fs::remove_dir_all(root).unwrap();
    }
}

#[cfg(test)]
mod android_unlock_regressions {
    use super::*;
    #[test]
    fn aosp_user_state_objects_match_the_requested_user_and_current_state() {
        let unlocked = "[UserState: id=10, state=RUNNING_UNLOCKED, lastState=RUNNING_UNLOCKING, switching=false]";
        assert_eq!(parsed_user_state(unlocked, 10), Some(true)); assert_eq!(parsed_user_state(unlocked, 0), None);
        let locked = "[UserState: id=10, state=RUNNING_LOCKED, lastState=RUNNING_UNLOCKED, switching=false]";
        assert_eq!(parsed_user_state(locked, 10), Some(false));
        assert_eq!(parsed_user_state(&unlocked.replace("switching=false", "switching=true"), 10), Some(false));
        assert!(check_user_unlocked(10, "false", |_, _| Ok(locked.into())).is_err());
    }
    #[test]
    fn unlock_fallback_uses_only_the_current_users_state() {
        let summary = "mStartedUsers:\n User #0: state=RUNNING_UNLOCKED\n User #10: state=RUNNING_LOCKED\nmCurrentUserId:10\n";
        assert_eq!(summary_user_state(summary, 10), Some(false)); assert_eq!(summary_user_state(summary, 11), None);
        assert!(check_user_unlocked(10, "", |program, _| {
            if program.ends_with("/dumpsys") { Ok(summary.into()) } else { Err("unsupported command".into()) }
        }).is_err());
        assert!(check_user_unlocked(10, "", |program, _| {
            if program.ends_with("/dumpsys") { Ok(summary.replace("User #10: state=RUNNING_LOCKED", "User #10: state=RUNNING_UNLOCKED")) }
            else { Err("unsupported command".into()) }
        }).is_ok());
        assert!(check_user_unlocked(10, "true", |_, _| panic!("CE property already confirms unlock")).is_ok());
        assert!(check_user_unlocked(10, "", |_, _| Err("Binder unavailable".into())).unwrap_err().contains("Binder unavailable"));
    }
}
