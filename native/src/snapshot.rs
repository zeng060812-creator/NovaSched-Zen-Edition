use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::logging::Logger;
use crate::util::{self, Result};

const HEADER: &str = "NOVASCHED_ZEN_SNAPSHOT_V2";
const LEGACY_HEADER: &str = "NOVASCHED_ZEN_SNAPSHOT_V1";

#[derive(Clone, Debug)]
struct Entry {
    mode: u32,
    value: Vec<u8>,
}

#[derive(Clone)]
pub struct Snapshot {
    file: PathBuf,
    boot_id: String,
    entries: Arc<Mutex<BTreeMap<PathBuf, Entry>>>,
    properties: Arc<Mutex<BTreeMap<String, String>>>,
    logger: Logger,
    capture_lock: Arc<Mutex<()>>,
    transaction: Arc<Mutex<Option<Vec<(PathBuf, Entry)>>>>,
    #[cfg(test)]
    writes: Arc<Mutex<Vec<(PathBuf, String)>>>,
    #[cfg(test)]
    persists: Arc<std::sync::atomic::AtomicUsize>,
}

impl Snapshot {
    pub fn load(state_dir: &Path, logger: Logger) -> Result<Self> {
        #[cfg(target_os = "android")]
        let boot = util::read_trimmed("/proc/sys/kernel/random/boot_id")?;
        #[cfg(not(target_os = "android"))]
        let boot = util::read_trimmed("/proc/sys/kernel/random/boot_id").unwrap_or_default();
        Self::load_for_boot(state_dir, logger, &boot)
    }

    fn load_for_boot(state_dir: &Path, logger: Logger, boot: &str) -> Result<Self> {
        util::create_dir(state_dir)?;
        let file = state_dir.join("stock_snapshot.tsv");
        let mut renew = false;
        let (entries, properties) = if file.exists() {
            let text = util::read_text(&file)?;
            let (entries, properties, saved_boot) = parse(&text)?;
            if !boot.is_empty() && saved_boot.as_deref() != Some(boot) {
                util::atomic_write(
                    &state_dir.join("stock_snapshot.previous.tsv"),
                    text.as_bytes(),
                    0o600,
                )?;
                logger.info("开机周期已改变或旧快照没有开机标识，重新采集当前内核原始值");
                renew = true;
                Default::default()
            } else {
                (entries, properties)
            }
        } else {
            Default::default()
        };
        let snapshot = Self {
            file,
            boot_id: boot.to_string(),
            entries: Arc::new(Mutex::new(entries)),
            properties: Arc::new(Mutex::new(properties)),
            logger,
            capture_lock: Arc::new(Mutex::new(())),
            transaction: Arc::new(Mutex::new(None)),
            #[cfg(test)]
            writes: Arc::new(Mutex::new(Vec::new())),
            #[cfg(test)]
            persists: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        };
        if renew {
            snapshot.persist()?;
        }
        Ok(snapshot)
    }

    pub fn write(&self, path: impl AsRef<Path>, value: &str) -> Result<()> {
        self.write_unlogged(path, value).map_err(|error| {
            self.logger.error(format!("sysfs/proc 写入失败: {error}"));
            error
        })
    }

    pub fn original_value(&self, path: &Path) -> Result<Option<String>> {
        let entries = self.entries.lock().map_err(|_| "快照锁损坏".to_string())?;
        entries
            .get(path)
            .map(|entry| {
                String::from_utf8(entry.value.clone())
                    .map_err(|error| format!("原厂节点 {} 不是 UTF-8: {error}", path.display()))
            })
            .transpose()
    }

    pub fn original_children(&self, directory: &Path) -> Result<Vec<(PathBuf, String)>> {
        let entries = self.entries.lock().map_err(|_| "快照锁损坏".to_string())?;
        entries
            .iter()
            .filter(|(path, _)| path.parent() == Some(directory))
            .map(|(path, entry)| {
                Ok((
                    path.clone(),
                    String::from_utf8(entry.value.clone()).map_err(|error| {
                        format!("原厂节点 {} 不是 UTF-8: {error}", path.display())
                    })?,
                ))
            })
            .collect()
    }

    /// Lets callers classify expected optional-kernel failures before deciding
    /// whether they are errors. Snapshot capture still happens first.
    pub fn write_unlogged(&self, path: impl AsRef<Path>, value: &str) -> Result<()> {
        let path = path.as_ref();
        self.capture(path)?;
        {
            let mut guard = self.transaction.lock().map_err(|_| "下发事务锁损坏")?;
            if let Some(entries) = guard.as_mut() {
                if !entries.iter().any(|(p, _)| p == path) {
                    let value = util::read_bytes(path)?;
                    let mode = fs::metadata(path).map_err(|e| e.to_string())?.mode() & 0o7777;
                    entries.push((path.to_path_buf(), Entry { mode, value }));
                }
            }
        }
        util::write_node(path, value)?;
        #[cfg(test)]
        self.writes
            .lock()
            .unwrap()
            .push((path.to_path_buf(), value.to_string()));
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn written_nodes(&self) -> Vec<(PathBuf, String)> {
        self.writes.lock().unwrap().clone()
    }

    #[cfg(test)]
    pub(crate) fn persist_count(&self) -> usize {
        self.persists.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Capture all known startup nodes durably before the first policy write.
    /// Nodes created later still use capture()'s save-before-write fallback.
    pub fn capture_batch(
        &self,
        paths: impl IntoIterator<Item = PathBuf>,
        services: &[String],
    ) -> Result<()> {
        let _capture = self.capture_lock.lock().map_err(|_| "快照保存锁损坏")?;
        let mut nodes = BTreeMap::new();
        let mut properties = BTreeMap::new();
        {
            let existing = self.entries.lock().map_err(|_| "快照锁损坏")?;
            for path in paths {
                if existing.contains_key(&path) || nodes.contains_key(&path) || !path.exists() {
                    continue;
                }
                let read = || -> Result<Entry> {
                    let value = util::read_bytes(&path)?;
                    let mode = fs::metadata(&path)
                        .map_err(|e| format!("读取节点属性 {} 失败: {e}", path.display()))?
                        .mode()
                        & 0o7777;
                    Ok(Entry { mode, value })
                };
                match read() {
                    Ok(entry) => {
                        nodes.insert(path, entry);
                    }
                    // Optional nodes can be unreadable. No write has happened;
                    // if later used, capture() must succeed before writing.
                    Err(error) => self
                        .logger
                        .warn(format!("启动预备快照跳过不可读节点: {error}")),
                }
            }
        }
        {
            let existing = self.properties.lock().map_err(|_| "服务快照锁损坏")?;
            for service in services {
                let key = format!("@service:{service}");
                if !existing.contains_key(&key) {
                    let status = util::getprop(&format!("init.svc.{service}"));
                    if matches!(status.as_str(), "running" | "restarting") {
                        properties.insert(key, status);
                    }
                }
            }
        }
        if nodes.is_empty() && properties.is_empty() {
            return Ok(());
        }
        self.entries
            .lock()
            .map_err(|_| "快照锁损坏")?
            .extend(nodes.clone());
        self.properties
            .lock()
            .map_err(|_| "服务快照锁损坏")?
            .extend(properties.clone());
        if let Err(error) = self.persist() {
            let mut entries = self.entries.lock().map_err(|_| "快照锁损坏")?;
            let mut saved = self.properties.lock().map_err(|_| "服务快照锁损坏")?;
            for path in nodes.keys() {
                entries.remove(path);
            }
            for key in properties.keys() {
                saved.remove(key);
            }
            return Err(error);
        }
        Ok(())
    }

    pub fn capture(&self, path: &Path) -> Result<()> {
        let _capture = self.capture_lock.lock().map_err(|_| "快照保存锁损坏")?;
        {
            let guard = self.entries.lock().map_err(|_| "快照锁损坏".to_string())?;
            if guard.contains_key(path) {
                return Ok(());
            }
        }
        if !path.exists() {
            return Ok(());
        }
        let value = util::read_bytes(path)?;
        let mode = fs::metadata(path)
            .map_err(|e| format!("读取节点属性 {} 失败: {e}", path.display()))?
            .mode()
            & 0o7777;
        {
            let mut guard = self.entries.lock().map_err(|_| "快照锁损坏".to_string())?;
            guard
                .entry(path.to_path_buf())
                .or_insert(Entry { mode, value });
        }
        if let Err(e) = self.persist() {
            self.entries.lock().map_err(|_| "快照锁损坏")?.remove(path);
            return Err(e);
        }
        Ok(())
    }

    pub fn capture_service(&self, name: &str) -> Result<()> {
        let _capture = self.capture_lock.lock().map_err(|_| "快照保存锁损坏")?;
        let key = format!("@service:{name}");
        {
            let guard = self
                .properties
                .lock()
                .map_err(|_| "服务快照锁损坏".to_string())?;
            if guard.contains_key(&key) {
                return Ok(());
            }
        }
        let value = util::getprop(&format!("init.svc.{name}"));
        {
            let mut guard = self
                .properties
                .lock()
                .map_err(|_| "服务快照锁损坏".to_string())?;
            guard.insert(key.clone(), value);
        }
        if let Err(e) = self.persist() {
            self.properties
                .lock()
                .map_err(|_| "快照锁损坏")?
                .remove(&key);
            return Err(e);
        }
        Ok(())
    }

    pub fn apply_transaction<T>(&self, operation: impl FnOnce() -> Result<T>) -> Result<T> {
        {
            let mut guard = self.transaction.lock().map_err(|_| "下发事务锁损坏")?;
            if guard.is_some() {
                return Err("拒绝嵌套事务".into());
            }
            *guard = Some(Vec::new());
        }
        let result = operation();
        let entries = self
            .transaction
            .lock()
            .map_err(|_| "下发事务锁损坏")?
            .take()
            .unwrap_or_default();
        match result {
            Ok(value) => Ok(value),
            Err(error) => {
                let mut failures = Vec::new();
                for (path, entry) in entries.iter().rev() {
                    if util::read_bytes(path).ok().as_deref() == Some(entry.value.as_slice()) {
                        continue;
                    }
                    if let Err(e) = util::write_node(path, &String::from_utf8_lossy(&entry.value)) {
                        failures.push(e);
                    }
                }
                if failures.is_empty() {
                    Err(format!("{error}；本次节点变更已回滚"))
                } else {
                    Err(format!("{error}；回滚未完成: {}", failures.join("；")))
                }
            }
        }
    }

    pub fn restore(&self) -> Result<usize> {
        let entries = self
            .entries
            .lock()
            .map_err(|_| "快照锁损坏".to_string())?
            .clone();
        let properties = self
            .properties
            .lock()
            .map_err(|_| "属性快照锁损坏".to_string())?
            .clone();
        let mut restored = 0usize;
        let mut failures = Vec::new();
        let mut max_first = std::collections::BTreeSet::new();
        let mut uclamp_min_first = std::collections::BTreeSet::new();
        let mut blocked_uclamp = std::collections::BTreeSet::new();
        let mut blocked = std::collections::BTreeSet::new();
        for (path, entry) in &entries {
            if path.file_name().and_then(|n| n.to_str()) == Some("sched_util_clamp_max")
                && path.exists()
            {
                if let Some(parent) = path.parent() {
                    let minimum = parent.join("sched_util_clamp_min");
                    if minimum.exists() {
                        let target = String::from_utf8_lossy(&entry.value).trim().parse::<u64>();
                        let current = util::read_trimmed(&minimum)
                            .and_then(|s| s.parse::<u64>().map_err(|e| e.to_string()));
                        match (target, current) {
                            (Ok(max), Ok(min)) => {
                                if max < min {
                                    uclamp_min_first.insert(parent.to_path_buf());
                                }
                            }
                            (target, current) => {
                                failures.push(format!(
                                    "恢复 uclamp 前读取/解析失败: {target:?}, {current:?}"
                                ));
                                blocked_uclamp.insert(parent.to_path_buf());
                            }
                        }
                    }
                }
            }
            if path.file_name().and_then(|n| n.to_str()) != Some("scaling_min_freq")
                || !path.exists()
            {
                continue;
            }
            if let Some(parent) = path.parent() {
                let target = String::from_utf8_lossy(&entry.value).trim().parse::<u64>();
                let current = util::read_trimmed(parent.join("scaling_max_freq"))
                    .and_then(|s| s.parse::<u64>().map_err(|e| e.to_string()));
                match (target, current) {
                    (Ok(min), Ok(max)) => {
                        if min > max {
                            max_first.insert(parent.to_path_buf());
                        }
                    }
                    (_, result) => {
                        failures.push(format!(
                            "恢复频率范围前读取/解析失败 {}: {result:?}",
                            parent.display()
                        ));
                        blocked.insert(parent.to_path_buf());
                    }
                }
            }
        }
        let mut ordered: Vec<(&PathBuf, &Entry)> = entries.iter().collect();
        ordered.sort_by_key(|(path, _)| {
            let higher = path
                .parent()
                .map(|p| max_first.contains(p))
                .unwrap_or(false);
            match path.file_name().and_then(|s| s.to_str()) {
                Some("sched_util_clamp_min") => {
                    if path.parent().is_some_and(|p| uclamp_min_first.contains(p)) {
                        4
                    } else {
                        5
                    }
                }
                Some("sched_util_clamp_max") => {
                    if path.parent().is_some_and(|p| uclamp_min_first.contains(p)) {
                        5
                    } else {
                        4
                    }
                }
                Some("scaling_min_freq") => {
                    if higher {
                        3
                    } else {
                        2
                    }
                }
                Some("scaling_max_freq") => {
                    if higher {
                        2
                    } else {
                        3
                    }
                }
                _ => restore_rank(path),
            }
        });
        for (path, entry) in ordered {
            if path
                .file_name()
                .and_then(|s| s.to_str())
                .is_some_and(|name| matches!(name, "sched_util_clamp_min" | "sched_util_clamp_max"))
                && path.parent().is_some_and(|p| blocked_uclamp.contains(p))
            {
                continue;
            }
            if path.parent().map(|p| blocked.contains(p)).unwrap_or(false) {
                continue;
            }
            if !path.exists() {
                continue;
            }
            let value = String::from_utf8_lossy(&entry.value);
            let result = if util::read_trimmed(path).ok().as_deref() == Some(value.trim()) {
                Ok(())
            } else {
                util::write_node(path, &value)
            };
            match result {
                Ok(()) => {
                    if let Err(error) = fs::set_permissions(
                        path,
                        std::os::unix::fs::PermissionsExt::from_mode(entry.mode),
                    ) {
                        failures.push(format!("恢复权限 {} 失败: {error}", path.display()));
                    }
                    restored += 1;
                }
                Err(error) => failures.push(error),
            }
        }
        for (name, value) in properties {
            if let Some(service_name) = name.strip_prefix("@service:") {
                let command = if matches!(value.as_str(), "running" | "restarting") {
                    "start"
                } else {
                    "stop"
                };
                match util::run_command(command, &[service_name]) {
                    Ok(_) => restored += 1,
                    Err(error) => failures.push(error),
                }
                continue;
            }
            let result = if value.is_empty() {
                util::run_command("resetprop", &["--delete", &name])
            } else {
                util::run_command("resetprop", &["-n", &name, &value])
            };
            match result {
                Ok(_) => restored += 1,
                Err(error) => failures.push(error),
            }
        }
        if failures.is_empty() {
            self.logger
                .info(format!("原厂快照恢复完成，共 {restored} 项"));
            Ok(restored)
        } else {
            for error in &failures {
                self.logger.error(format!("快照恢复失败: {error}"));
            }
            Err(format!("恢复完成但有 {} 项失败", failures.len()))
        }
    }

    pub fn count(&self) -> usize {
        let a = self.entries.lock().map(|v| v.len()).unwrap_or(0);
        let b = self.properties.lock().map(|v| v.len()).unwrap_or(0);
        a + b
    }

    fn persist(&self) -> Result<()> {
        #[cfg(test)]
        self.persists
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let entries = self.entries.lock().map_err(|_| "快照锁损坏".to_string())?;
        let properties = self
            .properties
            .lock()
            .map_err(|_| "属性快照锁损坏".to_string())?;
        let mut text = format!(
            "{HEADER}\nB\t{}\n",
            util::hex_encode(self.boot_id.as_bytes())
        );
        for (path, entry) in entries.iter() {
            text.push_str(&format!(
                "N\t{}\t{:o}\t{}\n",
                util::hex_encode(path.as_os_str().as_encoded_bytes()),
                entry.mode,
                util::hex_encode(&entry.value)
            ));
        }
        for (name, value) in properties.iter() {
            text.push_str(&format!(
                "P\t{}\t{}\n",
                util::hex_encode(name.as_bytes()),
                util::hex_encode(value.as_bytes())
            ));
        }
        util::atomic_write(&self.file, text.as_bytes(), 0o600)?;
        let parent = self.file.parent().ok_or("快照路径没有父目录")?;
        fs::File::open(parent)
            .and_then(|f| f.sync_all())
            .map_err(|e| format!("同步快照目录失败: {e}"))
    }
}

fn restore_rank(path: &Path) -> u8 {
    let text = path.to_string_lossy();
    if text.ends_with("/scaling_governor") {
        1
    } else if text.ends_with("/scaling_max_freq") {
        2
    } else if text.ends_with("/scaling_min_freq") {
        3
    } else if text.ends_with("/online") {
        10
    } else {
        5
    }
}

fn parse(
    text: &str,
) -> Result<(
    BTreeMap<PathBuf, Entry>,
    BTreeMap<String, String>,
    Option<String>,
)> {
    let mut lines = text.lines();
    let header = lines.next();
    if header != Some(HEADER) && header != Some(LEGACY_HEADER) {
        return Err("原厂快照头无效".to_string());
    }
    let mut entries = BTreeMap::new();
    let mut properties = BTreeMap::new();
    let mut boot = None;
    for (index, line) in lines.enumerate() {
        let fields: Vec<&str> = line.split('\t').collect();
        match fields.as_slice() {
            ["B", value] if header == Some(HEADER) && boot.is_none() => {
                boot = Some(
                    String::from_utf8(util::hex_decode(value)?)
                        .map_err(|_| "快照开机标识无效".to_string())?,
                );
            }
            ["N", path, mode, value] => {
                let path = String::from_utf8(util::hex_decode(path)?)
                    .map_err(|_| format!("快照第 {} 行路径 UTF-8 无效", index + 2))?;
                if !["/sys/", "/proc/sys/", "/dev/cpuset/", "/dev/cpuctl/"]
                    .iter()
                    .any(|prefix| path.starts_with(prefix))
                    || Path::new(&path)
                        .components()
                        .any(|p| matches!(p, std::path::Component::ParentDir))
                {
                    return Err(format!("快照第 {} 行路径非法", index + 2));
                }
                let mode =
                    u32::from_str_radix(mode, 8).map_err(|e| format!("快照权限无效: {e}"))?;
                entries.insert(
                    PathBuf::from(path),
                    Entry {
                        mode,
                        value: util::hex_decode(value)?,
                    },
                );
            }
            ["P", name, value] => {
                let name = String::from_utf8(util::hex_decode(name)?)
                    .map_err(|_| "快照属性名无效".to_string())?;
                let value = String::from_utf8(util::hex_decode(value)?)
                    .map_err(|_| "快照属性值无效".to_string())?;
                properties.insert(name, value);
            }
            _ => return Err(format!("快照第 {} 行格式无效", index + 2)),
        }
    }
    if header == Some(HEADER) && boot.is_none() {
        return Err("快照缺少开机标识".into());
    }
    Ok((entries, properties, boot))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_snapshot_format() {
        let text = "NOVASCHED_ZEN_SNAPSHOT_V1\nN\t2f7379732f74657374\t644\t3132330a\nP\t706572736973742e74657374\t31\n";
        let (nodes, properties, boot) = parse(text).expect("snapshot");
        assert!(boot.is_none());
        assert_eq!(
            nodes.get(Path::new("/sys/test")).expect("node").value,
            b"123\n"
        );
        assert_eq!(
            properties.get("persist.test").map(String::as_str),
            Some("1")
        );
    }

    #[test]
    fn boot_identity_retains_same_boot_and_renews_stale_or_unbound_snapshots() {
        let dir = std::env::temp_dir().join(format!("nova-snapshot-boots-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let nodes = "N\t2f7379732f74657374\t644\t3132330a\nP\t40736572766963653a7065726664\t72756e6e696e67\n";
        let current = format!("{HEADER}\nB\t{}\n{nodes}", util::hex_encode(b"boot-a"));
        fs::write(dir.join("stock_snapshot.tsv"), &current).unwrap();
        let same = Snapshot::load_for_boot(&dir, Logger::new(&dir), "boot-a").unwrap();
        assert_eq!(same.count(), 2);
        let new = Snapshot::load_for_boot(&dir, Logger::new(&dir), "boot-b").unwrap();
        assert_eq!(new.count(), 0);
        assert_eq!(
            fs::read_to_string(dir.join("stock_snapshot.previous.tsv")).unwrap(),
            current
        );
        let (_, _, boot) =
            parse(&fs::read_to_string(dir.join("stock_snapshot.tsv")).unwrap()).unwrap();
        assert_eq!(boot.as_deref(), Some("boot-b"));
        fs::write(
            dir.join("stock_snapshot.tsv"),
            format!("{LEGACY_HEADER}\n{nodes}"),
        )
        .unwrap();
        let legacy = Snapshot::load_for_boot(&dir, Logger::new(&dir), "boot-b").unwrap();
        assert_eq!(legacy.count(), 0);
        assert!(parse(&format!("{HEADER}\n")).is_err());
        assert!(parse(&format!("{HEADER}\nB\t61\nB\t62\n")).is_err());
        fs::remove_dir_all(dir).unwrap();
    }
}

#[cfg(test)]
mod regression_tests {
    use super::*;
    #[test]
    fn snapshot_persist_failure_blocks_every_policy_write() {
        let dir = std::env::temp_dir().join(format!("cts-snapshot-failure-{}", std::process::id()));
        util::create_dir(&dir).unwrap();
        let node = dir.join("node");
        fs::write(&node, b"old\n").unwrap();
        let snap = Snapshot::load(&dir, Logger::new(&dir)).unwrap();
        fs::create_dir(dir.join("stock_snapshot.tsv")).unwrap();
        for _ in 0..2 {
            assert!(snap.write_unlogged(&node, "new\n").is_err());
            assert_eq!(snap.count(), 0);
            assert_eq!(fs::read(&node).unwrap(), b"old\n");
        }
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn partial_policy_failure_rolls_back_previous_value() {
        let dir = std::env::temp_dir().join(format!("cts-transaction-{}", std::process::id()));
        util::create_dir(&dir).unwrap();
        let node = dir.join("node");
        fs::write(&node, b"old\n").unwrap();
        let snap = Snapshot::load(&dir, Logger::new(&dir)).unwrap();
        let result: Result<()> = snap.apply_transaction(|| {
            snap.write_unlogged(&node, "new\n")?;
            Err("injected next node failure".into())
        });
        assert!(result.unwrap_err().contains("本次节点变更已回滚"));
        assert_eq!(fs::read(&node).unwrap(), b"old\n");
        snap.apply_transaction(|| snap.write_unlogged(&node, "yes\n"))
            .unwrap();
        assert_eq!(fs::read(&node).unwrap(), b"yes\n");
        assert_eq!(
            snap.entries.lock().unwrap().get(&node).unwrap().value,
            b"old\n"
        );
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn restore_rejects_paths_outside_kernel_nodes() {
        for path in [
            "/data/adb/file",
            "/sys/../../data/adb/file",
            "/proc/123/mem",
        ] {
            let text = format!(
                "{HEADER}\nN\t{}\t644\t31\n",
                util::hex_encode(path.as_bytes())
            );
            assert!(parse(&text).is_err());
        }
    }
}

#[cfg(test)]
mod batch_tests {
    use super::*;
    use std::sync::atomic::Ordering;
    #[test]
    fn initial_capture_is_one_persist_before_any_writes_and_stock_is_never_replaced() {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "nova-snapshot-batch-{}-{stamp}",
            std::process::id()
        ));
        fs::create_dir_all(&dir).unwrap();
        let nodes: Vec<_> = (0..64).map(|i| dir.join(format!("node{i}"))).collect();
        for p in &nodes {
            fs::write(p, "stock").unwrap();
        }
        let snap = Snapshot::load(&dir, Logger::new(&dir)).unwrap();
        snap.capture_batch(nodes.clone(), &[]).unwrap();
        assert_eq!(snap.persists.load(Ordering::Relaxed), 1);
        assert!(snap.written_nodes().is_empty());
        let saved = fs::read_to_string(dir.join("stock_snapshot.tsv")).unwrap();
        assert_eq!(
            saved.lines().filter(|line| line.starts_with("N\t")).count(),
            64
        );
        assert!(saved.lines().any(|line| line.starts_with("B\t")));
        for p in &nodes {
            snap.write(p, "apply").unwrap();
        }
        assert_eq!(snap.persists.load(Ordering::Relaxed), 1);
        snap.capture_batch(nodes.clone(), &[]).unwrap();
        assert_eq!(snap.persists.load(Ordering::Relaxed), 1);
        assert_eq!(
            snap.original_value(&nodes[0]).unwrap().as_deref(),
            Some("stock")
        );
        let late = dir.join("late");
        fs::write(&late, "late0").unwrap();
        snap.write(&late, "late1").unwrap();
        assert_eq!(snap.persists.load(Ordering::Relaxed), 2);
        assert_eq!(
            snap.original_value(&late).unwrap().as_deref(),
            Some("late0")
        );
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn failed_batch_is_retryable_and_never_leaves_uncaptured_nodes_writable() {
        let dir = std::env::temp_dir().join(format!("nova-batch-fail-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let node = dir.join("node");
        fs::write(&node, "stock").unwrap();
        let snap = Snapshot::load(&dir, Logger::new(&dir)).unwrap();
        fs::create_dir(dir.join("stock_snapshot.tsv")).unwrap();
        assert!(snap.capture_batch([node.clone()], &[]).is_err());
        assert_eq!(snap.count(), 0);
        assert!(snap.write(&node, "apply").is_err());
        assert_eq!(fs::read_to_string(&node).unwrap(), "stock");
        fs::remove_dir(dir.join("stock_snapshot.tsv")).unwrap();
        snap.capture_batch([node.clone()], &[]).unwrap();
        snap.write(&node, "apply").unwrap();
        fs::remove_dir_all(dir).unwrap();
    }
}
