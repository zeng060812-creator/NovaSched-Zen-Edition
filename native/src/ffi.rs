use std::os::raw::{c_char, c_int, c_long, c_uint, c_void};

pub const LOCK_EX: c_int = 2;
pub const LOCK_NB: c_int = 4;
pub const SIGHUP: c_int = 1;
pub const SIGINT: c_int = 2;
pub const SIGPIPE: c_int = 13;
pub const SIGTERM: c_int = 15;
pub const SIG_IGN: usize = 1;
pub const IN_NONBLOCK: c_int = 0o0004000;
pub const IN_CLOEXEC: c_int = 0o2000000;
pub const IN_MODIFY: c_uint = 0x0000_0002;
pub const IN_ATTRIB: c_uint = 0x0000_0004;
pub const IN_CLOSE_WRITE: c_uint = 0x0000_0008;
pub const IN_MOVED_FROM: c_uint = 0x0000_0040;
pub const IN_MOVED_TO: c_uint = 0x0000_0080;
pub const IN_CREATE: c_uint = 0x0000_0100;
pub const IN_DELETE: c_uint = 0x0000_0200;
pub const IN_DELETE_SELF: c_uint = 0x0000_0400;
pub const IN_MOVE_SELF: c_uint = 0x0000_0800;
pub const POLLIN: i16 = 0x0001;
pub const POLLPRI: i16 = 0x0002;
pub const POLLERR: i16 = 0x0008;
pub const POLLHUP: i16 = 0x0010;
pub const POLLNVAL: i16 = 0x0020;

#[repr(C)]
pub struct PollFd {
    pub fd: c_int,
    pub events: i16,
    pub revents: i16,
}

#[repr(C)]
pub struct Tm {
    pub tm_sec: c_int,
    pub tm_min: c_int,
    pub tm_hour: c_int,
    pub tm_mday: c_int,
    pub tm_mon: c_int,
    pub tm_year: c_int,
    pub tm_wday: c_int,
    pub tm_yday: c_int,
    pub tm_isdst: c_int,
    pub tm_gmtoff: c_long,
    pub tm_zone: *const c_char,
}

extern "C" {
    pub fn flock(fd: c_int, operation: c_int) -> c_int;
    pub fn geteuid() -> c_uint;
    pub fn getpid() -> c_int;
    pub fn syscall(number: c_long, ...) -> c_long;
    pub fn setsid() -> c_int;
    pub fn fchown(fd: c_int, uid: c_uint, gid: c_uint) -> c_int;
    #[cfg(target_os = "android")]
    pub fn fgetxattr(fd: c_int, name: *const c_char, value: *mut c_void, size: usize) -> isize;
    #[cfg(target_os = "android")]
    pub fn fsetxattr(
        fd: c_int,
        name: *const c_char,
        value: *const c_void,
        size: usize,
        flags: c_int,
    ) -> c_int;
    pub fn localtime_r(timep: *const c_long, result: *mut Tm) -> *mut Tm;
    pub fn signal(signal: c_int, handler: usize) -> usize;
    pub fn strftime(buffer: *mut c_char, max: usize, format: *const c_char, tm: *const Tm)
        -> usize;
    pub fn time(tloc: *mut c_long) -> c_long;
    pub fn close(fd: c_int) -> c_int;
    pub fn read(fd: c_int, buf: *mut c_void, count: usize) -> isize;
    pub fn poll(fds: *mut PollFd, nfds: usize, timeout: c_int) -> c_int;
    pub fn inotify_init1(flags: c_int) -> c_int;
    pub fn inotify_add_watch(fd: c_int, pathname: *const c_char, mask: c_uint) -> c_int;
}

#[cfg(target_os = "android")]
extern "C" {
    pub fn __system_property_get(name: *const c_char, value: *mut c_char) -> c_int;
}

pub type SignalHandler = extern "C" fn(c_int);

pub fn install_signal(sig: c_int, handler: SignalHandler) {
    unsafe {
        signal(sig, handler as usize);
    }
}

pub fn ignore_signal(sig: c_int) {
    unsafe {
        signal(sig, SIG_IGN);
    }
}

pub fn errno_message(prefix: &str) -> String {
    format!("{prefix}: {}", std::io::Error::last_os_error())
}

#[allow(dead_code)]
pub type VoidPtr = *mut c_void;
