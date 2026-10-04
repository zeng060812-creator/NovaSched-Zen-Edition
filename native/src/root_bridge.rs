//! Root-authorized transport for WebViews that cannot open loopback WebSockets.
//! No HTTP API, unauthenticated localhost fallback, or separate policy writer.
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::unix::fs::MetadataExt;
use std::path::Path;

use crate::{
    app_modes,
    json::{self, Value},
    util::{self, Result},
    web_auth, websocket,
};

struct Request {
    endpoint: String,
    message: Option<String>,
    logs: bool,
    cursor: String,
}

fn parse_request(text: &str) -> Result<Request> {
    if text.len() > 4096 {
        return Err("root 通道请求过大".into());
    }
    let Value::Object(values) = json::parse(text)? else {
        return Err("root 通道请求需要 JSON 对象".into());
    };
    if values
        .keys()
        .any(|key| !["endpoint", "message", "logs", "cursor"].contains(&key.as_str()))
    {
        return Err("root 通道请求包含未知字段".into());
    }
    let endpoint = values
        .get("endpoint")
        .ok_or("root 通道缺少 endpoint")?
        .as_str()?
        .to_string();
    if !["snapshot", "modes", "apps"].contains(&endpoint.as_str()) {
        return Err("root 通道 endpoint 无效".into());
    }
    let message = values
        .get("message")
        .map(Value::as_str)
        .transpose()?
        .map(str::to_string);
    if let Some(message) = message.as_deref() {
        validate_message(&endpoint, message)?;
    }
    let logs = values
        .get("logs")
        .map(Value::as_bool)
        .transpose()?
        .unwrap_or(false);
    let cursor = values
        .get("cursor")
        .map(Value::as_str)
        .transpose()?
        .unwrap_or("")
        .to_string();
    if !cursor.is_empty() {
        parse_cursor(&cursor)?;
    }
    Ok(Request {
        endpoint,
        message,
        logs,
        cursor,
    })
}

fn validate_message(endpoint: &str, message: &str) -> Result<()> {
    let valid = match endpoint {
        "modes" => {
            app_modes::supported(message)
                || ["smooth\t0", "smooth\t1", "extreme\t0", "extreme\t1"].contains(&message)
        }
        "apps" => {
            message
                .strip_prefix("set\t")
                .and_then(|value| value.split_once('\t'))
                .is_some_and(|(package, mode)| {
                    package != "*" && util::valid_package(package) && app_modes::supported(mode)
                })
                || message
                    .strip_prefix("delete\t")
                    .is_some_and(|package| package != "*" && util::valid_package(package))
        }
        _ => false,
    };
    if valid {
        Ok(())
    } else {
        Err("root 通道命令格式无效".into())
    }
}

fn parse_cursor(cursor: &str) -> Result<(u64, u64)> {
    if cursor.len() > 41 {
        return Err("日志游标无效".into());
    }
    let (inode, offset) = cursor.split_once(':').ok_or("日志游标无效")?;
    if !inode.bytes().all(|b| b.is_ascii_digit()) || !offset.bytes().all(|b| b.is_ascii_digit()) {
        return Err("日志游标无效".into());
    }
    Ok((
        inode.parse().map_err(|_| "日志游标无效")?,
        offset.parse().map_err(|_| "日志游标无效")?,
    ))
}

fn log_chunk(path: &Path, cursor: &str) -> Result<(String, String, bool)> {
    let mut file = match File::open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok((String::new(), String::new(), false))
        }
        Err(e) => return Err(format!("root 通道读取内部日志失败: {e}")),
    };
    let metadata = file.metadata().map_err(|e| e.to_string())?;
    let (inode, offset) = if cursor.is_empty() {
        (0, 0)
    } else {
        parse_cursor(cursor)?
    };
    let reset = inode != metadata.ino() || offset > metadata.len();
    let mut start = if reset {
        metadata.len().saturating_sub(32_768)
    } else {
        offset
    };
    // Do not split a UTF-8 character between polls. An initial tail also skips
    // an incomplete first line so the existing UI log parser receives records.
    if reset && start > 0 {
        file.seek(SeekFrom::Start(start))
            .map_err(|e| e.to_string())?;
        let mut prefix = Vec::new();
        (&mut file)
            .take(4096)
            .read_to_end(&mut prefix)
            .map_err(|e| e.to_string())?;
        if let Some(end) = prefix.iter().position(|b| *b == b'\n') {
            start += end as u64 + 1;
        }
    }
    file.seek(SeekFrom::Start(start))
        .map_err(|e| e.to_string())?;
    let mut bytes = Vec::new();
    (&mut file)
        .take(16_384)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    let consumed = match std::str::from_utf8(&bytes) {
        Ok(_) => bytes.len(),
        Err(e) if e.error_len().is_none() => e.valid_up_to(),
        Err(_) => bytes.len(),
    };
    let text = String::from_utf8_lossy(&bytes[..consumed]).into_owned();
    Ok((
        text,
        format!("{}:{}", metadata.ino(), start + consumed as u64),
        reset,
    ))
}

pub fn run(module: &Path, origin: &str, request: &str) -> Result<()> {
    if unsafe { crate::ffi::geteuid() } != 0 {
        return Err("root 通道需要宿主 root 授权".into());
    }
    let request = parse_request(request)?;
    // Opaque file/content WebViews use only the root bridge. They are never
    // admitted as Origin:null on the public WebSocket listener.
    let origin = if origin == "null" {
        "https://mui.kernelsu.org"
    } else {
        origin
    };
    let session = web_auth::current(module, origin)?;
    let mut modes = String::new();
    let mut apps = String::new();
    if request.endpoint == "snapshot" || request.endpoint == "modes" {
        modes = websocket::bridge_request(
            &session,
            origin,
            false,
            if request.endpoint == "modes" {
                request.message.as_deref()
            } else {
                None
            },
        )?;
    }
    if request.endpoint == "snapshot" || request.endpoint == "apps" {
        apps = websocket::bridge_request(
            &session,
            origin,
            true,
            if request.endpoint == "apps" {
                request.message.as_deref()
            } else {
                None
            },
        )?;
    }
    let (logs, cursor, reset) = if request.logs {
        log_chunk(
            &Path::new(util::STATE_DIR).join("novasched.log"),
            &request.cursor,
        )?
    } else {
        (String::new(), request.cursor, false)
    };
    let q = websocket::json_escape;
    let reply = format!("{{\"modes\":\"{}\",\"apps\":\"{}\",\"logs\":\"{}\",\"cursor\":\"{}\",\"logsReset\":{reset}}}\n", q(&modes), q(&apps), q(&logs), q(&cursor));
    std::io::stdout()
        .write_all(reply.as_bytes())
        .map_err(|e| format!("root 通道输出失败: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_rpc_surface_rejects_paths_commands_and_unknown_fields() {
        assert!(parse_request(r#"{"endpoint":"snapshot","logs":true,"cursor":"12:34"}"#).is_ok());
        assert!(parse_request(r#"{"endpoint":"modes","message":"fast"}"#).is_ok());
        assert!(
            parse_request(r#"{"endpoint":"apps","message":"set\torg.example.game\tfast"}"#).is_ok()
        );
        for request in [
            r#"{"endpoint":"../../config.json"}"#,
            r#"{"endpoint":"modes","message":"fast; reboot"}"#,
            r#"{"endpoint":"snapshot","message":"fast"}"#,
            r#"{"endpoint":"apps","message":"delete\t*"}"#,
            r#"{"endpoint":"modes","shell":"id"}"#,
            r#"{"endpoint":"snapshot","cursor":"1:-1"}"#,
        ] {
            assert!(parse_request(request).is_err(), "{request}");
        }
    }

    #[test]
    fn log_cursor_is_incremental_and_survives_rotation_and_utf8_boundaries() {
        let dir = std::env::temp_dir().join(format!("nova-rpc-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("log");
        std::fs::write(&path, "2026-10-03 12:00:00 信息 -> 正常\n").unwrap();
        let (first, cursor, reset) = log_chunk(&path, "").unwrap();
        assert!(reset && first.contains("正常"));
        assert_eq!(log_chunk(&path, &cursor).unwrap().0, "");
        std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all("追加\n".as_bytes())
            .unwrap();
        assert_eq!(log_chunk(&path, &cursor).unwrap().0, "追加\n");
        std::fs::rename(&path, dir.join("old")).unwrap();
        std::fs::write(&path, "轮转\n").unwrap();
        let (text, _, reset) = log_chunk(&path, &cursor).unwrap();
        assert!(reset && text == "轮转\n");
        std::fs::write(&path, format!("{}中尾", "a".repeat(16_383))).unwrap();
        let (text, cursor, _) = log_chunk(&path, "").unwrap();
        assert_eq!(text, "a".repeat(16_383));
        assert_eq!(log_chunk(&path, &cursor).unwrap().0, "中尾");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
