use crate::{
    daemon, ffi,
    logging::Logger,
    process_identity,
    util::{self, Result, STATE_DIR},
};
use std::fs::{self, OpenOptions};
use std::io::{Read, Seek, SeekFrom};
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

fn root() -> Result<()> {
    if unsafe { ffi::geteuid() } != 0 {
        Err("请在 MT 使用 su -c 执行".into())
    } else {
        Ok(())
    }
}
pub fn start(module: &Path, restart: bool) -> Result<()> {
    root()?;
    ensure_not_removing(module)?;
    crate::boot_guard::ensure_ready()?;
    let state = Path::new(STATE_DIR);
    let _control = util::acquire_flock(&crate::storage::path(&state, "control.lock"))?;
    ensure_not_removing(module)?;
    let daemon_running = process_identity::lock_held(&crate::storage::path(&state, "daemon.lock"))?;
    let supervisor_running = process_identity::lock_held(&crate::storage::path(&state, "supervisor.lock"))?;
    if daemon_running && !restart {
        return daemon::print_status(module);
    }
    if restart && (daemon_running || supervisor_running) {
        stop_inner(module)?;
    } else if supervisor_running {
        return Err("崩溃监督正在恢复守护；请稍后再启动".into());
    }
    for _ in 0..100 {
        if !process_identity::lock_held(&crate::storage::path(&state, "supervisor.lock"))? {
            break;
        }
        util::sleep(Duration::from_millis(100));
    }
    if process_identity::lock_held(&crate::storage::path(&state, "supervisor.lock"))? {
        return Err("已有启动监督，未创建第二实例，请稍后重试".into());
    }
    util::remove_if_exists(&crate::storage::path(&state, "last_error"))?;
    util::remove_if_exists(&crate::storage::path(&state, "stop.requested"))?;
    util::atomic_write(&crate::storage::path(&state, "startup"), b"state=launching\n", 0o600)?;
    crate::logging::append_bounded(&crate::storage::path(state,"bootstrap.log"),"\n-- start --\n").map_err(|e|e.to_string())?;
    let log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(crate::storage::path(&state, "bootstrap.log"))
        .map_err(|e| e.to_string())?;
    let stderr = log.try_clone().map_err(|e| e.to_string())?;
    let exe = fs::canonicalize(module.join("bin/novasched")).map_err(|e| e.to_string())?;
    let mut command = Command::new(exe);
    command
        .arg("supervise")
        .arg("--module-dir")
        .arg(module)
        .stdin(Stdio::null())
        .stdout(log)
        .stderr(stderr);
    unsafe {
        command.pre_exec(|| {
            if ffi::setsid() < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = command.spawn().map_err(|e| format!("启动监督失败: {e}"))?;
    let deadline = Instant::now() + Duration::from_secs(130);
    while Instant::now() < deadline {
        if ensure_not_removing(module).is_err() {
            stop_inner(module)?;
            return Err("卸载已开始，已取消启动".into());
        }
        if let Some(exit) = child.try_wait().map_err(|e| e.to_string())? {
            return Err(format!(
                "调度未就绪，监督退出 {exit}；{}",
                util::read_trimmed(crate::storage::path(&state, "last_error"))
                    .unwrap_or_else(|_| "请运行 diagnose".into())
            ));
        }
        if util::read_trimmed(crate::storage::path(&state, "startup")).ok().as_deref() == Some("state=running")
            && daemon::health(module).is_ok()
        {
            println!("已验证进程身份、排他锁与调度心跳；实际下发状态见 phase");
            return daemon::print_status(module);
        }
        util::sleep(Duration::from_millis(200));
    }
    Err("启动等待超时，未报告成功；请运行 diagnose，勿重复点击启动".into())
}
pub fn supervise(module: &Path) -> Result<()> {
    root()?;
    let state = Path::new(STATE_DIR);
    let _lock = util::acquire_flock(&crate::storage::path(&state, "supervisor.lock"))?;
    if process_identity::lock_held(&crate::storage::path(&state, "daemon.lock"))? {
        return Err("已有守护持锁，取消重复启动".into());
    }
    let logger = Logger::new(state);
    logger.info("启动监督就绪；异常退出最多自动重启三次");
    let binary = module.join("bin/novasched");
    let mut last_error = String::new();
    for attempt in 0..=3 {
        if stop_requested(state) {
            finish_requested_stop(state)?;
            return Ok(());
        }
        logger.info(format!("启动守护进程，第 {}/4 次尝试", attempt + 1));
        let mut command = Command::new(&binary);
        command
            .arg("daemon")
            .arg("--module-dir")
            .arg(module)
            .stdin(Stdio::null());
        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(error) => {
                last_error = format!("执行 NovaSched ELF 失败: {error}");
                logger.error(&last_error);
                record_failure(state, "执行 ELF", &last_error, true)?;
                if attempt < 3 && wait_retry_or_stop(state, attempt + 1) {
                    finish_requested_stop(state)?;
                    return Ok(());
                }
                continue;
            }
        };
        let exit = wait_child(&mut child, module, state)?;
        if module.join("disable").exists() {
            return Err("启动保护已停用模块，本次不再重试".into());
        }
        if stop_requested(state) {
            logger.info("收到停止标记，取消守护重启");
            finish_requested_stop(state)?;
            return Ok(());
        }
        last_error = format!(
            "守护意外退出 code={:?}, signal={:?}",
            exit.code(),
            exit.signal()
        );
        logger.error(&last_error);
        record_failure(state, "进程退出", &last_error, true)?;
        if attempt < 3 && wait_retry_or_stop(state, attempt + 1) {
            finish_requested_stop(state)?;
            return Ok(());
        }
    }

    match daemon::restore_after_supervisor_crash(module) {
        Ok(()) => logger.warn("连续四次启动失败；原厂节点快照和可恢复的 Scene 回调已恢复"),
        Err(error) => {
            logger.error(format!("连续启动失败后的自动恢复未完成: {error}"));
            last_error.push_str(&format!("；自动恢复失败: {error}"));
        }
    }
    record_failure(state, "重启耗尽", &last_error, true)?;
    Err(format!("重试耗尽，已触发恢复: {last_error}"))
}

fn wait_child(child: &mut Child, module: &Path, state: &Path) -> Result<std::process::ExitStatus> {
    let mut term_sent = false;
    loop {
        if stop_requested(state) && !term_sent {
            if let Ok(process) = process_identity::verify(state, module) {
                term_sent = process.signal(ffi::SIGTERM).is_ok();
            }
        }
        if let Some(status) = child.try_wait().map_err(|error| error.to_string())? {
            return Ok(status);
        }
        util::sleep(Duration::from_secs(1));
    }
}

fn stop_requested(state: &Path) -> bool {
    crate::storage::path(&state, "stop.requested").exists()
}

fn finish_requested_stop(state: &Path) -> Result<()> {
    if util::read_trimmed(crate::storage::path(&state, "startup")).ok().as_deref() != Some("state=failed") {
        util::atomic_write(&crate::storage::path(&state, "startup"), b"state=stopped\n", 0o600)?;
    }
    util::remove_if_exists(&crate::storage::path(&state, "stop.requested"))
}

/// Returns true when a user stop arrived before the backoff expired.
fn wait_retry_or_stop(state: &Path, retry: usize) -> bool {
    let deadline = Instant::now() + Duration::from_secs(retry as u64);
    while Instant::now() < deadline {
        if stop_requested(state) {
            return true;
        }
        util::sleep(Duration::from_millis(100));
    }
    stop_requested(state)
}
pub fn record_failure(state: &Path, stage: &str, error: &str, preserve: bool) -> Result<()> {
    let old = if preserve {
        util::read_text(&crate::storage::path(&state, "last_error")).unwrap_or_default()
    } else {
        String::new()
    };
    util::atomic_write(
        &crate::storage::path(&state, "last_error"),
        format!(
            "{old}time={}\nstage={stage}\nerror={error}\n",
            util::local_timestamp()
        )
        .as_bytes(),
        0o600,
    )?;
    util::atomic_write(&crate::storage::path(&state, "startup"), b"state=failed\n", 0o600)
}
pub fn stop(module: &Path) -> Result<()> {
    root()?;
    let _control = util::acquire_flock(&crate::storage::path(Path::new(STATE_DIR), "control.lock"))?;
    stop_inner(module)
}
pub(crate) fn stop_inner(module: &Path) -> Result<()> {
    let state = Path::new(STATE_DIR);
    util::create_dir(state)?;
    let daemon_running = process_identity::lock_held(&crate::storage::path(&state, "daemon.lock"))?;
    let supervisor_running = process_identity::lock_held(&crate::storage::path(&state, "supervisor.lock"))?;
    if !daemon_running && !supervisor_running {
        println!("守护未持锁，无需发送信号");
        return Ok(());
    }
    util::atomic_write(&crate::storage::path(&state, "stop.requested"), b"stop=1\n", 0o600)?;
    if daemon_running {
        if let Err(error) =
            process_identity::verify(state, module).and_then(|process| process.signal(ffi::SIGTERM))
        {
            eprintln!("novasched: SIGTERM 未送达，将等待守护读取停止标记: {error}");
        }
    }
    for _ in 0..300 {
        if !process_identity::lock_held(&crate::storage::path(&state, "daemon.lock"))?
            && !process_identity::lock_held(&crate::storage::path(&state, "supervisor.lock"))?
        {
            util::remove_if_exists(&crate::storage::path(&state, "stop.requested"))?;
            if !daemon_running {
                println!("已取消守护重试；节点状态由最近一次失败恢复流程记录");
                return Ok(());
            }
            if util::read_trimmed(crate::storage::path(&state, "startup")).ok().as_deref() == Some("state=failed") {
                return Err(format!(
                    "守护已退出，但退出或回滚失败: {}",
                    util::read_trimmed(crate::storage::path(&state, "last_error"))
                        .unwrap_or_else(|_| "请查看内部日志".into())
                ));
            }
            println!("守护已退出；回滚结果保留于内部日志");
            return Ok(());
        }
        util::sleep(Duration::from_millis(100));
    }
    Err("停止/回滚超时；未发送 SIGKILL，未删除锁，未启动第二实例".into())
}
fn ensure_not_removing(module: &Path) -> Result<()> {
    if module.join("disable").exists() {
        Err("模块已停用，请先查看启动保护和诊断记录".into())
    } else if !module.is_dir() || module.join(".uninstalling").exists() || module.join("remove").exists() {
        Err("模块正在卸载，不能启动".into())
    } else {
        Ok(())
    }
}
pub fn request_webui_start(module: &Path) -> Result<&'static str> {
    root()?;
    ensure_not_removing(module)?;
    crate::boot_guard::ensure_ready()?;
    let state = Path::new(STATE_DIR);
    util::create_dir(state)?;
    let _lock = util::acquire_flock(&crate::storage::path(&state, "webui-start.lock"))?;
    let phase = util::read_trimmed(crate::storage::path(&state, "startup")).unwrap_or_default();
    if phase == "state=failed" || phase == "state=stopped" || stop_requested(state) {
        return Ok("服务已停止或启动失败；点状态卡读取诊断，可使用「启动服务」重试");
    }
    if process_identity::lock_held(&crate::storage::path(&state, "daemon.lock"))?
        || process_identity::lock_held(&crate::storage::path(&state, "supervisor.lock"))?
        || process_identity::lock_held(&crate::storage::path(&state, "control.lock"))?
    { return Ok("服务正在启动，页面会自动重连"); }
    let boot = util::read_trimmed("/proc/sys/kernel/random/boot_id")?;
    let ticket = crate::storage::path(&state, "webui-start.boot");
    if util::read_trimmed(&ticket).ok().as_deref() == Some(&boot) {
        return Ok("本次开机已尝试补启动；点状态卡读取诊断");
    }
    let log = OpenOptions::new().create(true).append(true)
        .open(module.join("boot-entry.log")).map_err(|e| e.to_string())?;
    let mut command = Command::new("/system/bin/sh");
    command.arg(module.join("service.sh")).stdin(Stdio::null())
        .stdout(log.try_clone().map_err(|e| e.to_string())?).stderr(log);
    unsafe {
        command.pre_exec(|| {
            if ffi::setsid() < 0 { return Err(std::io::Error::last_os_error()); }
            Ok(())
        });
    }
    let _child = command.spawn().map_err(|e| format!("补启动失败: {e}"))?;
    util::atomic_write(&ticket, boot.as_bytes(), 0o600)?;
    Ok("已补启动模块服务，首次解锁稳定等待结束后自动连接")
}

pub fn diagnose(module: &Path) -> Result<()> {
    root()?;
    println!(
        "NovaSched Zen Edition {}\ntime={}\nmodule={}\n",
        env!("CARGO_PKG_VERSION"),
        util::local_timestamp(),
        module.display()
    );
    let running = match daemon::print_status(module) {
        Ok(()) => true,
        Err(error) => { println!("status_error={error}"); false }
    };
    println!(
        "manager_gate=none\nmodule_disabled={}\nmodule_remove_pending={}\nboot_completed={}",
        module.join("disable").exists(),
        module.join("remove").exists(),
        util::getprop("sys.boot_completed")
    );
    println!("\n[android-user]\n{}", crate::boot_guard::diagnostic());
    for name in [
        "startup",
        "boot-guard",
        "daemon.identity",
        "last_error",
        "options.txt",
        "mode.txt",
        "scene-request.txt",
        "boot-entry.log",
        "bootstrap.log",
        "novasched.log",
    ] {
        println!("\n[{name}]");
        match tail(&crate::storage::path(Path::new(STATE_DIR), name), 32768) {
            Ok(s) => println!("{s}"),
            Err(e) => println!("{e}"),
        }
    }
    if module.join("boot-entry.log").exists(){println!("\n[legacy-module/boot-entry.log]\n{}",tail(&module.join("boot-entry.log"),32768)?);}
    println!(
        "\n[scene-telemetry]\n{}",
        crate::telemetry::inspect_scene().diagnostic_text()
    );
    match crate::config::Config::load(Path::new(crate::util::CONFIG_PATH)) {
        Ok(config) if running => println!("\n[policy-audit]\n{}", daemon::print_policy_audit(&config)),
        Ok(_) => println!("\n[policy-audit]\n守护未运行，当前配置尚未由本模块下发；启动后再核对策略。"),
        Err(e) => println!("\n[policy-audit] 配置读取失败: {e}"),
    }
    Ok(())
}
/// Preserve identity verification and add context, without guessing which
/// manager/version caused an absent identity. Only public diagnostic files
/// are read here; webui.session is deliberately never included.
pub fn connection_failure(state: &Path, module: &Path, identity_error: &str) -> String {
    let mut details = vec![format!("守护状态未通过验证: {identity_error}")];
    if module.join("disable").exists() {
        details.push("模块已被禁用，请在管理器中启用后重启手机".into());
    }
    if module.join("remove").exists() {
        details.push("模块已标记待卸载".into());
    }
    if let Ok(stage) = tail(&crate::storage::path(&state, "startup"), 256) {
        details.push(format!("最近启动阶段: {}", stage.trim()));
    }
    if let Ok(error) = tail(&crate::storage::path(&state, "last_error"), 2048) {
        if !error.trim().is_empty() {
            details.push(format!("启动错误: {}", error.trim()));
        }
    }
    details.push("请读取 novasched diagnose；其中 boot-entry.log 记录开机入口，bootstrap.log 记录监督与守护退出原因".into());
    details.join("；")
}

pub fn tail(path: &Path, limit: u64) -> Result<String> {
    let mut file = fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let len = file.metadata().map_err(|e| e.to_string())?.len();
    file.seek(SeekFrom::Start(len.saturating_sub(limit)))
        .map_err(|e| e.to_string())?;
    let mut bytes = Vec::new();
    file.take(limit)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

#[cfg(test)]
mod compatibility_tests {
    use super::*;
    #[test]
    fn missing_identity_explains_saved_startup_error_without_leaking_credentials() {
        let root =
            std::env::temp_dir().join(format!("nova-startup-diagnostic-{}", std::process::id()));
        let state = root.join("state");
        let module = root.join("module");
        crate::storage::prepare(&state).unwrap();
        fs::create_dir_all(&module).unwrap();
        fs::write(crate::storage::path(&state, "startup"), "state=failed\n").unwrap();
        fs::write(
            crate::storage::path(&state, "last_error"),
            "stage=环境校验\nerror=required cgroup missing\n",
        )
        .unwrap();
        fs::write(
            crate::storage::path(&state, "webui.session"),
            "PRIVATE_CREDENTIAL_MUST_NOT_BE_READ",
        )
        .unwrap();
        fs::write(module.join("disable"), "").unwrap();
        let report = connection_failure(&state, &module, "daemon.identity missing");
        assert!(report.contains("required cgroup missing"));
        assert!(report.contains("已被禁用"));
        assert!(report.contains("state=failed"));
        assert!(report.contains("boot-entry.log"));
        assert!(!report.contains("PRIVATE_CREDENTIAL"));
        fs::remove_file(crate::storage::path(&state, "last_error")).unwrap();
        fs::remove_file(crate::storage::path(&state, "startup")).unwrap();
        assert!(!connection_failure(&state, &module, "identity missing").contains("启动错误:"));
        fs::remove_dir_all(root).unwrap();
    }
}
/// Installation check: never writes a scheduling policy.
pub fn self_test(module: &Path) -> Result<()> {
    let time = util::local_timestamp();
    if time.len() != 19 {
        return Err("时间格式自检失败".into());
    }
    let worker = std::thread::Builder::new()
        .name("novasched-self-test".into())
        .spawn(|| {
            let lock = std::sync::Mutex::new(17u32);
            let value = lock.lock().map_err(|_| "线程锁自检失败".to_string())?;
            if *value != 17 {
                return Err("线程内存自检失败".to_string());
            }
            Ok(())
        })
        .map_err(|e| format!("线程/TLS 自检失败: {e}"))?;
    worker.join().map_err(|_| "线程自检异常".to_string())??;
    for soc in crate::soc::SUPPORTED {
        crate::config::Config::load(&module.join("config").join(soc.file()))?;
    }
    let _device = util::getprop("ro.product.device");
    process_identity::self_test().map_err(|error| format!("进程身份/pidfd 自检失败: {error}"))?;
    println!(
        "self_test=passed\nversion={}\ntime={time}\npolicy_writes=0",
        env!("CARGO_PKG_VERSION")
    );
    Ok(())
}
