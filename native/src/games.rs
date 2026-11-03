//! ZenJooo · 运行时游戏目录：分类器在编译期内置清单之外的合成来源。
//!
//! 三个只读来源按可用性合成，任何来源缺失都如实降级，绝不虚构包名：
//! 1. 模块自有存档 `/data/adb/novasched/config/games.json`（WebUI/CLI 增删，
//!    这是"自己写的"那份，永久保存、原子写、带锁）；
//! 2. Scene 遗留 `games.xml`（老版本 Scene 的 shared_prefs 游戏列表；键为
//!    包名的 `<string>` 条目。N1 2026.10 Beta10 已改用 SQLite，本文件通常
//!    不存在——找不到就跳过并在 status 里如实说明）；
//! 3. Scene 新版内置每应用刷新率表 `refresh_rate.txt`（APK 资产，运行期
//!    可能被解到 Scene 数据目录；格式 `包名 minHz maxHz`，出现即视为游戏）。
//!
//! 目录带 10 秒缓存；分类失败按"未知应用"处理，不影响档位下发。
use crate::json::{self, Value};
use crate::util::{self, Result};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

pub const OWN_PATH: &str = "/data/adb/novasched/config/games.json";
const OWN_LOCK: &str = "/data/adb/novasched/run/games.lock";
const CACHE_TTL: Duration = Duration::from_secs(10);
const MAX_OWN_ENTRIES: usize = 512;

/// Scene 数据目录里可能存放游戏列表的位置（legacy games.xml 与新版
/// refresh_rate.txt 都可能在 user / user_de 两个根下）。
fn scene_candidate_files(file: &str, user: Option<u32>) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    let user = match user {
        Some(user) => user,
        None => return paths,
    };
    for root in ["user", "user_de"] {
        for name in [crate::scene::SCENE_PACKAGE, crate::scene::SCENE_PACKAGE_FAMILY] {
            paths.push(PathBuf::from(format!(
                "/data/{root}/{user}/{name}/shared_prefs/{file}"
            )));
            paths.push(PathBuf::from(format!(
                "/data/{root}/{user}/{name}/files/{file}"
            )));
        }
    }
    paths
}

static CATALOG_CACHE: OnceLock<Mutex<Option<(Instant, BTreeSet<String>)>>> = OnceLock::new();

/// 合成后的运行时游戏目录（不含编译期内置清单）。缓存 10 秒。
pub fn catalog() -> BTreeSet<String> {
    let cache = CATALOG_CACHE.get_or_init(|| Mutex::new(None));
    if let Ok(guard) = cache.lock() {
        if let Some((at, catalog)) = guard.as_ref() {
            if at.elapsed() < CACHE_TTL {
                return catalog.clone();
            }
        }
    }
    let catalog = collect(util::current_android_user().ok());
    if let Ok(mut guard) = cache.lock() {
        *guard = Some((Instant::now(), catalog.clone()));
    }
    catalog
}

pub fn cataloged(package: &str) -> bool {
    catalog().contains(package)
}

pub fn invalidate_cache() {
    if let Ok(mut guard) = CATALOG_CACHE.get_or_init(|| Mutex::new(None)).lock() {
        *guard = None;
    }
}

fn collect(user: Option<u32>) -> BTreeSet<String> {
    let mut catalog = BTreeSet::new();
    // 模块自有存档（WebUI/CLI 维护）。
    if let Ok(text) = util::read_text(Path::new(OWN_PATH)) {
        for package in parse_own(&text).unwrap_or_default() {
            catalog.insert(package);
        }
    }
    // Scene 遗留 games.xml：shared_prefs 里键为包名的 string 条目。
    for path in scene_candidate_files("games.xml", user) {
        if let Ok(text) = util::read_text(&path) {
            for package in parse_games_xml(&text) {
                catalog.insert(package);
            }
            break; // user 根优先，命中即止。
        }
    }
    // Scene 新版每应用刷新率表：`包名 minHz maxHz`，出现即游戏。
    for path in scene_candidate_files("refresh_rate.txt", user) {
        if let Ok(text) = util::read_text(&path) {
            for package in parse_refresh_table(&text) {
                catalog.insert(package);
            }
            break;
        }
    }
    catalog
}

/// 模块自有存档：{"games":["com.pkg",…]}。
pub fn parse_own(text: &str) -> Result<BTreeSet<String>> {
    let mut games = BTreeSet::new();
    if text.trim().is_empty() {
        return Ok(games);
    }
    let root = json::parse(text)?;
    let Value::Object(map) = &root else {
        return Err("游戏存档根节点不是对象".into());
    };
    if let Some(Value::Array(items)) = map.get("games") {
        for item in items {
            let Value::String(package) = item else {
                return Err("游戏存档条目不是字符串".into());
            };
            if !util::valid_package(package) {
                return Err(format!("游戏存档包名非法: {package}"));
            }
            games.insert(package.clone());
        }
    }
    Ok(games)
}

/// 只追加/移除自有存档，不触碰 Scene 侧文件。带锁防止并发 CLI 互相覆盖。
pub fn set_own(package: &str, game: bool) -> Result<()> {
    if !util::valid_package(package) || package == "*" {
        return Err(format!("包名非法: {package}"));
    }
    let deadline = Instant::now() + Duration::from_millis(500);
    let _lock = loop {
        match util::acquire_flock(Path::new(OWN_LOCK)) {
            Ok(lock) => break lock,
            Err(_) if Instant::now() < deadline => util::sleep(Duration::from_millis(5)),
            Err(error) => return Err(error),
        }
    };
    let mut games = parse_own(&util::read_text(Path::new(OWN_PATH)).unwrap_or_default())?;
    if game {
        if games.len() >= MAX_OWN_ENTRIES && !games.contains(package) {
            return Err("自有游戏存档最多 512 项".into());
        }
        games.insert(package.into());
    } else {
        games.remove(package);
    }
    let items = games
        .iter()
        .map(|package| format!("\"{package}\""))
        .collect::<Vec<_>>()
        .join(", ");
    let text = format!("{{\n  \"schema\": \"novasched-games/1\",\n  \"games\": [{items}]\n}}\n");
    util::atomic_write(Path::new(OWN_PATH), text.as_bytes(), 0o600)?;
    invalidate_cache();
    Ok(())
}

/// 遗留 games.xml：键为合法包名的 string 条目（值不关心——老 Scene 里是
/// 每游戏配置 JSON，这里只取"出现在列表里"这个事实）。
fn parse_games_xml(xml: &str) -> BTreeSet<String> {
    let mut games = BTreeSet::new();
    let mut cursor = 0usize;
    while let Some(relative) = xml[cursor..].find("<string") {
        let start = cursor + relative;
        let Some(tag_end) = xml[start..].find('>').map(|offset| start + offset) else {
            break;
        };
        let Some(close) = xml[tag_end..].find("</string>").map(|offset| tag_end + offset) else {
            break;
        };
        let tag = &xml[start..=tag_end];
        if let Some(name) = xml_name_attribute(tag) {
            // 目录匹配的是主包名（分类器先 base_package 再查表）；子进程
            // 键取其 base 后并入，不会重复计数。
            let base = util::base_package(name);
            if util::valid_package(base) {
                games.insert(base.to_string());
            }
        }
        cursor = close + "</string>".len();
    }
    games
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

/// Scene 新版每应用刷新率表：`包名 minHz maxHz`，`#` 注释行跳过。
fn parse_refresh_table(text: &str) -> BTreeSet<String> {
    let mut games = BTreeSet::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut fields = line.split_whitespace();
        let Some(package) = fields.next() else { continue };
        let valid = fields.next().is_some_and(|min| min.parse::<f32>().is_ok())
            && fields.next().is_some_and(|max| max.parse::<f32>().is_ok());
        if valid && util::valid_package(package) {
            games.insert(package.to_string());
        }
    }
    games
}

/// 只读状态（诊断/健康检查）：各来源命中数与样本证据。
pub fn status() -> String {
    let user = util::current_android_user().ok();
    let mut out = String::new();
    let own = parse_own(&util::read_text(Path::new(OWN_PATH)).unwrap_or_default())
        .unwrap_or_default();
    let mut scene_xml = 0usize;
    let mut xml_source = String::from("未找到");
    for path in scene_candidate_files("games.xml", user) {
        if let Ok(text) = util::read_text(&path) {
            scene_xml = parse_games_xml(&text).len();
            xml_source = path.display().to_string();
            break;
        }
    }
    let mut refresh_table = 0usize;
    let mut table_source = String::from("未找到（新版 Scene 内置表可能未解出）");
    for path in scene_candidate_files("refresh_rate.txt", user) {
        if let Ok(text) = util::read_text(&path) {
            refresh_table = parse_refresh_table(&text).len();
            table_source = path.display().to_string();
            break;
        }
    }
    let catalog = catalog();
    out.push_str(&format!("own_games={}\n", own.len()));
    out.push_str(&format!("scene_games_xml={scene_xml}\nscene_games_xml_source={xml_source}\n"));
    out.push_str(&format!("scene_refresh_table={refresh_table}\nscene_refresh_table_source={table_source}\n"));
    out.push_str(&format!("catalog_total={}\n", catalog.len()));
    out.push_str("说明=N1 2026.10 起 Scene 的每应用配置改用 SQLite（user_configure.db/scene_config3），\n");
    out.push_str("本模块读取遗留 games.xml 与内置 refresh_rate.txt；两者都不存在时仅用自有存档。\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn own_archive_validates_packages_and_rejects_garbage() {
        let games = parse_own("{\"games\":[\"com.miHoYo.Yuanshen\",\"com.tencent.tmgp.sgame\"]}").unwrap();
        assert_eq!(games.len(), 2);
        assert!(games.contains("com.miHoYo.Yuanshen"));
        assert!(parse_own("").unwrap().is_empty());
        assert!(parse_own("{\"games\":[\"bad pkg\"]}").is_err());
        assert!(parse_own("{\"games\":[\"com.pkg\",42]}").is_err());
        assert!(parse_own("[]").is_err());
    }

    #[test]
    fn legacy_games_xml_extracts_package_keys_only() {
        let xml = "<map>\n<string name=\"com.miHoYo.Yuanshen\">{\"mode\":1}</string>\n\
            <string name=\"theme\">purple</string>\n\
            <string name=\"com.tencent.tmgp.sgame\">x</string>\n\
            <string name=\"com.tencent.tmgp.sgame:service\">y</string>\n</map>\n";
        let games = parse_games_xml(xml);
        assert_eq!(games.len(), 2);
        assert!(games.contains("com.tencent.tmgp.sgame"));
        assert!(!games.contains("theme"));
        assert!(!games.contains("com.tencent.tmgp.sgame:service"));
    }

    #[test]
    fn refresh_table_requires_two_numeric_fields_and_valid_package() {
        let text = "# 注释\ncom.miHoYo.Yuanshen 60 60\ncom.bad pkg 60\ncom.tencent.tmgp.sgame 120 120\n\ngarbage\n";
        let games = parse_refresh_table(text);
        assert_eq!(games.len(), 2);
        assert!(games.contains("com.miHoYo.Yuanshen"));
        assert!(!games.contains("com.bad"));
    }
}
