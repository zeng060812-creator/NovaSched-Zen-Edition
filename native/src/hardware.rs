//! Read-only SoC admission and physical CPU policy mapping.
use crate::config::{Config, MAX_CPU_COUNT};
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
    pub capacity: Option<u32>,
}
#[derive(Clone, Debug)]
pub struct Hardware {
    pub soc: Soc,
    pub samsung: bool,
    pub device: String,
    pub evidence: String,
    pub policies: Vec<CpuPolicy>,
}
pub fn detect() -> Result<Hardware> {
    let mut properties: Vec<(String, String)> = [
        "ro.soc.model",
        "ro.soc.manufacturer",
        "ro.vendor.soc.model",
        "ro.vendor.mediatek.soc.model",
        "ro.vendor.qti.soc_model",
        "ro.boot.soc_model",
        "ro.board.platform",
        "ro.vendor.board.platform",
        "ro.hardware",
    ]
    .iter()
    .map(|key| ((*key).into(), util::getprop(key)))
    .collect();
    if let Ok(cpuinfo) = fs::read_to_string("/proc/cpuinfo") {
        for line in cpuinfo.lines() {
            if let Some((key, value)) = line.split_once(':') {
                if key.trim().eq_ignore_ascii_case("Hardware") {
                    properties.push(("cpuinfo.hardware.soc_model".into(), value.trim().into()));
                }
            }
        }
    }
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
        samsung: samsung_firmware(&[
            util::getprop("ro.product.manufacturer"),
            util::getprop("ro.product.brand"),
            util::getprop("ro.product.vendor.manufacturer"),
            util::getprop("ro.build.version.oneui"),
        ]),
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
    let xring_vendor = properties.iter().any(|(key, value)| {
        key.ends_with("soc.manufacturer") && value.trim().eq_ignore_ascii_case("xring")
    });
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
            // qcom,sm8750 与 qcom,sm8750-ayn 指同一家族；qcom,sm8750p 同理。
            match soc::sm_token(model) {
                soc::SmToken::Match(soc) if valid_suffix => matches
                    .entry(soc)
                    .or_default()
                    .push(format!("device-tree={value}")),
                soc::SmToken::Match(_) | soc::SmToken::Unsupported => {
                    negative.push(format!("device-tree={value}"));
                }
                soc::SmToken::NotSm => {}
            }
        } else if let Some((vendor, model)) = lower.split_once(',') {
            if ["mediatek", "xring", "xiaomi"].contains(&vendor) {
                let found = Soc::from_id(model)
                    .or_else(|| soc::marketing_model(model))
                    .or_else(|| soc::mtk_platform(model).filter(|_| vendor == "mediatek"))
                    .or_else(|| (vendor == "xring" && model == "o3").then_some(Soc::XringO3));
                if let Some(soc) = found {
                    matches
                        .entry(soc)
                        .or_default()
                        .push(format!("device-tree={value}"));
                }
            }
        }
    }
    for (key, value) in properties.iter().filter(|(_, v)| !v.trim().is_empty()) {
        let evidence = format!("{key}={}", value.trim());
        let lower = value.trim().to_ascii_lowercase();
        if ((key.ends_with("soc.model") || key.ends_with("soc_model"))
            && lower == "o3" && xring_vendor)
            || ((key.ends_with("board.platform") || key == "ro.hardware")
                && matches!(lower.as_str(), "xring_o3" | "xring_o3_asic" | "xring-o3"))
        {
            let evidence = if lower == "o3" {
                format!("{evidence}, ro.soc.manufacturer=XRing")
            } else { evidence.clone() };
            matches.entry(Soc::XringO3).or_default().push(evidence);
        }
        if key.ends_with("soc_id") && qualcomm {
            if let Some(soc) = value.trim().parse::<u32>().ok().and_then(soc::kernel_id) {
                matches.entry(soc).or_default().push(evidence.clone());
            }
        }
        if key.ends_with("soc.model") || key.ends_with("soc_model") {
            let lower = value.trim().to_ascii_lowercase();
            if let Some(soc) = Soc::from_id(value.trim())
                .or_else(|| soc::marketing_model(value))
                .or_else(|| soc::mtk_platform(value.trim()))
            {
                matches.entry(soc).or_default().push(evidence.clone());
            }
            for token in lower.split(|c: char| !c.is_ascii_alphanumeric() && c != '-') {
                match soc::sm_token(token) {
                    soc::SmToken::Match(soc) => {
                        matches.entry(soc).or_default().push(evidence.clone());
                    }
                    soc::SmToken::Unsupported => negative.push(evidence.clone()),
                    soc::SmToken::NotSm => {
                        if let Some(soc) = soc::mtk_platform(token) {
                            matches.entry(soc).or_default().push(evidence.clone());
                        }
                    }
                }
            }
            if ["snapdragon", "dimensity", "xring"]
                .iter()
                .any(|brand| lower.contains(brand))
            {
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
                "未识别到支持的处理器（骁龙 8 系列、天玑 8350 / 8400 / 9300+ / 9400+、玄戒 O3）；检测值: {}",
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
    let brand = ["snapdragon", "dimensity", "xring"]
        .into_iter()
        .find(|brand| lower.contains(brand))?;
    let start = lower.find(brand)?;
    let mut words = lower[start..]
        .split(|c: char| !c.is_ascii_alphanumeric() && c != '+')
        .filter(|v| !v.is_empty());
    let mut selected = vec![words.next()?.to_owned()];
    for word in words {
        if word.starts_with("sm")
            || word.starts_with("mt") && word.as_bytes().get(2).is_some_and(u8::is_ascii_digit)
            || word.starts_with("rev")
            || ["mobile", "platform", "for", "galaxy"].contains(&word)
        {
            break;
        }
        selected.push(word.to_owned());
    }
    // Family labels without a model are not conflicting chip evidence.
    let specific = selected[0]
        .strip_prefix(brand)
        .is_some_and(|v| v.as_bytes().first().is_some_and(u8::is_ascii_digit))
        || selected.get(1).is_some_and(|v| {
            v.as_bytes().first().is_some_and(u8::is_ascii_digit)
                || brand == "xring"
                    && v.starts_with('o')
                    && v.as_bytes().get(1).is_some_and(u8::is_ascii_digit)
        });
    if !specific {
        None
    } else {
        Some(selected.join(" "))
    }
}
fn samsung_firmware(values: &[String; 4]) -> bool {
    values[..3].iter().any(|value| value.trim().eq_ignore_ascii_case("samsung"))
        || !values[3].trim().is_empty()
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
        if start > end || end >= MAX_CPU_COUNT as u32 {
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
        let capacity = root.parent().and_then(|cpu_root| {
            cpus.iter()
                .map(|cpu| {
                    util::read_trimmed(cpu_root.join(format!("cpu{cpu}/cpu_capacity")))
                        .ok()?.parse::<u32>().ok().filter(|value| *value > 0)
                })
                .collect::<Option<Vec<_>>>()?
                .into_iter().max()
        });
        policies.push(CpuPolicy {
            id,
            cpus,
            governors,
            current_governor,
            min_freq,
            max_freq,
            capacity,
        });
    }
    if all_cpus.is_empty() || !all_cpus.iter().copied().eq(0..all_cpus.len() as u32) {
        return Err(format!("CPU policy 未覆盖从 CPU0 开始的连续核心: {all_cpus:?}"));
    }
    policies.sort_by_key(|p| p.id);
    Ok(policies)
}
fn frequency_table(base: &Path) -> Vec<u64> {
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
    frequencies.dedup();
    frequencies
}
fn physical_frequency_range(base: &Path) -> Result<(u64, u64)> {
    let read = |name: &str| {
        util::read_trimmed(base.join(name))
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .filter(|v| *v > 0)
    };
    let table = frequency_table(base);
    let (table_min, table_max) = (
        table.first().copied().unwrap_or(0),
        table.last().copied().unwrap_or(0),
    );
    if let (Some(min), Some(max)) = (read("cpuinfo_min_freq"), read("cpuinfo_max_freq")) {
        if min <= max {
            // Qualcomm primes expose a boost OPP above cpuinfo_max_freq
            // (SM8550: 3187 vs 3200 MHz). Without this, 100% resolves one
            // OPP short and 极速 can never reach the true silicon maximum.
            let boost = util::read_trimmed(base.join("scaling_boost_frequencies"))
                .ok()
                .and_then(|raw| {
                    raw.split_whitespace()
                        .filter_map(|v| v.parse::<u64>().ok())
                        .filter(|v| *v > 0)
                        .max()
                });
            let mut min = min;
            let mut max = boost.map(|b| max.max(b)).unwrap_or(max);
            // 交叉校验：部分内核上报占位的伪范围（天玑 9300+ 真机实测
            // cpuinfo_min/max = 1/10000000 kHz）。若 cpuinfo 端点落在可用
            // 频表之外（上限允许超出频表一倍以内，覆盖骁龙未列表的 boost
            // OPP），以频表端点为准，否则百分比会按伪上限换算出硬件根本
            // 不存在的频率，写入即被内核拒绝或静默钳制。
            if table_min > 0 && (min < table_min || min > table_max) {
                min = table_min;
            }
            if table_max > 0 && (max < table_min || max > table_max.saturating_mul(2)) {
                max = table_max;
            }
            if min <= max {
                return Ok((min, max));
            }
        }
    }
    match (table_min, table_max) {
        (min, max) if min > 0 && max >= min => Ok((min, max)),
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
fn resolve_cluster(config: &mut Config, cluster: usize, policy: &CpuPolicy, soc: Soc) -> Result<()> {
    for (mode, profile) in config.modes.iter_mut() {
        let data = &mut profile.clusters[cluster];
        data.min_freq = resolve_frequency(&data.min_freq, policy)?;
        data.max_freq = if data.max_freq == "0" {
            policy.max_freq.to_string()
        } else {
            resolve_frequency(&data.max_freq, policy)?
        };
        if data.max_freq != "0"
            && data.min_freq.parse::<u64>().map_err(|e| e.to_string())?
                > data.max_freq.parse::<u64>().map_err(|e| e.to_string())?
        {
            data.min_freq = data.max_freq.clone();
        }
        data.governor = select_governor(&data.governor, policy, prefer_walt(soc, mode))?;
    }
    config.functions.launch_boost.frequencies[cluster] =
        resolve_frequency(&config.functions.launch_boost.frequencies[cluster], policy)?;
    Ok(())
}
/// 骁龙平台的性能/极速档优先 walt：WALT 窗口负载跟踪在重载下提频更快，
/// 服务于游戏 burst。省电/均衡保留 OEM 调速器（多为 schedutil），配合
/// 贴近省电档的上限把日用功耗压住；天玑/玄戒所有档位都保留原厂。
pub(crate) fn prefer_walt(soc: Soc, mode: &str) -> bool {
    soc.qualcomm() && matches!(mode, "performance" | "fast")
}
fn select_governor(requested: &str, policy: &CpuPolicy, prefer_walt: bool) -> Result<String> {
    if policy.governors.iter().any(|v| v == requested) {
        return Ok(requested.into());
    }
    if requested == "auto" {
        // 骁龙平台的 walt 调速器与内核 WALT 窗口联动，重载提频比 schedutil
        // 更快；天玑 / 玄戒保留 OEM 调速器，避免破坏原厂调校。
        if prefer_walt && policy.governors.iter().any(|v| v == "walt") {
            return Ok("walt".into());
        }
        if matches!(policy.current_governor.as_str(), "performance" | "powersave") {
            if let Some(governor) = ["walt", "schedutil", "interactive", "ondemand", "conservative"].into_iter()
                .find(|candidate| policy.governors.iter().any(|name| name.as_str() == *candidate)) {
                return Ok(governor.into());
            }
        }
    }
    for candidate in [
        &policy.current_governor,
        "walt",
        "schedutil",
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
        if cpus == (0..8).collect()
            || (self.soc.elite() && cpus == (0..7).collect())
            || (self.soc == Soc::XringO3 && cpus == (0..10).collect())
        {
            Ok(())
        } else {
            Err(format!("{} 不支持此 CPU 拓扑: {cpus:?}", self.soc.id()))
        }
    }
    fn adapt_xring_reference(&self, config: &mut Config, changes: &mut Vec<String>) -> Result<()> {
        if self.soc != Soc::XringO3 || self.cpus().len() != 10
            || config.policy != self.soc.anchors()
        {
            return Ok(());
        }
        let sources: &[usize] = match self.policies.len() {
            1 => &[0],
            2 => &[0, 2],
            3 => &[0, 1, 2],
            4 => &[0, 1, 1, 2],
            _ => return Err("玄戒 O3 的 CPU policy 数量超出 1..4，未下发调度策略".into()),
        };
        let use_capacity = self.policies.iter().all(|policy| policy.capacity.is_some());
        let mut policies: Vec<_> = self.policies.iter().collect();
        policies.sort_by_key(|policy| (
            if use_capacity { policy.capacity.unwrap_or(0) } else { 0 },
            policy.max_freq,
            policy.id,
        ));
        let original = config.clone();
        config.policy = [-1; 4];
        for (slot, (policy, source)) in policies.iter().zip(sources).enumerate() {
            config.policy[slot] = policy.id;
            for (mode, profile) in &mut config.modes {
                profile.clusters[slot] = original.modes[mode].clusters[*source].clone();
            }
            config.functions.launch_boost.frequencies[slot] =
                original.functions.launch_boost.frequencies[*source].clone();
        }
        // Migrate only the shipped placement defaults. User app/mode masks stay explicit.
        // 集合相等判断（而非字符串相等）：同一核心集合的 "0-7" 与
        // "0,1,2,3,4,5,6,7" 等写法都按旧默认迁移，避免 cpu8/9 被静默排除。
        let matches_set = |raw: &str, expected: &[u32]| -> bool {
            parse_cpu_list(raw)
                .map(|set| set.iter().copied().collect::<Vec<_>>() == expected)
                .unwrap_or(false)
        };
        let full = "0-9";
        let little = policies[0].cpus.iter().map(u32::to_string).collect::<Vec<_>>().join(",");
        let background = policies[0].cpus.iter().take(2).map(u32::to_string).collect::<Vec<_>>().join(",");
        let cpuset = &mut config.functions.cpuset;
        for raw in [&mut cpuset.top_app, &mut cpuset.foreground] {
            if matches_set(raw.as_str(), &(0..8).collect::<Vec<_>>())
                || matches_set(raw.as_str(), &(0..10).collect::<Vec<_>>())
            {
                *raw = full.into();
            }
        }
        for raw in [&mut cpuset.restricted, &mut cpuset.system_background] {
            if matches_set(raw.as_str(), &(4..8).collect::<Vec<_>>()) {
                *raw = little.clone();
            }
        }
        if matches_set(cpuset.background.as_str(), &[4, 5]) {
            cpuset.background = background;
        }
        changes.push(format!(
            "玄戒 O3 十核：按{}映射全部 {} 个实际 policy {:?}",
            if use_capacity { "内核 CPU capacity" } else { "物理最高频率回退排序（缺少 CPU capacity）" },
            policies.len(), config.policy,
        ));
        Ok(())
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
        let mut changes = Vec::new();
        self.adapt_xring_reference(config, &mut changes)?;
        if self.samsung {
            config.functions.node_watchdog = false;
            config.functions.cpuset.enable = false;
            config.functions.cpuctl.enable = false;
            config.functions.input_boost.enabled = false;
            config.functions.launch_boost.enable = false;
            config.functions.disable_gpu_boost = false;
            config.functions.scheduler.enable = false;
            config.functions.perf_service_lock.enable = false;
            config.functions.fas.enabled = false;
            config.functions.gpu_devfreq.enabled = false;
            for profile in config.modes.values_mut() {
                profile.online = vec![true; self.cpus().len()];
                for cluster in &mut profile.clusters {
                    cluster.governor = "auto".into();
                    cluster.sched_params.clear();
                }
            }
        }
        let reference = config.policy == self.soc.anchors();
        let original = config.clone();
        let original_policy = config.policy;
        let mut anchors = self.soc.anchors();
        let seven = self.cpus().len() == 7;
        if seven && self.soc.elite() {
            anchors[1] = 5;
        }
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
                let effective =
                    select_governor(&data.governor, policy, prefer_walt(self.soc, mode))?;
                if effective != data.governor {
                    changes.push(format!(
                        "{mode}.c{cluster}: {} → {effective}",
                        data.governor
                    ));
                    data.governor = effective;
                }
            }
            resolve_cluster(config, cluster, policy, self.soc)?;
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
                        true,
                    )?;
                    let min = tighter(
                        &profile.clusters[first].min_freq,
                        &profile.clusters[cluster].min_freq,
                        false,
                    )?;
                    profile.clusters[first].min_freq = if profile.clusters[first].max_freq == "0" {
                        min
                    } else {
                        tighter(&min, &profile.clusters[first].max_freq, false)?
                    };
                }
                config.functions.launch_boost.frequencies[first] = tighter(
                    &config.functions.launch_boost.frequencies[first],
                    &config.functions.launch_boost.frequencies[cluster],
                    false,
                )?;
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
                        .unwrap_or(self.cpus().len() as i32);
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
                .ok_or_else(|| {
                    format!(
                        "设备暴露 {} 个独立 cpufreq policy，超过当前 4 个控制槽位；本检查未下发任何调度策略，请保留 probe 日志反馈机型",
                        self.policies.len()
                    )
                })?;
            for (mode, profile) in &mut config.modes {
                profile.clusters[free] =
                    original.modes.get(mode).ok_or("配置缺少原始档位")?.clusters[source].clone();
                profile.clusters[free].governor =
                    select_governor(&profile.clusters[free].governor, policy, prefer_walt(self.soc, mode))?;
            }
            config.functions.launch_boost.frequencies[free] =
                original.functions.launch_boost.frequencies[source].clone();
            resolve_cluster(config, free, policy, self.soc)?;
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
            for placement in c.modes.values_mut().chain(c.app_rule.iter_mut()) {
                if let Some(raw) = &mut placement.top_app {
                    *raw = convert(raw)?;
                }
                if let Some(raw) = &mut placement.foreground {
                    *raw = convert(raw)?;
                }
            }
            for profile in config.modes.values_mut() {
                if reference && profile.online.len() == 8 {
                    let o = &profile.online;
                    profile.online = vec![o[0], o[1], o[2], o[3], o[4] && o[5], o[6], o[7]];
                }
            }
            changes.push("七核 Elite：按实际核心范围映射 CpuSet 与 online".into());
        }
        let actual = self.cpus();
        let cpuset = &mut config.functions.cpuset;
        let clip = |raw: &mut String| -> Result<()> {
            let selected = parse_cpu_list(raw)?;
            let available: BTreeSet<_> = selected.intersection(&actual).copied().collect();
            if available.is_empty() {
                return Err(format!("CpuSet 不包含设备上的可用核心: {raw}"));
            }
            if available != selected {
                *raw = available.iter().map(u32::to_string).collect::<Vec<_>>().join(",");
            }
            Ok(())
        };
        for raw in [&mut cpuset.top_app, &mut cpuset.foreground, &mut cpuset.restricted,
                    &mut cpuset.system_background, &mut cpuset.background] {
            clip(raw)?;
        }
        for placement in cpuset.modes.values_mut().chain(cpuset.app_rule.iter_mut()) {
            for raw in [&mut placement.top_app, &mut placement.foreground].into_iter().flatten() {
                clip(raw)?;
            }
        }
        for (mode, profile) in &mut config.modes {
            if profile.online.iter().skip(actual.len()).any(|online| !online) {
                return Err(format!("{mode}.online 指定了设备上不存在的离线核心"));
            }
            profile.online.resize(actual.len(), true);
            profile.online.truncate(actual.len());
        }
        if self.samsung {
            for profile in config.modes.values_mut() {
                for (cluster, data) in profile.clusters.iter_mut().enumerate() {
                    if let Some(policy) = self.policies.iter().find(|p| p.id == config.policy[cluster]) {
                        data.governor = policy.current_governor.clone();
                    }
                }
            }
            changes.push("三星保守策略：保留原厂调速器，仅设置 CPU 频率范围；暂停 NovaFAS、uclamp、输入加速、节点修复与性能服务接管".into());
        }
        Ok(changes)
    }
}

#[cfg(test)]
pub(crate) fn xring_ten_core_fixture() -> Hardware {
    Hardware {
        soc: Soc::XringO3,
        samsung: false,
        device: "synthetic-ten-core".into(),
        evidence: "synthetic policies; not a captured device frequency table".into(),
        policies: [
            (11, "0-1", 350, 2300000),
            (23, "2-5", 650, 3100000),
            (37, "6-7", 650, 2000000),
            (49, "8 9", 1024, 2800000),
        ].into_iter().map(|(id, cpus, capacity, max_freq)| CpuPolicy {
            id,
            cpus: parse_cpu_list(cpus).unwrap(),
            governors: vec!["schedutil".into()],
            current_governor: "schedutil".into(),
            min_freq: 300000,
            max_freq,
            capacity: Some(capacity),
        }).collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn props(value: &str) -> Vec<(String, String)> {
        vec![("ro.soc.model".into(), value.into())]
    }
    fn xring_config() -> Config {
        Config::from_text(include_str!("../../module-template/config/XRING_O3.json")).unwrap()
    }
    #[test]
    fn ten_core_probe_is_read_only_and_covers_every_physical_policy() {
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let root = std::env::temp_dir().join(format!("nova-o3-ten-{}-{stamp}", std::process::id()));
        let policy_root = root.join("cpufreq");
        let mut hardware = xring_ten_core_fixture();
        let mut original_nodes = BTreeMap::new();
        for policy in &hardware.policies {
            let base = policy_root.join(format!("policy{}", policy.id));
            fs::create_dir_all(&base).unwrap();
            for (name, value) in [
                ("related_cpus", policy.cpus.iter().map(u32::to_string).collect::<Vec<_>>().join(" ")),
                ("scaling_min_freq", "300000".into()),
                ("scaling_max_freq", policy.max_freq.to_string()),
                ("cpuinfo_min_freq", "300000".into()),
                ("cpuinfo_max_freq", policy.max_freq.to_string()),
                ("scaling_governor", "schedutil".into()),
                ("scaling_available_governors", "schedutil".into()),
            ] {
                let path = base.join(name);
                fs::write(&path, &value).unwrap();
                original_nodes.insert(path, value);
            }
            for cpu in &policy.cpus {
                let base = root.join(format!("cpu{cpu}"));
                fs::create_dir_all(&base).unwrap();
                let path = base.join("cpu_capacity");
                let value = policy.capacity.unwrap().to_string();
                fs::write(&path, &value).unwrap();
                original_nodes.insert(path, value);
            }
        }
        hardware.policies = inspect_policies(&policy_root).unwrap();
        hardware.validate_topology().unwrap();
        let mut config = xring_config();
        let changes = hardware.adapt_config(&mut config).unwrap();
        assert_eq!(config.policy, [11, 37, 23, 49]);
        assert!(changes.iter().any(|line| line.contains("内核 CPU capacity")));
        assert_eq!(parse_cpu_list(&config.functions.cpuset.top_app).unwrap(), (0..10).collect());
        assert_eq!(parse_cpu_list(&config.functions.cpuset.background).unwrap(), (0..2).collect());
        for profile in config.modes.values() {
            assert_eq!(profile.online, vec![true; 10]);
            for (slot, id) in config.policy.iter().enumerate() {
                let policy = hardware.policies.iter().find(|policy| policy.id == *id).unwrap();
                let max = profile.clusters[slot].max_freq.parse::<u64>().unwrap();
                assert!((policy.min_freq..=policy.max_freq).contains(&max));
            }
        }
        let before = format!("{config:?}");
        hardware.adapt_config(&mut config).unwrap();
        assert_eq!(format!("{config:?}"), before);
        for (path, value) in original_nodes {
            assert_eq!(fs::read_to_string(path).unwrap(), value);
        }
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn ten_core_upgrade_expands_legacy_online_and_preserves_custom_policy_selection() {
        let hardware = xring_ten_core_fixture();
        let mut config = xring_config();
        for profile in config.modes.values_mut() { profile.online.truncate(8); }
        config.functions.cpuset.top_app = "0-7".into();
        config.functions.cpuset.foreground = "0-7".into();
        hardware.adapt_config(&mut config).unwrap();
        assert!(config.modes.values().all(|profile| profile.online == vec![true; 10]));
        assert_eq!(parse_cpu_list(&config.functions.cpuset.foreground).unwrap(), (0..10).collect());

        let mut custom = xring_config();
        custom.policy = [49, 11, 23, 37];
        hardware.adapt_config(&mut custom).unwrap();
        assert_eq!(custom.policy, [49, 11, 23, 37]);
        let text = crate::json::stringify(&crate::config_v2::encode(&custom, Soc::XringO3)).unwrap();
        assert_eq!(Config::from_text(&text).unwrap().modes["fast"].online.len(), 10);
    }

    #[test]
    fn ten_core_cpuset_migration_matches_core_sets_not_strings() {
        let hardware = xring_ten_core_fixture();
        let mut config = xring_config();
        // 同一核心集合的不同写法都必须被迁移，否则 cpu8/9 被静默排除在
        // cpuset 之外且安装期无任何报错。
        config.functions.cpuset.top_app = "0,1,2,3,4,5,6,7".into();
        config.functions.cpuset.foreground = "0-7".into();
        config.functions.cpuset.restricted = "4,5,6,7".into();
        config.functions.cpuset.system_background = "4-7".into();
        config.functions.cpuset.background = "4,5".into();
        hardware.adapt_config(&mut config).unwrap();
        assert_eq!(config.functions.cpuset.top_app, "0-9");
        assert_eq!(config.functions.cpuset.foreground, "0-9");
        assert_eq!(config.functions.cpuset.restricted, "0,1");
        assert_eq!(config.functions.cpuset.system_background, "0,1");
        assert_eq!(config.functions.cpuset.background, "0,1");
    }
    #[test]
    fn ten_core_fallback_is_explicit_and_other_soc_admission_stays_strict() {
        let mut hardware = xring_ten_core_fixture();
        for policy in &mut hardware.policies { policy.capacity = None; }
        let mut config = xring_config();
        let changes = hardware.adapt_config(&mut config).unwrap();
        assert_eq!(config.policy, [37, 11, 49, 23]);
        assert!(changes.iter().any(|line| line.contains("缺少 CPU capacity")));
        hardware.soc = Soc::Gen3;
        assert!(hardware.validate_topology().is_err());
        hardware.soc = Soc::XringO3;
        hardware.policies.last_mut().unwrap().cpus.remove(&9);
        assert!(hardware.validate_topology().is_err());
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
            capacity: None,
        });
        let mut config = Config::load(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("../module-template/config/SM8550.json"),
        )
        .unwrap();
        config.modes.get_mut("fast").unwrap().clusters[1].min_freq = "25%".into();
        config.modes.get_mut("powersave").unwrap().clusters[1].max_freq = "55%".into();
        hardware.adapt_config(&mut config).unwrap();
        assert_eq!(config.policy, [0, 3, 7, 5]);
        assert_eq!(
            config.profile("powersave").unwrap().clusters[3].max_freq,
            "1100000"
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
            (Soc::Gen5, 685),
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
            "Snapdragon 8s Gen 5",
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
        assert_eq!(select_governor("auto", &policy, false).unwrap(), "walt");
        assert_eq!(select_governor("schedutil", &policy, false).unwrap(), "schedutil");
        assert_eq!(select_governor("unavailable", &policy, false).unwrap(), "walt");
    }
    #[test]
    fn all_reference_layouts_and_split_gen2_policy_keep_original_parameters() {
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
                    capacity: None,
                })
                .collect();
            let hw = Hardware {
                samsung: false,
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
                match soc {
                    Soc::Gen1 | Soc::Gen1Plus | Soc::Gen2 | Soc::XringO3 => "1620000",
                    Soc::Dimensity8350 | Soc::Dimensity8400 => "1680000",
                    // SM8650 按 2W 功耗预算重调：省电小核 66%（无表夹取），
                    // 缓解轻负载在小核排队而被推向中核的反噬。
                    Soc::Gen3 => "1980000",
                    Soc::Elite | Soc::Elite5 | Soc::Dimensity9400Plus | Soc::Gen5 => "1560000",
                    // MT6989 全大核：省电档首簇（A720）留 60% 保底日常流畅。
                    Soc::Dimensity9300Plus => "1800000",
                }
            );
        }
        let mut config = Config::load(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("../module-template/config/SM8550.json"),
        )
        .unwrap();
        config.modes.get_mut("fast").unwrap().clusters[1].min_freq = "25%".into();
        let mut original = config.profile("fast").unwrap().clusters[1].clone();
        original.min_freq = "750000".into();
        original.max_freq = "2940000".into();
        // Gen2 的 fast 档在骁龙上优先 walt（WALT 提频更快，服务游戏 burst）。
        original.governor = "walt".into();
        let hw = Hardware {
            samsung: false,
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
                    capacity: None,
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
                samsung: false,
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
                        capacity: None,
                    })
                    .collect(),
            };
            config.functions.cpuset.modes.insert(
                "fast".into(),
                crate::config::CpusetMode {
                    top_app: Some("6-7".into()),
                    foreground: Some("0-5".into()),
                },
            );
            config.functions.cpuset.app_rule = Some(crate::config::CpusetMode {
                top_app: Some("7".into()),
                foreground: Some("4-7".into()),
            });
            hw.adapt_config(&mut config).unwrap();
            assert_eq!(config.policy, [0, 5, -1, -1]);
            let cpuset = &config.functions.cpuset;
            assert_eq!(cpuset.modes["fast"].top_app.as_deref(), Some("5,6"));
            assert_eq!(
                cpuset.modes["fast"].foreground.as_deref(),
                Some("0,1,2,3,4")
            );
            assert_eq!(
                cpuset.app_rule.as_ref().unwrap().top_app.as_deref(),
                Some("6")
            );
            assert_eq!(
                cpuset.app_rule.as_ref().unwrap().foreground.as_deref(),
                Some("4,5,6")
            );
            let before = format!("{config:?}");
            hw.adapt_config(&mut config).unwrap();
            assert_eq!(format!("{config:?}"), before);
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
    fn gen5_and_dimensity9300plus_real_device_evidence_is_recognized() {
        // 一加 PLR110（骁龙 8 Gen 5 / SM8845 / canoe）真机采集证据。
        let oneplus = vec![
            ("ro.product.manufacturer".into(), "OnePlus".into()),
            ("ro.product.model".into(), "PLR110".into()),
            ("ro.product.device".into(), "OP6117L1".into()),
            ("ro.soc.manufacturer".into(), "QTI".into()),
            ("ro.soc.model".into(), "SM8845".into()),
            ("ro.board.platform".into(), "canoe".into()),
            ("ro.hardware".into(), "qcom".into()),
            ("ro.vendor.qti.soc_model".into(), "SM8845".into()),
            ("ro.vendor.qti.soc_name".into(), "alor".into()),
            ("ro.vendor.qti.soc_id".into(), "685".into()),
        ];
        assert_eq!(identify_soc(&oneplus, &[]).unwrap().0, Soc::Gen5);
        assert_eq!(Soc::Gen5.anchors(), [0, 6, -1, -1]);
        assert!(Soc::Gen5.qualcomm());
        for value in ["SM8845", "Snapdragon 8 Gen 5", "sm8845-ac"] {
            assert_eq!(identify_soc(&props(value), &[]).unwrap().0, Soc::Gen5, "{value}");
        }
        // vivo PD2417（天玑 9300+ / MT6989）真机采集证据。
        let vivo = vec![
            ("ro.product.manufacturer".into(), "vivo".into()),
            ("ro.product.model".into(), "V2417A".into()),
            ("ro.product.device".into(), "PD2417".into()),
            ("ro.soc.manufacturer".into(), "Mediatek".into()),
            ("ro.soc.model".into(), "MT6989".into()),
            ("ro.board.platform".into(), "mt6989".into()),
            ("ro.hardware".into(), "mt6989".into()),
            ("ro.vendor.mediatek.platform".into(), "MT6989".into()),
            ("ro.vendor.soc.model".into(), "MT6989".into()),
        ];
        assert_eq!(identify_soc(&vivo, &[]).unwrap().0, Soc::Dimensity9300Plus);
        assert_eq!(Soc::Dimensity9300Plus.anchors(), [0, 4, 7, -1]);
        assert!(!Soc::Dimensity9300Plus.qualcomm());
        for value in ["MT6989", "MediaTek Dimensity 9300+", "天玑9300+", "dimensity9300plus"] {
            assert_eq!(
                identify_soc(&props(value), &[]).unwrap().0,
                Soc::Dimensity9300Plus,
                "{value}"
            );
        }
        // 营销名精确匹配：天玑 9300（无 +）与 8s Gen 5 不属于本家族。
        for value in ["Dimensity 9300", "天玑9300", "Snapdragon 8s Gen 5"] {
            assert!(identify_soc(&props(value), &[]).is_err(), "{value}");
        }
    }
    #[test]
    fn bogus_cpuinfo_range_falls_back_to_the_frequency_table() {
        // 天玑 9300+ 真机：cpuinfo_min/max_freq 上报占位伪值 1/10000000 kHz，
        // 百分比换算必须回到可用频表边界，否则写出硬件不存在的频率。
        let root = std::env::temp_dir().join(format!("nova-bogus-range-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("cpuinfo_min_freq"), "1").unwrap();
        fs::write(root.join("cpuinfo_max_freq"), "10000000").unwrap();
        fs::write(
            root.join("scaling_available_frequencies"),
            "300000 1200000 2000000",
        )
        .unwrap();
        assert_eq!(physical_frequency_range(&root).unwrap(), (300000, 2000000));
        // 骁龙超大核 boost：cpuinfo 上限允许超出频表（SM8550 3187 vs 2956）。
        fs::write(root.join("cpuinfo_min_freq"), "595200").unwrap();
        fs::write(root.join("cpuinfo_max_freq"), "3187200").unwrap();
        assert_eq!(physical_frequency_range(&root).unwrap(), (595200, 3187200));
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn excludes_similar_other_and_unknown_chips() {
        for value in [
            "SM8635",
            "Snapdragon 8s Gen 3",
            "SM85500",
            "SM86500",
            "SM88450",
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
            samsung: false,
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
                    capacity: None,
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
            "1980000"
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
            // 共享 policy 的两簇取更紧的上限：min(45%,49%) = 45% × 3000000。
            "1350000"
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
    #[test]
    fn supplied_processor_dumps_match_named_profiles_and_nonstandard_policy_ids() {
        let samples = [
            (
                Soc::Dimensity8350,
                include_str!("../tests/fixtures/dimensity8350-cpuinfo.txt"),
                vec![(10, "0-3"), (14, "4-6"), (17, "7")],
                [10, 14, 17, -1],
            ),
            (
                Soc::Dimensity9400Plus,
                include_str!("../tests/fixtures/dimensity9400plus-cpuinfo.txt"),
                vec![(10, "0-3"), (14, "4-6"), (17, "7")],
                [10, 14, 17, -1],
            ),
            (
                Soc::XringO3,
                include_str!("../tests/fixtures/xring-o3-cpuinfo.txt"),
                vec![(10, "0"), (11, "1-3"), (14, "4-7")],
                [14, 10, 11, -1],
            ),
            (
                Soc::Elite5,
                include_str!("../tests/fixtures/sm8850-cpuinfo.txt"),
                vec![(10, "0-5"), (16, "6-7")],
                [10, 16, -1, -1],
            ),
            (
                Soc::Gen5,
                include_str!("../tests/fixtures/sm8845-cpuinfo.txt"),
                vec![(0, "0-5"), (6, "6-7")],
                [0, 6, -1, -1],
            ),
            (
                Soc::Dimensity9300Plus,
                include_str!("../tests/fixtures/mt6989-cpuinfo.txt"),
                vec![(0, "0-3"), (4, "4-6"), (7, "7")],
                [0, 4, 7, -1],
            ),
        ];
        for (soc, dump, groups, expected) in samples {
            let normalized = dump.replace("\r\n", "\n");
            let parts: Vec<String> = normalized
                .split("\n\n")
                .filter_map(|section| {
                    let fields: BTreeMap<_, _> = section
                        .lines()
                        .filter_map(|line| line.split_once(':'))
                        .map(|(k, v)| (k.trim(), v.trim()))
                        .collect();
                    fields.get("processor")?;
                    Some(fields.get("CPU part")?.to_string())
                })
                .collect();
            assert_eq!(parts.len(), 8);
            match soc {
                Soc::XringO3 => assert_eq!(
                    parts,
                    ["0xd4d", "0xd4e", "0xd4e", "0xd4e", "0xd46", "0xd46", "0xd46", "0xd46"]
                ),
                Soc::Dimensity8350 => assert_eq!(
                    parts,
                    ["0xd46", "0xd46", "0xd46", "0xd46", "0xd4d", "0xd4d", "0xd4d", "0xd4d"]
                ),
                Soc::Dimensity9400Plus => assert_eq!(
                    parts,
                    ["0xd81", "0xd81", "0xd81", "0xd81", "0xd82", "0xd82", "0xd82", "0xd85"]
                ),
                Soc::Dimensity9300Plus => assert_eq!(
                    parts,
                    ["0xd81", "0xd81", "0xd81", "0xd81", "0xd82", "0xd82", "0xd82", "0xd82"]
                ),
                Soc::Elite5 | Soc::Gen5 => assert!(parts.iter().all(|part| part == "0x002")),
                _ => unreachable!(),
            }
            let model = dump
                .lines()
                .filter_map(|line| line.split_once(':'))
                .find(|(key, _)| key.trim() == "Hardware")
                .map(|(_, v)| v.trim())
                .unwrap_or(soc.name());
            assert_eq!(identify_soc(&props(model), &[]).unwrap().0, soc);
            let hardware = Hardware {
                samsung: false,
                soc,
                device: "fixture".into(),
                evidence: "provided cpuinfo; simulated policies".into(),
                policies: groups
                    .into_iter()
                    .map(|(id, cpus)| CpuPolicy {
                        id,
                        cpus: parse_cpu_list(cpus).unwrap(),
                        governors: vec!["schedutil".into()],
                        current_governor: "schedutil".into(),
                        min_freq: 300000,
                        max_freq: 3000000,
                        capacity: None,
                    })
                    .collect(),
            };
            let mut config = Config::load(
                &Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("../module-template/config")
                    .join(soc.file()),
            )
            .unwrap();
            hardware.adapt_config(&mut config).unwrap();
            assert_eq!(config.policy, expected);
            let before = format!("{config:?}");
            hardware.adapt_config(&mut config).unwrap();
            assert_eq!(format!("{config:?}"), before);
            for mode in config.modes.values() {
                for (slot, id) in config.policy.iter().enumerate().filter(|(_, id)| **id >= 0) {
                    let data = &mode.clusters[slot];
                    let policy = hardware.policies.iter().find(|p| p.id == *id).unwrap();
                    assert!(
                        data.min_freq.parse::<u64>().unwrap()
                            <= data.max_freq.parse::<u64>().unwrap()
                    );
                    assert!(data.max_freq.parse::<u64>().unwrap() <= policy.max_freq);
                    assert_eq!(data.governor, "schedutil");
                    // 天玑 9300+ 模板刻意携带 schedutil 速率参数（极限策略），
                    // 其它非 Elite 模板仍保持空参数表。
                    if !soc.elite() && soc != Soc::Dimensity9300Plus {
                        assert!(data.sched_params.is_empty());
                    }
                }
            }
        }
    }
    #[test]
    fn processor_names_are_exact_and_shared_arm_parts_do_not_admit_other_models() {
        for (model, soc) in [
            ("天玑8350", Soc::Dimensity8350),
            ("MediaTek Dimensity 9400+", Soc::Dimensity9400Plus),
            ("天玑9400+", Soc::Dimensity9400Plus),
            ("玄戒O3", Soc::XringO3),
            ("Xiaomi Xring O3", Soc::XringO3),
        ] {
            assert_eq!(identify_soc(&props(model), &[]).unwrap().0, soc);
        }
        for model in [
            "Dimensity 83500",
            "Dimensity 9400",
            "Dimensity 9400 Ultra",
            "Xring O1",
            "Xring O30",
        ] {
            assert!(identify_soc(&props(model), &[]).is_err());
        }
        assert!(identify_soc(&[("ro.board.platform".into(),"MT6991".into())],&[]).is_err());
        assert!(identify_soc(&props("Dimensity 8350"), &["xring,o3".into()]).is_err());
    }
    #[test]
    fn redmi_dali_dimensity9400plus_full_evidence_is_recognized() {
        // Redmi dali（25060RK16C，天玑 9400+ / MT6991）真机证据（v2.9.6 采集）。
        let evidence = vec![
            ("ro.product.manufacturer".into(), "Xiaomi".into()),
            ("ro.product.brand".into(), "Redmi".into()),
            ("ro.product.model".into(), "25060RK16C".into()),
            ("ro.product.device".into(), "dali".into()),
            ("ro.soc.manufacturer".into(), "Mediatek".into()),
            ("ro.soc.model".into(), "MT6991".into()),
            ("ro.board.platform".into(), "mt6991".into()),
            ("ro.hardware".into(), "mt6991".into()),
            ("ro.vendor.mediatek.platform".into(), "MT6991".into()),
            ("ro.vendor.soc.manufacturer".into(), "Mediatek".into()),
        ];
        let (soc, _) = identify_soc(&evidence, &[]).unwrap();
        assert_eq!(soc, Soc::Dimensity9400Plus);
        // anchors 与真机 policy 布局一致：policy0=cpu0-3、policy4=cpu4-6、policy7=cpu7。
        assert_eq!(soc.anchors(), [0, 4, 7, -1]);
        // 平台码与骁龙平台码混报 → 冲突拒绝。
        let mut conflict = evidence.clone();
        conflict.push(("ro.vendor.qti.soc_model".into(), "SM8750".into()));
        assert!(identify_soc(&conflict, &[]).is_err());
    }
    #[test]
    fn a_shared_policy_keeps_a_real_cap_when_the_other_slot_is_unlimited() {
        let mut config = crate::profiles::test_config(Soc::Gen3);
        config.policy = Soc::Gen3.anchors();
        config.modes.get_mut("powersave").unwrap().clusters[1].max_freq = "0".into();
        config.modes.get_mut("powersave").unwrap().clusters[2].max_freq = "1200000".into();
        fixture(true, false).adapt_config(&mut config).unwrap();
        assert_eq!(
            config.profile("powersave").unwrap().clusters[1].max_freq,
            "1200000"
        );
    }
    #[test]
    fn unlimited_mode_ceiling_uses_physical_max_but_optional_boost_zero_stays_off() {
        let hardware = crate::profiles::test_hardware(Soc::Gen3);
        let mut config = crate::profiles::test_config(Soc::Gen3);
        config.modes.get_mut("fast").unwrap().clusters[0].max_freq = "0".into();
        hardware.adapt_config(&mut config).unwrap();
        assert_eq!(config.profile("fast").unwrap().clusters[0].max_freq, hardware.policies[0].max_freq.to_string());
        assert_eq!(config.functions.launch_boost.frequencies[0], "0");
    }
    #[test]
    fn automatic_governor_avoids_a_permanent_fixed_frequency_when_scaling_is_available() {
        let mut policy = crate::profiles::test_hardware(Soc::Gen3).policies[0].clone();
        policy.current_governor = "performance".into();
        policy.governors = vec!["performance".into(), "schedutil".into()];
        assert_eq!(select_governor("auto", &policy, true).unwrap(), "schedutil");
        assert_eq!(select_governor("performance", &policy, true).unwrap(), "performance");
    }

    #[test]
    fn leading_version_bins_and_dimensity8400_install_on_their_family_profile() {
        // 一加 Pad 2 Pro（骁龙 8 至尊版领先版）报告 SM8750P。
        for value in ["SM8750P", "sm8750p", "Snapdragon 8 Elite Leading Version", "骁龙8至尊版领先版"] {
            assert_eq!(identify_soc(&props(value), &[]).unwrap().0, Soc::Elite, "{value}");
        }
        // 红米 K80 至尊版（天玑 8400-Ultra）。
        for value in ["Dimensity 8400-Ultra", "dimensity8400", "天玑8400", "天玑8400Ultra"] {
            assert_eq!(
                identify_soc(&props(value), &[]).unwrap().0,
                Soc::Dimensity8400,
                "{value}"
            );
        }
        assert_eq!(
            identify_soc(&props("SM8850AC"), &["qcom,sm8850-qrd".into(), "qcom,sm8850".into()])
                .unwrap()
                .0,
            Soc::Elite5
        );
        // 同属性内的营销名与型号冲突仍然拒绝（8s Gen 3 不是 SM8650）。
        assert!(identify_soc(&props("Snapdragon 8s Gen 3 (SM8650)"), &[]).is_err());
        assert!(identify_soc(&props("SM8750P"), &["qcom,sm8635".into()]).is_err());
    }

    #[test]
    fn xring_o3_split_vendor_properties_match_the_failed_installation() {
        let properties = vec![
            ("ro.soc.model".into(), "O3".into()),
            ("ro.soc.manufacturer".into(), "XRing".into()),
            ("ro.board.platform".into(), "xring_o3_asic".into()),
            ("ro.hardware".into(), "xring_o3_asic".into()),
        ];
        assert_eq!(identify_soc(&properties, &[]).unwrap().0, Soc::XringO3);
        assert_eq!(identify_soc(&properties[..2], &[]).unwrap().0, Soc::XringO3);
        assert!(identify_soc(&properties[..1], &[]).is_err());
        assert_eq!(identify_soc(&properties[2..3], &[]).unwrap().0, Soc::XringO3);
        let mut conflict = properties.clone();
        conflict.push(("ro.vendor.soc.model".into(), "SM8550".into()));
        assert!(identify_soc(&conflict, &[]).is_err());
        let unrelated = vec![("ro.soc.model".into(), "O3".into()), ("ro.soc.manufacturer".into(), "Other".into())];
        assert!(identify_soc(&unrelated, &[]).is_err());
    }

    #[test]
    fn qualcomm_auto_prefers_walt_while_other_platforms_keep_the_oem_governor() {
        // walt 优先只服务骁龙的性能/极速档；省电/均衡与天玑/玄戒保留 OEM。
        assert!(prefer_walt(Soc::Gen3, "fast"));
        assert!(prefer_walt(Soc::Elite, "performance"));
        assert!(!prefer_walt(Soc::Gen3, "balance"));
        assert!(!prefer_walt(Soc::Gen3, "powersave"));
        assert!(!prefer_walt(Soc::Dimensity9400Plus, "fast"));
        assert!(!prefer_walt(Soc::XringO3, "fast"));
        let mut policy = crate::profiles::test_hardware(Soc::Gen3).policies[0].clone();
        policy.governors = vec!["performance".into(), "schedutil".into(), "walt".into()];
        policy.current_governor = "performance".into();
        assert_eq!(select_governor("auto", &policy, true).unwrap(), "walt");
        policy.current_governor = "schedutil".into();
        // 骁龙性能/极速：OEM 的 schedutil 让位于 walt（WALT 窗口负载跟踪提频更快）。
        assert_eq!(select_governor("auto", &policy, true).unwrap(), "walt");
        // 省电/均衡：保留 OEM 正在使用的调速器，不破坏原厂调校。
        assert_eq!(select_governor("auto", &policy, false).unwrap(), "schedutil");
        assert_eq!(select_governor("walt", &policy, false).unwrap(), "walt");
        policy.governors.retain(|value| value != "walt");
        assert_eq!(select_governor("walt", &policy, true).unwrap(), "schedutil");
    }

    #[test]
    fn samsung_keeps_the_factory_governor_and_blocks_extra_controls_after_legacy_config() {
        for soc in crate::soc::SUPPORTED {
            let mut hardware = crate::profiles::test_hardware(soc);
            hardware.samsung = true;
            for policy in &mut hardware.policies {
                policy.current_governor = "performance".into();
                policy.governors = vec!["performance".into(), "walt".into(), "schedutil".into()];
            }
            let mut config = crate::profiles::test_config(soc);
            config.functions.cpuset.enable = true;
            config.functions.cpuctl.enable = true;
            config.functions.input_boost.enabled = true;
            config.functions.scheduler.enable = true;
            config.functions.launch_boost.enable = true;
            config.functions.perf_service_lock.enable = true;
            config.functions.disable_gpu_boost = true;
            for profile in config.modes.values_mut() {
                profile.online = vec![false; 8];
                for data in &mut profile.clusters { data.governor = "schedutil".into(); }
            }
            hardware.adapt_config(&mut config).unwrap();
            assert!(!config.functions.cpuset.enable);
            assert!(!config.functions.cpuctl.enable);
            assert!(!config.functions.input_boost.enabled);
            assert!(!config.functions.scheduler.enable);
            assert!(!config.functions.launch_boost.enable);
            assert!(!config.functions.perf_service_lock.enable);
            assert!(!config.functions.disable_gpu_boost);
            assert!(!config.functions.node_watchdog);
            assert!(!config.functions.fas.enabled);
            for profile in config.modes.values() {
                assert!(profile.online.iter().all(|online| *online));
                for (index, data) in profile.clusters.iter().enumerate() {
                    if config.policy[index] >= 0 { assert_eq!(data.governor, "performance"); }
                    assert!(data.sched_params.is_empty());
                }
            }
            let first = format!("{config:?}");
            hardware.adapt_config(&mut config).unwrap();
            assert_eq!(first, format!("{config:?}"));
        }
    }

}
