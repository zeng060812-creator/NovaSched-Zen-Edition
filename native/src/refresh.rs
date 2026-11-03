//! ZenJooo · 按应用刷新率调节（Scene 同款机制，非强制帧率上限）。
//!
//! Scene N1 的做法（APK 静态分析证实）：`dumpsys display` 解析显示模式列表
//! （`modeId=` 与 `refreshRate=` 成对出现），前台切换时用
//! `service call SurfaceFlinger 1035 i32 <modeId>` 设置激活显示模式，
//! 由常驻 monitor 跟随前台应用。本模块提供同能力：
//! - 规则存储 `refresh_rules.json`（包名 → 目标 Hz；0 = 跟随系统）；
//! - 目标 Hz 必须命中设备真实模式列表（不捏造模式；解析不到列表就如实
//!   报告不可用）；
//! - 只在目标模式 ≠ 当前激活模式时下发一次 `service call SurfaceFlinger
//!   1035`，写后回读 `mActiveModeId` 确认；
//! - 前台离开有规则的应用、屏幕关闭或模块停止时恢复启动时记录的原激活
//!   模式；恢复点无法识别时保留现状并如实告警，不猜。
//!
//! 不承诺"锁定游戏渲染帧率"：面板刷新率改变的是合成器节奏，不是游戏
//! 渲染上限。
use crate::json::{self, Value};
use crate::util::{self, Result};
use std::collections::BTreeMap;
use std::path::Path;
use std::time::{Duration, Instant};

pub const RULES_PATH: &str = "/data/adb/novasched/config/refresh_rules.json";
const RULES_LOCK: &str = "/data/adb/novasched/run/refresh-rules.lock";
const SF_SET_ACTIVE_CODE: i32 = 1035;
const DUMPSYS_TIMEOUT: Duration = Duration::from_millis(1200);
/// 模式查询缓存：面板模式列表开机后不变，但解析失败要低频重试。
const MODES_CACHE: Duration = Duration::from_secs(30);

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DisplayMode {
    pub id: u32,
    pub hz: f32,
    pub pixels: u64,
}

pub fn parse_rules(text: &str) -> Result<BTreeMap<String, u32>> {
    let mut rules = BTreeMap::new();
    if text.trim().is_empty() {
        return Ok(rules);
    }
    let root = json::parse(text)?;
    let Value::Object(map) = &root else {
        return Err("刷新率规则根节点不是对象".into());
    };
    if let Some(Value::Object(entries)) = map.get("rules") {
        for (package, value) in entries {
            let Value::Number(hz) = value else {
                return Err(format!("刷新率规则 {package} 的值不是整数"));
            };
            if *hz < 0 || *hz > 1000 {
                return Err(format!("刷新率规则 {package} 的目标无效: {hz}"));
            }
            if !util::valid_package(package) {
                return Err(format!("刷新率规则包名非法: {package}"));
            }
            if *hz > 0 {
                rules.insert(package.clone(), *hz as u32);
            }
        }
    }
    Ok(rules)
}

pub fn load() -> BTreeMap<String, u32> {
    parse_rules(&util::read_text(Path::new(RULES_PATH)).unwrap_or_default()).unwrap_or_default()
}

fn save(entries: &BTreeMap<String, u32>) -> Result<()> {
    let mut items = String::new();
    for (index, (package, hz)) in entries.iter().enumerate() {
        if index > 0 {
            items.push(',');
        }
        items.push_str(&format!("\"{package}\": {hz}"));
    }
    let text = format!(
        "{{\n  \"schema\": \"novasched-refresh/1\",\n  \"rules\": {{{items}}}\n}}\n"
    );
    util::atomic_write(Path::new(RULES_PATH), text.as_bytes(), 0o600)
}

pub fn set(package: &str, hz: u32) -> Result<()> {
    if !util::valid_package(package) {
        return Err(format!("包名非法: {package}"));
    }
    let deadline = Instant::now() + Duration::from_millis(500);
    let _lock = loop {
        match util::acquire_flock(Path::new(RULES_LOCK)) {
            Ok(lock) => break lock,
            Err(_) if Instant::now() < deadline => util::sleep(Duration::from_millis(5)),
            Err(error) => return Err(error),
        }
    };
    let mut rules = load();
    if hz == 0 {
        rules.remove(package);
    } else {
        rules.insert(package.into(), hz);
    }
    save(&rules)
}

/// dumpsys display 的模式列表：`modeId=N` 与 `refreshRate=F` 成对的片段。
/// 分辨率取 `width=/height=` 或 `WxH`；两者皆缺失时 pixels 记 0（排序时
/// 保守垫底）。解析不到任何成对片段返回空——调用方按"不支持"处理。
pub fn parse_display_modes(text: &str) -> Vec<DisplayMode> {
    let mut modes: Vec<DisplayMode> = Vec::new();
    let mut push = |mode: DisplayMode| {
        if let Some(existing) = modes.iter_mut().find(|m| m.id == mode.id) {
            // 同一 id 重复出现（虚拟屏/多段输出）：保留分辨率更高的一条。
            if mode.pixels > existing.pixels {
                *existing = mode;
            }
        } else {
            modes.push(mode);
        }
    };
    let normalized = text.replace(['\n', '\r', '[', ']', '{', '}'], " ");
    let fields = normalized.split_whitespace();
    let mut pending_id: Option<u32> = None;
    let mut pending_hz: Option<f32> = None;
    let mut pending_pixels: u64 = 0;
    for field in fields {
        if let Some(id) = field.strip_prefix("modeId=").and_then(|v| v.trim_end_matches(',').parse::<u32>().ok()) {
            if let (Some(id), Some(hz)) = (pending_id, pending_hz) {
                push(DisplayMode { id, hz, pixels: pending_pixels }); // 上一个片段收尾
            }
            pending_id = Some(id);
            pending_hz = None;
            pending_pixels = 0;
            continue;
        }
        if let Some(hz) = field.strip_prefix("refreshRate=").and_then(|v| v.trim_end_matches(',').parse::<f32>().ok()) {
            pending_hz = Some(hz);
            continue;
        }
        if let Some(width) = field.strip_prefix("width=").and_then(|v| v.trim_end_matches(',').parse::<u64>().ok()) {
            // height= 通常紧随其后；单独 width 先记宽度，height= 到来相乘。
            pending_pixels = width;
            continue;
        }
        if let Some(height) = field.strip_prefix("height=").and_then(|v| v.trim_end_matches(',').parse::<u64>().ok()) {
            pending_pixels = pending_pixels.saturating_mul(height);
            continue;
        }
        // `1440x3200@120` 或 `1440x3200` 紧凑写法。
        if let Some((width, height, hz)) = parse_compact(field) {
            if pending_hz.is_none() {
                pending_hz = Some(hz);
            }
            pending_pixels = width.saturating_mul(height);
        }
    }
    if let (Some(id), Some(hz)) = (pending_id, pending_hz) {
        push(DisplayMode { id, hz, pixels: pending_pixels });
    }
    modes.retain(|mode| mode.hz > 20.0 && mode.hz <= 250.0);
    modes.sort_by_key(|mode| mode.id);
    modes
}

fn parse_compact(field: &str) -> Option<(u64, u64, f32)> {
    let compact = field.trim_end_matches(',');
    let (size, hz) = match compact.split_once('@') {
        Some((size, hz)) => (size, hz.parse::<f32>().ok()?),
        None => (compact, 0.0),
    };
    let (width, height) = size.split_once('x')?;
    let width = width.parse::<u64>().ok()?;
    let height = height.parse::<u64>().ok()?;
    Some((width, height, hz))
}

/// 设备真实显示模式（dumpsys display，缓存 30 秒）。
pub fn display_modes() -> Result<Vec<DisplayMode>> {
    static CACHE: std::sync::OnceLock<
        std::sync::Mutex<Option<(Instant, Result<Vec<DisplayMode>>)>>,
    > = std::sync::OnceLock::new();
    let cache = CACHE.get_or_init(|| std::sync::Mutex::new(None));
    if let Ok(guard) = cache.lock() {
        if let Some((at, modes)) = guard.as_ref() {
            if at.elapsed() < MODES_CACHE {
                return match modes {
                    Ok(modes) => Ok(modes.clone()),
                    Err(error) => Err(error.clone()),
                };
            }
        }
    }
    let result = (|| {
        let text = util::query_command_limited(
            "/system/bin/dumpsys",
            &["display"],
            DUMPSYS_TIMEOUT,
            262_144,
        )?;
        let modes = parse_display_modes(&text);
        if modes.is_empty() {
            return Err("未能从 dumpsys display 解析出任何显示模式（格式未知或权限不足），按不支持处理".into());
        }
        Ok(modes)
    })();
    if let Ok(mut guard) = cache.lock() {
        *guard = Some((Instant::now(), result.clone()));
    }
    result
}

/// 当前激活模式 id（`mActiveModeId=N`）；解析不到返回 None。
pub fn active_mode() -> Option<u32> {
    let text = util::query_command_limited(
        "/system/bin/dumpsys",
        &["display"],
        DUMPSYS_TIMEOUT,
        262_144,
    )
    .ok()?;
    for line in text.lines() {
        if let Some(value) = line.trim().split_once("mActiveModeId=") {
            let raw = value.1.split_whitespace().next().unwrap_or("");
            if let Ok(id) = raw.trim_end_matches(',').parse::<u32>() {
                return Some(id);
            }
        }
    }
    None
}

/// 目标 Hz → 模式 id：命中 refreshRate 的模式里取分辨率最高者（用户要的
/// 是该刷新率下的原生分辨率）。没有命中返回 None（不猜近似值）。
pub fn resolve_mode_id(modes: &[DisplayMode], target_hz: u32) -> Option<u32> {
    modes
        .iter()
        .filter(|mode| (mode.hz - target_hz as f32).abs() < 0.51)
        .max_by_key(|mode| (mode.pixels, std::cmp::Reverse(mode.id)))
        .map(|mode| mode.id)
}

/// 下发一次激活模式切换并回读确认。失败带具体原因。
pub fn apply_mode_id(mode_id: u32) -> Result<()> {
    util::run_command(
        "/system/bin/service",
        &["call", "SurfaceFlinger", &SF_SET_ACTIVE_CODE.to_string(), "i32", &mode_id.to_string()],
    )?;
    let observed = active_mode();
    match observed {
        Some(id) if id == mode_id => Ok(()),
        Some(id) => Err(format!(
            "SurfaceFlinger 已接受请求但激活模式未切换（目标 {mode_id}，读回 {id}）"
        )),
        None => Err("切换后无法从 dumpsys display 读回激活模式，未确认生效".into()),
    }
}

/// 状态输出（CLI / WebUI 探测）：规则 + 设备真实模式列表。
pub fn status() -> String {
    let mut out = String::new();
    let rules = load();
    out.push_str(&format!("rules_count={}\n", rules.len()));
    for (package, hz) in &rules {
        out.push_str(&format!("rule_{package}={hz}\n"));
    }
    match display_modes() {
        Ok(modes) => {
            out.push_str(&format!("modes_count={}\n", modes.len()));
            for mode in &modes {
                out.push_str(&format!(
                    "mode_{}={:.0}Hz {}px\n",
                    mode.id,
                    mode.hz,
                    if mode.pixels > 0 { mode.pixels.to_string() } else { "未知".into() }
                ));
            }
        }
        Err(error) => out.push_str(&format!("modes_unavailable={error}\n")),
    }
    out.push_str("语义=按应用切换面板刷新率（SurfaceFlinger 激活显示模式，Scene 同款）；不是强制帧率上限，不改变游戏渲染目标\n");
    out
}

/// 前台切换的处理结果。Idle = 无规则且无需恢复（调用方下次可整段跳过）。
#[derive(Debug, PartialEq)]
pub enum RefreshOutcome {
    Idle,
    Applied(String),
    Restored,
    Failed(String),
}

/// 守护进程在前台切换时调用。有规则 → 切到目标 Hz 的模式；无规则/锁屏
/// （空包名）→ 回到 stock_mode（守护启动时的原激活模式）。已处于目标
/// 模式时不重复下发。
pub fn on_foreground(package: &str, stock_mode: Option<u32>) -> RefreshOutcome {
    let base = util::base_package(package);
    let target_hz = if util::valid_package(base) {
        load().get(base).copied().unwrap_or(0)
    } else {
        0
    };
    let target_id = if target_hz > 0 {
        let Ok(modes) = display_modes() else {
            return RefreshOutcome::Failed(format!(
                "刷新率：无法读取设备显示模式，{base} 的 {target_hz}Hz 规则暂不生效"
            ));
        };
        resolve_mode_id(&modes, target_hz)
    } else {
        stock_mode
    };
    let Some(target_id) = target_id else {
        return if target_hz > 0 {
            RefreshOutcome::Failed(format!(
                "刷新率：目标 {target_hz}Hz 不在设备模式列表中，保持当前刷新率（规则存在但无可匹配模式）"
            ))
        } else {
            RefreshOutcome::Idle
        };
    };
    if active_mode() == Some(target_id) {
        return if target_hz > 0 { RefreshOutcome::Idle } else { RefreshOutcome::Restored };
    }
    match apply_mode_id(target_id) {
        Ok(()) => {
            let hz_text = display_modes()
                .ok()
                .and_then(|modes| modes.iter().find(|mode| mode.id == target_id).map(|mode| format!("{:.0}", mode.hz)))
                .unwrap_or_else(|| "?".into());
            if target_hz > 0 {
                RefreshOutcome::Applied(format!("刷新率：{base} → 模式 {target_id}（{hz_text}Hz）"))
            } else {
                RefreshOutcome::Restored
            }
        }
        Err(error) => RefreshOutcome::Failed(format!("刷新率切换失败: {error}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rules_validate_packages_and_range() {
        let rules = parse_rules("{\"rules\":{\"com.game\":120,\"com.other\":0}}").unwrap();
        assert_eq!(rules.get("com.game"), Some(&120));
        assert!(rules.get("com.other").is_none(), "0 = 跟随系统，不入生效表");
        for bad in ["{\"rules\":{\"com.game\":5000}}", "{\"rules\":{\"com.game\":\"120\"}}", "{\"rules\":{\"bad pkg\":60}}", "not json"] {
            assert!(parse_rules(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn display_modes_parse_modeid_pairs_and_compact_forms() {
        // Android 12+ dumpsys display 风格：modeId 与 refreshRate 成对，
        // width/height 相邻；再验证 `WxH@Hz` 紧凑写法。
        let text = "mSupportedModes=[{modeId=1, width=1440, height=3200, refreshRate=60.0, \
            xDpi=504.0}\n{modeId=3, width=1440, height=3200, refreshRate=120.0}]\n\
            mSfDisplayModes=1|1440x3200@60 3|1440x3200@120\n";
        let modes = parse_display_modes(text);
        assert_eq!(modes.len(), 2);
        assert_eq!(modes[0].id, 1);
        assert_eq!(modes[0].hz, 60.0);
        assert_eq!(modes[0].pixels, 1440 * 3200);
        assert_eq!(modes[1].id, 3);
        assert_eq!(modes[1].hz, 120.0);
    }

    #[test]
    fn unparseable_output_reports_unsupported_instead_of_guessing() {
        assert!(parse_display_modes("Error: service unavailable").is_empty());
        assert!(parse_display_modes("modeId=1, refreshRate=nan").is_empty());
    }

    #[test]
    fn resolve_prefers_highest_resolution_exact_hz_match() {
        let modes = vec![
            DisplayMode { id: 1, hz: 60.0, pixels: 1440 * 3200 },
            DisplayMode { id: 2, hz: 120.0, pixels: 720 * 1600 },
            DisplayMode { id: 3, hz: 120.0, pixels: 1440 * 3200 },
            DisplayMode { id: 4, hz: 90.5, pixels: 999_999_999 },
        ];
        assert_eq!(resolve_mode_id(&modes, 120), Some(3));
        // 半 Hz 容差（面板标称 90.5/119.88 之类）。
        assert_eq!(resolve_mode_id(&modes, 90), Some(4));
        // 没有匹配 Hz 就不猜。
        assert_eq!(resolve_mode_id(&modes, 144), None);
    }
}
