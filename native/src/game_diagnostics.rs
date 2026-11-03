//! Read-only diagnostics. No scheduler or snapshot is created by this command.
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::config::Config;
use crate::hardware;
use crate::util::{self, Result, STATE_DIR};

pub fn run(module: &Path) -> Result<()> {
    if unsafe { crate::ffi::geteuid() } != 0 {
        return Err("必须以 root 身份运行".into());
    }
    println!(
        "NovaSched {} game-diagnose\ntime={}\nmodel={}\ndevice={}",
        env!("CARGO_PKG_VERSION"),
        util::local_timestamp(),
        util::getprop("ro.product.model"),
        util::getprop("ro.product.device")
    );
    if let Err(error) = crate::daemon::print_status(module) {
        println!("status_error={error}");
    }
    print!("{}", kernel_report(Path::new("/")));
    print_configured_limits();
    print!("\n[fas]\n{}", crate::fas::inspect(Path::new("/")).report());
    println!("\n[samples: 10 seconds; curfreq may be a requested frequency]");
    let policies = policy_paths(Path::new("/"));
    for index in 0..11 {
        let cpu = policies
            .iter()
            .map(|path| {
                format!(
                    "{}:cur={},max={}",
                    path.file_name().unwrap_or_default().to_string_lossy(),
                    value(&path.join("scaling_cur_freq")),
                    value(&path.join("scaling_max_freq"))
                )
            })
            .collect::<Vec<_>>()
            .join(" ");
        println!("{} sample={} {} gpu_hz={} gpu_cap={} gpu_busy={} battery_temp_raw={} current_raw={} voltage_raw={}",
            util::local_timestamp(), index, cpu,
            value(Path::new("/sys/class/kgsl/kgsl-3d0/gpuclk")),
            value(Path::new("/sys/class/kgsl/kgsl-3d0/max_pwrlevel")),
            value(Path::new("/sys/class/kgsl/kgsl-3d0/gpubusy")),
            value(Path::new("/sys/class/power_supply/battery/temp")),
            value(Path::new("/sys/class/power_supply/battery/current_now")),
            value(Path::new("/sys/class/power_supply/battery/voltage_now")));
        if index < 10 {
            util::sleep(Duration::from_secs(1));
        }
    }
    println!(
        "\n[scene-telemetry]\n{}",
        crate::telemetry::inspect_scene().diagnostic_text()
    );
    println!("\n[recent-log]");
    match crate::lifecycle::tail(&crate::storage::path(Path::new(STATE_DIR), "novasched.log"), 8192) {
        Ok(text) => print!("{text}"),
        Err(error) => println!("unavailable: {error}"),
    }
    println!("\n单次频率和温度读数不能证明掉帧原因；请在出现掉帧时采集并与 Scene 帧率时间对应。");
    Ok(())
}

fn print_configured_limits() {
    println!("\n[configured-four-profile-limits: normalized targets, not measured efficiency]");
    let result = (|| -> Result<()> {
        let mut config = Config::load(Path::new(util::CONFIG_PATH))?;
        let hardware = hardware::detect()?;
        println!(
            "soc={}\nsoc_id={}\nprofile={}",
            hardware.soc.name(),
            hardware.soc.id(),
            hardware.soc.file()
        );
        for adaptation in hardware.adapt_config(&mut config)? {
            println!("adaptation={adaptation}");
        }
        for mode in crate::config::MODES {
            let profile = config.profile(mode)?;
            for cluster in 0..4 {
                if config.policy[cluster] < 0 {
                    continue;
                }
                let base = Path::new("/sys/devices/system/cpu/cpufreq")
                    .join(format!("policy{}", config.policy[cluster]));
                let data = &profile.clusters[cluster];
                println!("mode={mode} c{cluster} policy={} configured_max_khz={} target_max_khz={} target_min_khz={} governor={}", config.policy[cluster],
                    data.max_freq, crate::scheduler::normalize_frequency(&base, &data.max_freq, false),
                    crate::scheduler::normalize_frequency(&base, &data.min_freq, true), data.governor);
            }
        }
        Ok(())
    })();
    if let Err(error) = result {
        println!("unavailable: {error}");
    }
}

fn value(path: &Path) -> String {
    match util::read_trimmed(path) {
        Ok(text) => text.replace(['\n', '\r'], " "),
        Err(error) => format!("unavailable({error})"),
    }
}

fn rooted(root: &Path, relative: &str) -> PathBuf {
    root.join(relative.trim_start_matches('/'))
}

fn policy_paths(root: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(rooted(root, "sys/devices/system/cpu/cpufreq")) else {
        return Vec::new();
    };
    let mut policies: Vec<_> = entries
        .filter_map(|entry| entry.ok())
        .filter(|entry| {
            entry
                .file_name()
                .to_str()
                .and_then(|name| name.strip_prefix("policy"))
                .is_some_and(|number| number.parse::<u32>().is_ok())
        })
        .map(|entry| entry.path())
        .collect();
    policies.sort();
    policies.truncate(8);
    policies
}

fn append_values(report: &mut String, base: &Path, names: &[&str]) {
    for name in names {
        let path = base.join(name);
        if path.exists() {
            report.push_str(&format!("{}={}\n", path.display(), value(&path)));
        }
    }
}

/// 厂商限频/锁帧机制只读采集。游戏被锁 60/90 时先看这里：这些节点是
/// 温控、厂商游戏助手和 WALT 调度的真实约束来源，模块不修改它们——
/// 诊断输出用于判断“限制来自哪里”，而不是绕过。
pub(crate) fn vendor_limit_report(root: &Path) -> String {
    let mut report = String::from("\n[vendor-limit: read-only evidence of external limiters]\n");
    let mut found = 0usize;
    let mut section = String::new();
    let mut probe = |base: &Path, names: &[&str], out: &mut String| {
        let before = out.len();
        append_values(out, base, names);
        if out.len() > before {
            found += 1;
        }
    };
    // 高通 WALT 全局频率上限（温控/厂商服务常用钳制点）。
    probe(
        &rooted(root, "proc/sys/walt"),
        &["sched_fmax_cap", "sched_sbt_enable", "walt_rtg_cfs_boost_prio"],
        &mut section,
    );
    // 小米/澎湃：温控档位、逐核热限频、MIGT 游戏提频、metis 调度。
    probe(
        &rooted(root, "sys/class/thermal/thermal_message"),
        &["sconfig", "cpu_limits", "market_download_limit"],
        &mut section,
    );
    probe(
        &rooted(root, "sys/module/migt/parameters"),
        &["migt_freq", "migt_ceiling_freq", "enable_pkg_monitor", "affinity_only"],
        &mut section,
    );
    probe(
        &rooted(root, "sys/module/metis/parameters"),
        &["mi_freq_enable", "user_min_freq", "thermal_break_enable"],
        &mut section,
    );
    // msm_performance / 一加 game_opt 的静态 min/max 钳制。
    probe(
        &rooted(root, "sys/module/msm_performance/parameters"),
        &["cpu_max_freq", "cpu_min_freq"],
        &mut section,
    );
    probe(
        &rooted(root, "proc/game_opt"),
        &["cpu_max_freq", "cpu_min_freq"],
        &mut section,
    );
    // 小米面板实际输出帧率（max_fps_clone 为厂商信息流锁帧的写入点）。
    for panel in ["disp-DSI-0", "dsi-0-mi_display"] {
        probe(
            &rooted(root, &format!("sys/class/mi_display/{panel}")),
            &["dynamic_fps", "max_fps_clone", "fps_info"],
            &mut section,
        );
    }
    report.push_str(&section);
    // 应用请求/系统允许的刷新率设置（面板 Hz ≠ 游戏 FPS）。
    for key in ["peak_refresh_rate", "min_refresh_rate", "user_refresh_rate"] {
        let setting = util::run_command("settings", &["get", "system", key])
            .map(|v| v.trim().to_string())
            .unwrap_or_else(|_| "unavailable".into());
        report.push_str(&format!("settings.system.{key}={setting}\n"));
    }
    if found == 0 {
        report.push_str("no vendor limiter nodes found\n");
    }
    report
}

fn kernel_report(root: &Path) -> String {
    let mut report = String::from("\n[cpu: frequencies in kHz]\n");
    let policies = policy_paths(root);
    if policies.is_empty() {
        report.push_str("cpufreq policies unavailable\n");
    }
    for policy in policies {
        append_values(
            &mut report,
            &policy,
            &[
                "related_cpus",
                "scaling_governor",
                "scaling_min_freq",
                "scaling_max_freq",
                "scaling_cur_freq",
                "cpuinfo_min_freq",
                "cpuinfo_max_freq",
                "cpuinfo_cur_freq",
                "scaling_available_frequencies",
            ],
        );
        let governor = util::read_trimmed(policy.join("scaling_governor")).unwrap_or_default();
        if !governor.is_empty()
            && governor
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_')
        {
            append_values(
                &mut report,
                &policy.join(governor),
                &[
                    "hispeed_freq",
                    "rtg_boost_freq",
                    "target_loads",
                    "hispeed_load",
                    "boost",
                    "up_rate_limit_us",
                    "down_rate_limit_us",
                    "pl",
                ],
            );
        }
    }
    report.push_str("\n[cpuset/uclamp]\n");
    append_values(
        &mut report,
        &rooted(root, "sys/devices/system/cpu"),
        &["online", "offline"],
    );
    let cpu_root = rooted(root, "sys/devices/system/cpu");
    let indices: std::collections::BTreeSet<u32> = fs::read_dir(&cpu_root)
        .into_iter().flatten().filter_map(|entry| {
            let name = entry.ok()?.file_name();
            let index = name.to_str()?.strip_prefix("cpu")?.parse::<u32>().ok()?;
            (index < crate::config::MAX_CPU_COUNT as u32).then_some(index)
        }).collect();
    for index in indices {
        append_values(
            &mut report,
            &rooted(root, &format!("sys/devices/system/cpu/cpu{index}")),
            &["thermal_pressure", "cpu_capacity"],
        );
    }
    for group in ["top-app", "foreground"] {
        append_values(
            &mut report,
            &rooted(root, &format!("dev/cpuset/{group}")),
            &["cpus", "mems"],
        );
        append_values(
            &mut report,
            &rooted(root, &format!("sys/fs/cgroup/{group}")),
            &[
                "cpuset.cpus",
                "cpuset.cpus.effective",
                "cpu.uclamp.min",
                "cpu.uclamp.max",
            ],
        );
        append_values(
            &mut report,
            &rooted(root, &format!("dev/cpuctl/{group}")),
            &[
                "cpu.uclamp.min",
                "cpu.uclamp.max",
                "cpu.uclamp.latency_sensitive",
            ],
        );
    }
    append_values(
        &mut report,
        &rooted(root, "proc/sys/kernel"),
        &["sched_util_clamp_min", "sched_util_clamp_max"],
    );
    report.push_str("\n[gpu: gpuclk/devfreq frequencies in Hz]\n");
    let gpu = rooted(root, "sys/class/kgsl/kgsl-3d0");
    if !gpu.exists() {
        report.push_str("KGSL interface unavailable\n");
    }
    append_values(
        &mut report,
        &gpu,
        &[
            "gpuclk",
            "gpubusy",
            "min_pwrlevel",
            "max_pwrlevel",
            "default_pwrlevel",
            "num_pwrlevels",
            "thermal_pwrlevel",
            "throttling",
            "bus_split",
        ],
    );
    append_values(
        &mut report,
        &gpu.join("devfreq"),
        &[
            "cur_freq",
            "min_freq",
            "max_freq",
            "governor",
            "available_frequencies",
        ],
    );
    // MTK Mali 平台没有 kgsl/devfreq 接口，频率与负载走 gpufreqv2/GED。
    if !gpu.exists() {
        append_values(
            &mut report,
            &rooted(root, "sys/kernel/ged/hw"),
            &["gpu_freq", "gpu_meter", "gpu_busy"],
        );
    }
    report.push_str(&vendor_limit_report(root));
    report.push_str("\n[thermal: raw temp values; standard thermal ABI is millidegrees C]\n");
    let thermal = rooted(root, "sys/class/thermal");
    match fs::read_dir(&thermal) {
        Ok(entries) => {
            let mut devices: Vec<_> = entries
                .filter_map(|entry| entry.ok())
                .map(|entry| entry.path())
                .collect();
            devices.sort();
            let mut count = 0;
            for device in devices {
                let kind = value(&device.join("type"));
                let lower = kind.to_ascii_lowercase();
                if !["cpu", "gpu", "soc", "battery", "tsens", "skin", "cluster"]
                    .iter()
                    .any(|name| lower.contains(name))
                {
                    continue;
                }
                report.push_str(&format!("{} type={kind}\n", device.display()));
                append_values(&mut report, &device, &["temp", "cur_state", "max_state"]);
                count += 1;
                if count >= 64 {
                    break;
                }
            }
        }
        Err(error) => report.push_str(&format!("thermal unavailable: {error}\n")),
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diagnostics_read_kernel_controls_without_modifying_files() {
        let root = std::env::temp_dir().join(format!("nova-game-diag-{}", std::process::id()));
        let cpu = rooted(&root, "sys/devices/system/cpu/cpufreq/policy0");
        let gpu = rooted(&root, "sys/class/kgsl/kgsl-3d0");
        let cpuset = rooted(&root, "dev/cpuset/top-app");
        let thermal = rooted(&root, "sys/class/thermal/thermal_zone0");
        for path in [&cpu, &gpu, &cpuset, &thermal] {
            fs::create_dir_all(path).expect("fixture");
        }
        let files = [
            (cpu.join("scaling_max_freq"), "2265600"),
            (cpu.join("scaling_governor"), "walt"),
            (gpu.join("max_pwrlevel"), "3"),
            (cpuset.join("cpus"), "0-5"),
            (thermal.join("type"), "soc"),
            (thermal.join("temp"), "45000"),
        ];
        for (path, text) in &files {
            fs::write(path, text).expect("fixture file");
        }
        let report = kernel_report(&root);
        for expected in [
            "scaling_max_freq=2265600",
            "max_pwrlevel=3",
            "cpus=0-5",
            "temp=45000",
        ] {
            assert!(report.contains(expected), "{expected}");
        }
        for (path, original) in files {
            assert_eq!(fs::read_to_string(path).expect("read"), original);
        }
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn diagnostics_tolerate_missing_optional_kernel_interfaces() {
        let root = std::env::temp_dir().join(format!("nova-no-kernel-{}", std::process::id()));
        let report = kernel_report(&root);
        assert!(report.contains("cpufreq policies unavailable"));
        assert!(report.contains("KGSL interface unavailable"));
        assert!(report.contains("thermal unavailable"));
    }

    #[test]
    fn vendor_limit_section_reports_present_and_absent_limiters() {
        // 有 WALT 上限与 MIUI 热限频节点时输出原值。
        let root = std::env::temp_dir().join(format!("nova-vendor-{}", std::process::id()));
        let walt = rooted(&root, "proc/sys/walt");
        let message = rooted(&root, "sys/class/thermal/thermal_message");
        fs::create_dir_all(&walt).unwrap();
        fs::create_dir_all(&message).unwrap();
        fs::write(walt.join("sched_fmax_cap"), "2265600 3148800 2956800 3302400").unwrap();
        fs::write(message.join("sconfig"), "0").unwrap();
        let report = kernel_report(&root);
        assert!(report.contains("sched_fmax_cap=2265600 3148800 2956800 3302400"));
        assert!(report.contains("sconfig=0"));
        // settings 命令在真机上可能返回真实值（如 120），只断言行存在。
        assert!(report.contains("settings.system.peak_refresh_rate="));
        fs::remove_dir_all(root).unwrap();
        // 完全没有厂商节点时明确说明，不输出误导性的空段。
        let empty = std::env::temp_dir().join(format!("nova-vendor-empty-{}", std::process::id()));
        fs::create_dir_all(&empty).unwrap();
        let report = kernel_report(&empty);
        assert!(report.contains("no vendor limiter nodes found"));
        fs::remove_dir_all(empty).unwrap();
    }
}
