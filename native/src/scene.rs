use std::collections::BTreeSet;
use std::fs;
use std::io::Read;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::app_modes::{self, Rules};
use crate::json::{self, Value};
use crate::util::{self, Result};

pub const OWN_PROVIDER_MODULE: &str = "NovaSched_Zen_Edition";
pub const SCENE_PACKAGE: &str = "com.omarea.vtools";
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

/// Android regenerates packages.list from its installed package registry.
/// Unlike an app data directory or our callback, it is installation evidence.
/// Do not filter enabled apps or user 0: Scene need not currently be running.
pub fn presence() -> ScenePresence {
    let cache = AVAILABILITY.get_or_init(|| Mutex::new(None));
    if let Ok(cached) = cache.lock() {
        if let Some((at, state)) = cached.as_ref() {
            if at.elapsed() < PRESENCE_INTERVAL {
                return state.clone();
            }
        }
    }
    let registry = read_package_registry(Path::new("/data/system/packages.list"));
    let xml = crate::package_registry::installed_at(
        Path::new("/data/system/packages.xml"),
        SCENE_PACKAGE,
    );
    let state = resolve_presence(
        registry,
        xml,
        || crate::package_registry::process_running_at(Path::new("/proc"), SCENE_PACKAGE),
        |program, args| util::query_command(program, args, Duration::from_secs(2)),
    );
    if let Ok(mut cached) = cache.lock() {
        *cached = Some((Instant::now(), state.clone()));
    }
    state
}

fn resolve_presence(
    registry: Result<String>,
    xml: Result<bool>,
    process: impl FnOnce() -> Result<bool>,
    query: impl FnMut(&str, &[&str]) -> Result<String>,
) -> ScenePresence {
    let listed = registry
        .as_ref()
        .map(|text| parse_package_registry(text))
        .unwrap_or(SceneAvailability::Unknown);
    if listed == SceneAvailability::Available || xml == Ok(true) {
        return ScenePresence {
            availability: SceneAvailability::Available,
            evidence: if listed == SceneAvailability::Available {
                "系统 packages.list 已登记 Scene".into()
            } else {
                "系统 packages.xml 已登记 Scene（文本/ABX）".into()
            },
        };
    }
    // Complete, readable registries also provide authoritative absence.
    // A still-exiting process must not keep Scene ownership after uninstall.
    if listed == SceneAvailability::Absent || xml == Ok(false) {
        return ScenePresence {
            availability: SceneAvailability::Absent,
            evidence: "完整系统安装注册表中没有 Scene；忽略旧回调和数据目录".into(),
        };
    }
    let process = process();
    if process == Ok(true) {
        return ScenePresence {
            availability: SceneAvailability::Available,
            evidence: "安装注册表不可读；检测到正在运行的 Scene 本体/子进程".into(),
        };
    }
    let mut result = probe_presence(registry, query);
    if result.availability == SceneAvailability::Unknown {
        if let Err(error) = xml {
            result.evidence.push_str(&format!("；{error}"));
        }
        if let Err(error) = process {
            result.evidence.push_str(&format!("；{error}"));
        }
    }
    result
}

fn read_package_registry(path: &Path) -> Result<String> {
    let mut text = String::new();
    fs::File::open(path)
        .and_then(|file| file.take(2_097_153).read_to_string(&mut text))
        .map_err(|error| format!("读取系统安装记录失败: {error}"))?;
    if text.len() > 2_097_152 {
        return Err("系统安装记录超出限制".into());
    }
    Ok(text)
}

fn parse_package_registry(text: &str) -> SceneAvailability {
    let mut found = false;
    let mut rows = 0;
    for line in text.lines().filter(|line| !line.trim().is_empty()) {
        let mut fields = line.split_whitespace();
        let (Some(name), Some(uid), Some(debug), Some(data), Some(_seinfo), Some(_gids)) = (
            fields.next(),
            fields.next(),
            fields.next(),
            fields.next(),
            fields.next(),
            fields.next(),
        ) else {
            return SceneAvailability::Unknown;
        };
        if uid.parse::<u32>().is_err() || !matches!(debug, "0" | "1") || !data.starts_with('/') {
            return SceneAvailability::Unknown;
        }
        found |= name == SCENE_PACKAGE;
        rows += 1;
    }
    if found {
        SceneAvailability::Available
    } else if rows > 0 {
        SceneAvailability::Absent
    } else {
        SceneAvailability::Unknown
    }
}

fn probe_presence(
    registry: Result<String>,
    mut query: impl FnMut(&str, &[&str]) -> Result<String>,
) -> ScenePresence {
    let registry_state = registry
        .as_ref()
        .map(|text| parse_package_registry(text))
        .unwrap_or(SceneAvailability::Unknown);
    if registry_state == SceneAvailability::Available {
        return ScenePresence {
            availability: registry_state,
            evidence: "系统 packages.list 已登记 Scene".into(),
        };
    }
    let mut absent_queries = 0;
    let mut failures = Vec::new();
    for (program, args) in [
        (
            "/system/bin/cmd",
            vec!["package", "list", "packages", SCENE_PACKAGE],
        ),
        ("/system/bin/pm", vec!["list", "packages", SCENE_PACKAGE]),
    ] {
        match query(program, &args) {
            Ok(text) => match parse_package_query(&text) {
                SceneAvailability::Available => {
                    return ScenePresence {
                        availability: SceneAvailability::Available,
                        evidence: format!("{program} 已登记 Scene（不限制启用状态或用户 0）"),
                    }
                }
                SceneAvailability::Absent => absent_queries += 1,
                SceneAvailability::Unknown => {
                    failures.push(format!("{program} 返回无法识别的结果"))
                }
            },
            Err(error) => failures.push(error),
        }
    }
    if registry_state == SceneAvailability::Absent || absent_queries == 2 {
        return ScenePresence {
            availability: SceneAvailability::Absent,
            evidence: "系统安装记录中没有 Scene；未使用残留回调或数据目录判断".into(),
        };
    }
    if let Err(error) = registry {
        failures.push(error);
    }
    ScenePresence {
        availability: SceneAvailability::Unknown,
        evidence: format!("Scene 安装状态查询失败: {}", failures.join("；")),
    }
}

fn parse_package_query(text: &str) -> SceneAvailability {
    if text
        .lines()
        .any(|line| line.trim() == format!("package:{SCENE_PACKAGE}"))
    {
        SceneAvailability::Available
    } else if text
        .lines()
        .all(|line| line.trim().is_empty() || line.trim().starts_with("package:"))
    {
        SceneAvailability::Absent
    } else {
        SceneAvailability::Unknown
    }
}

const NATIVE_JSON: &str = "/data/powercfg.json";
const NATIVE_SCRIPT: &str = "/data/powercfg.sh";
const OWN_SCRIPT_MAGIC: &str = "NovaSched_Zen_Edition_Scene_Provider";
const PROVIDER_STATE_DIR: &str = "/data/adb/novasched/scene-provider";
const PROVIDER_STATE_HEADER: &str = "NOVASCHED_ZEN_SCENE_PROVIDER_V1";
const VTOOLS_XML: [&str; 2] = [
    "/data/user/0/com.omarea.vtools/shared_prefs/powercfg.xml",
    "/data/user_de/0/com.omarea.vtools/shared_prefs/powercfg.xml",
];

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

pub fn watched_paths() -> Vec<&'static str> {
    let mut paths = vec![
        NATIVE_JSON,
        NATIVE_SCRIPT,
        "/data/system/packages.list",
        "/data/system/users/0/package-restrictions.xml",
        "/data/system/packages.xml",
    ];
    paths.extend(VTOOLS_XML);
    paths
}

pub fn registration_needed(state: &SceneState, previous: &SceneState) -> bool {
    state.availability == SceneAvailability::Available
        && (previous.availability != SceneAvailability::Available
            || state.provider == ProviderKind::None)
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
        return match detect_provider_at(paths) {
            ProviderKind::Own | ProviderKind::None => {
                validate_provider_backup(paths)?;
                templates.write_to_targets(paths)?;
                Ok(ProviderInstallOutcome::Refreshed)
            }
            ProviderKind::Foreign => Ok(ProviderInstallOutcome::SkippedForeign),
        };
    }

    // Never take ownership of a Scene callback that existed before this
    // module had a verified backup. That callback may belong to another
    // scheduler, including an older installed module version.
    match detect_provider_at(paths) {
        ProviderKind::Foreign => return Ok(ProviderInstallOutcome::SkippedForeign),
        ProviderKind::Own => return Ok(ProviderInstallOutcome::SkippedExisting),
        ProviderKind::None => {}
    }

    let backup = BackupState {
        json: backup_target(&paths.native_json, &paths.backup_json)?,
        script: backup_target(&paths.native_script, &paths.backup_script)?,
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
    let _lock = util::acquire_flock(&Path::new(PROVIDER_STATE_DIR).join("rules.lock"))?;
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
        (Ok(json), Ok(script)) if is_own_json(&json) && is_own_script(&script) => ProviderKind::Own,
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
    VTOOLS_XML
        .iter()
        .map(PathBuf::from)
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
    text.contains(OWN_SCRIPT_MAGIC)
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
}

impl BackupState {
    fn encode(self) -> String {
        format!(
            "{PROVIDER_STATE_HEADER}\njson={}\nscript={}\n",
            self.json.encode(),
            self.script.encode()
        )
    }

    fn parse(text: &str) -> Result<Self> {
        let mut lines = text.lines();
        if lines.next() != Some(PROVIDER_STATE_HEADER) {
            return Err("Scene 回调备份头无效".to_string());
        }
        let mut json = None;
        let mut script = None;
        for line in lines {
            let Some((key, value)) = line.split_once('=') else {
                return Err("Scene 回调备份格式无效".to_string());
            };
            match key {
                "json" => json = Some(BackupSlot::parse(value)?),
                "script" => script = Some(BackupSlot::parse(value)?),
                _ => return Err("Scene 回调备份包含未知字段".to_string()),
            }
        }
        Ok(Self {
            json: json.ok_or_else(|| "Scene 回调备份缺少 JSON 状态".to_string())?,
            script: script.ok_or_else(|| "Scene 回调备份缺少脚本状态".to_string())?,
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
    native_json: PathBuf,
    native_script: PathBuf,
}

impl ProviderPaths {
    fn new() -> Self {
        let dir = PathBuf::from(PROVIDER_STATE_DIR);
        Self {
            lock: dir.join("lock"),
            marker: dir.join("managed"),
            backup_state: dir.join("backup-state"),
            backup_json: dir.join("powercfg.json.backup"),
            backup_script: dir.join("powercfg.sh.backup"),
            native_json: PathBuf::from(NATIVE_JSON),
            native_script: PathBuf::from(NATIVE_SCRIPT),
            dir,
        }
    }
}

struct ProviderTemplates {
    json: Vec<u8>,
    script: Vec<u8>,
}

impl ProviderTemplates {
    fn load(module_dir: &Path) -> Result<Self> {
        let json = util::read_bytes(&module_dir.join("vtools/powercfg.json"))?;
        let template = util::read_text(&module_dir.join("vtools/powercfg.sh"))?;
        let resolved = module_dir
            .canonicalize()
            .map_err(|e| format!("确定 Scene 模块目录失败: {e}"))?;
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
        Ok(Self { json, script })
    }

    fn write_to_targets(&self, paths: &ProviderPaths) -> Result<()> {
        // Script first: the JSON entry is written last, so Scene never sees an
        // entry that points to a missing CTS callback.
        util::atomic_write(&paths.native_script, &self.script, 0o755)?;
        util::atomic_write(&paths.native_json, &self.json, 0o644)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProviderInstallOutcome {
    Installed,
    Refreshed,
    SkippedForeign,
    SkippedExisting,
    SkippedMissing,
}

impl std::fmt::Display for ProviderInstallOutcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let text = match self {
            Self::Installed => "Scene 已注册 NovaSched Zen Edition（已备份原回调）",
            Self::Refreshed => "Scene CTS 回调已刷新",
            Self::SkippedForeign => "检测到用户改用其它 Scene 调度器，未覆盖",
            Self::SkippedExisting => "检测到未由当前安装备份的 Scene 回调，未覆盖",
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
    ] {
        if matches!(slot, BackupSlot::Present(_)) {
            util::read_bytes(path)?;
        }
    }
    Ok(())
}

fn restore_locked(paths: &ProviderPaths) -> Result<ProviderRestoreOutcome> {
    let state = match util::read_text(&paths.backup_state) {
        Ok(text) => BackupState::parse(&text)?,
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
    if !json_restored || !script_restored {
        return Ok(ProviderRestoreOutcome::SkippedChanged);
    }
    for path in [
        &paths.marker,
        &paths.backup_state,
        &paths.backup_json,
        &paths.backup_script,
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
    fn scene_presence_requires_exact_installed_package() {
        assert_eq!(
            parse_package_query("package:com.omarea.vtools\n"),
            SceneAvailability::Available
        );
        assert_eq!(parse_package_query(""), SceneAvailability::Absent);
        assert_eq!(
            parse_package_query("package:com.omarea.vtools.fake"),
            SceneAvailability::Absent
        );
        assert_eq!(
            parse_package_query("Error: package service not ready"),
            SceneAvailability::Unknown
        );
    }

    fn registry(package: &str) -> String {
        format!("{package} 10123 0 /data/user/0/{package} default:targetSdkVersion=35 none 0 100 0 @null\n")
    }

    #[test]
    fn installed_registry_does_not_depend_on_app_running_or_enabled_query() {
        let presence = probe_presence(Ok(registry(SCENE_PACKAGE)), |_, _| {
            panic!("installed record needs no binder query")
        });
        assert_eq!(presence.availability, SceneAvailability::Available);
        assert!(presence.evidence.contains("packages.list"));
        assert_eq!(
            parse_package_registry(&registry("com.omarea.vtools.fake")),
            SceneAvailability::Absent
        );
        assert_eq!(parse_package_registry(""), SceneAvailability::Unknown);
        assert_eq!(
            parse_package_registry("com.omarea.vtools malformed"),
            SceneAvailability::Unknown
        );
    }

    #[test]
    fn empty_first_query_cannot_skip_a_successful_fallback() {
        let mut calls = 0;
        let presence = probe_presence(Err("registry denied".into()), |program, args| {
            calls += 1;
            assert!(!args.contains(&"-e") && !args.contains(&"-u") && !args.contains(&"--user"));
            Ok(if program.ends_with("/cmd") {
                String::new()
            } else {
                format!("package:{SCENE_PACKAGE}\n")
            })
        });
        assert_eq!(calls, 2);
        assert_eq!(presence.availability, SceneAvailability::Available);
    }

    #[test]
    fn unknown_first_query_also_uses_the_fallback() {
        let presence = probe_presence(Err("missing registry".into()), |program, _| {
            Ok(if program.ends_with("/cmd") {
                "Error: package service not ready".into()
            } else {
                format!("package:{SCENE_PACKAGE}\n")
            })
        });
        assert_eq!(presence.availability, SceneAvailability::Available);
    }

    #[test]
    fn live_package_query_overrides_a_registry_update_in_progress() {
        let presence = probe_presence(Ok(registry("com.other.app")), |_, _| {
            Ok(format!("package:{SCENE_PACKAGE}\n"))
        });
        assert_eq!(presence.availability, SceneAvailability::Available);
    }

    #[test]
    fn failed_queries_are_unknown_instead_of_uninstalled() {
        let presence = probe_presence(Err("registry denied".into()), |_, _| {
            Err("binder denied".into())
        });
        assert_eq!(presence.availability, SceneAvailability::Unknown);
        assert!(
            presence.evidence.contains("binder denied")
                && presence.evidence.contains("registry denied")
        );
        let absent = probe_presence(Ok(registry("com.other.app")), |_, _| Ok(String::new()));
        assert_eq!(absent.availability, SceneAvailability::Absent);
        assert_eq!(
            active_provider(ProviderKind::Own, absent.availability),
            ProviderKind::None
        );
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
        assert!(!registration_needed(&foreign, &foreign));
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
            paths.native_json = root.join("powercfg.json");
            paths.native_script = root.join("powercfg.sh");
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
    fn repairing_our_missing_file_never_overwrites_a_foreign_fragment() {
        let fixture = Fixture::new();
        let paths = &fixture.0;
        install_provider_at(paths, &templates()).unwrap();
        fs::remove_file(&paths.native_json).unwrap();
        fs::write(&paths.native_script, "# another scheduler\n").unwrap();
        assert_eq!(detect_provider_at(paths), ProviderKind::Foreign);
        assert_eq!(
            install_provider_at(paths, &templates()).unwrap(),
            ProviderInstallOutcome::SkippedForeign
        );
        assert_eq!(
            fs::read_to_string(&paths.native_script).unwrap(),
            "# another scheduler\n"
        );
        assert!(!paths.native_json.exists());
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
            let presence = probe_presence(Ok(registry(package)), |_, _| Ok(String::new()));
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
}

#[cfg(test)]
mod registry_fallback_tests {
    use super::*;
    fn denied() -> Result<String> {
        Err("SELinux denied".into())
    }
    #[test]
    fn running_scene_recovers_from_binder_failure_and_xml_positive_needs_no_process() {
        let p = resolve_presence(
            denied(),
            Err("XML denied".into()),
            || Ok(true),
            |_, _| panic!("live process avoids Binder"),
        );
        assert_eq!(p.availability, SceneAvailability::Available);
        let p = resolve_presence(
            denied(),
            Ok(true),
            || panic!("XML is enough"),
            |_, _| panic!("XML is enough"),
        );
        assert_eq!(p.availability, SceneAvailability::Available);
    }
    #[test]
    fn uninstall_overrides_still_exiting_process_and_own_callback() {
        let p = resolve_presence(
            denied(),
            Ok(false),
            || panic!("absence does not need process"),
            |_, _| panic!("absence avoids Binder"),
        );
        assert_eq!(p.availability, SceneAvailability::Absent);
        assert_eq!(
            active_provider(ProviderKind::Own, p.availability),
            ProviderKind::None
        );
    }
    #[test]
    fn missing_all_evidence_stays_unknown_and_binder_can_supply_exact_installation() {
        let p = resolve_presence(
            denied(),
            Err("missing".into()),
            || Ok(false),
            |_, _| Err("Failed transaction".into()),
        );
        assert_eq!(p.availability, SceneAvailability::Unknown);
        assert!(p.evidence.contains("Failed transaction"));
        let p = resolve_presence(
            denied(),
            Err("missing".into()),
            || Ok(false),
            |_, _| Ok(format!("package:{SCENE_PACKAGE}")),
        );
        assert_eq!(p.availability, SceneAvailability::Available);
    }
    #[test]
    fn positive_xml_overrides_a_package_list_update_in_progress() {
        let text = "com.other.app 10000 0 /data/user/0/com.other.app default none".into();
        assert_eq!(
            resolve_presence(
                Ok(text),
                Ok(true),
                || Ok(false),
                |_, _| panic!("registry positive")
            )
            .availability,
            SceneAvailability::Available
        );
    }
}
