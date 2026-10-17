use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use crate::json::{self, Value};
use crate::util::{self, Result};

pub const MODES: [&str; 4] = ["powersave", "balance", "performance", "fast"];
/// Profile generation, independent of the UI version. Bump when defaults change
/// so unedited configurations update while custom tuning migrates in place.
pub const PROFILE_VERSION: i64 = 390;

#[derive(Clone, Debug)]
pub struct Meta {
    pub name: String,
    pub version: i64,
    pub author: String,
    pub loglevel: String,
    pub profile_soc: Option<crate::soc::Soc>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cpuset {
    pub enable: bool,
    pub top_app: String,
    pub foreground: String,
    pub restricted: String,
    pub system_background: String,
    pub background: String,
    /// Per-mode top_app/foreground overrides, e.g. shedding little cores in
    /// fast mode. Modes without an entry keep the static values.
    pub modes: BTreeMap<String, CpusetMode>,
    /// Applied only while an explicitly ruled app (a user-marked game) is
    /// foreground. The configuration can omit this override.
    pub app_rule: Option<CpusetMode>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CpusetMode {
    pub top_app: Option<String>,
    pub foreground: Option<String>,
}

/// cgroup-v2 cpuctl group clamps (cpu.uclamp.min/max), applied per mode as a
/// best-effort refinement: foreground keeps a util floor in fast mode,
/// background gets a util ceiling in powersave. Nodes missing on kernels
/// without CONFIG_UCLAMP_TASK_GROUP are skipped silently.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CpuctlMode {
    pub top_app_min: Option<String>,
    /// Applied only while an explicitly ruled app is foreground: the
    /// per-app utilization floor.
    pub top_app_min_rule: Option<String>,
    pub background_max: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Cpuctl {
    pub enable: bool,
    pub modes: BTreeMap<String, CpuctlMode>,
}

#[derive(Clone, Debug)]
pub struct LaunchBoost {
    pub enable: bool,
    pub rate_limit_ms: u64,
    pub frequencies: [String; 4],
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SchedulerConfig {
    pub enable: bool,
    pub energy_aware: bool,
    pub schedstats: bool,
    pub latency_ns: String,
    pub migration_cost_ns: String,
    pub min_granularity_ns: String,
    pub wakeup_granularity_ns: String,
    pub nr_migrate: String,
    pub util_clamp_min: String,
    pub util_clamp_max: String,
}

#[derive(Clone, Debug, Default)]
pub struct ForegroundMonitorConfig {
    pub ignored_packages: BTreeSet<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PerfServiceLock {
    pub enable: bool,
    pub services: Vec<String>,
}

/// Touch-event-driven transient boost: while input activity is detected the
/// top-app group carries a uclamp.min pulse for `duration_ms`, then returns
/// to the dispatcher-maintained baseline.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InputBoost {
    pub enabled: bool,
    pub top_app_min: String,
    pub duration_ms: u64,
    pub modes: BTreeMap<String, InputBoostMode>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InputBoostMode {
    pub top_app_min: String,
    pub duration_ms: u64,
}

impl Default for InputBoost {
    fn default() -> Self {
        Self {
            enabled: false,
            top_app_min: "30".into(),
            duration_ms: 300,
            modes: BTreeMap::new(),
        }
    }
}

/// Frame-aware scheduling engine (NovaFAS). Reads the foreground app's render
/// frame signal and dynamically raises/relaxes the top-app utilization floor
/// and per-cluster minimum frequencies, always inside the mode envelope.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FasMode {
    /// Highest top-app cpu.uclamp.min the engine may set, in percent.
    pub ceiling: u32,
    /// Highest per-cluster minimum frequency the engine may set, as a percent
    /// of that cluster's current maximum frequency.
    pub freq_scale: u32,
}

/// No fas block (legacy config) keeps the engine off until the template that
/// ships it is applied.
impl Default for FasMode {
    fn default() -> Self {
        Self { ceiling: 0, freq_scale: 0 }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FasConfig {
    pub enabled: bool,
    /// Suspend a running fas-rs (Scene's FAS) so two frame schedulers never
    /// fight over the same nodes; resumed when this engine stops.
    pub override_scene: bool,
    pub step_ms: u64,
    pub sample_ms: u64,
    pub modes: BTreeMap<String, FasMode>,
}

impl Default for FasConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            override_scene: true,
            step_ms: 200,
            sample_ms: 1000,
            modes: BTreeMap::new(),
        }
    }
}

/// 信息流视频应用锁帧：前台命中 packages 时把屏幕刷新率限制到 fps Hz。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FeedVideoConfig {
    pub enabled: bool,
    pub fps: u32,
    pub packages: Vec<String>,
}

impl Default for FeedVideoConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            fps: 30,
            packages: [
                "com.ss.android.ugc.aweme",
                "com.smile.gifmaker",
                "com.xingin.xhs",
                "tv.danmaku.bili",
            ]
            .iter()
            .map(|s| s.to_string())
            .collect(),
        }
    }
}

/// devfreq GPU 档位包络：按档位写 GPU min_freq/max_freq（可用频表归一化）。
/// 0% = 不限制（写设备频表端点）。设备缺少可调频 GPU 节点时运行时降级：
/// 记一条日志并保持 GPU 完全由内核管理，不影响其它功能。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GpuDevfreqMode {
    /// 最低频率下限，占频表最高 OPP 的百分比；0 = 物理最低。
    pub min: u32,
    /// 最高频率上限，占频表最高 OPP 的百分比；0 = 物理最高。
    pub max: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GpuDevfreq {
    pub enabled: bool,
    pub modes: BTreeMap<String, GpuDevfreqMode>,
}

#[derive(Clone, Debug)]
pub struct Functions {
    pub node_watchdog: bool,
    pub cpuset: Cpuset,
    pub cpuctl: Cpuctl,
    pub input_boost: InputBoost,
    pub launch_boost: LaunchBoost,
    pub disable_gpu_boost: bool,
    pub scheduler: SchedulerConfig,
    pub foreground_monitor: ForegroundMonitorConfig,
    pub perf_service_lock: PerfServiceLock,
    pub fas: FasConfig,
    pub feed_video: FeedVideoConfig,
    pub gpu_devfreq: GpuDevfreq,
}

#[derive(Clone, Debug, Default)]
pub struct ClusterProfile {
    pub min_freq: String,
    pub max_freq: String,
    pub governor: String,
    pub sched_params: Vec<(String, String)>,
}

#[derive(Clone, Debug)]
pub struct ModeProfile {
    pub clusters: [ClusterProfile; 4],
    pub online: [bool; 8],
}

#[derive(Clone, Debug)]
pub struct Config {
    pub meta: Meta,
    pub policy: [i32; 4],
    pub functions: Functions,
    pub modes: BTreeMap<String, ModeProfile>,
}

impl Config {
    pub fn load(path: &Path) -> Result<Self> {
        let text = util::read_text(path)?;
        Self::from_text(&text)
    }

    pub fn from_text(text: &str) -> Result<Self> {
        let root = json::parse(text)?;
        if root.get("schema").is_ok() {
            return crate::config_v2::parse(&root);
        }
        let meta_node = root.get("meta")?;
        let identity_keys = ["module_id", "soc_id", "profile_id", "profile_schema"];
        let profile_soc = if identity_keys
            .iter()
            .any(|key| optional_field(meta_node, key).is_some())
        {
            if string(meta_node, "module_id")? != "NovaSched_Zen_Edition" {
                return Err("配置 module_id 不属于 NovaSched Zen Edition".into());
            }
            let id = string(meta_node, "soc_id")?;
            let soc =
                crate::soc::Soc::from_id(&id).ok_or_else(|| format!("配置 soc_id 不支持: {id}"))?;
            if integer(meta_node, "profile_schema")? != 1
                || string(meta_node, "profile_id")?
                    != format!("novasched.{}", soc.id().to_ascii_lowercase())
            {
                return Err("配置 profile_id/profile_schema 与处理器不匹配".into());
            }
            Some(soc)
        } else {
            None
        };
        let meta = Meta {
            name: string(meta_node, "name")?,
            version: integer(meta_node, "version")?,
            author: string(meta_node, "author")?,
            loglevel: string(meta_node, "loglevel")?,
            profile_soc,
        };
        if meta.name.trim().is_empty() || meta.author.trim().is_empty() {
            return Err("meta.name/author 不得为空".to_string());
        }

        let policy_node = root.get("Policy")?;
        let mut policy = [-1i32; 4];
        for (index, slot) in policy.iter_mut().enumerate() {
            let value = integer(policy_node, &format!("c{index}"))?;
            if !(-1..=255).contains(&value) {
                return Err(format!("Policy.c{index} 越界: {value}"));
            }
            *slot = if value == 255 { -1 } else { value as i32 };
        }
        if policy.iter().all(|value| *value < 0) {
            return Err("Policy 未启用任何 CPU 簇".into());
        }

        let function = root.get("Function")?;
        let cpuset_node = function.get("Cpuset")?;
        let cpuset = Cpuset {
            enable: boolean(cpuset_node, "enable")?,
            top_app: cpu_list(cpuset_node, "top_app")?,
            foreground: cpu_list(cpuset_node, "foreground")?,
            restricted: cpu_list(cpuset_node, "restricted")?,
            system_background: cpu_list(cpuset_node, "system_background")?,
            background: cpu_list(cpuset_node, "background")?,
            app_rule: None,
            modes: BTreeMap::new(),
        };
        let launch_node = function.get("LaunchBoost")?;
        let rate = integer(launch_node, "boost_rate_limit_ms")?;
        if !(0..=10_000).contains(&rate) {
            return Err(format!("LaunchBoost.boost_rate_limit_ms 越界: {rate}"));
        }
        let boost_node = launch_node.get("BoostFreq")?;
        let mut boost_frequencies = four_strings(boost_node)?;
        for index in 0..4 {
            if policy[index] < 0 && boost_frequencies[index].is_empty() {
                boost_frequencies[index] = "0".into();
            }
        }
        for (index, value) in boost_frequencies.iter().enumerate() {
            validate_frequency(value, "LaunchBoost", index, "BoostFreq")?;
        }
        let launch_boost = LaunchBoost {
            enable: boolean(launch_node, "enable")?,
            rate_limit_ms: rate as u64,
            frequencies: boost_frequencies,
        };
        let disable_gpu_boost = boolean(function.get("DisableGpuBoost")?, "enable")?;
        let sched = function.get("Scheduler")?;
        // 与 config_v2 的 clamp 校验保持一致：非法值必须在加载期报错，
        // 否则每次写入 uclamp 都失败，守护进程陷入无意义的 degraded 重试。
        let util_clamp_min = numeric_string(sched, "sched_util_clamp_min")?;
        let util_clamp_max = numeric_string(sched, "sched_util_clamp_max")?;
        if util_clamp_min.parse::<u64>().unwrap_or(u64::MAX) > 1024
            || util_clamp_max.parse::<u64>().unwrap_or(u64::MAX) > 1024
        {
            return Err("sched_util_clamp_min/max 必须在 0..1024".into());
        }
        if util_clamp_min.parse::<u64>().unwrap_or(0) > util_clamp_max.parse::<u64>().unwrap_or(0) {
            return Err("sched_util_clamp_min 不能大于 sched_util_clamp_max".into());
        }
        let scheduler = SchedulerConfig {
            enable: boolean(sched, "enable")?,
            energy_aware: boolean(sched, "sched_energy_aware")?,
            schedstats: boolean(sched, "sched_schedstats")?,
            latency_ns: numeric_string(sched, "sched_latency_ns")?,
            migration_cost_ns: numeric_string(sched, "sched_migration_cost_ns")?,
            min_granularity_ns: numeric_string(sched, "sched_min_granularity_ns")?,
            wakeup_granularity_ns: numeric_string(sched, "sched_wakeup_granularity_ns")?,
            nr_migrate: numeric_string(sched, "sched_nr_migrate")?,
            util_clamp_min,
            util_clamp_max,
        };
        let foreground_monitor = optional_field(function, "ForegroundMonitor")
            .map(parse_foreground_monitor)
            .transpose()?
            .unwrap_or_default();
        let perf_service_lock = optional_field(function, "PerfServiceLock")
            .map(parse_perf_service_lock)
            .transpose()?
            .unwrap_or_default();

        let node_watchdog = optional_field(function, "NodeWatchdog")
            .map(|node| boolean(node, "enable"))
            .transpose()?
            .unwrap_or(true);

        let switch = root.get("Switch")?;
        let mut modes = BTreeMap::new();
        for mode in MODES {
            let node = switch.get(mode)?;
            let mut min = four_strings(node.get("MinFreq")?)?;
            let mut max = four_strings(node.get("MaxFreq")?)?;
            let governors = four_strings(node.get("governor")?)?;
            let sched_params = node.get("SchedParam")?;
            let mut clusters: [ClusterProfile; 4] =
                std::array::from_fn(|_| ClusterProfile::default());
            for index in 0..4 {
                if policy[index] < 0 {
                    if min[index].is_empty() {
                        min[index] = "0".into();
                    }
                    if max[index].is_empty() {
                        max[index] = "0".into();
                    }
                }
                validate_frequency(&min[index], mode, index, "MinFreq")?;
                validate_frequency(&max[index], mode, index, "MaxFreq")?;
                if policy[index] >= 0 || !governors[index].is_empty() {
                    validate_governor(&governors[index], mode, index)?;
                }
                let params = sched_params.get(&format!("c{index}"))?;
                let mut values = Vec::new();
                for number in 1..=12 {
                    let name = string(params, &format!("Path{number}"))?;
                    let value = string(params, &format!("value{number}"))?;
                    if name.is_empty() {
                        continue;
                    }
                    if name.contains('/') || name == "." || name == ".." {
                        return Err(format!("{mode}.SchedParam.c{index}.Path{number} 非法"));
                    }
                    values.push((name, value));
                }
                clusters[index] = ClusterProfile {
                    min_freq: min[index].clone(),
                    max_freq: max[index].clone(),
                    governor: governors[index].clone(),
                    sched_params: values,
                };
            }
            let online_node = node.get("CoreOnline")?;
            let mut online = [true; 8];
            for (index, slot) in online.iter_mut().enumerate() {
                let value = integer(online_node, &format!("Core{index}"))?;
                if value != 0 && value != 1 {
                    return Err(format!("{mode}.CoreOnline.Core{index} 只能为 0 或 1"));
                }
                *slot = value == 1;
            }
            modes.insert(mode.to_string(), ModeProfile { clusters, online });
        }
        Ok(Self {
            meta,
            policy,
            functions: Functions {
                node_watchdog,
                cpuset,
                cpuctl: Cpuctl::default(),
                input_boost: InputBoost::default(),
                launch_boost,
                disable_gpu_boost,
                scheduler,
                foreground_monitor,
                perf_service_lock,
                fas: FasConfig::default(),
            feed_video: FeedVideoConfig::default(),
            gpu_devfreq: GpuDevfreq::default(),
            },
            modes,
        })
    }

    pub fn profile(&self, mode: &str) -> Result<&ModeProfile> {
        self.modes
            .get(mode)
            .ok_or_else(|| format!("不支持的情景模式: {mode}"))
    }
}

fn string(node: &Value, key: &str) -> Result<String> {
    Ok(node.get(key)?.as_str()?.to_string())
}

fn integer(node: &Value, key: &str) -> Result<i64> {
    node.get(key)?.as_i64()
}

fn boolean(node: &Value, key: &str) -> Result<bool> {
    node.get(key)?.as_bool()
}

fn optional_field<'a>(node: &'a Value, key: &str) -> Option<&'a Value> {
    match node {
        Value::Object(values) => values.get(key),
        _ => None,
    }
}

fn parse_foreground_monitor(node: &Value) -> Result<ForegroundMonitorConfig> {
    let Some(Value::Array(values)) = optional_field(node, "IgnorePackages") else {
        return Err("ForegroundMonitor.IgnorePackages 必须为字符串数组".to_string());
    };
    let mut ignored_packages = BTreeSet::new();
    for value in values {
        let package = value.as_str()?;
        if !util::valid_package(package) {
            return Err(format!(
                "ForegroundMonitor.IgnorePackages 包名非法: {package}"
            ));
        }
        ignored_packages.insert(package.to_string());
    }
    Ok(ForegroundMonitorConfig { ignored_packages })
}

fn parse_perf_service_lock(node: &Value) -> Result<PerfServiceLock> {
    let Some(Value::Array(values)) = optional_field(node, "Services") else {
        return Err("PerfServiceLock.Services 必须为字符串数组".into());
    };
    if values.len() > 32 {
        return Err("PerfServiceLock.Services 最多允许 32 项".into());
    }
    let mut services = Vec::new();
    for value in values {
        let name = value.as_str()?;
        if name.is_empty()
            || name.len() > 91
            || name.starts_with('-')
            || !name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
        {
            return Err(format!("PerfServiceLock 服务名非法: {name}"));
        }
        if !services.iter().any(|s| s == name) {
            services.push(name.to_string());
        }
    }
    Ok(PerfServiceLock {
        enable: boolean(node, "enable")?,
        services,
    })
}

fn four_strings(node: &Value) -> Result<[String; 4]> {
    let values = [
        string(node, "c0")?,
        string(node, "c1")?,
        string(node, "c2")?,
        string(node, "c3")?,
    ];
    Ok(values)
}

fn numeric_string(node: &Value, key: &str) -> Result<String> {
    let value = string(node, key)?;
    value
        .parse::<u64>()
        .map_err(|_| format!("{key} 不是非负整数: {value}"))?;
    Ok(value)
}

fn cpu_list(node: &Value, key: &str) -> Result<String> {
    let value = string(node, key)?;
    if value.is_empty()
        || value
            .bytes()
            .any(|b| !b.is_ascii_digit() && b != b',' && b != b'-')
    {
        return Err(format!("{key} CPU 列表非法: {value}"));
    }
    crate::hardware::parse_cpu_list(&value)?;
    Ok(value)
}

fn validate_frequency(value: &str, mode: &str, cluster: usize, field: &str) -> Result<()> {
    value
        .parse::<u64>()
        .map_err(|_| format!("{mode}.{field}.c{cluster} 不是频率整数: {value}"))?;
    Ok(())
}

fn validate_governor(value: &str, mode: &str, cluster: usize) -> Result<()> {
    if value.is_empty()
        || value
            .bytes()
            .any(|b| !(b.is_ascii_alphanumeric() || b == b'_' || b == b'-'))
    {
        return Err(format!("{mode}.governor.c{cluster} 非法: {value}"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn all_defaults_have_owned_identity_and_valid_inactive_slots() {
        for soc in crate::soc::SUPPORTED {
            let path = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../module-template/config")
                .join(soc.file());
            let text = std::fs::read_to_string(path).unwrap();
            let config = Config::from_text(&text).unwrap();
            assert_eq!(config.policy, soc.anchors());
            assert_eq!(config.meta.profile_soc, Some(soc));
            assert_eq!(config.meta.author, "ZenJooo");
            assert_eq!(config.meta.version, PROFILE_VERSION);
            assert!(config.functions.node_watchdog);
            assert!(!config.functions.launch_boost.enable);
            assert!(Config::from_text(&text.replace("NovaSched_Zen_Edition", "wrong")).is_err());
            assert!(Config::from_text(&text.replace("novasched/2", "novasched/9")).is_err());
        }
    }
    #[test]
    fn own_format_rejects_bad_cpu_ranges_and_service_names() {
        let text = include_str!("../../module-template/config/SM8650.json");
        assert!(Config::from_text(&text.replace("\"0-7\"", "\"0-8\"")).is_err());
        assert!(
            Config::from_text(&text.replace("\"services\": []", "\"services\": [\"-bad\"]"))
                .is_err()
        );
    }
}
