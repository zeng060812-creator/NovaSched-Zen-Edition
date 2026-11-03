//! Foreground classification is separate from manual four-profile rules.
//! Automatic mode always chooses powersave unless the foreground is a game.
use std::collections::BTreeMap;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::json::{self, Value};
use crate::util::{self, Result};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AppKind {
    Game,
    Daily,
    Unknown,
}

impl AppKind {
    pub fn name(self) -> &'static str {
        match self {
            Self::Game => "game",
            Self::Daily => "daily",
            Self::Unknown => "unknown",
        }
    }
    pub fn mode(self) -> &'static str {
        if self == Self::Game {
            "fast"
        } else {
            "powersave"
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Preferences {
    enabled: bool,
    overrides: BTreeMap<String, AppKind>,
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            enabled: true,
            overrides: BTreeMap::new(),
        }
    }
}

#[derive(Clone)]
pub struct Automatic {
    inner: Arc<Mutex<Preferences>>,
    path: PathBuf,
}

impl Automatic {
    pub fn initialize(path: &Path) -> Result<Self> {
        let prefs = if path.exists() {
            read_preferences(path)?
        } else {
            Preferences::default()
        };
        let service = Self {
            inner: Arc::new(Mutex::new(prefs)),
            path: path.into(),
        };
        if !path.exists() {
            service.update(|_| Ok(()))?;
        }
        Ok(service)
    }

    pub fn enabled(&self) -> bool {
        self.inner.lock().map(|p| p.enabled).unwrap_or(true)
    }

    pub fn override_for(&self, process: &str) -> Option<AppKind> {
        self.inner
            .lock()
            .ok()?
            .overrides
            .get(util::base_package(process))
            .copied()
    }

    pub fn overrides_json(&self) -> String {
        self.inner
            .lock()
            .map(|p| {
                p.overrides
                    .iter()
                    .map(|(pkg, kind)| format!("\"{pkg}\":\"{}\"", kind.name()))
                    .collect::<Vec<_>>()
                    .join(",")
            })
            .unwrap_or_default()
    }

    pub fn set_enabled(&self, enabled: bool) -> Result<()> {
        self.update(|p| {
            p.enabled = enabled;
            Ok(())
        })
    }

    pub fn set_kind(&self, package: &str, kind: &str) -> Result<()> {
        if !valid_classification(package, kind) {
            return Err("应用分类格式无效".into());
        }
        self.update(|p| {
            if kind == "auto" {
                p.overrides.remove(package);
            } else {
                if p.overrides.len() >= 256 && !p.overrides.contains_key(package) {
                    return Err("应用分类最多保存 256 项".into());
                }
                p.overrides.insert(
                    package.into(),
                    if kind == "game" {
                        AppKind::Game
                    } else {
                        AppKind::Daily
                    },
                );
            }
            Ok(())
        })
    }

    pub fn reload(&self) -> Result<bool> {
        let mut guard = self.inner.lock().map_err(|_| "自动调度偏好锁损坏")?;
        let prefs = read_preferences(&self.path)?;
        let changed = *guard != prefs;
        *guard = prefs;
        Ok(changed)
    }

    fn update(&self, change: impl FnOnce(&mut Preferences) -> Result<()>) -> Result<()> {
        let mut guard = self.inner.lock().map_err(|_| "自动调度偏好锁损坏")?;
        let mut next = guard.clone();
        change(&mut next)?;
        let overrides = next
            .overrides
            .iter()
            .map(|(pkg, kind)| format!("\"{pkg}\":\"{}\"", kind.name()))
            .collect::<Vec<_>>()
            .join(",");
        let text = format!(
            "{{\"version\":1,\"enabled\":{},\"overrides\":{{{overrides}}}}}\n",
            next.enabled
        );
        util::atomic_write(&self.path, text.as_bytes(), 0o600)?;
        *guard = next;
        Ok(())
    }
}

pub fn valid_classification(package: &str, kind: &str) -> bool {
    package != "*"
        && util::valid_package(package)
        && !package.contains(':')
        && matches!(kind, "game" | "daily" | "auto")
}

fn read_preferences(path: &Path) -> Result<Preferences> {
    let mut text = String::new();
    File::open(path)
        .and_then(|f| f.take(131_073).read_to_string(&mut text))
        .map_err(|e| format!("读取自动调度偏好失败: {e}"))?;
    if text.len() > 131_072 {
        return Err("自动调度偏好超出限制".into());
    }
    parse_preferences(&text)
}

fn parse_preferences(text: &str) -> Result<Preferences> {
    let Value::Object(object) = json::parse(text)? else {
        return Err("自动调度偏好需要 JSON 对象".into());
    };
    if object
        .get("version")
        .ok_or("自动调度偏好缺少版本")?
        .as_i64()?
        != 1
    {
        return Err("自动调度偏好版本不支持".into());
    }
    let enabled = object.get("enabled").ok_or("缺少自动调度开关")?.as_bool()?;
    let Value::Object(overrides) = object.get("overrides").ok_or("缺少应用分类")? else {
        return Err("应用分类需要 JSON 对象".into());
    };
    if overrides.len() > 256 {
        return Err("应用分类超出 256 项限制".into());
    }
    let mut result = Preferences {
        enabled,
        overrides: BTreeMap::new(),
    };
    for (pkg, value) in overrides {
        let kind = value.as_str()?;
        if !valid_classification(pkg, kind) || kind == "auto" {
            return Err("保存的应用分类无效".into());
        }
        result.overrides.insert(
            pkg.clone(),
            if kind == "game" {
                AppKind::Game
            } else {
                AppKind::Daily
            },
        );
    }
    Ok(result)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Classification {
    pub kind: AppKind,
    pub source: &'static str,
}

#[derive(Default)]
pub struct Classifier {
    cache: BTreeMap<String, (Instant, Classification)>,
}

impl Classifier {
    #[cfg(test)]
    pub fn classify_with_for_test(
        &mut self,
        package: &str,
        query: impl FnOnce() -> Result<String>,
    ) -> AppKind {
        self.classify_with(package, None, |_| query()).kind
    }

    pub fn invalidate(&mut self) {
        self.cache.clear();
    }

    pub fn classify(&mut self, process: &str, preferences: &Automatic) -> Classification {
        self.classify_with(process, preferences.override_for(process), |pkg| {
            query_platform_metadata(pkg, |program, args| {
                util::query_command(program, args, Duration::from_secs(1))
            })
        })
    }

    fn classify_with(
        &mut self,
        process: &str,
        explicit: Option<AppKind>,
        query: impl FnOnce(&str) -> Result<String>,
    ) -> Classification {
        let package = util::base_package(process);
        if package.is_empty() {
            return Classification {
                kind: AppKind::Daily,
                source: "idle",
            };
        }
        if package == "*" || !util::valid_package(package) {
            return Classification {
                kind: AppKind::Unknown,
                source: "unknown",
            };
        }
        if let Some(kind) = explicit {
            return Classification {
                kind,
                source: "user",
            };
        }
        if known_game(package) {
            return Classification {
                kind: AppKind::Game,
                source: "catalog",
            };
        }
        if let Some((at, value)) = self.cache.get(package) {
            let ttl = if value.kind == AppKind::Unknown {
                30
            } else {
                600
            };
            if at.elapsed() < Duration::from_secs(ttl) {
                return value.clone();
            }
        }
        let kind = query(package)
            .ok()
            .and_then(|s| parse_platform_metadata(&s, package))
            .unwrap_or(AppKind::Unknown);
        let classification = Classification {
            kind,
            source: if kind == AppKind::Unknown {
                "unknown"
            } else {
                "android"
            },
        };
        if self.cache.len() >= 256 {
            self.cache.clear();
        }
        self.cache
            .insert(package.into(), (Instant::now(), classification.clone()));
        classification
    }
}

fn query_platform_metadata(
    package: &str,
    mut query: impl FnMut(&str, &[&str]) -> Result<String>,
) -> Result<String> {
    // AOSP's package dump often omits appCategory and FLAG_IS_GAME entirely.
    // The read-only GameManager command verifies ApplicationInfo.category for
    // the current user; supported game *modes* need not include performance.
    let game = query("/system/bin/cmd", &["game", "list-modes", package]);
    let output = match game {
        Ok(text) | Err(text) => text,
    };
    let status = parse_game_command(&output, package);
    if status == Some(AppKind::Game) {
        return Ok(output);
    }
    match query("/system/bin/dumpsys", &["package", package]) {
        Ok(text) if parse_game_metadata(&text, package).is_some() => Ok(text),
        _ if status == Some(AppKind::Daily) => Ok(output),
        _ => Err("系统没有返回可验证的应用游戏分类".into()),
    }
}

fn parse_game_command(text: &str, package: &str) -> Option<AppKind> {
    let prefix = format!("{package} current mode: ");
    for line in text.lines().map(str::trim) {
        if let Some(rest) = line.strip_prefix(&prefix) {
            // 格式不符的行只跳过本行，不能提前返回：末尾的“非游戏”证据检查必须有机会执行。
            let Some((mode, list)) = rest.split_once(", available game modes: [") else { continue; };
            let Some(list) = list.strip_suffix(']') else { continue; };
            if mode.parse::<i32>().is_ok()
                && (list.is_empty() || list.split(',').all(|n| n.trim().parse::<i32>().is_ok()))
            {
                return Some(AppKind::Game);
            }
        }
    }
    let negative = format!("Package {package} is not of game type, to use the game mode commands");
    text.contains(&negative).then_some(AppKind::Daily)
}

fn parse_platform_metadata(text: &str, package: &str) -> Option<AppKind> {
    parse_game_command(text, package).or_else(|| parse_game_metadata(text, package))
}

fn known_game(package: &str) -> bool {
    // Exact package IDs only. No names, substring guesses, running background
    // processes, launchers, game centres, or advertising SDKs count as games.
    // 编译期内置清单之外，合成运行时来源（模块自有存档 + Scene 的
    // games.xml / refresh_rate.txt，见 games.rs）；合成目录不可用或查不到
    // 都不算游戏，按原路径降级。
    if include_str!("../games.list")
        .lines()
        .any(|line| !line.starts_with('#') && line.trim() == package)
    {
        return true;
    }
    crate::games::cataloged(package)
}

fn parse_game_metadata(text: &str, package: &str) -> Option<AppKind> {
    let header = format!("Package [{package}] (");
    let mut in_package = false;
    let mut described = false;
    for line in text.lines().map(str::trim) {
        if line.starts_with("Package [") {
            if in_package {
                break;
            }
            in_package = line.starts_with(&header);
            continue;
        }
        if !in_package {
            continue;
        }
        for field in line.split_whitespace() {
            if let Some(value) = field.strip_prefix("category=") {
                if let Ok(category) = value.parse::<i32>() {
                    described = true;
                    if category == 0 {
                        return Some(AppKind::Game);
                    }
                }
            }
            if let Some(value) = field.strip_prefix("flags=0x") {
                if let Ok(flags) = u32::from_str_radix(value, 16) {
                    described = true;
                    if flags & (1 << 25) != 0 {
                        return Some(AppKind::Game);
                    }
                }
            }
        }
        if let Some(flags) = line
            .strip_prefix("flags=[")
            .and_then(|s| s.strip_suffix(']'))
        {
            if flags.split_whitespace().any(|s| {
                s == "IS_GAME"
                    || s.strip_prefix("0x")
                        .and_then(|hex| u32::from_str_radix(hex, 16).ok())
                        .is_some_and(|n| n & (1 << 25) != 0)
            }) {
                return Some(AppKind::Game);
            }
        }
    }
    described.then_some(AppKind::Daily)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn dump(package: &str, body: &str) -> String {
        format!("Packages:\n  Package [{package}] (abc):\n    {body}\n")
    }

    #[test]
    fn read_only_game_manager_query_recognizes_uncatalogued_system_games() {
        let mut calls = 0;
        let output = query_platform_metadata("com.example.newgame", |program, args| {
            calls += 1;
            assert_eq!(program, "/system/bin/cmd");
            assert_eq!(args, ["game", "list-modes", "com.example.newgame"]);
            Ok("com.example.newgame current mode: 1, available game modes: [1,4]".into())
        })
        .unwrap();
        assert_eq!(calls, 1);
        assert_eq!(
            parse_platform_metadata(&output, "com.example.newgame"),
            Some(AppKind::Game)
        );
        assert_eq!(
            parse_platform_metadata(&output, "com.example.newgame.fake"),
            None
        );
    }

    #[test]
    fn game_service_failure_is_unknown_and_non_game_exit_is_parsed_exactly() {
        assert!(query_platform_metadata("com.example.app", |_, _| Err(
            "Can't find service: game".into()
        ))
        .is_err());
        let output = query_platform_metadata("com.example.app", |program, _| {
            if program.ends_with("/cmd") { Err("查询 /system/bin/cmd 退出码 Some(255): Package com.example.app is not of game type, to use the game mode commands, it must specify game category in the manifest as android:appCategory=\"game\"".into()) }
            else { Err("dump unavailable".into()) }
        }).unwrap();
        assert_eq!(
            parse_platform_metadata(&output, "com.example.app"),
            Some(AppKind::Daily)
        );
        assert_eq!(
            parse_platform_metadata(&output, "com.example.app.fake"),
            None
        );
    }

    #[test]
    fn omitted_game_fields_in_aosp_flags_are_not_false_daily_evidence() {
        assert_eq!(
            parse_game_metadata(
                &dump(
                    "com.example.app",
                    "flags=[ HAS_CODE ALLOW_BACKUP ]\npkgFlags=[ HAS_CODE ]"
                ),
                "com.example.app"
            ),
            None
        );
    }

    #[test]
    fn android_game_category_and_legacy_game_flag_are_supported() {
        for body in [
            "category=0",
            "flags=[ HAS_CODE IS_GAME ]",
            "uid=10123 flags=0x2000000",
        ] {
            assert_eq!(
                parse_game_metadata(&dump("com.example.game", body), "com.example.game"),
                Some(AppKind::Game)
            );
        }
        assert_eq!(
            parse_game_metadata(
                &dump("com.example.chat", "category=4\nflags=[ HAS_CODE ]"),
                "com.example.chat"
            ),
            Some(AppKind::Daily)
        );
    }

    #[test]
    fn unrelated_package_or_permission_flags_cannot_mark_a_game() {
        let text = dump("com.other.game", "category=0")
            + &dump("com.example.chat", "pkgFlags=[ IS_GAME ]\ncategory=-1");
        assert_eq!(
            parse_game_metadata(&text, "com.example.chat"),
            Some(AppKind::Daily)
        );
        assert_eq!(
            parse_game_metadata("Error: can't find package", "com.example.chat"),
            None
        );
        assert_eq!(
            parse_game_metadata(
                &dump("com.example.chat.fake", "category=0"),
                "com.example.chat"
            ),
            None
        );
    }

    #[test]
    fn catalog_is_exact_and_child_processes_share_the_main_package() {
        let mut c = Classifier::default();
        assert_eq!(
            c.classify_with("com.tencent.tmgp.sgame:service", None, |_| panic!(
                "catalog needs no query"
            ))
            .kind,
            AppKind::Game
        );
        assert!(!known_game("com.tencent.tmgp.sgame.fake"));
        assert!(!known_game("com.xiaomi.gamecenter"));
        assert_eq!(
            c.classify_with("com.tencent.tmgp.sgame", Some(AppKind::Daily), |_| panic!())
                .kind,
            AppKind::Daily
        );
    }

    #[test]
    fn classification_is_cached_and_can_be_invalidated_after_package_changes() {
        let mut c = Classifier::default();
        assert_eq!(
            c.classify_with("com.example.app", None, |pkg| Ok(dump(pkg, "category=0")))
                .kind,
            AppKind::Game
        );
        assert_eq!(
            c.classify_with("com.example.app:worker", None, |_| panic!(
                "cache must avoid Binder"
            ))
            .kind,
            AppKind::Game
        );
        c.invalidate();
        assert_eq!(
            c.classify_with(
                "com.example.app",
                None,
                |_| Err("Binder unavailable".into())
            )
            .kind,
            AppKind::Unknown
        );
    }

    #[test]
    fn automatic_transitions_never_retain_fast_on_daily_idle_or_unknown() {
        let kinds = [
            AppKind::Game,
            AppKind::Daily,
            AppKind::Game,
            AppKind::Unknown,
            AppKind::Daily,
        ];
        let modes: Vec<_> = kinds.into_iter().map(AppKind::mode).collect();
        assert_eq!(
            modes,
            ["fast", "powersave", "fast", "powersave", "powersave"]
        );
    }

    #[test]
    fn preferences_round_trip_and_manual_off_survives_reinstall() {
        let dir = std::env::temp_dir().join(format!("novasched-auto-{}", util::current_pid()));
        let path = dir.join("automatic.json");
        let _ = std::fs::remove_dir_all(&dir);
        let p = Automatic::initialize(&path).unwrap();
        assert!(p.enabled());
        p.set_kind("com.example.app", "game").unwrap();
        p.set_enabled(false).unwrap();
        let restored = Automatic::initialize(&path).unwrap();
        assert!(!restored.enabled());
        assert_eq!(
            restored.override_for("com.example.app:worker"),
            Some(AppKind::Game)
        );
        restored.set_kind("com.example.app", "auto").unwrap();
        assert_eq!(restored.override_for("com.example.app"), None);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn malformed_preferences_and_injection_are_rejected() {
        for pkg in [
            "*",
            "com.example.game:worker",
            "com.example;reboot",
            "com.example\"bad",
        ] {
            assert!(!valid_classification(pkg, "game"));
        }
        assert!(parse_preferences("{\"version\":1,\"enabled\":\"yes\",\"overrides\":{}}").is_err());
        assert!(parse_preferences(
            "{\"version\":1,\"enabled\":true,\"overrides\":{\"com.example.app\":\"fast\"}}"
        )
        .is_err());
    }

    #[test]
    fn failed_preference_save_does_not_change_confirmed_memory_state() {
        let dir =
            std::env::temp_dir().join(format!("nova-auto-write-failure-{}", util::current_pid()));
        let parent = dir.join("state");
        let service = Automatic::initialize(&parent.join("automatic.json")).unwrap();
        std::fs::rename(&parent, dir.join("saved")).unwrap();
        std::fs::write(&parent, b"not a directory").unwrap();
        assert!(service.set_enabled(false).is_err());
        assert!(service.enabled());
        assert!(service.set_kind("com.example.game", "game").is_err());
        assert_eq!(service.override_for("com.example.game"), None);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
