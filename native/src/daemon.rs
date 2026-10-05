use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use crate::app_modes::{self, AppModes};
use crate::config::Config;
use crate::event_loop::{EventLoop, WakeReason};
use crate::ffi;
use crate::logging::Logger;
use crate::options::Options;
use crate::scene;
use crate::scheduler::{self, Scheduler};
use crate::snapshot::Snapshot;
use crate::util::{
    self, Result, APP_MODES_PATH, CONFIG_PATH, MODE_PATH, OPTIONS_PATH, SCENE_REQUEST_PATH,
    STATE_DIR,
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

pub fn run_daemon(module_dir: PathBuf) -> Result<()> {
    STOP_REQUESTED.store(false, Ordering::Relaxed);
    RELOAD_REQUESTED.store(false, Ordering::Relaxed);
    ffi::install_signal(ffi::SIGTERM, handle_signal);
    ffi::install_signal(ffi::SIGINT, handle_signal);
    ffi::install_signal(ffi::SIGHUP, handle_signal);
    ffi::ignore_signal(ffi::SIGPIPE);

    let state_dir = Path::new(STATE_DIR);
    util::create_dir(state_dir)?;
    let logger = Logger::new(state_dir);
    let _lock = util::acquire_flock(&state_dir.join("daemon.lock"))?;
    util::remove_if_exists(&state_dir.join("status"))?;
    logger.clear();
    let pid_path = state_dir.join("daemon.pid");
    util::atomic_write(
        &pid_path,
        format!("{}\n", util::current_pid()).as_bytes(),
        0o600,
    )?;
    let cleanup = PidCleanup(pid_path.clone());
    let identity = match process_identity::Identity::capture(util::current_pid()) {
        Ok(value) => value,
        Err(error) => return startup_fail(&logger, state_dir, "采集进程身份", error),
    };
    if let Err(error) = util::atomic_write(
        &state_dir.join("daemon.identity"),
        identity.encode().as_bytes(),
        0o600,
    ) {
        return startup_fail(&logger, state_dir, "保存进程身份", error);
    }

    logger.info(format!(
        "NovaSched Zen Edition {} 守护进程启动",
        env!("CARGO_PKG_VERSION")
    ));
    util::atomic_write(&state_dir.join("startup"), b"state=starting\n", 0o600)?;
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
    let config = match Config::load(Path::new(CONFIG_PATH)) {
        Ok(value) => value,
        Err(error) => return startup_fail(&logger, state_dir, "读取配置", error),
    };
    let options = match Options::initialize(
        config.functions.extreme_powersave.enabled_by_default,
        config
            .functions
            .smooth_powersave
            .as_ref()
            .is_some_and(|v| v.limits.enabled_by_default),
    ) {
        Ok(value) => value,
        Err(error) => return startup_fail(&logger, state_dir, "初始化选项", error),
    };
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
    } else if app_modes::scene_pedestal_active(scene.linked()) {
        "fast".to_string()
    } else {
        app_modes.default_mode()
    };
    let selected_soc = hardware.soc;
    let mut scheduler = match Scheduler::new(config, snapshot.clone(), logger.clone(), hardware) {
        Ok(value) => value,
        Err(error) => return startup_fail(&logger, state_dir, "内核能力适配", error),
    };
    scheduler.set_extreme_powersave(options.extreme_powersave());
    scheduler.set_smooth_powersave(options.smooth_powersave());
    // A failed first dispatch must not kill the daemon. Vendor frequency
    // managers race the first ceiling write at boot, and exiting here turned
    // one rejected write into a dead module once the supervisor gave up.
    // Degrade instead: the tick loop retries every 5 s and the WebUI stays
    // connected with the honest error, so the user can self-recover.
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
    runtime.set_profile(
        selected_soc,
        scheduler.supports_extreme(),
        scheduler.supports_smooth(),
    );
    runtime.set_power_profile(scheduler.current_power_profile());
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
        options: options.clone(),
        runtime: runtime.clone(),
        revision: revision.clone(),
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
    if let Err(e) = util::remove_if_exists(&state_dir.join("last_error")) {
        logger.error(e);
    }
    if let Err(e) = util::atomic_write(&state_dir.join("startup"), b"state=running\n", 0o600) {
        logger.error(e);
    }

    let mut monitor = ForegroundMonitor::default();
    let mut stamps = FileStamps::new();
    let mut watcher = EventLoop::new(&watch_paths());
    logger.info(watcher.describe());
    let mut last_status = String::new();
    let mut status_written = Instant::now();
    let mut retry_at = Instant::now();
    let mut suspended = scene.controls_locked();
    let mut first_scan = true;
    let mut node_audit = NodeAuditClock::new(Instant::now());
    let mut scene_probe_at = Instant::now() + scene::PRESENCE_INTERVAL;
    while !STOP_REQUESTED.load(Ordering::Relaxed) && !state_dir.join("stop.requested").exists() {
        let wake = if first_scan {
            first_scan = false;
            WakeReason::Event
        } else {
            watcher.wait(crate::event_loop::FOREGROUND_FALLBACK)
        };
        let forced = RELOAD_REQUESTED.swap(false, Ordering::Relaxed);
        if !wake.needs_rescan() && !forced {
            continue;
        }
        let config_changed = stamps.changed(Path::new(CONFIG_PATH));
        let rules_changed = stamps.changed(Path::new(APP_MODES_PATH));
        let mode_changed = stamps.changed(Path::new(MODE_PATH));
        let scene_request_changed = stamps.changed(Path::new(SCENE_REQUEST_PATH));
        let options_changed = stamps.changed(Path::new(OPTIONS_PATH));
        let scene_files_changed = scene::watched_paths().iter().fold(false, |changed, path| {
            stamps.changed(Path::new(path)) || changed
        });
        let presence_due = Instant::now() >= scene_probe_at;
        if presence_due || forced || scene_files_changed {
            scene::invalidate_presence();
            scene_probe_at = Instant::now() + scene::PRESENCE_INTERVAL;
        }
        let mut scene_changed = scene_files_changed;

        if forced || config_changed {
            match Config::load(Path::new(CONFIG_PATH)) {
                Ok(config) => match scheduler.replace_config(config) {
                    Ok(()) => {
                        runtime.set_profile(
                            selected_soc,
                            scheduler.supports_extreme(),
                            scheduler.supports_smooth(),
                        );
                        logger.info("config.json 已重新载入");
                    }
                    Err(error) => {
                        logger.error(format!("config.json 内核适配失败，保留当前配置: {error}"))
                    }
                },
                Err(error) => logger.error(format!("config.json 无效，保留当前配置: {error}")),
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
            if previous_scene.linked() && !scene.linked() {
                // Pedestal is transient. It must not survive an uninstall,
                // or a replacement provider in this boot.
                let pedestal = app_modes::scene_pedestal_active(true);
                if let Err(error) = util::remove_if_exists(Path::new(SCENE_REQUEST_PATH)) {
                    logger.warn(format!("清理失效 Scene 请求失败: {error}"));
                }
                if pedestal {
                    if let Err(error) = app_modes.write_current_mode(&app_modes.default_mode()) {
                        logger.error(format!("退出 Scene 底座档位失败: {error}"));
                    }
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

        if options_changed {
            match options.reload() {
                Ok(true) => logger.info("options.txt 已重新载入"),
                Ok(false) => {}
                Err(error) => logger.warn(format!("options.txt 无效，保留当前选项: {error}")),
            }
        }
        let option_changed = scheduler.set_extreme_powersave(options.extreme_powersave());
        let smooth_changed = scheduler.set_smooth_powersave(options.smooth_powersave());
        // Re-write ceilings that a vendor service refused until they stick.
        // Cheap when the queue is empty; skipped while a foreign scheduler
        // owns the nodes, and its queue is dropped to avoid write wars.
        if !scene.controls_locked() && scheduler.enforce_pending() {
            revision.fetch_add(1, Ordering::Relaxed);
        }

        let foreground = monitor.detect(
            &app_modes,
            scheduler.ignored_packages(),
            forced || config_changed || rules_changed || scene_changed,
        );
        let package = foreground
            .as_deref()
            .unwrap_or(&monitor.current)
            .to_string();
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
        } else {
            let explicit = app_modes.has_rule(&package);
            let configured = app_modes.resolve(&package);
            let leaving_explicit =
                !explicit && package != scheduler.current_package() && scheduler.last_explicit();
            let pedestal = app_modes::scene_pedestal_active(scene.linked());
            let selected = app_modes::select_local_mode(
                pedestal,
                explicit,
                configured,
                leaving_explicit,
                app_modes.default_mode(),
                app_modes::read_mode_file(),
            );
            (selected, explicit && !pedestal)
        };
        let force_apply = forced
            || config_changed
            || rules_changed
            || mode_changed
            || scene_request_changed
            || scene_changed
            || option_changed
            || smooth_changed;
        if scene.controls_locked() {
            suspended = true;
            scheduler.clear_pending();
            runtime.set(&package, "", scene.controller(), true, false);
            runtime.tick("suspended", "其它 Scene 调度器已接管，NovaSched 暂停下发");
        } else if Instant::now() >= retry_at || force_apply {
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
                runtime.set(
                    scheduler.current_package(),
                    scheduler.current_mode(),
                    scene.controller(),
                    scene.controls_locked(),
                    scene.linked(),
                );
                retry_at = Instant::now() + Duration::from_secs(5);
                runtime.tick("degraded", &last_apply_error);
            } else {
                suspended = false;
                last_apply_error.clear();
                if let Err(error) = app_modes.write_current_mode(scheduler.current_mode()) {
                    logger.error(format!("写入 mode.txt 失败: {error}"));
                    last_apply_error = error;
                    retry_at = Instant::now() + Duration::from_secs(5);
                }
                runtime.set(
                    scheduler.current_package(),
                    scheduler.current_mode(),
                    scene.controller(),
                    scene.controls_locked(),
                    scene.linked(),
                );
                runtime.tick(
                    if last_apply_error.is_empty() {
                        "ready"
                    } else {
                        "degraded"
                    },
                    &last_apply_error,
                );
            }
        } else {
            runtime.set(
                scheduler.current_package(),
                scheduler.current_mode(),
                scene.controller(),
                scene.controls_locked(),
                scene.linked(),
            );
            // Backoff window after an apply error: keep the honest phase
            // instead of forcing degraded onto a healthy scheduler.
            let phase = if last_apply_error.is_empty() {
                "ready"
            } else {
                "degraded"
            };
            runtime.tick(phase, &last_apply_error);
        }
        if foreground.is_some() || force_apply {
            revision.fetch_add(1, Ordering::Relaxed);
        }

        runtime.set_power_profile(if scene.controls_locked() {
            ""
        } else {
            scheduler.current_power_profile()
        });
        let status = format!(
            "pid={}\nmode={}\npackage={}\ncontroller={}\nscene_linked={}\nscene_locked={}\nscene_mode={}\nsnapshot_items={}\npower_profile={}\n",
            util::current_pid(), scheduler.current_mode(), scheduler.current_package(), scene.controller(), scene.linked(), scene.controls_locked(),
            scene.mode.as_deref().unwrap_or(""), snapshot.count(),
            if scene.controls_locked() { "" } else { scheduler.current_power_profile() }
        );
        let status = format!(
            "{status}phase={}\nerror={}\n",
            if scene.controls_locked() {
                "suspended"
            } else if last_apply_error.is_empty() {
                "ready"
            } else {
                "degraded"
            },
            last_apply_error.replace('\n', " ")
        );
        if status != last_status || status_written.elapsed() >= Duration::from_secs(10) {
            if let Err(error) =
                util::atomic_write(&state_dir.join("status"), status.as_bytes(), 0o600)
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
        if let Err(error) =
            util::atomic_write(&state_dir.join("startup"), b"state=stopped\n", 0o600)
        {
            logger.error(error);
        }
    }
    drop(cleanup);
    restore.map(|_| ())
}

pub fn send_signal(module: &Path, signal: i32) -> Result<()> {
    process_identity::verify(Path::new(STATE_DIR), module)?.signal(signal)
}
pub fn health(module: &Path) -> Result<String> {
    let process = process_identity::verify(Path::new(STATE_DIR), module)?;
    let path = Path::new(STATE_DIR).join("status");
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
    let _lock = util::acquire_flock(&state.join("daemon.lock"))?;
    Snapshot::load(state, Logger::new(state))?
        .restore()
        .map(|_| ())
}

pub fn restore_after_supervisor_crash(_module: &Path) -> Result<()> {
    let state = Path::new(STATE_DIR);
    let _lock = util::acquire_flock(&state.join("daemon.lock"))?;
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
}

impl ForegroundMonitor {
    fn detect(
        &mut self,
        rules: &AppModes,
        ignored_packages: &BTreeSet<String>,
        refresh: bool,
    ) -> Option<String> {
        let new_pids = read_top_app_pids();
        if new_pids == self.pids && !refresh {
            return None;
        }
        let added: BTreeSet<i32> = new_pids.difference(&self.pids).copied().collect();
        self.pids = new_pids;
        let processes: Vec<_> = self
            .pids
            .iter()
            .filter_map(|pid| read_process(*pid).map(|process| (*pid, process)))
            .collect();
        let Some(found) = select_foreground(
            &self.current,
            &processes,
            &added,
            ignored_packages,
            |package| rules.has_rule(package),
        ) else {
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

fn read_top_app_pids() -> BTreeSet<i32> {
    let path = [
        "/dev/cpuset/top-app/cgroup.procs",
        "/dev/cpuset/top-app/tasks",
        "/sys/fs/cgroup/top-app/cgroup.procs",
        "/sys/fs/cgroup/top-app/tasks",
    ]
    .iter()
    .find(|path| Path::new(path).exists())
    .copied();
    path.and_then(|path| util::read_text(Path::new(path)).ok())
        .unwrap_or_default()
        .split_whitespace()
        .filter_map(|value| value.parse().ok())
        .filter(|pid| *pid > 0)
        .collect()
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
        MODE_PATH,
        SCENE_REQUEST_PATH,
        OPTIONS_PATH,
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
