use std::collections::BTreeSet;
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::app_modes::{self, Rules};
use crate::json::{self, Value};
use crate::util::{self, Result};

pub const OWN_PROVIDER_MODULE: &str = "NovaSched_Zen_Edition";
pub const SCENE_PACKAGE: &str = "com.omarea.vtools";
/// Keep the classic package first; accept the Scene namespace as a fallback.
pub const SCENE_PACKAGE_FAMILY: &str = "com.omarea.scene";
pub const SCENE_NAMESPACE_FILTER: &str = "com.omarea";
pub const PRESENCE_INTERVAL: Duration = Duration::from_secs(10);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SceneAvailability {
    Available,
    Absent,
    #[default]
    Unknown,
}

#[derive(Clone, Debug)]
pub struct ScenePresence {
    pub availability: SceneAvailability,
    pub evidence: String,
}

static AVAILABILITY: OnceLock<Mutex<Option<(Instant, ScenePresence)>>> = OnceLock::new();

pub fn invalidate_presence() {
    if let Ok(mut cached) = AVAILABILITY.get_or_init(|| Mutex::new(None)).lock() {
        *cached = None;
    }
}

pub fn availability() -> SceneAvailability {
    presence().availability
}

/// Query the active Android user. Global registries also contain packages
/// retained for other users or after uninstall, so they cannot prove presence.
pub fn presence() -> ScenePresence {
    let cache = AVAILABILITY.get_or_init(|| Mutex::new(None));
    if let Ok(cached) = cache.lock() {
        if let Some((at, state)) = cached.as_ref() {
            if at.elapsed() < PRESENCE_INTERVAL {
                return state.clone();
            }
        }
    }
    let user = util::current_android_user();
    let state = probe_presence(user.clone(), |program, args| {
        util::query_command(program, args, Duration::from_secs(2))
    }).with_registry_fallback(user.ok());
    if let Ok(mut cached) = cache.lock() {
        *cached = Some((Instant::now(), state.clone()));
    }
    state
}

fn probe_presence(
    user: Result<u32>,
    mut query: impl FnMut(&str, &[&str]) -> Result<String>,
) -> ScenePresence {
    let user = match user {
        Ok(user) => user.to_string(),
        Err(error) => {
            return ScenePresence {
                availability: SceneAvailability::Unknown,
                evidence: error,
            }
        }
    };
    let mut failures = Vec::new();
    let mut absent = 0;
    for (program, args) in [
        (
            "/system/bin/cmd",
            vec![
                "package",
                "list",
                "packages",
                "--user",
                &user,
                SCENE_NAMESPACE_FILTER,
            ],
        ),
        (
            "/system/bin/pm",
            vec!["list", "packages", "--user", &user, SCENE_NAMESPACE_FILTER],
        ),
        ("/system/bin/cmd", vec!["package", "path", "--user", &user, SCENE_PACKAGE]),
        ("/system/bin/pm", vec!["path", "--user", &user, SCENE_PACKAGE]),
    ] {
        match query(program, &args) {
            Ok(text) => match if args.contains(&"path") { parse_package_path(&text) } else { parse_package_query(&text) } {
                SceneAvailability::Available => {
                    return ScenePresence {
                        availability: SceneAvailability::Available,
                        evidence: format!(
                            "{program} 在当前用户 {user} 的 com.omarea 命名空间中确认已安装 Scene（包含停用应用）"
                        ),
                    }
                }
                SceneAvailability::Absent => absent += 1,
                SceneAvailability::Unknown => {
                    failures.push(format!("{program} 返回无法识别的结果"))
                }
            },
            Err(error) => failures.push(error),
        }
    }
    if absent >= 2 {
        return ScenePresence {
            availability: SceneAvailability::Absent,
            evidence: format!("包管理器多次确认用户 {user} 未安装 {SCENE_PACKAGE} 或 Scene 系应用"),
        };
    }
    ScenePresence {
        availability: SceneAvailability::Unknown,
        evidence: format!(
            "当前用户 {user} 的 Scene 安装状态查询失败: {}",
            failures.join("；")
        ),
    }
}

/// PackageManager can be unavailable during installation. Its registry is
/// independent evidence; per-user restrictions still override global records.
fn registry_presence(global: &str, restrictions: Option<&str>, user: Option<u32>) -> ScenePresence {
    let packages: Vec<_> = global.lines()
        .filter_map(|line| line.split_whitespace().next())
        .filter(|name| is_scene_package(name))
        .collect();
    if packages.is_empty() {
        return ScenePresence {
            availability: SceneAvailability::Absent,
            evidence: "系统包记录中未安装 Scene".into(),
        };
    }
    if let Some(xml) = restrictions {
        if !xml.contains("<package-restrictions") || !xml.contains("</package-restrictions>") {
            return ScenePresence {
                availability: SceneAvailability::Unknown,
                evidence: "当前用户的包记录尚未完整写入".into(),
            };
        }
        let installed = packages.iter().any(|package| {
            !xml.split("<pkg ").skip(1).any(|part| {
                let tag = part.split('>').next().unwrap_or("");
                xml_attribute(tag, "name").as_deref() == Some(*package)
                    && (xml_attribute(tag, "installed").as_deref() == Some("false")
                        || xml_attribute(tag, "hidden").as_deref() == Some("true"))
            })
        });
        return ScenePresence {
            availability: if installed { SceneAvailability::Available } else { SceneAvailability::Absent },
            evidence: format!("系统包记录与用户 {} 安装记录确认 Scene {}", user.unwrap_or(0), if installed { "已安装" } else { "未安装" }),
        };
    }
    ScenePresence {
        availability: SceneAvailability::Available,
        evidence: "系统包记录确认 Scene 已安装；当前用户安装状态暂不可查询".into(),
    }
}

fn xml_attribute(tag: &str, name: &str) -> Option<String> {
    let needle = format!("{name}=\"");
    let value = tag.split_once(&needle)?.1.split('"').next()?;
    Some(value.to_string())
}

impl ScenePresence {
    fn with_registry_fallback(self, user: Option<u32>) -> Self {
        if self.availability != SceneAvailability::Unknown {
            return self;
        }
        let Ok(global) = util::read_text(Path::new("/data/system/packages.list")) else {
            return self;
        };
        let restrictions = user.and_then(|user| util::read_text(Path::new(
            &format!("/data/system/users/{user}/package-restrictions.xml")
        )).ok());
        let fallback = registry_presence(&global, restrictions.as_deref(), user);
        ScenePresence {
            availability: fallback.availability,
            evidence: format!("{}；{}", self.evidence, fallback.evidence),
        }
    }
}

fn parse_package_path(text: &str) -> SceneAvailability {
    if text.trim().is_empty() {
        return SceneAvailability::Absent;
    }
    if text.lines().all(|line| line.trim().strip_prefix("package:")
        .is_some_and(|path| path.starts_with('/') && path.ends_with(".apk"))) {
        SceneAvailability::Available
    } else {
        SceneAvailability::Unknown
    }
}

fn is_scene_package(name: &str) -> bool {
    name == SCENE_PACKAGE || name == SCENE_PACKAGE_FAMILY || name.starts_with("com.omarea.scene.")
}

/// The query filters on the com.omarea namespace, so the reply may contain
/// other packages from the same developer. Any Scene-family package proves
/// presence; an empty or foreign reply proves absence.
fn parse_package_query(text: &str) -> SceneAvailability {
    if !text
        .lines()
        .all(|line| line.trim().is_empty() || line.trim().starts_with("package:"))
    {
        return SceneAvailability::Unknown;
    }
    if text
        .lines()
        .filter_map(|line| line.trim().strip_prefix("package:"))
        .any(is_scene_package)
    {
        SceneAvailability::Available
    } else {
        SceneAvailability::Absent
    }
}

const NATIVE_JSON: &str = "/data/powercfg.json";
const NATIVE_SCRIPT: &str = "/data/powercfg.sh";
const NATIVE_BASE: &str = "/data/powercfg-base.sh";
const OWN_SCRIPT_MAGIC: &str = "NovaSched_Zen_Edition_Scene_Provider";
const PROVIDER_STATE_HEADER: &str = "NOVASCHED_ZEN_SCENE_PROVIDER_V2";
const LEGACY_PROVIDER_STATE_HEADER: &str = "NOVASCHED_ZEN_SCENE_PROVIDER_V1";
fn vtools_xml(user: u32) -> [PathBuf; 2] {
    ["user", "user_de"].map(|root| {
        PathBuf::from(format!(
            "/data/{root}/{user}/com.omarea.vtools/shared_prefs/powercfg.xml"
        ))
    })
}

static XML_CANDIDATES: OnceLock<Mutex<Option<(Instant, Vec<PathBuf>)>>> = OnceLock::new();

/// Scene writes its per-app mode map to `shared_prefs/powercfg.xml` inside
/// its own data directory. N1 era builds may use a renamed package, so
/// every com.omarea package directory is scanned (cached for 10s — this
/// runs on every watched-path refresh).
fn vtools_xml_candidates(user: u32) -> Vec<PathBuf> {
    let cache = XML_CANDIDATES.get_or_init(|| Mutex::new(None));
    if let Ok(cached) = cache.lock() {
        if let Some((at, paths)) = cached.as_ref() {
            if at.elapsed() < PRESENCE_INTERVAL {
                return paths.clone();
            }
        }
    }
    let mut paths = vtools_xml(user).to_vec();
    for root in ["user", "user_de"] {
        let base = PathBuf::from(format!("/data/{root}/{user}"));
        let Ok(entries) = fs::read_dir(&base) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name == SCENE_PACKAGE || name == SCENE_PACKAGE_FAMILY || name.starts_with("com.omarea.scene.") {
                let path = base.join(name.as_ref()).join("shared_prefs").join("powercfg.xml");
                if !paths.contains(&path) {
                    paths.push(path);
                }
            }
        }
    }
    if let Ok(mut cached) = cache.lock() {
        *cached = Some((Instant::now(), paths.clone()));
    }
    paths
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ProviderKind {
    #[default]
    None,
    Own,
    Foreign,
}

/// Scene has one native provider slot. The XML file is Scene's UI state for
/// that slot; it is not by itself another scheduler process.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SceneState {
    pub availability: SceneAvailability,
    pub provider: ProviderKind,
    pub config_path: Option<PathBuf>,
    pub source: String,
    pub mode: Option<String>,
}

impl SceneState {
    pub fn controller(&self) -> &str {
        match self.provider {
            ProviderKind::Own => "Scene（NovaSched Zen Edition）",
            ProviderKind::Foreign => "Scene（其它调度器）",
            ProviderKind::None => "WebUI",
        }
    }

    /// A foreign provider and CTS must never write policies concurrently.
    pub fn controls_locked(&self) -> bool {
        self.provider == ProviderKind::Foreign
    }

    /// Scene supplies profiles/rules while our Rust core performs scheduling.
    /// WebUI keeps diagnostics and independent module preferences available.
    pub fn linked(&self) -> bool {
        self.provider == ProviderKind::Own
    }

    pub fn xml_available(&self) -> bool {
        self.config_path.is_some()
    }
}

pub fn watched_paths() -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = [
        NATIVE_JSON,
        NATIVE_SCRIPT,
        NATIVE_BASE,
        "/data/system/packages.list",
        "/data/system/users/0/package-restrictions.xml",
        "/data/system/packages.xml",
    ]
    .into_iter()
    .map(PathBuf::from)
    .collect();
    if let Ok(user) = util::current_android_user() {
        paths.extend(vtools_xml_candidates(user));
        paths.push(PathBuf::from(format!(
            "/data/system/users/{user}/package-restrictions.xml"
        )));
    }
    paths
}

pub fn registration_needed(state: &SceneState, previous: &SceneState) -> bool {
    state.availability == SceneAvailability::Available
        && (previous.availability != SceneAvailability::Available
            || state.provider != ProviderKind::Own)
}

pub fn detect() -> SceneState {
    detect_with_availability(availability())
}

fn detect_with_availability(available: SceneAvailability) -> SceneState {
    let provider = active_provider(detect_provider(), available);
    let config_path = if available == SceneAvailability::Available {
        scene_config_path()
    } else {
        None
    };
    let xml_mode = config_path
        .as_ref()
        .and_then(|path| util::read_text(path).ok())
        .and_then(|text| parse_xml_mode(&text));
    let native_mode = if provider == ProviderKind::Foreign {
        util::read_text(Path::new(NATIVE_JSON))
            .ok()
            .and_then(|text| parse_json_mode(&text))
            .or_else(|| {
                util::read_text(Path::new(NATIVE_SCRIPT))
                    .ok()
                    .and_then(|text| parse_script_mode(&text))
            })
    } else {
        None
    };
    let source = match provider {
        ProviderKind::Own => "Scene（NovaSched Zen Edition）",
        ProviderKind::Foreign => "Scene（其它调度器）",
        ProviderKind::None => "WebUI",
    }
    .to_string();
    SceneState {
        availability: available,
        provider,
        config_path,
        source,
        mode: xml_mode.or(native_mode),
    }
}

fn active_provider(registered: ProviderKind, availability: SceneAvailability) -> ProviderKind {
    match availability {
        SceneAvailability::Available => registered,
        SceneAvailability::Absent => ProviderKind::None,
        // A failed PM query must not assert that Scene controls our callback.
        // Keep a foreign callback suspended to avoid competing writers.
        SceneAvailability::Unknown if registered == ProviderKind::Foreign => ProviderKind::Foreign,
        SceneAvailability::Unknown => ProviderKind::None,
    }
}

pub fn install_provider(module_dir: &Path) -> Result<ProviderInstallOutcome> {
    if availability() != SceneAvailability::Available {
        return Ok(ProviderInstallOutcome::SkippedMissing);
    }
    let templates = ProviderTemplates::load(module_dir)?;
    let paths = ProviderPaths::new();
    install_provider_at(&paths, &templates)
}

fn install_provider_at(
    paths: &ProviderPaths,
    templates: &ProviderTemplates,
) -> Result<ProviderInstallOutcome> {
    util::create_dir(&paths.dir)?;
    let _lock = util::acquire_flock(&paths.lock)?;

    if !paths.marker.exists() && paths.backup_state.exists() {
        let recovered = restore_locked(&paths)?;
        if !matches!(
            recovered,
            ProviderRestoreOutcome::Restored | ProviderRestoreOutcome::NotManaged
        ) {
            return Err("检测到未完成的 Scene 接管，原有回调未被安全恢复".to_string());
        }
    }

    if paths.marker.exists() {
        validate_provider_backup(paths)?;
        upgrade_base_backup(paths)?;
        templates.write_to_targets(paths)?;
        return Ok(ProviderInstallOutcome::Refreshed);
    }

    // Preserve the original Scene LP provider before taking the callback slot.
    let backup = BackupState {
        json: backup_target(&paths.native_json, &paths.backup_json)?,
        script: backup_target(&paths.native_script, &paths.backup_script)?,
        base: backup_target(&paths.native_base, &paths.backup_base)?,
    };
    util::atomic_write(&paths.backup_state, backup.encode().as_bytes(), 0o600)?;

    if let Err(error) = templates.write_to_targets(paths) {
        let recovered = restore_locked(&paths)
            .map(|outcome| outcome.to_string())
            .unwrap_or_else(|restore_error| format!("回滚失败: {restore_error}"));
        return Err(format!("写入 Scene CTS 回调失败: {error}；{recovered}"));
    }
    if let Err(error) = util::atomic_write(&paths.marker, b"managed=1\n", 0o600) {
        let recovered = restore_locked(&paths)
            .map(|outcome| outcome.to_string())
            .unwrap_or_else(|restore_error| format!("回滚失败: {restore_error}"));
        return Err(format!("写入 Scene 接管标记失败: {error}；{recovered}"));
    }
    Ok(ProviderInstallOutcome::Installed)
}

pub fn restore_provider() -> Result<ProviderRestoreOutcome> {
    let paths = ProviderPaths::new();
    if !paths.dir.exists() {
        return Ok(ProviderRestoreOutcome::NotManaged);
    }
    let _lock = util::acquire_flock(&paths.lock)?;
    restore_locked(&paths)
}

/// Uninstall also removes orphaned callbacks left without a backup marker.
/// Inspect each file separately: another provider may have replaced only half
/// of our registration, and that provider must survive the cleanup.
pub fn uninstall_provider() -> Result<Vec<String>> {
    let paths = ProviderPaths::new();
    let _lock = util::acquire_flock(&paths.lock)?;
    uninstall_provider_at(&paths)
}

fn uninstall_provider_at(paths: &ProviderPaths) -> Result<Vec<String>> {
    let mut warnings = Vec::new();
    let backup = match util::read_text(&paths.backup_state) {
        Ok(text) => match BackupState::parse(&text) {
            Ok(backup) => Some(backup),
            Err(error) => {
                warnings.push(format!("Scene 备份记录无效：{error}"));
                None
            }
        },
        Err(error) if paths.marker.exists() => {
            warnings.push(error);
            None
        }
        Err(_) => None,
    };
    let mut failures = Vec::new();
    for (target, saved, slot, own) in [
        (
            &paths.native_json,
            &paths.backup_json,
            backup.as_ref().map(|b| b.json),
            is_own_json as fn(&str) -> bool,
        ),
        (
            &paths.native_script,
            &paths.backup_script,
            backup.as_ref().map(|b| b.script),
            is_own_script as fn(&str) -> bool,
        ),
        (
            &paths.native_base,
            &paths.backup_base,
            backup.as_ref().map(|b| b.base),
            is_own_script as fn(&str) -> bool,
        ),
    ] {
        let current = match util::read_text(target) {
            Ok(text) => text,
            Err(_) if !target.exists() => continue,
            Err(error) => {
                failures.push(error);
                continue;
            }
        };
        if !own(&current) {
            continue;
        }
        if let Some(BackupSlot::Present(mode)) = slot {
            match util::read_bytes(saved).and_then(|bytes| {
                // An earlier incomplete registration may have backed up our
                // own fragment. Do not restore a callback to a removed module.
                if std::str::from_utf8(&bytes).is_ok_and(own) {
                    util::remove_if_exists(target)
                } else {
                    util::atomic_write(target, &bytes, mode)
                }
            }) {
                Ok(()) => continue,
                Err(error) => warnings.push(format!("Scene 原回调无法恢复：{error}")),
            }
        }
        if let Err(error) = fs::remove_file(target) {
            failures.push(format!(
                "删除 NovaSched 回调 {} 失败：{error}",
                target.display()
            ));
        }
    }
    if failures.is_empty() {
        Ok(warnings)
    } else {
        Err(failures.join("；"))
    }
}

/// Keep Scene's `powercfg.xml` aligned with CTS's local rule file, as the
/// original module does. It is deliberately limited to CTS's own provider;
/// another installed provider is never edited.
pub fn sync_rules(rules: &Rules) -> Result<bool> {
    if !detect().linked() {
        return Ok(false);
    }
    let Some(path) = scene_config_path() else {
        return Ok(false);
    };
    let original = util::read_text(&path)?;
    let merged = merge_xml_rules(&original, rules)?;
    if merged == original {
        return Ok(true);
    }
    let _lock = util::acquire_flock(&crate::storage::path(Path::new(util::STATE_DIR), "scene-provider").as_path().join("rules.lock"))?;
    util::atomic_replace_preserving(&path, merged.as_bytes(), original.as_bytes())?;
    Ok(true)
}

/// Read Scene's mode map when NovaSched owns the Scene callback. Scene stays
/// the source of truth for application-to-mode entries while linked.
pub fn load_rules() -> Result<Option<Rules>> {
    if !detect().linked() {
        return Ok(None);
    }
    let Some(path) = scene_config_path() else {
        return Ok(None);
    };
    let text = util::read_text(&path)?;
    let mut rules = Rules::new();
    for entry in xml_entries(&text) {
        let package = entry.name.trim();
        let mode = entry.value.trim();
        if (package == "*" || util::valid_package(package)) && app_modes::supported(mode) {
            rules.insert(package.to_string(), mode.to_string());
        }
    }
    if rules.is_empty() {
        Ok(None)
    } else {
        Ok(Some(rules))
    }
}

fn detect_provider() -> ProviderKind {
    detect_provider_at(&ProviderPaths::new())
}

fn detect_provider_at(paths: &ProviderPaths) -> ProviderKind {
    match (
        fs::read_to_string(&paths.native_json),
        fs::read_to_string(&paths.native_script),
    ) {
        (Ok(json), Ok(script)) if is_own_json(&json) && is_own_script(&script) => {
            match fs::read_to_string(&paths.native_base) {
                Ok(base) if is_own_script(&base) => ProviderKind::Own,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => ProviderKind::Own,
                _ => ProviderKind::Foreign,
            }
        }
        (Err(json), Err(script))
            if json.kind() == std::io::ErrorKind::NotFound
                && script.kind() == std::io::ErrorKind::NotFound =>
        {
            ProviderKind::None
        }
        (Ok(json), Err(script))
            if is_own_json(&json) && script.kind() == std::io::ErrorKind::NotFound =>
        {
            ProviderKind::None
        }
        (Err(json), Ok(script))
            if json.kind() == std::io::ErrorKind::NotFound && is_own_script(&script) =>
        {
            ProviderKind::None
        }
        _ => ProviderKind::Foreign,
    }
}

fn scene_config_path() -> Option<PathBuf> {
    vtools_xml_candidates(util::current_android_user().ok()?)
        .into_iter()
        .find(|path| path.exists())
}

fn is_own_json(text: &str) -> bool {
    let Ok(Value::Object(values)) = json::parse(text) else {
        return false;
    };
    matches!(
        values.get("module"),
        Some(Value::String(module)) if module == OWN_PROVIDER_MODULE
    )
}

fn is_own_script(text: &str) -> bool {
    text.lines().any(|line| {
        line.trim()
            .strip_prefix('#')
            .is_some_and(|comment| comment.trim() == OWN_SCRIPT_MAGIC)
    })
}

fn parse_json_mode(text: &str) -> Option<String> {
    let value = json::parse(text).ok()?;
    find_mode(&value)
}

fn find_mode(value: &Value) -> Option<String> {
    match value {
        Value::Object(map) => {
            for key in ["mode", "defaultMode", "default_mode", "currentMode", "*"] {
                if let Some(Value::String(mode)) = map.get(key) {
                    if app_modes::supported(mode.trim()) {
                        return Some(mode.trim().to_string());
                    }
                }
            }
            map.values().find_map(find_mode)
        }
        Value::Array(values) => values.iter().find_map(find_mode),
        _ => None,
    }
}

fn parse_script_mode(text: &str) -> Option<String> {
    for line in text.lines() {
        let line = line.split('#').next().unwrap_or("").trim();
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if !matches!(
            key.trim(),
            "mode" | "default_mode" | "defaultMode" | "currentMode"
        ) {
            continue;
        }
        let value = value.trim().trim_matches(['\'', '"']);
        if app_modes::supported(value) {
            return Some(value.to_string());
        }
    }
    None
}

fn parse_xml_mode(xml: &str) -> Option<String> {
    xml_entries(xml).into_iter().find_map(|entry| {
        (entry.name == "*" && app_modes::supported(&entry.value)).then_some(entry.value)
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BackupSlot {
    Absent,
    Present(u32),
}

#[derive(Clone, Copy, Debug)]
struct BackupState {
    json: BackupSlot,
    script: BackupSlot,
    base: BackupSlot,
}

impl BackupState {
    fn encode(self) -> String {
        format!(
            "{PROVIDER_STATE_HEADER}\njson={}\nscript={}\nbase={}\n",
            self.json.encode(),
            self.script.encode(),
            self.base.encode()
        )
    }

    fn parse(text: &str) -> Result<Self> {
        let mut lines = text.lines();
        let header = lines.next();
        if !matches!(header, Some(PROVIDER_STATE_HEADER | LEGACY_PROVIDER_STATE_HEADER)) {
            return Err("Scene 回调备份头无效".to_string());
        }
        let mut json = None;
        let mut script = None;
        let mut base = None;
        for line in lines {
            let Some((key, value)) = line.split_once('=') else {
                return Err("Scene 回调备份格式无效".to_string());
            };
            match key {
                "json" => json = Some(BackupSlot::parse(value)?),
                "script" => script = Some(BackupSlot::parse(value)?),
                "base" => base = Some(BackupSlot::parse(value)?),
                _ => return Err("Scene 回调备份包含未知字段".to_string()),
            }
        }
        Ok(Self {
            json: json.ok_or_else(|| "Scene 回调备份缺少 JSON 状态".to_string())?,
            script: script.ok_or_else(|| "Scene 回调备份缺少脚本状态".to_string())?,
            base: match (base, header) {
                (Some(base), _) => base,
                (None, Some(LEGACY_PROVIDER_STATE_HEADER)) => BackupSlot::Absent,
                _ => return Err("Scene 回调备份缺少初始化脚本状态".into()),
            },
        })
    }
}

impl BackupSlot {
    fn encode(self) -> String {
        match self {
            Self::Absent => "absent".to_string(),
            Self::Present(mode) => format!("present:{mode:o}"),
        }
    }

    fn parse(value: &str) -> Result<Self> {
        if value == "absent" {
            return Ok(Self::Absent);
        }
        let Some(mode) = value.strip_prefix("present:") else {
            return Err("Scene 回调备份状态无效".to_string());
        };
        u32::from_str_radix(mode, 8)
            .map(Self::Present)
            .map_err(|error| format!("Scene 回调备份权限无效: {error}"))
    }
}

struct ProviderPaths {
    dir: PathBuf,
    lock: PathBuf,
    marker: PathBuf,
    backup_state: PathBuf,
    backup_json: PathBuf,
    backup_script: PathBuf,
    backup_base: PathBuf,
    native_json: PathBuf,
    native_script: PathBuf,
    native_base: PathBuf,
}

impl ProviderPaths {
    fn new() -> Self {
        let dir = crate::storage::path(Path::new(util::STATE_DIR), "scene-provider");
        Self {
            lock: dir.join("lock"),
            marker: dir.join("managed"),
            backup_state: dir.join("backup-state"),
            backup_json: dir.join("powercfg.json.backup"),
            backup_script: dir.join("powercfg.sh.backup"),
            backup_base: dir.join("powercfg-base.sh.backup"),
            native_json: PathBuf::from(NATIVE_JSON),
            native_script: PathBuf::from(NATIVE_SCRIPT),
            native_base: PathBuf::from(NATIVE_BASE),
            dir,
        }
    }
}

struct ProviderTemplates {
    json: Vec<u8>,
    script: Vec<u8>,
    base: Vec<u8>,
}

impl ProviderTemplates {
    fn load(module_dir: &Path) -> Result<Self> {
        let json = util::read_bytes(&module_dir.join("vtools/powercfg.json"))?;
        let template = util::read_text(&module_dir.join("vtools/powercfg.sh"))?;
        let resolved = module_dir
            .canonicalize()
            .map_err(|e| format!("确定 Scene 模块目录失败: {e}"))?;
        let resolved = active_module_path(&resolved);
        let quoted = format!("'{}'", resolved.to_string_lossy().replace('\'', "'\\''"));
        let script = template
            .replace("@NOVASCHED_MODULE_DIR@", &quoted)
            .into_bytes();
        let json_text =
            std::str::from_utf8(&json).map_err(|_| "模块内 Scene JSON 不是 UTF-8".to_string())?;
        let script_text =
            std::str::from_utf8(&script).map_err(|_| "模块内 Scene 脚本不是 UTF-8".to_string())?;
        if !is_own_json(json_text) || !is_own_script(script_text) {
            return Err("模块内 Scene 回调身份校验失败".to_string());
        }
        let base = util::read_bytes(&module_dir.join("vtools/powercfg-base.sh"))?;
        if !std::str::from_utf8(&base).is_ok_and(is_own_script) {
            return Err("模块内 Scene 初始化脚本身份校验失败".into());
        }
        Ok(Self { json, script, base })
    }

    fn write_to_targets(&self, paths: &ProviderPaths) -> Result<()> {
        // Script first: the JSON entry is written last, so Scene never sees an
        // entry that points to a missing CTS callback.
        util::atomic_write(&paths.native_script, &self.script, 0o755)?;
        util::atomic_write(&paths.native_base, &self.base, 0o755)?;
        util::atomic_write(&paths.native_json, &self.json, 0o644)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProviderInstallOutcome {
    Installed,
    Refreshed,
    SkippedMissing,
}

impl std::fmt::Display for ProviderInstallOutcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let text = match self {
            Self::Installed => "Scene 已注册 NovaSched Zen Edition（已备份原回调）",
            Self::Refreshed => "Scene 已由 NovaSched 接管，原回调备份保留",
            Self::SkippedMissing => "系统安装记录未确认 Scene 已安装，未写入 Scene 回调",
        };
        f.write_str(text)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProviderRestoreOutcome {
    Restored,
    NotManaged,
    SkippedChanged,
}

impl std::fmt::Display for ProviderRestoreOutcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let text = match self {
            Self::Restored => "Scene 原回调已恢复",
            Self::NotManaged => "没有 CTS 管理的 Scene 回调",
            Self::SkippedChanged => "Scene 回调已被用户替换，未覆盖",
        };
        f.write_str(text)
    }
}

fn backup_target(target: &Path, backup_path: &Path) -> Result<BackupSlot> {
    if !target.exists() {
        return Ok(BackupSlot::Absent);
    }
    let bytes = util::read_bytes(target)?;
    let mode = fs::metadata(target)
        .map_err(|error| format!("读取 Scene 回调权限 {} 失败: {error}", target.display()))?
        .mode()
        & 0o7777;
    util::atomic_write(backup_path, &bytes, 0o600)?;
    Ok(BackupSlot::Present(mode))
}

fn validate_provider_backup(paths: &ProviderPaths) -> Result<()> {
    let backup = BackupState::parse(&util::read_text(&paths.backup_state)?)?;
    for (slot, path) in [
        (backup.json, &paths.backup_json),
        (backup.script, &paths.backup_script),
        (backup.base, &paths.backup_base),
    ] {
        if matches!(slot, BackupSlot::Present(_)) {
            util::read_bytes(path)?;
        }
    }
    Ok(())
}

fn active_module_path(path: &Path) -> PathBuf {
    if let Some(parent) = path.parent().filter(|p| p.file_name().is_some_and(|n| n == "modules_update")) {
        if let (Some(root), Some(name)) = (parent.parent(), path.file_name()) {
            return root.join("modules").join(name);
        }
    }
    path.to_path_buf()
}

fn upgrade_base_backup(paths: &ProviderPaths) -> Result<()> {
    let text = util::read_text(&paths.backup_state)?;
    if text.lines().next() == Some(LEGACY_PROVIDER_STATE_HEADER) {
        let mut state = BackupState::parse(&text)?;
        state.base = backup_target(&paths.native_base, &paths.backup_base)?;
        util::atomic_write(&paths.backup_state, state.encode().as_bytes(), 0o600)?;
    }
    Ok(())
}

fn restore_locked(paths: &ProviderPaths) -> Result<ProviderRestoreOutcome> {
    let (state, legacy) = match util::read_text(&paths.backup_state) {
        Ok(text) => (BackupState::parse(&text)?, text.lines().next() == Some(LEGACY_PROVIDER_STATE_HEADER)),
        Err(_) if !paths.marker.exists() => return Ok(ProviderRestoreOutcome::NotManaged),
        Err(error) => return Err(format!("读取 Scene 回调备份失败: {error}")),
    };
    let json_restored = restore_target(
        &paths.native_json,
        &paths.backup_json,
        state.json,
        is_own_json,
    )?;
    let script_restored = restore_target(
        &paths.native_script,
        &paths.backup_script,
        state.script,
        is_own_script,
    )?;
    let base_restored = if legacy { true } else {
        restore_target(&paths.native_base, &paths.backup_base, state.base, is_own_script)?
    };
    if !json_restored || !script_restored || !base_restored {
        return Ok(ProviderRestoreOutcome::SkippedChanged);
    }
    for path in [
        &paths.marker,
        &paths.backup_state,
        &paths.backup_json,
        &paths.backup_script,
        &paths.backup_base,
    ] {
        if let Err(error) = fs::remove_file(path) {
            if error.kind() != std::io::ErrorKind::NotFound {
                return Err(format!(
                    "清理 Scene 回调备份 {} 失败: {error}",
                    path.display()
                ));
            }
        }
    }
    Ok(ProviderRestoreOutcome::Restored)
}

fn restore_target(
    target: &Path,
    backup: &Path,
    slot: BackupSlot,
    is_own: impl Fn(&str) -> bool,
) -> Result<bool> {
    if !target.exists() {
        return Ok(matches!(slot, BackupSlot::Absent));
    }
    let current = util::read_text(target)?;
    if !is_own(&current) {
        return match slot {
            // A previous installation attempt may already have restored this
            // file before returning an error. Treat an exact backup match as
            // safely restored instead of blocking all later retries.
            BackupSlot::Present(_) => Ok(util::read_bytes(backup)? == current.as_bytes()),
            BackupSlot::Absent => Ok(false),
        };
    }
    match slot {
        BackupSlot::Absent => fs::remove_file(target)
            .map(|_| true)
            .map_err(|error| format!("删除 CTS Scene 回调 {} 失败: {error}", target.display())),
        BackupSlot::Present(mode) => {
            let bytes = util::read_bytes(backup)?;
            util::atomic_write(target, &bytes, mode)?;
            Ok(true)
        }
    }
}

#[derive(Debug)]
struct XmlEntry {
    start: usize,
    end: usize,
    name: String,
    value: String,
}

fn merge_xml_rules(xml: &str, rules: &Rules) -> Result<String> {
    let entries = xml_entries(xml);
    let mut merged = String::with_capacity(xml.len() + rules.len() * 80);
    let mut copied = 0usize;
    let mut written = BTreeSet::new();
    for entry in entries {
        merged.push_str(&xml[copied..entry.start]);
        if let Some(mode) = rules.get(&entry.name) {
            append_xml_entry(&mut merged, &entry.name, mode);
            written.insert(entry.name);
        } else if !app_modes::supported(&entry.value) || !valid_rule_name(&entry.name) {
            merged.push_str(&xml[entry.start..entry.end]);
        }
        copied = entry.end;
    }
    let map_end = xml[copied..]
        .find("</map>")
        .map(|offset| copied + offset)
        .ok_or_else(|| "Scene powercfg.xml 缺少 </map>".to_string())?;
    merged.push_str(&xml[copied..map_end]);
    if !merged.ends_with('\n') {
        merged.push('\n');
    }
    for (package, mode) in rules {
        if written.contains(package) {
            continue;
        }
        append_xml_entry(&mut merged, package, mode);
        merged.push('\n');
    }
    merged.push_str(&xml[map_end..]);
    Ok(merged)
}

fn xml_entries(xml: &str) -> Vec<XmlEntry> {
    let mut entries = Vec::new();
    let mut cursor = 0usize;
    while let Some(relative_start) = xml[cursor..].find("<string") {
        let start = cursor + relative_start;
        let Some(relative_tag_end) = xml[start + 7..].find('>') else {
            break;
        };
        let tag_end = start + 7 + relative_tag_end;
        let Some(relative_end) = xml[tag_end + 1..].find("</string>") else {
            break;
        };
        let end = tag_end + 1 + relative_end + "</string>".len();
        let tag = &xml[start..=tag_end];
        if let Some(name) = xml_name_attribute(tag) {
            entries.push(XmlEntry {
                start,
                end,
                name: xml_unescape(name),
                value: xml_unescape(&xml[tag_end + 1..tag_end + 1 + relative_end]),
            });
        }
        cursor = end;
    }
    entries
}

fn valid_rule_name(value: &str) -> bool {
    value == "*" || util::valid_package(value)
}

fn xml_name_attribute(tag: &str) -> Option<&str> {
    let name_start = tag.find("name=")? + "name=".len();
    let quote = *tag.as_bytes().get(name_start)?;
    if quote != b'\'' && quote != b'"' {
        return None;
    }
    let value_start = name_start + 1;
    let relative_end = tag[value_start..].find(quote as char)?;
    Some(&tag[value_start..value_start + relative_end])
}

fn append_xml_entry(output: &mut String, package: &str, mode: &str) {
    output.push_str("<string name=\"");
    output.push_str(&xml_escape(package));
    output.push_str("\">");
    output.push_str(&xml_escape(mode));
    output.push_str("</string>");
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn xml_unescape(value: &str) -> String {
    value
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn residual_provider_does_not_claim_uninstalled_scene() {
        for registered in [ProviderKind::Own, ProviderKind::Foreign] {
            assert_eq!(
                active_provider(registered, SceneAvailability::Absent),
                ProviderKind::None
            );
        }
        assert_eq!(
            active_provider(ProviderKind::Own, SceneAvailability::Available),
            ProviderKind::Own
        );
        assert_eq!(
            active_provider(ProviderKind::Own, SceneAvailability::Unknown),
            ProviderKind::None
        );
        assert_eq!(
            active_provider(ProviderKind::Foreign, SceneAvailability::Unknown),
            ProviderKind::Foreign
        );
    }

    #[test]
    fn scene_presence_accepts_the_omarea_family_but_not_lookalikes() {
        assert_eq!(
            parse_package_query("package:com.omarea.vtools\n"),
            SceneAvailability::Available
        );
        // N1 era builds may ship under a renamed id inside the family.
        assert_eq!(
            parse_package_query("package:com.omarea.scene\npackage:com.omarea.scene9.free"),
            SceneAvailability::Available
        );
        assert_eq!(parse_package_query(""), SceneAvailability::Absent);
        // Same-developer packages that are not Scene do not prove presence,
        // and a longer lookalike of the classic id is not Scene either.
        assert_eq!(
            parse_package_query("package:com.omarea.other.tool"),
            SceneAvailability::Absent
        );
        assert_eq!(
            parse_package_query("package:com.omarea.vtools.fake"),
            SceneAvailability::Absent
        );
        assert_eq!(
            parse_package_query("Error: package service not ready"),
            SceneAvailability::Unknown
        );
    }

    #[test]
    fn current_user_uninstall_is_authoritative_without_global_records() {
        let mut calls = 0;
        let p = probe_presence(Ok(10), |_, args| {
            calls += 1;
            assert!(args.windows(2).any(|pair| pair == ["--user", "10"]));
            assert!(!args.contains(&"-u") && !args.contains(&"-e"));
            Ok(String::new())
        });
        assert_eq!(calls, 4);
        assert_eq!(p.availability, SceneAvailability::Absent);
        assert_eq!(
            active_provider(ProviderKind::Own, p.availability),
            ProviderKind::None
        );
    }

    #[test]
    fn malformed_primary_query_uses_current_user_fallback() {
        let mut calls = 0;
        let p = probe_presence(Ok(0), |program, args| {
            calls += 1;
            assert!(args.windows(2).any(|pair| pair == ["--user", "0"]));
            Ok(if program.ends_with("/cmd") {
                "Error: package service not ready".into()
            } else {
                format!("package:{SCENE_PACKAGE}\n")
            })
        });
        assert_eq!(calls, 2);
        assert_eq!(p.availability, SceneAvailability::Available);
    }

    #[test]
    fn query_failures_remain_unknown_even_with_leftover_provider() {
        let p = probe_presence(Ok(0), |_, _| Err("binder denied".into()));
        assert_eq!(p.availability, SceneAvailability::Unknown);
        assert!(p.evidence.contains("binder denied"));
        assert_eq!(
            active_provider(ProviderKind::Own, p.availability),
            ProviderKind::None
        );
        assert_eq!(
            active_provider(ProviderKind::Foreign, p.availability),
            ProviderKind::Foreign
        );
        let p = probe_presence(Err("user unknown".into()), |_, _| {
            panic!("must not query user 0")
        });
        assert_eq!(p.availability, SceneAvailability::Unknown);
    }

    #[test]
    fn missing_callback_is_retried_while_scene_remains_installed() {
        let installed = SceneState {
            availability: SceneAvailability::Available,
            ..Default::default()
        };
        assert!(registration_needed(&installed, &installed));
        assert!(!registration_needed(&SceneState::default(), &installed));
        let linked = SceneState {
            provider: ProviderKind::Own,
            ..installed.clone()
        };
        assert!(!registration_needed(&linked, &linked));
        let foreign = SceneState {
            provider: ProviderKind::Foreign,
            ..installed.clone()
        };
        assert!(registration_needed(&foreign, &foreign));
    }

    #[test]
    fn empty_primary_reply_does_not_hide_an_installed_scene() {
        let mut calls = 0;
        let found = probe_presence(Ok(0), |program, _| {
            calls += 1;
            Ok(if program.ends_with("/cmd") { "".into() } else { "package:com.omarea.vtools\n".into() })
        });
        assert_eq!(calls, 2);
        assert_eq!(found.availability, SceneAvailability::Available);
        let found = probe_presence(Ok(0), |_, args| {
            if args.contains(&"path") { Ok("package:/data/app/Scene/base.apk".into()) }
            else { Err("list unavailable".into()) }
        });
        assert_eq!(found.availability, SceneAvailability::Available);
    }

    #[test]
    fn registry_fallback_respects_per_user_uninstall_and_disabled_apps() {
        let global = "com.omarea.vtools 10234 0 /data/user/0/com.omarea.vtools default none\n";
        assert_eq!(registry_presence(global, None, None).availability, SceneAvailability::Available);
        let xml = "<package-restrictions><pkg name=\"com.omarea.vtools\" installed=\"false\" /></package-restrictions>";
        assert_eq!(registry_presence(global, Some(xml), Some(10)).availability, SceneAvailability::Absent);
        let xml = "<package-restrictions><pkg name=\"com.omarea.vtools\" enabled=\"3\" /></package-restrictions>";
        assert_eq!(registry_presence(global, Some(xml), Some(0)).availability, SceneAvailability::Available);
        assert_eq!(registry_presence(global, Some("<package-restrictions>"), Some(0)).availability, SceneAvailability::Unknown);
        assert_eq!(parse_package_query("package:com.omarea.sceneFake"), SceneAvailability::Absent);
    }

    struct Fixture(ProviderPaths);
    impl Fixture {
        fn new() -> Self {
            use std::sync::atomic::{AtomicU64, Ordering};
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let root = std::env::temp_dir().join(format!(
                "nova-scene-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&root).unwrap();
            let mut paths = ProviderPaths::new();
            paths.dir = root.clone();
            paths.lock = root.join("lock");
            paths.marker = root.join("managed");
            paths.backup_state = root.join("backup-state");
            paths.backup_json = root.join("json.backup");
            paths.backup_script = root.join("script.backup");
            paths.backup_base = root.join("base.backup");
            paths.native_json = root.join("powercfg.json");
            paths.native_script = root.join("powercfg.sh");
            paths.native_base = root.join("powercfg-base.sh");
            Self(paths)
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0.dir);
        }
    }
    fn templates() -> ProviderTemplates {
        ProviderTemplates {
            json: br#"{"module":"NovaSched_Zen_Edition"}"#.to_vec(),
            script: b"#!/system/bin/sh\n# NovaSched_Zen_Edition_Scene_Provider\n".to_vec(),
            base: b"#!/system/bin/sh\n# NovaSched_Zen_Edition_Scene_Provider\nexit 0\n".to_vec(),
        }
    }

    #[test]
    fn uninstall_removes_orphaned_own_callbacks_without_markers() {
        let fixture = Fixture::new();
        let paths = &fixture.0;
        templates().write_to_targets(paths).unwrap();
        assert!(uninstall_provider_at(paths).unwrap().is_empty());
        assert!(!paths.native_json.exists() && !paths.native_script.exists());
        assert!(uninstall_provider_at(paths).unwrap().is_empty());
    }
    #[test]
    fn uninstall_restores_saved_bytes_and_permissions_before_state_cleanup() {
        let fixture = Fixture::new();
        let paths = &fixture.0;
        let original_json = b"original JSON bytes";
        let original_script = b"original script bytes";
        fs::write(&paths.backup_json, original_json).unwrap();
        fs::write(&paths.backup_script, original_script).unwrap();
        let state = BackupState {
            json: BackupSlot::Present(0o640),
            script: BackupSlot::Present(0o750),
            base: BackupSlot::Absent,
        };
        fs::write(&paths.backup_state, state.encode()).unwrap();
        templates().write_to_targets(paths).unwrap();
        assert!(uninstall_provider_at(paths).unwrap().is_empty());
        assert_eq!(fs::read(&paths.native_json).unwrap(), original_json);
        assert_eq!(fs::read(&paths.native_script).unwrap(), original_script);
        assert_eq!(
            fs::metadata(&paths.native_json).unwrap().mode() & 0o7777,
            0o640
        );
        assert_eq!(
            fs::metadata(&paths.native_script).unwrap().mode() & 0o7777,
            0o750
        );
    }
    #[test]
    fn uninstall_handles_half_replaced_provider_without_erasing_foreign_files() {
        let fixture = Fixture::new();
        let paths = &fixture.0;
        install_provider_at(paths, &templates()).unwrap();
        fs::write(&paths.native_script, b"# user's new scheduler\n").unwrap();
        uninstall_provider_at(paths).unwrap();
        assert!(!paths.native_json.exists());
        assert_eq!(
            fs::read(&paths.native_script).unwrap(),
            b"# user's new scheduler\n"
        );
        fs::write(
            &paths.native_json,
            b"{\"module\":\"other\",\"note\":\"NovaSched_Zen_Edition\"}",
        )
        .unwrap();
        uninstall_provider_at(paths).unwrap();
        assert!(paths.native_json.exists() && paths.native_script.exists());
    }

    #[test]
    fn uninstall_does_not_resurrect_our_callback_from_an_old_fragment_backup() {
        let fixture = Fixture::new();
        let paths = &fixture.0;
        fs::write(&paths.native_json, &templates().json).unwrap();
        install_provider_at(paths, &templates()).unwrap();
        let state = BackupState::parse(&fs::read_to_string(&paths.backup_state).unwrap()).unwrap();
        assert!(matches!(state.json, BackupSlot::Present(_)));
        uninstall_provider_at(paths).unwrap();
        assert!(!paths.native_json.exists() && !paths.native_script.exists());
        assert!(!is_own_script(
            "echo NovaSched_Zen_Edition_Scene_Provider\n"
        ));
    }
    #[test]
    fn uninstall_cleans_own_callbacks_after_invalid_or_missing_backup() {
        let fixture = Fixture::new();
        let paths = &fixture.0;
        for state in [
            "broken".to_string(),
            BackupState {
                json: BackupSlot::Present(0o644),
                script: BackupSlot::Absent,
                base: BackupSlot::Absent,
            }
            .encode(),
        ] {
            templates().write_to_targets(paths).unwrap();
            fs::write(&paths.backup_state, state).unwrap();
            let warnings = uninstall_provider_at(paths).unwrap();
            assert!(!warnings.is_empty());
            assert!(!paths.native_json.exists() && !paths.native_script.exists());
        }
    }

    #[test]
    fn cleared_managed_callbacks_are_recreated_and_restorable() {
        let fixture = Fixture::new();
        let paths = &fixture.0;
        assert_eq!(
            install_provider_at(paths, &templates()).unwrap(),
            ProviderInstallOutcome::Installed
        );
        let backup = fs::read(&paths.backup_state).unwrap();
        fs::remove_file(&paths.native_json).unwrap();
        assert_eq!(detect_provider_at(paths), ProviderKind::None);
        assert_eq!(
            install_provider_at(paths, &templates()).unwrap(),
            ProviderInstallOutcome::Refreshed
        );
        assert_eq!(detect_provider_at(paths), ProviderKind::Own);
        fs::remove_file(&paths.native_json).unwrap();
        fs::remove_file(&paths.native_script).unwrap();
        assert_eq!(
            install_provider_at(paths, &templates()).unwrap(),
            ProviderInstallOutcome::Refreshed
        );
        assert_eq!(fs::read(&paths.backup_state).unwrap(), backup);
        assert_eq!(
            restore_locked(paths).unwrap(),
            ProviderRestoreOutcome::Restored
        );
        assert!(!paths.native_json.exists() && !paths.native_script.exists());
    }

    #[test]
    fn scene_replacing_our_callback_is_reclaimed_without_losing_first_backup() {
        let fixture = Fixture::new();
        let paths = &fixture.0;
        install_provider_at(paths, &templates()).unwrap();
        fs::remove_file(&paths.native_json).unwrap();
        fs::write(&paths.native_script, "# another scheduler\n").unwrap();
        assert_eq!(detect_provider_at(paths), ProviderKind::Foreign);
        assert_eq!(
            install_provider_at(paths, &templates()).unwrap(),
            ProviderInstallOutcome::Refreshed
        );
        assert_eq!(detect_provider_at(paths), ProviderKind::Own);
        restore_locked(paths).unwrap();
        assert!(!paths.native_json.exists());
        assert!(!paths.native_script.exists() && !paths.native_base.exists());
    }

    #[test]
    fn callback_repair_requires_a_valid_recovery_record() {
        let fixture = Fixture::new();
        let paths = &fixture.0;
        install_provider_at(paths, &templates()).unwrap();
        fs::remove_file(&paths.native_json).unwrap();
        fs::write(&paths.backup_state, "invalid backup\n").unwrap();
        assert!(install_provider_at(paths, &templates()).is_err());
        assert!(!paths.native_json.exists());
    }

    #[test]
    fn install_remove_reinstall_transfers_control_without_trusting_residual_files() {
        let fixture = Fixture::new();
        let paths = &fixture.0;
        install_provider_at(paths, &templates()).unwrap();
        for (package, expected) in [
            (SCENE_PACKAGE, "Scene（NovaSched Zen Edition）"),
            ("com.other.app", "WebUI"),
            (SCENE_PACKAGE, "Scene（NovaSched Zen Edition）"),
        ] {
            let presence = probe_presence(Ok(0), |_, _| Ok(format!("package:{package}\n")));
            let state = SceneState {
                availability: presence.availability,
                provider: active_provider(detect_provider_at(paths), presence.availability),
                ..Default::default()
            };
            assert_eq!(state.controller(), expected);
            assert_eq!(state.linked(), package == SCENE_PACKAGE);
        }
        // The old callbacks deliberately survive an uninstall for restoration.
        assert_eq!(detect_provider_at(paths), ProviderKind::Own);
    }

    #[test]
    fn recognizes_own_provider_without_matching_other_modules() {
        assert!(is_own_json(r#"{"module":"NovaSched_Zen_Edition"}"#));
        assert!(!is_own_json(r#"{"module":"Scene_HP"}"#));
        assert!(is_own_script("# NovaSched_Zen_Edition_Scene_Provider\n"));
    }

    #[test]
    fn reads_native_scene_mode_without_assuming_full_schema() {
        assert_eq!(
            parse_json_mode(r#"{"profile":{"default_mode":"performance"}}"#),
            Some("performance".to_string())
        );
        assert_eq!(
            parse_script_mode("# Scene\ndefaultMode='fast'\n"),
            Some("fast".to_string())
        );
    }

    #[test]
    fn reads_legacy_scene_default() {
        assert_eq!(
            parse_xml_mode(r#"<map><string name="*">balance</string></map>"#),
            Some("balance".to_string())
        );
    }

    #[test]
    fn merges_only_scheduler_entries_and_preserves_other_scene_values() {
        let xml = "<map>\n<string name=\"*\">powersave</string>\n<string name=\"com.old.game\">fast</string>\n<string name=\"theme\">purple</string>\n</map>\n";
        let mut rules = Rules::new();
        rules.insert("*".to_string(), "balance".to_string());
        rules.insert("com.new.game".to_string(), "performance".to_string());
        let merged = merge_xml_rules(xml, &rules).expect("merge XML");
        assert!(merged.contains("<string name=\"*\">balance</string>"));
        assert!(merged.contains("<string name=\"com.new.game\">performance</string>"));
        assert!(merged.contains("<string name=\"theme\">purple</string>"));
        assert!(!merged.contains("com.old.game"));
    }

    #[test]
    fn parses_backup_state() {
        let state = BackupState::parse(
            "NOVASCHED_ZEN_SCENE_PROVIDER_V1\njson=present:644\nscript=absent\n",
        )
        .expect("backup state");
        assert_eq!(state.json, BackupSlot::Present(0o644));
        assert_eq!(state.script, BackupSlot::Absent);
    }
    #[test]
    fn scene_lp_is_backed_up_replaced_and_restored_with_its_base_script() {
        let fixture = Fixture::new(); let paths = &fixture.0;
        let original = [
            (&paths.native_json, b"{\"name\":\"Scene LP\"}".as_slice(), 0o644),
            (&paths.native_script, b"#!/system/bin/sh\n# Scene LP\n".as_slice(), 0o755),
            (&paths.native_base, b"#!/system/bin/sh\n# old initialization\n".as_slice(), 0o750),
        ];
        for (path, bytes, mode) in original { util::atomic_write(path, bytes, mode).unwrap(); }
        assert_eq!(install_provider_at(paths, &templates()).unwrap(), ProviderInstallOutcome::Installed);
        assert_eq!(detect_provider_at(paths), ProviderKind::Own);
        let backup = fs::read(&paths.backup_state).unwrap();
        fs::write(&paths.native_base, "# LP init restored\n").unwrap();
        assert_eq!(detect_provider_at(paths), ProviderKind::Foreign);
        assert_eq!(install_provider_at(paths, &templates()).unwrap(), ProviderInstallOutcome::Refreshed);
        assert_eq!(fs::read(&paths.backup_state).unwrap(), backup);
        assert_eq!(restore_locked(paths).unwrap(), ProviderRestoreOutcome::Restored);
        for (path, bytes, mode) in original {
            assert_eq!(fs::read(path).unwrap(), bytes);
            assert_eq!(fs::metadata(path).unwrap().mode() & 0o7777, mode);
        }
    }
    #[test]
    fn legacy_two_file_backup_acquires_base_backup_before_refresh() {
        let fixture = Fixture::new(); let paths = &fixture.0;
        fs::write(&paths.backup_state, "NOVASCHED_ZEN_SCENE_PROVIDER_V1\njson=absent\nscript=absent\n").unwrap();
        fs::write(&paths.marker, "managed=1\n").unwrap();
        fs::write(&paths.native_base, "# original base\n").unwrap();
        assert_eq!(install_provider_at(paths, &templates()).unwrap(), ProviderInstallOutcome::Refreshed);
        assert!(fs::read_to_string(&paths.backup_state).unwrap().starts_with(PROVIDER_STATE_HEADER));
        uninstall_provider_at(paths).unwrap();
        assert_eq!(fs::read_to_string(&paths.native_base).unwrap(), "# original base\n");
        assert!(!paths.native_json.exists() && !paths.native_script.exists());
    }
    #[test]
    fn staged_install_uses_the_post_reboot_module_path() {
        for prefix in ["/data/adb", "/data/adb/ap"] {
            let staged = PathBuf::from(format!("{prefix}/modules_update/{OWN_PROVIDER_MODULE}"));
            let active = PathBuf::from(format!("{prefix}/modules/{OWN_PROVIDER_MODULE}"));
            assert_eq!(active_module_path(&staged), active); assert_eq!(active_module_path(&active), active);
        }
    }

}
