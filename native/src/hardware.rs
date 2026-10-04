//! Read-only SoC admission and physical CPU policy mapping.
use crate::config::{Config, ExtremePowerSave};
use crate::soc::{self, Soc};
use crate::util::{self, Result};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

#[derive(Clone, Debug)]
pub struct CpuPolicy {
    pub id: i32,
    pub cpus: BTreeSet<u32>,
    pub governors: Vec<String>,
    pub current_governor: String,
    pub min_freq: u64,
    pub max_freq: u64,
}
#[derive(Clone, Debug)]
pub struct Hardware {
    pub soc: Soc,
    pub device: String,
    pub evidence: String,
    pub policies: Vec<CpuPolicy>,
}
pub fn detect() -> Result<Hardware> {
    let mut properties: Vec<(String, String)> = [
        "ro.soc.model",
        "ro.soc.manufacturer",
        "ro.vendor.qti.soc_model",
        "ro.boot.soc_model",
        "ro.board.platform",
        "ro.vendor.board.platform",
        "ro.hardware",
    ]
    .iter()
    .map(|key| ((*key).into(), util::getprop(key)))
    .collect();
    for base in ["/sys/devices/soc0", "/sys/devices/system/soc/soc0"] {
        for name in ["machine", "family", "soc_id"] {
            let path = format!("{base}/{name}");
            match fs::read_to_string(&path) {
                Ok(value) => properties.push((
                    format!(
                        "sysfs.{path}.{}",
                        if name == "soc_id" {
                            "soc_id"
                        } else {
                            "soc_model"
                        }
                    ),
                    value,
                )),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => eprintln!("novasched: 无法读取 SoC 证据 {path}: {error}"),
            }
        }
    }
    let mut compatibles = Vec::new();
    for path in [
        "/proc/device-tree/compatible",
        "/sys/firmware/devicetree/base/compatible",
    ] {
        match fs::read(path) {
            Ok(raw) => compatibles.extend(
                raw.split(|b| *b == 0)
                    .filter(|v| !v.is_empty())
                    .map(|v| String::from_utf8_lossy(v).into_owned()),
            ),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => eprintln!("novasched: 无法读取 SoC 证据 {path}: {error}"),
        }
    }
    let (soc, evidence) = identify_soc(&properties, &compatibles)?;
    let hardware = Hardware {
        soc,
        device: util::getprop("ro.product.device"),
        evidence,
        policies: inspect_policies(Path::new("/sys/devices/system/cpu/cpufreq"))?,
    };
    hardware.validate_topology()?;
    Ok(hardware)
}
pub fn identify_soc(
    properties: &[(String, String)],
    compatibles: &[String],
) -> Result<(Soc, String)> {
    let mut matches: BTreeMap<Soc, Vec<String>> = BTreeMap::new();
    let mut negative = Vec::new();
    let qualcomm = compatibles.iter().any(|v| v.starts_with("qcom,"))
        || properties.iter().any(|(_, v)| {
            v.to_ascii_lowercase().contains("qualcomm") || v.trim().eq_ignore_ascii_case("qcom")
        });
    for value in compatibles {
        let lower = value.trim().to_ascii_lowercase();
        if let Some(id) = lower.strip_prefix("qcom,") {
            let mut parts = id.split('-');
            let model = parts.next().unwrap_or("");
            let valid_suffix = parts
                .all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_alphanumeric()));
            if let Some(soc) = Soc::from_id(model).filter(|_| valid_suffix) {
                matches
                    .entry(soc)
                    .or_default()
                    .push(format!("device-tree={value}"));
            } else if id.starts_with("sm") && id.as_bytes().get(2).is_some_and(u8::is_ascii_digit) {
                negative.push(format!("device-tree={value}"));
            }
        }
    }
    for (key, value) in properties.iter().filter(|(_, v)| !v.trim().is_empty()) {
        let evidence = format!("{key}={}", value.trim());
        if key.ends_with("soc_id") && qualcomm {
            if let Some(soc) = value.trim().parse::<u32>().ok().and_then(soc::kernel_id) {
                matches.entry(soc).or_default().push(evidence.clone());
            }
        }
        if key.ends_with("soc.model") || key.ends_with("soc_model") {
            let lower = value.trim().to_ascii_lowercase();
            for token in lower.split(|c: char| !c.is_ascii_alphanumeric() && c != '-') {
                if token.starts_with("sm")
                    && token.as_bytes().get(2).is_some_and(u8::is_ascii_digit)
                {
                    let mut parts = token.split('-');
                    let id = parts.next().unwrap_or("");
                    let suffix_valid = parts
                        .all(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphanumeric()));
                    if let Some(soc) = Soc::from_id(id).filter(|_| suffix_valid) {
                        matches.entry(soc).or_default().push(evidence.clone());
                    } else {
                        negative.push(evidence.clone());
                    }
                }
            }
            if lower.contains("snapdragon") {
                let marketing = marketing_identity(value);
                if let Some(soc) = marketing.as_deref().and_then(soc::marketing_model) {
                    matches.entry(soc).or_default().push(evidence.clone());
                } else if marketing.is_some() {
                    negative.push(evidence.clone());
                }
            }
        }
        // pineapple is unambiguous. taro is shared by SM8450 and SM8475.
        if key.ends_with("board.platform") && value.trim().eq_ignore_ascii_case("pineapple") {
            matches.entry(Soc::Gen3).or_default().push(evidence);
        }
    }
    if !negative.is_empty() || matches.len() > 1 {
        return Err(format!(
            "处理器证据不匹配或相互冲突: {}；{}",
            negative.join(", "),
            matches
                .iter()
                .map(|(soc, e)| format!("{}: {}", soc.id(), e.join(", ")))
                .collect::<Vec<_>>()
                .join("；")
        ));
    }
    matches
        .into_iter()
        .next()
        .map(|(soc, e)| (soc, e.join(", ")))
        .ok_or_else(|| {
            format!(
                "未识别到支持的骁龙处理器（SM8450/SM8475/SM8550/SM8650/SM8750/SM8850）；检测值: {}",
                properties
                    .iter()
                    .filter(|(_, v)| !v.trim().is_empty())
                    .map(|(k, v)| format!("{k}={}", v.trim()))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        })
}
fn marketing_identity(value: &str) -> Option<String> {
    let lower = value
        .to_ascii_lowercase()
        .replace('®', "")
        .replace('™', "")
        .replace("(tm)", "");
    let start = lower.find("snapdragon")?;
    let mut words = lower[start..]
        .split(|c: char| !c.is_ascii_alphanumeric() && c != '+')
        .filter(|v| !v.is_empty());
    let mut selected = vec![words.next()?.to_owned()];
    for word in words {
        if word.starts_with("sm")
            || word.starts_with("rev")
            || ["mobile", "platform", "for", "galaxy"].contains(&word)
        {
            break;
        }
        selected.push(word.to_owned());
    }
    // Family labels without a model are not conflicting chip evidence.
    let specific = selected[0]
        .strip_prefix("snapdragon")
        .is_some_and(|v| v.as_bytes().first().is_some_and(u8::is_ascii_digit))
        || selected
            .get(1)
            .is_some_and(|v| v.as_bytes().first().is_some_and(u8::is_ascii_digit));
    if !specific {
        None
    } else {
        Some(selected.join(" "))
    }
}
pub(crate) fn parse_cpu_list(raw: &str) -> Result<BTreeSet<u32>> {
    let mut cpus = BTreeSet::new();
    for part in raw
        .split(|c: char| c.is_ascii_whitespace() || c == ',')
        .filter(|v| !v.is_empty())
    {
        let (start, end) = if let Some((a, b)) = part.split_once('-') {
            (a.parse::<u32>(), b.parse::<u32>())
        } else {
            (part.parse::<u32>(), part.parse::<u32>())
        };
        let (start, end) = (
            start.map_err(|_| format!("CPU 列表非法: {raw}"))?,
            end.map_err(|_| format!("CPU 列表非法: {raw}"))?,
        );
        if start > end || end > 7 {
            return Err(format!("CPU 列表超出支持的核心范围: {raw}"));
        }
        cpus.extend(start..=end);
    }
    if cpus.is_empty() {
        return Err("CPU policy 的 related_cpus 为空".into());
    }
    Ok(cpus)
}
pub fn inspect_policies(root: &Path) -> Result<Vec<CpuPolicy>> {
    let entries =
        fs::read_dir(root).map_err(|e| format!("无法枚举 CPU policy {}: {e}", root.display()))?;
    let mut policies = Vec::new();
    let mut all_cpus = BTreeSet::new();
    for entry in entries {
        let entry = entry.map_err(|e| format!("读取 CPU policy 目录失败: {e}"))?;
        let name = entry.file_name();
        let Some(id) = name
            .to_str()
            .and_then(|n| n.strip_prefix("policy"))
            .and_then(|n| n.parse::<i32>().ok())
        else {
            continue;
        };
        if id < 0 {
            return Err("CPU policy 编号非法".into());
        }
        let base = entry.path();
        let cpus = parse_cpu_list(&util::read_trimmed(base.join("related_cpus"))?)?;
        if cpus.iter().any(|cpu| !all_cpus.insert(*cpu)) {
            return Err("CPU policy 核心列表重复".into());
        }
        for node in ["scaling_min_freq", "scaling_max_freq", "scaling_governor"] {
            util::read_trimmed(base.join(node))?;
        }
        let current_governor = util::read_trimmed(base.join("scaling_governor"))?;
        let governors: Vec<String> =
            match util::read_trimmed(base.join("scaling_available_governors")) {
                Ok(raw) => raw.split_whitespace().map(str::to_owned).collect(),
                Err(_) => vec![current_governor.clone()],
            };
        if governors.is_empty() {
            return Err(format!("policy{id} 未提供可用调速器"));
        }
        let (min_freq, max_freq) = physical_frequency_range(&base)?;
        policies.push(CpuPolicy {
            id,
            cpus,
            governors,
            current_governor,
            min_freq,
            max_freq,
        });
    }
    if all_cpus != (0..7).collect() && all_cpus != (0..8).collect() {
        return Err(format!("CPU policy 未覆盖连续七核或八核: {all_cpus:?}"));
    }
    policies.sort_by_key(|p| p.id);
    Ok(policies)
}
fn physical_frequency_range(base: &Path) -> Result<(u64, u64)> {
    let read = |name: &str| {
        util::read_trimmed(base.join(name))
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .filter(|v| *v > 0)
    };
    if let (Some(min), Some(max)) = (read("cpuinfo_min_freq"), read("cpuinfo_max_freq")) {
        if min <= max {
            return Ok((min, max));
        }
    }
    let mut frequencies: Vec<u64> = util::read_trimmed(base.join("scaling_available_frequencies"))
        .unwrap_or_default()
        .split_whitespace()
        .filter_map(|v| v.parse().ok())
        .filter(|v| *v > 0)
        .collect();
    if frequencies.is_empty() {
        frequencies = util::read_trimmed(base.join("stats/time_in_state"))
            .unwrap_or_default()
            .lines()
            .filter_map(|line| line.split_whitespace().next()?.parse().ok())
            .filter(|v| *v > 0)
            .collect();
    }
    frequencies.sort_unstable();
    match (frequencies.first(), frequencies.last()) {
        (Some(min), Some(max)) => Ok((*min, *max)),
        _ => Err(format!(
            "{} 缺少 CPU 物理频率范围，请保留检测输出",
            base.display()
        )),
    }
}
fn resolve_frequency(raw: &str, policy: &CpuPolicy) -> Result<String> {
    crate::config_v2::validate_frequency(raw)?;
    let requested = if let Some(percent) = raw.strip_suffix('%') {
        let n = percent.parse::<u64>().map_err(|e| e.to_string())?;
        policy.max_freq.checked_mul(n).ok_or("频率百分比计算溢出")? / 100
    } else {
        let value = raw.parse::<u64>().map_err(|e| e.to_string())?;
        if value == 0 {
            return Ok("0".into());
        }
        value
    };
    if policy.min_freq == 0 || policy.max_freq < policy.min_freq {
        return Err("CPU 物理频率范围非法".into());
    }
    Ok(requested
        .clamp(policy.min_freq, policy.max_freq)
        .to_string())
}
fn resolve_cluster(config: &mut Config, cluster: usize, policy: &CpuPolicy) -> Result<()> {
    for profile in config.modes.values_mut() {
        let data = &mut profile.clusters[cluster];
        data.min_freq = resolve_frequency(&data.min_freq, policy)?;
        data.max_freq = resolve_frequency(&data.max_freq, policy)?;
        if data.max_freq != "0"
            && data.min_freq.parse::<u64>().map_err(|e| e.to_string())?
                > data.max_freq.parse::<u64>().map_err(|e| e.to_string())?
        {
            data.min_freq = data.max_freq.clone();
        }
        data.governor = select_governor(&data.governor, policy)?;
    }
    config.functions.launch_boost.frequencies[cluster] =
        resolve_frequency(&config.functions.launch_boost.frequencies[cluster], policy)?;
    config.functions.extreme_powersave.max_frequencies[cluster] = resolve_frequency(
        &config.functions.extreme_powersave.max_frequencies[cluster],
        policy,
    )?;
    if let Some(s) = config.functions.smooth_powersave.as_mut() {
        s.limits.max_frequencies[cluster] =
            resolve_frequency(&s.limits.max_frequencies[cluster], policy)?;
    }
    Ok(())
}
fn select_governor(requested: &str, policy: &CpuPolicy) -> Result<String> {
    if policy.governors.iter().any(|v| v == requested) {
        return Ok(requested.into());
    }
    for candidate in [
        &policy.current_governor,
        "schedutil",
        "walt",
        "interactive",
        "ondemand",
        "conservative",
    ] {
        if policy.governors.iter().any(|v| v == candidate) {
            return Ok(candidate.into());
        }
    }
    Err(format!("policy{} 没有可用的兼容调速器", policy.id))
}
fn tighter(a: &str, b: &str, zero_unlimited: bool) -> Result<String> {
    let a = a.parse::<u64>().map_err(|e| e.to_string())?;
    let b = b.parse::<u64>().map_err(|e| e.to_string())?;
    Ok(if zero_unlimited && a == 0 {
        b
    } else if zero_unlimited && b == 0 {
        a
    } else {
        a.min(b)
    }
    .to_string())
}
impl Hardware {
    pub fn cpus(&self) -> BTreeSet<u32> {
        self.policies
            .iter()
            .flat_map(|p| p.cpus.iter().copied())
            .collect()
    }
    pub fn validate_topology(&self) -> Result<()> {
        let cpus = self.cpus();
        if cpus == (0..8).collect() || (self.soc.elite() && cpus == (0..7).collect()) {
            Ok(())
        } else {
            Err(format!("{} 不支持此 CPU 拓扑: {cpus:?}", self.soc.id()))
        }
    }
    pub fn adapt_config(&self, config: &mut Config) -> Result<Vec<String>> {
        self.validate_topology()?;
        if config.meta.profile_soc.is_some_and(|soc| soc != self.soc) {
            return Err(format!(
                "配置处理器 {} 与当前处理器 {} 不符",
                config.meta.profile_soc.map(Soc::id).unwrap_or("unknown"),
                self.soc.id()
            ));
        }
        let reference = config.policy == self.soc.anchors();
        let original = config.clone();
        let original_policy = config.policy;
        let mut anchors = self.soc.anchors();
        let seven = self.cpus().len() == 7;
        if seven && self.soc.elite() {
            anchors[1] = 5;
        }
        let mut changes = Vec::new();
        for (cluster, cpu) in anchors.iter().enumerate() {
            let configured = config.policy[cluster];
            if configured < 0 {
                continue;
            }
            let actual = if reference {
                None
            } else {
                self.policies.iter().find(|p| p.id == configured)
            };
            let policy = actual
                .or_else(|| {
                    self.policies
                        .iter()
                        .find(|p| *cpu >= 0 && p.cpus.contains(&(*cpu as u32)))
                })
                .ok_or_else(|| format!("找不到 c{cluster} 对应的 CPU policy"))?;
            if policy.id != configured {
                changes.push(format!(
                    "c{cluster}: policy{configured} → policy{}",
                    policy.id
                ));
            }
            config.policy[cluster] = policy.id;
            for (mode, profile) in &mut config.modes {
                let data = &mut profile.clusters[cluster];
                let effective = select_governor(&data.governor, policy)?;
                if effective != data.governor {
                    changes.push(format!(
                        "{mode}.c{cluster}: {} → {effective}",
                        data.governor
                    ));
                    data.governor = effective;
                }
            }
            resolve_cluster(config, cluster, policy)?;
        }
        for cluster in 0..4 {
            if config.policy[cluster] < 0 {
                continue;
            }
            if let Some(first) = (0..cluster).find(|i| config.policy[*i] == config.policy[cluster])
            {
                for profile in config.modes.values_mut() {
                    profile.clusters[first].max_freq = tighter(
                        &profile.clusters[first].max_freq,
                        &profile.clusters[cluster].max_freq,
                        false,
                    )?;
                    let min = tighter(
                        &profile.clusters[first].min_freq,
                        &profile.clusters[cluster].min_freq,
                        false,
                    )?;
                    profile.clusters[first].min_freq =
                        tighter(&min, &profile.clusters[first].max_freq, false)?;
                }
                config.functions.launch_boost.frequencies[first] = tighter(
                    &config.functions.launch_boost.frequencies[first],
                    &config.functions.launch_boost.frequencies[cluster],
                    false,
                )?;
                let merge = |limits: &mut ExtremePowerSave| -> Result<()> {
                    limits.max_frequencies[first] = tighter(
                        &limits.max_frequencies[first],
                        &limits.max_frequencies[cluster],
                        true,
                    )?;
                    Ok(())
                };
                merge(&mut config.functions.extreme_powersave)?;
                if let Some(smooth) = config.functions.smooth_powersave.as_mut() {
                    merge(&mut smooth.limits)?;
                }
                changes.push(format!(
                    "c{cluster} 与 c{first} 共用 policy{}，合并下发",
                    config.policy[cluster]
                ));
                config.policy[cluster] = -1;
            }
        }
        // A reference performance cluster can be split into multiple policies.
        // Every physical policy must be represented, unless explicitly disabled.
        for policy in &self.policies {
            if config.policy.contains(&policy.id) {
                continue;
            }
            let source = (0..4)
                .find(|index| {
                    let start = anchors[*index];
                    let end = anchors
                        .iter()
                        .copied()
                        .filter(|v| *v > start)
                        .min()
                        .unwrap_or(if seven { 7 } else { 8 });
                    start >= 0
                        && policy
                            .cpus
                            .iter()
                            .all(|cpu| *cpu >= start as u32 && *cpu < end as u32)
                })
                .ok_or_else(|| format!("policy{} 无法映射到此配置簇范围", policy.id))?;
            if original_policy[source] < 0 {
                changes.push(format!("policy{} 所属 c{source} 已由用户停用", policy.id));
                continue;
            }
            let free = (0..4)
                .find(|i| config.policy[*i] < 0)
                .ok_or("硬件需要超过四个可独立控制的 CPU policy")?;
            for (mode, profile) in &mut config.modes {
                profile.clusters[free] =
                    original.modes.get(mode).ok_or("配置缺少原始档位")?.clusters[source].clone();
                profile.clusters[free].governor =
                    select_governor(&profile.clusters[free].governor, policy)?;
            }
            config.functions.launch_boost.frequencies[free] =
                original.functions.launch_boost.frequencies[source].clone();
            config.functions.extreme_powersave.max_frequencies[free] =
                original.functions.extreme_powersave.max_frequencies[source].clone();
            if let (Some(smooth), Some(old)) = (
                config.functions.smooth_powersave.as_mut(),
                original.functions.smooth_powersave.as_ref(),
            ) {
                smooth.limits.max_frequencies[free] = old.limits.max_frequencies[source].clone();
            }
            resolve_cluster(config, free, policy)?;
            config.policy[free] = policy.id;
            changes.push(format!(
                "policy{} 分拆自 c{source}，使用 c{free} 下发",
                policy.id
            ));
        }
        if seven {
            let actual = self.cpus();
            let convert = |raw: &str| -> Result<String> {
                let selected = parse_cpu_list(raw)?;
                let mapped: BTreeSet<u32> = if reference {
                    selected
                        .into_iter()
                        .map(|cpu| match cpu {
                            0..=5 => cpu.min(4),
                            6 => 5,
                            _ => 6,
                        })
                        .collect()
                } else {
                    selected.intersection(&actual).copied().collect()
                };
                if mapped.is_empty() {
                    return Err(format!("七核设备 CpuSet 不包含可用核心: {raw}"));
                }
                Ok(mapped
                    .into_iter()
                    .map(|cpu| cpu.to_string())
                    .collect::<Vec<_>>()
                    .join(","))
            };
            let c = &mut config.functions.cpuset;
            for raw in [
                &mut c.top_app,
                &mut c.foreground,
                &mut c.restricted,
                &mut c.system_background,
                &mut c.background,
            ] {
                *raw = convert(raw)?;
            }
            let limits = |v: &mut ExtremePowerSave| -> Result<()> {
                for raw in [&mut v.cpuset_top_app, &mut v.cpuset_foreground]
                    .into_iter()
                    .flatten()
                {
                    *raw = convert(raw)?;
                }
                Ok(())
            };
            limits(&mut config.functions.extreme_powersave)?;
            if let Some(smooth) = config.functions.smooth_powersave.as_mut() {
                limits(&mut smooth.limits)?;
            }
            for profile in config.modes.values_mut() {
                let o = profile.online;
                profile.online = if reference {
                    [o[0], o[1], o[2], o[3], o[4] && o[5], o[6], o[7], true]
                } else {
                    [o[0], o[1], o[2], o[3], o[4], o[5], o[6], true]
                };
            }
            changes.push("七核 Elite：按实际核心范围映射 CpuSet 与 online".into());
        }
        Ok(changes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn props(value: &str) -> Vec<(String, String)> {
        vec![("ro.soc.model".into(), value.into())]
    }
    #[test]
    fn combined_marketing_names_revision_and_family_labels_do_not_reject_gen3() {
        for name in [
            "Qualcomm Snapdragon 8 Gen 3 (SM8650-AB)",
            "Snapdragon 8 Gen 3 SM8650 rev1",
            "Snapdragon 8 Gen 3 Mobile Platform (SM8650)",
            "Snapdragon8Gen3",
        ] {
            let mut p = props(name);
            p.push(("sysfs.family.soc_model".into(), "Snapdragon".into()));
            assert_eq!(identify_soc(&p, &[]).unwrap().0, Soc::Gen3, "{name}");
        }
        for name in [
            "Snapdragon 8s Gen 3 (SM8650)",
            "Snapdragon 8 Gen 30 (SM8650)",
        ] {
            assert!(identify_soc(&props(name), &[]).is_err());
        }
    }
    #[test]
    fn split_policy_resolves_percentages_against_its_own_physical_range() {
        let mut hardware = crate::profiles::test_hardware(Soc::Gen2);
        hardware.policies[1].cpus = parse_cpu_list("3-4").unwrap();
        hardware.policies.push(CpuPolicy {
            id: 5,
            cpus: parse_cpu_list("5-6").unwrap(),
            governors: vec!["schedutil".into()],
            current_governor: "schedutil".into(),
            min_freq: 400000,
            max_freq: 2000000,
        });
        let mut config = Config::load(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("../module-template/config/SM8550.json"),
        )
        .unwrap();
        hardware.adapt_config(&mut config).unwrap();
        assert_eq!(config.policy, [0, 3, 7, 5]);
        assert_eq!(
            config.profile("powersave").unwrap().clusters[3].max_freq,
            "1400000"
        );
        assert_eq!(
            config.profile("fast").unwrap().clusters[3].min_freq,
            "500000"
        );
        let before = format!("{config:?}");
        hardware.adapt_config(&mut config).unwrap();
        assert_eq!(format!("{config:?}"), before);
    }
    #[test]
    fn frequency_percentages_clamp_to_physical_bounds_and_zero_keeps_optional_caps_off() {
        let p = &crate::profiles::test_hardware(Soc::Gen3).policies[0];
        assert_eq!(resolve_frequency("0%", p).unwrap(), "300000");
        assert_eq!(resolve_frequency("70%", p).unwrap(), "2100000");
        assert_eq!(resolve_frequency("100%", p).unwrap(), "3000000");
        assert_eq!(resolve_frequency("9000000", p).unwrap(), "3000000");
        assert_eq!(resolve_frequency("0", p).unwrap(), "0");
        assert!(resolve_frequency("101%", p).is_err());
    }
    #[test]
    fn all_profile_ids_marketing_names_and_kernel_numeric_ids_are_exact() {
        for (soc, numeric) in [
            (Soc::Gen1, 457),
            (Soc::Gen1Plus, 530),
            (Soc::Gen2, 519),
            (Soc::Gen3, 557),
            (Soc::Elite, 618),
            (Soc::Elite5, 660),
        ] {
            for value in [
                soc.id().to_string(),
                format!("{}-AB", soc.id()),
                soc.name().into(),
                format!("Qualcomm {} Mobile Platform", soc.name()),
            ] {
                assert_eq!(identify_soc(&props(&value), &[]).unwrap().0, soc, "{value}");
            }
            let p = vec![
                ("ro.soc.manufacturer".into(), "Qualcomm".into()),
                ("sysfs.soc_id".into(), numeric.to_string()),
            ];
            assert_eq!(identify_soc(&p, &[]).unwrap().0, soc);
            assert!(identify_soc(&p[1..], &[]).is_err());
        }
        for name in [
            "Snapdragon 8s Gen 3",
            "Snapdragon 8 Gen 5",
            "SM8845",
            "SM8650--AB",
            "SM86500",
            "8 Elite 2",
        ] {
            assert!(identify_soc(&props(name), &[]).is_err(), "{name}");
        }
        assert!(identify_soc(&[("ro.board.platform".into(), "taro".into())], &[]).is_err());
    }

    #[test]
    fn auto_preserves_the_oem_governor_and_explicit_selection_still_works() {
        let mut policy = crate::profiles::test_hardware(Soc::Gen3).policies[0].clone();
        policy.governors = vec!["schedutil".into(), "walt".into()];
        policy.current_governor = "walt".into();
        assert_eq!(select_governor("auto", &policy).unwrap(), "walt");
        assert_eq!(select_governor("schedutil", &policy).unwrap(), "schedutil");
        assert_eq!(select_governor("unavailable", &policy).unwrap(), "walt");
    }
    #[test]
    fn all_six_reference_layouts_and_split_gen2_policy_keep_original_parameters() {
        for soc in crate::soc::SUPPORTED {
            let anchors = soc.anchors();
            let policies = anchors
                .iter()
                .copied()
                .filter(|v| *v >= 0)
                .map(|id| CpuPolicy {
                    id,
                    cpus: (id as u32
                        ..anchors
                            .iter()
                            .copied()
                            .filter(|v| *v > id)
                            .min()
                            .unwrap_or(8) as u32)
                        .collect(),
                    governors: vec!["walt".into(), "schedutil".into()],
                    current_governor: "schedutil".into(),
                    min_freq: 300000,
                    max_freq: 3000000,
                })
                .collect();
            let hw = Hardware {
                soc,
                device: "test".into(),
                evidence: "test".into(),
                policies,
            };
            let mut config = Config::load(
                &Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("../module-template/config")
                    .join(soc.file()),
            )
            .unwrap();
            let before = format!("{config:?}");
            hw.adapt_config(&mut config).unwrap();
            assert!(format!("{config:?}") != before);
            assert_eq!(
                config.profile("powersave").unwrap().clusters[0].max_freq,
                "2100000"
            );
        }
        let mut config = Config::load(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("../module-template/config/SM8550.json"),
        )
        .unwrap();
        let mut original = config.profile("fast").unwrap().clusters[1].clone();
        original.min_freq = "750000".into();
        original.max_freq = "3000000".into();
        original.governor = "schedutil".into();
        let hw = Hardware {
            soc: Soc::Gen2,
            device: "test".into(),
            evidence: "test".into(),
            policies: [(0, "0-2"), (3, "3-4"), (5, "5-6"), (7, "7")]
                .into_iter()
                .map(|(id, c)| CpuPolicy {
                    id,
                    cpus: parse_cpu_list(c).unwrap(),
                    governors: vec!["walt".into(), "schedutil".into()],
                    current_governor: "schedutil".into(),
                    min_freq: 300000,
                    max_freq: 3000000,
                })
                .collect(),
        };
        hw.adapt_config(&mut config).unwrap();
        assert_eq!(config.policy, [0, 3, 7, 5]);
        assert_eq!(
            format!("{:?}", config.profile("fast").unwrap().clusters[3]),
            format!("{original:?}")
        );
    }
    #[test]
    fn seven_core_elite_maps_cpus_and_policy_without_admitting_other_seven_core_soc() {
        for soc in [Soc::Elite, Soc::Elite5] {
            let mut config = Config::load(
                &Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("../module-template/config")
                    .join(soc.file()),
            )
            .unwrap();
            let mut hw = Hardware {
                soc,
                device: "test".into(),
                evidence: "test".into(),
                policies: [(0, "0-4"), (5, "5-6")]
                    .into_iter()
                    .map(|(id, c)| CpuPolicy {
                        id,
                        cpus: parse_cpu_list(c).unwrap(),
                        governors: vec!["schedutil".into(), "walt".into()],
                        current_governor: "schedutil".into(),
                        min_freq: 300000,
                        max_freq: 3000000,
                    })
                    .collect(),
            };
            hw.adapt_config(&mut config).unwrap();
            assert_eq!(config.policy, [0, 5, -1, -1]);
            assert!(parse_cpu_list(&config.functions.cpuset.top_app)
                .unwrap()
                .iter()
                .all(|c| *c < 7));
            hw.soc = Soc::Gen3;
            assert!(hw.validate_topology().is_err());
        }
    }
    #[test]
    fn permits_sm8650_variants_and_marketing_names() {
        for value in [
            "SM8650",
            "SM8650-AB",
            "sm8650-ac",
            "SM8650-Q-AB",
            "Qualcomm SM8650",
            "Snapdragon 8 Gen 3",
            "Snapdragon 8 Gen 3 for Galaxy",
        ] {
            assert!(identify_soc(&props(value), &[]).is_ok(), "{value}");
        }
    }
    #[test]
    fn excludes_similar_other_and_unknown_chips() {
        for value in [
            "SM8635",
            "Snapdragon 8s Gen 3",
            "SM85500",
            "SM86500",
            "SM8845",
            "Snapdragon 8s Gen 2",
            "houji",
            "",
        ] {
            assert!(identify_soc(&props(value), &[]).is_err(), "{value}");
        }
    }
    #[test]
    fn uses_platform_or_device_tree_without_a_codename_allowlist() {
        assert!(identify_soc(&[("ro.board.platform".into(), "pineapple".into())], &[]).is_ok());
        assert!(identify_soc(&[], &["samsung,example".into(), "qcom,sm8650".into()]).is_ok());
        assert_eq!(
            identify_soc(&[], &["qcom,sm8550-qrd".into(), "qcom,sm8550".into()])
                .unwrap()
                .0,
            Soc::Gen2
        );
        assert!(identify_soc(&[], &["qcom,sm86500".into()]).is_err());
    }
    #[test]
    fn rejects_conflicting_soc_evidence() {
        let mut p = props("SM8635");
        p.push(("ro.board.platform".into(), "pineapple".into()));
        assert!(identify_soc(&p, &[]).is_err());
        assert!(identify_soc(&props("SM8650"), &["qcom,sm8550".into()]).is_err());
    }
    fn fixture(merged: bool, fallback: bool) -> Hardware {
        let groups = if merged {
            vec![(10, "0-1"), (12, "2-6"), (17, "7")]
        } else {
            vec![(0, "0-1"), (2, "2-4"), (5, "5-6"), (7, "7")]
        };
        Hardware {
            soc: Soc::Gen3,
            device: "any-oem".into(),
            evidence: "SM8650".into(),
            policies: groups
                .into_iter()
                .map(|(id, cpus)| CpuPolicy {
                    id,
                    cpus: parse_cpu_list(cpus).expect("cpus"),
                    governors: if fallback {
                        vec!["schedutil".into()]
                    } else {
                        vec!["walt".into(), "schedutil".into()]
                    },
                    current_governor: "schedutil".into(),
                    min_freq: 300000,
                    max_freq: 3000000,
                })
                .collect(),
        }
    }
    #[test]
    fn standard_layout_preserves_every_profile_and_option() {
        let mut config =
            Config::from_text(include_str!("../../module-template/config/SM8650.json"))
                .expect("config");
        let before = format!("{config:?}");
        fixture(false, false)
            .adapt_config(&mut config)
            .expect("adapt");
        assert_ne!(before, format!("{config:?}"));
        assert_eq!(
            config.profile("powersave").unwrap().clusters[0].max_freq,
            "2100000"
        );
    }
    #[test]
    fn translates_policy_ids_merges_shared_clusters_and_falls_back() {
        let mut config =
            Config::from_text(include_str!("../../module-template/config/SM8650.json"))
                .expect("config");
        assert!(!fixture(true, true)
            .adapt_config(&mut config)
            .expect("adapt")
            .is_empty());
        assert_eq!(config.policy, [10, 12, -1, 17]);
        assert!(config
            .modes
            .values()
            .all(|p| p.clusters[1].governor == "schedutil"));
        assert_eq!(
            config.profile("powersave").expect("mode").clusters[1].max_freq,
            "2100000"
        );
    }
    #[test]
    fn parses_kernel_cpu_list_formats_and_rejects_invalid_ranges() {
        assert_eq!(
            parse_cpu_list("0 1 2").expect("spaces"),
            parse_cpu_list("0-2").expect("range")
        );
        for raw in ["", "7-1", "0-80", "no", "0-1-2"] {
            assert!(parse_cpu_list(raw).is_err(), "{raw}");
        }
    }

    #[test]
    fn shared_policy_preserves_the_stricter_smooth_frequency_cap() {
        let mut config =
            Config::from_text(include_str!("../../module-template/config/SM8650.json"))
                .expect("config");
        config
            .functions
            .smooth_powersave
            .as_mut()
            .expect("smooth")
            .limits
            .max_frequencies[2] = "1209600".into();
        fixture(true, false)
            .adapt_config(&mut config)
            .expect("adapt");
        assert_eq!(config.policy[2], -1);
        assert_eq!(
            config
                .functions
                .smooth_powersave
                .expect("smooth")
                .limits
                .max_frequencies[1],
            "1209600"
        );
    }

    #[test]
    fn probes_real_directory_fixtures_without_writing_nodes() {
        let root = std::env::temp_dir().join(format!("nova-policy-probe-{}", std::process::id()));
        fs::create_dir_all(&root).expect("temp directory");
        for policy in fixture(false, false).policies {
            let base = root.join(format!("policy{}", policy.id));
            fs::create_dir_all(&base).expect("policy directory");
            fs::write(
                base.join("related_cpus"),
                policy
                    .cpus
                    .iter()
                    .map(u32::to_string)
                    .collect::<Vec<_>>()
                    .join(" "),
            )
            .expect("cpus");
            for (name, value) in [
                ("scaling_min_freq", "300000"),
                ("scaling_max_freq", "1000000"),
                ("cpuinfo_min_freq", "300000"),
                ("cpuinfo_max_freq", "3000000"),
                ("scaling_governor", "walt"),
                ("scaling_available_governors", "walt schedutil"),
            ] {
                fs::write(base.join(name), value).expect("fixture node");
            }
        }
        assert_eq!(inspect_policies(&root).expect("probe").len(), 4);
        assert_eq!(
            fs::read_to_string(root.join("policy0/scaling_governor")).expect("unchanged"),
            "walt"
        );
        fs::remove_file(root.join("policy0/scaling_available_governors")).unwrap();
        let detected = inspect_policies(&root).unwrap();
        assert_eq!(detected[0].governors, vec!["walt"]);
        assert_eq!(
            (detected[0].min_freq, detected[0].max_freq),
            (300000, 3000000)
        );
        fs::remove_file(root.join("policy7/scaling_governor")).expect("missing required node");
        assert!(inspect_policies(&root).is_err());
        fs::remove_dir_all(root).expect("cleanup");
    }
}
