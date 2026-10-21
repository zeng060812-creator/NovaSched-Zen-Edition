use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::config::{Config, ModeProfile};
use crate::hardware::{self, Hardware};
use crate::logging::Logger;
use crate::snapshot::Snapshot;
use crate::util::{self, Result};

/// Shared live state for the cpuctl top-app clamp, updated by the dispatch
/// loop and read by the input-boost thread.
#[derive(Default)]
pub struct CpuctlLive {
    pub inner: Mutex<CpuctlLiveState>,
}

#[derive(Clone, Debug, Default)]
pub struct CpuctlLiveState {
    /// The baseline cpu.uclamp.min the dispatcher maintains for top-app
    /// (rule floor while an explicitly ruled app is foreground, stock
    /// otherwise). Input-boost pulses expire back to this value.
    pub top_app_min: String,
    /// The pulse value applied on input activity; empty = disabled.
    pub boost_min: String,
    pub duration_ms: u64,
    /// False while a foreign scheduler owns the nodes.
    pub allowed: bool,
    /// True while an application is in the foreground. Pulses are short and
    /// mode-specific, including daily apps in powersave.
    pub active: bool,
}

pub struct Scheduler {
    config: Config,
    snapshot: Snapshot,
    logger: Logger,
    kernel_root: PathBuf,
    expected_controls: Vec<ExpectedControl>,
    current_mode: String,
    current_package: String,
    last_explicit: bool,
    hardware: Hardware,
    pending_note: String,
    /// Keep unconfirmed ceilings visible after the retry budget is spent.
    pending_enforcement: Vec<(PathBuf, String)>,
    enforcement_attempts: u8,
    enforcement_after: std::time::Instant,
    cpuctl_live: Arc<CpuctlLive>,
    open_drift: Option<Vec<String>>,
    /// Last mode pushed to Scene N1's FAS node, for change-only logging.
    fas_mode: Option<String>,
    /// 可调频 GPU 的 devfreq 目录（运行时能力检测；None = 本设备不可用，
    /// gpu_devfreq 特性降级为停用，不影响其它功能）。
    gpu_devfreq_base: Option<PathBuf>,
    extreme_power: bool,
}

#[derive(Clone, Copy, Debug, Default)]
struct OptionalWriteSummary {
    applied: usize,
    missing: usize,
    unsupported: usize,
    failed: usize,
}

impl OptionalWriteSummary {
    fn record(&mut self, outcome: OptionalWriteOutcome) {
        match outcome {
            OptionalWriteOutcome::Applied => self.applied += 1,
            OptionalWriteOutcome::Missing => self.missing += 1,
            OptionalWriteOutcome::Unsupported => self.unsupported += 1,
            OptionalWriteOutcome::Failed => self.failed += 1,
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum OptionalWriteOutcome {
    Applied,
    Missing,
    Unsupported,
    Failed,
}

/// SystemUI 前台（下拉通知栏、超级岛、控制中心）时的突发借用：省电/均衡
/// 档位下临时使用性能档的频率包络。HyperOS 超级岛的通知展开动画需要
/// burst CPU，低档位上限会把 SystemUI 动画压出掉帧；性能/极速档本身不受
/// 限。只借包络不改档位状态——通知栏收起、真实应用回到前台后立即回到
/// 原档位包络。三星保守策略不参与。
pub(crate) fn shell_burst(samsung: bool, package: &str, mode: &str) -> bool {
    !samsung && package == "com.android.systemui" && matches!(mode, "powersave" | "balance")
}

impl Scheduler {
    pub fn new(
        mut config: Config,
        snapshot: Snapshot,
        logger: Logger,
        hardware: Hardware,
        cpuctl_live: Arc<CpuctlLive>,
    ) -> Result<Self> {
        for change in hardware.adapt_config(&mut config)? {
            logger.info(format!("内核兼容适配: {change}"));
        }
        let gpu_devfreq_base = if config.functions.gpu_devfreq.enabled {
            match detect_gpu_devfreq(Path::new("/")) {
                Some(base) => {
                    logger.info(format!("GPU 档位控制接口: {}", base.display()));
                    Some(base)
                }
                None => {
                    logger.warn(
                        "未检测到可调频的 GPU devfreq 节点，gpu_devfreq 降级为停用；CPU 调度不受影响"
                            .to_string(),
                    );
                    None
                }
            }
        } else {
            None
        };
        Ok(Self {
            config,
            snapshot,
            logger,
            kernel_root: PathBuf::from("/"),
            expected_controls: Vec::new(),
            current_mode: String::new(),
            current_package: String::new(),
            last_explicit: false,
            hardware,
            pending_note: String::new(),
            pending_enforcement: Vec::new(),
            enforcement_attempts: 0,
            enforcement_after: std::time::Instant::now(),
            cpuctl_live,
            open_drift: None,
            fas_mode: None,
            gpu_devfreq_base,
            extreme_power: false,
        })
    }

    pub fn set_extreme_power(&mut self, enabled: bool) {
        self.extreme_power = enabled;
    }

    pub fn extreme_power_status(&self) -> &'static str {
        if !self.extreme_power {
            return "off";
        }
        let Ok(profile) = self.config.profile(&self.current_mode) else {
            return "unsupported";
        };
        let mut supported = 0usize;
        let mut applied = 0usize;
        for cluster in 0..4 {
            let policy = self.config.policy[cluster];
            if policy < 0 {
                continue;
            }
            let base = self.node(format!("/sys/devices/system/cpu/cpufreq/policy{policy}"));
            let Some(cap) = conservative_frequency_ceiling(&base, &profile.clusters[cluster].max_freq) else {
                continue;
            };
            supported += 1;
            if util::read_trimmed(base.join("scaling_max_freq"))
                .ok()
                .and_then(|value| value.parse::<u64>().ok())
                .is_some_and(|value| value <= cap)
            {
                applied += 1;
            }
        }
        match (supported, applied) {
            (0, _) => "unsupported",
            (count, applied) if count == applied => "active",
            (_count, applied) if applied > 0 => "partial",
            _ => "applying",
        }
    }

    pub fn replace_config(&mut self, mut config: Config) -> Result<()> {
        for change in self.hardware.adapt_config(&mut config)? {
            self.logger.info(format!("内核兼容适配: {change}"));
        }
        let old = &self.config.functions;
        let new = &config.functions;
        let hotplug_changed = self.hardware.cpus().into_iter().any(|cpu| {
            self.config.modes.values().any(|m| !m.cpu_online(cpu))
                != config.modes.values().any(|m| !m.cpu_online(cpu))
        });
        if old.cpuset != new.cpuset
            || old.cpuctl != new.cpuctl
            || old.input_boost != new.input_boost
            || old.scheduler != new.scheduler
            || old.perf_service_lock != new.perf_service_lock
            || old.disable_gpu_boost != new.disable_gpu_boost
            || old.gpu_devfreq != new.gpu_devfreq
            || hotplug_changed
        {
            return Err(
                "CPUSet、uclamp、输入加速、CFS、GPU、性能服务或热插拔控制范围发生变化，请重启模块；当前运行配置保留"
                    .into(),
            );
        }
        self.logger.set_level(&config.meta.loglevel);
        self.config = config;
        Ok(())
    }

    pub fn ignored_packages(&self) -> &BTreeSet<String> {
        &self.config.functions.foreground_monitor.ignored_packages
    }
    pub fn fas_config(&self) -> crate::config::FasConfig {
        self.config.functions.fas.clone()
    }

    pub fn current_mode(&self) -> &str {
        &self.current_mode
    }
    pub fn current_power_profile(&self) -> &str {
        if self.current_mode == "powersave" {
            "standard"
        } else {
            ""
        }
    }
    pub fn current_package(&self) -> &str {
        &self.current_package
    }
    pub fn last_explicit(&self) -> bool {
        self.last_explicit
    }

    pub fn initialize(&mut self, mode: &str) -> Result<()> {
        if self.config.functions.perf_service_lock.enable {
            for service in &self.config.functions.perf_service_lock.services {
                let lower = service.to_ascii_lowercase();
                if ["thermal", "health", "battery", "charger"]
                    .iter()
                    .any(|name| lower.contains(name))
                {
                    return Err(format!(
                        "性能服务列表不能停用温控、电池或充电服务: {service}"
                    ));
                }
            }
        }
        self.logger.set_level(&self.config.meta.loglevel);
        self.logger.info(format!("名称: {}", self.config.meta.name));
        self.logger
            .info(format!("版本: {}", self.config.meta.version));
        self.logger
            .info(format!("作者: {}", self.config.meta.author));
        self.logger
            .info(format!("日志等级: {}", self.config.meta.loglevel));
        self.capture_initial_nodes()?;
        self.snapshot.apply_transaction(|| self.apply_functions())?;
        self.apply("", mode, false, true, false)
    }

    pub fn apply(
        &mut self,
        package: &str,
        mode: &str,
        launch_boost: bool,
        force: bool,
        explicit: bool,
    ) -> Result<()> {
        let snapshot = self.snapshot.clone();
        let result = snapshot.apply_transaction(|| {
            self.apply_inner(package, mode, launch_boost, force, explicit, false)
        });
        if result.is_err() { self.clear_pending(); }
        result
    }
    fn apply_inner(
        &mut self,
        package: &str,
        mode: &str,
        launch_boost: bool,
        force: bool,
        explicit: bool,
        preserve_lower_caps: bool,
    ) -> Result<()> {
        let burst = shell_burst(self.hardware.samsung, package, mode);
        if !force
            && package == self.current_package
            && mode == self.current_mode
            && explicit == self.last_explicit
        {
            return Ok(());
        }
        let mut profile = self
            .config
            .profile(if burst { "performance" } else { mode })?
            .clone();
        if self.extreme_power {
            for cluster in 0..4 {
                let policy = self.config.policy[cluster];
                if policy < 0 {
                    continue;
                }
                let base = self.node(format!("/sys/devices/system/cpu/cpufreq/policy{policy}"));
                let data = &mut profile.clusters[cluster];
                if let Some(cap) = conservative_frequency_ceiling(&base, &data.max_freq) {
                    data.max_freq = cap.to_string();
                    let min = normalize_frequency(&base, &data.min_freq, true)
                        .parse::<u64>()
                        .unwrap_or(0);
                    data.min_freq = min.min(cap).to_string();
                }
            }
        }
        let package_changed = package != self.current_package;
        let mode_changed = mode != self.current_mode;
        let boost = launch_boost
            && package_changed
            && !package.is_empty()
            && self.config.functions.launch_boost.enable;

        // 切应用不切档时通常无需写节点；但 SystemUI 突发状态的进出意味着
        // 频率包络变化，必须真实下发一次。
        let previous_burst = shell_burst(self.hardware.samsung, &self.current_package, mode);
        if !force
            && !mode_changed
            && burst == previous_burst
            && !self.current_mode.is_empty()
            && explicit == self.last_explicit
            && !boost
        {
            self.current_package = package.to_string();
            self.last_explicit = explicit;
            return Ok(());
        }

        let expected = self.mode_controls(mode, explicit, &profile)?;
        if preserve_lower_caps {
            for cluster in 0..4 {
                let policy = self.config.policy[cluster];
                if policy < 0 {
                    continue;
                }
                let base = self.node(format!("/sys/devices/system/cpu/cpufreq/policy{policy}"));
                let current = util::read_trimmed(base.join("scaling_max_freq"))?
                    .parse::<u64>()
                    .map_err(|e| e.to_string())?;
                if current == 0 {
                    return Err(format!("policy{policy} 当前频率上限为 0"));
                }
                let data = &mut profile.clusters[cluster];
                let (minimum, maximum) = frequency_range(&base, &data.min_freq, &data.max_freq)?;
                let cap = maximum
                    .parse::<u64>()
                    .map_err(|e| e.to_string())?
                    .min(current);
                data.max_freq = cap.to_string();
                data.min_freq = minimum
                    .parse::<u64>()
                    .map_err(|e| e.to_string())?
                    .min(cap)
                    .to_string();
            }
        }
        if boost {
            self.apply_boost(&profile)?;
            util::sleep(Duration::from_millis(
                self.config.functions.launch_boost.rate_limit_ms,
            ));
        }
        self.pending_enforcement.clear();
        self.enforcement_attempts = 0;
        self.enforcement_after = std::time::Instant::now() + Duration::from_secs(10);
        self.apply_frequency(&profile)?;
        self.apply_gpu_devfreq(mode);
        self.apply_sched_params(&profile)?;
        self.apply_online(&profile)?;
        self.apply_mode_cpuset(mode, explicit)?;
        self.apply_mode_cpuctl(mode, explicit)?;
        self.apply_mode_uclamp()?;
        let drift = mismatched_controls(&expected)?;
        // The per-cluster readbacks and this final sweep are separate reads:
        // a vendor service rewriting a ceiling in between used to fail the
        // whole dispatch and roll back. Ceiling drift is enforcement work,
        // not a failure — queue it; only semantic controls (governor) and
        // non-frequency drift stay hard errors.
        let mut hard = Vec::new();
        for item in drift {
            let ceiling =
                item.path.file_name().and_then(|v| v.to_str()) == Some("scaling_max_freq");
            if ceiling {
                if !self
                    .pending_enforcement
                    .iter()
                    .any(|(path, value)| *path == item.path && *value == item.expected)
                {
                    self.pending_enforcement
                        .push((item.path.clone(), item.expected.clone()));
                }
            } else {
                hard.push(item.describe());
            }
        }
        if !hard.is_empty() {
            return Err(format!("下发后关键节点读回不一致: {}", hard.join("；")));
        }
        self.report_pending();
        self.open_drift = None;
        self.expected_controls = expected;
        self.current_mode = mode.to_string();
        self.current_package = package.to_string();
        self.last_explicit = explicit;
        if package_changed || mode_changed || force {
            if burst {
                self.logger.info(format!(
                    "情景模式: {mode} 已启用（SystemUI 突发：借用性能档频率包络）"
                ));
            } else {
                self.logger.info(format!("情景模式: {mode} 已启用"));
            }
        }
        Ok(())
    }

    /// Refresh independently of CPU writes: FAS can start or restart after boot.
    pub fn refresh_fas(&mut self, allowed: bool) -> crate::fas::Status {
        let mut status = crate::fas::inspect(&self.kernel_root);
        if !allowed || !self.config.functions.fas.enabled {
            status.sync = "paused";
            return status;
        }
        if !matches!(status.state, "ready" | "invalid")
            || !self.node("/dev/fas_rs/mode").is_file()
            || !crate::app_modes::supported(&self.current_mode)
        {
            self.fas_mode = None;
            return status;
        }
        if status.mode != self.current_mode {
            let path = self.node("/dev/fas_rs/mode");
            let result = crate::fas::write_mode(&path, &self.current_mode).and_then(|_| {
                let observed = util::read_trimmed(&path)?;
                if observed == self.current_mode {
                    Ok(())
                } else {
                    Err(format!("请求={} 读回={observed}", self.current_mode))
                }
            });
            if let Err(error) = result {
                status.sync = "failed";
                status.error = error;
                self.fas_mode = None;
                self.logger.debug(format!("FAS 档位同步未确认: {}", status.error));
                return status;
            }
            status.mode = self.current_mode.clone();
            status.state = "ready";
            status.error.clear();
        }
        status.sync = "synced";
        if self.fas_mode.as_deref() != Some(self.current_mode.as_str()) {
            self.logger.info(format!("FAS 模式读回已确认: {}", self.current_mode));
            self.fas_mode = Some(self.current_mode.clone());
        }
        status
    }

    pub fn pending_control_count(&self) -> usize {
        self.pending_enforcement.len()
    }

    pub fn input_boost_node(&self) -> PathBuf {
        self.cpuctl_path("top-app", "min")
    }

    /// 当前档位各簇最低频率（内核适配后的 kHz 字符串）。NovaFAS 以此为
    /// 回落底线，未启用簇返回空串。
    pub fn profile_min_floors(&self) -> Vec<String> {
        match self.config.profile(&self.current_mode) {
            Ok(profile) => profile.clusters.iter().map(|c| c.min_freq.clone()).collect(),
            Err(_) => Vec::new(),
        }
    }

    /// 各簇当前实际频率（kHz），供 WebUI 实时展示；读取失败为 0。
    /// 每个 tick 最多四次小文件读取，代价可忽略。
    pub fn live_frequencies(&self) -> [u32; 4] {
        let mut out = [0u32; 4];
        for (slot, policy) in self.config.policy.iter().enumerate() {
            if *policy < 0 {
                continue;
            }
            out[slot] = util::read_trimmed(
                self.node(format!("/sys/devices/system/cpu/cpufreq/policy{policy}"))
                    .join("scaling_cur_freq"),
            )
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(0);
        }
        out
    }

    fn cpuctl_path(&self, group: &str, field: &str) -> PathBuf {
        let paths = [
            self.node(format!("/dev/cpuctl/{group}/cpu.uclamp.{field}")),
            self.node(format!("/sys/fs/cgroup/{group}/cpu.uclamp.{field}")),
            self.node(format!("/sys/fs/cgroup/cpu/{group}/cpu.uclamp.{field}")),
        ];
        paths.iter().find(|path| path.exists()).unwrap_or(&paths[0]).clone()
    }

    /// The daemon calls this on a separate 60-second deadline. A healthy
    /// inspection is read-only; repair stays inside the usual transaction.
    pub fn audit_and_repair(&mut self) -> Result<bool> {
        if !self.config.functions.node_watchdog || self.current_mode.is_empty() {
            return Ok(false);
        }
        let drift = mismatched_controls(&self.expected_controls)?;
        if drift.is_empty() {
            self.open_drift = None;
            return Ok(false);
        }
        // Ceiling drift is queued enforcement work handled silently by
        // enforce_pending every tick; only drift the queue cannot cover
        // (governor, cpuset, uclamp) warrants a repair pass here.
        let described: Vec<String> = drift.iter().map(|item| item.describe()).collect();
        let pending_only = drift.iter().all(|item| {
            item.path.file_name().and_then(|v| v.to_str()) == Some("scaling_max_freq")
                && self
                    .pending_enforcement
                    .iter()
                    .any(|(path, value)| *path == item.path && *value == item.expected)
        });
        if pending_only {
            self.open_drift = Some(described);
            return Ok(false);
        }
        if self.open_drift.as_ref() == Some(&described) {
            // The same divergence already failed a repair; keep re-asserting
            // quietly instead of spamming the log every audit cycle.
            return Ok(false);
        }
        self.logger
            .warn(format!("检测到节点漂移: {}", described.join("；")));
        let package = self.current_package.clone();
        let mode = self.current_mode.clone();
        let snapshot = self.snapshot.clone();
        snapshot.apply_transaction(|| {
            self.apply_inner(&package, &mode, false, true, self.last_explicit, true)
        })?;
        let remaining = mismatched_controls(&self.expected_controls)?;
        self.open_drift = if remaining.is_empty() {
            self.logger.info("检测到漂移已纠正（关键节点读回一致）");
            None
        } else {
            self.logger.warn(format!(
                "部分节点仍未生效，保留诊断状态: {}",
                remaining
                    .iter()
                    .map(|item| item.describe())
                    .collect::<Vec<_>>()
                    .join("；")
            ));
            Some(remaining.iter().map(|item| item.describe()).collect())
        };
        Ok(self.open_drift.is_none())
    }

    pub fn pending_retry_exhausted(&self) -> bool {
        self.hardware.samsung || self.enforcement_attempts >= 3
    }

    /// At most three retries, ten seconds apart. Samsung keeps the first
    /// readback and yields immediately instead of competing with its HAL.
    pub fn enforce_pending(&mut self) -> bool {
        if self.pending_enforcement.is_empty() || self.pending_retry_exhausted()
            || std::time::Instant::now() < self.enforcement_after {
            return false;
        }
        self.enforcement_attempts += 1;
        self.enforcement_after = std::time::Instant::now() + Duration::from_secs(10);
        let mut still = Vec::new();
        let mut resolved = false;
        for (path, value) in std::mem::take(&mut self.pending_enforcement) {
            let outcome = self.snapshot.write(&path, &value);
            let observed = util::read_trimmed(&path).unwrap_or_default();
            if frequency_ceiling_matches(&observed, &value) {
                resolved = true;
            } else {
                if let Err(error) = outcome {
                    self.logger
                        .debug(format!("重申上限暂未写入: {} ({error})", path.display()));
                }
                still.push((path, value));
            }
        }
        self.pending_enforcement = still;
        if resolved {
            self.report_pending();
        }
        if self.pending_retry_exhausted() && !self.pending_enforcement.is_empty() {
            self.logger.warn("系统未接受频率上限，三次重试已用完；保留降级状态，停止自动重写");
        }
        true
    }

    /// Drops the enforcement queue; used while a foreign scheduler owns the
    /// nodes so background re-writes never fight it.
    pub fn clear_pending(&mut self) {
        self.pending_enforcement.clear();
    }

    /// One info line per distinct set of vendor-held ceilings, plus a
    /// positive line when the kernel finally accepts the values.
    fn report_pending(&mut self) {
        if self.pending_enforcement.is_empty() {
            if !self.pending_note.is_empty() {
                self.pending_note.clear();
                self.logger
                    .info("此前被占用的频率上限已彻底写入，配置完整生效");
            }
            return;
        }
        let note = self
            .pending_enforcement
            .iter()
            .map(|(path, value)| {
                format!(
                    "{} 保持目标 {value}",
                    path.parent()
                        .and_then(|p| p.file_name())
                        .and_then(|p| p.to_str())
                        .unwrap_or("policy")
                )
            })
            .collect::<Vec<_>>()
            .join("；");
        if self.pending_note != note {
            self.pending_note = note.clone();
            self.logger.info(format!(
                "部分频率上限被系统服务占用，最多重试三次（三星不重试）: {note}"
            ));
        }
    }

    fn node(&self, path: impl AsRef<Path>) -> PathBuf {
        self.kernel_root
            .join(path.as_ref().strip_prefix("/").unwrap_or(path.as_ref()))
    }

    fn cpuset_path(&self, group: &str) -> PathBuf {
        let v1 = self.node(format!("/dev/cpuset/{group}/cpus"));
        let v2 = self.node(format!("/sys/fs/cgroup/{group}/cpuset.cpus"));
        if v1.exists() || !v2.exists() {
            v1
        } else {
            v2
        }
    }

    fn mode_controls(
        &self,
        mode: &str,
        explicit: bool,
        profile: &ModeProfile,
    ) -> Result<Vec<ExpectedControl>> {
        let mut controls = Vec::new();
        let mut add = |path: PathBuf, value: String, comparison: ControlKind, required: bool| {
            // A node present on a previous successful apply must not silently
            // disappear from health checks when a driver removes it.
            if required || path.exists() || self.expected_controls.iter().any(|c| c.path == path) {
                controls.push(ExpectedControl {
                    path,
                    value,
                    comparison,
                });
            }
        };
        for cluster in 0..4 {
            let policy = self.config.policy[cluster];
            if policy < 0 {
                continue;
            }
            let base = self.node(format!("/sys/devices/system/cpu/cpufreq/policy{policy}"));
            let data = &profile.clusters[cluster];
            add(
                base.join("scaling_max_freq"),
                normalize_frequency(&base, &data.max_freq, false),
                ControlKind::FrequencyCeiling,
                true,
            );
            if !self.hardware.samsung {
                add(
                    base.join("scaling_governor"),
                    data.governor.clone(),
                    ControlKind::Text,
                    true,
                );
            }
        }
        let cpuset = &self.config.functions.cpuset;
        // 功能停用时相应节点不进审计：模块没有接管它们，Scene 的 fas-rs 或
        // 厂商 powerhal 改写全局 uclamp / cpuset 属于正常外部行为。把原厂值
        // 留在期望列表里会造成“检测到节点漂移”→修复→再漂移的拉锯告警。
        // 必须镜像 apply_mode_cpuset 的优先级：应用规则（仅显式规则应用在前台
        // 时），然后档位覆盖，最后静态值。
        let rule = if explicit && cpuset.enable {
            cpuset.app_rule.as_ref()
        } else {
            None
        };
        for (group, normal, mode_override) in [
            (
                "top-app",
                &cpuset.top_app,
                cpuset.modes.get(mode).and_then(|o| o.top_app.as_ref()),
            ),
            (
                "foreground",
                &cpuset.foreground,
                cpuset.modes.get(mode).and_then(|o| o.foreground.as_ref()),
            ),
        ] {
            let path = self.cpuset_path(group);
            let value = if cpuset.enable {
                let rule_value = rule.and_then(|r| match group {
                    "top-app" => r.top_app.as_ref(),
                    _ => r.foreground.as_ref(),
                });
                Some(rule_value.or(mode_override).unwrap_or(normal).clone())
            } else {
                None
            };
            if let Some(value) = value {
                add(path, value, ControlKind::CpuList, false);
            }
        }
        let sched = &self.config.functions.scheduler;
        if sched.enable {
            for (name, normal) in [
                ("sched_util_clamp_min", &sched.util_clamp_min),
                ("sched_util_clamp_max", &sched.util_clamp_max),
            ] {
                add(
                    self.node(format!("/proc/sys/kernel/{name}")),
                    normal.clone(),
                    ControlKind::Number,
                    false,
                );
            }
        }
        Ok(controls)
    }

    fn capture_initial_nodes(&self) -> Result<()> {
        let mut paths = BTreeSet::new();
        for profile in self.config.modes.values() {
            for (cluster, data) in profile.clusters.iter().enumerate() {
                let policy = self.config.policy[cluster];
                if policy < 0 {
                    continue;
                }
                let base = self.node(format!("/sys/devices/system/cpu/cpufreq/policy{policy}"));
                for name in ["scaling_min_freq", "scaling_max_freq", "scaling_governor"] {
                    if self.hardware.samsung && name == "scaling_governor" { continue; }
                    paths.insert(base.join(name));
                }
                for (name, _) in &data.sched_params {
                    paths.insert(base.join(&data.governor).join(name));
                }
            }
        }
        for cpu in self.hardware.cpus() {
            if self
                .config
                .modes
                .values()
                .any(|profile| !profile.cpu_online(cpu))
            {
                paths.insert(self.node(format!("/sys/devices/system/cpu/cpu{cpu}/online")));
            }
        }
        if self.config.functions.cpuset.enable {
            for group in [
                "top-app",
                "foreground",
                "background",
                "system-background",
                "restricted",
            ] {
                paths.insert(self.cpuset_path(group));
            }
        }
        if self.config.functions.cpuctl.enable || self.config.functions.input_boost.enabled {
            for group in ["top-app", "background"] {
                for field in ["cpu.uclamp.min", "cpu.uclamp.max"] {
                    paths.insert(self.cpuctl_path(group, field.strip_prefix("cpu.uclamp.").unwrap_or(field)));
                }
            }
        }
        if self.config.functions.scheduler.enable {
            for name in [
                "sched_schedstats",
                "sched_latency_ns",
                "sched_migration_cost_ns",
                "sched_min_granularity_ns",
                "sched_wakeup_granularity_ns",
                "sched_nr_migrate",
                "sched_util_clamp_min",
                "sched_util_clamp_max",
                "sched_energy_aware",
            ] {
                paths.insert(self.node(format!("/proc/sys/kernel/{name}")));
            }
        }
        let gpu = self.node("/sys/class/kgsl/kgsl-3d0");
        if self.config.functions.disable_gpu_boost {
            paths.insert(gpu.join("max_pwrlevel"));
        }
        if self.config.functions.gpu_devfreq.enabled {
            if let Some(base) = &self.gpu_devfreq_base {
                paths.insert(base.join("min_freq"));
                paths.insert(base.join("max_freq"));
            }
        }
        if self.config.functions.disable_gpu_boost {
            for name in [
                "default_pwrlevel",
                "min_pwrlevel",
                "force_bus_on",
                "force_clk_on",
                "force_no_nap",
                "force_rail_on",
                "bus_split",
            ] {
                paths.insert(gpu.join(name));
            }
        }
        let services = if self.config.functions.perf_service_lock.enable {
            self.config.functions.perf_service_lock.services.as_slice()
        } else {
            &[]
        };
        self.snapshot.capture_batch(paths, services)
    }

    fn apply_frequency(&mut self, profile: &ModeProfile) -> Result<()> {
        for cluster in 0..4 {
            let policy = self.config.policy[cluster];
            if policy < 0 {
                continue;
            }
            let data = &profile.clusters[cluster];
            self.write_frequency(
                policy,
                cluster,
                &data.min_freq,
                &data.max_freq,
                &data.governor,
            )?;
        }
        Ok(())
    }

    fn apply_mode_cpuset(&mut self, mode: &str, explicit: bool) -> Result<()> {
        let cpuset = &self.config.functions.cpuset;
        if !cpuset.enable {
            // Stock was never captured for a disabled feature and the enable
            // flag cannot change at runtime; nothing to write or audit.
            return Ok(());
        }
        // Precedence: the per-app game placement (only while an explicitly
        // ruled app is foreground), then the mode override, then the static values.
        let rule = if explicit {
            cpuset.app_rule.as_ref()
        } else {
            None
        };
        let top_app = rule
            .and_then(|r| r.top_app.as_deref())
            .or_else(|| cpuset.modes.get(mode).and_then(|o| o.top_app.as_deref()))
            .unwrap_or(&cpuset.top_app);
        let foreground = rule
            .and_then(|r| r.foreground.as_deref())
            .or_else(|| cpuset.modes.get(mode).and_then(|o| o.foreground.as_deref()))
            .unwrap_or(&cpuset.foreground);
        for (group, value) in [("top-app", top_app), ("foreground", foreground)] {
            write_mode_control(
                &self.snapshot,
                &self.cpuset_path(group),
                value,
                ControlKind::CpuList,
            )?;
        }
        Ok(())
    }

    /// Best-effort cgroup-v2 uclamp clamps for the mode (cpuctl). Nodes
    /// missing on kernels without CONFIG_UCLAMP_TASK_GROUP are skipped;
    /// failures degrade to warnings because foreground scheduling keeps
    /// working without them. Groups without a mode entry return to stock.
    /// The rule-gated top-app floor is transient: it applies only while an
    /// explicitly ruled app is foreground, and the live baseline is shared
    /// with the input-boost thread for pulse expiry.
    fn apply_mode_cpuctl(&mut self, mode: &str, explicit: bool) -> Result<()> {
        let live = self.cpuctl_live.clone();
        let mut state = live.inner.lock().map_err(|_| "输入加速状态锁损坏".to_string())?;
        let cpuctl = &self.config.functions.cpuctl;
        let entry = cpuctl.modes.get(mode);
        let mut top_app_baseline = self
            .snapshot
            .original_value(&self.input_boost_node())
            .ok()
            .flatten()
            .unwrap_or_else(|| "0".into());
        if cpuctl.enable {
            for (group, knob, field) in [
                ("top-app", "top_app_min", "min"),
                ("background", "background_max", "max"),
            ] {
                let requested = entry.and_then(|entry| match knob {
                    "top_app_min" => {
                        if explicit {
                            entry
                                .top_app_min_rule
                                .as_deref()
                                .or(entry.top_app_min.as_deref())
                        } else {
                            entry.top_app_min.as_deref()
                        }
                    }
                    _ => entry.background_max.as_deref(),
                });
                let path = self.cpuctl_path(group, field);
                let stock = if requested.is_none() {
                    self.snapshot.original_value(&path).ok().flatten()
                } else {
                    None
                };
                let Some(value) = requested.or(stock.as_deref()) else {
                    continue;
                };
                if group == "top-app" {
                    top_app_baseline = value.to_string();
                }
                if !path.exists() {
                    continue;
                }
                if util::read_trimmed(&path).ok().as_deref() == Some(value.trim()) {
                    continue;
                }
                match self.snapshot.write(&path, value.trim()) {
                    Ok(()) => self
                        .logger
                        .debug(format!("cpuctl 钳制已写入: {} = {value}", path.display())),
                    Err(error) => self.logger.warn(format!(
                        "cpuctl 钳制写入失败，保持当前值: {} ({error})",
                        path.display()
                    )),
                }
            }
        }
        state.top_app_min = top_app_baseline;
        let boost = &self.config.functions.input_boost;
        let tuned = boost.modes.get(mode);
        state.boost_min = if boost.enabled {
            tuned.map(|value| value.top_app_min.clone())
                .unwrap_or_else(|| boost.top_app_min.clone())
        } else {
            String::new()
        };
        state.duration_ms = tuned.map(|value| value.duration_ms).unwrap_or(boost.duration_ms);
        Ok(())
    }

    fn apply_boost(&mut self, profile: &ModeProfile) -> Result<()> {
        for cluster in 0..4 {
            let policy = self.config.policy[cluster];
            if policy < 0 {
                continue;
            }
            let boost_ceiling = self.config.functions.launch_boost.frequencies[cluster].clone();
            self.write_frequency(
                policy,
                cluster,
                &profile.clusters[cluster].min_freq,
                &boost_ceiling,
                &profile.clusters[cluster].governor,
            )?;
        }
        Ok(())
    }

    fn apply_mode_uclamp(&self) -> Result<()> {
        if !self.config.functions.scheduler.enable {
            // 功能停用时不再回写原厂全局 uclamp：模块没有接管这些节点，
            // 每次切档都把 fas-rs / powerhal 的设置改回去只会互相拉锯。
            // 原厂值由模块停止时的快照回滚负责恢复。
            return Ok(());
        }
        let minimum = &self.node("/proc/sys/kernel/sched_util_clamp_min");
        let maximum = &self.node("/proc/sys/kernel/sched_util_clamp_max");
        let min = &self.config.functions.scheduler.util_clamp_min;
        let max = &self.config.functions.scheduler.util_clamp_max;
        write_uclamp_range(&self.snapshot, minimum, maximum, min, max)
    }

    fn write_frequency(
        &mut self,
        policy: i32,
        cluster: usize,
        min: &str,
        max: &str,
        governor: &str,
    ) -> Result<()> {
        let base = self.node(format!("/sys/devices/system/cpu/cpufreq/policy{policy}"));
        let (effective_min, effective_max) = frequency_range(&base, min, max)?;
        let current_max = util::read_trimmed(base.join("scaling_max_freq"))?;
        if range_max_first(&effective_min, &current_max) {
            self.required_write(base.join("scaling_max_freq"), &effective_max)?;
            self.required_write(base.join("scaling_min_freq"), &effective_min)?;
        } else {
            self.required_write(base.join("scaling_min_freq"), &effective_min)?;
            self.required_write(base.join("scaling_max_freq"), &effective_max)?;
        }
        // An OEM may already use the requested governor. Avoid an unnecessary
        // driver callback without weakening the snapshot for changed values.
        if !self.hardware.samsung && util::read_trimmed(base.join("scaling_governor"))? != governor {
            self.required_write(base.join("scaling_governor"), governor)?;
        }
        // One readback per dispatch. A lower vendor limit is accepted; a
        // higher limit remains visible with a bounded retry budget.
        let ceiling = base.join("scaling_max_freq");
        let observed_max = util::read_trimmed(&ceiling)?;
        if !frequency_ceiling_matches(&observed_max, &effective_max) {
            let target = effective_max.clone();
            if !self
                .pending_enforcement
                .iter()
                .any(|(path, value)| *path == ceiling && *value == target)
            {
                self.pending_enforcement.push((ceiling.clone(), target));
            }
        }
        let observed_governor = util::read_trimmed(base.join("scaling_governor"))?;
        if !self.hardware.samsung && observed_governor != governor {
            return Err(format!(
                "CPU policy{policy} 调速器未按请求生效: 请求={governor} 读回={observed_governor}"
            ));
        }
        self.logger.debug(format!("CPU簇: {policy} (c{cluster}) 最小频率: {effective_min} 最大频率: {effective_max} 调速器: {observed_governor}"));
        Ok(())
    }

    /// 按档位写 GPU devfreq min/max_freq。GPU 是可选功能：任何一步失败只
    /// 记日志，绝不让 GPU 包络拖累 CPU 档位下发。0% = 频表物理端点（不限制）。
    fn apply_gpu_devfreq(&mut self, mode: &str) {
        if !self.config.functions.gpu_devfreq.enabled {
            return;
        }
        let Some(base) = self.gpu_devfreq_base.clone() else {
            return;
        };
        let Some(envelope) = self.config.functions.gpu_devfreq.modes.get(mode).cloned() else {
            return;
        };
        let table = gpu_frequency_table(&base);
        let (Some(bottom), Some(top)) = (table.first().copied(), table.last().copied()) else {
            self.logger.debug(format!(
                "GPU 频表不可读，本次跳过 GPU 包络下发: {}",
                base.display()
            ));
            return;
        };
        let snap_down = |pct: u32| -> u64 {
            let target = top.saturating_mul(pct as u64) / 100;
            table.iter().copied().rev().find(|v| *v <= target).unwrap_or(bottom)
        };
        let snap_up = |pct: u32| -> u64 {
            let target = top.saturating_mul(pct as u64) / 100;
            table.iter().copied().find(|v| *v >= target).unwrap_or(top)
        };
        let target_max = if envelope.max == 0 { top } else { snap_down(envelope.max) };
        let target_min = if envelope.min == 0 { bottom } else { snap_up(envelope.min) };
        let target_min = target_min.min(target_max);
        // 维持 min <= max 不变式：抬高下限超过当前上限时先写上限。
        let current_max = util::read_trimmed(base.join("max_freq"))
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(top);
        let ordered = if target_min > current_max {
            [("max_freq", target_max), ("min_freq", target_min)]
        } else {
            [("min_freq", target_min), ("max_freq", target_max)]
        };
        for (name, value) in ordered {
            let path = base.join(name);
            let text = value.to_string();
            if util::read_trimmed(&path).ok().as_deref() == Some(text.as_str()) {
                continue;
            }
            match self.snapshot.write(&path, &text) {
                Ok(()) => {
                    let observed = util::read_trimmed(&path).unwrap_or_default();
                    if observed != text {
                        self.logger.debug(format!(
                            "GPU {name} 读回 {observed}（内核已按驱动边界钳制 {text}）"
                        ));
                    }
                }
                Err(error) => self.logger.warn(format!(
                    "GPU {name} 写入失败，保持当前值: {error}"
                )),
            }
        }
    }

    fn apply_sched_params(&self, profile: &ModeProfile) -> Result<()> {
        for cluster in 0..4 {
            let policy = self.config.policy[cluster];
            if policy < 0 {
                continue;
            }
            let data = &profile.clusters[cluster];
            let policy_base = self.node(format!("/sys/devices/system/cpu/cpufreq/policy{policy}"));
            let base = policy_base.join(&data.governor);
            // Governor knobs accept raw kHz, but percent values resolve against
            // this policy's own maximum and snap to its frequency table — the
            // same contract as min/max, so configs stay device-independent.
            let resolved: Vec<(String, String)> = data
                .sched_params
                .iter()
                .map(|(key, value)| (key.clone(), resolve_param_frequency(value, &policy_base)))
                .collect();
            apply_governor_params(&self.snapshot, &self.logger, &base, &resolved)?;
        }
        Ok(())
    }

    fn apply_online(&self, profile: &ModeProfile) -> Result<()> {
        for cpu in self.hardware.cpus() {
            if self.config.modes.values().all(|mode| mode.cpu_online(cpu)) {
                continue;
            }
            let path = self.node(format!("/sys/devices/system/cpu/cpu{cpu}/online"));
            if path.exists() {
                let value = if profile.cpu_online(cpu) { "1" } else { "0" };
                if util::read_trimmed(&path)? != value {
                    // Some SM8650 kernels do not permit userspace hotplug.
                    // Log that optional capability; frequency/cpuset still apply.
                    self.optional_write_tolerating_unsupported(path, value);
                }
            }
        }
        Ok(())
    }

    fn apply_functions(&self) -> Result<()> {
        let functions = &self.config.functions;
        if functions.perf_service_lock.enable {
            for service in &functions.perf_service_lock.services {
                let status = util::getprop(&format!("init.svc.{service}"));
                if !matches!(status.as_str(), "running" | "restarting") {
                    self.logger
                        .debug(format!("perf 服务未运行，跳过: {service}"));
                    continue;
                }
                self.snapshot.capture_service(service)?;
                util::run_command("stop", &[service])?;
                self.logger
                    .info(format!("已停用 perf 服务（退出时恢复）: {service}"));
            }
        }
        if functions.cpuset.enable {
            let mut summary = OptionalWriteSummary::default();
            for (name, value) in [
                ("top-app", functions.cpuset.top_app.as_str()),
                ("foreground", functions.cpuset.foreground.as_str()),
                ("background", functions.cpuset.background.as_str()),
                (
                    "system-background",
                    functions.cpuset.system_background.as_str(),
                ),
                ("restricted", functions.cpuset.restricted.as_str()),
            ] {
                summary.record(self.optional_write(self.cpuset_path(name), value));
            }
            self.log_optional_summary("CpuSet 调整", summary);
        }
        if functions.disable_gpu_boost {
            let base = self.node("/sys/class/kgsl/kgsl-3d0");
            if base.exists() {
                let mut summary = OptionalWriteSummary::default();
                if let Ok(levels) = util::read_trimmed(base.join("num_pwrlevels"))
                    .and_then(|v| v.parse::<i32>().map_err(|e| e.to_string()))
                {
                    if levels > 0 {
                        summary.record(self.optional_write(
                            base.join("default_pwrlevel"),
                            &(levels - 1).to_string(),
                        ));
                        summary.record(
                            self.optional_write(
                                base.join("min_pwrlevel"),
                                &(levels - 1).to_string(),
                            ),
                        );
                    }
                }
                for (name, value) in [
                    ("max_pwrlevel", "0"),
                    ("force_bus_on", "0"),
                    ("force_clk_on", "0"),
                    ("force_no_nap", "0"),
                    ("force_rail_on", "0"),
                    ("bus_split", "1"),
                ] {
                    let outcome = if is_kgsl_optional_force_node(name) {
                        self.optional_write_tolerating_unsupported(base.join(name), value)
                    } else {
                        self.optional_write(base.join(name), value)
                    };
                    summary.record(outcome);
                }
                self.log_optional_summary("高通 GPU", summary);
            } else {
                self.logger
                    .debug("未检测到 KGSL GPU 节点，跳过 GPU 可选优化");
            }
        }
        if functions.scheduler.enable {
            let sched = &functions.scheduler;
            let mut summary = OptionalWriteSummary::default();
            for (name, value) in [
                ("sched_schedstats", if sched.schedstats { "1" } else { "0" }),
                ("sched_latency_ns", sched.latency_ns.as_str()),
                ("sched_migration_cost_ns", sched.migration_cost_ns.as_str()),
                (
                    "sched_min_granularity_ns",
                    sched.min_granularity_ns.as_str(),
                ),
                (
                    "sched_wakeup_granularity_ns",
                    sched.wakeup_granularity_ns.as_str(),
                ),
                ("sched_nr_migrate", sched.nr_migrate.as_str()),
                ("sched_util_clamp_min", sched.util_clamp_min.as_str()),
                ("sched_util_clamp_max", sched.util_clamp_max.as_str()),
            ] {
                summary.record(
                    self.optional_write(self.node(format!("/proc/sys/kernel/{name}")), value),
                );
            }
            summary.record(self.optional_write(
                self.node("/proc/sys/kernel/sched_energy_aware"),
                if sched.energy_aware { "1" } else { "0" },
            ));
            self.log_optional_summary("CFS 调度器", summary);
        }
        Ok(())
    }

    fn required_write(&self, path: PathBuf, value: &str) -> Result<()> {
        if !path.exists() {
            return Err(format!("必要节点不存在: {}", path.display()));
        }
        self.snapshot.write(&path, value)
    }

    fn log_optional_summary(&self, label: &str, summary: OptionalWriteSummary) {
        if summary.failed > 0 {
            self.logger.warn(format!(
                "{label}：{} 项已写入，{} 项失败，{} 项不支持，{} 项不存在",
                summary.applied, summary.failed, summary.unsupported, summary.missing
            ));
        } else if summary.unsupported > 0 {
            self.logger.info(format!(
                "{label}：{} 项已写入，{} 个可选强制节点不受当前内核支持，已跳过",
                summary.applied, summary.unsupported
            ));
        } else if summary.missing > 0 {
            self.logger.info(format!(
                "{label}：{} 项已写入，{} 个可选节点不存在，已跳过",
                summary.applied, summary.missing
            ));
        } else {
            self.logger.info(format!("{label} 已完成"));
        }
    }

    fn optional_write(&self, path: PathBuf, value: &str) -> OptionalWriteOutcome {
        self.optional_write_inner(path, value, false)
    }

    fn optional_write_tolerating_unsupported(
        &self,
        path: PathBuf,
        value: &str,
    ) -> OptionalWriteOutcome {
        self.optional_write_inner(path, value, true)
    }

    fn optional_write_inner(
        &self,
        path: PathBuf,
        value: &str,
        tolerate_unsupported: bool,
    ) -> OptionalWriteOutcome {
        if !path.exists() {
            self.logger
                .debug(format!("可选节点不存在，跳过: {}", path.display()));
            return OptionalWriteOutcome::Missing;
        }
        match self.snapshot.write_unlogged(&path, value) {
            Ok(()) => OptionalWriteOutcome::Applied,
            Err(error) if tolerate_unsupported && optional_node_not_supported(&error) => {
                self.logger.debug(format!(
                    "内核不支持可选节点，跳过: {} ({error})",
                    path.display()
                ));
                OptionalWriteOutcome::Unsupported
            }
            Err(error) => {
                self.logger.error(format!("sysfs/proc 写入失败: {error}"));
                OptionalWriteOutcome::Failed
            }
        }
    }
}

fn uclamp_max_first(target_min: u64, current_max: u64) -> bool {
    target_min > current_max
}

fn write_uclamp_range(
    snapshot: &Snapshot,
    minimum: &Path,
    maximum: &Path,
    min: &str,
    max: &str,
) -> Result<()> {
    let requested_min = min.parse::<u64>().map_err(|e| e.to_string())?;
    let requested_max = max.parse::<u64>().map_err(|e| e.to_string())?;
    if requested_min > requested_max || requested_max > 1024 {
        return Err("uclamp 请求范围必须满足 0 <= min <= max <= 1024".into());
    }
    let max_first = if maximum.exists() {
        uclamp_max_first(
            requested_min,
            util::read_trimmed(maximum)?
                .parse::<u64>()
                .map_err(|e| e.to_string())?,
        )
    } else {
        false
    };
    let ordered = if max_first {
        [(maximum, max), (minimum, min)]
    } else {
        [(minimum, min), (maximum, max)]
    };
    for (path, value) in ordered {
        write_mode_control(snapshot, path, value, ControlKind::Number)?;
    }
    Ok(())
}

fn range_max_first(min: &str, current_max: &str) -> bool {
    matches!((min.parse::<u64>(),current_max.parse::<u64>()),(Ok(a),Ok(b)) if a>b)
}

fn frequency_ceiling_matches(actual: &str, requested: &str) -> bool {
    actual
        .trim()
        .parse::<u64>()
        .ok()
        .zip(requested.trim().parse::<u64>().ok())
        .is_some_and(|(actual, requested)| actual > 0 && actual <= requested)
}

fn frequency_range(base: &Path, min: &str, max: &str) -> Result<(String, String)> {
    let minimum = normalize_frequency(base, min, true)
        .parse::<u64>()
        .map_err(|e| format!("最低频率无效: {e}"))?;
    let maximum = normalize_frequency(base, max, false)
        .parse::<u64>()
        .map_err(|e| format!("最高频率无效: {e}"))?;
    if maximum == 0 {
        return Err("有效频率上限必须大于 0".into());
    }

    // A narrow requested range can round up and down to opposite OPPs.
    Ok((minimum.min(maximum).to_string(), maximum.to_string()))
}

fn conservative_frequency_ceiling(base: &Path, requested: &str) -> Option<u64> {
    let raw = util::read_trimmed(base.join("scaling_available_frequencies"))
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| {
            util::read_trimmed(base.join("stats/time_in_state"))
                .ok()
                .map(|value| {
                    value
                        .lines()
                        .filter_map(|line| line.split_whitespace().next())
                        .collect::<Vec<_>>()
                        .join(" ")
                })
        })?;
    let mut frequencies: Vec<u64> = raw
        .split_whitespace()
        .filter_map(|value| value.parse().ok())
        .filter(|value| *value > 0)
        .collect();
    frequencies.sort_unstable();
    frequencies.dedup();
    if frequencies.len() < 2 {
        return None;
    }
    let ceiling = normalize_frequency(base, requested, false)
        .parse::<u64>()
        .ok()?;
    let index = frequencies
        .iter()
        .rposition(|frequency| *frequency <= ceiling)?;
    (index > 0).then(|| frequencies[index - 1])
}

#[derive(Clone, Copy)]
enum ControlKind {
    Number,
    FrequencyCeiling,
    CpuList,
    Text,
}

#[derive(Clone)]
struct ExpectedControl {
    path: PathBuf,
    value: String,
    comparison: ControlKind,
}

/// A control whose readback does not match the requested value. Ceilings are
/// queued for background enforcement instead of failing the dispatch.
struct Drift {
    path: PathBuf,
    expected: String,
    actual: String,
}

impl Drift {
    fn describe(&self) -> String {
        format!(
            "{} 期望={} 实际={}",
            self.path.display(),
            self.expected.trim(),
            self.actual
        )
    }
}

fn mismatched_controls(controls: &[ExpectedControl]) -> Result<Vec<Drift>> {
    let mut drift = Vec::new();
    for control in controls {
        let actual =
            util::read_trimmed(&control.path).map_err(|e| format!("节点巡检读取失败: {e}"))?;
        let matches = match control.comparison {
            ControlKind::FrequencyCeiling => frequency_ceiling_matches(&actual, &control.value),
            ControlKind::Number => actual
                .parse::<u64>()
                .ok()
                .zip(control.value.trim().parse::<u64>().ok())
                .is_some_and(|(a, b)| a == b),
            ControlKind::CpuList => hardware::parse_cpu_list(&actual)
                .ok()
                .zip(hardware::parse_cpu_list(&control.value).ok())
                .is_some_and(|(a, b)| a == b),
            ControlKind::Text => actual == control.value.trim(),
        };
        if !matches {
            drift.push(Drift {
                path: control.path.clone(),
                expected: control.value.clone(),
                actual,
            });
        }
    }
    Ok(drift)
}

fn write_mode_control(
    snapshot: &Snapshot,
    path: &Path,
    value: &str,
    kind: ControlKind,
) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }
    let matches = |actual: &str| -> Result<bool> {
        match kind {
            ControlKind::FrequencyCeiling => Ok(frequency_ceiling_matches(actual, value)),
            ControlKind::Number => {
                let requested = value.trim().parse::<u64>().map_err(|e| e.to_string())?;
                let observed = actual.trim().parse::<u64>().map_err(|e| e.to_string())?;
                Ok(requested == observed)
            }
            ControlKind::CpuList => {
                Ok(hardware::parse_cpu_list(actual)? == hardware::parse_cpu_list(value)?)
            }
            ControlKind::Text => Ok(actual.trim() == value.trim()),
        }
    };
    if matches(&util::read_trimmed(path)?)? {
        return Ok(());
    }
    snapshot.write(path, value)?;
    let observed = util::read_trimmed(path)?;
    if !matches(&observed)? {
        return Err(format!(
            "档位控制节点未按请求生效: {} 请求={value} 读回={observed}",
            path.display()
        ));
    }
    Ok(())
}

fn apply_governor_params(
    snapshot: &Snapshot,
    logger: &Logger,
    base: &Path,
    params: &[(String, String)],
) -> Result<()> {
    // Empty Path fields mean this profile does not own the knob. Restore only
    // knobs previously captured by NovaSched, using their original values.
    // A driver rejecting one knob must not fail the whole mode switch: the
    // remaining scheduling keeps working and the failure stays visible in
    // the log instead.
    for (path, stock) in snapshot.original_children(base)? {
        let owned = path
            .file_name()
            .and_then(|v| v.to_str())
            .is_some_and(|name| params.iter().any(|(key, _)| key == name));
        if !owned && path.exists() && util::read_trimmed(&path)? != stock.trim() {
            match snapshot.write(&path, stock.trim()) {
                Ok(()) => logger.info(format!("清理上一档残留参数: {}", path.display())),
                Err(error) => logger.warn(format!(
                    "恢复上一档参数失败，保持当前值: {} ({error})",
                    path.display()
                )),
            }
        }
    }
    for (name, value) in params {
        let path = base.join(name);
        if !path.exists() {
            continue;
        }
        if util::read_trimmed(&path)? != value.trim() {
            if let Err(error) = snapshot.write(path, value) {
                logger.warn(format!(
                    "调速器参数写入失败，保持当前值: {} ({error})",
                    base.join(name).display()
                ));
            }
        }
    }
    Ok(())
}

fn is_kgsl_optional_force_node(name: &str) -> bool {
    matches!(
        name,
        "force_bus_on" | "force_clk_on" | "force_no_nap" | "force_rail_on"
    )
}

/// 读取 GPU devfreq 可用频表（升序、去重、去零）。
fn gpu_frequency_table(base: &Path) -> Vec<u64> {
    let mut table: Vec<u64> = util::read_trimmed(base.join("available_frequencies"))
        .unwrap_or_default()
        .split_whitespace()
        .filter_map(|v| v.parse().ok())
        .filter(|v| *v > 0)
        .collect();
    table.sort_unstable();
    table.dedup();
    table
}

/// 能力检测：扫描 /sys/class/devfreq，找到第一个名称指向 GPU 且
/// min/max/频表均可读的设备。总线监控（kgsl-busmon）、存储（ufshci）与
/// MTK 内存降压（dvfsrc）等 devfreq 设备不算 GPU 调频接口。
pub(crate) fn detect_gpu_devfreq(root: &Path) -> Option<PathBuf> {
    let entries = std::fs::read_dir(root.join("sys/class/devfreq")).ok()?;
    let mut candidates: Vec<PathBuf> = entries.flatten().map(|entry| entry.path()).collect();
    candidates.sort();
    for path in candidates {
        let Some(name) = path.file_name().and_then(|v| v.to_str()) else {
            continue;
        };
        let lower = name.to_ascii_lowercase();
        let looks_gpu = lower.contains("kgsl-3d0")
            || lower.contains("mali")
            || lower.starts_with("gpu");
        if !looks_gpu {
            continue;
        }
        let readable = ["min_freq", "max_freq", "available_frequencies"]
            .iter()
            .all(|node| util::read_trimmed(path.join(node)).is_ok());
        let table = gpu_frequency_table(&path);
        if readable && table.len() >= 2 {
            return Some(path);
        }
    }
    None
}

/// Resolve a governor parameter value: raw strings pass through untouched,
/// "N%" resolves against the policy's physical maximum and snaps down to the
/// nearest available OPP. Unreadable nodes keep the raw value; the param
/// write itself is best-effort anyway.
pub(crate) fn resolve_param_frequency(value: &str, policy_base: &Path) -> String {
    let Some(percent) = value.strip_suffix('%') else {
        return value.to_string();
    };
    let Ok(n) = percent.parse::<u64>() else {
        return value.to_string();
    };
    if n > 100 {
        return value.to_string();
    }
    let max = match util::read_trimmed(policy_base.join("cpuinfo_max_freq"))
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
    {
        Some(max) if max > 0 => max,
        _ => return value.to_string(),
    };
    let target = max.saturating_mul(n) / 100;
    normalize_frequency(policy_base, &target.to_string(), false)
}

fn optional_node_not_supported(error: &str) -> bool {
    error.contains("os error 95")
        || error.contains("Operation not supported")
        || error.contains("Not supported")
}

pub(crate) fn normalize_frequency(base: &Path, requested: &str, minimum: bool) -> String {
    let Ok(target) = requested.parse::<u64>() else {
        return requested.to_string();
    };
    if minimum && target == 0 {
        return requested.to_string();
    }
    let frequencies = util::read_trimmed(base.join("scaling_available_frequencies"))
        .ok()
        .filter(|raw| !raw.trim().is_empty())
        .or_else(|| {
            util::read_trimmed(base.join("stats/time_in_state"))
                .ok()
                .map(|raw| {
                    raw.lines()
                        .filter_map(|line| line.split_whitespace().next())
                        .collect::<Vec<_>>()
                        .join(" ")
                })
        });
    if let Some(raw) = frequencies {
        let mut values: Vec<u64> = raw
            .split_whitespace()
            .filter_map(|v| v.parse().ok())
            .filter(|v| *v > 0)
            .collect();
        values.sort_unstable();
        values.dedup();
        if !values.is_empty() {
            if minimum {
                return values
                    .iter()
                    .copied()
                    .find(|v| *v >= target)
                    .unwrap_or(*values.last().unwrap_or(&target))
                    .to_string();
            }
            // A ceiling above every listed OPP is a boost frequency the
            // driver omits from the OPP table. Pass it through unchanged and
            // let the kernel clamp to the highest OPP it actually accepts —
            // the ceiling readback comparison tolerates a lower accepted
            // value. Snapping down here would cap 极速 one OPP short forever.
            if target > *values.last().unwrap() {
                return target.to_string();
            }
            return values
                .iter()
                .rev()
                .copied()
                .find(|v| *v <= target)
                .unwrap_or(values[0])
                .to_string();
        }
    }
    let lower = util::read_trimmed(base.join("cpuinfo_min_freq"))
        .ok()
        .and_then(|v| v.parse::<u64>().ok());
    let upper = util::read_trimmed(base.join("cpuinfo_max_freq"))
        .ok()
        .and_then(|v| v.parse::<u64>().ok());
    let mut value = target;
    if let Some(lower) = lower {
        value = value.max(lower);
    }
    if let Some(upper) = upper {
        value = value.min(upper);
    }
    value.to_string()
}

/// 绝对路径的 cgroup uclamp 节点探测，与具体 Scheduler 实例无关。
/// 生产环境 kernel_root 固定为 "/"，NovaFAS 引擎线程直接使用本函数。
pub(crate) fn cpuctl_node(group: &str, field: &str) -> PathBuf {
    let paths = [
        PathBuf::from(format!("/dev/cpuctl/{group}/cpu.uclamp.{field}")),
        PathBuf::from(format!("/sys/fs/cgroup/{group}/cpu.uclamp.{field}")),
        PathBuf::from(format!("/sys/fs/cgroup/cpu/{group}/cpu.uclamp.{field}")),
    ];
    paths.iter().find(|path| path.exists()).unwrap_or(&paths[0]).clone()
}

pub fn preflight() -> Result<Hardware> {    if unsafe { crate::ffi::geteuid() } != 0 {
        return Err("必须以 root 身份运行".to_string());
    }
    let hardware = hardware::detect()?;
    // Root framework names/directories are not capabilities. Magisk/Alpha,
    // KernelSU variants and APatch all use this same hardware/node preflight.
    let legacy = Path::new("/data/adb/modules/LittleYouran_CTS_Rust");
    if legacy.is_dir() && !legacy.join("disable").exists() && !legacy.join("remove").exists() {
        return Err("检测到仍启用的旧模块 LittleYouran_CTS_Rust；请先停用/卸载并重启，避免两个守护同时写节点".into());
    }
    if ![
        "/dev/cpuset/top-app/cgroup.procs",
        "/dev/cpuset/top-app/tasks",
        "/sys/fs/cgroup/top-app/cgroup.procs",
        "/sys/fs/cgroup/top-app/tasks",
    ]
    .iter()
    .any(|p| Path::new(p).exists())
    {
        return Err("top-app cpuset 节点缺失（cgroup.procs/tasks 均不可用）".to_string());
    }
    Ok(hardware)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    #[test]
    fn missing_frequency_table_uses_stats_and_both_physical_bounds() {
        let directory =
            std::env::temp_dir().join(format!("nova-freq-fallback-{}", std::process::id()));
        fs::create_dir_all(directory.join("stats")).unwrap();
        fs::write(
            directory.join("stats/time_in_state"),
            "0 9\n100 10\n200 20\n300 30\n",
        )
        .unwrap();
        assert_eq!(normalize_frequency(&directory, "250", false), "200");
        assert_eq!(normalize_frequency(&directory, "250", true), "300");
        fs::remove_file(directory.join("stats/time_in_state")).unwrap();
        fs::write(directory.join("cpuinfo_min_freq"), "100").unwrap();
        fs::write(directory.join("cpuinfo_max_freq"), "300").unwrap();
        assert_eq!(normalize_frequency(&directory, "50", false), "100");
        assert_eq!(normalize_frequency(&directory, "500", true), "300");
        assert_eq!(normalize_frequency(&directory, "0", true), "0");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn normalizes_to_available_steps() {
        let directory = std::env::temp_dir().join(format!("cts-freq-{}", std::process::id()));
        fs::create_dir_all(&directory).expect("temp dir");
        fs::write(
            directory.join("scaling_available_frequencies"),
            b"100 200 300\n",
        )
        .expect("freqs");
        assert_eq!(normalize_frequency(&directory, "150", true), "200");
        assert_eq!(normalize_frequency(&directory, "250", false), "200");
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn extreme_power_ceiling_uses_each_policy_frequency_table() {
        let directory = std::env::temp_dir().join(format!("nova-extreme-{}", std::process::id()));
        fs::create_dir_all(&directory).unwrap();
        fs::write(
            directory.join("scaling_available_frequencies"),
            "300 400 500 600 700\n",
        )
        .unwrap();
        assert_eq!(conservative_frequency_ceiling(&directory, "700"), Some(600));
        fs::write(
            directory.join("scaling_available_frequencies"),
            "1000 1500 2000 2500\n",
        )
        .unwrap();
        assert_eq!(conservative_frequency_ceiling(&directory, "2500"), Some(2000));
        fs::write(
            directory.join("scaling_available_frequencies"),
            "100 200\n",
        )
        .unwrap();
        assert_eq!(conservative_frequency_ceiling(&directory, "100"), None);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn recognizes_kgsl_optional_not_supported_error() {
        assert!(optional_node_not_supported(
            "写入节点 /sys/class/kgsl/kgsl-3d0/force_bus_on=0 失败: Operation not supported on transport endpoint (os error 95)"
        ));
        assert!(!optional_node_not_supported(
            "Permission denied (os error 13)"
        ));
    }
}

#[cfg(test)]
mod regression_tests {
    use super::*;
    use std::fs;

    fn shipping_config() -> Config {
        crate::profiles::test_config(crate::soc::Soc::Gen3)
    }

    #[test]
    fn uclamp_transitions_keep_kernel_min_max_invariant() {
        for (old_min, old_max) in [(0, 512), (640, 640), (1024, 1024)] {
            for (new_min, new_max) in [(0, 512), (640, 640), (0, 1024), (128, 512)] {
                // Model the kernel's rejection whenever MIN exceeds MAX.
                let mut range = (old_min, old_max);
                let writes = if uclamp_max_first(new_min, old_max) {
                    [(false, new_max), (true, new_min)]
                } else {
                    [(true, new_min), (false, new_max)]
                };
                for (minimum, value) in writes {
                    if minimum {
                        range.0 = value;
                    } else {
                        range.1 = value;
                    }
                    assert!(
                        range.0 <= range.1,
                        "transition {old_min}..{old_max} -> {new_min}..{new_max}"
                    );
                }
                assert_eq!(range, (new_min, new_max));
            }
        }
    }

    fn fixture(name: &str) -> (PathBuf, Snapshot, Logger) {
        let root = std::env::temp_dir().join(format!("nova-{name}-{}", std::process::id()));
        fs::create_dir_all(root.join("walt")).expect("fixture dir");
        let logger = Logger::new(&root.join("state"));
        let snapshot = Snapshot::load(&root.join("state"), logger.clone()).expect("snapshot");
        (root, snapshot, logger)
    }

    #[test]
    fn powersave_to_fast_restores_omitted_governor_knobs_and_keeps_stock_snapshot() {
        let (root, snapshot, logger) = fixture("governor-reset");
        let base = root.join("walt");
        let mut config = shipping_config();
        config.modes.get_mut("powersave").unwrap().clusters[0].sched_params = [
            ("hispeed_freq", "0"),
            ("rtg_boost_freq", "0"),
            ("target_loads", "90"),
            ("boost", "0"),
        ]
        .into_iter()
        .map(|(key, value)| (key.into(), value.into()))
        .collect();
        config.modes.get_mut("fast").unwrap().clusters[0].sched_params =
            vec![("boost".into(), "0".into())];
        let stock = [
            ("hispeed_freq", "2016000"),
            ("rtg_boost_freq", "1500000"),
            ("target_loads", "80"),
            ("boost", "1"),
        ];
        for (key, value) in stock {
            fs::write(base.join(key), value).expect("node");
        }
        // A driver knob NovaSched never touched must stay untouched.
        fs::write(base.join("driver_private"), "17").expect("private node");
        let powersave = &config.profile("powersave").expect("save").clusters[0].sched_params;
        snapshot
            .apply_transaction(|| apply_governor_params(&snapshot, &logger, &base, powersave))
            .expect("save apply");
        assert_eq!(
            util::read_trimmed(base.join("target_loads")).expect("read"),
            "90"
        );
        // Regular fixture files do not have sysfs write semantics; model the
        // driver's shorter value after the first store callback explicitly.
        fs::write(base.join("hispeed_freq"), "0").expect("driver value");
        fs::write(base.join("rtg_boost_freq"), "0").expect("driver value");
        let fast = &config.profile("fast").expect("fast").clusters[0].sched_params;
        snapshot
            .apply_transaction(|| apply_governor_params(&snapshot, &logger, &base, fast))
            .expect("fast apply");
        for (key, value) in stock {
            let expected = if key == "boost" { "0" } else { value };
            assert_eq!(
                util::read_trimmed(base.join(key)).expect("read"),
                expected,
                "{key}"
            );
            assert_eq!(
                snapshot.original_value(&base.join(key)).expect("stock"),
                Some(value.into())
            );
        }
        assert_eq!(
            util::read_trimmed(base.join("driver_private")).expect("read"),
            "17"
        );
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn critical_cap_readback_mismatch_rolls_back_prior_node_writes() {
        let (root, snapshot, _) = fixture("cap-readback");
        let cpus = root.join("cpus");
        let cap = root.join("uclamp");
        fs::write(&cpus, "0-5").expect("cpus");
        // A retained tail models a driver returning a value other than the
        // requested cap; successful write() alone must not mean ready.
        fs::write(&cap, "99999").expect("uclamp");
        let error = snapshot
            .apply_transaction(|| {
                write_mode_control(&snapshot, &cpus, "0-7", ControlKind::CpuList)?;
                write_mode_control(&snapshot, &cap, "1024", ControlKind::Number)
            })
            .expect_err("readback must reject");
        assert!(error.contains("未按请求生效"));
        assert_eq!(util::read_trimmed(cpus).expect("cpus"), "0-5");
        assert_eq!(util::read_trimmed(cap).expect("cap"), "99999");
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn cpuset_readback_accepts_equivalent_kernel_list_formats() {
        let (root, snapshot, _) = fixture("cpuset-format");
        let cpus = root.join("cpus");
        fs::write(&cpus, "0,1,2,3,4,5,6,7\n").expect("cpus");
        write_mode_control(&snapshot, &cpus, "0-7", ControlKind::CpuList).expect("equivalent cpus");
        assert_eq!(snapshot.original_value(&cpus).expect("stock"), None);
        fs::remove_dir_all(root).expect("cleanup");
    }
    #[test]
    fn frequency_order_allows_moving_between_disjoint_ranges() {
        assert!(range_max_first("1800000", "1200000"));
        assert!(!range_max_first("600000", "2400000"));
        assert!(!range_max_first("1200000", "1200000"));
    }
}

#[cfg(test)]
mod daemon_policy_tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicUsize, Ordering};
    #[test]
    fn each_soc_runs_all_four_profiles_and_never_writes_inactive_policy_slots() {
        for soc in crate::soc::SUPPORTED {
            let stamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let root = std::env::temp_dir().join(format!(
                "nova-soc-{}-{}-{stamp}",
                soc.id(),
                std::process::id()
            ));
            let config = crate::profiles::test_config(soc);
            let anchors = soc.anchors();
            let mut policies = Vec::new();
            for (slot, id) in anchors.into_iter().enumerate().filter(|(_, id)| *id >= 0) {
                let base = root.join(format!("sys/devices/system/cpu/cpufreq/policy{id}"));
                fs::create_dir_all(&base).unwrap();
                let table = config
                    .modes
                    .values()
                    .map(|p| p.clusters[slot].max_freq.as_str())
                    .collect::<Vec<_>>()
                    .join(" ");
                for (name, value) in [
                    ("scaling_min_freq", "0"),
                    ("scaling_max_freq", "9000000"),
                    ("scaling_governor", "walt"),
                    ("scaling_available_frequencies", &table),
                ] {
                    fs::write(base.join(name), value).unwrap();
                }
                let end = anchors.into_iter().filter(|a| *a > id).min().unwrap_or(8);
                policies.push(hardware::CpuPolicy {
                    id,
                    cpus: (id as u32..end as u32).collect(),
                    governors: vec!["walt".into(), "schedutil".into()],
                    current_governor: "walt".into(),
                    min_freq: 300000,
                    max_freq: 3000000,
                    capacity: None,
                });
            }
            for group in [
                "top-app",
                "foreground",
                "restricted",
                "system-background",
                "background",
            ] {
                let p = root.join(format!("dev/cpuset/{group}"));
                fs::create_dir_all(&p).unwrap();
                fs::write(p.join("cpus"), "0-7").unwrap();
            }
            let logger = Logger::new(&root.join("state"));
            let snapshot = Snapshot::load(&root.join("state"), logger.clone()).unwrap();
            let hardware = Hardware {
                samsung: false,
                soc,
                device: "test".into(),
                evidence: "fake sysfs".into(),
                policies,
            };
            let mut scheduler = Scheduler::new(
                config.clone(),
                snapshot,
                logger,
                hardware,
                Arc::new(CpuctlLive::default()),
            )
            .unwrap();
            scheduler.kernel_root = root.clone();
            scheduler.initialize("balance").unwrap();
            for mode in crate::config::MODES {
                // sysfs buffers are not regular files: shorten the fixture buffer
                // before a shorter governor name, after factory capture occurred.
                for id in anchors.into_iter().filter(|id| *id >= 0) {
                    for name in ["scaling_governor", "scaling_max_freq"] {
                        fs::write(
                            root.join(format!("sys/devices/system/cpu/cpufreq/policy{id}/{name}")),
                            "",
                        )
                        .unwrap();
                    }
                }
                scheduler
                    .apply("org.example.game", mode, false, true, false)
                    .unwrap();
                for (slot, id) in anchors.into_iter().enumerate().filter(|(_, id)| *id >= 0) {
                    let base = root.join(format!("sys/devices/system/cpu/cpufreq/policy{id}"));
                    let expected = &config.profile(mode).unwrap().clusters[slot];
                    assert_eq!(
                        util::read_trimmed(base.join("scaling_max_freq")).unwrap(),
                        expected.max_freq,
                        "{} {mode}",
                        soc.id()
                    );
                    assert_eq!(
                        util::read_trimmed(base.join("scaling_governor")).unwrap(),
                        expected.governor
                    );
                }
                assert!(scheduler
                    .snapshot
                    .written_nodes()
                    .iter()
                    .all(|(p, _)| !p.to_string_lossy().contains("policy-1")));
            }
            fs::remove_dir_all(root).unwrap();
        }
    }

    struct Fixture {
        root: PathBuf,
        scheduler: Scheduler,
    }
    impl Fixture {
        fn new() -> Self {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let root = std::env::temp_dir().join(format!(
                "nova-policy-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            let mut config =
                Config::from_text(include_str!("../../module-template/config/SM8650.json"))
                    .unwrap();
            config.functions.launch_boost.enable = true;
            config.functions.launch_boost.rate_limit_ms = 0;
            config.functions.launch_boost.frequencies = std::array::from_fn(|_| "3000000".into());
            config.functions.disable_gpu_boost = false;
            config.functions.scheduler.enable = true;
            config.functions.cpuset.enable = true;
            config.functions.cpuset.top_app = "0-7".into();
            config.functions.cpuset.foreground = "0-7".into();
            config.functions.scheduler.util_clamp_min = "1024".into();
            config.functions.scheduler.util_clamp_max = "1024".into();
            for (name, mode) in &mut config.modes {
                for cluster in &mut mode.clusters {
                    cluster.min_freq = "0".into();
                    cluster.max_freq = if name == "fast" { "2500000" } else { "2000000" }.into();
                    cluster.governor = "walt".into();
                    cluster.sched_params.clear();
                }
            }
            let mut policies = Vec::new();
            for (id, cpus) in [
                (0, vec![0, 1]),
                (2, vec![2, 3, 4]),
                (5, vec![5, 6]),
                (7, vec![7]),
            ] {
                let base = root.join(format!("sys/devices/system/cpu/cpufreq/policy{id}"));
                fs::create_dir_all(&base).unwrap();
                for (name, value) in [
                    ("scaling_min_freq", "0"),
                    ("scaling_max_freq", "1000000"),
                    ("scaling_governor", "walt"),
                    (
                        "scaling_available_frequencies",
                        "1000000 2000000 2500000 3000000",
                    ),
                ] {
                    fs::write(base.join(name), value).unwrap();
                }
                policies.push(hardware::CpuPolicy {
                    id,
                    cpus: cpus.into_iter().collect(),
                    governors: vec!["walt".into()],
                    current_governor: "walt".into(),
                    min_freq: 300000,
                    max_freq: 3000000,
                    capacity: None,
                });
            }
            for group in ["top-app", "foreground"] {
                let dir = root.join(format!("dev/cpuset/{group}"));
                fs::create_dir_all(&dir).unwrap();
                fs::write(dir.join("cpus"), "0-7").unwrap();
            }
            let sysctl = root.join("proc/sys/kernel");
            fs::create_dir_all(&sysctl).unwrap();
            for name in ["sched_util_clamp_min", "sched_util_clamp_max"] {
                fs::write(sysctl.join(name), "1024").unwrap();
            }
            let logger = Logger::new(&root.join("state"));
            let snapshot = Snapshot::load(&root.join("state"), logger.clone()).unwrap();
            let hardware = Hardware {
                samsung: false,
                soc: crate::soc::Soc::Gen3,
                device: "fixture".into(),
                evidence: "fake node tree".into(),
                policies,
            };
            let mut scheduler = Scheduler::new(
                config,
                snapshot,
                logger,
                hardware,
                Arc::new(CpuctlLive::default()),
            )
            .unwrap();
            scheduler.kernel_root = root.clone();
            scheduler.initialize("balance").unwrap();
            scheduler
                .apply("org.example.a", "balance", false, false, false)
                .unwrap();
            Self { root, scheduler }
        }
        fn count(&self) -> usize {
            self.scheduler.snapshot.written_nodes().len()
        }
        fn writes_since(&self, start: usize) -> Vec<(PathBuf, String)> {
            self.scheduler.snapshot.written_nodes()[start..].to_vec()
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn shell_burst_borrows_performance_envelope_for_systemui_only() {
        assert!(shell_burst(false, "com.android.systemui", "powersave"));
        assert!(shell_burst(false, "com.android.systemui", "balance"));
        assert!(!shell_burst(false, "com.android.systemui", "fast"));
        assert!(!shell_burst(false, "com.miui.home", "balance"));
        assert!(!shell_burst(true, "com.android.systemui", "balance"));
    }

    #[test]
    fn notification_shade_burst_lifts_caps_and_closing_restores_mode() {
        let mut f = Fixture::new();
        // 让性能档有独立于均衡档的可观测上限。
        for cluster in 0..4 {
            f.scheduler.config.modes.get_mut("performance").unwrap().clusters[cluster].max_freq =
                "2400000".into();
        }
        let policy0 = f
            .root
            .join("sys/devices/system/cpu/cpufreq/policy0/scaling_max_freq");
        for policy in [0, 2, 5, 7] {
            fs::write(f.root.join(format!("sys/devices/system/cpu/cpufreq/policy{policy}/scaling_available_frequencies")), "1000000 2000000 2400000 2500000 3000000").unwrap();
        }
        // 均衡档下普通应用：2000000。
        f.scheduler
            .apply("com.example.app", "balance", false, true, false)
            .unwrap();
        assert_eq!(util::read_trimmed(&policy0).unwrap(), "2000000");
        // 下拉通知栏（SystemUI 前台）：借用性能档包络 2400000，档位状态不变。
        f.scheduler
            .apply("com.android.systemui", "balance", false, false, false)
            .unwrap();
        assert_eq!(util::read_trimmed(&policy0).unwrap(), "2400000");
        assert_eq!(f.scheduler.current_mode(), "balance");
        assert_eq!(f.scheduler.current_package(), "com.android.systemui");
        // 通知栏收起，应用回到前台：立即回到均衡档包络。
        f.scheduler
            .apply("com.example.app", "balance", false, false, false)
            .unwrap();
        assert_eq!(util::read_trimmed(&policy0).unwrap(), "2000000");
        // 极速档下 SystemUI 不借用（已高于性能档）。
        f.scheduler
            .apply("com.example.app", "fast", false, false, false)
            .unwrap();
        f.scheduler
            .apply("com.android.systemui", "fast", false, false, false)
            .unwrap();
        assert_eq!(util::read_trimmed(&policy0).unwrap(), "2500000");
    }

    #[test]
    fn same_mode_app_switch_writes_each_boost_cap_once_then_restores_profile() {
        let mut f = Fixture::new();
        let before = f.count();
        f.scheduler
            .apply("org.example.b", "balance", true, false, false)
            .unwrap();
        let writes = f.writes_since(before);
        for policy in [0, 2, 5, 7] {
            let path = f.root.join(format!(
                "sys/devices/system/cpu/cpufreq/policy{policy}/scaling_max_freq"
            ));
            let caps: Vec<_> = writes
                .iter()
                .filter(|(p, _)| p == &path)
                .map(|(_, v)| v.as_str())
                .collect();
            assert_eq!(caps, ["3000000", "2000000"], "policy{policy}");
            assert_eq!(util::read_trimmed(path).unwrap(), "2000000");
        }
        let before = f.count();
        f.scheduler
            .apply("org.example.b", "balance", true, false, false)
            .unwrap();
        assert_eq!(f.count(), before, "same package must not retrigger boost");
    }

    #[test]
    fn startup_batches_all_fixture_nodes_before_first_policy_write() {
        let f = Fixture::new();
        assert!(f.count() > 0);
        assert_eq!(f.scheduler.snapshot.persist_count(), 1);
        let max = f
            .root
            .join("sys/devices/system/cpu/cpufreq/policy0/scaling_max_freq");
        assert_eq!(
            f.scheduler
                .snapshot
                .original_value(&max)
                .unwrap()
                .as_deref(),
            Some("1000000")
        );
        assert_eq!(util::read_trimmed(&max).unwrap(), "2000000");
    }

    #[test]
    fn disabled_launch_boost_keeps_same_mode_switch_read_only() {
        let mut f = Fixture::new();
        f.scheduler.config.functions.launch_boost.enable = false;
        let before = f.count();
        f.scheduler
            .apply("org.example.b", "balance", true, false, false)
            .unwrap();
        assert_eq!(f.count(), before);
        assert_eq!(f.scheduler.current_package(), "org.example.b");
    }

    #[test]
    fn healthy_audit_is_read_only_and_numeric_cpuset_formatting_is_equivalent() {
        let mut f = Fixture::new();
        fs::write(f.scheduler.cpuset_path("top-app"), "0,1,2,3,4,5,6,7\n").unwrap();
        let before = f.count();
        assert!(!f.scheduler.audit_and_repair().unwrap());
        assert_eq!(f.count(), before);
    }

    #[test]
    fn drift_in_each_critical_control_is_repaired_without_launch_boost() {
        let cases = [
            (
                "sys/devices/system/cpu/cpufreq/policy0/scaling_max_freq",
                "3000000",
                "2000000",
            ),
            (
                "sys/devices/system/cpu/cpufreq/policy2/scaling_max_freq",
                "3000000",
                "2000000",
            ),
            (
                "sys/devices/system/cpu/cpufreq/policy5/scaling_max_freq",
                "3000000",
                "2000000",
            ),
            (
                "sys/devices/system/cpu/cpufreq/policy7/scaling_max_freq",
                "3000000",
                "2000000",
            ),
            (
                "sys/devices/system/cpu/cpufreq/policy7/scaling_governor",
                "noop",
                "walt",
            ),
            ("proc/sys/kernel/sched_util_clamp_min", "0000", "1024"),
            ("proc/sys/kernel/sched_util_clamp_max", "1000", "1024"),
            ("dev/cpuset/top-app/cpus", "0-5", "0-7"),
        ];
        for (name, drift, expected) in cases {
            let mut f = Fixture::new();
            let path = f.root.join(name);
            fs::write(&path, drift).unwrap();
            let before = f.count();
            assert!(f.scheduler.audit_and_repair().unwrap(), "{name}");
            assert_eq!(util::read_trimmed(&path).unwrap(), expected);
            assert!(f.writes_since(before).iter().all(|(_, v)| v != "3000000"));
            assert!(!f.scheduler.audit_and_repair().unwrap());
            let log = fs::read_to_string(f.root.join("state/logs/novasched.log")).unwrap();
            assert_eq!(log.matches("检测到漂移已纠正").count(), 1);
        }
    }

    #[test]
    fn repair_failure_rolls_back_and_does_not_log_correction_or_accept_new_mode() {
        let mut f = Fixture::new();
        let max = f
            .root
            .join("sys/devices/system/cpu/cpufreq/policy0/scaling_max_freq");
        fs::write(&max, "3000000").unwrap();
        let params = f.root.join("sys/devices/system/cpu/cpufreq/policy0/walt");
        fs::create_dir_all(params.join("blocked")).unwrap();
        f.scheduler
            .config
            .modes
            .get_mut("balance")
            .unwrap()
            .clusters[0]
            .sched_params
            .push(("blocked".into(), "1".into()));
        assert!(f.scheduler.audit_and_repair().is_err());
        assert_eq!(util::read_trimmed(&max).unwrap(), "3000000");
        assert_eq!(f.scheduler.current_mode(), "balance");
        assert!(!fs::read_to_string(f.root.join("state/logs/novasched.log"))
            .unwrap()
            .contains("漂移已纠正"));
        f.scheduler
            .config
            .modes
            .get_mut("balance")
            .unwrap()
            .clusters[0]
            .sched_params
            .clear();
        assert!(
            f.scheduler.audit_and_repair().unwrap(),
            "retry repairs after cause removed"
        );
    }

    #[test]
    fn unreadable_or_disappearing_managed_nodes_are_not_reported_healthy() {
        let mut f = Fixture::new();
        let path = f.scheduler.cpuset_path("top-app");
        fs::remove_file(path).unwrap();
        assert!(f.scheduler.audit_and_repair().is_err());
        assert!(f
            .scheduler
            .apply("org.example.a", "balance", false, true, false)
            .is_err());
    }

    #[test]
    fn node_watchdog_can_be_disabled_without_rewriting_nodes() {
        let mut f = Fixture::new();
        f.scheduler.config.functions.node_watchdog = false;
        fs::write(f.scheduler.cpuset_path("top-app"), "0-5").unwrap();
        let before = f.count();
        assert!(!f.scheduler.audit_and_repair().unwrap());
        assert_eq!(before, f.count());
    }

    #[test]
    fn lower_system_frequency_caps_are_accepted_without_watchdog_writes() {
        let mut f = Fixture::new();
        for policy in [0, 2, 5, 7] {
            fs::write(
                f.root.join(format!(
                    "sys/devices/system/cpu/cpufreq/policy{policy}/scaling_max_freq"
                )),
                "1000000",
            )
            .unwrap();
        }
        let before = f.count();
        assert!(!f.scheduler.audit_and_repair().unwrap());
        assert_eq!(before, f.count());
        assert!(frequency_ceiling_matches("1000000", "2000000"));
        assert!(!frequency_ceiling_matches("0", "2000000"));
        assert!(!frequency_ceiling_matches("invalid", "2000000"));
        assert!(!frequency_ceiling_matches("3000000", "2000000"));
    }

    #[test]
    fn repairing_another_control_does_not_raise_a_lower_system_cap() {
        let mut f = Fixture::new();
        let path = f
            .root
            .join("sys/devices/system/cpu/cpufreq/policy0/scaling_max_freq");
        fs::write(&path, "1000000").unwrap();
        fs::write(f.scheduler.cpuset_path("top-app"), "0-5").unwrap();
        let before = f.count();
        assert!(f.scheduler.audit_and_repair().unwrap());
        assert_eq!(util::read_trimmed(&path).unwrap(), "1000000");
        assert!(f
            .writes_since(before)
            .iter()
            .filter(|(p, _)| p == &path)
            .all(|(_, value)| value.parse::<u64>().unwrap() <= 1000000));
        assert!(!f.scheduler.audit_and_repair().unwrap());
    }

    #[test]
    fn narrow_frequency_ranges_do_not_invert_after_opp_rounding() {
        let f = Fixture::new();
        let base = f.root.join("sys/devices/system/cpu/cpufreq/policy0");
        assert_eq!(
            frequency_range(&base, "1500000", "1600000").unwrap(),
            ("1000000".into(), "1000000".into())
        );
    }

    #[test]
    fn ten_core_xring_applies_all_policies_and_tracks_cpu9_hotplug() {
        let f = Fixture::new();
        let mut hardware = hardware::xring_ten_core_fixture();
        for policy in &mut hardware.policies { policy.min_freq = 1000000; }
        let mut config = Config::from_text(include_str!("../../module-template/config/XRING_O3.json")).unwrap();
        config.functions.cpuctl.enable = false;
        config.functions.input_boost.enabled = false;
        config.functions.fas.enabled = false;
        config.modes.get_mut("powersave").unwrap().online[9] = false;
        for policy in &hardware.policies {
            let base = f.root.join(format!("sys/devices/system/cpu/cpufreq/policy{}", policy.id));
            fs::create_dir_all(&base).unwrap();
            // Equal-width values avoid regular-file tail bytes; sysfs replaces values itself.
            let frequencies = [1000000, 1500000, 1800000, 2000000, 2300000, 2800000, 3100000]
                .into_iter().filter(|frequency| *frequency <= policy.max_freq)
                .map(|frequency| frequency.to_string()).collect::<Vec<_>>().join(" ");
            for (name, value) in [
                ("scaling_min_freq", "1000000".into()),
                ("scaling_max_freq", policy.max_freq.to_string()),
                ("cpuinfo_min_freq", "1000000".into()),
                ("cpuinfo_max_freq", policy.max_freq.to_string()),
                ("scaling_governor", "schedutil".into()),
                ("scaling_available_frequencies", frequencies),
            ] { fs::write(base.join(name), value).unwrap(); }
        }
        let cpu9 = f.root.join("sys/devices/system/cpu/cpu9/online");
        fs::create_dir_all(cpu9.parent().unwrap()).unwrap();
        fs::write(&cpu9, "1").unwrap();
        let logger = Logger::new(&f.root.join("xring-state"));
        let snapshot = Snapshot::load(&f.root.join("xring-state"), logger.clone()).unwrap();
        let mut scheduler = Scheduler::new(config, snapshot, logger, hardware, Arc::new(CpuctlLive::default())).unwrap();
        scheduler.kernel_root = f.root.clone();
        scheduler.initialize("balance").unwrap();
        assert_eq!(scheduler.snapshot.original_value(&cpu9).unwrap().as_deref(), Some("1"));
        for mode in ["powersave", "balance", "performance", "fast"] {
            scheduler.apply("org.example.o3", mode, false, true, false).unwrap();
            assert_eq!(util::read_trimmed(&cpu9).unwrap(), if mode == "powersave" { "0" } else { "1" });
            for policy in &scheduler.hardware.policies {
                let path = f.root.join(format!("sys/devices/system/cpu/cpufreq/policy{}/scaling_max_freq", policy.id));
                assert!(scheduler.snapshot.written_nodes().iter().any(|(written, _)| written == &path));
                assert!(util::read_trimmed(path).unwrap().parse::<u64>().unwrap() <= policy.max_freq);
            }
        }
        let mut replacement = scheduler.config.clone();
        replacement.modes.get_mut("powersave").unwrap().online[9] = true;
        assert!(scheduler.replace_config(replacement).unwrap_err().contains("热插拔控制范围"));
    }

    #[test]
    fn default_online_values_leave_oem_offline_cpus_untouched() {
        let mut f = Fixture::new();
        let path = f.root.join("sys/devices/system/cpu/cpu7/online");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "0").unwrap();
        f.scheduler.capture_initial_nodes().unwrap();
        assert!(f
            .scheduler
            .snapshot
            .original_value(&path)
            .unwrap()
            .is_none());
        f.scheduler
            .apply("org.example.b", "fast", false, true, false)
            .unwrap();
        assert_eq!(util::read_trimmed(&path).unwrap(), "0");
        assert!(f
            .scheduler
            .snapshot
            .written_nodes()
            .iter()
            .all(|(p, _)| p != &path));
    }

    #[test]
    fn gpu_option_does_not_capture_or_write_thermal_controls() {
        let mut f = Fixture::new();
        let base = f.root.join("sys/class/kgsl/kgsl-3d0");
        fs::create_dir_all(&base).unwrap();
        for (name, value) in [
            ("thermal_pwrlevel", "2"),
            ("throttling", "1"),
            ("num_pwrlevels", "4"),
            ("default_pwrlevel", "3"),
            ("min_pwrlevel", "3"),
            ("max_pwrlevel", "0"),
        ] {
            fs::write(base.join(name), value).unwrap();
        }
        f.scheduler.config.functions.disable_gpu_boost = true;
        f.scheduler.capture_initial_nodes().unwrap();
        f.scheduler
            .snapshot
            .apply_transaction(|| f.scheduler.apply_functions())
            .unwrap();
        for (name, value) in [("thermal_pwrlevel", "2"), ("throttling", "1")] {
            let path = base.join(name);
            assert_eq!(util::read_trimmed(&path).unwrap(), value);
            assert!(f
                .scheduler
                .snapshot
                .original_value(&path)
                .unwrap()
                .is_none());
            assert!(f
                .scheduler
                .snapshot
                .written_nodes()
                .iter()
                .all(|(p, _)| p != &path));
        }
    }

    #[test]
    fn default_profiles_do_not_capture_or_reset_an_oem_gpu_cap() {
        let mut f = Fixture::new();
        let path = f.root.join("sys/class/kgsl/kgsl-3d0/max_pwrlevel");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "2").unwrap();
        f.scheduler.capture_initial_nodes().unwrap();
        assert!(f
            .scheduler
            .snapshot
            .original_value(&path)
            .unwrap()
            .is_none());
        f.scheduler
            .apply("org.example.b", "fast", false, true, false)
            .unwrap();
        assert_eq!(util::read_trimmed(&path).unwrap(), "2");
        assert!(f
            .scheduler
            .snapshot
            .written_nodes()
            .iter()
            .all(|(p, _)| p != &path));
    }

    #[test]
    fn startup_only_changes_are_rejected_without_replacing_live_config() {
        let mut f = Fixture::new();
        let mut next = f.scheduler.config.clone();
        next.functions.cpuset.enable = false;
        assert!(f
            .scheduler
            .replace_config(next)
            .unwrap_err()
            .contains("重启"));
        assert!(f.scheduler.config.functions.cpuset.enable);
        assert!(!f.scheduler.audit_and_repair().unwrap());
    }

    #[test]
    fn thermal_and_battery_services_are_rejected_before_node_writes() {
        let mut f = Fixture::new();
        f.scheduler.config.functions.perf_service_lock.enable = true;
        for name in [
            "thermal-engine",
            "vendor.thermal-hal",
            "vendor.health",
            "charger",
        ] {
            f.scheduler.config.functions.perf_service_lock.services = vec![name.into()];
            let before = f.count();
            assert!(f
                .scheduler
                .initialize("balance")
                .unwrap_err()
                .contains("不能停用"));
            assert_eq!(before, f.count());
        }
    }
    #[test]
    fn same_mode_rule_changes_apply_and_restore_placement_without_launch_boost() {
        let mut fixture = Fixture::new();
        fixture.scheduler.config.functions.launch_boost.enable = false;
        fixture.scheduler.config.functions.cpuset.app_rule = Some(crate::config::CpusetMode {
            top_app: Some("2-7".into()),
            foreground: Some("2-7".into()),
        });
        fixture
            .scheduler
            .apply("org.example.a", "balance", false, false, true)
            .unwrap();
        let node = fixture.root.join("dev/cpuset/top-app/cpus");
        assert_eq!(util::read_trimmed(&node).unwrap(), "2-7");
        fixture
            .scheduler
            .apply("org.example.b", "balance", false, false, false)
            .unwrap();
        assert_eq!(util::read_trimmed(&node).unwrap(), "0-7");
        fixture
            .scheduler
            .apply("org.example.b", "balance", false, false, true)
            .unwrap();
        assert_eq!(util::read_trimmed(&node).unwrap(), "2-7");
    }

    #[test]
    fn automatic_game_exit_lowers_actual_caps_and_restores_cpu_placement() {
        use crate::automatic::{AppKind, Classifier};
        let mut fixture = Fixture::new();
        fixture.scheduler.config.functions.launch_boost.enable = false;
        fixture.scheduler.config.functions.cpuset.app_rule = Some(crate::config::CpusetMode {
            top_app: Some("2-7".into()),
            foreground: Some("2-7".into()),
        });
        let mut classifier = Classifier::default();
        for package in [
            "com.tencent.tmgp.sgame",
            "com.android.launcher3",
            "com.tencent.ig",
            "com.example.chat",
            "",
        ] {
            let kind =
                classifier.classify_with_for_test(package, || Err("metadata unavailable".into()));
            fixture
                .scheduler
                .apply(package, kind.mode(), false, false, kind == AppKind::Game)
                .unwrap();
            let game = kind == AppKind::Game;
            for policy in [0, 2, 5, 7] {
                let node = fixture.root.join(format!(
                    "sys/devices/system/cpu/cpufreq/policy{policy}/scaling_max_freq"
                ));
                assert_eq!(
                    util::read_trimmed(node).unwrap(),
                    if game { "2500000" } else { "2000000" }
                );
            }
            assert_eq!(
                util::read_trimmed(fixture.root.join("dev/cpuset/top-app/cpus")).unwrap(),
                if game { "2-7" } else { "0-7" }
            );
            assert_eq!(fixture.scheduler.current_mode(), kind.mode());
            assert_eq!(fixture.scheduler.last_explicit(), game);
        }
    }
    #[test]
    fn live_cpuctl_and_input_configuration_changes_require_a_restart() {
        let mut fixture = Fixture::new();
        let mut config = fixture.scheduler.config.clone();
        let old_input = config.functions.input_boost.enabled;
        config.functions.input_boost.enabled = !old_input;
        assert!(fixture
            .scheduler
            .replace_config(config)
            .unwrap_err()
            .contains("重启"));
        assert_eq!(fixture.scheduler.config.functions.input_boost.enabled, old_input);
        let mut config = fixture.scheduler.config.clone();
        let old_cpuctl = config.functions.cpuctl.enable;
        config.functions.cpuctl.enable = !old_cpuctl;
        assert!(fixture
            .scheduler
            .replace_config(config)
            .unwrap_err()
            .contains("重启"));
        assert_eq!(fixture.scheduler.config.functions.cpuctl.enable, old_cpuctl);
    }

    #[test]
    fn input_boost_can_use_stock_baseline_without_static_cpuctl_tuning() {
        let mut fixture = Fixture::new();
        let path = fixture.root.join("dev/cpuctl/top-app/cpu.uclamp.min");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "12.34").unwrap();
        fixture.scheduler.snapshot.capture(&path).unwrap();
        fixture.scheduler.config.functions.cpuctl.enable = false;
        fixture.scheduler.config.functions.input_boost.enabled = true;
        fixture.scheduler.apply_mode_cpuctl("fast", true).unwrap();
        let state = fixture.scheduler.cpuctl_live.inner.lock().unwrap();
        assert_eq!(state.top_app_min, "12.34");
        assert_eq!(state.boost_min, "40");
        assert_eq!(state.duration_ms, 220);
        assert_eq!(util::read_trimmed(&path).unwrap(), "12.34");
    }
    #[test]
    fn fas_enabled_late_restarted_or_corrupted_refreshes_without_cpu_writes() {
        let mut fixture = Fixture::new();
        let path = fixture.root.join("dev/fas_rs/mode");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let cpu_writes = fixture.scheduler.snapshot.written_nodes().len();
        fs::write(&path, "fast").unwrap();
        let status = fixture.scheduler.refresh_fas(true);
        assert_eq!(status.sync, "synced");
        assert_eq!(status.mode, fixture.scheduler.current_mode());
        assert_eq!(fixture.scheduler.snapshot.written_nodes().len(), cpu_writes);
        fs::write(&path, "fastormance").unwrap();
        assert_eq!(fixture.scheduler.refresh_fas(true).state, "ready");
        fs::write(&path, "performance").unwrap();
        assert_eq!(fixture.scheduler.refresh_fas(false).sync, "paused");
        assert_eq!(fs::read_to_string(&path).unwrap(), "performance");
        fs::remove_file(&path).unwrap();
        assert_ne!(fixture.scheduler.refresh_fas(true).state, "ready");
        fs::write(&path, "balance").unwrap();
        assert_eq!(fixture.scheduler.refresh_fas(true).sync, "synced");
    }
    #[test]
    fn mode_input_pulses_use_the_selected_profile_and_restore_stock_baseline() {
        let mut fixture = Fixture::new();
        let path = fixture.root.join("sys/fs/cgroup/top-app/cpu.uclamp.min");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "12.34").unwrap();
        fixture.scheduler.snapshot.capture(&path).unwrap();
        assert_eq!(fixture.scheduler.input_boost_node(), path);
        for (mode, value, duration) in [("powersave", "15", 100), ("balance", "20", 140), ("performance", "28", 180), ("fast", "40", 220)] {
            fixture.scheduler.apply_mode_cpuctl(mode, false).unwrap();
            let state = fixture.scheduler.cpuctl_live.inner.lock().unwrap();
            assert_eq!(state.top_app_min, "12.34");
            assert_eq!(state.boost_min, value);
            assert_eq!(state.duration_ms, duration);
        }
    }

    #[test]
    fn rejected_ceilings_get_only_three_spaced_retries_and_stay_visible() {
        let mut fixture = Fixture::new();
        let path = fixture.root.join("sys/devices/system/cpu/cpufreq/policy0/scaling_max_freq");
        // A regular fixture retains trailing bytes, modelling an unaccepted
        // smaller ceiling without a live kernel or vendor HAL.
        fs::write(&path, "3000000").unwrap();
        fixture.scheduler.pending_enforcement = vec![(path, "100000".into())];
        fixture.scheduler.enforcement_after = std::time::Instant::now() + Duration::from_secs(10);
        let before = fixture.count();
        assert!(!fixture.scheduler.enforce_pending());
        assert_eq!(fixture.count(), before);
        for _ in 0..3 {
            fixture.scheduler.enforcement_after = std::time::Instant::now();
            assert!(fixture.scheduler.enforce_pending());
        }
        assert_eq!(fixture.count(), before + 3);
        assert!(fixture.scheduler.pending_retry_exhausted());
        assert_eq!(fixture.scheduler.pending_control_count(), 1);
        fixture.scheduler.enforcement_after = std::time::Instant::now();
        assert!(!fixture.scheduler.enforce_pending());
        assert_eq!(fixture.count(), before + 3);
    }

    #[test]
    fn samsung_never_reasserts_vendor_ceilings_or_follows_external_fas() {
        let mut fixture = Fixture::new();
        fixture.scheduler.hardware.samsung = true;
        fixture.scheduler.config.functions.fas.enabled = false;
        let path = fixture.root.join("sys/devices/system/cpu/cpufreq/policy0/scaling_max_freq");
        fixture.scheduler.pending_enforcement = vec![(path, "100000".into())];
        let before = fixture.count();
        assert!(!fixture.scheduler.enforce_pending());
        assert_eq!(fixture.count(), before);
        let fas = fixture.root.join("dev/fas_rs/mode");
        fs::create_dir_all(fas.parent().unwrap()).unwrap();
        fs::write(&fas, "fast").unwrap();
        assert_eq!(fixture.scheduler.refresh_fas(true).sync, "paused");
        assert_eq!(fs::read_to_string(fas).unwrap(), "fast");
    }

    #[test]
    fn samsung_only_captures_frequency_limits_and_never_restores_a_vendor_governor_change() {
        let fixture = Fixture::new();
        let mut hardware = fixture.scheduler.hardware.clone();
        hardware.samsung = true;
        let logger = Logger::new(&fixture.root.join("samsung-state"));
        let snapshot = Snapshot::load(&fixture.root.join("samsung-state"), logger.clone()).unwrap();
        let mut scheduler = Scheduler::new(
            fixture.scheduler.config.clone(), snapshot.clone(), logger,
            hardware, Arc::new(CpuctlLive::default()),
        ).unwrap();
        scheduler.kernel_root = fixture.root.clone();
        scheduler.initialize("balance").unwrap();
        let governor = fixture.root.join("sys/devices/system/cpu/cpufreq/policy0/scaling_governor");
        assert!(snapshot.original_value(&governor).unwrap().is_none());
        fs::write(&governor, "schedutil").unwrap();
        scheduler.apply("org.example.b", "balance", false, true, false).unwrap();
        assert_eq!(fs::read_to_string(&governor).unwrap(), "schedutil");
        snapshot.restore().unwrap();
        assert_eq!(fs::read_to_string(&governor).unwrap(), "schedutil");
        for (path, _) in snapshot.written_nodes() {
            assert!(matches!(path.file_name().and_then(|name| name.to_str()), Some("scaling_min_freq" | "scaling_max_freq")), "{}", path.display());
        }
    }

}
