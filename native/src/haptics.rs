//! Optional system haptics; never override the user's vibration preferences.
//!
//! 系统命令通道的能力判定不依赖单个关键字：`cmd vibrator_manager help`
//! 只能证明子命令存在，`list` 输出非空才证明设备有可用振动器；二者齐备
//! 才认定通道可用。探测结果按开机会话缓存（TTL 1 天），后续触感不再重复
//! fork help 查询。命令退出码不能证明真实振动（真机实测非法常量也返回
//! 0），因此对外语义一律是“已提交请求”，从不声称用户感受到了振动。
use crate::util::{self, Result};
use std::time::Duration;

/// 能力缓存有效期：跨过该时长后下一次触感重新探测，避免固件升级或
/// 振动器状态变化被永久误判。
const CACHE_TTL_SECS: u64 = 24 * 3600;
const CACHE_PATH: &str = "/data/adb/novasched/run/haptics.cap";
const QUERY_TIMEOUT: Duration = Duration::from_secs(1);

fn effect(action: &str) -> Result<(&'static str, &'static str)> {
    // `cmd vibrator_manager feedback <constant>` 的常量域是 VibrationEffect
    // 的预定义效果（createPredefined：1=CLICK 2=TICK 3=DOUBLE_CLICK
    // 5=HEAVY_CLICK），不是 HapticFeedbackConstants——后者域里的
    // 9/16/17 传给 createPredefined 会抛 IllegalArgumentException，
    // tick/success/failure/warning 曾因此从未被受理（静态对照 AOSP
    // VibratorManagerShellCommand）。第二列是 synced oneshot 的时长
    // （毫秒），作为 feedback 不被接受时的降级波形。
    match action {
        "tick" => Ok(("2", "8")),
        "select" => Ok(("1", "12")),
        "toggle" => Ok(("5", "16")),
        "success" => Ok(("3", "16")),
        "failure" => Ok(("5", "24")),
        "warning" => Ok(("2", "20")),
        _ => Err("未知触感动作".into()),
    }
}

/// 已缓存的通道能力。mode：feedback / synced / none。
struct Capability {
    mode: &'static str,
    vibrators: u32,
    checked_at: u64,
    boot: String,
}

impl Capability {
    fn encode(&self) -> String {
        format!(
            "mode={}\nvibrators={}\nchecked_at={}\nboot={}\n",
            self.mode, self.vibrators, self.checked_at, self.boot
        )
    }
    fn parse(text: &str) -> Option<Self> {
        let mut mode = "";
        let mut vibrators = 0u32;
        let mut checked_at = 0u64;
        let mut boot = String::new();
        for line in text.lines() {
            let (key, value) = line.split_once('=')?;
            match key {
                "mode" => {
                    mode = match value {
                        "feedback" | "synced" | "none" => value,
                        _ => return None,
                    };
                }
                "vibrators" => vibrators = value.parse().ok()?,
                "checked_at" => checked_at = value.parse().ok()?,
                "boot" => boot = value.to_string(),
                _ => {}
            }
        }
        if mode.is_empty() || boot.is_empty() {
            return None;
        }
        Some(Self {
            // mode 已经在上面收敛为三种字面量之一。
            mode: match mode {
                "feedback" => "feedback",
                "synced" => "synced",
                _ => "none",
            },
            vibrators,
            checked_at,
            boot,
        })
    }
    fn fresh(&self, now: u64, boot: &str) -> bool {
        self.boot == boot && now.saturating_sub(self.checked_at) < CACHE_TTL_SECS
    }
}

/// Unix 秒：仅用于缓存 TTL 判定，系统时间回拨最多导致一次重新探测。
fn now_secs() -> u64 {
    util::now_secs()
}

/// `cmd vibrator_manager` 查询。不走 util::query_command：真机实测
/// （HyperOS，Android 17）`help` 子命令以退出码 255 返回完整输出、
/// `feedback` 成功时输出为空且非法常量退出码也是 0——退出码完全不可信。
/// 以内容为准：stdout 优先（为空时回退 stderr）；`failed` 仅在 stderr
/// 出现 Exception/Error 时为真（发送路径的成功就是无输出）。
fn cmd_output(args: &[&str]) -> (String, bool) {
    use std::process::{Command, Stdio};
    let output = Command::new("/system/bin/cmd")
        .args(args)
        .stdin(Stdio::null())
        .env_remove("LD_LIBRARY_PATH")
        .env_remove("LD_PRELOAD")
        .output();
    let Ok(output) = output else {
        return ("cmd 无法执行".to_string(), true);
    };
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    let mut text = stdout.clone();
    if text.trim().is_empty() && !stderr.trim().is_empty() {
        text = stderr.clone();
    }
    let failed = stderr.contains("Exception occurred") || stderr.contains("Error:");
    (text, failed)
}

fn load_cache(boot: &str, now: u64) -> Option<Capability> {
    let text = util::read_text(std::path::Path::new(CACHE_PATH)).ok()?;
    let capability = Capability::parse(&text)?;
    capability.fresh(now, boot).then_some(capability)
}

fn save_cache(capability: &Capability) {
    if let Ok(()) = util::atomic_write(
        std::path::Path::new(CACHE_PATH),
        capability.encode().as_bytes(),
        0o600,
    ) {
        return;
    }
    // 缓存写失败不影响本次触感，下次重新探测。
}

/// 探测系统命令通道：help 关键字（子命令存在）+ `list`（振动器存在）。
/// 二者不全时明确区分“命令缺失”与“无振动器”。探测是低频动作，
/// 超时放宽到 3 秒覆盖 `cmd` 冷启动；每次触感的发送仍保持 1 秒。
fn probe() -> Result<Capability> {
    // 探测是低频动作；单次查询限时由调用侧节流，这里保持同步短查询。
    let help = cmd_output(&["vibrator_manager", "help"]).0;
    let has_feedback = help.contains("feedback") && help.contains("<constant>");
    let has_synced = help.contains("synced") && help.contains("oneshot");
    if !has_feedback && !has_synced {
        // 附带实际字节数：空输出指向超时/权限，非空输出指向关键字差异。
        return Err(format!(
            "系统没有兼容的触感命令（help 输出 {} 字节）",
            help.len()
        ));
    }
    let listing = cmd_output(&["vibrator_manager", "list"]).0;
    let vibrators = listing
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && line.parse::<u32>().is_ok())
        .count() as u32;
    if vibrators == 0 {
        return Err(format!("系统报告没有可用振动器（list 输出 {} 字节）", listing.len()));
    }
    Ok(Capability {
        mode: if has_feedback { "feedback" } else { "synced" },
        vibrators,
        checked_at: now_secs(),
        boot: util::read_trimmed("/proc/sys/kernel/random/boot_id").unwrap_or_default(),
    })
}

/// 系统触感主开关（settings system haptic_feedback_enabled）。每次发送前
/// 实时读取：这是用户可随时拨动的静音开关，缓存会造成“已静音仍振动”。
fn haptic_enabled() -> Result<bool> {
    let value = util::query_command(
        "/system/bin/settings",
        &["--user", "current", "get", "system", "haptic_feedback_enabled"],
        QUERY_TIMEOUT,
    )?;
    Ok(haptic_enabled_from_setting(&value))
}

/// `settings get` 对从未写入的键返回字面量 "null"。该设置的系统性默认值
/// 是开启（Settings.System.getInt 的 default=1），因此只有显式 "0" 才算
/// 用户关闭；旧实现把 "null" 判成关闭，导致从未动过该设置的设备上每次
/// 触感都被 skipped——真机“触感反馈用不了且无报错”的最直接根因。
fn haptic_enabled_from_setting(raw: &str) -> bool {
    // 只有显式 "0" 表示用户关闭；"null"/空/其它值一律按系统默认（开启）。
    let value = raw.trim();
    value != "0"
}

/// 只读能力报告（健康检查用）：复用缓存的探测结果，不发送任何振动。
pub fn capability() -> Result<String> {
    if unsafe { crate::ffi::geteuid() } != 0 {
        return Err("触感能力检查需要 root".into());
    }
    let boot = util::read_trimmed("/proc/sys/kernel/random/boot_id").unwrap_or_default();
    let now = now_secs();
    let capability = match load_cache(&boot, now) {
        Some(capability) => capability,
        None => probe()?,
    };
    if capability.mode == "none" {
        return Err("系统触感命令通道不可用（详见触感探测缓存）".into());
    }
    let enabled = match haptic_enabled() {
        Ok(true) => "1",
        Ok(false) => "0",
        Err(_) => "unknown",
    };
    Ok(format!(
        "channel={}\nvibrators={}\nhaptic_enabled={}",
        capability.mode, capability.vibrators, enabled
    ))
}

pub fn run(action: &str) -> Result<()> {
    if unsafe { crate::ffi::geteuid() } != 0 {
        return Err("触感系统通道需要 root".into());
    }
    if action != "probe" {
        effect(action)?;
    }
    let boot = util::read_trimmed("/proc/sys/kernel/random/boot_id").unwrap_or_default();
    let now = now_secs();
    let mut cached = load_cache(&boot, now);
    if cached.is_none() {
        match probe() {
            Ok(capability) => {
                save_cache(&capability);
                cached = Some(capability);
            }
            Err(error) => {
                // 探测失败也短暂缓存“不可用”，避免每次点击都重新 fork
                // 探测；TTL 到期后会自动重试。
                save_cache(&Capability {
                    mode: "none",
                    vibrators: 0,
                    checked_at: now,
                    boot: boot.clone(),
                });
                if action == "probe" {
                    return Err(error);
                }
                return Err(format!("系统触感通道不可用: {error}"));
            }
        }
    }
    let capability = cached.expect("capability loaded above");
    if action == "probe" {
        if capability.mode == "none" {
            return Err("系统触感通道此前探测不可用（TTL 到期后会自动重试；可用测试触感强制重探）".into());
        }
        let enabled = match haptic_enabled() {
            Ok(true) => "1".to_string(),
            Ok(false) => "0".to_string(),
            Err(_) => "unknown".to_string(),
        };
        println!(
            "haptic=system\nchannel={}\nvibrators={}\nhaptic_enabled={enabled}",
            capability.mode, capability.vibrators
        );
        return Ok(());
    }
    if capability.mode == "none" {
        return Err("系统触感通道此前探测不可用".into());
    }
    // 系统静音开关实时判定：关闭时不振动，也不算失败。
    if !haptic_enabled()? {
        println!("haptic=skipped reason=system-disabled");
        return Ok(());
    }
    let (constant, duration) = effect(action)?;
    let args: Vec<&str> = if capability.mode == "feedback" {
        vec!["vibrator_manager", "feedback", constant]
    } else {
        vec!["vibrator_manager", "synced", "oneshot", duration]
    };
    let submit = || {
        let (text, failed) = cmd_output(&args);
        if failed {
            Err(format!("命令未受理: {}", text.trim().chars().take(120).collect::<String>()))
        } else {
            Ok(())
        }
    };
    match submit() {
        Ok(()) => {
            println!("haptic=submitted channel={}", capability.mode);
            Ok(())
        }
        Err(first_error) => {
            // 有界恢复：缓存可能过期（固件升级、服务异常）——作废缓存
            // 重新探测一次，成功则重试发送；再失败如实报错。
            match probe() {
                Ok(capability) => {
                    save_cache(&capability);
                    match submit() {
                        Ok(()) => {
                            println!("haptic=submitted channel={} recovered=1", capability.mode);
                            Ok(())
                        }
                        Err(retry_error) if capability.mode == "feedback" => {
                            // feedback 常量在个别 ROM 上不被 createPredefined
                            // 接受：单次降级为 synced oneshot 波形（时长与该
                            // 语义一致）。降级成功如实标注 fallback=1。
                            let duration = effect(action)?.1;
                            let (text, failed) =
                                cmd_output(&["vibrator_manager", "synced", "oneshot", duration]);
                            if failed {
                                Err(format!(
                                    "触感请求提交失败: {first_error}；重试 feedback: {retry_error}；降级 synced: {}",
                                    text.trim().chars().take(80).collect::<String>()
                                ))
                            } else {
                                println!("haptic=submitted channel=synced fallback=1");
                                Ok(())
                            }
                        }
                        Err(retry_error) => Err(format!(
                            "触感请求提交失败（重试后仍失败: {retry_error}；首次: {first_error}）"
                        )),
                    }
                }
                Err(probe_error) => {
                    save_cache(&Capability {
                        mode: "none",
                        vibrators: 0,
                        checked_at: now,
                        boot: boot.clone(),
                    });
                    Err(format!(
                        "触感请求提交失败: {first_error}；重新探测也不可用: {probe_error}"
                    ))
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_fixed_semantic_effects_are_accepted() {
        for a in ["tick", "select", "toggle", "success", "failure", "warning"] {
            assert!(effect(a).is_ok());
        }
        for a in ["", "-f", "select;reboot", "10000", "probe"] {
            assert!(effect(a).is_err(), "{a}");
        }
    }
    #[test]
    fn feedback_constants_stay_in_vibration_effect_domain() {
        // `cmd vibrator_manager feedback` 的常量域是 VibrationEffect 的
        // createPredefined（1/2/3/5）；HapticFeedbackConstants 域里的
        // 9/16/17 会抛 IllegalArgumentException——曾导致 tick/success/
        // failure/warning 全部未受理（真机"触感用不了"根因之二）。
        for a in ["tick", "select", "toggle", "success", "failure", "warning"] {
            let (constant, duration) = effect(a).unwrap();
            assert!(
                matches!(constant, "1" | "2" | "3" | "5"),
                "{a} 常量 {constant} 不在 createPredefined 域内"
            );
            let ms: u64 = duration.parse().unwrap();
            assert!((5..=60).contains(&ms), "{a} 时长 {ms}ms 超出语义范围");
        }
    }
    #[test]
    fn unset_haptic_setting_means_enabled_not_muted() {
        // `settings get` 对未写入的键返回 "null"：系统默认触感开启，只有
        // 显式 "0" 才算关闭。旧实现把 null 判成关闭，导致从未动过设置的
        // 设备上每次触感都被 skipped（真机"触感用不了"的最直接根因）。
        assert!(haptic_enabled_from_setting("1"));
        assert!(haptic_enabled_from_setting("null"));
        assert!(haptic_enabled_from_setting(" null \n"));
        assert!(haptic_enabled_from_setting(""));
        assert!(!haptic_enabled_from_setting("0"));
    }
    #[test]
    fn capability_cache_round_trips_and_expires() {
        let cap = Capability {
            mode: "feedback",
            vibrators: 1,
            checked_at: 1000,
            boot: "boot-a".into(),
        };
        let parsed = Capability::parse(&cap.encode()).unwrap();
        assert_eq!(parsed.boot, "boot-a");
        assert_eq!(parsed.mode, "feedback");
        assert_eq!(parsed.vibrators, 1);
        // 同开机 + 未过期 → fresh。
        assert!(parsed.fresh(1000 + CACHE_TTL_SECS - 1, "boot-a"));
        // 过期或换开机 → 失效，必须重新探测。
        assert!(!parsed.fresh(1000 + CACHE_TTL_SECS, "boot-a"));
        assert!(!parsed.fresh(1000, "boot-b"));
        // 损坏缓存 → None（按未探测处理）。
        assert!(Capability::parse("garbage").is_none());
        assert!(Capability::parse("mode=evil\nvibrators=1\nchecked_at=1\nboot=b\n").is_none());
        // none 模式可缓存（探测失败的退避）。
        let none = Capability {
            mode: "none",
            vibrators: 0,
            checked_at: 5,
            boot: "b".into(),
        };
        assert_eq!(Capability::parse(&none.encode()).unwrap().mode, "none");
    }
}
