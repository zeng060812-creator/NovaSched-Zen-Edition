//! Touch-event-driven transient top-app boost. Polls /dev/input directly (no
//! subprocess), applies a short uclamp.min pulse and returns to the latest
//! application baseline. A foreign scheduler stops all pulse writes.
use std::fs::{self, File, OpenOptions};
use std::io::Read;
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::ffi;
use crate::logging::Logger;
use crate::scheduler::{CpuctlLive, CpuctlLiveState};
use crate::snapshot::Snapshot;

pub struct InputBoostContext {
    pub live: Arc<CpuctlLive>,
    pub node: PathBuf,
    pub stock: String,
    pub stop: Arc<AtomicBool>,
    pub snapshot: Snapshot,
    pub logger: Logger,
    /// eventfd 写端（dup 出的独立 fd，所有权随线程，退出时关闭）：
    /// 触摸按下沿写入 1，让主循环立即识别通知栏等 UI 突发。-1 表示不可用。
    pub touch_signal_fd: std::os::raw::c_int,
}

/// RAII：线程退出（无论何种路径）时关闭 dup 出的 eventfd 写端。
struct TouchFd(std::os::raw::c_int);
impl Drop for TouchFd {
    fn drop(&mut self) {
        if self.0 >= 0 {
            unsafe { ffi::close(self.0) };
        }
    }
}

/// Opens every /dev/input/event* device and spawns the boost loop. Returns
/// None when no input device is readable (feature silently inert).
pub fn spawn(ctx: InputBoostContext) -> Option<std::thread::JoinHandle<()>> {
    if !ctx.node.exists() {
        ctx.logger.debug("内核未提供 top-app uclamp，使用 CPU 频率策略");
        return None;
    }
    let mut files = Vec::new();
    let Ok(entries) = fs::read_dir("/dev/input") else {
        ctx.logger.debug("未找到 /dev/input，输入突频未启用");
        return None;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let is_event = path
            .file_name()
            .and_then(|v| v.to_str())
            .is_some_and(|name| name.starts_with("event"));
        if !is_event {
            continue;
        }
        let name = path.file_name().unwrap_or_default();
        let capabilities = PathBuf::from("/sys/class/input").join(name).join("device/capabilities");
        let abs = crate::util::read_trimmed(capabilities.join("abs")).unwrap_or_default();
        let key = crate::util::read_trimmed(capabilities.join("key")).unwrap_or_default();
        if !touchscreen_capabilities(&abs, &key) { continue; }
        match OpenOptions::new()
            .read(true)
            .custom_flags(ffi::O_NONBLOCK | ffi::O_CLOEXEC)
            .open(&path)
        {
            Ok(file) => files.push(file),
            Err(error) => ctx.logger.debug(format!(
                "输入设备不可读，跳过: {} ({error})",
                path.display()
            )),
        }
    }
    if files.is_empty() {
        ctx.logger.debug("没有可读输入设备，输入突频未启用");
        return None;
    }
    std::thread::Builder::new()
        .name("novasched-input".into())
        .spawn(move || run(ctx, files))
        .ok()
}

fn run(ctx: InputBoostContext, mut files: Vec<File>) {
    let _touch_fd = TouchFd(ctx.touch_signal_fd);
    let mut poll_fds: Vec<ffi::PollFd> = files
        .iter()
        .map(|file| ffi::PollFd {
            fd: file.as_raw_fd(),
            events: ffi::POLLIN,
            revents: 0,
        })
        .collect();
    let mut pulse = Pulse::default();
    let mut previous_activity = false;
    while !ctx.stop.load(Ordering::Relaxed) {
        let timeout = pulse.until.map(|until| until.saturating_duration_since(Instant::now()).as_millis().clamp(1, 150) as i32).unwrap_or(150);
        let readable = unsafe { ffi::poll(poll_fds.as_mut_ptr(), poll_fds.len(), timeout) };
        let mut activity = false;
        if readable > 0 {
            for (file, entry) in files.iter_mut().zip(poll_fds.iter_mut()) {
                if entry.revents & (ffi::POLLERR | ffi::POLLHUP | ffi::POLLNVAL) != 0 {
                    entry.fd = -1;
                    continue;
                }
                if entry.revents & ffi::POLLIN == 0 {
                    continue;
                }
                let mut chunk = [0u8; 24 * 32];
                if let Ok(n) = file.read(&mut chunk) {
                    activity |= touch_activity(&chunk[..n]);
                }
            }
        }
        if poll_fds.iter().all(|entry| entry.fd < 0) {
            break;
        }
        let Ok(state) = ctx.live.inner.lock() else {
            previous_activity = activity;
            continue;
        };
        if !state.allowed {
            pulse = Pulse::default();
            previous_activity = activity;
            continue;
        }
        // 触摸按下沿唤醒主循环：通知栏/超级岛这类不改变 top-app 的 UI 突发
        // 依赖前台查询，按下即查比 2 秒心跳快一个数量级。
        if activity && !previous_activity && ctx.touch_signal_fd >= 0 {
            let value = 1u64;
            unsafe {
                ffi::write(
                    ctx.touch_signal_fd,
                    &value as *const u64 as *const ffi::c_void,
                    std::mem::size_of::<u64>(),
                );
            }
        }
        previous_activity = activity;
        if !activity && pulse.value.is_none() { continue; }
        let Ok(current) = crate::util::read_trimmed(&ctx.node) else {
            continue;
        };
        let mut bounded = state.clone();
        if let Some(maximum) = crate::util::read_trimmed(ctx.node.with_file_name("cpu.uclamp.max")).ok().and_then(|value| percent(&value)) {
            if percent(&bounded.boost_min).is_some_and(|value| value > maximum) {
                bounded.boost_min = format!("{}.{:02}", maximum / 100, maximum % 100);
            }
        }
        if let Some(value) = pulse.update(&bounded, Instant::now(), activity, &current, &ctx.stock) {
            if let Err(error) = ctx.snapshot.write(&ctx.node, &value) {
                ctx.logger.debug(format!("输入加速写入失败: {error}"));
                pulse = Pulse::default();
            }
        }
    }
    if let Ok(state) = ctx.live.inner.lock() {
        if let Ok(current) = crate::util::read_trimmed(&ctx.node) {
            let released = CpuctlLiveState {
                active: false,
                ..state.clone()
            };
            if let Some(value) =
                pulse.update(&released, Instant::now(), false, &current, &ctx.stock)
            {
                let _ = ctx.snapshot.write(&ctx.node, &value);
            }
        }
    }
}

fn touchscreen_capabilities(abs: &str, keys: &str) -> bool {
    fn has(raw: &str, bit: usize) -> bool {
        raw.split_whitespace().rev().nth(bit / 64)
            .and_then(|word| u64::from_str_radix(word, 16).ok())
            .is_some_and(|word| word & (1u64 << (bit % 64)) != 0)
    }
    has(keys, 0x14a) || (has(abs, 0x35) && has(abs, 0x36))
}

// Linux input_event is 24 bytes on the supported 64-bit targets.
fn touch_activity(bytes: &[u8]) -> bool {
    bytes.chunks_exact(24).any(|event| {
        let kind = u16::from_ne_bytes([event[16], event[17]]);
        let code = u16::from_ne_bytes([event[18], event[19]]);
        let value = i32::from_ne_bytes(event[20..24].try_into().unwrap());
        (kind == 3 && (code <= 1 || (0x2f..=0x3b).contains(&code)) && value >= 0)
            || (kind == 1 && code == 0x14a && value > 0)
    })
}

// The cgroup interface returns rational percentages, e.g. "30.00", even
// after writing "30". Compare hundredths rather than text representations.
fn percent(raw: &str) -> Option<u32> {
    if raw == "max" {
        return Some(10000);
    }
    let (whole, fraction) = raw.split_once('.').unwrap_or((raw, ""));
    if whole.is_empty()
        || !whole.bytes().all(|b| b.is_ascii_digit())
        || fraction.len() > 2
        || !fraction.bytes().all(|b| b.is_ascii_digit())
    {
        return None;
    }
    let decimal = if fraction.is_empty() {
        0
    } else {
        fraction.parse::<u32>().ok()? * if fraction.len() == 1 { 10 } else { 1 }
    };
    let value = whole
        .parse::<u32>()
        .ok()?
        .checked_mul(100)?
        .checked_add(decimal)?;
    (value <= 10000).then_some(value)
}

fn equivalent(a: &str, b: &str) -> bool {
    percent(a).zip(percent(b)).is_some_and(|(a, b)| a == b)
}

#[derive(Default)]
struct Pulse {
    value: Option<String>,
    until: Option<Instant>,
}

impl Pulse {
    fn update(
        &mut self,
        state: &CpuctlLiveState,
        now: Instant,
        activity: bool,
        current: &str,
        stock: &str,
    ) -> Option<String> {
        if !state.allowed {
            *self = Self::default();
            return None;
        }
        if self
            .value
            .as_deref()
            .is_some_and(|value| !equivalent(value, current))
        {
            *self = Self::default();
        }
        let baseline = if state.top_app_min.is_empty() {
            stock
        } else {
            &state.top_app_min
        };
        if activity && state.active && !state.boost_min.is_empty() {
            let target = if percent(baseline)? >= percent(&state.boost_min)? {
                baseline.to_string()
            } else {
                state.boost_min.clone()
            };
            self.until = Some(now + Duration::from_millis(state.duration_ms));
            self.value = Some(target.clone());
            return (!equivalent(&target, current)).then_some(target);
        }
        if self.value.is_some() && (!state.active || self.until.is_some_and(|until| now >= until)) {
            *self = Self::default();
            return (!equivalent(baseline, current)).then(|| baseline.to_string());
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn state() -> CpuctlLiveState {
        CpuctlLiveState {
            allowed: true,
            active: true,
            boost_min: "30".into(),
            top_app_min: "15".into(),
            duration_ms: 300,
        }
    }
    #[test]
    fn expired_pulse_restores_latest_baseline_and_does_not_reduce_a_higher_floor() {
        let now = Instant::now();
        let mut pulse = Pulse::default();
        let mut state = state();
        assert_eq!(
            pulse.update(&state, now, true, "15", "0"),
            Some("30".into())
        );
        state.top_app_min = "20".into();
        assert_eq!(
            pulse.update(&state, now + Duration::from_millis(301), false, "30", "0"),
            Some("20".into())
        );
        state.top_app_min = "40".into();
        assert_eq!(pulse.update(&state, now, true, "40", "0"), None);
        assert_eq!(pulse.value.as_deref(), Some("40"));
    }
    #[test]
    fn scene_takeover_and_dispatcher_changes_are_never_overwritten() {
        let now = Instant::now();
        let mut pulse = Pulse::default();
        let mut state = state();
        pulse.update(&state, now, true, "15", "0");
        state.allowed = false;
        assert_eq!(
            pulse.update(&state, now + Duration::from_secs(1), true, "30", "0"),
            None
        );
        state.allowed = true;
        assert_eq!(pulse.update(&state, now, false, "55", "0"), None);
        pulse.update(&state, now, true, "15", "0");
        assert_eq!(
            pulse.update(&state, now + Duration::from_secs(1), false, "25", "0"),
            None
        );
    }
    #[test]
    fn volume_keys_and_sync_events_do_not_trigger_touch_boost() {
        let event = |kind: u16, code: u16, value: i32| {
            let mut raw = [0u8; 24];
            raw[16..18].copy_from_slice(&kind.to_ne_bytes());
            raw[18..20].copy_from_slice(&code.to_ne_bytes());
            raw[20..24].copy_from_slice(&value.to_ne_bytes());
            raw
        };
        assert!(!touch_activity(&event(1, 115, 1)));
        assert!(!touch_activity(&event(0, 0, 0)));
        assert!(touch_activity(&event(3, 0x35, 90)));
        assert!(touch_activity(&event(1, 0x14a, 1)));
        assert!(!touch_activity(&event(3, 0x39, -1)));
    }

    #[test]
    fn decimal_kernel_readbacks_keep_pulse_ownership_and_restore_fractional_stock() {
        let now = Instant::now();
        let mut pulse = Pulse::default();
        let mut state = state();
        state.top_app_min.clear();
        assert_eq!(
            pulse.update(&state, now, true, "12.34", "12.34"),
            Some("30".into())
        );
        assert_eq!(
            pulse.update(
                &state,
                now + Duration::from_millis(301),
                false,
                "30.00",
                "12.34"
            ),
            Some("12.34".into())
        );
        state.top_app_min = "40.25".into();
        assert_eq!(pulse.update(&state, now, true, "40.25", "12.34"), None);
        assert!(equivalent("max", "100.00"));
        for invalid in ["NaN", "inf", "100.01", "-1", "1e2", "101", "1.234", ""] {
            assert_eq!(percent(invalid), None, "{invalid}");
        }
    }
    #[test]
    fn sensor_and_button_events_cannot_keep_touch_boost_alive() {
        assert!(!touchscreen_capabilities("7", "0"));
        assert!(!touchscreen_capabilities("0", "100000 0 0 0 0 0"));
        assert!(touchscreen_capabilities("60000000000000", "0"));
        assert!(touchscreen_capabilities("3", "400 0 0 0 0 0"));
        assert!(!touchscreen_capabilities("", ""));
    }

}
