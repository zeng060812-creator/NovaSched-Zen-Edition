use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use crate::app_modes::{self, AppModes};
use crate::automatic::{Automatic, Classification, Classifier};
use crate::config::Config;
use crate::event_loop::{EventLoop, WakeReason};
use crate::ffi;
use crate::logging::Logger;
use crate::scene;
use crate::scheduler::CpuctlLive;
use crate::scheduler::{self, Scheduler};
use crate::snapshot::Snapshot;
use crate::util::{
    self, Result, APP_MODES_PATH, CONFIG_PATH, MODE_PATH, SCENE_REQUEST_PATH, STATE_DIR,
};
use crate::websocket::{self, RuntimeState, WebState};
use crate::{lifecycle, process_identity};

static STOP_REQUESTED: AtomicBool = AtomicBool::new(false);
static RELOAD_REQUESTED: AtomicBool = AtomicBool::new(false);

extern "C" fn handle_signal(signal: i32) {
    if signal == ffi::SIGHUP {
        RELOAD_REQUESTED.store(true, Ordering::Relaxed);
    } else {
        STOP_REQUESTED.store(true, Ordering::Relaxed);
    }
}

fn recover_legacy_feed_fps(state_dir: &Path, logger: &Logger) {
    let record = crate::storage::path(state_dir, "feed-fps.saved");
    let Ok(text) = util::read_text(&record) else {
        return;
    };
    let mut restored = 0usize;
    let mut failed = false;
    for line in text.lines() {
        let mut fields = line.splitn(3, '\t');
        match (fields.next(), fields.next(), fields.next()) {
            (Some("node"), Some(node), Some(value))
                if [
                    "/sys/class/mi_display/disp-DSI-0/max_fps_clone",
                    "/sys/class/mi_display/dsi-0-mi_display/max_fps_clone",
                    "/sys/class/mi_display/disp-DSI-0/dynamic_fps",
                    "/sys/class/mi_display/dsi-0-mi_display/dynamic_fps",
                ]
                .contains(&node) =>
            {
                match fs::write(node, value.as_bytes()) {
                    Ok(()) => restored += 1,
                    Err(error) => {
                        failed = true;
                        logger.warn(format!("恢复旧信息流刷新率限制失败: {error}"));
                    }
                }
            }
            (Some("setting"), Some(key), Some(value))
                if matches!(key, "min_refresh_rate" | "peak_refresh_rate") =>
            {
                let result = if value == "null" || value.is_empty() {
                    util::run_command("settings", &["delete", "system", key])
                } else {
                    util::run_command("settings", &["put", "system", key, value])
                };
                match result {
                    Ok(_) => restored += 1,
                    Err(error) => {
                        failed = true;
                        logger.warn(format!("恢复旧信息流刷新率设置 {key} 失败: {error}"));
                    }
                }
            }
            _ => {
                failed = true;
                logger.warn("旧信息流刷新率恢复记录格式无效，保留记录以便后续检查");
            }
        }
    }
    if !failed {
        if let Err(error) = util::remove_if_exists(&record) {
            logger.warn(format!("清理旧信息流刷新率恢复记录失败: {error}"));
        }
        if restored > 0 {
            logger.warn(format!("已恢复 {restored} 项旧信息流锁帧设置"));
        }
    }
}

fn strip_legacy_feed_config(logger: &Logger) {
    let path = Path::new(CONFIG_PATH);
    let result = (|| -> Result<bool> {
        let text = fs::read_to_string(path).map_err(|error| error.to_string())?;
        let mut config = crate::json::parse(&text)?;
        let crate::json::Value::Object(root) = &mut config else {
            return Err("配置根节点不是对象".into());
        };
        let Some(crate::json::Value::Object(features)) = root.get_mut("features") else {
            return Ok(false);
        };
        if features.remove("feed_video").is_none() {
            return Ok(false);
        }
        let encoded = crate::json::stringify(&config)?;
        util::atomic_write(path, encoded.as_bytes(), 0o600)?;
        Ok(true)
    })();
    match result {
        Ok(true) => logger.info("已移除旧配置中的信息流锁帧选项，其他配置保持不变"),
        Ok(false) => {}
        Err(error) => logger.warn(format!("清理旧信息流锁帧配置失败（该配置已停用）: {error}")),
    }
}

pub fn run_daemon(module_dir: PathBuf) -> Result<()> {
    crate::boot_guard::ensure_ready()?;
    STOP_REQUESTED.store(false, Ordering::Relaxed);
    RELOAD_REQUESTED.store(false, Ordering::Relaxed);
    ffi::install_signal(ffi::SIGTERM, handle_signal);
    ffi::install_signal(ffi::SIGINT, handle_signal);
    ffi::install_signal(ffi::SIGHUP, handle_signal);
    ffi::ignore_signal(ffi::SIGPIPE);

    let state_dir = Path::new(STATE_DIR);
    util::create_dir(state_dir)?;
    let logger = Logger::new(state_dir);
    let _lock = util::acquire_flock(&crate::storage::path(&state_dir, "daemon.lock"))?;
    util::remove_if_exists(&crate::storage::path(&state_dir, "status"))?;
    logger.clear();
    let pid_path = crate::storage::path(&state_dir, "daemon.pid");
    util::atomic_write(
        &pid_path,
        format!("{}\n", util::current_pid()).as_bytes(),
        0o600,
    )?;
    let cleanup = PidCleanup(pid_path.clone());
    recover_legacy_feed_fps(state_dir, &logger);
    let identity = match process_identity::Identity::capture(util::current_pid()) {
        Ok(value) => value,
        Err(error) => return startup_fail(&logger, state_dir, "采集进程身份", error),
    };
    if let Err(error) = util::atomic_write(
        &crate::storage::path(&state_dir, "daemon.identity"),
        identity.encode().as_bytes(),
        0o600,
    ) {
        return startup_fail(&logger, state_dir, "保存进程身份", error);
    }

    logger.info(format!(
        "NovaSched Zen Edition {} 守护进程启动",
        env!("CARGO_PKG_VERSION")
    ));
    util::atomic_write(&crate::storage::path(&state_dir, "startup"), b"state=starting\n", 0o600)?;
    let hardware = match scheduler::preflight() {
        Ok(value) => value,
        Err(error) => return startup_fail(&logger, state_dir, "环境校验", error),
    };
    logger.info(format!(
        "环境校验通过: {} / {} / root 用户态守护；设备={}；证据={}",
        hardware.soc.name(),
        hardware.soc.id(),
        hardware.device,
        hardware.evidence
    ));
    match crate::profiles::initialize(&module_dir, &hardware, state_dir) {
        Ok(report) => logger.info(report.replace('\n', "；")),
        Err(error) => return startup_fail(&logger, state_dir, "选择处理器配置", error),
    }
    logger.info(scene::presence().evidence);
    match scene::install_provider(&module_dir) {
        Ok(outcome) => logger.info(outcome.to_string()),
        Err(error) => logger.warn(format!("Scene 注册失败: {error}")),
    }

    let app_modes = match AppModes::initialize(&module_dir, logger.clone()) {
        Ok(value) => value,
        Err(error) => return startup_fail(&logger, state_dir, "初始化应用规则", error),
    };
    let automatic = match Automatic::initialize(Path::new(util::AUTO_POLICY_PATH)) {
        Ok(value) => value,
        Err(error) => return startup_fail(&logger, state_dir, "初始化自动调度", error),
    };
    let mut config = match Config::load(Path::new(CONFIG_PATH)) {
        Ok(value) => value,
        Err(first_error) => {
            // 配置损坏自愈：隔离损坏文件，优先回滚到升级前备份，否则从
            // 模板重建。不自愈的结果是守护永远起不来、调度完全失效。
            match crate::profiles::recover_corrupt(&module_dir, &hardware, state_dir) {
                Ok(true) => {
                    logger.warn(format!(
                        "配置损坏（{first_error}），已隔离损坏文件并恢复可用配置"
                    ));
                    match Config::load(Path::new(CONFIG_PATH)) {
                        Ok(value) => value,
                        Err(error) => return startup_fail(&logger, state_dir, "读取配置", error),
                    }
                }
                Ok(false) => return startup_fail(&logger, state_dir, "读取配置", first_error),
                Err(error) => {
                    return startup_fail(&logger, state_dir, "配置损坏自愈", error)
                }
            }
        }
    };
    strip_legacy_feed_config(&logger);
    // Thread settings must come from the same adapted configuration as the
    // dispatcher; otherwise a disabled FAS engine can still be spawned.
    match hardware.adapt_config(&mut config) {
        Ok(changes) => for change in changes { logger.info(change); },
        Err(error) => return startup_fail(&logger, state_dir, "内核能力适配", error),
    }
    logger.set_level(&config.meta.loglevel);
    let snapshot = match Snapshot::load(state_dir, logger.clone()) {
        Ok(value) => value,
        Err(error) => return startup_fail(&logger, state_dir, "加载原厂快照", error),
    };
    let mut scene = scene::detect();
    if !scene.linked() {
        if let Err(error) = util::remove_if_exists(Path::new(SCENE_REQUEST_PATH)) {
            logger.warn(format!("清理上次失效的 Scene 请求失败: {error}"));
        }
    }
    if scene.controls_locked() {
        logger.info(format!(
            "检测到 {}，为避免双调度，WebUI 四档和应用规则已锁定",
            scene.source
        ));
        if scene.mode.is_none() {
            logger.warn("未在 Scene 配置中识别出四档模式，暂用本模块已保存的默认档位");
        }
    } else if scene.linked() {
        logger.info("已注册到 Scene：NovaSched Zen Edition 是当前调度回调");
        if let Some(mode) = scene.mode.as_deref() {
            if let Err(error) = app_modes.set_default_from_scene(mode) {
                logger.warn(format!("读取 Scene 默认档位失败，保留本地默认: {error}"));
            }
        }
    }
    let default_mode = if scene.controls_locked() {
        scene
            .mode
            .clone()
            .or_else(app_modes::read_mode_file)
            .unwrap_or_else(|| app_modes.default_mode())
    } else if local_automatic_enabled(automatic.enabled(), &scene) {
        "powersave".to_string()
    } else {
        app_modes.default_mode()
    };
    let selected_soc = hardware.soc;
    let samsung = hardware.samsung;
    let input_boost_cfg = config.functions.input_boost.clone();
    let fas_cfg = config.functions.fas.clone();
    let fas_policies = config.policy;
    let cpuctl_live = Arc::new(CpuctlLive::default());
    let mut scheduler = match Scheduler::new(
        config,
        snapshot.clone(),
        logger.clone(),
        hardware,
        cpuctl_live.clone(),
    ) {
        Ok(value) => value,
        Err(error) => return startup_fail(&logger, state_dir, "内核能力适配", error),
    };
    let extreme_power = Arc::new(AtomicBool::new(
        fs::read_to_string(crate::storage::path(&state_dir, "extreme-power"))
            .is_ok_and(|value| value.trim() == "1"),
    ));
    scheduler.set_extreme_power(extreme_power.load(Ordering::Relaxed));
    let mut boot_guard = match crate::boot_guard::BootGuard::begin(&module_dir, state_dir) {
        Ok(guard) => guard,
        Err(error) => return startup_fail(&logger, state_dir, "启动保护", error),
    };
    // A failed first dispatch must not kill the daemon. Vendor frequency
    // managers race the first ceiling write at boot, and exiting here turned
    // one rejected write into a dead module once the supervisor gave up.
    // Keep the WebUI connected on failure; the retry budget is bounded.
    let mut last_apply_error = if scene.controls_locked() {
        String::new()
    } else {
        match scheduler.initialize(&default_mode) {
            Ok(()) => String::new(),
            Err(error) => {
                logger.error(format!("首次策略下发失败，转入降级重试: {error}"));
                error
            }
        }
    };
    if last_apply_error.is_empty() {
        if let Err(error) = app_modes.write_current_mode(&default_mode) {
            logger.error(format!("写入初始模式失败，等待循环重试: {error}"));
        }
    }

    let runtime = RuntimeState::default();
    runtime.set_profile(selected_soc);
    runtime.set_samsung(samsung);
    let mut fas_status = scheduler.refresh_fas(!scene.controls_locked() && last_apply_error.is_empty());
    runtime.set_fas(&fas_status);
    let mut fas_probe_at = Instant::now() + Duration::from_secs(10);
    // 接管模型：检测到 Scene（含 N1 改名包）即注册为它的调度回调，Scene 里切
    // 档位由本模块执行；未安装 Scene 时由 WebUI 直接接管。
    runtime.set_scene_installed(scene.availability == crate::scene::SceneAvailability::Available);
    runtime.set_scene_presence(scene.availability);
    runtime.set_automatic_active(
        local_automatic_enabled(automatic.enabled(), &scene) && last_apply_error.is_empty(),
    );
    runtime.set_power_profile(scheduler.current_power_profile());
    let mut reported_extreme_power = extreme_power.load(Ordering::Relaxed);
    let mut reported_extreme_status = if !reported_extreme_power {
        "off"
    } else if !last_apply_error.is_empty() {
        "failed"
    } else if scene.controls_locked() {
        "unsupported"
    } else {
        scheduler.extreme_power_status()
    };
    let mut reported_extreme_error = last_apply_error.clone();
    runtime.set_extreme_power(
        reported_extreme_power,
        reported_extreme_status,
        &reported_extreme_error,
    );
    runtime.set_classification(&Classification {
        kind: crate::automatic::AppKind::Daily,
        source: "idle",
    });
    runtime.set(
        "",
        scheduler.current_mode(),
        scene.controller(),
        scene.controls_locked(),
        scene.linked(),
    );
    let revision = Arc::new(AtomicU64::new(1));
    let web_stop = Arc::new(AtomicBool::new(false));
    let web_state = WebState {
        app_modes: app_modes.clone(),
        automatic: automatic.clone(),
        runtime: runtime.clone(),
        revision: revision.clone(),
        extreme_power: extreme_power.clone(),
    };
    let web_thread = match websocket::start(web_state, logger.clone(), web_stop.clone()) {
        Ok((handle, port)) => {
            runtime.set_port(port);
            revision.fetch_add(1, Ordering::Relaxed);
            Some(handle)
        }
        Err(error) => {
            logger.error(format!("WebUI 启动失败，开始自动回滚: {error}"));
            if !scene.controls_locked() {
                if let Err(e) = snapshot.restore() {
                    logger.error(format!("WebUI 失败后回滚失败: {e}"));
                }
            }
            return startup_fail(&logger, state_dir, "启动 WebUI", error);
        }
    };
    // Touch-driven transient boost: input thread applies a short uclamp.min
    // pulse on activity and expires back to the dispatcher baseline. Skipped
    // while a foreign scheduler owns the nodes (allowed flag per tick). The
    // same touch edges also wake the main loop via an eventfd so UI bursts
    // (notification shade / Super Island) are recognised within one frame.
    let touch_fd = unsafe { ffi::eventfd(0, ffi::O_NONBLOCK | ffi::O_CLOEXEC) };
    let touch_file = (touch_fd >= 0).then(|| {
        unsafe { <std::fs::File as std::os::fd::FromRawFd>::from_raw_fd(touch_fd) }
    });
    let input_thread = if input_boost_cfg.enabled {
        let node = scheduler.input_boost_node();
        let stock = snapshot
            .original_value(&node)
            .ok()
            .flatten()
            .unwrap_or_else(|| "0".into());
        // 输入线程使用 dup 出的独立 fd：eventfd 本体归 EventLoop 所有，
        // 裸共享 fd 号会在任一端关闭后变成悬垂引用，写入误伤复用的 fd。
        let input_touch_fd = if touch_file.is_some() {
            unsafe { ffi::fcntl(touch_fd, ffi::F_DUPFD_CLOEXEC, 0) }
        } else {
            -1
        };
        let ctx = crate::input_boost::InputBoostContext {
            live: cpuctl_live.clone(),
            node,
            stock,
            stop: web_stop.clone(),
            snapshot: snapshot.clone(),
            logger: logger.clone(),
            touch_signal_fd: input_touch_fd,
        };
        match crate::input_boost::spawn(ctx) {
            Some(handle) => Some(handle),
            None => {
                logger.debug("输入突频未启用（无可用输入设备）");
                None
            }
        }
    } else {
        None
    };
    // NovaFAS 帧感知调度引擎：独立线程按帧信号在档位包络内动态调频。
    // 配置关闭或外部调度器接管时线程自转入待命，不写任何节点。
    let fas_shared = Arc::new(std::sync::Mutex::new(crate::frame_ctrl::FasShared::default()));
    let fas_config = Arc::new(std::sync::RwLock::new(fas_cfg.clone()));
    let fas_status_slot = Arc::new(std::sync::Mutex::new(crate::frame_ctrl::EngineStatus::default()));
    let fas_thread = if fas_cfg.enabled && !scene.controls_locked() {
        let ctx = crate::frame_ctrl::EngineCtx {
            fas: fas_config.clone(),
            shared: fas_shared.clone(),
            live: cpuctl_live.clone(),
            status: fas_status_slot.clone(),
            stop: web_stop.clone(),
            snapshot: snapshot.clone(),
            logger: logger.clone(),
            policies: fas_policies,
        };
        match crate::frame_ctrl::spawn(ctx) {
            Some(handle) => {
                logger.info("NovaFAS 帧感知调度引擎已启动");
                Some(handle)
            }
            None => {
                logger.debug("NovaFAS 未启动（无可调度的 CPU 簇）");
                None
            }
        }
    } else {
        None
    };
    if let Err(e) = util::remove_if_exists(&crate::storage::path(&state_dir, "last_error")) {
        logger.error(e);
    }
    if let Err(e) = util::atomic_write(&crate::storage::path(&state_dir, "startup"), b"state=running\n", 0o600) {
        logger.error(e);
    }

    let mut monitor = ForegroundMonitor::default();
    let mut classifier = Classifier::default();
    let mut stamps = FileStamps::new();
    let mut watcher = EventLoop::new(&watch_paths(), touch_file);
    logger.info(watcher.describe());
    let mut last_status = String::new();
    let mut status_written = Instant::now();
    let mut retry_at = Instant::now();
    let mut apply_failures = u8::from(!last_apply_error.is_empty());
    let mut suspended = scene.controls_locked() || !last_apply_error.is_empty();
    let mut first_scan = true;
    let mut node_audit = NodeAuditClock::new(Instant::now());
    let mut scene_probe_at = Instant::now() + scene::PRESENCE_INTERVAL;
    let mut current_extreme_power = extreme_power.load(Ordering::Relaxed);
    // 运行期间感知管理器的停用/卸载标记：守护继续写节点会与卸载流程并发
    // 冲突，且与"已停用"语义相悖。低频（15 秒）检查，命中即走正常停止
    // 与恢复路径。
    let mut module_flag_at = Instant::now() + Duration::from_secs(15);
    while !STOP_REQUESTED.load(Ordering::Relaxed) && !crate::storage::path(&state_dir, "stop.requested").exists() {
        if Instant::now() >= module_flag_at {
            module_flag_at = Instant::now() + Duration::from_secs(15);
            if module_dir.join("disable").exists()
                || module_dir.join("remove").exists()
                || module_dir.join(".uninstalling").exists()
            {
                logger.warn("模块已被标记停用或卸载，守护进程停止并恢复原厂节点");
                break;
            }
        }
        let wake = if first_scan {
            first_scan = false;
            WakeReason::Event
        } else {
            watcher.wait(crate::event_loop::FOREGROUND_FALLBACK)
        };
        let touch = watcher.take_touch_signal();
        let forced = RELOAD_REQUESTED.swap(false, Ordering::Relaxed);
        if !wake.needs_rescan() && !forced {
            continue;
        }
        let config_changed = stamps.changed(Path::new(CONFIG_PATH));
        let rules_changed = stamps.changed(Path::new(APP_MODES_PATH));
        let automatic_changed = stamps.changed(Path::new(util::AUTO_POLICY_PATH));
        let mode_changed = stamps.changed(Path::new(MODE_PATH));
        let requested_extreme_power = extreme_power.load(Ordering::Relaxed);
        let extreme_power_changed = requested_extreme_power != current_extreme_power;
        if extreme_power_changed {
            current_extreme_power = requested_extreme_power;
            scheduler.set_extreme_power(current_extreme_power);
        }
        let scene_request_changed = stamps.changed(Path::new(SCENE_REQUEST_PATH));
        let scene_files_changed = scene::watched_paths().iter().fold(false, |changed, path| {
            stamps.changed(Path::new(path)) || changed
        });
        let presence_due = Instant::now() >= scene_probe_at;
        if presence_due || forced || scene_files_changed {
            scene::invalidate_presence();
            scene_probe_at = Instant::now() + scene::PRESENCE_INTERVAL;
        }
        if scene_files_changed || forced {
            classifier.invalidate();
        }
        if automatic_changed || forced {
            if let Err(error) = automatic.reload() {
                logger.error(format!("自动调度偏好无效，保留上次偏好: {error}"));
            }
        }
        let mut scene_changed = scene_files_changed;

        if forced || config_changed {
            let fas_block = match Config::load(Path::new(CONFIG_PATH)) {
                Ok(config) => {
                    match scheduler.replace_config(config) {
                        Ok(()) => {
                            runtime.set_profile(selected_soc);
                            logger.info("config.json 已重新载入");
                            Some(scheduler.fas_config())
                        }
                        Err(error) => {
                            logger.error(format!("config.json 内核适配失败，保留当前配置: {error}"));
                            None
                        }
                    }
                }
                Err(error) => {
                    logger.error(format!("config.json 无效，保留当前配置: {error}"));
                    None
                }
            };
            if let Some(fas_block) = fas_block {
                if let Ok(mut slot) = fas_config.write() {
                    *slot = fas_block;
                }
            }
        }
        if rules_changed && !scene_changed {
            match app_modes.reload() {
                Ok(true) => logger.info("app_modes.txt 已重新载入"),
                Ok(false) => {}
                Err(error) => logger.error(format!("app_modes.txt 无效，保留当前规则: {error}")),
            }
        }
        if scene_changed || forced || presence_due {
            let previous_scene = scene.clone();
            scene = scene::detect();
            if scene::registration_needed(&scene, &previous_scene) {
                match scene::install_provider(&module_dir) {
                    Ok(outcome) => logger.info(outcome.to_string()),
                    Err(error) => logger.warn(format!("Scene 注册失败: {error}")),
                }
                scene = scene::detect();
            }
            scene_changed |= previous_scene != scene;
            runtime.set_scene_installed(
                scene.availability == crate::scene::SceneAvailability::Available,
            );
            runtime.set_scene_presence(scene.availability);
            if previous_scene.linked() && !scene.linked() {
                if let Err(error) = util::remove_if_exists(Path::new(SCENE_REQUEST_PATH)) {
                    logger.warn(format!("清理失效 Scene 请求失败: {error}"));
                }
            }
            if previous_scene != scene {
                logger.info(scene::presence().evidence);
                if scene.controls_locked() {
                    logger.info(format!(
                        "{} 配置已更新，为避免双调度，CTS 已锁定 WebUI 控制",
                        scene.source
                    ));
                    if scene.mode.is_none() {
                        logger.warn("Scene 配置未提供可识别的四档模式，保留当前档位");
                    }
                } else if scene.linked() {
                    logger.info("Scene 已接入 NovaSched Zen Edition 调度回调");
                } else {
                    logger.info("未检测到外部 Scene 调度器，WebUI 保持可用");
                }
            }
            if scene.linked() {
                match app_modes.reload_from_scene() {
                    Ok(Some(true)) => logger.info("Scene 默认档位和应用规则已重新载入"),
                    Ok(Some(false)) => {}
                    Ok(None) if !previous_scene.xml_available() && scene.xml_available() => {
                        app_modes.sync_to_scene();
                    }
                    Ok(None) => {}
                    Err(error) => logger.warn(format!("Scene 规则读取失败: {error}")),
                }
                if let Some(mode) = scene.mode.as_deref() {
                    if let Err(error) = app_modes.set_default_from_scene(mode) {
                        logger.warn(format!("同步 Scene 默认档位失败，保留本地默认: {error}"));
                    }
                }
            }
        }

        // The input-boost thread pulses only while no foreign scheduler owns
        // the nodes.
        if let Ok(mut state) = cpuctl_live.inner.lock() {
            state.allowed = !scene.controls_locked();
        }
        // Bounded, spaced retries yield to vendor controls when exhausted.
        if !scene.controls_locked() && last_apply_error.is_empty() && scheduler.enforce_pending() {
            revision.fetch_add(1, Ordering::Relaxed);
        }

        let automatic_enabled = local_automatic_enabled(automatic.enabled(), &scene);
        let foreground = monitor.detect(
            &app_modes,
            scheduler.ignored_packages(),
            forced || config_changed || rules_changed || scene_changed || automatic_changed,
            automatic_enabled && !scene.controls_locked(),
            touch,
        );
        let package = foreground
            .as_deref()
            .unwrap_or(&monitor.current)
            .to_string();
        let classification = if package.is_empty() && monitor.activity.is_none() {
            Classification {
                kind: crate::automatic::AppKind::Unknown,
                source: "foreground_unknown",
            }
        } else {
            classifier.classify(&package, &automatic)
        };
        runtime.set_classification(&classification);
        let (selected, explicit) = if scene.controls_locked() {
            let retained = if app_modes::supported(scheduler.current_mode()) {
                scheduler.current_mode().to_string()
            } else {
                app_modes.default_mode()
            };
            (
                scene
                    .mode
                    .clone()
                    .or_else(app_modes::read_mode_file)
                    .unwrap_or(retained),
                false,
            )
        } else if automatic_enabled {
            // Neither mode.txt nor a manual daily-app rule can keep
            // fast selected after the foreground game ends.
            (
                classification.kind.mode().to_string(),
                classification.kind == crate::automatic::AppKind::Game,
            )
        } else {
            let explicit = app_modes.has_rule(&package);
            let configured = app_modes.resolve(&package);
            let leaving_explicit =
                !explicit && package != scheduler.current_package() && scheduler.last_explicit();
            let selected = app_modes::select_local_mode(
                explicit,
                configured,
                leaving_explicit,
                app_modes.default_mode(),
                if automatic_changed {
                    None
                } else {
                    app_modes::read_mode_file()
                },
            );
            (selected, explicit)
        };
        if let Ok(mut state) = cpuctl_live.inner.lock() {
            state.active = !package.is_empty() && !scene.controls_locked();
        }
        let force_apply = forced
            || config_changed
            || rules_changed
            || automatic_changed
            || mode_changed
            || extreme_power_changed
            || scene_request_changed
            || scene_changed;
        if force_apply { apply_failures = 0; }
        if scene.controls_locked() {
            suspended = true;
            scheduler.clear_pending();
        } else if force_apply || (Instant::now() >= retry_at
            && (last_apply_error.is_empty() || (!samsung && apply_failures < 3))) {
            let mut result = if suspended {
                scheduler.initialize(&selected)
            } else {
                scheduler.apply(
                    &package,
                    &selected,
                    foreground.as_deref().is_some_and(|value| !value.is_empty()),
                    force_apply || !last_apply_error.is_empty(),
                    explicit,
                )
            };
            if node_audit.take_due(Instant::now()) && result.is_ok() {
                result = scheduler.audit_and_repair().map(|corrected| {
                    if corrected {
                        revision.fetch_add(1, Ordering::Relaxed);
                    }
                });
            }
            if let Err(error) = result {
                if error != last_apply_error {
                    logger.error(format!("调度下发失败: {error}"));
                }
                last_apply_error = error;
                apply_failures = apply_failures.saturating_add(1);
                retry_at = Instant::now() + Duration::from_secs(10);
                if samsung || apply_failures == 3 {
                    logger.warn("调度写入失败，已停止自动重试；保留诊断，可在 WebUI 手动切档后重试");
                }
            } else {
                suspended = false;
                last_apply_error.clear();
                apply_failures = 0;
                if let Err(error) = app_modes.write_current_mode(scheduler.current_mode()) {
                    logger.error(format!("写入 mode.txt 失败: {error}"));
                    last_apply_error = error;
                    retry_at = Instant::now() + Duration::from_secs(10);
                    apply_failures = 1;
                }
            }
        }
        if foreground.is_some() || force_apply {
            revision.fetch_add(1, Ordering::Relaxed);
        }

        // NovaFAS 共享上下文：档位、前台包、可写状态与档位频率底线每轮同步。
        {
            let allowed = !scene.controls_locked();
            // 待确认的频率上限（pending）不阻塞引擎：FAS 只抬最低频率，且
            // 以内核当前实际接受的最高频率为顶，天然落在已生效的包络内。
            // 之前把 pending 也算进就绪条件，厂商服务钳制上限的设备上引擎
            // 会永远待命，WebUI 看到的就是“接管了却一动不动”。
            // 桌面/系统界面不算可调度前台：待机停在桌面上时 FAS 不保持
            // 任何加压与底频，把待机功耗交给内核 idle。
            let idle_surface = is_idle_surface(scheduler.current_package());
            let ready = allowed && last_apply_error.is_empty() && !idle_surface;
            let floors = scheduler.profile_min_floors();
            if let Ok(mut state) = fas_shared.lock() {
                if state.mode != scheduler.current_mode()
                    || state.package != scheduler.current_package()
                    || state.allowed != allowed
                    || state.ready != ready
                    || state.min_floor != floors
                {
                    state.mode = scheduler.current_mode().to_string();
                    state.package = scheduler.current_package().to_string();
                    state.allowed = allowed;
                    state.ready = ready;
                    state.min_floor = floors;
                }
            }
        }
        let engine_status = fas_status_slot
            .lock()
            .map(|status| status.clone())
            .unwrap_or_default();
        runtime.set_fas_engine(&engine_status);
        runtime.set_frequencies(scheduler.live_frequencies());

        if engine_status.state != "active"
            && (force_apply
                || Instant::now() >= fas_probe_at
                || (fas_status.state == "ready" && fas_status.mode != scheduler.current_mode()))
        {
            fas_status = scheduler.refresh_fas(!scene.controls_locked() && last_apply_error.is_empty());
            fas_probe_at = Instant::now() + Duration::from_secs(10);
            revision.fetch_add(1, Ordering::Relaxed);
        }
        if engine_status.state == "active" && fas_status.sync != "superseded" {
            // 本引擎接管帧感知调度期间，fas-rs 已被暂停，档位同步随之失效。
            fas_status.sync = "superseded";
            revision.fetch_add(1, Ordering::Relaxed);
        }
        let policy_error = if scene.controls_locked() {
            "其它 Scene 调度器已接管，NovaSched 暂停下发".to_string()
        } else if !last_apply_error.is_empty() {
            last_apply_error.clone()
        } else if scheduler.pending_control_count() > 0 {
            format!("{} 个 CPU 频率上限未获内核确认；{}", scheduler.pending_control_count(),
                if scheduler.pending_retry_exhausted() { "已停止自动重写，请查看日志" } else { "正在有限重试" })
        } else { String::new() };
        let phase = if scene.controls_locked() { "suspended" }
            else if policy_error.is_empty() { "ready" } else { "degraded" };
        match boot_guard.confirm_if_stable(policy_error.is_empty()) {
            Ok(true) => logger.info("启动保护：调度与心跳已连续稳定三分钟"),
            Ok(false) => {},
            Err(error) => logger.error(format!("保存启动稳定记录失败: {error}")),
        }
        runtime.publish_policy(crate::websocket::PolicyStatus {
            package: if scene.controls_locked() { &package } else { scheduler.current_package() },
            mode: if scene.controls_locked() { "" } else { scheduler.current_mode() },
            controller: scene.controller(),
            foreign: scene.controls_locked(),
            linked: scene.linked(),
            automatic: automatic_enabled && !scene.controls_locked() && last_apply_error.is_empty(),
            phase,
            error: &policy_error,
            pending: scheduler.pending_control_count(),
            fas: &fas_status,
        });
        revision.fetch_add(1, Ordering::Relaxed);
        runtime.set_power_profile(if scene.controls_locked() {
            ""
        } else {
            scheduler.current_power_profile()
        });
        let extreme_status = if !current_extreme_power {
            "off"
        } else if !last_apply_error.is_empty() {
            "failed"
        } else if scene.controls_locked() {
            "unsupported"
        } else {
            scheduler.extreme_power_status()
        };
        let extreme_error = last_apply_error.as_str();
        if reported_extreme_power != current_extreme_power
            || reported_extreme_status != extreme_status
            || reported_extreme_error != extreme_error
        {
            runtime.set_extreme_power(current_extreme_power, extreme_status, extreme_error);
            reported_extreme_power = current_extreme_power;
            reported_extreme_status = extreme_status;
            reported_extreme_error = extreme_error.to_string();
        }
        let status = format!(
            "pid={}\nmode={}\npackage={}\ncontroller={}\nscene_linked={}\nscene_locked={}\nscene_mode={}\nsnapshot_items={}\npower_profile={}\nfasAvailable={}\n",
            util::current_pid(), scheduler.current_mode(), scheduler.current_package(), scene.controller(), scene.linked(), scene.controls_locked(),
            scene.mode.as_deref().unwrap_or(""), snapshot.count(),
            if scene.controls_locked() { "" } else { scheduler.current_power_profile() },
            fas_status.available()
        );
        let status = format!(
            "{status}automatic_enabled={}\napp_kind={}\nclassification_source={}\nphase={}\nerror={}\n",
            automatic.enabled(), classification.kind.name(), classification.source,
            phase,
            policy_error.replace('\n', " ")
        );
        let status = format!("{status}{}pending_controls={}\n", fas_status.report(), scheduler.pending_control_count());
        if status != last_status || status_written.elapsed() >= Duration::from_secs(10) {
            if let Err(error) =
                util::atomic_write(&crate::storage::path(&state_dir, "status"), status.as_bytes(), 0o600)
            {
                logger.error(format!("状态文件写入失败: {error}"));
            } else {
                last_status = status;
                status_written = Instant::now();
            }
        }
    }

    logger.info("收到停止请求，正在恢复原厂快照");
    web_stop.store(true, Ordering::Relaxed);
    if let Some(handle) = input_thread {
        if handle.join().is_err() {
            logger.error("输入加速线程异常退出");
        }
    }
    if let Some(handle) = fas_thread {
        if handle.join().is_err() {
            logger.error("NovaFAS 引擎线程异常退出");
        }
    }
    if let Some(handle) = web_thread {
        if handle.join().is_err() {
            logger.error("WebUI 接入线程异常退出");
        }
    }
    let restore = if scene.controls_locked() {
        logger.warn("外部调度器控制中，退出时不覆盖其节点；原快照保留");
        Ok(0)
    } else {
        snapshot.restore()
    };
    if let Err(e) = &restore {
        if let Err(error) = lifecycle::record_failure(state_dir, "停止回滚", e, false) {
            logger.error(error);
        }
    }
    if restore.is_ok() {
        if let Err(error) = boot_guard.finish_cleanly() {
            logger.error(format!("保存正常停止记录失败: {error}"));
        }
        if let Err(error) =
            util::atomic_write(&crate::storage::path(&state_dir, "startup"), b"state=stopped\n", 0o600)
        {
            logger.error(error);
        }
    }
    drop(cleanup);
    restore.map(|_| ())
}

/// Per-cluster audit for the diagnose command: what the config requests vs
/// what the kernel actually runs right now. This is the definitive answer to
/// "功耗为什么高" - one table showing whether the caps are applied, being
/// overridden by a QoS/thermal floor, or applied while the cluster simply
/// carries real load.
pub fn print_policy_audit(config: &Config) -> String {
    let active_mode = app_modes::read_mode_file()
        .unwrap_or_else(|| config.modes.keys().next().cloned().unwrap_or_default());
    let Some(profile) = config.modes.get(active_mode.as_str()) else {
        return format!("当前档位 {active_mode} 不在配置中，无法审计");
    };
    let mut out = String::from(
        "cluster           配置min/max     实际min/max      当前频率(两次采样)   判定\n",
    );
    for (cluster, policy) in config.policy.iter().enumerate() {
        if *policy < 0 {
            continue;
        }
        let base: PathBuf = format!("/sys/devices/system/cpu/cpufreq/policy{policy}").into();
        let read = |name: &str| -> String {
            util::read_trimmed(base.join(name)).unwrap_or_else(|_| "?".into())
        };
        let configured_min = profile.clusters[cluster].min_freq.clone();
        let configured_max = profile.clusters[cluster].max_freq.clone();
        let actual_min = read("scaling_min_freq");
        let actual_max = read("scaling_max_freq");
        let cur_a = read("scaling_cur_freq");
        std::thread::sleep(Duration::from_millis(400));
        let cur_b = read("scaling_cur_freq");
        let ceiling_matches = actual_max
            .parse::<u64>()
            .ok()
            .zip(
                // 配置里普遍是 "58%" 这类百分比，必须先按策略物理上限换算成 kHz，
                // 否则对比永远失败，审计会把正常生效误报为“上限被占用”。
                scheduler::resolve_param_frequency(&configured_max, &base)
                    .parse::<u64>()
                    .ok(),
            )
            .is_some_and(|(actual, requested)| actual > 0 && actual <= requested);
        let verdict = if ceiling_matches {
            "已生效"
        } else {
            "上限被系统服务占用(温控/QoS/boost)"
        };
        out.push_str(&format!(
            "policy{policy}(c{cluster})  {configured_min}/{configured_max}  {actual_min}/{actual_max}  {cur_a}..{cur_b}  {verdict}\n"
        ));
    }
    out.push_str("当前档位: ");
    out.push_str(&active_mode);
    out.push_str("；当前频率长期贴近配置上限=负载真实存在；上限显示被占用=厂商服务在钳制该簇；已生效但仍高=瓶颈在 GPU/屏幕/网络而非 CPU\n");
    out
}

pub fn send_signal(module: &Path, signal: i32) -> Result<()> {
    process_identity::verify(Path::new(STATE_DIR), module)?.signal(signal)
}
pub fn health(module: &Path) -> Result<String> {
    let process = process_identity::verify(Path::new(STATE_DIR), module)?;
    let path = crate::storage::path(Path::new(STATE_DIR), "status");
    let modified = fs::metadata(&path)
        .and_then(|m| m.modified())
        .map_err(|e| e.to_string())?;
    if SystemTime::now()
        .duration_since(modified)
        .unwrap_or(Duration::MAX)
        > Duration::from_secs(30)
    {
        return Err("进程存在但调度心跳过期".into());
    }
    let status = util::read_text(&path)?;
    if !status
        .lines()
        .any(|line| line == format!("pid={}", process.identity.pid))
    {
        return Err("状态文件不属于当前守护".into());
    }
    Ok(status)
}
pub fn print_status(module: &Path) -> Result<()> {
    match health(module) {
        Ok(status) => {
            print!("running=true\nverified=true\n{status}");
            Ok(())
        }
        Err(error) => {
            let reason = lifecycle::connection_failure(Path::new(STATE_DIR), module, &error);
            println!("running=false\nverified=false\nreason={reason}");
            Err(reason)
        }
    }
}
pub fn restore_stock(module: &Path) -> Result<()> {
    lifecycle::stop(module)?;
    let state = Path::new(STATE_DIR);
    let _lock = util::acquire_flock(&crate::storage::path(&state, "daemon.lock"))?;
    Snapshot::load(state, Logger::new(state))?
        .restore()
        .map(|_| ())
}

pub fn restore_after_supervisor_crash(_module: &Path) -> Result<()> {
    let state = Path::new(STATE_DIR);
    let _lock = util::acquire_flock(&crate::storage::path(&state, "daemon.lock"))?;
    let logger = Logger::new(state);
    let nodes = Snapshot::load(state, logger).and_then(|snapshot| snapshot.restore().map(|_| ()));
    let scene_restore = scene::restore_provider().map(|outcome| {
        eprintln!("novasched: {outcome}");
    });
    match (nodes, scene_restore) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(nodes), Ok(())) => Err(format!("原厂节点恢复失败: {nodes}")),
        (Ok(()), Err(scene)) => Err(format!("Scene 回调恢复失败: {scene}")),
        (Err(nodes), Err(scene)) => Err(format!(
            "原厂节点恢复失败: {nodes}；Scene 回调恢复失败: {scene}"
        )),
    }
}

pub fn probe(module_dir: &Path) -> Result<()> {
    let hardware = scheduler::preflight()?;
    let mut config = Config::load(&module_dir.join("config").join(hardware.soc.file()))?;
    let changes = hardware.adapt_config(&mut config)?;
    println!("compatibility={}", if hardware.samsung { "samsung" } else { "standard" });
    println!("soc={}\nsoc_id={}\nprofile={}\nprofile_id=novasched.{}\nconfig_name={}\nconfig_author={}\nconfig_version={}\ndevice={}\nevidence={}\nroot=true\nmanager_gate=none\npolicies={},{},{},{}\ntop_app=true\npolicy_writes=0",
        hardware.soc.name(), hardware.soc.id(), hardware.soc.file(), hardware.soc.id().to_ascii_lowercase(),config.meta.name,config.meta.author,config.meta.version,hardware.device, hardware.evidence, config.policy[0],config.policy[1],config.policy[2],config.policy[3]);
    for change in changes {
        println!("adaptation={change}");
    }
    Ok(())
}

pub fn check_config(_module_dir: &Path) -> Result<()> {
    let mut config = Config::load(Path::new(CONFIG_PATH))?;
    scheduler::preflight()?.adapt_config(&mut config)?;
    println!(
        "config=valid\nname={}\nversion={}\nauthor={}\npolicies={},{},{},{}",
        config.meta.name,
        config.meta.version,
        config.meta.author,
        config.policy[0],
        config.policy[1],
        config.policy[2],
        config.policy[3]
    );
    Ok(())
}

fn startup_fail<T>(logger: &Logger, state_dir: &Path, stage: &str, error: String) -> Result<T> {
    logger.error(format!("{stage}失败: {error}"));
    if let Err(e) = lifecycle::record_failure(state_dir, stage, &error, false) {
        logger.error(format!("保存失败诊断也失败: {e}"));
    }
    Err(error)
}

struct PidCleanup(PathBuf);
impl Drop for PidCleanup {
    fn drop(&mut self) {
        for path in [
            &self.0,
            &self.0.with_file_name("daemon.identity"),
            &self.0.with_file_name("status"),
        ] {
            if let Err(e) = util::remove_if_exists(path) {
                eprintln!("novasched: {e}");
            }
        }
    }
}

#[derive(Default)]
struct ForegroundMonitor {
    pids: BTreeSet<i32>,
    current: String,
    activity_at: Option<Instant>,
    activity: Option<String>,
    touch_suppress_until: Option<Instant>,
}

impl ForegroundMonitor {
    fn detect(
        &mut self,
        rules: &AppModes,
        ignored_packages: &BTreeSet<String>,
        refresh: bool,
        automatic: bool,
        touch: bool,
    ) -> Option<String> {
        let new_pids = read_top_app_pids();
        // Touch edges may force an early activity query so a notification
        // shade pull is detected right away; a query that does not change the
        // foreground backs off for two seconds so games never see a dumpsys
        // storm. The regular cadence stays two seconds.
        let now = Instant::now();
        let regular_due = self
            .activity_at
            .map(|at| at.elapsed() >= Duration::from_secs(2))
            .unwrap_or(true);
        let touch_due = touch
            && self
                .touch_suppress_until
                .map(|until| now >= until)
                .unwrap_or(true)
            && self
                .activity_at
                .map(|at| at.elapsed() >= Duration::from_millis(400))
                .unwrap_or(true);
        let activity_due = refresh || regular_due || touch_due;
        if new_pids == self.pids && !activity_due {
            return None;
        }
        let added: BTreeSet<i32> = new_pids.difference(&self.pids).copied().collect();
        self.pids = new_pids;
        let processes: Vec<_> = self.pids.iter()
            .filter_map(|pid| read_process(*pid).map(|process| (*pid, process))).collect();
        if activity_due {
            self.activity_at = Some(now);
            let previous = self.activity.clone();
            self.activity = query_foreground();
            if touch && !refresh && previous == self.activity {
                self.touch_suppress_until = Some(now + Duration::from_secs(2));
            } else {
                self.touch_suppress_until = None;
            }
        }
        // ActivityManager/WindowManager wins even when cpuset is missing or
        // retains a launcher/PiP process. Ambiguous automatic scans stay idle.
        let selected = if let Some(activity) = &self.activity {
            Some(activity.clone())
        } else if automatic {
            unambiguous_top_app(&processes)
        } else {
            select_foreground(&self.current, &processes, &added, ignored_packages,
                |package| rules.has_rule(package))
        };
        let Some(found) = selected else {
            if !self.current.is_empty() {
                self.current.clear();
                return Some(String::new());
            }
            return None;
        };
        if found == self.current {
            return None;
        }
        self.current = found.clone();
        Some(found)
    }
}

fn local_automatic_enabled(enabled: bool, scene: &scene::SceneState) -> bool {
    enabled && !scene.linked() && !scene.controls_locked()
}

/// 桌面/系统界面：这些表面待机时不需要 NovaFAS 保持任何加压与底频，
/// 待机功耗全部交给内核 idle。
fn is_idle_surface(package: &str) -> bool {
    matches!(
        package,
        "com.android.systemui"
            | "com.miui.home"
            | "com.android.launcher3"
            | "com.android.launcher"
            | "com.google.android.apps.nexuslauncher"
            | "com.oppo.launcher"
            | "com.bbk.launcher2"
            | "com.sec.android.app.launcher"
            | "com.huawei.android.launcher"
    )
}

fn query_foreground() -> Option<String> {
    let user = util::current_android_user().ok();
    for args in [&["activity", "activities"][..], &["window", "windows"][..], &["activity", "top"][..]] {
        if let Ok(text) = util::query_command_limited(
            "/system/bin/dumpsys", args, Duration::from_millis(900), 1_048_576,
        ) {
            if let Some(package) = activity_package(&text, user) {
                return Some(package);
            }
        }
    }
    None
}

#[cfg(test)]
fn resumed_package(text: &str, user: u32) -> Option<String> {
    activity_package(text, Some(user))
}

fn activity_package(text: &str, user: Option<u32>) -> Option<String> {
    if text.split_whitespace().any(|field| matches!(field.trim_end_matches(','),
        "mSleeping=true" | "mKeyguardShowing=true" | "isKeyguardShowing=true")) {
        return Some(String::new());
    }
    let mut resumed = BTreeSet::new();
    let mut focused_app = None;
    for line in text.lines() {
        let top = line.contains("topResumedActivity") || line.contains("mTopResumedActivity");
        let focus = line.contains("mCurrentFocus=");
        let marker = top || focus || line.contains("ResumedActivity")
            || line.contains("resumedActivity") || line.contains("mFocusedApp=")
            || line.trim_start().starts_with("ACTIVITY ");
        if !marker { continue; }
        let parts: Vec<_> = line.split_whitespace().collect();
        let record_user = parts.iter().find_map(|field| field.strip_prefix('u')?.parse::<u32>().ok());
        if user.zip(record_user).is_some_and(|(active, record)| active != record) { continue; }
        let package = parts.iter().find_map(|field| {
            let (package, _) = field.split_once('/')?;
            util::valid_package(package).then(|| package.to_string())
        });
        if let Some(package) = package {
            if top || focus { return Some(package); }
            if line.contains("mFocusedApp=") { focused_app = Some(package); }
            else { resumed.insert(package); }
        }
    }
    focused_app.or_else(|| if resumed.len() == 1 { resumed.into_iter().next() } else { None })
}

fn read_top_app_pids() -> BTreeSet<i32> {
    ["/dev/cpuset/top-app/cgroup.procs", "/dev/cpuset/top-app/tasks",
        "/sys/fs/cgroup/top-app/cgroup.procs", "/sys/fs/cgroup/top-app/tasks",
        "/sys/fs/cgroup/cpuset/top-app/cgroup.procs", "/dev/stune/top-app/cgroup.procs"]
        .iter().filter_map(|path| util::read_text(Path::new(path)).ok())
        .flat_map(|text| text.split_whitespace().filter_map(|value| value.parse::<i32>().ok())
            .filter(|pid| *pid > 0).collect::<Vec<_>>()).collect()
}

fn unambiguous_top_app(processes: &[(i32, String)]) -> Option<String> {
    let packages: BTreeSet<_> = processes.iter().filter_map(|(_, name)| {
        if name.contains(':') || !util::valid_package(name) { None } else { Some(name.clone()) }
    }).collect();
    if packages.len() == 1 { packages.into_iter().next() } else { None }
}

fn read_process(pid: i32) -> Option<String> {
    let data = fs::read(format!("/proc/{pid}/cmdline")).ok()?;
    let end = data
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(data.len());
    String::from_utf8(data[..end].to_vec()).ok()
}

fn select_foreground(
    current: &str,
    processes: &[(i32, String)],
    added: &BTreeSet<i32>,
    ignored: &BTreeSet<String>,
    has_rule: impl Fn(&str) -> bool,
) -> Option<String> {
    let candidates: Vec<_> = processes
        .iter()
        .filter(|(_, process)| looks_like_package(process) && !ignored_shell(process, ignored))
        .collect();
    // A newly foregrounded main process outranks a retained PiP/previous app,
    // even if that old app has an explicit rule. New sandbox workers alone
    // must not displace a still-present main application.
    candidates
        .iter()
        .copied()
        .filter(|(pid, process)| added.contains(pid) && !process.contains(':'))
        .max_by_key(|(pid, process)| (has_rule(process), *pid))
        .map(|(_, process)| process.clone())
        .or_else(|| {
            candidates
                .iter()
                .copied()
                .filter(|(_, process)| {
                    !current.is_empty()
                        && util::base_package(process) == util::base_package(current)
                })
                .max_by_key(|(pid, process)| (!process.contains(':'), *pid))
                .map(|(_, process)| process.clone())
        })
        .or_else(|| {
            candidates
                .iter()
                .copied()
                .max_by_key(|(pid, process)| (!process.contains(':'), has_rule(process), *pid))
                .map(|(_, process)| process.clone())
        })
}

fn looks_like_package(process: &str) -> bool {
    util::valid_package(util::base_package(process))
}

fn ignored_shell(process: &str, ignored_packages: &BTreeSet<String>) -> bool {
    let package = util::base_package(process);
    ignored_packages.contains(package)
}

fn watch_paths() -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = [
        CONFIG_PATH,
        APP_MODES_PATH,
        util::AUTO_POLICY_PATH,
        MODE_PATH,
        SCENE_REQUEST_PATH,
        "/dev/cpuset/top-app/cgroup.procs",
        "/dev/cpuset/top-app/tasks",
        "/sys/fs/cgroup/top-app/cgroup.procs",
        "/sys/fs/cgroup/top-app/tasks",
    ]
    .into_iter()
    .map(PathBuf::from)
    .collect();
    paths.extend(scene::watched_paths().into_iter().map(PathBuf::from));
    paths
}

// Absolute deadline: unrelated inotify traffic must not postpone the audit.
struct NodeAuditClock {
    next: Instant,
}
impl NodeAuditClock {
    fn new(now: Instant) -> Self {
        Self {
            next: now + Duration::from_secs(60),
        }
    }
    fn take_due(&mut self, now: Instant) -> bool {
        if now < self.next {
            return false;
        }
        self.next = now + Duration::from_secs(60);
        true
    }
}

#[cfg(test)]
mod audit_clock_tests {
    use super::*;
    #[test]
    fn a_new_main_app_beats_a_retained_app_rule_and_sandbox_workers() {
        let processes = vec![
            (100, "org.video.old".into()),
            (90, "org.game.new".into()),
            (300, "com.google.android.webview:sandboxed_process".into()),
        ];
        let added = [90, 300].into_iter().collect();
        let ignored = BTreeSet::new();
        assert_eq!(
            select_foreground(
                "org.video.old",
                &processes,
                &added,
                &ignored,
                |package| package == "org.video.old"
            ),
            Some("org.game.new".into())
        );
        assert_eq!(
            select_foreground(
                "org.video.old",
                &processes,
                &[300].into_iter().collect(),
                &ignored,
                |_| false
            ),
            Some("org.video.old".into())
        );
    }
    #[test]
    fn no_candidate_does_not_keep_a_dead_foreground_app() {
        assert_eq!(
            select_foreground(
                "org.old.app",
                &[(300, "com.android.systemui".into())],
                &[300].into_iter().collect(),
                &["com.android.systemui".into()].into_iter().collect(),
                |_| false
            ),
            None
        );
    }
    #[test]
    fn audit_runs_on_sixty_second_deadline_despite_frequent_events() {
        let now = Instant::now();
        let mut clock = NodeAuditClock::new(now);
        for second in 0..60 {
            assert!(!clock.take_due(now + Duration::from_secs(second)));
        }
        assert!(clock.take_due(now + Duration::from_secs(60)));
        assert!(!clock.take_due(now + Duration::from_secs(60)));
        assert!(!clock.take_due(now + Duration::from_secs(119)));
        assert!(clock.take_due(now + Duration::from_secs(120)));
    }
}

#[derive(Default)]
struct FileStamps(std::collections::BTreeMap<PathBuf, Option<(i64, i64, u64)>>);
impl FileStamps {
    fn new() -> Self {
        Self::default()
    }
    fn changed(&mut self, path: &Path) -> bool {
        let stamp = util::file_stamp(path);
        match self.0.insert(path.to_path_buf(), stamp) {
            Some(old) => old != stamp,
            None => false,
        }
    }
}

#[cfg(test)]
mod automatic_foreground_tests {
    use super::*;
    use crate::automatic::{AppKind, Classifier};
    fn activity(package: &str, user: u32) -> String {
        format!("topResumedActivity=ActivityRecord{{abc u{user} {package}/.Main t3}}\n")
    }
    #[test]
    fn exiting_games_for_home_daily_apps_and_keyguard_always_selects_powersave() {
        let mut classifier = Classifier::default();
        let frames = [
            activity("com.tencent.tmgp.sgame", 0),
            activity("com.android.launcher3", 0),
            activity("com.tencent.ig", 0),
            activity("com.example.chat", 0),
            "mSleeping=true\n".into(),
            activity("com.tencent.tmgp.sgame", 0) + "mKeyguardShowing=true\n",
        ];
        let modes: Vec<_> = frames
            .iter()
            .map(|frame| {
                let package = resumed_package(frame, 0).unwrap_or_default();
                // Exact game catalogue does not require a package dump. Other
                // packages can fail classification without retaining fast.
                let kind = classifier
                    .classify_with_for_test(&package, || Err("metadata unavailable".into()));
                kind.mode()
            })
            .collect();
        assert_eq!(
            modes,
            [
                "fast",
                "powersave",
                "fast",
                "powersave",
                "powersave",
                "powersave"
            ]
        );
    }
    #[test]
    fn focused_task_outranks_retained_game_and_inactive_user() {
        let text = "mResumedActivity: ActivityRecord{aa u0 com.tencent.ig/.Main t1}\n".to_string()
            + &activity("com.android.launcher3", 0)
            + &activity("com.tencent.ig", 10);
        assert_eq!(
            resumed_package(&text, 0),
            Some("com.android.launcher3".into())
        );
        assert_eq!(resumed_package(&text, 10), Some("com.tencent.ig".into()));
        assert_eq!(resumed_package(&text, 11), None);
    }
    #[test]
    fn missing_or_ambiguous_activity_output_does_not_invent_a_game() {
        let text = "mResumedActivity: ActivityRecord{aa u0 com.tencent.ig/.Main t1}\nmResumedActivity: ActivityRecord{bb u0 com.example.chat/.Main t2}";
        assert_eq!(resumed_package(text, 0), None);
        assert_eq!(resumed_package("Error: service unavailable", 0), None);
        assert_eq!(AppKind::Unknown.mode(), "powersave");
    }
}

#[cfg(test)]
mod scene_and_foreground_regressions {
    use super::*;
    #[test]
    fn scene_callback_keeps_ownership_with_saved_automatic_preference() {
        let mut state = scene::SceneState::default();
        assert!(local_automatic_enabled(true, &state));
        state.provider = scene::ProviderKind::Own;
        assert!(!local_automatic_enabled(true, &state));
        state.provider = scene::ProviderKind::Foreign;
        assert!(!local_automatic_enabled(true, &state));
        state.provider = scene::ProviderKind::None;
        assert!(local_automatic_enabled(true, &state));
    }
    #[test]
    fn window_focus_and_activity_top_cover_rom_output_variants() {
        assert_eq!(activity_package("mCurrentFocus=Window{cc u0 com.omarea.vtools/com.omarea.vtools.MainActivity}", Some(0)), Some("com.omarea.vtools".into()));
        assert_eq!(activity_package("mFocusedApp=ActivityRecord{cc u10 com.example.work/.Main t2}", Some(10)), Some("com.example.work".into()));
        assert_eq!(activity_package("ACTIVITY com.example.chat/.Main 123 pid=12", Some(0)), Some("com.example.chat".into()));
        assert_eq!(activity_package("topResumedActivity=ActivityRecord{cc u0 com.example.chat/.Main t1}", None), Some("com.example.chat".into()));
        assert_eq!(activity_package("mCurrentFocus=Window{cc u10 com.example.work/.Main}", Some(0)), None);
    }
    #[test]
    fn ambiguous_cpuset_does_not_promote_background_games() {
        assert_eq!(unambiguous_top_app(&[(1,"com.example.game".into()),(2,"com.android.launcher3".into())]), None);
        assert_eq!(unambiguous_top_app(&[(1,"com.example.game".into()),(2,"com.example.game:render".into())]), Some("com.example.game".into()));
    }
}
