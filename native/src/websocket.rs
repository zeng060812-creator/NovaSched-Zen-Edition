use std::fs;
use std::io::{ErrorKind, Read, Seek, SeekFrom, Write};
use std::net::{TcpListener, TcpStream};
use std::os::fd::AsRawFd;
use std::os::unix::fs::MetadataExt;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crate::app_modes::{self, AppModes};
use crate::logging::Logger;
use crate::options::Options;
use crate::util::Result;
use crate::web_auth::{self, Session};

const WS_KEY: &str = "!TcsEUQ#Be8bk4bk!dcf341Bo4NwXdQi8hik2_l3BXOJ2$hQOeHKDAUL1jrQUnkx";
const WS_GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";
// The stock manager origin is always trusted; custom exact origins are added
// only by the root credential command after validating the browser origin.
// The public WS_KEY identifies the protocol; it is not an authentication secret.
const TRUSTED_WEBUI_ORIGINS: &[&str] = &["https://mui.kernelsu.org"];
const PORT_START: u16 = 31415;
const PORT_END: u16 = 31425;

#[derive(Clone, Default)]
pub struct RuntimeState {
    inner: Arc<Mutex<RuntimeData>>,
}

#[derive(Clone, Default)]
struct RuntimeData {
    package: String,
    effective_mode: String,
    controller: String,
    scene_active: bool,
    scene_linked: bool,
    phase: String,
    error: String,
    port: u16,
    tick: Option<Instant>,
    power_profile: String,
    soc: String,
    soc_id: String,
    config_profile: String,
    extreme_supported: bool,
    smooth_supported: bool,
}

impl RuntimeState {
    pub fn set_profile(&self, soc: crate::soc::Soc, extreme: bool, smooth: bool) {
        if let Ok(mut data) = self.inner.lock() {
            data.soc = soc.name().into();
            data.soc_id = soc.id().into();
            data.config_profile = soc.file().into();
            data.extreme_supported = extreme;
            data.smooth_supported = smooth;
        }
    }
    pub fn set_power_profile(&self, profile: &str) {
        if let Ok(mut data) = self.inner.lock() {
            data.power_profile = profile.into();
        }
    }
    pub fn tick(&self, phase: &str, error: &str) {
        if let Ok(mut data) = self.inner.lock() {
            data.phase = phase.into();
            data.error = error.into();
            data.tick = Some(Instant::now());
        }
    }
    pub fn set(
        &self,
        package: &str,
        mode: &str,
        controller: &str,
        scene_active: bool,
        scene_linked: bool,
    ) {
        if let Ok(mut guard) = self.inner.lock() {
            guard.package = package.to_string();
            guard.effective_mode = mode.to_string();
            guard.controller = controller.to_string();
            guard.scene_active = scene_active;
            guard.scene_linked = scene_linked;
        }
    }

    fn get(&self) -> RuntimeData {
        self.inner.lock().map(|v| v.clone()).unwrap_or_default()
    }

    fn webui_allows_message(&self, _message: &str) -> bool {
        self.inner
            .lock()
            .map(|v| {
                // A foreign scheduler owns the node writers, so nothing flows.
                // Linked mode is two-way: WebUI edits reach Scene's
                // powercfg.xml through AppModes.sync_scene_rules, while
                // Scene's own switches return through the watched XML and
                // the scene-mode request file. Last writer wins on both ends.
                !v.scene_active
            })
            .unwrap_or(false)
    }

    pub fn set_port(&self, port: u16) {
        if let Ok(mut data) = self.inner.lock() {
            data.port = port;
        }
    }
}

#[derive(Clone)]
pub struct WebState {
    pub app_modes: AppModes,
    pub options: Options,
    pub runtime: RuntimeState,
    pub revision: Arc<AtomicU64>,
}

impl WebState {
    pub fn changed(&self) {
        self.revision.fetch_add(1, Ordering::Relaxed);
    }
}

pub fn start(
    state: WebState,
    logger: Logger,
    stop: Arc<AtomicBool>,
) -> Result<(thread::JoinHandle<()>, u16)> {
    let (listener, port) = bind_listener()?;
    let session = Arc::new(Session::rotate(
        std::path::Path::new(crate::util::STATE_DIR),
        port,
    )?);
    listener
        .set_nonblocking(true)
        .map_err(|e| format!("WebUI 设置非阻塞失败: {e}"))?;
    let clients = Arc::new(AtomicUsize::new(0));
    let handle = thread::Builder::new()
        .name("cts-websocket".to_string())
        .spawn(move || {
            logger.info(format!("WebUI 服务已监听 127.0.0.1:{port}"));
            while !stop.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        if clients.load(Ordering::Relaxed) >= 16 {
                            logger.warn("WebUI 连接达到 16 个上限");
                            continue;
                        }
                        let client_state = state.clone();
                        let client_logger = logger.clone();
                        let client_stop = stop.clone();
                        let client_session = session.clone();
                        clients.fetch_add(1, Ordering::Relaxed);
                        let counter = ClientCount(clients.clone());
                        if let Err(error) = thread::Builder::new()
                            .name("novasched-web-client".to_string())
                            .spawn(move || {
                                let _counter = counter;
                                if let Err(error) = handle_connection(
                                    stream,
                                    client_state,
                                    &client_logger,
                                    client_stop,
                                    &client_session,
                                ) {
                                    client_logger.debug(format!("WebUI 客户端断开: {error}"));
                                }
                            })
                        {
                            logger.error(format!("创建 WebUI 客户线程失败: {error}"));
                        }
                    }
                    Err(error) if error.kind() == ErrorKind::WouldBlock => {
                        let mut fd = crate::ffi::PollFd {
                            fd: listener.as_raw_fd(),
                            events: crate::ffi::POLLIN,
                            revents: 0,
                        };
                        if unsafe { crate::ffi::poll(&mut fd, 1, 1000) } < 0
                            && std::io::Error::last_os_error().kind() != ErrorKind::Interrupted
                        {
                            thread::sleep(Duration::from_secs(1));
                        }
                    }
                    Err(error) => {
                        logger.error(format!("WebUI accept 失败: {error}"));
                        thread::sleep(Duration::from_millis(500));
                    }
                }
            }
        })
        .map_err(|e| format!("创建 WebUI 线程失败: {e}"))?;
    Ok((handle, port))
}

fn bind_listener() -> Result<(TcpListener, u16)> {
    bind_listener_range(PORT_START, PORT_END)
}

fn bind_listener_range(start: u16, end: u16) -> Result<(TcpListener, u16)> {
    let mut failures = Vec::new();
    for port in start..=end {
        match TcpListener::bind(("127.0.0.1", port)) {
            Ok(listener) => return Ok((listener, port)),
            Err(error) => failures.push(format!("{port}: {error}")),
        }
    }
    // A busy preferred range must not prevent the scheduler from starting.
    // The root credential command supplies this exact port to both transports.
    if let Ok(listener) = TcpListener::bind(("127.0.0.1", 0)) {
        let port = listener.local_addr().map_err(|e| e.to_string())?.port();
        if port >= 1024 {
            return Ok((listener, port));
        }
    }
    Err(format!(
        "WebUI 无法绑定备用端口 {start}-{end} 或系统分配端口: {}",
        failures.join("; ")
    ))
}

struct ClientCount(Arc<AtomicUsize>);
impl Drop for ClientCount {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Relaxed);
    }
}

fn handle_connection(
    mut stream: TcpStream,
    state: WebState,
    logger: &Logger,
    stop: Arc<AtomicBool>,
    session: &Session,
) -> Result<()> {
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .map_err(|e| e.to_string())?;
    stream
        .set_write_timeout(Some(Duration::from_secs(5)))
        .map_err(|e| e.to_string())?;
    let path = websocket_handshake(&mut stream, session)?;
    logger.debug(format!("WebUI 已连接 {path}"));
    match path.as_str() {
        "/logs" => handle_logs(stream, logger, stop),
        "/modes" => handle_state(stream, state, false, stop),
        "/app-modes" => handle_state(stream, state, true, stop),
        _ => Ok(()),
    }
}

fn websocket_handshake(stream: &mut TcpStream, session: &Session) -> Result<String> {
    websocket_handshake_allow(stream, session, web_auth::origin_is_allowed)
}

fn websocket_handshake_allow(
    stream: &mut TcpStream,
    session: &Session,
    origin_allowed: impl Fn(&str) -> bool,
) -> Result<String> {
    let request = read_http_headers(stream)?;
    let mut lines = request.lines();
    let request_line = lines
        .next()
        .ok_or_else(|| "WebSocket 请求为空".to_string())?;
    let request_parts: Vec<_> = request_line.split_whitespace().collect();
    let get =
        request_parts.len() == 3 && request_parts[0] == "GET" && request_parts[2] == "HTTP/1.1";
    let path = request_line
        .split_whitespace()
        .nth(1)
        .unwrap_or("")
        .split(['?', '#'])
        .next()
        .unwrap_or("");
    let mut key = String::new();
    let mut protocol_ok = false;
    let mut origin = None;
    let mut origin_count = 0;
    let mut auth_count = 0;
    let mut authenticated = false;
    let mut upgrade = false;
    let mut connection = false;
    let mut version_count = 0;
    let mut version_ok = false;
    let mut key_count = 0;
    for line in lines {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        if name.eq_ignore_ascii_case("Sec-WebSocket-Key") {
            key_count += 1;
            key = value.trim().to_string();
        }
        if name.eq_ignore_ascii_case("Upgrade") {
            upgrade = value.trim().eq_ignore_ascii_case("websocket");
        }
        if name.eq_ignore_ascii_case("Connection") {
            connection = value
                .split(',')
                .any(|v| v.trim().eq_ignore_ascii_case("upgrade"));
        }
        if name.eq_ignore_ascii_case("Sec-WebSocket-Version") {
            version_count += 1;
            version_ok = value.trim() == "13";
        }
        if name.eq_ignore_ascii_case("Sec-WebSocket-Protocol") {
            for item in value.split(',').map(str::trim) {
                protocol_ok |= item == WS_KEY;
                if item.starts_with(web_auth::AUTH_PREFIX) {
                    auth_count += 1;
                    authenticated |= session.accepts(item);
                }
            }
        }
        if name.eq_ignore_ascii_case("Origin") {
            origin_count += 1;
            // HTTP optional whitespace is SP / HTAB only, not arbitrary Unicode.
            origin = Some(value.trim_matches([' ', '\t']));
        }
    }
    // Reject before sending 101, returning state/logs, or dispatching commands.
    // Missing, opaque (null), combined, and duplicate origins all fail closed.
    if origin_count != 1
        || auth_count != 1
        || !authenticated
        || !origin
            .is_some_and(|value| TRUSTED_WEBUI_ORIGINS.contains(&value) || origin_allowed(value))
    {
        let body = "NovaSched: WebSocket authorization rejected\n";
        let response = format!(
            "HTTP/1.1 403 Forbidden\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        stream
            .write_all(response.as_bytes())
            .map_err(|e| e.to_string())?;
        return Err("WebSocket Origin 或令牌校验失败".into());
    }
    let valid_key = key.len() == 24
        && key.ends_with("==")
        && key.as_bytes()[..22]
            .iter()
            .all(|b| b.is_ascii_alphanumeric() || *b == b'+' || *b == b'/');
    if !get
        || !upgrade
        || !connection
        || version_count != 1
        || !version_ok
        || key_count != 1
        || !valid_key
        || !protocol_ok
        || !matches!(path, "/logs" | "/modes" | "/app-modes")
    {
        let response = "HTTP/1.1 426 Upgrade Required\r\nContent-Type: text/plain; charset=utf-8\r\nConnection: close\r\n\r\nNovaSched: 需要有效的 WebSocket 子协议\r\n";
        stream
            .write_all(response.as_bytes())
            .map_err(|e| e.to_string())?;
        return Err(format!("握手校验失败，路径={path}"));
    }
    let accept = base64(&sha1(format!("{key}{WS_GUID}").as_bytes()));
    let response = format!(
        "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {accept}\r\nSec-WebSocket-Protocol: {WS_KEY}\r\n\r\n"
    );
    stream
        .write_all(response.as_bytes())
        .map_err(|e| format!("发送握手失败: {e}"))?;
    Ok(path.to_string())
}

fn handle_logs(mut stream: TcpStream, logger: &Logger, stop: Arc<AtomicBool>) -> Result<()> {
    stream
        .set_read_timeout(Some(Duration::from_millis(300)))
        .map_err(|e| e.to_string())?;
    let mut position = 0;
    let mut inode = 0;
    let mut heartbeat = Instant::now();
    let mut last_seen = Instant::now();
    while !stop.load(Ordering::Relaxed) {
        let mut file = fs::File::open(logger.persistent_path())
            .map_err(|e| format!("读取内部日志失败: {e}"))?;
        let meta = file.metadata().map_err(|e| e.to_string())?;
        if inode != meta.ino() || meta.len() < position {
            position = meta.len().saturating_sub(32768);
            inode = meta.ino();
        }
        file.seek(SeekFrom::Start(position))
            .map_err(|e| e.to_string())?;
        let mut bytes = Vec::new();
        file.take(16384)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        if !bytes.is_empty() {
            position += bytes.len() as u64;
            send_text(&mut stream, &String::from_utf8_lossy(&bytes))?;
        }
        let frame = read_frame(&mut stream)?;
        if !matches!(frame, Frame::Timeout) {
            last_seen = Instant::now();
        }
        if last_seen.elapsed() > Duration::from_secs(30) {
            return Err("日志客户端心跳超时".into());
        }
        match frame {
            Frame::Close => return Ok(()),
            Frame::Ping(bytes) => send_control(&mut stream, 0xA, &bytes)?,
            _ => {}
        }
        if heartbeat.elapsed() >= Duration::from_secs(2) {
            send_control(&mut stream, 0x9, b"cts")?;
            heartbeat = Instant::now();
        }
    }
    Ok(())
}

fn handle_state(
    mut stream: TcpStream,
    state: WebState,
    app_endpoint: bool,
    stop: Arc<AtomicBool>,
) -> Result<()> {
    stream
        .set_read_timeout(Some(Duration::from_millis(500)))
        .map_err(|e| e.to_string())?;
    let mut revision = state.revision.load(Ordering::Relaxed);
    send_text(&mut stream, &payload(&state, app_endpoint))?;
    let mut heartbeat = Instant::now();
    let mut last_seen = Instant::now();
    while !stop.load(Ordering::Relaxed) {
        let frame = read_frame(&mut stream)?;
        if !matches!(frame, Frame::Timeout) {
            last_seen = Instant::now();
        }
        if last_seen.elapsed() > Duration::from_secs(30) {
            return Err("状态客户端心跳超时".into());
        }
        match frame {
            Frame::Text(message) => {
                if state.runtime.webui_allows_message(&message) {
                    if let Some(value) = message.strip_prefix("extreme\t") {
                        match value.trim() {
                            "0" => state.options.set_extreme_powersave(false)?,
                            "1" => state.options.set_extreme_powersave(true)?,
                            _ => return Err("extreme 选项只接受 0 或 1".into()),
                        }
                    } else if let Some(value) = message.strip_prefix("smooth\t") {
                        match value.trim() {
                            "0" => state.options.set_smooth_powersave(false)?,
                            "1" => state.options.set_smooth_powersave(true)?,
                            _ => return Err("smooth 选项只接受 0 或 1".into()),
                        }
                    } else if app_endpoint {
                        if let Some(rest) = message.strip_prefix("set\t") {
                            if let Some((package, mode)) = rest.split_once('\t') {
                                state.app_modes.set_rule(package, mode)?;
                            }
                        } else if let Some(package) = message.strip_prefix("delete\t") {
                            state.app_modes.remove_rule(package)?;
                        }
                    } else if app_modes::supported(message.trim()) {
                        state.app_modes.set_rule("*", message.trim())?;
                    }
                }
                state.changed();
                revision = state.revision.load(Ordering::Relaxed);
                send_text(&mut stream, &payload(&state, app_endpoint))?;
            }
            Frame::Ping(data) => send_control(&mut stream, 0xA, &data)?,
            Frame::Close => return Ok(()),
            Frame::Timeout | Frame::Ignore => {}
        }
        let current = state.revision.load(Ordering::Relaxed);
        if current != revision || heartbeat.elapsed() >= Duration::from_secs(2) {
            revision = current;
            send_text(&mut stream, &payload(&state, app_endpoint))?;
            send_control(&mut stream, 0x9, b"cts")?;
            heartbeat = Instant::now();
        }
    }
    Ok(())
}

fn payload(state: &WebState, app_endpoint: bool) -> String {
    let runtime = state.runtime.get();
    let default = if runtime.scene_active && !runtime.effective_mode.is_empty() {
        runtime.effective_mode.clone()
    } else {
        state.app_modes.default_mode()
    };
    if !app_endpoint {
        return format!(
            "mode:{},effective:{},package:{},controller:{},sceneActive:{},sceneLinked:{},phase:{},heartbeatMs:{},port:{},extremePowerSave:{},smoothPowerSave:{},powerSaveProfile:{},socName:{},socId:{},configProfile:{},extremeSupported:{},smoothSupported:{}",
            default,
            runtime.effective_mode,
            runtime.package,
            runtime.controller,
            runtime.scene_active,
            runtime.scene_linked,
            runtime.phase,runtime.tick.map(|t|t.elapsed().as_millis()).unwrap_or(u128::MAX),
            runtime.port,
            state.options.extreme_powersave(),
            state.options.smooth_powersave(), runtime.power_profile,
            runtime.soc, runtime.soc_id, runtime.config_profile, runtime.extreme_supported, runtime.smooth_supported,
        );
    }
    let mut rules = String::new();
    for (rule_package, mode) in state.app_modes.rules() {
        if rule_package == "*" {
            continue;
        }
        if !rules.is_empty() {
            rules.push(',');
        }
        rules.push_str(&format!(
            "{{\"package\":\"{}\",\"mode\":\"{}\"}}",
            json_escape(&rule_package),
            json_escape(&mode)
        ));
    }
    format!(
        "{{\"type\":\"app-modes\",\"defaultMode\":\"{}\",\"currentPackage\":\"{}\",\"effectiveMode\":\"{}\",\"controller\":\"{}\",\"sceneAvailable\":{},\"sceneLinked\":{},\"locked\":{},\"rules\":[{}],\"revision\":{},\"phase\":\"{}\",\"error\":\"{}\",\"version\":\"{}\",\"port\":{},\"extremePowerSave\":{},\"smoothPowerSave\":{},\"powerSaveProfile\":\"{}\",\"socName\":\"{}\",\"socId\":\"{}\",\"configProfile\":\"{}\",\"extremeSupported\":{},\"smoothSupported\":{}}}",
        json_escape(&default), json_escape(&runtime.package), json_escape(&runtime.effective_mode), json_escape(&runtime.controller),
        runtime.scene_active || runtime.scene_linked, runtime.scene_linked, runtime.scene_active, rules,
        state.revision.load(Ordering::Relaxed),
        json_escape(&runtime.phase),json_escape(&runtime.error),env!("CARGO_PKG_VERSION"),
        runtime.port,state.options.extreme_powersave(),state.options.smooth_powersave(),json_escape(&runtime.power_profile),
        json_escape(&runtime.soc),json_escape(&runtime.soc_id),json_escape(&runtime.config_profile),runtime.extreme_supported,runtime.smooth_supported,
    )
}

enum Frame {
    Text(String),
    Ping(Vec<u8>),
    Close,
    Timeout,
    Ignore,
}

fn read_frame(stream: &mut TcpStream) -> Result<Frame> {
    let mut header = [0u8; 2];
    match stream.read_exact(&mut header[..1]) {
        Ok(()) => {}
        Err(error) if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {
            return Ok(Frame::Timeout)
        }
        Err(error) => return Err(format!("读取 WebSocket 帧头失败: {error}")),
    }
    let opcode = header[0] & 0x0f;
    stream
        .read_exact(&mut header[1..])
        .map_err(|e| format!("帧头不完整: {e}"))?;
    let masked = header[1] & 0x80 != 0;
    if header[0] & 0x80 == 0 || header[0] & 0x70 != 0 || !masked {
        return Err("拒绝分片、扩展或未掩码客户端帧".into());
    }
    let mut length = (header[1] & 0x7f) as u64;
    if length == 126 {
        let mut ext = [0u8; 2];
        stream.read_exact(&mut ext).map_err(|e| e.to_string())?;
        length = u16::from_be_bytes(ext) as u64;
    } else if length == 127 {
        let mut ext = [0u8; 8];
        stream.read_exact(&mut ext).map_err(|e| e.to_string())?;
        length = u64::from_be_bytes(ext);
    }
    if length > 1024 * 1024 {
        return Err("WebSocket 帧超过 1 MiB".to_string());
    }
    if opcode >= 8 && length > 125 {
        return Err("控制帧超长".into());
    }
    let mut mask = [0u8; 4];
    if masked {
        stream.read_exact(&mut mask).map_err(|e| e.to_string())?;
    }
    let mut data = vec![0u8; length as usize];
    stream
        .read_exact(&mut data)
        .map_err(|e| format!("读取 WebSocket 载荷失败: {e}"))?;
    if masked {
        for (index, byte) in data.iter_mut().enumerate() {
            *byte ^= mask[index % 4];
        }
    }
    match opcode {
        0x1 => String::from_utf8(data)
            .map(Frame::Text)
            .map_err(|_| "WebSocket 文本不是 UTF-8".to_string()),
        0x8 => Ok(Frame::Close),
        0x9 => Ok(Frame::Ping(data)),
        _ => Ok(Frame::Ignore),
    }
}

fn send_text(stream: &mut TcpStream, text: &str) -> Result<()> {
    send_frame(stream, text.as_bytes())
}

fn send_frame(stream: &mut TcpStream, data: &[u8]) -> Result<()> {
    if data.len() > 65_535 {
        return Err("WebSocket 单帧过大".to_string());
    }
    let mut header = vec![0x81];
    if data.len() < 126 {
        header.push(data.len() as u8);
    } else {
        header.push(126);
        header.extend_from_slice(&(data.len() as u16).to_be_bytes());
    }
    stream
        .write_all(&header)
        .and_then(|_| stream.write_all(data))
        .map_err(|e| format!("发送 WebSocket 帧失败: {e}"))
}

fn send_control(stream: &mut TcpStream, opcode: u8, data: &[u8]) -> Result<()> {
    if data.len() > 125 {
        return Err("WebSocket 控制帧过大".to_string());
    }
    stream
        .write_all(&[0x80 | opcode, data.len() as u8])
        .and_then(|_| stream.write_all(data))
        .map_err(|e| e.to_string())
}

fn read_http_headers(stream: &mut TcpStream) -> Result<String> {
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut data = Vec::with_capacity(1024);
    let mut byte = [0u8; 1];
    while data.len() < 8192 {
        if Instant::now() >= deadline {
            return Err("WebSocket 握手超时".into());
        }
        stream
            .read_exact(&mut byte)
            .map_err(|e| format!("读取 HTTP 握手失败: {e}"))?;
        data.push(byte[0]);
        if data.ends_with(b"\r\n\r\n") || data.ends_with(b"\n\n") {
            break;
        }
    }
    if data.len() >= 8192 {
        return Err("HTTP 握手头过大".to_string());
    }
    String::from_utf8(data).map_err(|_| "HTTP 握手不是 UTF-8".to_string())
}

pub(crate) fn json_escape(value: &str) -> String {
    let mut out = String::new();
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if !c.is_control() => out.push(c),
            _ => {}
        }
    }
    out
}

fn sha1(input: &[u8]) -> [u8; 20] {
    let mut message = input.to_vec();
    let bit_len = (message.len() as u64) * 8;
    message.push(0x80);
    while message.len() % 64 != 56 {
        message.push(0);
    }
    message.extend_from_slice(&bit_len.to_be_bytes());
    let mut h = [
        0x67452301u32,
        0xEFCDAB89,
        0x98BADCFE,
        0x10325476,
        0xC3D2E1F0,
    ];
    for chunk in message.chunks_exact(64) {
        let mut w = [0u32; 80];
        for index in 0..16 {
            w[index] = u32::from_be_bytes([
                chunk[index * 4],
                chunk[index * 4 + 1],
                chunk[index * 4 + 2],
                chunk[index * 4 + 3],
            ]);
        }
        for index in 16..80 {
            w[index] = (w[index - 3] ^ w[index - 8] ^ w[index - 14] ^ w[index - 16]).rotate_left(1);
        }
        let (mut a, mut b, mut c, mut d, mut e) = (h[0], h[1], h[2], h[3], h[4]);
        for index in 0..80 {
            let (f, k) = match index {
                0..=19 => ((b & c) | ((!b) & d), 0x5A827999),
                20..=39 => (b ^ c ^ d, 0x6ED9EBA1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1BBCDC),
                _ => (b ^ c ^ d, 0xCA62C1D6),
            };
            let temp = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(w[index]);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = temp;
        }
        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
    }
    let mut out = [0u8; 20];
    for (index, word) in h.iter().enumerate() {
        out[index * 4..index * 4 + 4].copy_from_slice(&word.to_be_bytes());
    }
    out
}

fn base64(input: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in input.chunks(3) {
        let value = ((chunk[0] as u32) << 16)
            | ((chunk.get(1).copied().unwrap_or(0) as u32) << 8)
            | chunk.get(2).copied().unwrap_or(0) as u32;
        out.push(TABLE[((value >> 18) & 63) as usize] as char);
        out.push(TABLE[((value >> 12) & 63) as usize] as char);
        out.push(if chunk.len() > 1 {
            TABLE[((value >> 6) & 63) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            TABLE[(value & 63) as usize] as char
        } else {
            '='
        });
    }
    out
}

/// Local root bridge client. This traverses the same authenticated endpoint
/// and command handlers as the browser; it never writes policy files itself.
pub(crate) fn bridge_request(
    session: &Session,
    origin: &str,
    apps: bool,
    message: Option<&str>,
) -> Result<String> {
    let address = std::net::SocketAddr::from(([127, 0, 0, 1], session.port()));
    let mut stream = TcpStream::connect_timeout(&address, Duration::from_secs(2))
        .map_err(|e| format!("root 通道无法连接守护: {e}"))?;
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .map_err(|e| e.to_string())?;
    stream
        .set_write_timeout(Some(Duration::from_secs(5)))
        .map_err(|e| e.to_string())?;
    let mut nonce = [0u8; 16];
    fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut nonce))
        .map_err(|_| "root 通道无法读取随机源".to_string())?;
    let key = base64(&nonce);
    let path = if apps { "/app-modes" } else { "/modes" };
    let headers = format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: {key}\r\nOrigin: {origin}\r\nSec-WebSocket-Protocol: {WS_KEY}, {}\r\n\r\n", session.port(), session.protocol());
    stream
        .write_all(headers.as_bytes())
        .map_err(|_| "root 通道握手发送失败".to_string())?;
    let reply = read_http_headers(&mut stream)?;
    let accepted = base64(&sha1(format!("{key}{WS_GUID}").as_bytes()));
    if !reply
        .lines()
        .next()
        .is_some_and(|s| s.starts_with("HTTP/1.1 101 "))
        || !reply.lines().any(|s| {
            s.split_once(':').is_some_and(|(k, v)| {
                k.eq_ignore_ascii_case("sec-websocket-accept") && v.trim() == accepted
            })
        })
    {
        return Err("root 通道握手未获守护确认".into());
    }
    let initial = read_server_text(&mut stream)?;
    let result = if let Some(message) = message {
        send_masked(&mut stream, 0x1, message.as_bytes())?;
        read_server_text(&mut stream)?
    } else {
        initial
    };
    // Explicit close keeps the 16-client budget available to other hosts.
    send_masked(&mut stream, 0x8, &[])?;
    Ok(result)
}

fn send_masked(stream: &mut TcpStream, opcode: u8, bytes: &[u8]) -> Result<()> {
    if bytes.len() > 4096 || (opcode >= 8 && bytes.len() > 125) {
        return Err("root 通道请求超出限制".into());
    }
    let mut mask = [0u8; 4];
    fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut mask))
        .map_err(|_| "root 通道无法生成掩码".to_string())?;
    let mut frame = vec![0x80 | opcode];
    if bytes.len() < 126 {
        frame.push(0x80 | bytes.len() as u8);
    } else {
        frame.push(0x80 | 126);
        frame.extend_from_slice(&(bytes.len() as u16).to_be_bytes());
    }
    frame.extend_from_slice(&mask);
    frame.extend(bytes.iter().enumerate().map(|(i, b)| *b ^ mask[i % 4]));
    stream
        .write_all(&frame)
        .map_err(|e| format!("root 通道请求发送失败: {e}"))
}

fn read_server_text(stream: &mut TcpStream) -> Result<String> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err("root 通道等待回复超时".into());
        }
        stream
            .set_read_timeout(Some(remaining))
            .map_err(|e| e.to_string())?;
        let mut head = [0u8; 2];
        stream
            .read_exact(&mut head)
            .map_err(|e| format!("root 通道未取得回复: {e}"))?;
        if head[0] & 0xf0 != 0x80 || head[1] & 0x80 != 0 {
            return Err("root 通道回复帧格式无效".into());
        }
        let opcode = head[0] & 15;
        let mut length = (head[1] & 127) as u64;
        if length == 126 {
            let mut ext = [0u8; 2];
            stream.read_exact(&mut ext).map_err(|e| e.to_string())?;
            length = u16::from_be_bytes(ext) as u64;
        } else if length == 127 {
            let mut ext = [0u8; 8];
            stream.read_exact(&mut ext).map_err(|e| e.to_string())?;
            length = u64::from_be_bytes(ext);
        }
        if length > 1_048_576 || (opcode >= 8 && length > 125) {
            return Err("root 通道回复超出限制".into());
        }
        let mut bytes = vec![0u8; length as usize];
        stream.read_exact(&mut bytes).map_err(|e| e.to_string())?;
        match opcode {
            1 => return String::from_utf8(bytes).map_err(|_| "root 通道回复不是 UTF-8".into()),
            8 => return Err("守护关闭了 root 通道，请确认请求与授权".into()),
            9 => send_masked(stream, 10, &bytes)?,
            10 => {}
            _ => return Err("root 通道回复类型无效".into()),
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn linked_scene_syncs_webui_mode_and_rule_messages_while_foreign_blocks_all() {
        let runtime = super::RuntimeState::default();
        runtime.set("com.test.app", "balance", "Scene", false, true);
        for message in [
            "fast",
            "powersave",
            "set\tcom.test.app\tfast",
            "delete\tcom.test.app",
            "smooth\t1",
            "extreme\t0",
        ] {
            assert!(
                runtime.webui_allows_message(message),
                "linked must stay editable: {message}"
            );
        }
        runtime.set("com.test.app", "balance", "WebUI", false, false);
        assert!(runtime.webui_allows_message("fast"));
        assert!(runtime.webui_allows_message("set\tcom.test.app\tfast"));
        runtime.set("com.test.app", "", "Scene foreign", true, false);
        for message in ["smooth\t1", "fast", "set\tcom.test.app\tfast"] {
            assert!(!runtime.webui_allows_message(message));
        }
    }
    use super::*;

    #[test]
    fn websocket_accept_vector() {
        assert_eq!(
            base64(&sha1(
                b"dGhlIHNhbXBsZSBub25jZQ==258EAFA5-E914-47DA-95CA-C5AB0DC85B11"
            )),
            "s3pPLMBiTxaQ9kYGzzhZRbK+xOo="
        );
    }
}

#[cfg(test)]
mod regression_tests {
    use super::*;
    #[test]
    fn busy_preferred_range_uses_a_system_assigned_loopback_port() {
        let busy = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = busy.local_addr().unwrap().port();
        let (listener, actual) = bind_listener_range(port, port).unwrap();
        assert_ne!(actual, port);
        assert_eq!(
            listener.local_addr().unwrap().ip(),
            std::net::Ipv4Addr::LOCALHOST
        );
        assert!(actual >= 1024);
    }

    #[test]
    fn root_bridge_uses_authenticated_handshake_and_same_masked_wire_command() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let token = "a".repeat(64);
        let session = Session::for_test_at(port, &token);
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            assert_eq!(
                websocket_handshake_allow(
                    &mut stream,
                    &Session::for_test_at(port, &token),
                    |origin| origin == "https://mmrl.test"
                )
                .unwrap(),
                "/modes"
            );
            send_text(&mut stream, "initial").unwrap();
            match read_frame(&mut stream).unwrap() {
                Frame::Text(message) => assert_eq!(message, "fast"),
                _ => panic!("expected text"),
            }
            send_control(&mut stream, 9, b"root").unwrap();
            send_text(&mut stream, "saved").unwrap();
            assert!(matches!(read_frame(&mut stream).unwrap(), Frame::Ignore));
            assert!(matches!(read_frame(&mut stream).unwrap(), Frame::Close));
        });
        assert_eq!(
            bridge_request(&session, "https://mmrl.test", false, Some("fast")).unwrap(),
            "saved"
        );
        server.join().unwrap();
    }
    fn pair() -> (TcpStream, TcpStream) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let sender = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (receiver, _) = listener.accept().unwrap();
        receiver
            .set_read_timeout(Some(Duration::from_millis(40)))
            .unwrap();
        (sender, receiver)
    }

    fn handshake_response(
        path: &str,
        origin_headers: &str,
        protocol: &str,
    ) -> (Result<String>, String) {
        handshake_response_with_auth(
            path,
            origin_headers,
            protocol,
            Some(&format!("{}{}", web_auth::AUTH_PREFIX, "a".repeat(64))),
        )
    }

    fn handshake_response_with_auth(
        path: &str,
        origin_headers: &str,
        protocol: &str,
        auth: Option<&str>,
    ) -> (Result<String>, String) {
        handshake_response_using(
            path,
            origin_headers,
            protocol,
            auth,
            web_auth::origin_is_allowed,
        )
    }

    fn handshake_response_using(
        path: &str,
        origin_headers: &str,
        protocol: &str,
        auth: Option<&str>,
        origin_allowed: impl Fn(&str) -> bool,
    ) -> (Result<String>, String) {
        let (mut client, mut server) = pair();
        client
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        let protocols = auth
            .map(|value| format!("{protocol}, {value}"))
            .unwrap_or_else(|| protocol.into());
        let request = format!(
            "GET {path} HTTP/1.1\r\nHost: 127.0.0.1:31415\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Protocol: {protocols}\r\n{origin_headers}\r\n"
        );
        client.write_all(request.as_bytes()).unwrap();
        let result = websocket_handshake_allow(
            &mut server,
            &Session::for_test(&"a".repeat(64)),
            origin_allowed,
        );
        drop(server);
        let mut response = String::new();
        client.read_to_string(&mut response).unwrap();
        (result, response)
    }

    #[test]
    fn trusted_ksu_origin_upgrades_all_endpoints() {
        for path in ["/logs", "/modes", "/app-modes"] {
            let (result, response) =
                handshake_response(path, "Origin: https://mui.kernelsu.org\r\n", WS_KEY);
            assert_eq!(result.unwrap(), path);
            assert!(response.starts_with("HTTP/1.1 101 Switching Protocols\r\n"));
            assert!(response.contains("Sec-WebSocket-Accept: s3pPLMBiTxaQ9kYGzzhZRbK+xOo=\r\n"));
            assert!(!response.contains(web_auth::AUTH_PREFIX));
            assert!(!response.contains(&"a".repeat(64)));
        }
    }

    #[test]
    fn malformed_http_upgrade_is_rejected_despite_valid_origin_and_token() {
        let auth = format!("{}{}", web_auth::AUTH_PREFIX, "a".repeat(64));
        let valid = format!("GET /modes HTTP/1.1\r\nHost: 127.0.0.1\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nOrigin: https://mui.kernelsu.org\r\nSec-WebSocket-Protocol: {WS_KEY}, {auth}\r\n\r\n");
        for bad in [
            valid.replace("GET /", "POST /"),
            valid.replace("Upgrade: websocket\r\n", ""),
            valid.replace("Connection: Upgrade", "Connection: close"),
            valid.replace("Version: 13", "Version: 12"),
            valid.replace("dGhlIHNhbXBsZSBub25jZQ==", "bad"),
        ] {
            let (mut client, mut server) = pair();
            client.write_all(bad.as_bytes()).unwrap();
            assert!(websocket_handshake_allow(
                &mut server,
                &Session::for_test(&"a".repeat(64)),
                |_| true
            )
            .is_err());
            let response = read_http_headers(&mut client).unwrap();
            assert!(response.starts_with("HTTP/1.1 426"));
        }
    }

    #[test]
    fn root_registered_custom_origin_upgrades_and_unregistered_origin_stays_blocked() {
        let token = format!("{}{}", web_auth::AUTH_PREFIX, "a".repeat(64));
        let trusted = handshake_response_using(
            "/modes",
            "Origin: https://mmrl.custom.example\r\n",
            WS_KEY,
            Some(&token),
            |origin| origin == "https://mmrl.custom.example",
        );
        assert!(trusted.0.is_ok());
        assert!(trusted
            .1
            .starts_with("HTTP/1.1 101 Switching Protocols\r\n"));
        let rejected = handshake_response_using(
            "/modes",
            "Origin: https://attacker.example\r\n",
            WS_KEY,
            Some(&token),
            |origin| origin == "https://mmrl.custom.example",
        );
        assert!(rejected.0.is_err());
        assert!(rejected.1.starts_with("HTTP/1.1 403 Forbidden\r\n"));
    }

    #[test]
    fn untrusted_browser_origin_is_forbidden_even_with_public_protocol() {
        for path in ["/logs", "/modes", "/app-modes"] {
            let (result, response) =
                handshake_response(path, "Origin: https://evil.example\r\n", WS_KEY);
            assert!(result.is_err());
            assert!(response.starts_with("HTTP/1.1 403 Forbidden\r\n"));
            assert!(!response.contains("101 Switching Protocols"));
            assert!(!response.contains("Sec-WebSocket-Accept:"));
        }
    }

    #[test]
    fn missing_null_empty_and_non_ksu_origins_are_forbidden() {
        for origin in [
            "",
            "Origin: null\r\n",
            "Origin: \r\n",
            "Origin: file://\r\n",
            "Origin: http://localhost\r\n",
            "Origin: http://127.0.0.1:31415\r\n",
            "Origin: http://mui.kernelsu.org\r\n",
            "Origin: https://mui.kernelsu.org:444\r\n",
        ] {
            let (result, response) = handshake_response("/modes", origin, WS_KEY);
            assert!(result.is_err(), "accepted {origin:?}");
            assert!(
                response.starts_with("HTTP/1.1 403 Forbidden\r\n"),
                "{origin:?}"
            );
        }
    }

    #[test]
    fn origin_suffix_userinfo_path_and_combined_values_cannot_bypass_allowlist() {
        for origin in [
            "https://mui.kernelsu.org.evil.example",
            "https://evil.mui.kernelsu.org",
            "https://mui.kernelsu.org@evil.example",
            "https://evil.example@mui.kernelsu.org",
            "https://mui.kernelsu.org/",
            "https://mui.kernelsu.org?origin=trusted",
            "https://mui.kernelsu.org, https://evil.example",
            "https://mui.kernelsu.org https://evil.example",
            "https://mui.kernelsu.org\u{a0}",
        ] {
            let (result, response) =
                handshake_response("/app-modes", &format!("Origin: {origin}\r\n"), WS_KEY);
            assert!(result.is_err(), "accepted {origin:?}");
            assert!(response.starts_with("HTTP/1.1 403 Forbidden\r\n"));
        }
    }

    #[test]
    fn duplicate_origins_are_forbidden_regardless_of_order_or_case() {
        for headers in [
            "Origin: https://mui.kernelsu.org\r\norigin: https://evil.example\r\n",
            "Origin: https://evil.example\r\nORIGIN: https://mui.kernelsu.org\r\n",
            "Origin: https://mui.kernelsu.org\r\nOrigin: https://mui.kernelsu.org\r\n",
        ] {
            let (result, response) = handshake_response("/modes", headers, WS_KEY);
            assert!(result.is_err());
            assert!(response.starts_with("HTTP/1.1 403 Forbidden\r\n"));
        }
    }

    #[test]
    fn origin_header_name_is_case_insensitive_and_allows_only_http_whitespace() {
        let (result, response) = handshake_response(
            "/modes",
            "oRiGiN: \thttps://mui.kernelsu.org \t\r\n",
            WS_KEY,
        );
        assert_eq!(result.unwrap(), "/modes");
        assert!(response.starts_with("HTTP/1.1 101 Switching Protocols\r\n"));
    }

    #[test]
    fn referer_does_not_replace_origin_and_protocol_check_is_retained() {
        let (result, response) =
            handshake_response("/modes", "Referer: https://mui.kernelsu.org/\r\n", WS_KEY);
        assert!(result.is_err());
        assert!(response.starts_with("HTTP/1.1 403 Forbidden\r\n"));
        let (result, response) = handshake_response(
            "/modes",
            "Origin: https://mui.kernelsu.org\r\n",
            "wrong-protocol",
        );
        assert!(result.is_err());
        assert!(response.starts_with("HTTP/1.1 426 Upgrade Required\r\n"));
    }

    #[test]
    fn untrusted_upgrade_with_queued_mutation_closes_before_command_dispatch() {
        let (mut client, mut server) = pair();
        client
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        let request = format!(
            "GET /app-modes HTTP/1.1\r\nHost: 127.0.0.1:31415\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Protocol: {WS_KEY}, {}{}\r\nOrigin: https://evil.example\r\n\r\n", web_auth::AUTH_PREFIX, "a".repeat(64)
        );
        let command = b"set\tcom.example.game\tfast";
        let mask = [1, 2, 3, 4];
        let mut attack = request.into_bytes();
        attack.extend_from_slice(&[0x81, 0x80 | command.len() as u8]);
        attack.extend_from_slice(&mask);
        attack.extend(
            command
                .iter()
                .enumerate()
                .map(|(i, byte)| byte ^ mask[i % 4]),
        );
        client.write_all(&attack).unwrap();
        let result = websocket_handshake(&mut server, &Session::for_test(&"a".repeat(64)));
        assert!(result.is_err());
        // No upgrade means handle_connection cannot reach handle_state at all.
        let response = read_http_headers(&mut client).unwrap();
        assert!(response.starts_with("HTTP/1.1 403 Forbidden\r\n"));
        assert!(!response.contains("101 Switching Protocols"));
    }

    #[test]
    fn forged_trusted_origin_without_token_is_forbidden_on_all_endpoints() {
        for path in ["/logs", "/modes", "/app-modes"] {
            let (result, response) = handshake_response_with_auth(
                path,
                "Origin: https://mui.kernelsu.org\r\n",
                WS_KEY,
                None,
            );
            assert!(result.is_err());
            assert!(response.starts_with("HTTP/1.1 403 Forbidden\r\n"));
            assert!(!response.contains("Sec-WebSocket-Accept:"));
        }
    }

    #[test]
    fn wrong_short_duplicate_and_previous_tokens_cannot_authenticate() {
        let prefix = web_auth::AUTH_PREFIX;
        for auth in [
            prefix.to_string(),
            format!("{prefix}{}", "a".repeat(63)),
            format!("{prefix}{}", "a".repeat(65)),
            format!("{prefix}{}", "b".repeat(64)),
            format!("{prefix}{}, {prefix}{}", "a".repeat(64), "a".repeat(64)),
            format!("{prefix}{}, {prefix}{}", "b".repeat(64), "a".repeat(64)),
            format!("{prefix}{}, {prefix}{}", "a".repeat(64), "b".repeat(64)),
        ] {
            for path in ["/logs", "/modes", "/app-modes"] {
                let (result, response) = handshake_response_with_auth(
                    path,
                    "Origin: https://mui.kernelsu.org\r\n",
                    WS_KEY,
                    Some(&auth),
                );
                assert!(result.is_err());
                assert!(response.starts_with("HTTP/1.1 403 Forbidden\r\n"));
                assert!(!response.contains(&"a".repeat(64)));
                assert!(!response.contains(&"b".repeat(64)));
            }
        }
    }

    #[test]
    fn split_protocol_headers_are_valid_but_duplicate_auth_is_forbidden() {
        let auth = format!("{}{}", web_auth::AUTH_PREFIX, "a".repeat(64));
        let headers =
            format!("Origin: https://mui.kernelsu.org\r\nSec-WebSocket-Protocol: {auth}\r\n");
        let (result, response) = handshake_response_with_auth("/modes", &headers, WS_KEY, None);
        assert!(result.is_ok());
        assert!(response.starts_with("HTTP/1.1 101 Switching Protocols\r\n"));
        let (result, response) =
            handshake_response_with_auth("/modes", &headers, WS_KEY, Some(&auth));
        assert!(result.is_err());
        assert!(response.starts_with("HTTP/1.1 403 Forbidden\r\n"));
    }

    #[test]
    fn idle_timeout_is_safe_but_partial_header_is_fatal() {
        let (mut tx, mut rx) = pair();
        assert!(matches!(read_frame(&mut rx).unwrap(), Frame::Timeout));
        tx.write_all(&[0x81]).unwrap();
        assert!(read_frame(&mut rx).is_err());
    }
    #[test]
    fn accepts_masked_text_and_rejects_unmasked_clients() {
        let (mut tx, mut rx) = pair();
        tx.write_all(&[0x81, 0x82, 1, 2, 3, 4, b'h' ^ 1, b'i' ^ 2])
            .unwrap();
        assert!(matches!(read_frame(&mut rx).unwrap(),Frame::Text(s) if s=="hi"));
        tx.write_all(&[0x81, 0]).unwrap();
        assert!(read_frame(&mut rx).is_err());
    }
    #[test]
    fn closed_log_connection_exits_and_client_slot_is_released() {
        let dir = std::env::temp_dir().join(format!("cts-ws-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("novasched.log"), "").unwrap();
        let (tx, rx) = pair();
        drop(tx);
        let count = Arc::new(AtomicUsize::new(1));
        {
            let _guard = ClientCount(count.clone());
            assert!(handle_logs(rx, &Logger::new(&dir), Arc::new(AtomicBool::new(false))).is_err());
        }
        assert_eq!(count.load(Ordering::Relaxed), 0);
        fs::remove_dir_all(dir).unwrap();
    }
}
