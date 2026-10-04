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
    let scene = crate::scene::detect();
    println!(
        "scene_pedestal_active={}",
        crate::app_modes::scene_pedestal_active(scene.linked())
    );
    print!("{}", kernel_report(Path::new("/")));
    print_configured_limits();
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
    match crate::lifecycle::tail(&Path::new(STATE_DIR).join("novasched.log"), 8192) {
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
    for index in 0..8 {
        append_values(
            &mut report,
            &rooted(root, &format!("sys/devices/system/cpu/cpu{index}")),
            &["thermal_pressure"],
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
}
