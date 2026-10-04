//! Per-daemon WebUI credentials. Never include secrets in logs or status payloads.
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::net::{Ipv4Addr, Ipv6Addr};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::Path;

use crate::util::{self, Result, STATE_DIR};

pub const AUTH_PREFIX: &str = "novasched-auth.";
const SESSION_FILE: &str = "webui.session";
const ORIGINS_FILE: &str = "webui.origins";
const ORIGINS_LOCK: &str = "webui-origins.lock";
const DEFAULT_ORIGIN: &str = "https://mui.kernelsu.org";
const MAX_ORIGINS: usize = 8;
// Linux/Android O_NOFOLLOW; using the same fd for metadata/read avoids TOCTOU.
const O_NOFOLLOW: i32 = 0o400000;

pub struct Session {
    pid: u32,
    port: u16,
    token: String,
}

impl Session {
    pub(crate) fn port(&self) -> u16 {
        self.port
    }
    pub(crate) fn protocol(&self) -> String {
        format!("{AUTH_PREFIX}{}", self.token)
    }
    #[cfg(test)]
    pub(crate) fn for_test(token: &str) -> Self {
        assert_eq!(token.len(), 64);
        Self {
            pid: 123,
            port: 31415,
            token: token.into(),
        }
    }
    #[cfg(test)]
    pub(crate) fn for_test_at(port: u16, token: &str) -> Self {
        let mut session = Self::for_test(token);
        session.port = port;
        session
    }

    pub fn rotate(state: &Path, port: u16) -> Result<Self> {
        if unsafe { crate::ffi::geteuid() } != 0 {
            return Err("生成 WebUI 凭据需要 root".into());
        }
        validate_port(port)?;
        let token = random_token(
            &mut File::open("/dev/urandom")
                .map_err(|_| "无法打开系统随机源，未启用 WebUI".to_string())?,
        )?;
        let session = Self {
            pid: std::process::id(),
            port,
            token,
        };
        session.persist(state)?;
        // A new daemon session starts with only the built-in manager origin.
        // Custom exact origins are registered again by an authenticated root read.
        util::atomic_write(
            &state.join(ORIGINS_FILE),
            format!("{DEFAULT_ORIGIN}\n").as_bytes(),
            0o600,
        )
        .map_err(|_| "无法初始化 WebUI 来源白名单，未启用 WebUI".to_string())?;
        Ok(session)
    }

    fn persist(&self, state: &Path) -> Result<()> {
        util::atomic_write(
            &state.join(SESSION_FILE),
            format!(
                "pid={}\nport={}\ntoken={}\n",
                self.pid, self.port, self.token
            )
            .as_bytes(),
            0o600,
        )
        .map_err(|_| "无法安全保存 WebUI 凭据，未启用 WebUI".into())
    }

    pub fn accepts(&self, protocol: &str) -> bool {
        let Some(candidate) = protocol.strip_prefix(AUTH_PREFIX) else {
            return false;
        };
        if candidate.len() != 64 {
            return false;
        }
        // Compare all 64 bytes rather than stop on the first mismatching byte.
        let difference = candidate
            .bytes()
            .zip(self.token.bytes())
            .fold(0u8, |difference, (a, b)| difference | (a ^ b));
        difference == 0
    }
}

fn random_token(source: &mut impl Read) -> Result<String> {
    let mut bytes = [0u8; 32];
    source
        .read_exact(&mut bytes)
        .map_err(|_| "系统随机源读取失败，未启用 WebUI".to_string())?;
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut token = String::with_capacity(64);
    for byte in bytes {
        token.push(HEX[(byte >> 4) as usize] as char);
        token.push(HEX[(byte & 15) as usize] as char);
    }
    Ok(token)
}

fn validate_port(port: u16) -> Result<()> {
    if port >= 1024 {
        Ok(())
    } else {
        Err("WebUI 凭据端口无效".into())
    }
}

fn parse(text: &str) -> Result<Session> {
    let mut lines = text.lines();
    let pid = lines
        .next()
        .and_then(|line| line.strip_prefix("pid="))
        .and_then(|value| value.parse::<u32>().ok())
        .filter(|pid| *pid > 0)
        .ok_or_else(|| "WebUI 凭据进程无效".to_string())?;
    let port = lines
        .next()
        .and_then(|line| line.strip_prefix("port="))
        .and_then(|value| value.parse::<u16>().ok())
        .ok_or_else(|| "WebUI 凭据端口无效".to_string())?;
    validate_port(port)?;
    let token = lines
        .next()
        .and_then(|line| line.strip_prefix("token="))
        .ok_or_else(|| "WebUI 凭据格式无效".to_string())?;
    if token.len() != 64
        || !token
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        || lines.next().is_some()
    {
        return Err("WebUI 凭据格式无效".into());
    }
    Ok(Session {
        pid,
        port,
        token: token.into(),
    })
}

fn read_protected(state: &Path, expected_uid: u32) -> Result<Session> {
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(O_NOFOLLOW)
        .open(state.join(SESSION_FILE))
        .map_err(|_| "WebUI 凭据不可读取，请确认守护已启动".to_string())?;
    let metadata = file
        .metadata()
        .map_err(|_| "无法核对 WebUI 凭据权限".to_string())?;
    if !metadata.is_file() || metadata.uid() != expected_uid || metadata.mode() & 0o7777 != 0o600 {
        return Err("WebUI 凭据所有者或权限不安全".into());
    }
    if metadata.len() > 256 {
        return Err("WebUI 凭据格式无效".into());
    }
    let mut text = String::new();
    (&mut file)
        .take(256)
        .read_to_string(&mut text)
        .map_err(|_| "无法读取 WebUI 凭据".to_string())?;
    parse(&text)
}

fn belongs_to(session: &Session, status: &str) -> Result<()> {
    if status
        .lines()
        .any(|line| line == format!("pid={}", session.pid))
    {
        Ok(())
    } else {
        Err("WebUI 凭据不属于当前守护".into())
    }
}

pub fn print_current(module: &Path, origin: &str) -> Result<()> {
    let session = current(module, origin)?;
    let reply = format!(
        "{{\"port\":{},\"token\":\"{}\",\"origin\":\"{}\"}}\n",
        session.port, session.token, origin
    );
    std::io::stdout()
        .write_all(reply.as_bytes())
        .map_err(|_| "输出 WebUI 连接凭据失败".into())
}

pub(crate) fn current(module: &Path, origin: &str) -> Result<Session> {
    if unsafe { crate::ffi::geteuid() } != 0 {
        return Err("读取 WebUI 连接凭据需要 root".into());
    }
    let status = crate::daemon::health(module).map_err(|error| {
        crate::lifecycle::connection_failure(Path::new(STATE_DIR), module, &error)
    })?;
    let session = read_protected(Path::new(STATE_DIR), 0)?;
    belongs_to(&session, &status)?;
    if !valid_origin(origin) {
        return Err("WebUI 页面来源不是允许的 HTTPS 或本机 HTTP 来源".into());
    }
    register_origin(Path::new(STATE_DIR), origin)?;
    // Recheck process identity, lock and heartbeat after reading the credential.
    belongs_to(&session, &crate::daemon::health(module)?)?;
    Ok(session)
}

pub fn valid_origin(origin: &str) -> bool {
    if origin.is_empty()
        || origin.len() > 512
        || !origin.is_ascii()
        || origin
            .bytes()
            .any(|b| b.is_ascii_control() || b.is_ascii_whitespace())
    {
        return false;
    }
    let (https, rest) = if let Some(rest) = origin.strip_prefix("https://") {
        (true, rest)
    } else if let Some(rest) = origin.strip_prefix("http://") {
        (false, rest)
    } else {
        return false;
    };
    if rest.is_empty()
        || rest
            .bytes()
            .any(|b| matches!(b, b'/' | b'?' | b'#' | b'@' | b'%' | b'\\' | b'\'' | b'"'))
    {
        return false;
    }
    let (host, port) = if let Some(bracketed) = rest.strip_prefix('[') {
        let Some((literal, suffix)) = bracketed.split_once(']') else {
            return false;
        };
        let Ok(ip) = literal.parse::<Ipv6Addr>() else {
            return false;
        };
        let port = if suffix.is_empty() {
            None
        } else {
            let Some(p) = suffix.strip_prefix(':') else {
                return false;
            };
            Some(p)
        };
        if !https && !ip.is_loopback() {
            return false;
        }
        return valid_port(port);
    } else {
        if rest.matches(':').count() > 1 {
            return false;
        }
        match rest.rsplit_once(':') {
            Some((h, p)) => (h, Some(p)),
            None => (rest, None),
        }
    };
    if !valid_port(port) || host.is_empty() || host.len() > 253 {
        return false;
    }
    if let Ok(ip) = host.parse::<Ipv4Addr>() {
        return https || ip.is_loopback();
    }
    if !host
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-')
    {
        return false;
    }
    if !https && host != "localhost" && !host.ends_with(".localhost") {
        return false;
    }
    host.split('.').all(|label| {
        !label.is_empty()
            && label.len() <= 63
            && label.as_bytes()[0] != b'-'
            && label.as_bytes()[label.len() - 1] != b'-'
    })
}

fn valid_port(port: Option<&str>) -> bool {
    port.map_or(true, |value| {
        !value.is_empty()
            && value.bytes().all(|b| b.is_ascii_digit())
            && value.parse::<u16>().is_ok_and(|p| p > 0)
    })
}

fn register_origin(state: &Path, origin: &str) -> Result<()> {
    register_origin_owned(state, origin, 0)
}

fn register_origin_owned(state: &Path, origin: &str, expected_uid: u32) -> Result<()> {
    if origin == DEFAULT_ORIGIN {
        return Ok(());
    }
    let _lock = util::acquire_flock(&state.join(ORIGINS_LOCK))?;
    let mut origins = read_origins(state, expected_uid)?;
    if origins.iter().any(|item| item.as_str() == origin) {
        return Ok(());
    }
    if !origins.iter().any(|item| item.as_str() == origin) {
        if origins.len() >= MAX_ORIGINS {
            origins.remove(1);
        }
        origins.push(origin.to_string());
    }
    let mut contents = origins.join("\n");
    contents.push('\n');
    util::atomic_write(&state.join(ORIGINS_FILE), contents.as_bytes(), 0o600)
        .map_err(|_| "无法安全登记 WebUI 页面来源".into())
}

pub fn origin_is_allowed(origin: &str) -> bool {
    origin_is_allowed_in(Path::new(STATE_DIR), origin, 0)
}

fn origin_is_allowed_in(state: &Path, origin: &str, expected_uid: u32) -> bool {
    if origin == DEFAULT_ORIGIN {
        return true;
    }
    if !valid_origin(origin) {
        return false;
    }
    read_origins(state, expected_uid)
        .is_ok_and(|origins| origins.iter().any(|item| item.as_str() == origin))
}

fn read_origins(state: &Path, expected_uid: u32) -> Result<Vec<String>> {
    let path = state.join(ORIGINS_FILE);
    let mut file = match OpenOptions::new()
        .read(true)
        .custom_flags(O_NOFOLLOW)
        .open(&path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(vec![DEFAULT_ORIGIN.into()])
        }
        Err(_) => return Err("WebUI 来源白名单不可安全读取".into()),
    };
    let meta = file
        .metadata()
        .map_err(|_| "无法核对白名单权限".to_string())?;
    if !meta.is_file()
        || meta.uid() != expected_uid
        || meta.mode() & 0o7777 != 0o600
        || meta.len() > 4096
    {
        return Err("WebUI 来源白名单权限不安全".into());
    }
    let mut text = String::new();
    (&mut file)
        .take(4096)
        .read_to_string(&mut text)
        .map_err(|_| "读取 WebUI 来源白名单失败".to_string())?;
    let mut origins: Vec<String> = Vec::new();
    for line in text.lines() {
        if !valid_origin(line)
            || origins.iter().any(|item| item.as_str() == line)
            || origins.len() >= MAX_ORIGINS
        {
            return Err("WebUI 来源白名单内容无效".into());
        }
        origins.push(line.to_string());
    }
    if !origins.iter().any(|item| item.as_str() == DEFAULT_ORIGIN) {
        if origins.len() >= MAX_ORIGINS {
            return Err("WebUI 来源白名单超出上限".into());
        }
        origins.insert(0, DEFAULT_ORIGIN.into());
    }
    Ok(origins)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn fixture() -> std::path::PathBuf {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "nova-auth-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }
    fn session() -> Session {
        Session {
            pid: 123,
            port: 31421,
            token: "a".repeat(64),
        }
    }

    #[test]
    fn uses_all_256_random_bits_and_fails_on_incomplete_entropy() {
        let bytes: Vec<u8> = (0..32).collect();
        assert_eq!(
            random_token(&mut bytes.as_slice()).unwrap(),
            "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f"
        );
        assert!(random_token(&mut &[0u8; 31][..]).is_err());
    }
    #[test]
    fn correct_token_required_and_previous_session_cannot_authenticate() {
        let old = session();
        assert!(old.accepts(&format!("{AUTH_PREFIX}{}", old.token)));
        assert!(!old.accepts(&format!("{AUTH_PREFIX}{}", "b".repeat(64))));
        assert!(!old.accepts(&format!("{AUTH_PREFIX}{}", "a".repeat(63))));
        let next = Session {
            token: "b".repeat(64),
            ..session()
        };
        assert!(!next.accepts(&format!("{AUTH_PREFIX}{}", old.token)));
    }
    #[test]
    fn malformed_or_extra_credential_fields_never_become_shell_or_json_input() {
        for text in [
            "pid=0\nport=31415\ntoken=a",
            "pid=123\nport=80\ntoken=a",
            "pid=123\nport=31415\ntoken=\"\n",
            "token=a\nport=31415\npid=123",
        ] {
            assert!(parse(text).is_err());
        }
        let text = format!("pid=123\nport=31415\ntoken={}\n", "a".repeat(64));
        assert!(parse(&text).is_ok());
        assert!(parse(&(text + "token=extra\n")).is_err());
    }
    #[test]
    fn persisted_session_is_owner_only_and_unsafe_permissions_are_rejected() {
        let dir = fixture();
        session().persist(&dir).unwrap();
        let path = dir.join(SESSION_FILE);
        assert_eq!(fs::metadata(&path).unwrap().mode() & 0o7777, 0o600);
        let uid = unsafe { crate::ffi::geteuid() };
        assert_eq!(read_protected(&dir, uid).unwrap().port, 31421);
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(read_protected(&dir, uid).is_err());
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn symlink_and_wrong_owner_are_rejected_without_printing_contents() {
        let dir = fixture();
        session().persist(&dir).unwrap();
        let uid = unsafe { crate::ffi::geteuid() };
        assert!(read_protected(&dir, uid.wrapping_add(1)).is_err());
        fs::rename(dir.join(SESSION_FILE), dir.join("saved")).unwrap();
        std::os::unix::fs::symlink(dir.join("saved"), dir.join(SESSION_FILE)).unwrap();
        assert!(read_protected(&dir, uid).is_err());
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn stale_credentials_are_not_released_for_another_daemon_pid() {
        assert!(belongs_to(&session(), "pid=123\nmode=balance\n").is_ok());
        assert!(belongs_to(&session(), "pid=124\nmode=balance\n").is_err());
    }
    #[test]
    fn credential_write_failure_does_not_fall_back_to_public_protocol() {
        let dir = fixture();
        fs::create_dir(dir.join(SESSION_FILE)).unwrap();
        assert!(session().persist(&dir).is_err());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn new_random_session_replaces_file_and_invalidates_previous_credential() {
        let dir = fixture();
        let mut source = File::open("/dev/urandom").unwrap();
        let old = Session {
            token: random_token(&mut source).unwrap(),
            ..session()
        };
        old.persist(&dir).unwrap();
        let next = Session {
            token: random_token(&mut source).unwrap(),
            ..session()
        };
        next.persist(&dir).unwrap();
        assert_ne!(old.token, next.token);
        assert!(!next.accepts(&format!("{AUTH_PREFIX}{}", old.token)));
        let saved = read_protected(&dir, unsafe { crate::ffi::geteuid() }).unwrap();
        assert_eq!(saved.token, next.token);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn origin_parser_accepts_exact_https_and_loopback_http_only() {
        for origin in [
            "https://mui.kernelsu.org",
            "https://mmrl.local",
            "https://example.com:9443",
            "https://[2001:db8::1]:443",
            "http://127.0.0.1:8090",
            "http://localhost",
            "http://[::1]:80",
        ] {
            assert!(valid_origin(origin), "{origin}");
        }
        for origin in [
            "",
            "null",
            "file://",
            "http://evil.example",
            "http://192.168.1.10",
            "https://user@example.com",
            "https://example.com/path",
            "https://example.com?x=1",
            "https://example.com#x",
            "https://-example.com",
            "https://bad..example",
            "https://example.com:0",
            "https://example.com:99999",
            "http://[2001:db8::1]",
        ] {
            assert!(!valid_origin(origin), "{origin}");
        }
    }

    #[test]
    fn custom_origin_registry_is_exact_bounded_root_owned_and_resettable() {
        let dir = fixture();
        let uid = unsafe { crate::ffi::geteuid() };
        util::atomic_write(
            &dir.join(ORIGINS_FILE),
            format!("{DEFAULT_ORIGIN}\n").as_bytes(),
            0o600,
        )
        .unwrap();
        assert!(origin_is_allowed_in(&dir, DEFAULT_ORIGIN, uid));
        let custom = "https://mmrl.custom.example";
        register_origin_owned(&dir, custom, uid).unwrap();
        assert!(origin_is_allowed_in(&dir, custom, uid));
        assert!(!origin_is_allowed_in(&dir, "https://evil.example", uid));
        assert!(!origin_is_allowed_in(&dir, custom, uid.wrapping_add(1)));
        for i in 0..10 {
            register_origin_owned(&dir, &format!("https://ui-{i}.example"), uid).unwrap();
        }
        assert_eq!(read_origins(&dir, uid).unwrap().len(), MAX_ORIGINS);
        assert!(!origin_is_allowed_in(&dir, custom, uid));
        util::atomic_write(
            &dir.join(ORIGINS_FILE),
            format!("{DEFAULT_ORIGIN}\n").as_bytes(),
            0o600,
        )
        .unwrap();
        assert!(!origin_is_allowed_in(&dir, "https://ui-9.example", uid));
        register_origin_owned(&dir, custom, uid).unwrap();
        fs::set_permissions(dir.join(ORIGINS_FILE), fs::Permissions::from_mode(0o644)).unwrap();
        assert!(!origin_is_allowed_in(&dir, custom, uid));
        fs::remove_dir_all(dir).unwrap();
    }
}
