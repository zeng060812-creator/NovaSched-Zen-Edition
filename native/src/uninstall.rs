//! Stop writers and restore their changes before removing module-owned files.
use crate::{
    ffi, lifecycle,
    logging::Logger,
    process_identity, scene,
    snapshot::Snapshot,
    util::{self, Result, STATE_DIR},
};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

pub fn run(module: &Path) -> Result<()> {
    if unsafe { ffi::geteuid() } != 0 {
        return Err("卸载需要 root".into());
    }
    let prop = match util::read_text(&module.join("module.prop")) {
        Ok(prop) => prop,
        Err(error) => {
            // The manager may already have removed the module directory (or
            // an interrupted flash left it incomplete). The in-module binary
            // and identity checks are unavailable then, so degrade to data
            // cleanup only instead of failing the whole uninstall.
            println!("模块目录不可用（{error}），仅清理 NovaSched 运行数据。");
            return cleanup_data_only();
        }
    };
    if !prop.lines().any(|line| line == "id=NovaSched_Zen_Edition") {
        return Err("模块身份不匹配，取消清理".into());
    }
    util::atomic_write(&module.join(".uninstalling"), b"uninstall=1\n", 0o600)?;
    let state = Path::new(STATE_DIR);
    if fs::symlink_metadata(state).is_ok_and(|m| m.file_type().is_symlink()) {
        return Err("运行目录被替换为符号链接，拒绝访问链接目标".into());
    }
    println!("[1/4] 停止守护和启动监督。");
    let deadline = Instant::now() + Duration::from_secs(10);
    let _control = loop {
        match util::acquire_flock(&crate::storage::path(&state, "control.lock")) {
            Ok(lock) => break lock,
            Err(error) if Instant::now() >= deadline => {
                return Err(format!("等待控制锁失败：{error}"))
            }
            Err(_) => util::sleep(Duration::from_millis(100)),
        }
    };
    let stopped = lifecycle::stop_inner(module);
    if !idle(state)? {
        // Only an identity-verified pidfd may force a hung daemon to exit.
        // Keep stop.requested in place so the supervisor will not restart it.
        println!("  正常停止未完成，正在核验进程身份后结束卡住的守护。");
        match process_identity::verify(state, module)
            .and_then(|identity| identity.signal(ffi::SIGKILL))
        {
            Ok(()) => {
                let deadline = Instant::now() + Duration::from_secs(5);
                while !idle(state)? && Instant::now() < deadline {
                    util::sleep(Duration::from_millis(100));
                }
            }
            // Identity verification failing must not abort the uninstall:
            // require_idle below still guarantees nothing is deleted while a
            // live scheduler holds the locks.
            Err(error) => println!("  ! 无法核验或结束残留进程：{error}"),
        }
    }
    require_idle(state)?;
    if let Err(error) = stopped {
        println!("  停止记录：{error}；已确认所有运行锁释放，继续恢复。");
    }
    let _daemon = util::acquire_flock(&crate::storage::path(&state, "daemon.lock"))?;
    let _supervisor = util::acquire_flock(&crate::storage::path(&state, "supervisor.lock"))?;

    println!("[2/4] 恢复节点值、权限、属性和服务。");
    let mut warnings = Vec::new();
    let mut errors = Vec::new();
    if scene::detect().controls_locked() {
        // A foreign scheduler owns the nodes right now and this daemon has
        // not written anything since the takeover. Restoring the snapshot
        // would overwrite the foreign controller's live values.
        println!("  节点由外部调度器接管，NovaSched 自接管起未修改节点，跳过恢复。");
    } else if crate::storage::path(&state, "stock_snapshot.tsv").exists() {
        match Snapshot::load(state, Logger::new(state)).and_then(|snapshot| snapshot.restore()) {
            Ok(count) => println!("  已恢复 {count} 项原厂设置。"),
            Err(error) => errors.push(format!("部分原厂设置未恢复：{error}")),
        }
    } else {
        println!("  没有原厂快照，本次无需恢复节点。");
    }

    println!("[3/4] Scene 收尾：恢复原回调，移除 NovaSched 自己的回调。");
    match scene::uninstall_provider() {
        Ok(notes) => warnings.extend(notes),
        Err(error) => errors.push(format!("Scene 清理未完成：{error}")),
    }

    if !errors.is_empty(){return Err(format!("恢复未完成，已保留配置、日志和 restore 恢复数据：{}；请重启后重试卸载",errors.join("；")));}
    println!("[4/4] 清理配置、规则、凭据、备份、缓存和日志。");
    let storage_roots = [
        PathBuf::from("/storage/emulated"),
        PathBuf::from("/data/media"),
    ];
    let mirrors = mirror_paths(&storage_roots);
    let pending = [
        PathBuf::from("/data/adb/modules_update/NovaSched_Zen_Edition"),
        PathBuf::from("/data/adb/ap/modules_update/NovaSched_Zen_Edition"),
    ];
    errors.extend(cleanup(module, state, &mirrors, &pending));
    for warning in warnings {
        println!("  ! {warning}");
    }
    if !errors.is_empty() {
        return Err(format!("卸载清理未完成：{}", errors.join("；")));
    }
    println!("  ✓ 模块运行目录、可访问的日志镜像和待更新副本已清理。");
    println!("NovaSched 数据已清理。模块本体由 root 管理器移除 · ZenJooo");
    Ok(())
}

fn idle(state: &Path) -> Result<bool> {
    Ok(!process_identity::lock_held(&crate::storage::path(&state, "daemon.lock"))?
        && !process_identity::lock_held(&crate::storage::path(&state, "supervisor.lock"))?)
}
fn require_idle(state: &Path) -> Result<()> {
    if idle(state)? {
        Ok(())
    } else {
        Err("仍有调度进程持锁，未删除恢复备份或运行锁；重启后再卸载".into())
    }
}

fn mirror_paths(roots: &[PathBuf]) -> Vec<PathBuf> {
    let mut result = vec![PathBuf::from("/sdcard/Android/NovaSched")];
    for root in roots {
        let Ok(users) = fs::read_dir(root) else {
            continue;
        };
        for user in users.flatten() {
            let name = user.file_name();
            let Some(name) = name.to_str() else { continue };
            if !name.is_empty()
                && name.bytes().all(|b| b.is_ascii_digit())
                && user.file_type().is_ok_and(|kind| kind.is_dir())
            {
                result.push(user.path().join("Android/NovaSched"));
            }
        }
    }
    result
}

fn remove_owned_tree(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.is_dir() => fs::remove_dir_all(path),
        Ok(_) => fs::remove_file(path),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(format!("读取 {} 失败：{error}", path.display())),
    }
    .map_err(|e| format!("清理 {} 失败：{e}", path.display()))
}

/// Pending module copies in modules_update must only be removed after
/// verifying they belong to this module; `exclude` protects the live module
/// directory when it is passed as one of the candidates.
fn remove_pending_module_copies(pending: &[PathBuf], exclude: &Path, errors: &mut Vec<String>) {
    for path in pending {
        if path == exclude {
            continue;
        }
        let missing_owned = path
            .file_name()
            .is_some_and(|name| name == "NovaSched_Zen_Edition")
            && !path.join("module.prop").exists();
        match util::read_text(&path.join("module.prop")) {
            Ok(prop) if prop.lines().any(|line| line == "id=NovaSched_Zen_Edition") => {
                if let Err(error) = remove_owned_tree(path) {
                    errors.push(error);
                }
            }
            Err(_) if missing_owned => {
                if let Err(error) = remove_owned_tree(path) {
                    errors.push(error);
                }
            }
            _ => {}
        }
    }
}

fn cleanup(module: &Path, state: &Path, mirrors: &[PathBuf], pending: &[PathBuf]) -> Vec<String> {
    let mut errors = Vec::new();
    // All supplied paths belong to this module. Never recurse into a symlink.
    for mirror in mirrors {
        if let Err(error) = remove_owned_tree(mirror) {
            errors.push(error);
        }
    }
    remove_pending_module_copies(pending, module, &mut errors);
    if let Ok(entries) = fs::read_dir(module) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name == "boot-entry.log"
                || (name.starts_with("install-")
                    && (name.ends_with(".log") || name.ends_with(".exit")))
            {
                if let Err(error) = remove_owned_tree(&entry.path()) {
                    errors.push(error);
                }
            }
        }
    }
    if let Err(error) = remove_owned_tree(state) {
        errors.push(error);
    }
    // Deferred cleanup scripts left in service.d from earlier interrupted
    // uninstalls would otherwise run forever on every boot.
    if let Err(error) = crate::storage::cleanup_deferred_scripts(Path::new("/data/adb/service.d")) {
        errors.push(error);
    }
    errors
}

/// Data-only cleanup used when the module directory itself is already gone:
/// stop signalling, snapshot restore and Scene teardown all require the
/// in-module binary, so only NovaSched-owned data can be removed safely.
fn cleanup_data_only() -> Result<()> {
    let state = Path::new(STATE_DIR);
    if fs::symlink_metadata(state).is_ok_and(|m| m.file_type().is_symlink()) {
        return Err("运行目录被替换为符号链接，拒绝访问链接目标".into());
    }
    if state.exists() && !idle(state)? {
        return Err("调度进程仍在运行，未删除运行数据；请先停止模块后再清理".into());
    }
    let mut errors = Vec::new();
    let mirrors = mirror_paths(&[
        PathBuf::from("/storage/emulated"),
        PathBuf::from("/data/media"),
    ]);
    for mirror in &mirrors {
        if let Err(error) = remove_owned_tree(mirror) {
            errors.push(error);
        }
    }
    let pending = [
        PathBuf::from("/data/adb/modules_update/NovaSched_Zen_Edition"),
        PathBuf::from("/data/adb/ap/modules_update/NovaSched_Zen_Edition"),
    ];
    remove_pending_module_copies(&pending, Path::new(""), &mut errors);
    if let Err(error) = remove_owned_tree(state) {
        errors.push(error);
    }
    if let Err(error) = crate::storage::cleanup_deferred_scripts(Path::new("/data/adb/service.d")) {
        errors.push(error);
    }
    if errors.is_empty() {
        println!("NovaSched 数据已清理（模块本体已由管理器移除）。");
        Ok(())
    } else {
        Err(format!("数据清理未完成：{}", errors.join("；")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;
    fn fixture(name: &str) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("nova-uninstall-{}-{name}", std::process::id()));
        crate::storage::prepare(&root).unwrap();
        root
    }
    #[test]
    fn removes_nested_state_logs_and_pending_upgrade_but_keeps_other_modules() {
        let root = fixture("all");
        let module = root.join("module");
        let state = root.join("state");
        let mirror = root.join("Android/NovaSched");
        let pending = root.join("update/NovaSched_Zen_Edition");
        let foreign = root.join("update/foreign");
        let incomplete = root.join("incomplete/NovaSched_Zen_Edition");
        for dir in [
            &module,
            &crate::storage::path(&state, "scene-provider"),
            &mirror,
            &pending,
            &foreign,
            &incomplete,
        ] {
            fs::create_dir_all(dir).unwrap();
        }
        for file in [
            "config.json",
            "automatic.json",
            "webui.session",
            "stock_snapshot.tsv",
            "daemon.identity",
            "novasched.log",
            "scene-provider/powercfg.json.backup",
        ] {
            let path=if file.starts_with("scene-provider/"){state.join("restore/scene/powercfg.json.backup")}else{crate::storage::path(&state,file)};
            fs::create_dir_all(path.parent().unwrap()).unwrap();fs::write(path,b"saved").unwrap();
        }
        fs::write(module.join("install-check.log"), b"log").unwrap();
        fs::write(module.join("install-scene-status.exit"), b"124").unwrap();
        fs::write(module.join("module.prop"), b"id=NovaSched_Zen_Edition\n").unwrap();
        fs::write(mirror.join("log.txt"), b"mirror").unwrap();
        fs::write(mirror.join("log.log.1"), b"old").unwrap();
        fs::write(pending.join("module.prop"), b"id=NovaSched_Zen_Edition\n").unwrap();
        fs::write(foreign.join("module.prop"), b"id=another_module\n").unwrap();
        assert!(cleanup(
            &module,
            &state,
            &[mirror.clone()],
            &[pending.clone(), foreign.clone(), incomplete.clone()]
        )
        .is_empty());
        assert!(!state.exists() && !mirror.exists() && !pending.exists());
        assert!(!incomplete.exists());
        assert!(foreign.join("module.prop").exists() && module.join("module.prop").exists());
        assert!(
            !module.join("install-check.log").exists()
                && !module.join("install-scene-status.exit").exists()
        );
        assert!(
            cleanup(&module, &state, &[mirror], &[pending]).is_empty(),
            "repeat uninstall must be harmless"
        );
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn removing_a_log_link_preserves_its_foreign_target() {
        let root = fixture("link");
        let other = root.join("other");
        fs::create_dir(&other).unwrap();
        fs::write(other.join("keep"), b"user data").unwrap();
        let owned = root.join("NovaSched");
        symlink(&other, &owned).unwrap();
        remove_owned_tree(&owned).unwrap();
        assert_eq!(fs::read(other.join("keep")).unwrap(), b"user data");
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn running_daemon_or_supervisor_blocks_deletion() {
        let state = fixture("locks");
        fs::write(crate::storage::path(&state, "stock_snapshot.tsv"), b"backup").unwrap();
        for file in ["daemon.lock", "supervisor.lock"] {
            let lock = util::acquire_flock(&state.join(file)).unwrap();
            assert!(require_idle(&state).is_err());
            assert!(crate::storage::path(&state, "stock_snapshot.tsv").exists());
            drop(lock);
        }
        require_idle(&state).unwrap();
        fs::remove_dir_all(state).unwrap();
    }
    #[test]
    fn finds_all_numeric_users_without_traversing_user_symlinks() {
        let root = fixture("users");
        let storage = root.join("emulated");
        for user in ["0", "10", "legacy", "self"] {
            fs::create_dir_all(storage.join(user)).unwrap();
        }
        symlink(storage.join("0"), storage.join("11")).unwrap();
        let paths = mirror_paths(&[storage.clone()]);
        assert!(paths.contains(&storage.join("0/Android/NovaSched")));
        assert!(paths.contains(&storage.join("10/Android/NovaSched")));
        assert!(!paths.iter().any(|p| p.starts_with(storage.join("11"))
            || p.starts_with(storage.join("self"))
            || p.starts_with(storage.join("legacy"))));
        fs::remove_dir_all(root).unwrap();
    }

}
