use std::collections::BTreeSet;
use std::ffi::CString;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::os::fd::AsRawFd;
use std::os::raw::c_int;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::ffi;

pub const FOREGROUND_FALLBACK: Duration = Duration::from_secs(2);

const WATCH_MASK: u32 = ffi::IN_MODIFY
    | ffi::IN_ATTRIB
    | ffi::IN_CLOSE_WRITE
    | ffi::IN_MOVED_FROM
    | ffi::IN_MOVED_TO
    | ffi::IN_CREATE
    | ffi::IN_DELETE
    | ffi::IN_DELETE_SELF
    | ffi::IN_MOVE_SELF;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WakeReason {
    Event,
    Watchdog,
    Interrupted,
    Unavailable,
}

impl WakeReason {
    pub fn needs_rescan(self) -> bool {
        matches!(self, Self::Event | Self::Watchdog | Self::Unavailable)
    }
}

/// Blocks on cgroup / config / touch changes instead of waking every few
/// hundred ms. Android kernels do not expose the same notification
/// capabilities everywhere, so a two-second foreground scan is retained even
/// when cgroup.events opens. Its populated notification is not a notification
/// of every member change. The touch signal (eventfd written by the input
/// thread) wakes the loop immediately so UI bursts like the notification
/// shade are recognised within one frame instead of two seconds.
pub struct EventLoop {
    inotify_fd: Option<c_int>,
    watch_count: usize,
    cgroup_events: Option<File>,
    touch_signal: Option<File>,
    touch_pending: bool,
}

impl EventLoop {
    pub fn new(paths: &[PathBuf], touch_signal: Option<File>) -> Self {
        let fd = unsafe { ffi::inotify_init1(ffi::IN_NONBLOCK | ffi::IN_CLOEXEC) };
        let mut event_loop = Self {
            inotify_fd: (fd >= 0).then_some(fd),
            watch_count: 0,
            cgroup_events: open_cgroup_events(),
            touch_signal,
            touch_pending: false,
        };
        event_loop.watch_paths(paths);
        event_loop
    }

    /// Consumes the pending touch wake (if any). Call once per loop turn.
    pub fn take_touch_signal(&mut self) -> bool {
        std::mem::take(&mut self.touch_pending)
    }

    pub fn describe(&self) -> String {
        match (self.inotify_fd, self.cgroup_events.is_some()) {
            (Some(_), true) => format!(
                "事件驱动：inotify {} 项监听 + cgroup 通知；2 秒前台扫描兜底",
                self.watch_count
            ),
            (Some(_), false) => format!(
                "事件驱动：inotify {} 项监听；2 秒前台扫描兜底",
                self.watch_count
            ),
            (None, _) => "当前内核未提供 inotify；仅以 2 秒前台扫描兜底".to_string(),
        }
    }

    pub fn wait(&mut self, timeout: Duration) -> WakeReason {
        let timeout = timeout.min(FOREGROUND_FALLBACK);
        let mut fds = Vec::with_capacity(3);
        if let Some(fd) = self.inotify_fd {
            fds.push(ffi::PollFd {
                fd,
                events: ffi::POLLIN,
                revents: 0,
            });
        }
        if let Some(file) = &self.cgroup_events {
            fds.push(ffi::PollFd {
                fd: file.as_raw_fd(),
                events: ffi::POLLPRI,
                revents: 0,
            });
        }
        if let Some(file) = &self.touch_signal {
            fds.push(ffi::PollFd {
                fd: file.as_raw_fd(),
                events: ffi::POLLIN,
                revents: 0,
            });
        }
        if fds.is_empty() {
            std::thread::sleep(timeout);
            return WakeReason::Unavailable;
        }
        let millis = timeout.as_millis().min(c_int::MAX as u128) as c_int;
        let result = unsafe { ffi::poll(fds.as_mut_ptr(), fds.len(), millis) };
        if result == 0 {
            return WakeReason::Watchdog;
        }
        if result < 0 {
            return if std::io::Error::last_os_error().raw_os_error() == Some(4) {
                WakeReason::Interrupted
            } else {
                // A broken descriptor must not turn the fallback into a
                // tight loop that burns CPU.
                std::thread::sleep(timeout);
                WakeReason::Unavailable
            };
        }
        let mut broken = false;
        if let Some(fd) = self.inotify_fd {
            let flags = fds
                .iter()
                .find(|entry| entry.fd == fd)
                .map(|e| e.revents)
                .unwrap_or(0);
            if flags & (ffi::POLLERR | ffi::POLLHUP | ffi::POLLNVAL) != 0
                || (flags & ffi::POLLIN != 0 && !self.drain_inotify(fd))
            {
                unsafe { ffi::close(fd) };
                self.inotify_fd = None;
                self.watch_count = 0;
                broken = true;
                eprintln!("novasched: inotify 失效，继续使用 2 秒前台扫描");
            }
        }
        if let Some(file) = self.cgroup_events.as_mut() {
            let flags = fds
                .iter()
                .find(|entry| entry.fd == file.as_raw_fd())
                .map(|e| e.revents)
                .unwrap_or(0);
            if flags & (ffi::POLLHUP | ffi::POLLNVAL) != 0 {
                self.cgroup_events = None;
                broken = true;
            } else if flags & (ffi::POLLPRI | ffi::POLLERR) != 0 {
                let mut discard = String::new();
                if let Err(error) = file
                    .seek(SeekFrom::Start(0))
                    .and_then(|_| file.read_to_string(&mut discard))
                {
                    eprintln!("novasched: cgroup 通知读取失败，继续使用 2 秒前台扫描: {error}");
                    self.cgroup_events = None;
                    broken = true;
                }
            }
        }
        if let Some(file) = self.touch_signal.as_mut() {
            let flags = fds
                .iter()
                .find(|entry| entry.fd == file.as_raw_fd())
                .map(|e| e.revents)
                .unwrap_or(0);
            if flags & (ffi::POLLIN | ffi::POLLERR) != 0 {
                let mut value = [0u8; 8];
                // 信号是电平触发的；读掉计数值即可。读失败按坏描述符处理：
                // 只置 None 让 File 的 Drop 关闭 fd。不能再显式 close——
                // Drop 会二次关闭同一 fd 号，若其间其它线程打开新文件复用了
                // 该号，会被误关。
                if file.read_exact(&mut value).is_err() {
                    self.touch_signal = None;
                    broken = true;
                } else {
                    self.touch_pending = true;
                }
            }
        }
        if broken {
            std::thread::sleep(timeout);
            return WakeReason::Unavailable;
        }
        WakeReason::Event
    }

    fn watch_paths(&mut self, paths: &[PathBuf]) {
        let Some(fd) = self.inotify_fd else {
            return;
        };
        let mut watched = BTreeSet::new();
        for path in paths {
            for candidate in [Some(path.as_path()), path.parent()] {
                let Some(candidate) = candidate.filter(|p| p.exists()) else {
                    continue;
                };
                if !watched.insert(candidate.to_path_buf()) {
                    continue;
                }
                let Ok(name) = CString::new(candidate.as_os_str().as_bytes()) else {
                    continue;
                };
                let added = unsafe { ffi::inotify_add_watch(fd, name.as_ptr(), WATCH_MASK) };
                if added >= 0 {
                    self.watch_count += 1;
                }
            }
        }
    }

    fn drain_inotify(&self, fd: c_int) -> bool {
        let mut buffer = [0u8; 4096];
        loop {
            let read = unsafe { ffi::read(fd, buffer.as_mut_ptr().cast(), buffer.len()) };
            if read == 0 {
                return false;
            }
            if read < 0 {
                match std::io::Error::last_os_error().raw_os_error() {
                    Some(4) => continue,
                    Some(11) => return true,
                    _ => return false,
                }
            }
        }
    }
}

impl Drop for EventLoop {
    fn drop(&mut self) {
        if let Some(fd) = self.inotify_fd.take() {
            unsafe { ffi::close(fd) };
        }
    }
}

fn open_cgroup_events() -> Option<File> {
    [
        "/sys/fs/cgroup/top-app/cgroup.events",
        "/sys/fs/cgroup/cgroup.events",
        "/dev/cpuset/top-app/cgroup.events",
    ]
    .iter()
    .find_map(|path| {
        let mut file = File::open(Path::new(path)).ok()?;
        let mut initial = String::new();
        file.read_to_string(&mut initial).ok()?;
        Some(file)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn watchdog_or_event_is_a_rescan() {
        assert_eq!(FOREGROUND_FALLBACK, Duration::from_secs(2));
        assert!(WakeReason::Event.needs_rescan());
        assert!(WakeReason::Watchdog.needs_rescan());
        assert!(!WakeReason::Interrupted.needs_rescan());
    }

    #[test]
    fn no_notification_source_caps_old_ten_second_wait_at_two_seconds() {
        let mut watcher = EventLoop {
            inotify_fd: None,
            watch_count: 0,
            cgroup_events: None,
            touch_signal: None,
            touch_pending: false,
        };
        let start = std::time::Instant::now();
        assert_eq!(
            watcher.wait(Duration::from_secs(10)),
            WakeReason::Unavailable
        );
        let elapsed = start.elapsed();
        assert!(elapsed >= FOREGROUND_FALLBACK);
        assert!(
            elapsed < Duration::from_secs(5),
            "fallback slept for {elapsed:?}"
        );
    }

    #[test]
    fn inotify_file_change_wakes_before_the_watchdog() {
        let dir = std::env::temp_dir().join(format!("nova-inotify-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config");
        std::fs::write(&path, "old").unwrap();
        let mut watcher = EventLoop::new(&[path.clone()], None);
        assert!(watcher.watch_count > 0, "test requires Linux inotify");
        std::fs::write(&path, "new").unwrap();
        let start = std::time::Instant::now();
        assert_eq!(watcher.wait(FOREGROUND_FALLBACK), WakeReason::Event);
        assert!(start.elapsed() < FOREGROUND_FALLBACK);
        drop(watcher);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn touch_signal_wakes_the_loop_and_is_consumed_once() {
        // 本测试依赖 Linux eventfd；其它平台跳过。
        let fd = unsafe { ffi::eventfd(0, ffi::O_NONBLOCK | ffi::O_CLOEXEC) };
        if fd < 0 {
            return;
        }
        use std::os::fd::FromRawFd;
        let read_end = unsafe { File::from_raw_fd(fd) };
        let dir = std::env::temp_dir().join(format!("nova-touch-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut watcher = EventLoop::new(&[], Some(read_end));
        let value: u64 = 1;
        let written = unsafe {
            ffi::write(fd, &value as *const u64 as *const ffi::c_void, std::mem::size_of::<u64>())
        };
        assert_eq!(written, 8, "test requires eventfd");
        let start = std::time::Instant::now();
        assert_eq!(watcher.wait(FOREGROUND_FALLBACK), WakeReason::Event);
        assert!(start.elapsed() < FOREGROUND_FALLBACK);
        assert!(watcher.take_touch_signal());
        assert!(!watcher.take_touch_signal(), "signal must be consumed once");
        drop(watcher);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
