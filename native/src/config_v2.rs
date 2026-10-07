use crate::config::*;
use crate::json::Value;
use crate::soc::Soc;
use crate::util::{self, Result};
use std::collections::{BTreeMap, BTreeSet};

pub const SCHEMA: &str = "novasched/2";

fn string(node: &Value, key: &str) -> Result<String> {
    Ok(node.get(key)?.as_str()?.to_owned())
}
fn flag(node: &Value, key: &str) -> Result<bool> {
    node.get(key)?.as_bool()
}
fn field<'a>(node: &'a Value, key: &str) -> Option<&'a Value> {
    match node {
        Value::Object(map) => map.get(key),
        _ => None,
    }
}
fn array(node: &Value, len: usize) -> Result<&[Value]> {
    match node {
        Value::Array(values) if values.len() == len => Ok(values),
        _ => Err(format!("配置数组必须包含 {len} 项")),
    }
}
fn number(node: &Value, key: &str) -> Result<String> {
    let value = string(node, key)?;
    value
        .parse::<u64>()
        .map_err(|_| format!("{key} 必须为非负整数字符串"))?;
    Ok(value)
}
fn clamp(node: &Value, key: &str) -> Result<String> {
    let value = number(node, key)?;
    if value.parse::<u64>().map_err(|e| e.to_string())? > 1024 {
        return Err(format!("{key} 必须在 0..1024"));
    }
    Ok(value)
}
fn cpus(node: &Value, key: &str) -> Result<String> {
    let value = string(node, key)?;
    crate::hardware::parse_cpu_list(&value)?;
    Ok(value)
}
pub fn validate_frequency(value: &str) -> Result<()> {
    if let Some(percent) = value.strip_suffix('%') {
        let n = percent
            .parse::<u32>()
            .map_err(|_| format!("频率百分比非法: {value}"))?;
        if n > 100 {
            return Err(format!("频率百分比超出 0..100: {value}"));
        }
    } else {
        value
            .parse::<u64>()
            .map_err(|_| format!("频率必须为 kHz 整数或百分比: {value}"))?;
    }
    Ok(())
}
fn frequencies(node: &Value, key: &str) -> Result<[String; 4]> {
    let values = array(node.get(key)?, 4)?;
    let mut out = std::array::from_fn(|_| String::new());
    for (index, value) in values.iter().enumerate() {
        out[index] = value.as_str()?.to_owned();
        validate_frequency(&out[index])?;
    }
    Ok(out)
}
fn strings(node: &Value) -> Result<Vec<String>> {
    let Value::Array(values) = node else {
        return Err("配置字段必须为字符串数组".into());
    };
    values
        .iter()
        .map(|value| Ok(value.as_str()?.to_owned()))
        .collect()
}
/// Per-mode cpuset overrides: {"fast": {"top_app": "2-7", "foreground": "2-7"}}.
/// Mode names must be the four scheduler modes; groups are optional.
fn parse_cpuset_modes(cpuset: &Value) -> Result<BTreeMap<String, CpusetMode>> {
    let mut modes = BTreeMap::new();
    let Some(Value::Object(entries)) = field(cpuset, "modes") else {
        return Ok(modes);
    };
    for (name, value) in entries {
        if !crate::config::MODES.contains(&name.as_str()) {
            return Err(format!("cpuset.modes 含未知档位: {name}"));
        }
        let top_app = field(value, "top_app")
            .map(|_| cpus(value, "top_app"))
            .transpose()?;
        let foreground = field(value, "foreground")
            .map(|_| cpus(value, "foreground"))
            .transpose()?;
        modes.insert(name.clone(), CpusetMode { top_app, foreground });
    }
    Ok(modes)
}

/// Optional named cpuset override block (currently the per-app game rule).
fn parse_cpuset_mode_optional(cpuset: &Value, key: &str) -> Result<Option<CpusetMode>> {
    let Some(value) = field(cpuset, key) else {
        return Ok(None);
    };
    let top_app = field(value, "top_app")
        .map(|_| cpus(value, "top_app"))
        .transpose()?;
    let foreground = field(value, "foreground")
        .map(|_| cpus(value, "foreground"))
        .transpose()?;
    if top_app.is_none() && foreground.is_none() {
        return Err(format!("cpuset.{key} 不能为空"));
    }
    Ok(Some(CpusetMode { top_app, foreground }))
}

/// cpu.uclamp values accept "max" or a 0..=100 percent. The strict variant
/// rejects "max": a max util floor on any group is the reverted power bomb.
fn clamp_percent_strict(node: &Value, key: &str) -> Result<String> {
    let value = clamp_percent(node, key)?;
    if value == "max" {
        return Err(format!("{key} 不允许 max"));
    }
    Ok(value)
}

/// cpu.uclamp values accept "max" or a 0..=100 percent.
fn clamp_percent(node: &Value, key: &str) -> Result<String> {
    let value = string(node, key)?;
    if value == "max" {
        return Ok(value);
    }
    let n = value
        .parse::<u32>()
        .map_err(|_| format!("{key} 必须为 0..100 或 max: {value}"))?;
    if n > 100 {
        return Err(format!("{key} 必须为 0..100 或 max: {value}"));
    }
    Ok(value)
}

/// Per-mode cpuctl clamps: {"fast": {"top_app_min": "30"},
/// "powersave": {"background_max": "60"}}.
fn parse_cpuctl_modes(cpuctl: &Value) -> Result<BTreeMap<String, CpuctlMode>> {
    let mut modes = BTreeMap::new();
    let Some(Value::Object(entries)) = field(cpuctl, "modes") else {
        return Ok(modes);
    };
    for (name, value) in entries {
        if !crate::config::MODES.contains(&name.as_str()) {
            return Err(format!("cpuctl.modes 含未知档位: {name}"));
        }
        let top_app_min = field(value, "top_app_min")
            .map(|_| clamp_percent(value, "top_app_min"))
            .transpose()?;
        let top_app_min_rule = field(value, "top_app_min_rule")
            .map(|_| clamp_percent_strict(value, "top_app_min_rule"))
            .transpose()?;
        let background_max = field(value, "background_max")
            .map(|_| clamp_percent(value, "background_max"))
            .transpose()?;
        modes.insert(
            name.clone(),
            CpuctlMode {
                top_app_min,
                top_app_min_rule,
                background_max,
            },
        );
    }
    Ok(modes)
}

fn limits(node: &Value) -> Result<ExtremePowerSave> {
    let gpu = field(node, "gpu_max_pwrlevel")
        .map(|value| -> Result<u32> {
            let n = value.as_i64()?;
            if !(0..=255).contains(&n) {
                return Err("gpu_max_pwrlevel 必须在 0..255".into());
            }
            Ok(n as u32)
        })
        .transpose()?;
    Ok(ExtremePowerSave {
        enabled_by_default: flag(node, "enabled")?,
        max_frequencies: frequencies(node, "max")?,
        cpuset_top_app: field(node, "top_app")
            .map(|_| cpus(node, "top_app"))
            .transpose()?,
        cpuset_foreground: field(node, "foreground")
            .map(|_| cpus(node, "foreground"))
            .transpose()?,
        util_clamp_max: field(node, "uclamp_max")
            .map(|_| clamp(node, "uclamp_max"))
            .transpose()?,
        gpu_max_pwrlevel: gpu,
    })
}

pub fn parse(root: &Value) -> Result<Config> {
    if string(root, "schema")? != SCHEMA || string(root, "module")? != "NovaSched_Zen_Edition" {
        return Err("配置格式或模块身份不属于 NovaSched".into());
    }
    let meta = root.get("meta")?;
    let id = string(meta, "soc")?;
    let soc = Soc::from_id(&id).ok_or_else(|| format!("配置处理器不支持: {id}"))?;
    let meta = Meta {
        name: string(meta, "name")?,
        author: string(meta, "author")?,
        version: meta.get("version")?.as_i64()?,
        loglevel: string(meta, "loglevel")?,
        profile_soc: Some(soc),
    };
    if meta.name.trim().is_empty() || meta.author.trim().is_empty() {
        return Err("配置名称和作者不得为空".into());
    }
    let mut policy = [-1; 4];
    for (index, value) in array(root.get("policies")?, 4)?.iter().enumerate() {
        let id = value.as_i64()?;
        if !(-1..=255).contains(&id) {
            return Err("CPU policy 编号越界".into());
        }
        policy[index] = if id == 255 { -1 } else { id as i32 };
    }
    if policy.iter().all(|p| *p < 0) {
        return Err("配置未启用任何 CPU 簇".into());
    }
    let features = root.get("features")?;
    let c = features.get("cpuset")?;
    let cpuset = Cpuset {
        enable: flag(c, "enabled")?,
        top_app: cpus(c, "top_app")?,
        foreground: cpus(c, "foreground")?,
        restricted: cpus(c, "restricted")?,
        system_background: cpus(c, "system_background")?,
        background: cpus(c, "background")?,
        modes: parse_cpuset_modes(c)?,
        app_rule: parse_cpuset_mode_optional(c, "app_rule")?,
    };
    let l = features.get("launch_boost")?;
    let rate = l.get("rate_limit_ms")?.as_i64()?;
    if !(0..=10000).contains(&rate) {
        return Err("启动加速间隔必须在 0..10000 ms".into());
    }
    let launch_boost = LaunchBoost {
        enable: flag(l, "enabled")?,
        rate_limit_ms: rate as u64,
        frequencies: frequencies(l, "min")?,
    };
    let s = features.get("scheduler")?;
    let scheduler = SchedulerConfig {
        enable: flag(s, "enabled")?,
        energy_aware: flag(s, "energy_aware")?,
        schedstats: flag(s, "schedstats")?,
        latency_ns: number(s, "latency_ns")?,
        migration_cost_ns: number(s, "migration_cost_ns")?,
        min_granularity_ns: number(s, "min_granularity_ns")?,
        wakeup_granularity_ns: number(s, "wakeup_granularity_ns")?,
        nr_migrate: number(s, "nr_migrate")?,
        util_clamp_min: clamp(s, "util_clamp_min")?,
        util_clamp_max: clamp(s, "util_clamp_max")?,
    };
    let mut ignored_packages = BTreeSet::new();
    for package in strings(features.get("foreground_ignore")?)? {
        if !util::valid_package(&package) {
            return Err(format!("忽略列表包名非法: {package}"));
        }
        ignored_packages.insert(package);
    }
    let extreme_powersave = limits(features.get("extreme")?)?;
    // cpuctl is optional so older configs keep parsing; absent means the
    // feature stays off until a template that ships it is applied.
    let cpuctl = match field(features, "cpuctl") {
        Some(block) => {
            let enable = flag(block, "enabled")?;
            let modes = parse_cpuctl_modes(block)?;
            if enable && modes.is_empty() {
                return Err("cpuctl 已启用但未提供任何档位钳制".into());
            }
            Cpuctl { enable, modes }
        }
        None => Cpuctl::default(),
    };
    // Input boost is optional so older configs keep parsing; absent means
    // the touch-pulse feature stays off.
    let input_boost = match field(features, "input_boost") {
        Some(block) => {
            let enabled = flag(block, "enabled")?;
            let top_app_min = clamp_percent_strict(block, "top_app_min")?;
            let rate = number(block, "duration_ms")?;
            if !(50..=5000).contains(&rate.parse::<u64>().map_err(|e| e.to_string())?) {
                return Err("输入突频持续时间必须在 50..5000 ms".into());
            }
            InputBoost {
                enabled,
                top_app_min,
                duration_ms: rate.parse::<u64>().map_err(|e| e.to_string())?,
            }
        }
        None => InputBoost::default(),
    };
    let smooth_powersave = field(features, "smooth")
        .map(|s| -> Result<SmoothPowerSave> {
            let limits = limits(s)?;
            let minimum = clamp(s, "uclamp_min_limit")?;
            let maximum = limits
                .util_clamp_max
                .as_deref()
                .ok_or("流畅省电缺少 uclamp_max")?;
            if minimum.parse::<u32>().map_err(|e| e.to_string())?
                > maximum.parse::<u32>().map_err(|e| e.to_string())?
            {
                return Err("流畅省电的 MIN 请求范围超过 MAX 范围".into());
            }
            let rate = number(s, "up_rate_limit_us")?;
            if rate.parse::<u64>().map_err(|e| e.to_string())? > 1000000 {
                return Err("调速响应间隔超过 1 秒".into());
            }
            Ok(SmoothPowerSave {
                limits,
                util_clamp_min_limit: minimum,
                up_rate_limit_us: rate,
                restore_stock_response: flag(s, "restore_stock_response")?,
            })
        })
        .transpose()?;
    let p = features.get("perf_lock")?;
    let services = strings(p.get("services")?)?;
    if services.len() > 32
        || services.iter().any(|name| {
            name.is_empty()
                || name.len() > 91
                || name.starts_with('-')
                || !name
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
        })
    {
        return Err("性能服务列表包含非法名称".into());
    }
    let mut modes = BTreeMap::new();
    for mode in MODES {
        let m = root.get("modes")?.get(mode)?;
        let min = frequencies(m, "min")?;
        let max = frequencies(m, "max")?;
        let governors = array(m.get("governors")?, 4)?;
        let params = array(m.get("params")?, 4)?;
        let mut clusters = std::array::from_fn(|_| ClusterProfile::default());
        for index in 0..4 {
            let governor = governors[index].as_str()?;
            if (policy[index] >= 0 && governor.is_empty())
                || governor
                    .bytes()
                    .any(|b| !b.is_ascii_alphanumeric() && b != b'_' && b != b'-')
            {
                return Err(format!("{mode} 的调速器名称非法"));
            }
            let Value::Object(values) = &params[index] else {
                return Err("调速参数必须为对象".into());
            };
            let mut sched_params = Vec::new();
            for (key, value) in values {
                if key.is_empty()
                    || key == "."
                    || key == ".."
                    || key
                        .bytes()
                        .any(|b| !b.is_ascii_alphanumeric() && b != b'_' && b != b'-')
                {
                    return Err(format!("调速参数名称非法: {key}"));
                }
                sched_params.push((key.clone(), value.as_str()?.to_owned()));
            }
            clusters[index] = ClusterProfile {
                min_freq: min[index].clone(),
                max_freq: max[index].clone(),
                governor: governor.into(),
                sched_params,
            };
        }
        let mut online = [true; 8];
        for (index, value) in array(m.get("online")?, 8)?.iter().enumerate() {
            online[index] = value.as_bool()?;
        }
        modes.insert(mode.into(), ModeProfile { clusters, online });
    }
    Ok(Config {
        meta,
        policy,
        modes,
        functions: Functions {
            cpuset,
            cpuctl,
            input_boost,
            launch_boost,
            scheduler,
            node_watchdog: flag(features, "node_watchdog")?,
            disable_gpu_boost: flag(features, "disable_gpu_boost")?,
            foreground_monitor: ForegroundMonitorConfig { ignored_packages },
            extreme_powersave,
            smooth_powersave,
            perf_service_lock: PerfServiceLock {
                enable: flag(p, "enabled")?,
                services,
            },
        },
    })
}

fn object<const N: usize>(items: [(&str, Value); N]) -> Value {
    Value::Object(items.into_iter().map(|(k, v)| (k.to_owned(), v)).collect())
}
fn text(value: &str) -> Value {
    Value::String(value.into())
}
fn list<'a>(items: impl IntoIterator<Item = &'a String>) -> Value {
    Value::Array(items.into_iter().map(|v| text(v)).collect())
}
fn encode_limits(limits: &ExtremePowerSave) -> Value {
    let mut out = object([
        ("enabled", Value::Bool(limits.enabled_by_default)),
        ("max", list(&limits.max_frequencies)),
    ]);
    if let Value::Object(map) = &mut out {
        for (key, value) in [
            ("top_app", &limits.cpuset_top_app),
            ("foreground", &limits.cpuset_foreground),
            ("uclamp_max", &limits.util_clamp_max),
        ] {
            if let Some(value) = value {
                map.insert(key.into(), text(value));
            }
        }
        if let Some(value) = limits.gpu_max_pwrlevel {
            map.insert("gpu_max_pwrlevel".into(), Value::Number(value as i64));
        }
    }
    out
}

pub fn encode(config: &Config, soc: Soc) -> Value {
    let f = &config.functions;
    let c = &f.cpuset;
    let s = &f.scheduler;
    let scheduler = object([
        ("enabled", Value::Bool(s.enable)),
        ("energy_aware", Value::Bool(s.energy_aware)),
        ("schedstats", Value::Bool(s.schedstats)),
        ("latency_ns", text(&s.latency_ns)),
        ("migration_cost_ns", text(&s.migration_cost_ns)),
        ("min_granularity_ns", text(&s.min_granularity_ns)),
        ("wakeup_granularity_ns", text(&s.wakeup_granularity_ns)),
        ("nr_migrate", text(&s.nr_migrate)),
        ("util_clamp_min", text(&s.util_clamp_min)),
        ("util_clamp_max", text(&s.util_clamp_max)),
    ]);
    let mut cpuset = object([
        ("enabled", Value::Bool(c.enable)),
        ("top_app", text(&c.top_app)),
        ("foreground", text(&c.foreground)),
        ("restricted", text(&c.restricted)),
        ("system_background", text(&c.system_background)),
        ("background", text(&c.background)),
    ]);
    if !c.modes.is_empty() {
        if let Value::Object(map) = &mut cpuset {
            map.insert(
                "modes".into(),
                Value::Object(
                    c.modes
                        .iter()
                        .map(|(mode, entry)| {
                            let mut entry_out = object([]);
                            if let Value::Object(fields) = &mut entry_out {
                                if let Some(value) = &entry.top_app {
                                    fields.insert("top_app".into(), text(value));
                                }
                                if let Some(value) = &entry.foreground {
                                    fields.insert("foreground".into(), text(value));
                                }
                            }
                            (mode.clone(), entry_out)
                        })
                        .collect(),
                ),
            );
        }
    }
    let mut cpuctl_out = object([
        ("enabled", Value::Bool(f.cpuctl.enable)),
        (
            "modes",
            Value::Object(
                f.cpuctl
                    .modes
                    .iter()
                    .map(|(mode, entry)| {
                        let mut entry_out = object([]);
                        if let Value::Object(fields) = &mut entry_out {
                            if let Some(value) = &entry.top_app_min {
                                fields.insert("top_app_min".into(), text(value));
                            }
                            if let Some(value) = &entry.background_max {
                                fields.insert("background_max".into(), text(value));
                            }
                        }
                        (mode.clone(), entry_out)
                    })
                    .collect(),
            ),
        ),
    ]);
    if let Value::Object(map) = &mut cpuctl_out {
        if f.cpuctl.modes.is_empty() {
            map.remove("modes");
        }
    }
    let mut features = object([
        ("node_watchdog", Value::Bool(f.node_watchdog)),
        ("cpuset", cpuset),
        ("cpuctl", cpuctl_out),
        (
            "launch_boost",
            object([
                ("enabled", Value::Bool(f.launch_boost.enable)),
                (
                    "rate_limit_ms",
                    Value::Number(f.launch_boost.rate_limit_ms as i64),
                ),
                ("min", list(&f.launch_boost.frequencies)),
            ]),
        ),
        ("scheduler", scheduler),
        ("disable_gpu_boost", Value::Bool(f.disable_gpu_boost)),
        (
            "foreground_ignore",
            list(&f.foreground_monitor.ignored_packages),
        ),
        ("extreme", encode_limits(&f.extreme_powersave)),
        (
            "perf_lock",
            object([
                ("enabled", Value::Bool(f.perf_service_lock.enable)),
                ("services", list(&f.perf_service_lock.services)),
            ]),
        ),
    ]);
    if let Some(s) = &f.smooth_powersave {
        let mut smooth = encode_limits(&s.limits);
        if let Value::Object(map) = &mut smooth {
            map.insert("uclamp_min_limit".into(), text(&s.util_clamp_min_limit));
            map.insert("up_rate_limit_us".into(), text(&s.up_rate_limit_us));
            map.insert(
                "restore_stock_response".into(),
                Value::Bool(s.restore_stock_response),
            );
        }
        if let Value::Object(map) = &mut features {
            map.insert("smooth".into(), smooth);
        }
    }
    let modes = Value::Object(
        config
            .modes
            .iter()
            .map(|(mode, p)| {
                (
                    mode.clone(),
                    object([
                        ("min", list(p.clusters.iter().map(|c| &c.min_freq))),
                        ("max", list(p.clusters.iter().map(|c| &c.max_freq))),
                        ("governors", list(p.clusters.iter().map(|c| &c.governor))),
                        (
                            "params",
                            Value::Array(
                                p.clusters
                                    .iter()
                                    .map(|c| {
                                        Value::Object(
                                            c.sched_params
                                                .iter()
                                                .map(|(k, v)| (k.clone(), text(v)))
                                                .collect(),
                                        )
                                    })
                                    .collect(),
                            ),
                        ),
                        (
                            "online",
                            Value::Array(p.online.iter().map(|v| Value::Bool(*v)).collect()),
                        ),
                    ]),
                )
            })
            .collect(),
    );
    object([
        ("schema", text(SCHEMA)),
        ("module", text("NovaSched_Zen_Edition")),
        (
            "meta",
            object([
                ("name", text("NovaSched Zen Edition")),
                ("author", text("ZenJooo")),
                ("version", Value::Number(crate::config::PROFILE_VERSION)),
                ("soc", text(soc.id())),
                ("loglevel", text(&config.meta.loglevel)),
            ]),
        ),
        (
            "policies",
            Value::Array(
                config
                    .policy
                    .iter()
                    .map(|v| Value::Number(*v as i64))
                    .collect(),
            ),
        ),
        ("features", features),
        ("modes", modes),
    ])
}

pub fn migrate(root: &Value, config: &Config, soc: Soc) -> Result<Value> {
    if field(root, "schema").is_some() {
        let mut out = root.clone();
        if let Value::Object(map) = &mut out {
            let mut meta = root.get("meta")?.clone();
            if let Value::Object(fields) = &mut meta {
                fields.insert("name".into(), text("NovaSched Zen Edition"));
                fields.insert("author".into(), text("ZenJooo"));
                fields.insert(
                    "version".into(),
                    Value::Number(crate::config::PROFILE_VERSION),
                );
                fields.insert("soc".into(), text(soc.id()));
            }
            map.insert("meta".into(), meta);
            // Feature blocks the user's config predates: inject only missing
            // blocks from the template so hand-edits survive the upgrade.
            let template = encode(config, soc);
            if let (Value::Object(map), Value::Object(defaults)) = (&mut out, &template) {
                if let (Some(Value::Object(features)), Some(Value::Object(default_features))) = (
                    map.get_mut("features"),
                    defaults.get("features"),
                ) {
                    if !features.contains_key("cpuctl") {
                        if let Some(cpuctl) = default_features.get("cpuctl") {
                            features.insert("cpuctl".into(), cpuctl.clone());
                        }
                    }
                    if let Some(Value::Object(cpuset_block)) = features.get_mut("cpuset") {
                        if !cpuset_block.contains_key("modes") {
                            if let Some(Value::Object(default_cpuset)) =
                                default_features.get("cpuset")
                            {
                                if let Some(modes) = default_cpuset.get("modes") {
                                    cpuset_block.insert("modes".into(), modes.clone());
                                }
                            }
                        }
                    }
                }
            }
        }
        return Ok(out);
    }
    let mut out = encode(config, soc);
    if let (Value::Object(old), Value::Object(new)) = (root, &mut out) {
        let mut collisions = BTreeMap::new();
        for (key, value) in old {
            if ["meta", "Policy", "Function", "Switch"].contains(&key.as_str()) {
                continue;
            }
            if new.contains_key(key) || key == "legacy_extensions" {
                collisions.insert(key.clone(), value.clone());
            } else {
                new.insert(key.clone(), value.clone());
            }
        }
        let mut extensions = BTreeMap::new();
        if !collisions.is_empty() {
            extensions.insert("root".into(), Value::Object(collisions));
        }
        if let Some(Value::Object(functions)) = old.get("Function") {
            let extras: BTreeMap<_, _> = functions
                .iter()
                .filter(|(key, _)| {
                    ![
                        "Cpuset",
                        "LaunchBoost",
                        "DisableGpuBoost",
                        "Scheduler",
                        "ForegroundMonitor",
                        "NodeWatchdog",
                        "ExtremePowerSave",
                        "SmoothPowerSave",
                        "PerfServiceLock",
                    ]
                    .contains(&key.as_str())
                })
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            if !extras.is_empty() {
                extensions.insert("functions".into(), Value::Object(extras));
            }
        }
        if !extensions.is_empty() {
            new.insert("legacy_extensions".into(), Value::Object(extensions));
        }
    }
    Ok(out)
}

#[cfg(test)]
pub fn legacy_fixture(soc: Soc) -> String {
    let config = crate::profiles::test_config(soc);
    let f = &config.functions;
    let c = &f.cpuset;
    let s = &f.scheduler;
    let four = |values: &[String; 4]| {
        Value::Object(
            values
                .iter()
                .enumerate()
                .map(|(i, v)| (format!("c{i}"), text(v)))
                .collect(),
        )
    };
    let old_limits = |l: &ExtremePowerSave| {
        let mut out = object([
            ("enable", Value::Bool(l.enabled_by_default)),
            ("MaxFreq", four(&l.max_frequencies)),
            (
                "CpuSet",
                object([
                    (
                        "top_app",
                        text(l.cpuset_top_app.as_deref().unwrap_or("0-7")),
                    ),
                    (
                        "foreground",
                        text(l.cpuset_foreground.as_deref().unwrap_or("0-7")),
                    ),
                ]),
            ),
        ]);
        if let Value::Object(map) = &mut out {
            if let Some(v) = &l.util_clamp_max {
                map.insert("UtilClampMax".into(), text(v));
            }
            if let Some(v) = l.gpu_max_pwrlevel {
                map.insert("GpuMaxPwrLevel".into(), Value::Number(v as i64));
            }
        }
        out
    };
    let mut functions = object([
        (
            "NodeWatchdog",
            object([("enable", Value::Bool(f.node_watchdog))]),
        ),
        (
            "Cpuset",
            object([
                ("enable", Value::Bool(c.enable)),
                ("top_app", text(&c.top_app)),
                ("foreground", text(&c.foreground)),
                ("restricted", text(&c.restricted)),
                ("system_background", text(&c.system_background)),
                ("background", text(&c.background)),
            ]),
        ),
        (
            "LaunchBoost",
            object([
                ("enable", Value::Bool(f.launch_boost.enable)),
                (
                    "boost_rate_limit_ms",
                    Value::Number(f.launch_boost.rate_limit_ms as i64),
                ),
                ("BoostFreq", four(&f.launch_boost.frequencies)),
            ]),
        ),
        (
            "DisableGpuBoost",
            object([("enable", Value::Bool(f.disable_gpu_boost))]),
        ),
        (
            "Scheduler",
            object([
                ("enable", Value::Bool(s.enable)),
                ("sched_energy_aware", Value::Bool(s.energy_aware)),
                ("sched_schedstats", Value::Bool(s.schedstats)),
                ("sched_latency_ns", text(&s.latency_ns)),
                ("sched_migration_cost_ns", text(&s.migration_cost_ns)),
                ("sched_min_granularity_ns", text(&s.min_granularity_ns)),
                (
                    "sched_wakeup_granularity_ns",
                    text(&s.wakeup_granularity_ns),
                ),
                ("sched_nr_migrate", text(&s.nr_migrate)),
                ("sched_util_clamp_min", text(&s.util_clamp_min)),
                ("sched_util_clamp_max", text(&s.util_clamp_max)),
            ]),
        ),
        (
            "ForegroundMonitor",
            object([(
                "IgnorePackages",
                list(&f.foreground_monitor.ignored_packages),
            )]),
        ),
        ("ExtremePowerSave", old_limits(&f.extreme_powersave)),
        (
            "PerfServiceLock",
            object([
                ("enable", Value::Bool(f.perf_service_lock.enable)),
                ("Services", list(&f.perf_service_lock.services)),
            ]),
        ),
    ]);
    if let (Some(s), Value::Object(map)) = (&f.smooth_powersave, &mut functions) {
        let mut smooth = old_limits(&s.limits);
        if let Value::Object(fields) = &mut smooth {
            fields.insert("UtilClampMinLimit".into(), text(&s.util_clamp_min_limit));
            fields.insert("UpRateLimitUs".into(), text(&s.up_rate_limit_us));
            fields.insert(
                "RestoreStockResponse".into(),
                Value::Bool(s.restore_stock_response),
            );
        }
        map.insert("SmoothPowerSave".into(), smooth);
    }
    let switch = Value::Object(
        config
            .modes
            .iter()
            .map(|(name, p)| {
                let params = Value::Object(
                    p.clusters
                        .iter()
                        .enumerate()
                        .map(|(i, c)| {
                            let mut fields = BTreeMap::new();
                            for n in 1..=12 {
                                let pair = c.sched_params.get(n - 1);
                                fields.insert(
                                    format!("Path{n}"),
                                    text(pair.map(|v| v.0.as_str()).unwrap_or("")),
                                );
                                fields.insert(
                                    format!("value{n}"),
                                    text(pair.map(|v| v.1.as_str()).unwrap_or("0")),
                                );
                            }
                            (format!("c{i}"), Value::Object(fields))
                        })
                        .collect(),
                );
                (
                    name.clone(),
                    object([
                        (
                            "MinFreq",
                            four(&std::array::from_fn(|i| p.clusters[i].min_freq.clone())),
                        ),
                        (
                            "MaxFreq",
                            four(&std::array::from_fn(|i| p.clusters[i].max_freq.clone())),
                        ),
                        (
                            "governor",
                            four(&std::array::from_fn(|i| p.clusters[i].governor.clone())),
                        ),
                        ("SchedParam", params),
                        (
                            "CoreOnline",
                            Value::Object(
                                p.online
                                    .iter()
                                    .enumerate()
                                    .map(|(i, v)| (format!("Core{i}"), Value::Number(*v as i64)))
                                    .collect(),
                            ),
                        ),
                    ]),
                )
            })
            .collect(),
    );
    crate::json::stringify(&object([
        (
            "meta",
            object([
                ("name", text(soc.name())),
                ("version", Value::Number(1)),
                ("author", text("Previous author")),
                ("loglevel", text(&config.meta.loglevel)),
            ]),
        ),
        (
            "Policy",
            Value::Object(
                config
                    .policy
                    .iter()
                    .enumerate()
                    .map(|(i, v)| (format!("c{i}"), Value::Number(*v as i64)))
                    .collect(),
            ),
        ),
        ("Function", functions),
        ("Switch", switch),
    ]))
    .unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn migration_keeps_user_extensions_that_collide_with_new_field_names() {
        let text = legacy_fixture(Soc::Gen3);
        let mut root = crate::json::parse(&text).unwrap();
        if let Value::Object(map) = &mut root {
            map.insert("features".into(), Value::String("user data".into()));
            map.insert(
                "legacy_extensions".into(),
                Value::String("existing extension".into()),
            );
        }
        let config = Config::from_text(&crate::json::stringify(&root).unwrap()).unwrap();
        let after = migrate(&root, &config, Soc::Gen3).unwrap();
        let extras = after.get("legacy_extensions").unwrap().get("root").unwrap();
        assert_eq!(
            extras.get("features").unwrap().as_str().unwrap(),
            "user data"
        );
        assert_eq!(
            extras.get("legacy_extensions").unwrap().as_str().unwrap(),
            "existing extension"
        );
        assert!(parse(&after).is_ok());
    }
    #[test]
    fn own_defaults_round_trip_and_reject_invalid_limits() {
        for soc in crate::soc::SUPPORTED {
            let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../module-template/config")
                .join(soc.file());
            let config = Config::load(&path).unwrap();
            let encoded = encode(&config, soc);
            assert_eq!(
                format!("{:?}", parse(&encoded).unwrap()),
                format!("{config:?}")
            );
        }
        for value in ["101%", "-1%", "1.5%", "", "300MHz"] {
            assert!(validate_frequency(value).is_err());
        }
        for value in ["0%", "70%", "100%", "300000", "0"] {
            assert!(validate_frequency(value).is_ok());
        }
    }
}
