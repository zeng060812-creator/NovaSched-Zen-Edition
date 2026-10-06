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
use std::time::{SystemTime, UNIX_EPOCH};

fn read_optional(path: &Path) -> Result<Option<String>> {
    match fs::read_to_string(path) {
        Ok(value) => Ok(Some(value)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("读取 {} 失败: {e}", path.display())),
    }
}
// Canonical fingerprints identify unedited shipped defaults without shipping their text.
const STOCK_FINGERPRINTS: &[u64] = &[
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
        std::env::temp_dir().join(format!("nova-config-{}-{n}", util::current_pid()))
    }
    #[test]
    fn all_six_defaults_initialize_and_manual_edits_survive_repeated_upgrade() {
        for soc in SUPPORTED {
            let state = state();
            let hardware = test_hardware(soc);
            assert!(initialize(&module(), &hardware, &state)
                .unwrap()
                .contains("config=created"));
            let text = fs::read_to_string(state.join("config.json"))
                .unwrap()
                .replace("\"INFO\"", "\"DEBUG\"");
            fs::write(state.join("config.json"), &text).unwrap();
            assert!(initialize(&module(), &hardware, &state)
                .unwrap()
                .contains("config=preserved"));
            assert_eq!(fs::read_to_string(state.join("config.json")).unwrap(), text);
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
            fs::write(state.join("config.json"), &text).unwrap();
            fs::write(state.join("config.json.bak"), "existing backup").unwrap();
            fs::write(
                state.join("options.txt"),
                "extreme_powersave=1\nsmooth_powersave=0\n",
            )
            .unwrap();
            let report = initialize(&module(), &test_hardware(soc), &state).unwrap();
            assert!(report.contains("config=upgraded_preserved"), "{report}");
            let saved = fs::read_to_string(state.join("config.json")).unwrap();
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
                fs::read_to_string(state.join("config.json.bak")).unwrap(),
                "existing backup"
            );
            assert_eq!(
                fs::read_to_string(state.join("options.txt")).unwrap(),
                "extreme_powersave=1\nsmooth_powersave=0\n"
            );
            assert!(initialize(&module(), &test_hardware(soc), &state)
                .unwrap()
                .contains("config=preserved"));
            assert_eq!(
                fs::read_to_string(state.join("config.json")).unwrap(),
                saved
            );
            fs::remove_dir_all(state).unwrap();
        }
    }
    #[test]
    fn chip_change_archives_config_and_conflicting_or_invalid_configs_are_untouched() {
        let state = state();
        initialize(&module(), &test_hardware(Soc::Gen3), &state).unwrap();
        let original = fs::read(state.join("config.json")).unwrap();
        let report = initialize(&module(), &test_hardware(Soc::Elite), &state).unwrap();
        let backup = report
            .lines()
            .find_map(|l| l.strip_prefix("backup="))
            .unwrap();
        assert_eq!(fs::read(backup).unwrap(), original);
        fs::write(state.join("config.json"), original.clone()).unwrap();
        assert!(initialize(&module(), &test_hardware(Soc::Elite), &state).is_err());
        assert_eq!(fs::read(state.join("config.json")).unwrap(), original);
        fs::write(state.join("config.json"), "invalid").unwrap();
        assert!(initialize(&module(), &test_hardware(Soc::Elite), &state).is_err());
        assert_eq!(
            fs::read_to_string(state.join("config.json")).unwrap(),
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
        fs::write(state.join("config.json"), &text).unwrap();
        assert!(initialize(&module(), &test_hardware(Soc::Gen3), &state).is_err());
        assert_eq!(fs::read_to_string(state.join("config.json")).unwrap(), text);
        assert!(!state.join("config.profile").exists());
        fs::remove_dir_all(state).unwrap();
    }
}
fn restore(path: &Path, previous: &Option<String>) -> Result<()> {
    match previous {
        Some(text) => util::atomic_write(path, text.as_bytes(), 0o600),
        None => util::remove_if_exists(path),
    }
}
pub fn initialize(module: &Path, hardware: &Hardware, state: &Path) -> Result<String> {
    let source = module.join("config").join(hardware.soc.file());
    let template = util::read_text(&source)?;
    let mut candidate = Config::from_text(&template)?;
    if candidate.meta.profile_soc != Some(hardware.soc) {
        return Err(format!(
            "模板 {} 缺少或使用错误的 NovaSched 处理器身份",
            source.display()
        ));
    }
    hardware.adapt_config(&mut candidate)?;
    util::create_dir(state)?;
    let _lock = util::acquire_flock(&state.join("config.install.lock"))?;
    let path = state.join("config.json");
    let marker = state.join("config.profile");
    let current = read_optional(&path)?;
    let previous_marker = read_optional(&marker)?;
    let installed = previous_marker
        .as_deref()
        .map(|id| {
            Soc::from_id(id.trim())
                .ok_or_else(|| format!("config.profile 处理器标识无效: {}", id.trim()))
        })
        .transpose()?;
    let parsed = current.as_deref().map(Config::from_text).transpose()?;
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
    // The shipped template IS the tuning. A runtime config whose version
    // differs from the template's is an upgrade candidate: adopt the template
    // (with backup) so retunes reach upgrading users instead of living only
    // in fresh installs. Hand-edits are preserved via the timestamped backup.
    let version_upgrade = parsed
        .as_ref()
        .is_some_and(|parsed| parsed.meta.version != candidate.meta.version);
    let replace = current.is_none()
        || previous_soc != Some(hardware.soc)
        || stock_upgrade
        || version_upgrade;
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
        let mut retained = Config::from_text(&rendered)?;
        hardware.adapt_config(&mut retained)?;
        if &merged != old {
            next = Some(rendered);
        }
    }
    let mut backup = None;
    if next.is_some() {
        if let Some(text) = current.as_ref() {
            let unique = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_err(|e| e.to_string())?
                .as_nanos();
            let name = format!(
                "config.{}.before-{}.{}.{}.json",
                previous_soc.map(Soc::id).unwrap_or("unidentified"),
                hardware.soc.id(),
                unique,
                util::current_pid()
            );
            let backup_path = state.join(name);
            if backup_path.exists() {
                return Err("配置备份文件名冲突，原配置已保留".into());
            }
            util::atomic_write(&backup_path, text.as_bytes(), 0o600)?;
            backup = Some(backup_path);
        }
    }
    let default = state.join("config.default.json");
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
