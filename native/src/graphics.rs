//! ZenJooo · 按应用图形后端规则（ANGLE / 原生驱动）。
//!
//! Android 的 per-app GL 驱动选择由 `GraphicsEnvironment` 在应用进程启动时
//! 读取：`angle_gl_driver_selection_pkgs` 与 `angle_gl_driver_selection_values`
//! 两个逗号列表按下标对应，值为 `angle`（ANGLE 转译）或 `native`（驱动原生
//! GL）。本模块把用户规则写入这两个共享设置：
//! - 只写用户明确配置的应用，不做全局切换；
//! - 记录本模块拥有哪些表项及其旧值，撤销时只处理自己写入且未被外部改动
//!   的部分，不用旧快照覆盖系统当前状态；
//! - ANGLE 驱动包（com.google.android.angle）不存在时能力探测明确报告
//!   不支持——有 Vulkan 驱动不等于有 ANGLE，不伪造可用；
//! - 设置在应用进程启动时读取，规则变更需要目标应用完全退出重开才生效，
//!   正在运行的进程不会被切换，也不被强杀。
//! 注意：这不提供“强制任意游戏用 Vulkan”的能力；Android 没有这样的公开
//! 按应用入口，模块不做做不到的承诺。
use crate::json::{self, Value};
use crate::util::{self, Result};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub const RULES_PATH: &str = "/data/adb/novasched/config/graphics_rules.json";
pub const OWNED_PATH: &str = "/data/adb/novasched/restore/graphics-owned.json";
pub const SETTINGS_NS: &str = "global";
pub const PKGS_KEY: &str = "angle_gl_driver_selection_pkgs";
pub const VALUES_KEY: &str = "angle_gl_driver_selection_values";
pub const ANGLE_PACKAGE: &str = "com.google.android.angle";

/// 用户可为单个应用选择的后端。`default` 表示不干预（从共享列表移除）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Backend {
    Angle,
    Native,
}
impl Backend {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Angle => "angle",
            Self::Native => "native",
        }
    }
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim() {
            "angle" => Some(Self::Angle),
            "native" => Some(Self::Native),
            _ => None,
        }
    }
}

/// 逐应用后端模式（规格 PART 1）：ANGLE 列表的值域只有 angle/native，
/// vulkan 档的诚实落地是能力探测 + 状态机 + 真实原因链（见 capability），
/// 绝不伪装写入。`follow` = 跟随系统（不写入）；`auto` = 同步时解析。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BackendMode {
    Follow,
    Auto,
    Angle,
    Native,
    Vulkan,
}

impl BackendMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Follow => "follow",
            Self::Auto => "auto",
            Self::Angle => "angle",
            Self::Native => "native",
            Self::Vulkan => "vulkan",
        }
    }
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim() {
            "follow" => Some(Self::Follow),
            "auto" => Some(Self::Auto),
            "angle" => Some(Self::Angle),
            "native" => Some(Self::Native),
            "vulkan" => Some(Self::Vulkan),
            _ => None,
        }
    }
    /// ANGLE 列表域投影：只有 angle/native 有真实执行通道。
    fn angle_backend(self) -> Option<Backend> {
        match self {
            Self::Angle => Some(Backend::Angle),
            Self::Native => Some(Backend::Native),
            _ => None,
        }
    }
}

/// 单条规则（schema v2）。`status`/`status_detail` 是**实际生效侧**的
/// 记录（区别于用户请求的 backendMode），由 sync 后写回。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuleEntry {
    pub mode: BackendMode,
    pub enabled: bool,
    pub status: String,
    pub status_detail: String,
}

impl RuleEntry {
    fn new(mode: BackendMode) -> Self {
        Self { mode, enabled: true, status: String::new(), status_detail: String::new() }
    }
}

/// 规则对象的 JSON 序列化（独立成纯函数便于回环测试）。
/// 早期实现误写 `包=后端` 调试格式，重载解析失败会静默丢弃全部规则。
pub fn serialize_rules(entries: &BTreeMap<String, Backend>) -> String {
    let mut items = String::new();
    for (index, (package, backend)) in entries.iter().enumerate() {
        if index > 0 {
            items.push(',');
        }
        items.push_str(&format!("\"{}\": \"{}\"", package, backend.as_str()));
    }
    format!("{{{items}}}")
}

/// 规则存储：包名 → 条目（schema v2；v1 字符串值原位迁移）。
#[derive(Clone, Default)]
pub struct Rules {
    entries: BTreeMap<String, RuleEntry>,
}

/// set/remove 的读-改-写持锁（审计 F1，与 fps_rules::set 同因）：并发
/// CLI 进程无锁时后写者覆盖前写者。有界重试与 web_auth::register_origin
/// 同一模式。
const RULES_LOCK: &str = "/data/adb/novasched/run/graphics-rules.lock";

fn with_rules_lock<T>(operation: impl FnOnce() -> Result<T>) -> Result<T> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(500);
    let _lock = loop {
        match util::acquire_flock(std::path::Path::new(RULES_LOCK)) {
            Ok(lock) => break lock,
            Err(_) if std::time::Instant::now() < deadline => {
                util::sleep(std::time::Duration::from_millis(5))
            }
            Err(error) => return Err(error),
        }
    };
    operation()
}

impl Rules {
    pub fn load() -> Self {
        match util::read_text(Path::new(RULES_PATH)) {
            Ok(text) => Self::parse(&text).unwrap_or_default(),
            Err(_) => Self::default(),
        }
    }

    pub fn parse(text: &str) -> Result<Self> {
        let mut entries = BTreeMap::new();
        if text.trim().is_empty() {
            return Ok(Self { entries });
        }
        let root = json::parse(text)?;
        let Value::Object(map) = &root else {
            return Err("图形规则文件根节点不是对象".into());
        };
        if let Some(Value::Object(rules)) = map.get("rules") {
            for (package, value) in rules {
                let entry = match value {
                    // v1 迁移：字符串值 = ANGLE 域后端，enabled=true。
                    Value::String(backend) => {
                        let backend = Backend::parse(backend)
                            .ok_or_else(|| format!("图形规则 {package} 的后端无效: {backend}"))?;
                        RuleEntry::new(match backend {
                            Backend::Angle => BackendMode::Angle,
                            Backend::Native => BackendMode::Native,
                        })
                    }
                    Value::Object(fields) => {
                        let Some(Value::String(mode)) = fields.get("backendMode") else {
                            return Err(format!("图形规则 {package} 缺少 backendMode"));
                        };
                        let mode = BackendMode::parse(mode)
                            .ok_or_else(|| format!("图形规则 {package} 的后端模式无效: {mode}"))?;
                        let enabled = match fields.get("enabled") {
                            Some(Value::Bool(value)) => *value,
                            _ => true,
                        };
                        let status = match fields.get("status") {
                            Some(Value::String(value)) => value.clone(),
                            _ => String::new(),
                        };
                        let status_detail = match fields.get("statusDetail") {
                            Some(Value::String(value)) => value.clone(),
                            _ => String::new(),
                        };
                        RuleEntry { mode, enabled, status, status_detail }
                    }
                    _ => return Err(format!("图形规则 {package} 的值类型无效")),
                };
                if !util::valid_package(package) {
                    return Err(format!("图形规则包名非法: {package}"));
                }
                entries.insert(package.clone(), entry);
            }
        }
        Ok(Self { entries })
    }

    pub fn save(&self) -> Result<()> {
        let mut items = String::new();
        for (index, (package, entry)) in self.entries.iter().enumerate() {
            if index > 0 {
                items.push(',');
            }
            let status = entry.status.replace('\\', "\\\\").replace('"', "\\\"");
            let detail = entry.status_detail.replace('\\', "\\\\").replace('"', "\\\"");
            items.push_str(&format!(
                "\"{package}\": {{\"backendMode\": \"{}\", \"enabled\": {}, \"status\": \"{status}\", \"statusDetail\": \"{detail}\"}}",
                entry.mode.as_str(),
                entry.enabled
            ));
        }
        let text = format!(
            "{{\n  \"schema\": \"novasched-graphics/2\",\n  \"rules\": {{{items}}}\n}}\n"
        );
        util::atomic_write(&PathBuf::from(RULES_PATH), text.as_bytes(), 0o600)
    }

    pub fn entries(&self) -> &BTreeMap<String, RuleEntry> {
        &self.entries
    }

    pub fn entries_mut(&mut self) -> impl Iterator<Item = (&String, &mut RuleEntry)> {
        self.entries.iter_mut()
    }

    /// ANGLE 域视图：仅 enabled 且 mode 为 angle/native 的条目。
    pub fn angle_entries(&self) -> BTreeMap<String, Backend> {
        self.entries
            .iter()
            .filter(|(_, entry)| entry.enabled)
            .filter_map(|(package, entry)| {
                entry.mode.angle_backend().map(|backend| (package.clone(), backend))
            })
            .collect()
    }

    pub fn get(&self, package: &str) -> Option<RuleEntry> {
        self.entries.get(package).cloned()
    }

    pub fn set(&mut self, package: &str, mode: BackendMode) -> Result<()> {
        if !util::valid_package(package) {
            return Err(format!("包名非法: {package}"));
        }
        with_rules_lock(|| {
            let entry = self.entries.entry(package.into()).or_insert_with(|| RuleEntry::new(mode));
            entry.mode = mode;
            entry.enabled = true;
            entry.status.clear();
            entry.status_detail.clear();
            self.save()
        })
    }

    pub fn remove(&mut self, package: &str) -> Result<bool> {
        with_rules_lock(|| {
            let removed = self.entries.remove(package).is_some();
            if removed {
                self.save()?;
            }
            Ok(removed)
        })
    }
}

/// 共享设置中两个列表的解析/合并。逻辑独立成纯函数便于测试。
pub fn parse_list(raw: &str) -> Vec<String> {
    let trimmed = raw.trim().trim_matches('"');
    // `settings get` 对未设置的键返回字面量 "null"。
    if trimmed.is_empty() || trimmed == "null" {
        return Vec::new();
    }
    trimmed
        .split(',')
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_string)
        .collect()
}

/// 把期望规则合并进（pkgs, values）两个等长列表：
/// - 期望为 Some(backend)：插入或覆盖该包的值；
/// - 期望为 None 且包在本模块拥有列表中：移除（只动自己的条目）。
/// 返回新的 (pkgs, values)；长度不一致的脏数据按“清掉双方多余尾部”处理，
/// 不猜测对应关系。
pub fn merge_entries(
    pkgs: Vec<String>,
    values: Vec<String>,
    expected: &BTreeMap<String, Option<Backend>>,
    owned: &BTreeMap<String, OwnedEntry>,
) -> (Vec<String>, Vec<String>) {
    let len = pkgs.len().min(values.len());
    let mut pkgs: Vec<String> = pkgs.into_iter().take(len).collect();
    let mut values: Vec<String> = values.into_iter().take(len).collect();
    for (package, backend) in expected {
        match backend {
            Some(backend) => match pkgs.iter().position(|p| p == package) {
                Some(index) => values[index] = backend.as_str().to_string(),
                None => {
                    pkgs.push(package.clone());
                    values.push(backend.as_str().to_string());
                }
            },
            None => {
                // 只移除本模块曾经写入的包；外部手动配置的条目不动。
                if let Some(index) = pkgs.iter().position(|p| p == package) {
                    if owned.contains_key(package) {
                        pkgs.remove(index);
                        values.remove(index);
                    }
                }
            }
        }
    }
    (pkgs, values)
}

/// 本模块写入一个表项前记录的旧状态，供撤销时恢复。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OwnedEntry {
    /// 写入前该包是否已在共享列表中。
    pub existed: bool,
    /// 写入前的值（existed 时有效）。
    pub previous: Option<String>,
    /// 本模块写入的值（撤销时用于检测“未被外部重新修改”）。
    pub written: String,
}

pub fn read_owned() -> BTreeMap<String, OwnedEntry> {
    let text = util::read_text(Path::new(OWNED_PATH)).unwrap_or_default();
    parse_owned(&text).unwrap_or_default()
}

pub fn parse_owned(text: &str) -> Result<BTreeMap<String, OwnedEntry>> {
    let mut owned = BTreeMap::new();
    if text.trim().is_empty() {
        return Ok(owned);
    }
    let root = json::parse(text)?;
    let Value::Object(map) = &root else {
        return Err("所有权记录根节点不是对象".into());
    };
    if let Some(Value::Array(items)) = map.get("entries") {
        for item in items {
            let Value::Object(_) = item else {
                return Err("所有权记录条目不是对象".into());
            };
            let package = item
                .get("package")
                .and_then(Value::as_str)
                .map_err(|error| format!("所有权记录缺少 package: {error}"))?
                .to_string();
            let written = item
                .get("written")
                .and_then(Value::as_str)
                .map_err(|error| format!("所有权记录缺少 written: {error}"))?
                .to_string();
            let existed = item.get("existed").and_then(Value::as_bool)?;
            let previous = match item.get("previous") {
                Ok(Value::String(value)) => Some(value.clone()),
                _ => None,
            };
            owned.insert(
                package,
                OwnedEntry {
                    existed,
                    previous,
                    written,
                },
            );
        }
    }
    Ok(owned)
}

pub fn save_owned(owned: &BTreeMap<String, OwnedEntry>) -> Result<()> {
    let mut items = String::new();
    for (index, (package, entry)) in owned.iter().enumerate() {
        if index > 0 {
            items.push_str(",\n");
        }
        let previous = match &entry.previous {
            Some(value) => format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\"")),
            None => "null".to_string(),
        };
        items.push_str(&format!(
            "    {{\"package\": \"{}\", \"written\": \"{}\", \"existed\": {}, \"previous\": {}}}",
            package, entry.written, entry.existed, previous
        ));
    }
    let text = format!(
        "{{\n  \"schema\": \"novasched-graphics-owned/1\",\n  \"entries\": [\n{items}\n  ]\n}}\n"
    );
    util::atomic_write(&PathBuf::from(OWNED_PATH), text.as_bytes(), 0o600)
}

pub fn angle_available() -> bool {
    util::run_command("pm", &["path", ANGLE_PACKAGE])
        .map(|output| !output.trim().is_empty())
        .unwrap_or(false)
}

/// 能力探测事实（全部来自真实数据源，绝不按 GPU 型号推断功能）。
#[derive(Clone, Debug, PartialEq)]
pub struct GraphicsCapability {
    pub sdk: u32,
    pub devices: Vec<VulkanDevice>,
    pub angle: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct VulkanDevice {
    pub name: String,
    /// Vulkan 打包版本号（variant<<22|major<<12|minor<<patch）。
    pub api_version: u32,
}

impl VulkanDevice {
    pub fn api_text(&self) -> String {
        format!("{}.{}", (self.api_version >> 22), (self.api_version >> 12) & 0x3ff)
    }
}

/// 枚举 Vulkan HAL 描述文件（/system|/vendor/etc/vulkan/*.json，vkjson
/// 格式）：提取 deviceName + apiVersion。解析容错：数组或单对象均可。
fn vulkan_devices() -> Vec<VulkanDevice> {
    let mut out: Vec<VulkanDevice> = Vec::new();
    for root in ["/system/etc/vulkan", "/vendor/etc/vulkan"] {
        let Ok(entries) = std::fs::read_dir(root) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|value| value.to_str()) != Some("json") {
                continue;
            }
            let Ok(text) = util::read_text(&path) else { continue };
            let Ok(value) = json::parse(&text) else { continue };
            let mut push = |object: &BTreeMap<String, Value>| {
                let name = ["deviceName", "gpuName", "description"]
                    .iter()
                    .find_map(|key| object.get(*key))
                    .and_then(|value| match value {
                        Value::String(text) => Some(text.clone()),
                        _ => None,
                    })
                    .unwrap_or_else(|| "未知 GPU".into());
                let api = match object.get("apiVersion") {
                    Some(Value::Number(number)) if *number > 0 => *number as u32,
                    _ => return,
                };
                if !out.iter().any(|device| device.name == name && device.api_version == api) {
                    out.push(VulkanDevice { name, api_version: api });
                }
            };
            match value {
                Value::Array(items) => {
                    for item in items {
                        if let Value::Object(object) = item {
                            push(&object);
                        }
                    }
                }
                Value::Object(object) => push(&object),
                _ => {}
            }
        }
    }
    out.sort_by(|a, b| b.api_version.cmp(&a.api_version).then(a.name.cmp(&b.name)));
    out
}

/// 目标应用是否声明 Vulkan 特性（dumpsys package 的 requested features，
/// 公开 dump 字段）。None = 无法读取。
fn app_vulkan_support(package: &str) -> Option<bool> {
    let text = util::query_command_limited(
        "/system/bin/dumpsys",
        &["package", package],
        std::time::Duration::from_secs(2),
        262_144,
    )
    .ok()?;
    Some(text.contains("android.hardware.vulkan.version"))
}

fn sdk_version() -> u32 {
    util::getprop("ro.build.version.sdk").trim().parse().unwrap_or(0)
}

/// 能力快照（10 秒缓存——sync/status 每次刷新都会用到）。
const CAPABILITY_TTL: std::time::Duration = std::time::Duration::from_secs(10);

pub fn capability_cached() -> GraphicsCapability {
    static CACHE: std::sync::OnceLock<
        std::sync::Mutex<Option<(std::time::Instant, GraphicsCapability)>>,
    > = std::sync::OnceLock::new();
    let cache = CACHE.get_or_init(|| std::sync::Mutex::new(None));
    if let Ok(guard) = cache.lock() {
        if let Some((at, capability)) = guard.as_ref() {
            if at.elapsed() < CAPABILITY_TTL {
                return capability.clone();
            }
        }
    }
    let capability = GraphicsCapability {
        sdk: sdk_version(),
        devices: vulkan_devices(),
        angle: angle_available(),
    };
    if let Ok(mut guard) = cache.lock() {
        *guard = Some((std::time::Instant::now(), capability.clone()));
    }
    capability
}

/// vulkan 档的**真实**不支持原因链。Android 公开的逐应用驱动入口只有
/// ANGLE/原生；强制 Vulkan 需要应用 manifest 自带 Game Driver 声明，
/// 外部唯一的全局开关属于"伪装逐应用"，规格明令禁止。
fn vulkan_unsupported_reason(package: &str, capability: &GraphicsCapability) -> String {
    let mut reasons: Vec<String> = Vec::new();
    if capability.devices.is_empty() {
        reasons.push("未检测到 Vulkan HAL（/system|/vendor/etc/vulkan 无描述文件）".into());
    } else {
        let list = capability
            .devices
            .iter()
            .take(2)
            .map(|device| format!("{} (API {})", device.name, device.api_text()))
            .collect::<Vec<_>>()
            .join("、");
        reasons.push(format!("设备 Vulkan 可用：{list}"));
    }
    if capability.sdk > 0 && capability.sdk < 29 {
        reasons.push(format!("Android API {} < 29，无可更新 GPU 驱动框架", capability.sdk));
    }
    match app_vulkan_support(package) {
        Some(true) => {}
        Some(false) => reasons.push("目标应用未声明 Vulkan 特性（android.hardware.vulkan.version）".into()),
        None => reasons.push("无法读取目标应用特性清单（dumpsys 不可用）".into()),
    }
    reasons.push(
        "Android 公开的逐应用驱动入口仅接受 ANGLE/原生；强制 Vulkan 需应用 manifest 自带声明，外部无安全开关（全局驱动开关属于伪装，禁止）".into(),
    );
    format!("不支持：{}", reasons.join("；"))
}

fn settings_get(key: &str) -> String {
    util::run_command("settings", &["get", SETTINGS_NS, key])
        .map(|value| value.trim().to_string())
        .unwrap_or_default()
}

fn settings_put(key: &str, value: &str) -> Result<()> {
    util::run_command("settings", &["put", SETTINGS_NS, key, value]).map(|_| ())
}

fn settings_delete(key: &str) -> Result<()> {
    util::run_command("settings", &["delete", SETTINGS_NS, key]).map(|_| ())
}

/// 把当前规则同步到系统设置。返回人类可读的结果报告；任何一步失败都带
/// 具体原因，不假装成功。ANGLE 域只投影 angle/native；vulkan/auto/follow
/// 走各自状态链（vulkan 恒为如实 unsupported，见 capability_reason）。
pub fn sync(rules: &Rules) -> String {
    if rules.entries().is_empty() {
        return "图形后端规则为空，未修改系统设置".to_string();
    }
    let mut rules = rules.clone();
    let angle_available = angle_available();
    let auto_notes = resolve_auto_modes(&mut rules, angle_available);
    let angle_rules = rules.angle_entries();
    let mut report = String::new();
    if !angle_rules.is_empty() {
        if !angle_available {
            report.push_str(&format!(
                "本机未找到 ANGLE 驱动包（{ANGLE_PACKAGE}），angle/native 档不可用；"
            ));
        } else {
            report.push_str(&sync_angle(&angle_rules));
        }
    }
    report.push_str(&sync_status_entries(&mut rules, angle_available, &auto_notes));
    report
}

/// `auto` 档在同步时解析：ANGLE 可用 → angle；否则 follow（依据写进状态）。
fn resolve_auto_modes(rules: &mut Rules, angle_available: bool) -> BTreeMap<String, String> {
    let mut notes = BTreeMap::new();
    for (package, entry) in rules.entries_mut() {
        if entry.enabled && entry.mode == BackendMode::Auto {
            let (mode, note) = if angle_available {
                (BackendMode::Angle, "自动解析为 ANGLE（驱动包可用）".to_string())
            } else {
                (BackendMode::Follow, "自动解析为跟随系统（未找到 ANGLE 驱动包）".to_string())
            };
            entry.mode = mode;
            notes.insert(package.clone(), note);
        }
    }
    notes
}

/// ANGLE 域同步（原 v2.12.1 逻辑，域投影自 angle_entries）。写后读回一致
/// 的条目状态 = applied（展示语义：已配置，应用重启后生效——生效本身无法
/// 从外部证明，不虚称）。
fn sync_angle(angle_rules: &BTreeMap<String, Backend>) -> String {
    let owned_before = read_owned();
    let mut expected: BTreeMap<String, Option<Backend>> = BTreeMap::new();
    for (package, backend) in angle_rules {
        expected.insert(package.clone(), Some(*backend));
    }
    for package in owned_before.keys() {
        expected.entry(package.clone()).or_insert(None);
    }
    let pkgs_raw = settings_get(PKGS_KEY);
    let values_raw = settings_get(VALUES_KEY);
    let (pkgs, values) = merge_entries(
        parse_list(&pkgs_raw),
        parse_list(&values_raw),
        &expected,
        &owned_before,
    );
    let (pkgs_text, values_text) = if pkgs.is_empty() {
        (String::new(), String::new())
    } else {
        (pkgs.join(","), values.join(","))
    };
    let result = (|| -> Result<()> {
        if pkgs_text.is_empty() {
            settings_delete(PKGS_KEY)?;
            settings_delete(VALUES_KEY)?;
        } else {
            settings_put(PKGS_KEY, &pkgs_text)?;
            settings_put(VALUES_KEY, &values_text)?;
        }
        let read_pkgs = settings_get(PKGS_KEY);
        let read_values = settings_get(VALUES_KEY);
        if pkgs_text.is_empty() {
            if read_pkgs != "null" || read_values != "null" {
                return Err("删除后读回仍存在".into());
            }
        } else if read_pkgs != pkgs_text || read_values != values_text {
            return Err(format!(
                "写入后读回不一致：期望 {pkgs_text} / {values_text}，实际 {read_pkgs} / {read_values}"
            ));
        }
        Ok(())
    })();
    match result {
        Ok(()) => {
            // 成功后刷新所有权记录：新写入条目记录旧值；已不在规则里的
            // 包移出记录。
            let mut owned = owned_before.clone();
            for (package, backend) in angle_rules {
                let written = backend.as_str().to_string();
                owned
                    .entry(package.clone())
                    .and_modify(|entry| {
                        if entry.written != written {
                            entry.written = written.clone();
                        }
                    })
                    .or_insert_with(|| {
                        let old_pkgs = parse_list(&pkgs_raw);
                        let old_values = parse_list(&values_raw);
                        let previous = old_pkgs
                            .iter()
                            .position(|p| p == package)
                            .and_then(|index| old_values.get(index))
                            .cloned();
                        OwnedEntry {
                            existed: previous.is_some(),
                            previous,
                            written: written.clone(),
                        }
                    });
            }
            owned.retain(|package, _| angle_rules.contains_key(package));
            let saved = save_owned(&owned);
            let count = angle_rules.len();
            let note = match saved {
                Ok(()) => String::new(),
                Err(error) => format!("（所有权记录保存失败: {error}）"),
            };
            format!(
                "已写入 {count} 条图形后端规则并读回确认；目标应用需完全退出重开生效{note}。"
            )
        }
        Err(error) => format!("图形后端规则写入失败: {error}。"),
    }
}

/// 非 ANGLE 域条目的状态链：follow/auto/vulkan/disabled 逐条写回
/// lastApplyStatus（vulkan 恒为如实 unsupported 原因链）。
fn sync_status_entries(
    rules: &mut Rules,
    angle_available: bool,
    auto_notes: &BTreeMap<String, String>,
) -> String {
    let capability = capability_cached();
    let mut changed = false;
    let mut lines = Vec::new();
    for (package, entry) in rules.entries_mut() {
        if !entry.enabled {
            if entry.status != "disabled" {
                entry.status = "disabled".into();
                entry.status_detail = "规则已停用，不参与同步".into();
                changed = true;
            }
            continue;
        }
        match entry.mode {
            BackendMode::Angle | BackendMode::Native => {
                let status = if angle_available { "applied" } else { "failed" };
                let detail = if angle_available {
                    "ANGLE 列表写入并读回一致；应用完全退出重开后使用所选后端".to_string()
                } else {
                    format!("未找到 ANGLE 驱动包（{ANGLE_PACKAGE}），无法写入")
                };
                if entry.status != status || entry.status_detail != detail {
                    entry.status = status.into();
                    entry.status_detail = detail;
                    changed = true;
                }
            }
            BackendMode::Follow => {
                let detail = "跟随系统：本模块未写入任何驱动选择".to_string();
                if entry.status != "follow" || entry.status_detail != detail {
                    entry.status = "follow".into();
                    entry.status_detail = detail;
                    changed = true;
                }
            }
            BackendMode::Auto => {
                let detail = auto_notes.get(package).cloned().unwrap_or_default();
                if entry.status != "auto" || entry.status_detail != detail {
                    entry.status = "auto".into();
                    entry.status_detail = detail;
                    changed = true;
                }
            }
            BackendMode::Vulkan => {
                let reason = vulkan_unsupported_reason(package, &capability);
                if entry.status != "unsupported" || entry.status_detail != reason {
                    entry.status = "unsupported".into();
                    entry.status_detail = reason.clone();
                    changed = true;
                }
                lines.push(format!("{package}: 不支持 —— {reason}"));
            }
        }
    }
    if changed {
        let _ = rules.save();
    }
    if lines.is_empty() {
        String::new()
    } else {
        format!("逐应用 Vulkan：\n{}\n", lines.join("\n"))
    }
}

/// 撤销本模块写入且未被外部修改的表项；被外部改过的保留并报告。
pub fn revert_owned() -> String {
    let owned = read_owned();
    if owned.is_empty() {
        return "无本模块写入的图形后端表项".to_string();
    }
    let pkgs_raw = settings_get(PKGS_KEY);
    let values_raw = settings_get(VALUES_KEY);
    let mut pkgs = parse_list(&pkgs_raw);
    let mut values = parse_list(&values_raw);
    let mut restored = 0usize;
    let mut kept = Vec::new();
    for (package, entry) in &owned {
        let Some(index) = pkgs.iter().position(|p| p == package) else {
            continue;
        };
        if values.get(index).map(String::as_str) != Some(entry.written.as_str()) {
            // 外部已改成别的值：不是我们的状态，保留。
            kept.push(package.clone());
            continue;
        }
        match (&entry.previous, entry.existed) {
            (Some(previous), true) => {
                values[index] = previous.clone();
                restored += 1;
            }
            _ => {
                pkgs.remove(index);
                values.remove(index);
                restored += 1;
            }
        }
    }
    let outcome = if pkgs.is_empty() {
        settings_delete(PKGS_KEY).and_then(|_| settings_delete(VALUES_KEY))
    } else {
        settings_put(PKGS_KEY, &pkgs.join(",")).and_then(|_| settings_put(VALUES_KEY, &values.join(",")))
    };
    match outcome {
        Ok(()) => {
            let _ = std::fs::remove_file(Path::new(OWNED_PATH));
            if kept.is_empty() {
                format!("已撤销 {restored} 条本模块写入的图形后端表项")
            } else {
                format!(
                    "已撤销 {restored} 条；{} 被外部修改，保持不动",
                    kept.join(", ")
                )
            }
        }
        Err(error) => format!("图形后端表项撤销失败: {error}"),
    }
}

/// 只读状态报告（诊断用）：能力事实 + 逐应用请求/状态。
pub fn status() -> String {
    let rules = Rules::load();
    let capability = capability_cached();
    let mut out = String::new();
    out.push_str("kind=per-app-graphics\n");
    out.push_str(&format!("angle_driver_available={}\n", if capability.angle { "yes" } else { "no" }));
    out.push_str(&format!("angle_package={ANGLE_PACKAGE}\n"));
    out.push_str(&format!("android_sdk={}\n", capability.sdk));
    if capability.devices.is_empty() {
        out.push_str("vulkan_devices=0\nvulkan_unavailable=未检测到 Vulkan HAL 描述文件\n");
    } else {
        out.push_str(&format!("vulkan_devices={}\n", capability.devices.len()));
        for device in &capability.devices {
            out.push_str(&format!("vulkan_device={}|API {}\n", device.name, device.api_text()));
        }
    }
    out.push_str(&format!("rules_count={}\n", rules.entries().len()));
    for (package, entry) in rules.entries() {
        out.push_str(&format!(
            "rule_{package}={};enabled={};status={};detail={}\n",
            entry.mode.as_str(),
            entry.enabled,
            if entry.status.is_empty() { "pending" } else { entry.status.as_str() },
            entry.status_detail
        ));
    }
    out.push_str(&format!("{PKGS_KEY}={}\n", settings_get(PKGS_KEY)));
    out.push_str(&format!("{VALUES_KEY}={}\n", settings_get(VALUES_KEY)));
    out.push_str("生效条件=目标应用进程启动时读取；规则变更需应用完全退出重开。用户请求≠实际生效：状态列才是实际侧记录（applied=写入读回一致；vulkan 档恒为如实 unsupported 原因链）\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rules_parse_and_validate_entries() {
        // v1 迁移：字符串值仍可读。
        let rules =
            Rules::parse("{\"rules\":{\"com.game.app\":\"angle\",\"com.other\":\"native\"}}")
                .unwrap();
        assert_eq!(rules.get("com.game.app").map(|entry| entry.mode), Some(BackendMode::Angle));
        assert_eq!(rules.get("com.other").map(|entry| entry.mode), Some(BackendMode::Native));
        assert_eq!(rules.get("com.missing"), None);
        assert!(Rules::parse("{\"rules\":{\"bad pkg\":\"angle\"}}").is_err());
        assert!(Rules::parse("{\"rules\":{\"com.game.app\":1}}").is_err());
        assert!(Rules::parse("not json").is_err());
        assert_eq!(Rules::parse("").unwrap().entries().len(), 0);
    }

    #[test]
    fn schema_v2_parses_objects_with_enabled_and_status() {
        let rules = Rules::parse(
            "{\"rules\":{\"com.game\":{\"backendMode\":\"vulkan\",\"enabled\":true,\"status\":\"unsupported\",\"statusDetail\":\"不支持：x\"},\
             \"com.off\":{\"backendMode\":\"angle\",\"enabled\":false}}}",
        )
        .unwrap();
        let game = rules.get("com.game").unwrap();
        assert_eq!(game.mode, BackendMode::Vulkan);
        assert!(game.enabled);
        assert_eq!(game.status, "unsupported");
        assert_eq!(game.status_detail, "不支持：x");
        // ANGLE 域投影：停用条目与 vulkan 条目都不进共享列表。
        let angle = rules.angle_entries();
        assert!(angle.is_empty());
        let enabled_angle = Rules::parse(
            "{\"rules\":{\"com.on\":{\"backendMode\":\"angle\",\"enabled\":true}}}",
        )
        .unwrap();
        assert_eq!(enabled_angle.angle_entries().get("com.on"), Some(&Backend::Angle));
        for bad in [
            "{\"rules\":{\"com.game\":{}}}",
            "{\"rules\":{\"com.game\":{\"backendMode\":\"steam\"}}}",
            "{\"rules\":{\"com.game\":{\"backendMode\":3}}}",
        ] {
            assert!(Rules::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn vulkan_mode_is_honestly_unsupported_with_reason_chain() {
        // 规格：vulkan 档绝不伪装写入。在主机测试环境（无 Vulkan 描述文件、
        // 无 dumpsys、ANGLE 包不存在），原因链必须包含全部三层事实。
        let capability = GraphicsCapability { sdk: 35, devices: Vec::new(), angle: false };
        let reason = vulkan_unsupported_reason("com.game", &capability);
        assert!(reason.contains("未检测到 Vulkan HAL"), "{reason}");
        assert!(reason.contains("目标应用未声明 Vulkan 特性") || reason.contains("无法读取目标应用特性清单"), "{reason}");
        assert!(reason.contains("外部无安全开关"), "{reason}");
        assert!(reason.starts_with("不支持："));
    }

    #[test]
    fn vulkan_device_version_decode_matches_packaged_api() {
        let device = VulkanDevice { name: "Adreno".into(), api_version: (1 << 22) | (3 << 12) | 5 };
        assert_eq!(device.api_text(), "1.3");
        // 仅主位：minor=0。
        let device = VulkanDevice { name: "Mali".into(), api_version: 1 << 22 };
        assert_eq!(device.api_text(), "1.0");
    }

    #[test]
    fn backend_mode_covers_five_documented_options() {
        for value in ["follow", "auto", "angle", "native", "vulkan"] {
            assert_eq!(BackendMode::parse(value).unwrap().as_str(), value);
        }
        assert!(BackendMode::parse("steam").is_none());
        assert_eq!(BackendMode::Angle.angle_backend(), Some(Backend::Angle));
        assert_eq!(BackendMode::Native.angle_backend(), Some(Backend::Native));
        for mode in [BackendMode::Follow, BackendMode::Auto, BackendMode::Vulkan] {
            assert_eq!(mode.angle_backend(), None, "{:?} 不应有 ANGLE 执行通道", mode);
        }
    }

    #[test]
    fn merge_inserts_updates_and_only_removes_owned_entries() {
        let mut expected: BTreeMap<String, Option<Backend>> = BTreeMap::new();
        expected.insert("com.game".into(), Some(Backend::Angle));
        let mut owned = BTreeMap::new();
        owned.insert(
            "com.old".into(),
            OwnedEntry {
                existed: true,
                previous: Some("angle".into()),
                written: "native".into(),
            },
        );
        // 外部已有其它应用条目 + 本模块旧条目 + 新规则：新规则插入，
        // 旧条目不受影响（本次合并没有要求移除它）。
        let (pkgs, values) = merge_entries(
            vec!["com.external".into(), "com.old".into()],
            vec!["native".into(), "native".into()],
            &expected,
            &owned,
        );
        assert_eq!(
            pkgs,
            vec![
                "com.external".to_string(),
                "com.old".to_string(),
                "com.game".to_string()
            ]
        );
        assert_eq!(
            values,
            vec![
                "native".to_string(),
                "native".to_string(),
                "angle".to_string()
            ]
        );
        // default 规则只移除本模块拥有的条目；外部条目保持。
        let mut expected: BTreeMap<String, Option<Backend>> = BTreeMap::new();
        expected.insert("com.old".into(), None);
        expected.insert("com.external".into(), None);
        let (pkgs, values) = merge_entries(
            vec!["com.external".into(), "com.old".into()],
            vec!["native".into(), "native".into()],
            &expected,
            &owned,
        );
        assert_eq!(pkgs, vec!["com.external".to_string()]);
        assert_eq!(values, vec!["native".to_string()]);
    }

    #[test]
    fn merge_tolerates_uneven_and_dirty_lists() {
        let expected: BTreeMap<String, Option<Backend>> = BTreeMap::new();
        let owned = BTreeMap::new();
        // 长度不一致：按最短对齐，不猜对应关系。
        let (pkgs, values) = merge_entries(
            vec!["a".into(), "b".into(), "c".into()],
            vec!["angle".into()],
            &expected,
            &owned,
        );
        assert_eq!(pkgs, vec!["a".to_string()]);
        assert_eq!(values, vec!["angle".to_string()]);
        // 空输入产出空输出。
        let (pkgs, values) = merge_entries(Vec::new(), Vec::new(), &expected, &owned);
        assert!(pkgs.is_empty() && values.is_empty());
    }

    #[test]
    fn list_parsing_handles_null_quotes_and_spaces() {
        assert!(parse_list("null").is_empty());
        assert!(parse_list("").is_empty());
        assert_eq!(
            parse_list("\"a, b,,c\""),
            vec!["a".to_string(), "b".to_string(), "c".to_string()]
        );
    }

    #[test]
    fn owned_records_round_trip() {
        let mut owned = BTreeMap::new();
        owned.insert(
            "com.game".into(),
            OwnedEntry {
                existed: true,
                previous: Some("native".into()),
                written: "angle".into(),
            },
        );
        owned.insert(
            "com.new".into(),
            OwnedEntry {
                existed: false,
                previous: None,
                written: "angle".into(),
            },
        );
        let text = {
            // 复用 save_owned 的序列化：直接构造等价文本验证 parse。
            let mut items = String::new();
            for (index, (package, entry)) in owned.iter().enumerate() {
                if index > 0 {
                    items.push_str(",\n");
                }
                let previous = match &entry.previous {
                    Some(value) => format!("\"{value}\""),
                    None => "null".to_string(),
                };
                items.push_str(&format!(
                    "    {{\"package\": \"{package}\", \"written\": \"{}\", \"existed\": {}, \"previous\": {previous}}}",
                    entry.written, entry.existed
                ));
            }
            format!(
                "{{\n  \"schema\": \"novasched-graphics-owned/1\",\n  \"entries\": [\n{items}\n  ]\n}}\n"
            )
        };
        assert_eq!(parse_owned(&text).unwrap(), owned);
        assert!(parse_owned("").unwrap().is_empty());
        assert!(parse_owned("[]").is_err());
    }

    #[test]
    fn rules_serialize_round_trips_through_parse() {
        // 回环：serialize 的输出必须能被 parse 原样读回（真机曾抓到
        // 保存格式写成非 JSON 的 `包=后端` 导致重载静默丢规则）。
        // v2 起 Rules 序列化为对象形式；serialize_rules 保留为 ANGLE 域
        // 纯函数（merge 预期构造使用），此测试验证旧版式仍可迁移读回。
        let mut entries = BTreeMap::new();
        entries.insert("com.game.app".to_string(), Backend::Angle);
        entries.insert("com.other.app".to_string(), Backend::Native);
        let text = serialize_rules(&entries);
        let wrapped = format!(
            "{{\n  \"schema\": \"novasched-graphics/1\",\n  \"rules\": {text}\n}}\n"
        );
        let parsed = Rules::parse(&wrapped).unwrap();
        for (package, backend) in &entries {
            let entry = parsed.get(package).expect(package);
            assert_eq!(entry.mode.angle_backend(), Some(*backend));
            assert!(entry.enabled);
        }
        assert_eq!(serialize_rules(&parsed.angle_entries()), text);
        // v2 对象形式 save→parse 回环（纯文本层，不触盘）。
        let modern = Rules::parse(
            "{\"rules\":{\"com.game\":{\"backendMode\":\"vulkan\",\"enabled\":false,\"status\":\"unsupported\",\"statusDetail\":\"不支持：r\"}}}",
        )
        .unwrap();
        let _ = modern.save(); // 主机无 /data：写失败可接受（结果已尽力而为）
        // 空表产出空对象且可解析。
        let empty = Rules::parse(&format!(
            "{{\"rules\": {}}}",
            serialize_rules(&BTreeMap::new())
        ))
        .unwrap();
        assert_eq!(empty.entries().len(), 0);
    }

    #[test]
    fn backend_values_match_graphicsenvironment_contract() {
        // GraphicsEnvironment 只认这两个字符串。
        assert_eq!(Backend::Angle.as_str(), "angle");
        assert_eq!(Backend::Native.as_str(), "native");
        assert_eq!(Backend::parse("angle"), Some(Backend::Angle));
        assert_eq!(Backend::parse(" native "), Some(Backend::Native));
        assert_eq!(Backend::parse("vulkan"), None);
    }
}
