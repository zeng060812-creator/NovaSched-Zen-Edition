//! ZenJooo · DeepSeek 自适应调度引擎（C 混合模式）。
//!
//! 本地引擎（FAS/Scheduler）负责实时调度；本模块只做"看得全、想得深"的
//! 低频分析：事件驱动 + 双重限频（冷却 + 每日配额）+ 重复签名去重，产出
//! **带 TTL 的结构化建议**，经本地验证层后才允许被仲裁消费。
//!
//! 三态：off（完全本地）/ observe（只分析不改参数，默认）/ auto（验证后参与）。
//!
//! 安全边界（规格红线）：
//! - API Key 唯一落盘点是 `run/ai.key`（0600 root-only）；从不进入
//!   JS/localStorage/日志/URL 参数；HTTP 经 `curl -K <0600 配置>` 携带
//!   Authorization（Key 不进 argv/URL）。
//! - AI 输出只接受固定 JSON 字段；不能产生 shell 命令，不能触碰温控，
//!   不能写 Scene 配置；fas_ceiling 只能作为**上限钳制**（不得高于配置）。
//! - 验证失败的策略直接拒绝并计数；AI 关闭即删除临时策略（清理自身请求）。
use crate::config::MODES;
use crate::json::{self, Value};
use crate::util::{self, Result};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

pub const CONFIG_PATH: &str = "/data/adb/novasched/config/ai.json";
pub const KEY_PATH: &str = "/data/adb/novasched/run/ai.key";
pub const POLICY_PATH: &str = "/data/adb/novasched/run/ai-policy.json";
pub const USAGE_PATH: &str = "/data/adb/novasched/run/ai-usage.json";
pub const DECISION_PATH: &str = "/data/adb/novasched/run/ai-decision.json";
pub const LOG_PATH: &str = "/data/adb/novasched/logs/ai-log.jsonl";
const CURL_CONFIG_PATH: &str = "/data/adb/novasched/run/ai-curl.cfg";
const BODY_PATH: &str = "/data/adb/novasched/run/ai-body.json";
const LOG_CAP: u64 = 64 * 1024;
const LOG_KEEP_LINES: usize = 60;
const DEFAULT_MODEL: &str = "deepseek-chat";
const DEFAULT_BASE_URL: &str = "https://api.deepseek.com";
const DEFAULT_MAX_CALLS_PER_DAY: u32 = 6;
const DEFAULT_COOLDOWN_MIN: u64 = 20;
const REQUEST_TIMEOUT_SECS: u64 = 25;
/// 策略 TTL 钳制范围（分钟）。
const TTL_MIN: u64 = 5;
const TTL_MAX: u64 = 120;
/// 单字节脱敏展示：sk-****abcd。
const KEY_HINT_LEN: usize = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Off,
    Observe,
    Auto,
}

impl Mode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Observe => "observe",
            Self::Auto => "auto",
        }
    }
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim() {
            "off" => Some(Self::Off),
            "observe" => Some(Self::Observe),
            "auto" => Some(Self::Auto),
            _ => None,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "关闭（完全本地调度）",
            Self::Observe => "观察（只分析建议，不修改参数）",
            Self::Auto => "自动（建议经本地验证后参与调度）",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct AiConfig {
    pub mode: Mode,
    pub model: String,
    pub base_url: String,
    pub max_calls_per_day: u32,
    pub cooldown_min: u64,
}

impl Default for AiConfig {
    fn default() -> Self {
        Self {
            mode: Mode::Off,
            model: DEFAULT_MODEL.into(),
            base_url: DEFAULT_BASE_URL.into(),
            max_calls_per_day: DEFAULT_MAX_CALLS_PER_DAY,
            cooldown_min: DEFAULT_COOLDOWN_MIN,
        }
    }
}

fn sanitize_base_url(raw: &str) -> Result<String> {
    let url = raw.trim().trim_end_matches('/');
    let Some((scheme, rest)) = url.split_once("://") else {
        return Err("接口地址必须以 http:// 或 https:// 开头".into());
    };
    if scheme != "https" && scheme != "http" {
        return Err(format!("不支持的协议: {scheme}（仅 http/https）"));
    }
    let host = rest.split('/').next().unwrap_or_default();
    let host = host.rsplit_once('@').map(|(_, host)| host).unwrap_or(host);
    let host = host.split(':').next().unwrap_or_default();
    if host.is_empty() {
        return Err("接口地址缺少主机名".into());
    }
    if !valid_remote_host(host) {
        return Err(format!("拒绝的主机: {host}（localhost/私有/保留地址不允许）"));
    }
    Ok(url.to_string())
}

/// Mimosa 约束：拒绝 localhost、环回、私有与保留地址。字面 IP 按段判定；
/// 域名含 "localhost" 拒绝；无点的单标签主机拒绝（公网域名至少一个点）。
fn valid_remote_host(host: &str) -> bool {
    let host = host.trim().trim_start_matches('[').trim_end_matches(']');
    let lower = host.to_ascii_lowercase();
    if lower.is_empty() || lower == "localhost" || lower.ends_with(".localhost") || !lower.contains('.') {
        return false;
    }
    // IPv4 字面量：逐段筛私有/保留/环回。
    let octets: Vec<Option<u32>> = lower
        .split('.')
        .map(|part| part.parse::<u32>().ok().filter(|_| part.bytes().all(|b| b.is_ascii_digit())))
        .collect();
    if octets.len() == 4 && octets.iter().all(|value| value.is_some()) {
        let octets: Vec<u32> = octets.into_iter().map(|value| value.unwrap()).collect();
        let (a, b) = (octets[0], octets[1]);
        return match (a, b) {
            (0, _) | (10, _) | (127, _) | (169, 254) | (172, 16..=31) | (192, 0)
            | (192, 168) | (198, 18..=19) | (203, 0) | (224..=255, _) => false,
            (100, 64..=127) => false,
            _ => true,
        };
    }
    // IPv6 关键前缀（环回/链路本地/ULA/文档前缀）。
    if lower.contains(':') {
        for bad in ["::1", "fe80:", "fc", "fd", "2001:db8"] {
            if lower == bad || lower.starts_with(bad) {
                return false;
            }
        }
    }
    true
}

pub fn load_config() -> AiConfig {
    parse_config(&util::read_text(Path::new(CONFIG_PATH)).unwrap_or_default()).unwrap_or_default()
}

pub fn parse_config(text: &str) -> Result<AiConfig> {
    let mut config = AiConfig::default();
    if text.trim().is_empty() {
        return Ok(config);
    }
    let root = json::parse(text)?;
    let Value::Object(map) = &root else {
        return Err("AI 配置根节点不是对象".into());
    };
    if let Some(Value::String(mode)) = map.get("mode") {
        config.mode = Mode::parse(mode).ok_or("AI 模式无效（off/observe/auto）")?;
    }
    if let Some(Value::String(model)) = map.get("model") {
        let model = model.trim();
        if !model.is_empty() && model.len() <= 96 {
            config.model = model.to_string();
        }
    }
    if let Some(Value::String(url)) = map.get("base_url") {
        config.base_url = sanitize_base_url(url)?;
    }
    if let Some(Value::Number(number)) = map.get("max_calls_per_day") {
        let value = *number;
        if (1..=60).contains(&value) {
            config.max_calls_per_day = value as u32;
        }
    }
    if let Some(Value::Number(number)) = map.get("cooldown_min") {
        let value = *number;
        if (1..=720).contains(&value) {
            config.cooldown_min = value as u64;
        }
    }
    Ok(config)
}

fn save_config(config: &AiConfig) -> Result<()> {
    let text = format!(
        "{{\n  \"schema\": \"novasched-ai/1\",\n  \"mode\": \"{}\",\n  \"model\": \"{}\",\n  \"base_url\": \"{}\",\n  \"max_calls_per_day\": {},\n  \"cooldown_min\": {}\n}}\n",
        config.mode.as_str(),
        config.model,
        config.base_url,
        config.max_calls_per_day,
        config.cooldown_min
    );
    util::atomic_write(Path::new(CONFIG_PATH), text.as_bytes(), 0o600)
}

fn key_present() -> bool {
    util::read_text(Path::new(KEY_PATH))
        .map(|text| !text.trim().is_empty())
        .unwrap_or(false)
}

fn key_hint() -> String {
    let key = util::read_text(Path::new(KEY_PATH)).unwrap_or_default();
    let key = key.trim();
    if key.is_empty() {
        return "未设置".into();
    }
    let tail: String = key.chars().rev().take(KEY_HINT_LEN).collect::<Vec<_>>().into_iter().rev().collect();
    format!("已设置（****{tail}）")
}

// ============================================================
// 用量（费用可见：调用次数 + token，当日计数）
// ============================================================
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Usage {
    pub day: u64,
    pub calls: u32,
    pub total_tokens: u64,
    pub last_tokens: u64,
}

fn day_index(now: u64) -> u64 {
    now / 86400
}

pub fn usage_today(now: u64) -> Usage {
    let today = day_index(now);
    let Ok(root) = json::parse(&util::read_text(Path::new(USAGE_PATH)).unwrap_or_default()) else {
        return Usage { day: today, ..Default::default() };
    };
    let Value::Object(map) = root else { return Usage { day: today, ..Default::default() } };
    let day = match map.get("day") {
        Some(Value::Number(number)) => *number as u64,
        _ => today,
    };
    if day != today {
        return Usage { day: today, ..Default::default() };
    }
    Usage {
        day,
        calls: match map.get("calls") {
            Some(Value::Number(number)) => *number as u32,
            _ => 0,
        },
        total_tokens: match map.get("total_tokens") {
            Some(Value::Number(number)) => *number as u64,
            _ => 0,
        },
        last_tokens: match map.get("last_tokens") {
            Some(Value::Number(number)) => *number as u64,
            _ => 0,
        },
    }
}

fn record_usage(now: u64, tokens: u64) -> Usage {
    let mut usage = usage_today(now);
    usage.day = day_index(now);
    usage.calls += 1;
    usage.last_tokens = tokens;
    usage.total_tokens += tokens;
    let text = format!(
        "{{\"day\":{},\"calls\":{},\"total_tokens\":{},\"last_tokens\":{}}}\n",
        usage.day, usage.calls, usage.total_tokens, usage.last_tokens
    );
    let _ = util::atomic_write(Path::new(USAGE_PATH), text.as_bytes(), 0o600);
    usage
}

// ============================================================
// 冷却/配额（可注入时钟的纯逻辑，便于测试）
// ============================================================
#[derive(Clone, Debug, PartialEq)]
pub struct GateState {
    pub last_call_at: u64,
    pub usage: Usage,
}

#[derive(Clone, Debug, PartialEq)]
pub enum GateVerdict {
    Allow,
    RateLimited { wait_min: u64 },
    QuotaExhausted { max: u32 },
    ModeOff,
    SameSignature,
}

/// 冷却+配额+签名去重的纯判定。`signature` 为设备快照摘要哈希——重复
/// 状态不重复请求（规格要求）。`signature_change_only=true` 时冷却减半
/// （状态确实变化的事件优先于常规节流，但仍受配额硬限制）。
pub fn gate_verdict(
    config: &AiConfig,
    gate: &GateState,
    now: u64,
    signature: u64,
    last_signature: u64,
    signature_changed: bool,
) -> GateVerdict {
    if config.mode == Mode::Off {
        return GateVerdict::ModeOff;
    }
    if gate.usage.calls >= config.max_calls_per_day && day_index(now) == gate.usage.day {
        return GateVerdict::QuotaExhausted { max: config.max_calls_per_day };
    }
    let cooldown_secs = config.cooldown_min * 60;
    let effective = if signature_changed && signature != last_signature {
        cooldown_secs / 2
    } else {
        cooldown_secs
    };
    if gate.last_call_at != 0 && now.saturating_sub(gate.last_call_at) < effective {
        let wait_min = effective.saturating_sub(now.saturating_sub(gate.last_call_at)) / 60 + 1;
        return GateVerdict::RateLimited { wait_min };
    }
    if signature == last_signature && gate.last_call_at != 0 {
        return GateVerdict::SameSignature;
    }
    GateVerdict::Allow
}

// ============================================================
// 策略（验证层产物）与生命周期
// ============================================================
#[derive(Clone, Debug, PartialEq)]
pub struct Policy {
    pub suggested_mode: Option<String>,
    /// FAS ceiling 上限钳制（0 = 不设置；只能降不能升）。
    pub fas_ceiling: u32,
    pub issued_at: u64,
    pub expires_at: u64,
    pub scene_signature: String,
    pub summary: String,
    pub event: String,
}

fn clamp_ttl(min: u64) -> u64 {
    min.clamp(TTL_MIN, TTL_MAX)
}

/// 验证层：LLM 输出 → 受约束策略。未知/越界内容直接拒绝并返回原因。
pub fn validate_decision(
    content: &str,
    now: u64,
    scene_signature: &str,
    event: &str,
) -> std::result::Result<Policy, String> {
    let stripped = strip_code_fences(content);
    let root = json::parse(stripped.trim())
        .map_err(|error| format!("AI 输出不是合法 JSON: {error}"))?;
    let Value::Object(map) = root else {
        return Err("AI 输出不是 JSON 对象".into());
    };
    let suggested_mode = match map.get("suggested_mode") {
        Some(Value::String(mode)) => {
            let mode = mode.trim();
            if MODES.contains(&mode) {
                Some(mode.to_string())
            } else {
                return Err(format!("suggested_mode 越界: {mode}（不在四档内）"));
            }
        }
        _ => None,
    };
    let fas_ceiling = match map.get("fas_ceiling") {
        Some(Value::Number(number)) if *number >= 0 && *number <= 100 => *number as u32,
        Some(Value::Number(number)) => {
            return Err(format!("fas_ceiling 越界: {number}（须 0..=100）"))
        }
        _ => 0,
    };
    if suggested_mode.is_none() && fas_ceiling == 0 {
        return Err("建议不包含任何可执行字段（suggested_mode/fas_ceiling 均缺省）".into());
    }
    let ttl_min = match map.get("ttl_min") {
        Some(Value::Number(number)) if *number > 0 => clamp_ttl(*number as u64),
        _ => 30,
    };
    let summary = match map.get("summary") {
        Some(Value::String(text)) => text.trim().chars().take(300).collect(),
        _ => String::new(),
    };
    Ok(Policy {
        suggested_mode,
        fas_ceiling,
        issued_at: now,
        expires_at: now + ttl_min * 60,
        scene_signature: scene_signature.to_string(),
        summary,
        event: event.to_string(),
    })
}

/// 剥离 ```json 围栏与前后噪声（LLM 常见输出形态）。
fn strip_code_fences(content: &str) -> &str {
    let trimmed = content.trim();
    if let Some(rest) = trimmed.strip_prefix("```") {
        let rest = rest.split_once('\n').map(|(_, rest)| rest).unwrap_or(rest);
        if let Some(end) = rest.rfind("```") {
            return rest[..end].trim();
        }
        return rest.trim();
    }
    // 容错：从首个 { 到最后一个 } 截取。
    match (trimmed.find('{'), trimmed.rfind('}')) {
        (Some(start), Some(end)) if end > start => &trimmed[start..=end],
        _ => trimmed,
    }
}

pub fn save_policy(policy: &Policy) -> Result<()> {
    let text = format!(
        "{{\"schema\":\"novasched-ai-policy/1\",\"suggested_mode\":{},\"fas_ceiling\":{},\"issued_at\":{},\"expires_at\":{},\"scene_signature\":\"{}\",\"summary\":\"{}\",\"event\":\"{}\"}}\n",
        match &policy.suggested_mode {
            Some(mode) => format!("\"{mode}\""),
            None => "null".into(),
        },
        policy.fas_ceiling,
        policy.issued_at,
        policy.expires_at,
        escape(policy.scene_signature.as_str()),
        escape(policy.summary.as_str()),
        escape(policy.event.as_str()),
    );
    util::atomic_write(Path::new(POLICY_PATH), text.as_bytes(), 0o600)
}

pub fn active_policy(now: u64, scene_signature: &str) -> Option<Policy> {
    let text = util::read_text(Path::new(POLICY_PATH)).ok()?;
    let root = json::parse(&text).ok()?;
    let Value::Object(map) = root else { return None };
    let expires_at = match map.get("expires_at") {
        Some(Value::Number(number)) => *number as u64,
        _ => return None,
    };
    let issued_at = match map.get("issued_at") {
        Some(Value::Number(number)) => *number as u64,
        _ => return None,
    };
    if now >= expires_at {
        return None;
    }
    let signature = match map.get("scene_signature") {
        Some(Value::String(value)) => value.clone(),
        _ => return None,
    };
    if signature != scene_signature {
        // Scene 切换即失效（规格 PART 3 第 6 条）：需要重新分析。
        return None;
    }
    let suggested_mode = match map.get("suggested_mode") {
        Some(Value::String(mode)) if MODES.contains(&mode.as_str()) => Some(mode.clone()),
        _ => None,
    };
    let fas_ceiling = match map.get("fas_ceiling") {
        Some(Value::Number(number)) if *number > 0 => *number as u32,
        _ => 0,
    };
    if suggested_mode.is_none() && fas_ceiling == 0 {
        return None;
    }
    let summary = match map.get("summary") {
        Some(Value::String(value)) => value.clone(),
        _ => String::new(),
    };
    let event = match map.get("event") {
        Some(Value::String(value)) => value.clone(),
        _ => String::new(),
    };
    Some(Policy {
        suggested_mode,
        fas_ceiling,
        issued_at,
        expires_at,
        scene_signature: signature,
        summary,
        event,
    })
}

/// 清理 AI 临时策略（关闭 AI / 恢复本地调度）。
pub fn clear_policy() {
    let _ = util::remove_if_exists(Path::new(POLICY_PATH));
}

fn escape(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

// ============================================================
// 决策日志（脱敏：无 Key、无原始长文本，只存结论与原因）
// ============================================================
fn append_log(entry: &str) {
    util::create_dir(Path::new("/data/adb/novasched/logs")).ok();
    let path = PathBuf::from(LOG_PATH);
    let _ = crate::logging::append_bounded(&path, &format!("{entry}\n"));
    // 容量守卫：超限截断到最近 LOG_KEEP_LINES 行。
    if let Ok(meta) = std::fs::metadata(&path) {
        if meta.len() > LOG_CAP {
            if let Ok(text) = util::read_text(&path) {
                let lines: Vec<&str> = text.lines().collect();
                let keep = lines.len().saturating_sub(LOG_KEEP_LINES);
                let trimmed = lines[keep..].join("\n");
                let _ = util::atomic_write(&path, trimmed.as_bytes(), 0o600);
            }
        }
    }
}

pub fn log_entries(limit: usize) -> Vec<String> {
    let text = util::read_text(Path::new(LOG_PATH)).unwrap_or_default();
    text.lines().rev().take(limit.clamp(1, 60)).map(|line| line.to_string()).collect()
}

// ============================================================
// HTTP 客户端（curl -K 0600 配置优先，wget 兜底；Key 永不进 argv/URL）
// ============================================================
/// HTTP 结果：区分「请求已到达服务端」（计入用量/可能计费）与
/// 「网络层失败」（未计费），费用统计才可信。
struct AiError {
    reached: bool,
    message: String,
}

impl AiError {
    fn network(message: impl Into<String>) -> Self {
        Self { reached: false, message: message.into() }
    }
    fn http(message: impl Into<String>) -> Self {
        Self { reached: true, message: message.into() }
    }
}

fn curl_path() -> Option<String> {
    let output = util::run_command("sh", &["-c", "command -v curl 2>/dev/null"]).ok()?;
    let path = output.trim().to_string();
    (!path.is_empty()).then_some(path)
}

fn chat_request(config: &AiConfig, payload: &str) -> std::result::Result<String, AiError> {
    let key = util::read_text(Path::new(KEY_PATH))
        .ok()
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty())
        .ok_or_else(|| AiError::network("未设置 API Key"))?;
    let url = format!("{}/chat/completions", config.base_url);
    util::atomic_write(Path::new(BODY_PATH), payload.as_bytes(), 0o600)
        .map_err(AiError::network)?;
    // curl 优先：Authorization 走 0600 配置文件（Key 不进 argv/URL）。平台路径
    // 由 command -v 解析，不写死 /system/bin/curl（曾因此在部分 ROM 上失效）。
    if let Some(curl) = curl_path() {
        let cfg = format!(
            "header = \"Content-Type: application/json\"\nheader = \"Authorization: Bearer {}\"\nrequest = \"POST\"\ndata = \"@{BODY_PATH}\"\n",
            escape(key.as_str())
        );
        util::atomic_write(Path::new(CURL_CONFIG_PATH), cfg.as_bytes(), 0o600)
            .map_err(AiError::network)?;
        let output = util::run_command(
            &curl,
            &[
                "-sS",
                "--max-time",
                &REQUEST_TIMEOUT_SECS.to_string(),
                "-w",
                "\n%{http_code}",
                "-K",
                CURL_CONFIG_PATH,
                &url,
            ],
        )
        .map_err(|error| AiError::network(format!("网络请求失败: {error}")));
        // 立即清掉含 Authorization 的配置文件与请求体：Key 不长期留在第二个文件里。
        let _ = util::remove_if_exists(Path::new(CURL_CONFIG_PATH));
        let _ = util::remove_if_exists(Path::new(BODY_PATH));
        return output.and_then(|text| finish_response(&text));
    }
    // wget 兜底：header 在 argv（root 域短暂可见）——如实降级路径。
    let result = util::run_command(
        "/system/bin/wget",
        &[
            "-q",
            "-O",
            "-",
            "--timeout=25",
            &format!("--header=Authorization: Bearer {key}"),
            "--header=Content-Type: application/json",
            &format!("--post-data={payload}"),
            &url,
        ],
    );
    let _ = util::remove_if_exists(Path::new(BODY_PATH));
    match result {
        Ok(output) => finish_response_wget(&output),
        Err(error) => Err(AiError::network(format!("网络请求失败: {error}"))),
    }
}

fn finish_response(output: &str) -> std::result::Result<String, AiError> {
    // curl -w 追加的末行是 http_code。
    let (body, code) = match output.rsplit_once('\n') {
        Some((body, code)) => (body, code.trim().to_string()),
        None => (output, String::new()),
    };
    classify_http(&code, body)
}

fn finish_response_wget(output: &str) -> std::result::Result<String, AiError> {
    // wget 无状态码输出：成功=有 JSON body；失败由空体/错误文本体现。
    if output.trim().is_empty() {
        return Err(AiError::network("网络请求失败（wget 无输出）"));
    }
    Ok(output.to_string())
}

fn classify_http(code: &str, body: &str) -> std::result::Result<String, AiError> {
    let code_value: u32 = code.trim().parse().unwrap_or(0);
    match code_value {
        200..=299 => Ok(body.to_string()),
        401 | 403 => Err(AiError::http(format!(
            "鉴权失败（HTTP {code_value}）：API Key 无效或无权限"
        ))),
        429 => Err(AiError::http("API 限流（HTTP 429）：冷却将加倍")),
        402 => Err(AiError::http("余额不足（HTTP 402）：请到 DeepSeek 控制台充值")),
        code if code >= 400 => Err(AiError::http(format!(
            "HTTP {code}: {}",
            body.trim().chars().take(160).collect::<String>()
        ))),
        _ => Err(AiError::network(if body.trim().is_empty() {
            "网络请求失败（无响应体）".into()
        } else {
            format!("响应异常: {}", body.trim().chars().take(160).collect::<String>())
        })),
    }
}

// ============================================================
// 决策流程：门禁 → 网络（独立线程）→ 解析 → 验证 → 落盘
// ============================================================
static IN_FLIGHT: AtomicBool = AtomicBool::new(false);

/// 设备快照载荷由守护进程构造（最小字段）；签名是其稳定哈希。
pub fn fnv1a(text: &str) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in text.bytes() {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

pub fn build_payload(snapshot: &BTreeMap<String, String>) -> String {
    let mut items = snapshot
        .iter()
        .map(|(key, value)| format!("\"{key}\":\"{}\"", escape(value)))
        .collect::<Vec<_>>();
    items.sort();
    format!("{{{}}}", items.join(","))
}

/// 决策入口（守护事件/手动触发）。门禁同步判定，网络在独立线程执行，
/// 绝不阻塞 200ms 调度热路径。
pub fn consider(
    event: &str,
    payload: &str,
    scene_signature: &str,
    now: u64,
) -> Result<String> {
    consider_with(event, payload, scene_signature, now, false)
}

/// 带诊断标志的决策入口：`diagnostic=true`（CLI 手动分析）只做分析记录，
/// **绝不写入临时策略**——旧实现由 ai-decide 写入 scene_signature="manual"
/// 的策略，守护侧签名永不匹配，文件白留且状态显示错乱。
pub fn consider_with(
    event: &str,
    payload: &str,
    scene_signature: &str,
    now: u64,
    diagnostic: bool,
) -> Result<String> {
    let config = load_config();
    if config.mode == Mode::Off && !diagnostic {
        return Err("AI 引擎处于关闭状态（手动分析请用 ai-decide）".into());
    }
    if !key_present() {
        return Err("未设置 API Key".into());
    }
    let signature = fnv1a(payload);
    let usage = usage_today(now);
    let gate = GateState { last_call_at: last_call_at(), usage };
    let last_signature = last_signature_value();
    // signature_changed 用于「状态确实变化」时冷却减半（规格要求优先响应
    // 状态变化）；旧实现恒传 false，减半分支从未生效。
    let signature_changed = last_signature != 0 && signature != last_signature;
    let verdict = gate_verdict(&config, &gate, now, signature, last_signature, signature_changed);
    match verdict {
        GateVerdict::ModeOff if diagnostic => {}
        GateVerdict::ModeOff => return Err("AI 引擎处于关闭状态".into()),
        GateVerdict::QuotaExhausted { max } => {
            return Err(format!("今日配额已用尽（{max} 次），明日自动恢复——费用保护"))
        }
        GateVerdict::RateLimited { wait_min } => {
            return Err(format!("冷却中，约 {wait_min} 分钟后可再次分析"))
        }
        GateVerdict::SameSignature => return Ok("设备状态无变化，复用最近决策（未调用 API）".into()),
        GateVerdict::Allow => {}
    }
    if IN_FLIGHT.swap(true, Ordering::SeqCst) {
        return Err("上一次分析仍在进行中".into());
    }
    mark_call_started(now, signature);
    let event = event.to_string();
    let scene_signature = scene_signature.to_string();
    let payload = payload.to_string();
    let observe_only = config.mode != Mode::Auto;
    match std::thread::Builder::new()
        .name("novasched-ai".into())
        .spawn(move || {
            run_decision(event, payload, scene_signature, observe_only, diagnostic, now)
        })
    {
        Ok(handle) => {
            let _ = handle; // 结果写盘，不 join：绝不让网络等待阻塞守护。
            Ok("分析任务已提交（低频事件驱动）".into())
        }
        Err(_) => {
            IN_FLIGHT.store(false, Ordering::SeqCst);
            Err("无法创建分析线程".into())
        }
    }
}

fn run_decision(
    event: String,
    payload: String,
    scene_signature: String,
    observe_only: bool,
    diagnostic: bool,
    now: u64,
) {
    let config = load_config();
    // 系统提示只放在 system 消息里；user 消息只带设备快照——旧实现把
    // SYSTEM_PROMPT 重复拼进 user 内容，白付一倍 input token。
    let outcome = (|| -> std::result::Result<(String, u64), AiError> {
        let body = format!(
            "{{\"model\":\"{}\",\"messages\":[{{\"role\":\"system\",\"content\":{} }},{{\"role\":\"user\",\"content\":{} }}],\"temperature\":0.2,\"max_tokens\":400,\"response_format\":{{\"type\":\"json_object\"}}}}",
            json_string(&config.model),
            json_string(SYSTEM_PROMPT),
            json_string(&payload),
        );
        let raw = chat_request(&config, &body)?;
        let root = json::parse(raw.trim())
            .map_err(|error| AiError::http(format!("响应解析失败: {error}")))?;
        let Value::Object(map) = root else {
            return Err(AiError::http("响应不是 JSON 对象"));
        };
        let content = map
            .get("choices")
            .and_then(|value| match value {
                Value::Array(items) => items.first().cloned(),
                _ => None,
            })
            .and_then(|choice| match choice {
                Value::Object(object) => object.get("message").cloned(),
                _ => None,
            })
            .and_then(|message| match message {
                Value::Object(object) => object.get("content").cloned(),
                _ => None,
            })
            .and_then(|content| match content {
                Value::String(text) => Some(text),
                _ => None,
            })
            .ok_or_else(|| AiError::http("响应缺少 choices[0].message.content"))?;
        let tokens = match map.get("usage").and_then(|usage| match usage {
            Value::Object(object) => object.get("total_tokens").cloned(),
            _ => None,
        }) {
            Some(Value::Number(number)) => number as u64,
            _ => 0,
        };
        Ok((content, tokens))
    })();
    // 只统计「已到达服务端」的请求：纯网络失败不计费，不能虚增用量。
    if outcome.as_ref().map(|_| true).unwrap_or_else(|error| error.reached) {
        record_usage(now, outcome.as_ref().map(|(_, tokens)| *tokens).unwrap_or(0));
    }
    match outcome {
        Ok((content, _tokens)) => match validate_decision(&content, now, &scene_signature, &event) {
            Ok(policy) => {
                let apply_note = if diagnostic {
                    "手动诊断：建议仅记录，不写入临时策略"
                } else if observe_only {
                    "观察模式：建议仅记录，不参与调度"
                } else {
                    "自动模式：建议已通过验证，交由仲裁消费"
                };
                if !observe_only && !diagnostic {
                    let _ = save_policy(&policy);
                }
                let decision = format!(
                    "{{\"at\":{},\"event\":\"{}\",\"accepted\":true,\"observe\":{},\"mode\":{},\"fas_ceiling\":{},\"summary\":\"{}\"}}",
                    now,
                    escape(&event),
                    observe_only,
                    match &policy.suggested_mode {
                        Some(mode) => format!("\"{mode}\""),
                        None => "null".into(),
                    },
                    policy.fas_ceiling,
                    escape(policy.summary.as_str()),
                );
                append_log(&decision);
                write_decision(&decision);
                log_info(&format!("DeepSeek 决策（{event}）：{}；{apply_note}", policy.summary));
            }
            Err(reject_reason) => {
                let decision = format!(
                    "{{\"at\":{},\"event\":\"{}\",\"accepted\":false,\"reason\":\"{}\"}}",
                    now,
                    escape(&event),
                    escape(&reject_reason),
                );
                append_log(&decision);
                write_decision(&decision);
                log_info(&format!("DeepSeek 建议被本地验证拒绝（{event}）：{reject_reason}"));
            }
        },
        Err(error) => {
            if error.message.contains("429") {
                // 限流：冷却加倍（把 last_call 推到未来，推迟下一次尝试）。
                mark_call_started(now + config.cooldown_min * 60, 0);
            }
            let decision = format!(
                "{{\"at\":{},\"event\":\"{}\",\"accepted\":false,\"error\":\"{}\"}}",
                now,
                escape(&event),
                escape(&error.message),
            );
            append_log(&decision);
            write_decision(&decision);
            log_info(&format!(
                "DeepSeek 分析失败（{event}）：{}；本地调度不受影响",
                error.message
            ));
        }
    }
    IN_FLIGHT.store(false, Ordering::SeqCst);
}

const SYSTEM_PROMPT: &str = "你是 Android 性能调度分析引擎。只输出一个 JSON 对象，字段：suggested_mode(powersave|balance|performance|fast 之一，可省略)、fas_ceiling(0-100 整数，可省略)、ttl_min(5-120)、summary(不超过200字的中文结论)。规则：电池温度≥43℃或温度上升明显时建议降档或降低 fas_ceiling；帧率长期达标且加压高时建议降低 fas_ceiling；持续掉帧且 GPU 负载不高时才建议升档；不确定时省略字段并在 summary 说明。";

fn json_string(value: &str) -> String {
    format!("\"{}\"", escape(value))
}

fn write_decision(decision: &str) {
    let _ = util::atomic_write(Path::new(DECISION_PATH), decision.as_bytes(), 0o600);
}

pub fn last_decision() -> String {
    util::read_text(Path::new(DECISION_PATH)).unwrap_or_else(|_| "{\"at\":0}".into())
}

// ---- last_call / last_signature 持久化（随 usage 文件同域） ----
const STATE_PATH: &str = "/data/adb/novasched/run/ai-state.txt";

fn mark_call_started(now: u64, signature: u64) {
    let text = format!("last_call={now}\nlast_signature={signature}\n");
    let _ = util::atomic_write(Path::new(STATE_PATH), text.as_bytes(), 0o600);
}

fn last_call_at() -> u64 {
    read_state("last_call")
}

fn last_signature_value() -> u64 {
    read_state("last_signature")
}

fn read_state(key: &str) -> u64 {
    let text = util::read_text(Path::new(STATE_PATH)).unwrap_or_default();
    for line in text.lines() {
        if let Some((name, value)) = line.split_once('=') {
            if name.trim() == key {
                return value.trim().parse().unwrap_or(0);
            }
        }
    }
    0
}

fn log_info(text: &str) {
    append_log(&format!(
        "{{\"at\":{},\"kind\":\"info\",\"text\":\"{}\"}}",
        util::now_secs(),
        escape(text)
    ));
}

// ============================================================
// CLI 状态输出与设置入口
// ============================================================
pub fn set_mode(mode: Mode) -> Result<()> {
    let mut config = load_config();
    config.mode = mode;
    save_config(&config)?;
    if mode != Mode::Auto {
        // 规格 PART 3 第 7 条：非自动态（关闭/观察）不得保留可消费的临时策略。
        // 旧实现只在 Off 清理，自动→观察后旧策略在 TTL 内仍可被「切回自动」复活。
        clear_policy();
    }
    Ok(())
}

pub fn set_model(model: &str) -> Result<()> {
    let mut config = load_config();
    let model = model.trim();
    if model.is_empty() || model.len() > 96 {
        return Err("模型名非法".into());
    }
    config.model = model.to_string();
    save_config(&config)
}

pub fn set_base_url(url: &str) -> Result<()> {
    let mut config = load_config();
    config.base_url = sanitize_base_url(url)?;
    save_config(&config)
}

pub fn set_key(key: &str) -> Result<()> {
    let key = key.trim();
    if key.len() < 8 || key.len() > 256 || !key.bytes().all(|b| b.is_ascii_graphic()) {
        return Err("API Key 格式非法（8-256 个可见字符）".into());
    }
    util::atomic_write(Path::new(KEY_PATH), key.as_bytes(), 0o600)
}

pub fn clear_key() -> Result<()> {
    util::remove_if_exists(Path::new(KEY_PATH))
}

pub fn restore_local() -> Result<()> {
    clear_policy();
    Ok(())
}

/// 手动分析（CLI `ai-decide` / WebUI「立即分析」）的最小快照：直接读设备
/// 节点，不依赖守护线程，因此关掉 AI 或守护重启后也能做一次体检。
pub fn snapshot_from_device() -> String {
    let mut snapshot = BTreeMap::new();
    for key in ["ro.soc.model", "ro.board.platform", "ro.product.model"] {
        let value = util::getprop(key);
        if !value.is_empty() {
            snapshot.insert(key.replace("ro.", ""), value);
        }
    }
    let mut freqs = Vec::new();
    if let Ok(entries) = std::fs::read_dir("/sys/devices/system/cpu/cpufreq") {
        let mut policies: Vec<_> = entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with("policy"))
            })
            .collect();
        policies.sort();
        for policy in policies {
            if let Ok(value) = util::read_trimmed(&policy.join("scaling_cur_freq")) {
                freqs.push(value);
            }
        }
    }
    if !freqs.is_empty() {
        snapshot.insert("freqs_khz".into(), freqs.join(";"));
    }
    if let Some(busy) = crate::frame_ctrl::read_gpu_busy() {
        snapshot.insert("gpu_busy".into(), format!("{busy:.0}"));
    }
    let (battery, charging, temp) = crate::history::Recorder::read_battery();
    snapshot.insert(
        "battery".into(),
        format!(
            "{}% {} {}℃",
            battery.map(|value| value.to_string()).unwrap_or_else(|| "?".into()),
            match charging {
                Some(true) => "charging",
                Some(false) => "discharging",
                None => "unknown",
            },
            temp.map(|value| format!("{value:.1}")).unwrap_or_else(|| "?".into()),
        ),
    );
    snapshot.insert(
        "mode".into(),
        crate::app_modes::read_mode_file().unwrap_or_else(|| "unknown".into()),
    );
    build_payload(&snapshot)
}

/// 连接测试（最小请求，费用由用户自担并计入用量）。
pub fn test_connection() -> Result<String> {
    if !key_present() {
        return Err("未设置 API Key".into());
    }
    let config = load_config();
    let started = std::time::Instant::now();
    let payload = format!(
        "{{\"model\":\"{}\",\"messages\":[{{\"role\":\"user\",\"content\":\"输出 JSON: {{\\\"ok\\\":true}}\"}}],\"max_tokens\":10}}",
        json_string(&config.model),
    );
    let raw = chat_request(&config, &payload).map_err(|error| error.message)?;
    let _ = json::parse(raw.trim()).map_err(|error| format!("响应解析失败: {error}"))?;
    Ok(format!(
        "连接成功（{} ms，模型 {}）",
        started.elapsed().as_millis(),
        config.model
    ))
}

/// 状态展示用：读取策略文件，不做签名匹配（ai-status 不持有守护内的实时
/// 签名）。返回 (策略, 是否已过期, 签名是否与本进程可验证的上下文无关)。
fn peek_policy(now: u64) -> Option<(Policy, bool)> {
    let text = util::read_text(Path::new(POLICY_PATH)).ok()?;
    let root = json::parse(&text).ok()?;
    let Value::Object(map) = root else { return None };
    let number = |key: &str| -> Option<u64> {
        match map.get(key) {
            Some(Value::Number(value)) => Some(*value as u64),
            _ => None,
        }
    };
    let issued_at = number("issued_at")?;
    let expires_at = number("expires_at")?;
    let suggested_mode = match map.get("suggested_mode") {
        Some(Value::String(mode)) if MODES.contains(&mode.as_str()) => Some(mode.clone()),
        _ => None,
    };
    let fas_ceiling = number("fas_ceiling").unwrap_or(0) as u32;
    if suggested_mode.is_none() && fas_ceiling == 0 {
        return None;
    }
    Some((
        Policy {
            suggested_mode,
            fas_ceiling,
            issued_at,
            expires_at,
            scene_signature: match map.get("scene_signature") {
                Some(Value::String(value)) => value.clone(),
                _ => String::new(),
            },
            summary: match map.get("summary") {
                Some(Value::String(value)) => value.clone(),
                _ => String::new(),
            },
            event: match map.get("event") {
                Some(Value::String(value)) => value.clone(),
                _ => String::new(),
            },
        },
        now >= expires_at,
    ))
}

pub fn status(now: u64) -> String {
    let config = load_config();
    let usage = usage_today(now);
    let mut out = String::new();
    out.push_str(&format!("ai_mode={}\n", config.mode.as_str()));
    out.push_str(&format!("ai_mode_label={}\n", config.mode.label()));
    out.push_str(&format!("model={}\n", config.model));
    out.push_str(&format!("base_url={}\n", config.base_url));
    out.push_str(&format!("key={}\n", key_hint()));
    out.push_str(&format!("key_present={}\n", if key_present() { "yes" } else { "no" }));
    out.push_str(&format!("calls_today={}/{}\n", usage.calls, config.max_calls_per_day));
    out.push_str(&format!("tokens_today={}\n", usage.total_tokens));
    out.push_str(&format!("last_tokens={}\n", usage.last_tokens));
    out.push_str(&format!("cooldown_min={}\n", config.cooldown_min));
    // 冷却剩余：last_call 在未来（限流加倍）时 remaining 正常计算。
    let last_call = last_call_at();
    let elapsed = now.saturating_sub(last_call);
    let remaining = (config.cooldown_min * 60).saturating_sub(elapsed);
    out.push_str(&format!(
        "cooldown_remaining={}\n",
        if last_call == 0 { 0 } else { remaining }
    ));
    // 策略状态：旧实现用空签名调用 active_policy → 永远 policy=none，
    // 界面因此看不到真实有效期。这里直接读文件并如实标注过期/待匹配。
    match peek_policy(now) {
        Some((policy, expired)) => {
            out.push_str(&format!(
                "policy={};mode={};fas_ceiling={};expires_in={};event={}\n",
                if expired { "expired" } else { "stored" },
                policy.suggested_mode.as_deref().unwrap_or(""),
                policy.fas_ceiling,
                policy.expires_at.saturating_sub(now),
                policy.event,
            ));
            if !policy.summary.is_empty() {
                out.push_str(&format!("policy_summary={}\n", policy.summary));
            }
            out.push_str(
                "policy_note=是否被调度消费由守护侧校验（auto 模式 + 未过期 + Scene 签名一致）\n",
            );
        }
        None => out.push_str("policy=none\n"),
    }
    let decision = last_decision();
    if decision != "{\"at\":0}" {
        out.push_str(&format!("last_decision={decision}\n"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(mode: Mode) -> AiConfig {
        AiConfig { mode, ..AiConfig::default() }
    }

    #[test]
    fn base_url_guard_rejects_private_and_local_targets() {
        assert!(sanitize_base_url("https://api.deepseek.com").is_ok());
        assert!(sanitize_base_url("https://api.deepseek.com/").is_ok());
        assert!(sanitize_base_url("http://example.com").is_ok());
        for bad in [
            "https://localhost",
            "http://localhost:8080",
            "http://127.0.0.1/v1",
            "http://10.0.0.5",
            "https://192.168.1.1",
            "https://172.16.0.9",
            "https://169.254.1.1",
            "https://[::1]/v1",
            "https://fe80::1",
            "ftp://example.com",
            "example.com",
            "https://0.0.0.0",
            "https://100.64.0.1",
            "https://198.18.0.1",
        ] {
            assert!(sanitize_base_url(bad).is_err(), "{bad} 应被拒绝");
        }
    }

    #[test]
    fn mode_and_config_defaults_match_documented_values() {
        let config = AiConfig::default();
        assert_eq!(config.mode, Mode::Off);
        assert_eq!(config.model, "deepseek-chat");
        assert_eq!(config.base_url, "https://api.deepseek.com");
        assert_eq!(config.max_calls_per_day, 6);
        assert_eq!(config.cooldown_min, 20);
        assert_eq!(Mode::parse("observe"), Some(Mode::Observe));
        assert!(Mode::parse("yolo").is_none());
    }

    #[test]
    fn gates_enforce_quota_cooldown_and_signature_dedup() {
        let test_config = config(Mode::Auto);
        let now = 1_000_000;
        let gate = GateState { last_call_at: 0, usage: Usage { day: day_index(now), ..Default::default() } };
        assert_eq!(gate_verdict(&test_config, &gate, now, 7, 0, false), GateVerdict::Allow);
        // 冷却期：刚调用过 → RateLimited。
        let gate = GateState { last_call_at: now - 60, usage: gate.usage.clone() };
        assert!(matches!(gate_verdict(&test_config, &gate, now, 9, 7, false),
            GateVerdict::RateLimited { wait_min } if wait_min >= 15));
        // 配额耗尽：同日调用次数打满。
        let gate = GateState {
            last_call_at: now - 7200,
            usage: Usage { day: day_index(now), calls: 6, total_tokens: 0, last_tokens: 0 },
        };
        assert_eq!(
            gate_verdict(&test_config, &gate, now, 11, 7, false),
            GateVerdict::QuotaExhausted { max: 6 }
        );
        // 次日：配额重置。
        let gate = GateState {
            last_call_at: now - 86400,
            usage: Usage { day: day_index(now) - 1, calls: 6, total_tokens: 0, last_tokens: 0 },
        };
        assert_eq!(gate_verdict(&test_config, &gate, now, 11, 7, false), GateVerdict::Allow);
        // 重复签名：不重复请求。
        let gate = GateState { last_call_at: now - 7200, usage: gate.usage.clone() };
        assert_eq!(gate_verdict(&test_config, &gate, now, 7, 7, false), GateVerdict::SameSignature);
        // 关闭态直接拒绝。
        assert_eq!(
            gate_verdict(&config(Mode::Off), &gate, now, 99, 7, false),
            GateVerdict::ModeOff
        );
    }

    #[test]
    fn decision_validation_rejects_out_of_range_and_accepts_clean_json() {
        let now = 500_000;
        let ok = validate_decision(
            "{\"suggested_mode\":\"balance\",\"fas_ceiling\":55,\"ttl_min\":45,\"summary\":\"温度偏高，降上限\"}",
            now,
            "sig-a",
            "bottleneck",
        )
        .unwrap();
        assert_eq!(ok.suggested_mode.as_deref(), Some("balance"));
        assert_eq!(ok.fas_ceiling, 55);
        assert_eq!(ok.expires_at - ok.issued_at, 45 * 60);
        assert_eq!(ok.scene_signature, "sig-a");
        // 围栏包裹也能解析。
        let fenced = validate_decision("```json\n{\"fas_ceiling\":20,\"ttl_min\":3}\n```", now, "s", "e").unwrap();
        assert_eq!(fenced.fas_ceiling, 20);
        assert_eq!(fenced.expires_at - fenced.issued_at, TTL_MIN * 60, "ttl 钳到下限");
        // 越界拒绝。
        for bad in [
            "{\"suggested_mode\":\"turbo\"}",
            "{\"fas_ceiling\":180}",
            "{\"summary\":\"没有任何可执行字段\"}",
            "这不是 JSON",
        ] {
            assert!(validate_decision(bad, now, "s", "e").is_err(), "{bad}");
        }
    }

    #[test]
    fn observe_mode_still_analyzes_but_diagnostic_needs_no_mode() {
        // 观察模式必须允许分析（只记录），只有关闭态拒绝。
        let now = 1_000_000;
        let gate = GateState { last_call_at: 0, usage: Usage { day: day_index(now), ..Default::default() } };
        assert_eq!(gate_verdict(&config(Mode::Observe), &gate, now, 5, 0, false), GateVerdict::Allow);
        // 诊断（手动分析）在关闭态不走「引擎已关闭」短路；Key 缺失时如实报
        // 未设置 Key（主机环境无 /data，key 必然缺失）——旧实现返回的是
        // 模糊的“引擎关闭”，无法区分是没开还是没 Key。
        let closed = consider_with("manual", "{}", "manual", now, true).unwrap_err();
        assert!(closed.contains("未设置 API Key"), "{closed}");
        let normal = consider_with("event", "{}", "sig", now, false).unwrap_err();
        assert!(normal.contains("AI 引擎处于关闭状态"), "{normal}");
    }

    #[test]
    fn negative_or_missing_fields_are_rejected_by_validation() {
        let now = 5_000;
        // 负数 ceiling 落到默认 0；没有任何可执行字段 → 拒绝。
        assert!(validate_decision("{\"fas_ceiling\":-5,\"ttl_min\":10}", now, "s", "e").is_err());
        // 只有 summary → 拒绝（不允许只有描述没有动作）。
        assert!(validate_decision("{\"summary\":\"温度高\"}", now, "s", "e").is_err());
        // 合法边界：0 与 100。
        assert_eq!(validate_decision("{\"fas_ceiling\":0,\"suggested_mode\":\"balance\"}", now, "s", "e").unwrap().fas_ceiling, 0);
        assert_eq!(validate_decision("{\"fas_ceiling\":100}", now, "s", "e").unwrap().fas_ceiling, 100);
    }

    #[test]
    fn policy_expiry_and_scene_signature_invalidate() {
        let now = 10_000;
        let policy = validate_decision("{\"suggested_mode\":\"fast\",\"ttl_min\":5}", now, "sig-1", "e").unwrap();
        assert_eq!(policy.scene_signature, "sig-1");
        // 过期语义：expires_at 超过 now 才可消费（active_policy 的判定条件）。
        assert!(policy.expires_at > now);
        // 签名不匹配语义：Scene 切换后旧策略不可消费。
        let mut mismatched = policy.clone();
        mismatched.scene_signature = "sig-2".into();
        assert_ne!(mismatched.scene_signature, policy.scene_signature);
    }

    #[test]
    fn payload_snapshot_is_stable_and_keyed() {
        let mut snapshot = BTreeMap::new();
        snapshot.insert("soc".into(), "SM8650".into());
        snapshot.insert("mode".into(), "fast".into());
        let a = build_payload(&snapshot);
        let b = build_payload(&snapshot);
        assert_eq!(a, b, "同快照必须产出同载荷（签名去重依赖）");
        assert!(a.contains("\"soc\":\"SM8650\""));
        // 档位变化 → 载荷与签名都必须变化（重复状态不重复请求的基石）。
        snapshot.insert("mode".into(), "balance".into());
        let c = build_payload(&snapshot);
        assert_ne!(c, a);
        assert_eq!(fnv1a(&a), fnv1a(&a));
        assert_ne!(fnv1a(&a), fnv1a(&c));
    }

    #[test]
    fn key_hint_never_exposes_full_key() {
        // 脱敏展示的纯逻辑等价：只保留末 4 位。
        let key = "sk-1234567890abcdef";
        let tail: String = key.chars().rev().take(KEY_HINT_LEN).collect::<Vec<_>>().into_iter().rev().collect();
        assert_eq!(tail, "cdef");
        assert!(!format!("****{tail}").contains("sk-"));
    }
}
