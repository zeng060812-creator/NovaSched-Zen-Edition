use std::collections::BTreeMap;
#[cfg(test)]
use std::fs;
use std::path::Path;
use std::sync::{Arc, Mutex};

use crate::config::MODES;
use crate::logging::Logger;
use crate::scene;
use crate::util::{self, Result, APP_MODES_PATH, CONFIG_PATH, MODE_PATH, RUNTIME_DIR};

pub type Rules = BTreeMap<String, String>;

pub fn scene_callback(mode: &str) -> Result<()> {
    let normalized = normalize_scene_action(mode);
    if normalized.is_none() && mode != "init" {
        return Ok(());
    }
    if !scene::detect().linked() {
        return Err("Scene 当前回调不属于 CTS，未修改档位".into());
    }
    let result = (|| {
        let boot = util::read_trimmed("/proc/sys/kernel/random/boot_id")?;
        if boot.is_empty() {
            return Err("无法识别当前启动会话".into());
        }
        if let Some(profile) = normalized {
            util::atomic_write(
                Path::new(MODE_PATH),
                format!("{profile}\n").as_bytes(),
                0o664,
            )?;
        }
        // Keep the action separate from mode.txt, which the daemon also updates.
        // A boot token prevents an old pedestal request surviving a reboot.
        util::atomic_write(
            Path::new(util::SCENE_REQUEST_PATH),
            format!("boot={boot}\naction={mode}\n").as_bytes(),
            0o600,
        )
    })();
    if let Err(error) = &result {
        Logger::new(Path::new(util::STATE_DIR)).error(format!("Scene 回调保存失败: {error}"));
    }
    result
}

fn normalize_scene_action(action: &str) -> Option<&str> {
    match action {
        "pedestal" => Some("fast"),
        "standby" => Some("powersave"),
        action if supported(action) => Some(action),
        _ => None,
    }
}

fn pedestal_request_matches(record: &str, boot: &str) -> bool {
    let mut record_boot = None;
    let mut action = None;
    for line in record.lines() {
        if let Some(value) = line.strip_prefix("boot=") {
            record_boot = Some(value);
        }
        if let Some(value) = line.strip_prefix("action=") {
            action = Some(value);
        }
    }
    !boot.is_empty() && record_boot == Some(boot) && action == Some("pedestal")
}

pub fn scene_pedestal_active(linked: bool) -> bool {
    if !linked {
        return false;
    }
    let Ok(record) = util::read_text(Path::new(util::SCENE_REQUEST_PATH)) else {
        return false;
    };
    let Ok(boot) = util::read_trimmed("/proc/sys/kernel/random/boot_id") else {
        return false;
    };
    pedestal_request_matches(&record, &boot)
}

pub fn select_local_mode(
    pedestal: bool,
    explicit: bool,
    configured: String,
    leaving_explicit: bool,
    default: String,
    saved: Option<String>,
) -> String {
    if pedestal {
        "fast".into()
    } else if explicit {
        configured
    } else if leaving_explicit {
        default
    } else {
        saved.unwrap_or(configured)
    }
}

#[derive(Clone)]
pub struct AppModes {
    rules: Arc<Mutex<Rules>>,
    logger: Logger,
    mutation: Arc<Mutex<()>>,
}

impl AppModes {
    pub fn initialize(module_dir: &Path, logger: Logger) -> Result<Self> {
        util::create_dir(Path::new(RUNTIME_DIR))?;
        let destination = Path::new(CONFIG_PATH);
        if !destination.exists() {
            let hardware = crate::hardware::detect()?;
            crate::profiles::initialize(module_dir, &hardware, Path::new(util::STATE_DIR))?;
        }
        if !Path::new(MODE_PATH).exists() {
            util::atomic_write(Path::new(MODE_PATH), b"balance\n", 0o664)?;
        }
        let mut rules = if scene::detect().linked() {
            match scene::load_rules() {
                Ok(Some(scene_rules)) => {
                    logger.info("从 Scene powercfg.xml 载入默认档位和应用规则");
                    scene_rules
                }
                Ok(None) => read_local_rules_or_empty()?,
                Err(error) => {
                    logger.warn(format!("读取 Scene 应用规则失败，回退到本地规则: {error}"));
                    read_local_rules_or_empty()?
                }
            }
        } else {
            read_local_rules_or_empty()?
        };
        if rules.is_empty() {
            let legacy = read_mode_file().unwrap_or_else(|| "balance".to_string());
            rules.insert("*".to_string(), legacy);
        }
        if !rules.contains_key("*") {
            rules.insert("*".to_string(), "balance".to_string());
        }
        let service = Self {
            rules: Arc::new(Mutex::new(rules)),
            logger,
            mutation: Arc::new(Mutex::new(())),
        };
        service.save(false)?;
        service.sync_scene_rules();
        Ok(service)
    }

    pub fn reload(&self) -> Result<bool> {
        let _mutation = self.mutation.lock().map_err(|_| "规则事务锁损坏")?;
        let loaded = read_rules(Path::new(APP_MODES_PATH))?;
        let changed = {
            let mut guard = self
                .rules
                .lock()
                .map_err(|_| "应用规则锁损坏".to_string())?;
            if *guard == loaded {
                false
            } else {
                *guard = loaded;
                true
            }
        };
        if changed {
            self.sync_scene_rules();
        }
        Ok(changed)
    }

    pub fn reload_from_scene(&self) -> Result<Option<bool>> {
        let Some(mut loaded) = scene::load_rules()? else {
            return Ok(None);
        };
        loaded
            .entry("*".to_string())
            .or_insert_with(|| read_mode_file().unwrap_or_else(|| "balance".to_string()));
        let _mutation = self.mutation.lock().map_err(|_| "规则事务锁损坏")?;
        let mut guard = self.rules.lock().map_err(|_| "应用规则锁损坏")?;
        if *guard == loaded {
            return Ok(Some(false));
        }
        self.save_rules(&loaded, false)?;
        *guard = loaded;
        Ok(Some(true))
    }

    pub fn resolve(&self, process: &str) -> String {
        let Ok(guard) = self.rules.lock() else {
            return "balance".to_string();
        };
        if let Some(mode) = guard.get(process) {
            return mode.clone();
        }
        if let Some((base, _)) = process.split_once(':') {
            if let Some(mode) = guard.get(base) {
                return mode.clone();
            }
        }
        guard
            .get("*")
            .cloned()
            .unwrap_or_else(|| "balance".to_string())
    }

    pub fn has_rule(&self, process: &str) -> bool {
        let Ok(guard) = self.rules.lock() else {
            return false;
        };
        guard.contains_key(process)
            || process
                .split_once(':')
                .map(|(base, _)| guard.contains_key(base))
                .unwrap_or(false)
    }

    pub fn default_mode(&self) -> String {
        self.resolve("")
    }

    pub fn rules(&self) -> Rules {
        self.rules
            .lock()
            .map(|value| value.clone())
            .unwrap_or_default()
    }

    pub fn set_rule(&self, package: &str, mode: &str) -> Result<()> {
        if !valid_rule_package(package) || !supported(mode) {
            return Err("应用规则非法".into());
        }
        self.change(
            |rules| {
                rules.insert(package.into(), mode.into());
            },
            package == "*",
            true,
        )?;
        self.logger
            .info(format!("应用规则已保存: {package} -> {mode}"));
        Ok(())
    }
    pub fn set_default_from_scene(&self, mode: &str) -> Result<bool> {
        if !supported(mode) {
            return Err("Scene 模式不支持".into());
        }
        self.change(
            |rules| {
                rules.insert("*".into(), mode.into());
            },
            true,
            false,
        )
    }
    pub fn remove_rule(&self, package: &str) -> Result<()> {
        if package == "*" || !util::valid_package(package) {
            return Err("不能删除该规则".into());
        }
        self.change(
            |rules| {
                rules.remove(package);
            },
            false,
            true,
        )?;
        Ok(())
    }
    fn change(
        &self,
        mutate: impl FnOnce(&mut Rules),
        update_mode: bool,
        sync: bool,
    ) -> Result<bool> {
        let _mutation = self.mutation.lock().map_err(|_| "规则事务锁损坏")?;
        let mut guard = self.rules.lock().map_err(|_| "规则锁损坏")?;
        let mut candidate = guard.clone();
        mutate(&mut candidate);
        if candidate == *guard {
            return Ok(false);
        }
        if let Err(error) = self.save_rules(&candidate, update_mode) {
            if let Err(rollback) = self.save_rules(&guard, false) {
                self.logger.error(format!("规则保存回滚失败: {rollback}"));
            }
            return Err(error);
        }
        *guard = candidate;
        drop(guard);
        if sync {
            self.sync_scene_rules();
        }
        Ok(true)
    }

    pub fn write_current_mode(&self, mode: &str) -> Result<()> {
        if !supported(mode) {
            return Err(format!("不支持的模式: {mode}"));
        }
        if read_mode_file().as_deref() == Some(mode) {
            return Ok(());
        }
        util::atomic_write(Path::new(MODE_PATH), format!("{mode}\n").as_bytes(), 0o664)
    }

    pub fn sync_to_scene(&self) {
        self.sync_scene_rules();
    }

    fn save(&self, update_mode: bool) -> Result<()> {
        let snapshot = self.rules();
        self.save_rules(&snapshot, update_mode)
    }

    fn save_rules(&self, snapshot: &Rules, update_mode: bool) -> Result<()> {
        let fallback = snapshot
            .get("*")
            .cloned()
            .unwrap_or_else(|| "balance".to_string());
        let mut text = format!("* {fallback}\n");
        for (package, mode) in snapshot {
            if package != "*" {
                text.push_str(&format!("{package} {mode}\n"));
            }
        }
        util::atomic_write(Path::new(APP_MODES_PATH), text.as_bytes(), 0o664)?;
        if update_mode {
            self.write_current_mode(&fallback)?;
        }
        Ok(())
    }

    fn sync_scene_rules(&self) {
        match scene::sync_rules(&self.rules()) {
            Ok(true) => self.logger.debug("CTS 应用规则已同步到 Scene"),
            Ok(false) => {}
            Err(error) => self
                .logger
                .warn(format!("同步 Scene 应用规则失败，已保留本地规则: {error}")),
        }
    }
}

pub fn supported(mode: &str) -> bool {
    MODES.contains(&mode)
}

pub fn read_mode_file() -> Option<String> {
    let value = util::read_trimmed(Path::new(MODE_PATH)).ok()?;
    supported(&value).then_some(value)
}

fn read_rules(path: &Path) -> Result<Rules> {
    let text = util::read_text(path)?;
    let mut rules = Rules::new();
    for (line_number, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut words = line.split_whitespace();
        let (Some(package), Some(mode), None) = (words.next(), words.next(), words.next()) else {
            return Err(format!("app_modes.txt 第 {} 行格式错误", line_number + 1));
        };
        if !valid_rule_package(package) || !supported(mode) {
            return Err(format!("app_modes.txt 第 {} 行规则非法", line_number + 1));
        }
        rules.insert(package.to_string(), mode.to_string());
    }
    if !rules.contains_key("*") {
        return Err("app_modes.txt 缺少默认规则 *".to_string());
    }
    Ok(rules)
}

fn read_local_rules_or_empty() -> Result<Rules> {
    if Path::new(APP_MODES_PATH).exists() {
        read_rules(Path::new(APP_MODES_PATH))
    } else {
        Ok(Rules::new())
    }
}

fn valid_rule_package(package: &str) -> bool {
    package == "*" || util::valid_package(package)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scene_actions_include_pedestal_and_standby() {
        assert_eq!(normalize_scene_action("pedestal"), Some("fast"));
        assert_eq!(normalize_scene_action("standby"), Some("powersave"));
        for mode in MODES {
            assert_eq!(normalize_scene_action(mode), Some(mode));
        }
        assert_eq!(normalize_scene_action("init"), None);
        assert_eq!(normalize_scene_action("unknown"), None);
    }

    #[test]
    fn pedestal_is_limited_to_current_boot_and_cleared_by_regular_callbacks() {
        assert!(pedestal_request_matches(
            "boot=abc\naction=pedestal\n",
            "abc"
        ));
        assert!(!pedestal_request_matches(
            "boot=abc\naction=pedestal\n",
            "def"
        ));
        assert!(!pedestal_request_matches(
            "boot=abc\naction=balance\n",
            "abc"
        ));
        assert!(!pedestal_request_matches("boot=abc\naction=init\n", "abc"));
        assert!(!pedestal_request_matches("action=pedestal\n", ""));
    }

    #[test]
    fn pedestal_overrides_app_rules_without_changing_normal_priority() {
        let choose = |pedestal, explicit, leaving| {
            select_local_mode(
                pedestal,
                explicit,
                "performance".into(),
                leaving,
                "balance".into(),
                Some("powersave".into()),
            )
        };
        assert_eq!(choose(true, true, false), "fast");
        assert_eq!(choose(false, true, false), "performance");
        assert_eq!(choose(false, false, true), "balance");
        assert_eq!(choose(false, false, false), "powersave");
    }

    #[test]
    fn parses_rules_and_base_processes() {
        let path = std::env::temp_dir().join(format!("cts-app-modes-{}", std::process::id()));
        fs::write(&path, b"* balance\ncom.example.game fast\n").expect("write rules");
        let rules = read_rules(&path).expect("parse rules");
        let _ = fs::remove_file(path);
        assert_eq!(rules.get("*"), Some(&"balance".to_string()));
        assert_eq!(rules.get("com.example.game"), Some(&"fast".to_string()));
    }
}
