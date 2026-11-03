//! ZenJooo · 按应用目标 FPS（调度目标，非强制限帧）。
//!
//! Android 没有公开的按应用 FPS 强制入口；这里的数值是**调度目标**——
//! 传给 NovaFAS 后作为帧基线，用于判断"当前帧率是否达标、要不要加压"：
//! - 应用被限制在 60 FPS 时设置目标 60，控制器不会为追逐 120Hz 屏幕持续加压；
//! - 达不到目标时按收益加压到档位上限，不无限提高底频；
//! - 目标由用户显式设置，控制器不根据平均帧率自动下调。
//! 它不改变游戏渲染上限，不能当作"已解锁/已限帧"。
use crate::json::{self, Value};
use crate::util::{self, Result};
use std::collections::BTreeMap;
use std::path::PathBuf;

pub const RULES_PATH: &str = "/data/adb/novasched/config/fps_rules.json";
/// 允许的目标档位：0 表示跟随应用/系统（不设置目标）。数值须与常见
/// 游戏帧率选项对应；不提供任意自定义值。
pub const ALLOWED: [u32; 9] = [0, 30, 40, 45, 60, 90, 120, 144, 165];

pub fn valid_target(value: u32) -> bool {
    ALLOWED.contains(&value)
}

pub fn parse_rules(text: &str) -> Result<BTreeMap<String, u32>> {
    let mut rules = BTreeMap::new();
    if text.trim().is_empty() {
        return Ok(rules);
    }
    let root = json::parse(text)?;
    let Value::Object(map) = &root else {
        return Err("FPS 规则文件根节点不是对象".into());
    };
    if let Some(Value::Object(rules_node)) = map.get("rules") {
        for (package, value) in rules_node {
            let Value::Number(fps) = value else {
                return Err(format!("FPS 规则 {package} 的值不是整数"));
            };
            if !valid_target(*fps as u32) {
                return Err(format!("FPS 规则 {package} 的目标无效: {fps}"));
            }
            if !util::valid_package(package) {
                return Err(format!("FPS 规则包名非法: {package}"));
            }
            if *fps > 0 {
                rules.insert(package.clone(), *fps as u32);
            }
        }
    }
    Ok(rules)
}

pub fn load() -> BTreeMap<String, u32> {
    parse_rules(&util::read_text(PathBuf::from(RULES_PATH).as_path()).unwrap_or_default())
        .unwrap_or_default()
}

/// 只序列化内层规则对象；`"rules"` 外壳由调用方（save/测试）负责。
/// 审计 P0-1：本函数曾自带 `"rules"` 外壳，save 再包一层后落盘变成
/// `{"rules":{"rules":…}}`，parse_rules 读回恒失败，目标 FPS 从不持久化。
/// 与 graphics.rs 的 serialize_rules 保持同一约定： serialize 不包外壳。
pub fn serialize(entries: &BTreeMap<String, u32>) -> String {
    let mut items = String::new();
    for (index, (package, fps)) in entries.iter().enumerate() {
        if index > 0 {
            items.push(',');
        }
        items.push_str(&format!("\"{package}\": {fps}"));
    }
    format!("{{{items}}}")
}

pub fn save(entries: &BTreeMap<String, u32>) -> Result<()> {
    let text = format!(
        "{{\n  \"schema\": \"novasched-fps/1\",\n  \"rules\": {}\n}}\n",
        serialize(entries)
    );
    util::atomic_write(PathBuf::from(RULES_PATH).as_path(), text.as_bytes(), 0o600)
}

/// set() 的读-改-写持锁（审计 F1）：WebUI 连续保存两条规则会并发起两个
/// CLI 进程，无锁的后写者覆盖前写者丢规则。有界重试与
/// web_auth::register_origin 同一模式。
const RULES_LOCK: &str = "/data/adb/novasched/run/fps-rules.lock";

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

pub fn set(package: &str, fps: u32) -> Result<()> {
    if !util::valid_package(package) {
        return Err(format!("包名非法: {package}"));
    }
    if !valid_target(fps) {
        return Err(format!("目标 FPS 无效: {fps}（可选 {}）", {
            let list: Vec<String> = ALLOWED.iter().map(u32::to_string).collect();
            list.join("/")
        }));
    }
    with_rules_lock(|| {
        let mut rules = load();
        if fps == 0 {
            rules.remove(package);
        } else {
            rules.insert(package.into(), fps);
        }
        save(&rules)
    })
}

/// 只读状态输出（诊断/WebUI 探测）：每行 rule_<包>=<目标>。
pub fn status() -> String {
    let rules = load();
    let mut out = String::from("kind=scheduling-target\n");
    out.push_str(&format!("rules_count={}\n", rules.len()));
    for (package, fps) in &rules {
        out.push_str(&format!("rule_{package}={fps}\n"));
    }
    out.push_str(
        "语义=调度目标（NovaFAS 帧基线），不是强制限帧；不改变游戏渲染上限\n",
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rules_parse_validate_and_skip_follow_system() {
        let rules = parse_rules("{\"rules\":{\"com.game\":60,\"com.other\":30}}").unwrap();
        assert_eq!(rules.get("com.game"), Some(&60));
        assert_eq!(rules.get("com.other"), Some(&30));
        // 0 = 跟随系统：合法但不进入生效表。
        let rules = parse_rules("{\"rules\":{\"com.game\":0}}").unwrap();
        assert!(rules.get("com.game").is_none());
        for bad in ["{\"rules\":{\"com.game\":61}}", "{\"rules\":{\"com.game\":\"60\"}}", "{\"rules\":{\"bad pkg\":60}}", "not json"] {
            assert!(parse_rules(bad).is_err(), "{bad}");
        }
        assert_eq!(parse_rules("").unwrap().len(), 0);
    }

    #[test]
    fn serialize_round_trips_through_parse() {
        let mut entries = BTreeMap::new();
        entries.insert("com.game".to_string(), 60u32);
        entries.insert("com.other".to_string(), 120u32);
        let wrapped = format!("{{\"rules\":{}}}", serialize(&entries));
        assert_eq!(parse_rules(&wrapped).unwrap(), entries);
    }

    #[test]
    fn saved_file_layout_round_trips_through_load_semantics() {
        // 审计 P0-1 回归：save() 落盘的真实版式必须能被 parse_rules 原样
        // 读回。曾因 serialize 自带 "rules" 外壳 + save 再包一层，盘上是
        // {"rules":{"rules":…}}，load() 恒为空，目标 FPS 从不持久化。
        let mut entries = BTreeMap::new();
        entries.insert("com.game".to_string(), 60u32);
        let on_disk = format!(
            "{{\n  \"schema\": \"novasched-fps/1\",\n  \"rules\": {}\n}}\n",
            serialize(&entries)
        );
        assert_eq!(parse_rules(&on_disk).unwrap(), entries);
        assert_eq!(serialize(&parse_rules(&on_disk).unwrap()), serialize(&entries));
    }

    #[test]
    fn target_whitelist_matches_common_game_options() {
        for value in [0, 30, 40, 45, 60, 90, 120, 144, 165] {
            assert!(valid_target(value), "{value}");
        }
        for value in [24, 50, 61, 75, 1000] {
            assert!(!valid_target(value), "{value}");
        }
    }
}
