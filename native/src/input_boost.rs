//! Touch-event-driven transient top-app boost. Polls /dev/input directly (no
//! subprocess), applies a short uclamp.min pulse on activity and expires back
//! to the dispatcher-maintained baseline. Event-driven and transient by
//! design - the antithesis of the static floor that caused the v1.2.0
//! power regression.
use std::fs::{self, File};
use std::io::Read;
use std::os::fd::AsRawFd;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::ffi;
use crate::logging::Logger;
use crate::scheduler::CpuctlLive;
use crate::snapshot::Snapshot;

pub struct InputBoostContext {
    pub live: Arc<CpuctlLive>,
    pub node: PathBuf,
    pub stock: String,
    pub stop: Arc<AtomicBool>,
    pub snapshot: Snapshot,
    pub logger: Logger,
}

/// Opens every /dev/input/event* device and spawns the boost loop. Returns
/// None when no input device is readable (feature silently inert).
pub fn spawn(ctx: InputBoostContext) -> Option<std::thread::JoinHandle<()>> {
    let mut files = Vec::new();
    let Ok(entries) = fs::read_dir("/dev/input") else {
        ctx.logger.debug("未找到 /dev/input，输入突频未启用");
        return None;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let is_event = path
            .file_name()
            .and_then(|v| v.to_str())
            .is_some_and(|name| name.starts_with("event"));
        if !is_event {
            continue;
        }
        match File::open(&path) {
            Ok(file) => files.push(file),
            Err(error) => ctx
                .logger
                .debug(format!("输入设备不可读，跳过: {} ({error})", path.display())),
        }
    }
    if files.is_empty() {
        ctx.logger.debug("没有可读输入设备，输入突频未启用");
        return None;
    }
    std::thread::Builder::new()
        .name("novasched-input".into())
        .spawn(move || run(ctx, files))
        .ok()
}

fn run(ctx: InputBoostContext, mut files: Vec<File>) {
    let mut poll_fds: Vec<ffi::PollFd> = files
        .iter()
        .map(|file| ffi::PollFd {
            fd: file.as_raw_fd(),
            events: ffi::POLLIN,
            revents: 0,
        })
        .collect();
    let mut boosted = false;
    let mut until = Instant::now();
    loop {
        if ctx.stop.load(Ordering::Relaxed) {
            break;
        }
        let readable = unsafe { ffi::poll(poll_fds.as_mut_ptr(), poll_fds.len(), 150) };
        let mut activity = false;
        if readable > 0 {
            for (file, entry) in files.iter_mut().zip(poll_fds.iter_mut()) {
                if entry.revents & ffi::POLLIN == 0 {
                    continue;
                }
                let mut chunk = [0u8; 128];
                match file.read(&mut chunk) {
                    Ok(n) if n > 0 => activity = true,
                    Ok(_) => {}
                    Err(_) => {}
                }
                let _ = chunk;
            }
        }
        let now = Instant::now();
        if boosted && now >= until {
            if let Err(error) = ctx.snapshot.write(&ctx.node, &ctx.stock) {
                ctx.logger
                    .debug(format!("输入突频回落写入失败: {error}"));
            }
            boosted = false;
        }
        if !activity {
            continue;
        }
        let Ok(state) = ctx.live.inner.lock() else {
            continue;
        };
        if !state.allowed || !state.active || state.boost_min.is_empty() {
            continue;
        }
        if boosted {
            until = now + Duration::from_millis(state.duration_ms);
            continue;
        }
        match ctx.snapshot.write(&ctx.node, &state.boost_min) {
            Ok(()) => {
                boosted = true;
                until = now + Duration::from_millis(state.duration_ms);
                ctx.logger
                    .debug(format!("输入突频已启用: {} = {}", ctx.node.display(), state.boost_min));
            }
            Err(error) => ctx
                .logger
                .debug(format!("输入突频写入失败: {error}")),
        }
    }
    if boosted {
        let _ = ctx.snapshot.write(&ctx.node, &ctx.stock);
    }
}
