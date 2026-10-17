//! Keep installation checks bounded without modifying the manager's traps.
use crate::{
    ffi,
    util::{self, Result},
};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::{
    fs::OpenOptionsExt,
    process::{CommandExt, ExitStatusExt},
};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const OUTPUT_LIMIT: u64 = 1_048_576;

pub fn run(module: &Path, phase: &str) -> Result<()> {
    let (seconds, waiting) = match phase {
        "prepare-storage" => (15,"正在整理模块数据目录。"),
        "self-test" => (20, "等待核心自检结果。"),
        "probe" => (20, "正在读取处理器与内核能力。"),
        "prepare-config" => (30, "正在准备配置和备份。"),
        "prepare-auto" => (10, "正在保存自动调度偏好。"),
        "scene-status" => (15, "正在查询 Scene 安装状态。"),
        "scene-install" => (20, "正在备份并接管 Scene 调度。"),
        _ => return Err("未知安装检查，拒绝执行".into()),
    };
    let state=Path::new(util::STATE_DIR);
    let temp=state.join("run/install");
    util::create_dir(&temp)?;
    let log=temp.join(format!("{phase}.log"));
    let receipt=temp.join(format!("{phase}.exit"));
    util::remove_if_exists(&receipt)?;
    let mut command = Command::new(module.join("bin/novasched"));
    command.arg(phase).arg("--module-dir").arg(module);
    let outcome = execute(
        &mut command,
        &log,
        Duration::from_secs(seconds),
        |elapsed| {
            println!("{waiting} 已等待 {elapsed} 秒。");
            let _ = std::io::stdout().flush();
        },
    );
    let (code, error) = match outcome {
        Ok(code) => (code, None),
        Err(error) => (1, Some(error)),
    };
    // The check binary prints Chinese text, but a child killed mid-write can
    // leave truncated multi-byte sequences. Lossy decoding keeps the rest of
    // the report readable instead of aborting the whole installation chain.
    let mut output = String::new();
    if let Ok(file) = File::open(&log) {
        let mut bytes = Vec::new();
        file.take(OUTPUT_LIMIT)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        output = String::from_utf8_lossy(&bytes).into_owned();
    }
    let detail = error.unwrap_or_else(|| match code {
        0 => String::new(),
        124 => format!("{phase} 超过 {seconds} 秒，已终止该检查及其查询子进程"),
        _ => format!("{phase} 退出码 {code}"),
    });
    crate::logging::append_bounded(&state.join("logs/install.log"),&format!("\n[{phase}]\n{output}\nexit={code}\n{detail}\n")).map_err(|e|e.to_string())?;
    if code != 0 {
        if !output.is_empty() {
            println!("{output}");
        }
        println!("{detail}");
    } else {
        // Keep the console readable; the full hardware report stays in logs.
        for line in output.lines().filter(|line| {
            line.starts_with("storage_deferred=")
                || line.starts_with("soc=")
                || line.starts_with("profile=")
                || line.starts_with("config=")
                || line.starts_with("backup=")
                || line.starts_with("automatic_enabled=")
                || line.starts_with("scene_presence=")
                || line.starts_with("scene_detection=")
                || line.starts_with("Scene ")
                || line.starts_with("compatibility=")
        }) {
            println!("{line}");
        }
        let adaptations = output
            .lines()
            .filter(|line| line.starts_with("adaptation="))
            .count();
        if adaptations > 0 {
            println!("已按内核能力调整 {adaptations} 项配置，明细见安装记录。");
        }
    }
    util::atomic_write(&receipt, format!("{code}\n").as_bytes(), 0o600)
}

fn execute(
    command: &mut Command,
    log: &Path,
    budget: Duration,
    mut progress: impl FnMut(u64),
) -> Result<i32> {
    let output = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .mode(0o600)
        .open(log)
        .map_err(|e| e.to_string())?;
    let stderr = output.try_clone().map_err(|e| e.to_string())?;
    command.stdin(Stdio::null()).stdout(output).stderr(stderr);
    unsafe {
        command.pre_exec(|| {
            if ffi::setsid() < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = command
        .spawn()
        .map_err(|e| format!("启动安装检查失败: {e}"))?;
    let started = Instant::now();
    let mut next = Duration::from_secs(3);
    loop {
        match child.try_wait() {
            Ok(Some(exit)) => {
                if fs::metadata(log).map_err(|e| e.to_string())?.len() > OUTPUT_LIMIT {
                    return Err("安装检查输出超过 1 MiB".into());
                }
                return Ok(exit.code().unwrap_or(128 + exit.signal().unwrap_or(1)));
            }
            Ok(None) => {}
            Err(error) => {
                terminate(&mut child)?;
                return Err(format!("检查安装进程失败: {error}"));
            }
        }
        let elapsed = started.elapsed();
        if elapsed >= budget {
            terminate(&mut child)?;
            return Ok(124);
        }
        match fs::metadata(log) {
            Ok(meta) if meta.len() <= OUTPUT_LIMIT => {}
            result => {
                terminate(&mut child)?;
                return Err(match result {
                    Err(error) => format!("读取安装检查记录失败: {error}"),
                    _ => "安装检查输出超过 1 MiB，已停止该检查".into(),
                });
            }
        }
        if elapsed >= next {
            progress(elapsed.as_secs());
            next += Duration::from_secs(3);
        }
        util::sleep(Duration::from_millis(50));
    }
}

fn terminate(child: &mut std::process::Child) -> Result<()> {
    // The unreaped child is a new session leader: its process-group ID cannot
    // belong to another application. Kill the entire installation check tree.
    if unsafe { ffi::kill(-(child.id() as i32), ffi::SIGKILL) } != 0 {
        child.kill().map_err(|e| format!("终止安装检查失败: {e}"))?;
    }
    child
        .wait()
        .map(|_| ())
        .map_err(|e| format!("回收安装检查失败: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("nova-install-{}-{name}", std::process::id()))
    }
    #[test]
    fn captures_output_and_nonzero_exit_without_losing_status() {
        let log = fixture("exit");
        let mut cmd = Command::new("sh");
        cmd.args(["-c", "printf success; printf failure >&2; exit 7"]);
        assert_eq!(
            execute(&mut cmd, &log, Duration::from_secs(1), |_| {}).unwrap(),
            7
        );
        assert_eq!(fs::read_to_string(&log).unwrap(), "successfailure");
        fs::remove_file(log).unwrap();
    }
    #[test]
    fn deadline_stops_the_query_and_its_descendants() {
        let log = fixture("timeout");
        let leaked = fixture("leaked");
        let mut cmd = Command::new("sh");
        cmd.args(["-c", "(sleep 0.4; printf leaked > \"$1\") & wait", "test"]);
        cmd.arg(&leaked);
        assert_eq!(
            execute(&mut cmd, &log, Duration::from_millis(100), |_| {}).unwrap(),
            124
        );
        util::sleep(Duration::from_millis(500));
        assert!(
            !leaked.exists(),
            "a timed-out query descendant kept running"
        );
        fs::remove_file(log).unwrap();
    }
    #[test]
    fn signalled_checks_cannot_look_successful() {
        let log = fixture("signal");
        let mut cmd = Command::new("sh");
        cmd.args(["-c", "kill -TERM $$"]);
        assert_eq!(
            execute(&mut cmd, &log, Duration::from_secs(1), |_| {}).unwrap(),
            143
        );
        fs::remove_file(log).unwrap();
    }
}
