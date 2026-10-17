//! Shared installer/boot selection. No scheduling nodes are written here.
use crate::{
    config::Config,
    hardware::Hardware,
    json::{self, Value},
    soc::{Soc, SUPPORTED},
    util::{self, Result},
};
use std::fs;
use std::path::Path;
#[cfg(test)]
use std::time::{SystemTime, UNIX_EPOCH};

fn read_optional(path: &Path) -> Result<Option<String>> {
    match fs::read_to_string(path) {
        Ok(value) => Ok(Some(value)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("读取 {} 失败: {e}", path.display())),
    }
}
// Canonical fingerprints identify unedited shipped defaults without shipping their text.
// 历史版本（v2.2.x 及更早）9 项 + v2.6.1 默认 9 项 + v2.7.0-2.7.2（370）9 项
// + 当前默认（380）10 项。
// `scripts/stock-fingerprints.cjs` 生成；tests::shipped_templates_are_stock
// 在编译期保证列表与模板一致。
const STOCK_FINGERPRINTS: &[u64] = &[
    // Current schema-2 defaults.
    0x0a54bd0af41800f6,0xc75ef8e9e069bed6,0x6401b330f5ea6fa5,0x9b85f833053fee13,0xc2617dcfd60f3372,0xf1707125ea2a1933,0xe1d863d1e4e5cc37,0x19bba46aa364a9fd,0xe2d7accce3dd4fa8,0xda9e3b2b3c4a74cc,
    // v2.11.0 新增：SM8845（骁龙 8 Gen 5）、MT6989（天玑 9300+ 极限性能策略）
    0x2aeebe4e4883bcfb,
    0xc45688b305d5d683,
    0x00df72641cb5315d,
    0x187662c4444d7f9f,
    0x2fd8e8592f6d7ab2,
    0x32191511220e5ccf,
    0x6642209c268b8f78,
    0x66ba31feb9b806a2,
    0x6c536466aa41d751,
    0x8168b275abecce9c,
    0x85fbe9a57f9adf3a,
    0xa87177da09103441,
    0xbb0eb2dd1bf523b9,
    0xc81eb91a20d18da4,
    0xee17150670468fe5,
    // v2.6.1 / 361 出厂默认
    0xbc888f88624ddbb3,
    0xaf9e94ec7766e2f0,
    0x2f5fe631f9a855f6,
    0x3c5b29772b46f6f1,
    0xa2a4c3e8937ff85c,
    0x565e21d2d03dc48d,
    0x479caa59910101a7,
    0x7281d3cce7742fba,
    0x8ef9f2339863c33f,
    // v2.7.0 / 370 出厂默认（四档重调 + NovaFAS）
    0x85f03d50479b1e1e,
    0x6966517043dc4bb8,
    0x7d9416ec59dfd55c,
    0x3e15a63338c2b3d5,
    0xc5418ce71710190c,
    0xf469f19ad3502f57,
    0x36b1c6f3524d2773,
    0x4ecaa5ac6f408daa,
    0x6e09071e6d874491,
    // v2.8.0-2.9.0 / 380 出厂默认（governor 仍为 auto 的历史批次，升级识别用）。
    // 注意 SM8650 有两个历史指纹：b282 为均衡重调批次，2d07 为真机 OPP
    // 重调批次——v2.8.0 与 v2.9.0 实际出货的 SM8650 模板是后者。
    0xd1585868a9fe1cdd,
    0x5833b279736bbd40,
    0xdb12cd59ecbe6b09,
    0xb282aaed50e40878,
    0x2d070c1ed3e39a2a,
    0xb203f6581fae553d,
    0x633e1f5195b72938,
    0xc2ddd15923fb2f02,
    0x1924994cb0ed6562,
    0xd792f594b13ed6dd,
    0xe2d74ecb699e484e,
    // v2.9.1 / 380 出厂默认（显式最优调速器：骁龙 walt、天玑/玄戒 schedutil）
    0xac7d7ec329c7f993,
    0xd359045ffa973ef2,
    0x0267f7b60eb224b3,
    0xc194c312f6f62b8a,
    0x30f3d0017e8adc7d,
    0xfa0fd863bf038228,
    0xd346cb66ec3c0476,
    0x90510745d88dc256,
    0xb487f424ae626b25,
    0x4581603aaa8b806c,
    // v2.10.0 / 390 出厂默认（2W/3W 功耗档 + feed_video 信息流锁帧块）
    0x23bcc8578177bdd2,
    0x47e9345a8d7f2b2f,
    0x6ddd5769b9d56236,
    0xfc9cc4e67ba70a60,
    0x97be111ab8857ef2,
    0x474f79f27199e493,
    0x1e6e3228410a8a6f,
    0x213f50fbbe493423,
    0x3d39d13da3354968,
    0xe96c37185fef01bb,
    // v2.10.1 / 390 出厂默认（FAS 省电/均衡上限下调：待机与日用功耗）
    0xea7610b186503073,
    0x643b3a5081b59492,
    0x9d52dd505fe0de93,
    0x7b39d1dc00ae4bd7,
    0x8f960f1f8ebb24dd,
    0x9d7a4f2612e155c8,
    0x068a64a8f2f4aa16,
    0xad0de19d7c8c2836,
    0x786c1009353902c5,
    0x0bd737356d40feac,
];
const OWNED_META: &[&str] = &[
    "name",
    "author",
    "version",
    "module_id",
    "module_version",
    "profile_id",
    "profile_schema",
    "soc_id",
    "processor",
];
fn stock_fingerprint(root: &Value) -> Result<u64> {
    let mut value = root.clone();
    if let Value::Object(fields) = &mut value {
        if let Some(Value::Object(meta)) = fields.get_mut("meta") {
            for key in OWNED_META {
                meta.remove(*key);
            }
        }
    }
    Ok(json::stringify(&value)?
        .bytes()
        .fold(0xcbf29ce484222325, |hash, byte| {
            (hash ^ byte as u64).wrapping_mul(0x100000001b3)
        }))
}
fn infer_legacy_soc(config: &Config) -> Option<Soc> {
    if let Some(soc) = crate::soc::marketing_model(&config.meta.name) {
        return Some(soc);
    }
    let name = config.meta.name.to_ascii_lowercase().replace(' ', "");
    for (soc, names) in [
        (Soc::Gen1, &["骁龙8g1", "sdm8g1"][..]),
        (Soc::Gen1Plus, &["骁龙8g1+", "sdm8g1+"][..]),
        (Soc::Gen2, &["骁龙8gen2", "sdm8gen2"][..]),
        (Soc::Gen3, &["骁龙8g3", "sdm8g3"][..]),
        (Soc::Elite, &["sdm8elite", "骁龙8elite"][..]),
        (Soc::Elite5, &["骁龙8elite5", "sdm8elite5"][..]),
    ] {
        if names.contains(&name.as_str()) {
            return Some(soc);
        }
    }
    let matches: Vec<Soc> = SUPPORTED
        .into_iter()
        .filter(|soc| soc.anchors() == config.policy)
        .collect();
    if matches.len() == 1 {
        matches.first().copied()
    } else {
        None
    }
}

#[cfg(test)]
pub fn test_hardware(soc: Soc) -> Hardware {
    let anchors = soc.anchors();
    Hardware {
        samsung: false,
        soc,
        device: "fixture".into(),
        evidence: "fixture".into(),
        policies: anchors
            .iter()
            .copied()
            .filter(|id| *id >= 0)
            .map(|id| {
                let end = anchors
                    .iter()
                    .copied()
                    .filter(|v| *v > id)
                    .min()
                    .unwrap_or(8);
                crate::hardware::CpuPolicy {
                    id,
                    cpus: (id as u32..end as u32).collect(),
                    governors: vec!["schedutil".into(), "walt".into()],
                    current_governor: "schedutil".into(),
                    min_freq: 300000,
                    max_freq: 3000000,
                }
            })
            .collect(),
    }
}
#[cfg(test)]
pub fn test_config(soc: Soc) -> Config {
    let mut config = Config::load(
        &Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../module-template/config")
            .join(soc.file()),
    )
    .unwrap();
    test_hardware(soc).adapt_config(&mut config).unwrap();
    config
}

#[cfg(test)]
mod tests {
    use super::*;
    fn module() -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../module-template")
    }
    pub(super) fn state() -> std::path::PathBuf {
        let n = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root=std::env::temp_dir().join(format!("nova-config-{}-{n}", util::current_pid()));
        crate::storage::prepare(&root).unwrap();
        root
    }
    #[test]
    fn shipped_templates_are_stock() {
        for soc in SUPPORTED {
            let path = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../module-template/config")
                .join(soc.file());
            let root = json::parse(&fs::read_to_string(&path).unwrap()).unwrap();
            let hash = stock_fingerprint(&root).unwrap();
            assert!(
                STOCK_FINGERPRINTS.contains(&hash),
                "模板 {} 指纹 {hash:#x} 不在 STOCK_FINGERPRINTS：模板被修改或列表过期，请运行 scripts/stock-fingerprints.cjs 重新生成",
                soc.file()
            );
        }
    }
    #[test]
    fn all_defaults_initialize_and_manual_edits_survive_repeated_upgrade() {
        for soc in SUPPORTED {
            let state = state();
            let hardware = test_hardware(soc);
            assert!(initialize(&module(), &hardware, &state)
                .unwrap()
                .contains("config=created"));
            let text = fs::read_to_string(crate::storage::path(&state, "config.json"))
                .unwrap()
                .replace("\"INFO\"", "\"DEBUG\"");
            fs::write(crate::storage::path(&state, "config.json"), &text).unwrap();
            assert!(initialize(&module(), &hardware, &state)
                .unwrap()
                .contains("config=preserved"));
            assert_eq!(fs::read_to_string(crate::storage::path(&state, "config.json")).unwrap(), text);
            fs::remove_dir_all(state).unwrap();
        }
    }
    #[test]
    fn legacy_conversion_preserves_tuning_extensions_options_and_existing_backup() {
        for soc in SUPPORTED {
            let state = state();
            util::create_dir(&state).unwrap();
            let text = crate::config_v2::legacy_fixture(soc);
            let mut root = json::parse(&text).unwrap();
            if let Value::Object(map) = &mut root {
                map.insert("custom_note".into(), Value::String("我的参数\nZen".into()));
                if let Some(Value::Object(function)) = map.get_mut("Function") {
                    function.insert("MyExtension".into(), Value::Bool(true));
                }
            }
            let text = json::stringify(&root)
                .unwrap()
                .replace("\"INFO\"", "\"DEBUG\"");
            let before = Config::from_text(&text).unwrap();
            fs::write(crate::storage::path(&state, "config.json"), &text).unwrap();
            fs::write(crate::storage::path(&state, "config.json.bak"), "existing backup").unwrap();
            fs::write(
                crate::storage::path(&state, "options.txt"),
                "extreme_powersave=1\nsmooth_powersave=0\n",
            )
            .unwrap();
            let report = initialize(&module(), &test_hardware(soc), &state).unwrap();
            assert!(report.contains("config=upgraded_preserved"), "{report}");
            let saved = fs::read_to_string(crate::storage::path(&state, "config.json")).unwrap();
            let after = Config::from_text(&saved).unwrap();
            assert_eq!(format!("{:?}", after.modes), format!("{:?}", before.modes));
            assert_eq!(
                format!("{:?}", after.functions),
                format!("{:?}", before.functions)
            );
            assert_eq!(after.policy, before.policy);
            assert_eq!(after.meta.loglevel, "DEBUG");
            assert_eq!(after.meta.profile_soc, Some(soc));
            assert_eq!(after.meta.author, "ZenJooo");
            let root = json::parse(&saved).unwrap();
            assert_eq!(
                root.get("custom_note").unwrap().as_str().unwrap(),
                "我的参数\nZen"
            );
            assert!(root
                .get("legacy_extensions")
                .unwrap()
                .get("functions")
                .unwrap()
                .get("MyExtension")
                .unwrap()
                .as_bool()
                .unwrap());
            let backup = report
                .lines()
                .find_map(|v| v.strip_prefix("backup="))
                .unwrap();
            assert_eq!(fs::read_to_string(backup).unwrap(), text);
            assert_eq!(
                fs::read_to_string(crate::storage::path(&state, "config.json.bak")).unwrap(),
                "existing backup"
            );
            assert_eq!(
                fs::read_to_string(crate::storage::path(&state, "options.txt")).unwrap(),
                "extreme_powersave=1\nsmooth_powersave=0\n"
            );
            assert!(initialize(&module(), &test_hardware(soc), &state)
                .unwrap()
                .contains("config=preserved"));
            assert_eq!(
                fs::read_to_string(crate::storage::path(&state, "config.json")).unwrap(),
                saved
            );
            fs::remove_dir_all(state).unwrap();
        }
    }
    #[test]
    fn chip_change_archives_config_and_conflicting_or_invalid_configs_are_untouched() {
        let state = state();
        initialize(&module(), &test_hardware(Soc::Gen3), &state).unwrap();
        let original = fs::read(crate::storage::path(&state, "config.json")).unwrap();
        let report = initialize(&module(), &test_hardware(Soc::Elite), &state).unwrap();
        let backup = report
            .lines()
            .find_map(|l| l.strip_prefix("backup="))
            .unwrap();
        assert_eq!(fs::read(backup).unwrap(), original);
        fs::write(crate::storage::path(&state, "config.json"), original.clone()).unwrap();
        assert!(initialize(&module(), &test_hardware(Soc::Elite), &state).is_err());
        assert_eq!(fs::read(crate::storage::path(&state, "config.json")).unwrap(), original);
        fs::write(crate::storage::path(&state, "config.json"), "invalid").unwrap();
        assert!(initialize(&module(), &test_hardware(Soc::Elite), &state).is_err());
        assert_eq!(
            fs::read_to_string(crate::storage::path(&state, "config.json")).unwrap(),
            "invalid"
        );
        fs::remove_dir_all(state).unwrap();
    }
    #[test]
    fn ambiguous_legacy_identity_is_not_assumed_to_be_gen3() {
        let state = state();
        util::create_dir(&state).unwrap();
        let text = crate::config_v2::legacy_fixture(Soc::Gen1)
            .replace(Soc::Gen1.name(), "unknown user config");
        fs::write(crate::storage::path(&state, "config.json"), &text).unwrap();
        assert!(initialize(&module(), &test_hardware(Soc::Gen3), &state).is_err());
        assert_eq!(fs::read_to_string(crate::storage::path(&state, "config.json")).unwrap(), text);
        assert!(!crate::storage::path(&state, "config.profile").exists());
        fs::remove_dir_all(state).unwrap();
    }
    #[test]
    fn release_upgrade_retunes_stock_and_preserves_custom_parameters_with_backup() {
        let shipped = Config::load(&module().join("config/SM8650.json")).unwrap();
        let stock_max = &shipped.profile("powersave").unwrap().clusters[0].max_freq;
        for custom in [false, true] {
            let state = state();
            let hardware = test_hardware(Soc::Gen3);
            initialize(&module(), &hardware, &state).unwrap();
            let mut old =
                json::parse(&fs::read_to_string(crate::storage::path(&state, "config.json")).unwrap()).unwrap();
            if let Value::Object(root) = &mut old {
                if let Some(Value::Object(meta)) = root.get_mut("meta") {
                    meta.insert("version".into(), Value::Number(237));
                }
                if let Some(Value::Object(features)) = root.get_mut("features") {
                    features.insert("extreme".into(), Value::Bool(true));
                    features.insert("smooth".into(), Value::String("retired".into()));
                }
            }
            let default = json::stringify(&old).unwrap();
            fs::write(crate::storage::path(&state, "config.default.json"), &default).unwrap();
            let text = if custom {
                default.replace(stock_max, "51%")
            } else {
                default
            };
            fs::write(crate::storage::path(&state, "config.json"), &text).unwrap();
            let report = initialize(&module(), &hardware, &state).unwrap();
            let saved = fs::read_to_string(crate::storage::path(&state, "config.json")).unwrap();
            let parsed = Config::from_text(&saved).unwrap();
            assert_eq!(parsed.meta.version, crate::config::PROFILE_VERSION);
            assert_eq!(
                parsed.profile("powersave").unwrap().clusters[0].max_freq,
                if custom { "51%" } else { stock_max }
            );
            let root = json::parse(&saved).unwrap();
            assert!(root.get("features").unwrap().get("extreme").is_err());
            assert!(root.get("features").unwrap().get("smooth").is_err());
            let backup = report
                .lines()
                .find_map(|line| line.strip_prefix("backup="))
                .unwrap();
            assert_eq!(fs::read_to_string(backup).unwrap(), text);
            assert!(initialize(&module(), &hardware, &state)
                .unwrap()
                .contains("config=preserved"));
            fs::remove_dir_all(state).unwrap();
        }
    }

    #[test]
    fn upgrade_handles_missing_and_numeric_fas_without_losing_custom_config() {
        for soc in SUPPORTED {
            for numeric_fas in [false, true] {
                let state = state();
                util::create_dir(&state).unwrap();
                let mut root = json::parse(&fs::read_to_string(module().join("config").join(soc.file())).unwrap()).unwrap();
                let Value::Object(fields) = &mut root else { unreachable!() };
                fields.insert("custom_note".into(), Value::String("保留我的配置".into()));
                let Some(Value::Object(meta)) = fields.get_mut("meta") else { unreachable!() };
                meta.insert("version".into(), Value::Number(361));
                meta.insert("loglevel".into(), Value::String("DEBUG".into()));
                let Some(Value::Object(features)) = fields.get_mut("features") else { unreachable!() };
                if numeric_fas {
                    let Some(Value::Object(fas)) = features.get_mut("fas") else { unreachable!() };
                    fas.insert("step_ms".into(), Value::Number(200));
                    fas.insert("sample_ms".into(), Value::Number(1000));
                    let Some(Value::Object(modes)) = fas.get_mut("modes") else { unreachable!() };
                    for values in modes.values_mut() {
                        let Value::Object(values) = values else { unreachable!() };
                        for key in ["ceiling", "freq_scale"] {
                            let number = values.get(key).unwrap().as_str().unwrap().parse().unwrap();
                            values.insert(key.into(), Value::Number(number));
                        }
                    }
                } else {
                    features.remove("fas");
                }
                let Some(Value::Object(modes)) = fields.get_mut("modes") else { unreachable!() };
                let Some(Value::Object(powersave)) = modes.get_mut("powersave") else { unreachable!() };
                let Some(Value::Array(max)) = powersave.get_mut("max") else { unreachable!() };
                max[0] = Value::String("43%".into());
                let text = json::stringify(&root).unwrap();
                let before = Config::from_text(&text).unwrap();
                fs::write(crate::storage::path(&state, "config.json"), &text).unwrap();
                fs::write(crate::storage::path(&state, "mode.txt"), "balance\n").unwrap();
                fs::write(crate::storage::path(&state, "options.txt"), "extreme_powersave=1\n").unwrap();
                let report = initialize(&module(), &test_hardware(soc), &state).unwrap();
                assert!(report.contains("config=upgraded_preserved"), "{report}");
                let saved = fs::read_to_string(crate::storage::path(&state, "config.json")).unwrap();
                let after = Config::from_text(&saved).unwrap();
                assert_eq!(after.meta.version, crate::config::PROFILE_VERSION);
                assert_eq!(after.meta.loglevel, "DEBUG");
                assert_eq!(after.functions.fas, before.functions.fas);
                assert_eq!(after.profile("powersave").unwrap().clusters[0].max_freq, "43%");
                assert_eq!(json::parse(&saved).unwrap().get("custom_note").unwrap().as_str().unwrap(), "保留我的配置");
                let backup = report.lines().find_map(|line| line.strip_prefix("backup=")).unwrap();
                assert_eq!(fs::read_to_string(backup).unwrap(), text);
                assert_eq!(fs::read_to_string(crate::storage::path(&state, "mode.txt")).unwrap(), "balance\n");
                assert_eq!(fs::read_to_string(crate::storage::path(&state, "options.txt")).unwrap(), "extreme_powersave=1\n");
                assert!(initialize(&module(), &test_hardware(soc), &state).unwrap().contains("config=preserved"));
                assert_eq!(fs::read_to_string(crate::storage::path(&state, "config.json")).unwrap(), saved);
                fs::remove_dir_all(state).unwrap();
            }
        }
    }

    #[test]
    fn invalid_fas_config_reports_its_file_and_leaves_it_untouched() {
        let state = state();
        util::create_dir(&state).unwrap();
        let text = fs::read_to_string(module().join("config/SM8750.json")).unwrap()
            .replace("\"step_ms\": \"200\"", "\"step_ms\": true");
        fs::write(crate::storage::path(&state, "config.json"), &text).unwrap();
        let error = initialize(&module(), &test_hardware(Soc::Elite), &state).unwrap_err();
        assert!(error.contains(&crate::storage::path(&state, "config.json").display().to_string()), "{error}");
        assert!(error.contains("features.fas.step_ms"), "{error}");
        assert_eq!(fs::read_to_string(crate::storage::path(&state, "config.json")).unwrap(), text);
        assert!(!crate::storage::path(&state, "config.profile").exists());
        assert!(!crate::storage::path(&state, "config.default.json").exists());
        fs::remove_dir_all(state).unwrap();
    }
}
fn restore(path: &Path, previous: &Option<String>) -> Result<()> {
    match previous {
        Some(text) => util::atomic_write(path, text.as_bytes(), 0o600),
        None => util::remove_if_exists(path),
    }
}
/// 配置损坏自愈（守护启动路径）：config.json 解析失败时，把损坏文件隔离
/// 为 config/config.corrupt-<时间戳>.json（不丢弃用户数据），优先恢复上次
/// 升级前的备份 restore/config.previous.json，随后按正常初始化流程补齐
/// （无有效备份时从出厂模板重建）。返回是否有损坏被处理。
pub fn recover_corrupt(module: &Path, hardware: &Hardware, state: &Path) -> Result<bool> {
    let path = crate::storage::path(state, "config.json");
    let current = match read_optional(&path)? {
        Some(text) => text,
        None => return Ok(false),
    };
    if Config::from_text(&current).is_ok() {
        return Ok(false);
    }
    let stamp = {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        format!("config.corrupt-{now}.json")
    };
    let quarantine = crate::storage::path(state, &stamp);
    fs::rename(&path, &quarantine)
        .map_err(|e| format!("隔离损坏配置失败: {e}"))?;
    if let Some(backup) = read_optional(&crate::storage::path(state, "config.previous.json"))? {
        if Config::from_text(&backup).is_ok() {
            util::atomic_write(&path, backup.as_bytes(), 0o600)?;
        }
    }
    // 让标准初始化决定后续：有效备份触发迁移，无备份则从模板重建。
    initialize(module, hardware, state)?;
    Ok(true)
}
pub fn initialize(module: &Path, hardware: &Hardware, state: &Path) -> Result<String> {
    let source = module.join("config").join(hardware.soc.file());
    let template = util::read_text(&source)?;
    let mut candidate = Config::from_text(&template)
        .map_err(|error| format!("模板 {} 校验失败: {error}", source.display()))?;
    if candidate.meta.profile_soc != Some(hardware.soc) {
        return Err(format!(
            "模板 {} 缺少或使用错误的 NovaSched 处理器身份",
            source.display()
        ));
    }
    hardware.adapt_config(&mut candidate)?;
    util::create_dir(state)?;
    let _lock = util::acquire_flock(&crate::storage::path(&state, "config.install.lock"))?;
    let path = crate::storage::path(&state, "config.json");
    let marker = crate::storage::path(&state, "config.profile");
    let current = read_optional(&path)?;
    let previous_marker = read_optional(&marker)?;
    let installed = previous_marker
        .as_deref()
        .map(|id| {
            Soc::from_id(id.trim())
                .ok_or_else(|| format!("config.profile 处理器标识无效: {}", id.trim()))
        })
        .transpose()?;
    let parsed = current
        .as_deref()
        .map(|text| {
            Config::from_text(text)
                .map_err(|error| format!("已有配置 {} 校验失败: {error}；原文件已保留", path.display()))
        })
        .transpose()?;
    let current_root = current.as_deref().map(json::parse).transpose()?;
    if installed.is_some()
        && parsed
            .as_ref()
            .and_then(|config| config.meta.profile_soc)
            .is_some_and(|soc| Some(soc) != installed)
    {
        return Err("config.profile 与配置 soc_id 不一致；原配置已保留".into());
    }
    let previous_soc = installed.or_else(|| {
        parsed
            .as_ref()
            .and_then(|c| c.meta.profile_soc.or_else(|| infer_legacy_soc(c)))
    });
    if current.is_some() && previous_soc.is_none() {
        return Err("旧配置无法确定所属处理器；原文件已保留，请补充 config.profile 后重试".into());
    }
    let stock_upgrade = previous_soc == Some(hardware.soc)
        && current_root.as_ref().is_some_and(|root| {
            root.get("schema").is_err()
                && stock_fingerprint(root).is_ok_and(|hash| STOCK_FINGERPRINTS.contains(&hash))
        });
    // Retune an unedited default on upgrade. User edits migrate in place;
    // both paths keep a backup before replacing the runtime file.
    let version_upgrade = parsed
        .as_ref()
        .is_some_and(|parsed| parsed.meta.version != candidate.meta.version);
    let previous_default_text = read_optional(&crate::storage::path(&state, "config.default.json"))?;
    let previous_default = previous_default_text.as_deref()
        .and_then(|text| json::parse(text).ok());
    let unedited_default = match (&current_root, &previous_default) {
        (Some(current), Some(default)) => {
            stock_fingerprint(current)? == stock_fingerprint(default)?
        }
        _ => false,
    };
    let replace = current.is_none()
        || previous_soc != Some(hardware.soc)
        || stock_upgrade
        || (version_upgrade && unedited_default);
    let mut next = if replace {
        Some(template.clone())
    } else {
        None
    };
    if !replace {
        let old = current_root.as_ref().ok_or("保留配置不存在")?;
        let merged =
            crate::config_v2::migrate(old, parsed.as_ref().ok_or("配置不存在")?, hardware.soc)?;
        let rendered = json::stringify(&merged)?;
        let mut retained = Config::from_text(&rendered)
            .map_err(|error| format!("迁移后的配置校验失败: {error}；原文件已保留"))?;
        hardware.adapt_config(&mut retained)?;
        if &merged != old {
            next = Some(rendered);
        }
    }
    let mut backup = None;
    if next.is_some() {
        if let Some(text) = current.as_ref() {
            let backup_path = crate::storage::target(state,"config.previous.json");
            util::atomic_write(&backup_path, text.as_bytes(), 0o600)?;
            backup = Some(backup_path);
        }
    }
    let default = crate::storage::path(&state, "config.default.json");
    if read_optional(&default)?.as_deref() != Some(template.as_str()) {
        util::atomic_write(&default, template.as_bytes(), 0o600)?;
    }
    let commit = (|| -> Result<()> {
        if let Some(text) = next.as_ref() {
            util::atomic_write(&path, text.as_bytes(), 0o600)?;
        }
        util::atomic_write(
            &marker,
            format!("{}\n", hardware.soc.id()).as_bytes(),
            0o600,
        )
    })();
    if let Err(error) = commit {
        let mut issues = Vec::new();
        if let Err(e)=restore(&default,&previous_default_text){issues.push(e);}
        if next.is_some() {
            if let Err(e) = restore(&path, &current) {
                issues.push(e);
            }
        }
        if let Err(e) = restore(&marker, &previous_marker) {
            issues.push(e);
        }
        return Err(format!(
            "选择处理器配置失败: {error}；回滚{}",
            if issues.is_empty() {
                "完成".into()
            } else {
                issues.join("；")
            }
        ));
    }
    Ok(format!("soc={}\nsoc_id={}\nprofile={}\nprofile_id=novasched.{}\nname={}\nauthor={}\nconfig={}\nbackup={}\npolicy_writes=0",
        hardware.soc.name(),hardware.soc.id(),hardware.soc.file(),
        hardware.soc.id().to_ascii_lowercase(),candidate.meta.name,candidate.meta.author,
        if !replace && next.is_some(){"upgraded_preserved"}else if backup.is_some(){"selected_with_backup"}else if replace{"created"}else{"preserved"},
        backup.map(|p|p.display().to_string()).unwrap_or_else(||"none".into())))
}
