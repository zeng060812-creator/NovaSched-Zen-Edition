//! Bounded readers for Android's installed-package registry (text/ABX).
//! Does not infer installation from app data directories or old callbacks.
use crate::util::Result;
use std::fs;
use std::io::Read;
use std::path::Path;

pub fn installed_at(path: &Path, package: &str) -> Result<bool> {
    let mut bytes = Vec::new();
    fs::File::open(path)
        .and_then(|f| f.take(8_388_609).read_to_end(&mut bytes))
        .map_err(|e| format!("读取 {} 失败: {e}", path.display()))?;
    if bytes.len() > 8_388_608 {
        return Err("packages.xml 超出读取限制".into());
    }
    parse(&bytes, package)
}
fn parse(bytes: &[u8], package: &str) -> Result<bool> {
    if bytes.starts_with(b"ABX\0") {
        binary(bytes, package)
    } else {
        text(
            std::str::from_utf8(bytes).map_err(|_| "packages.xml 不是 UTF-8 或 ABX")?,
            package,
        )
    }
}
fn text(source: &str, package: &str) -> Result<bool> {
    let bytes = source.as_bytes();
    let mut offset = 0;
    let mut stack: Vec<String> = Vec::new();
    let mut root = false;
    let mut completed = false;
    let mut found = false;
    while offset < bytes.len() {
        let Some(relative) = source[offset..].find('<') else {
            if !source[offset..].trim().is_empty() {
                return Err("packages.xml 标签外文本非法".into());
            }
            break;
        };
        let start = offset + relative;
        if !source[offset..start].trim().is_empty() {
            return Err("packages.xml 标签外文本非法".into());
        }
        if source[start..].starts_with("<!--") {
            offset = start
                + 4
                + source[start + 4..]
                    .find("-->")
                    .ok_or("packages.xml 注释不完整")?
                + 3;
            continue;
        }
        if source[start..].starts_with("<?") {
            offset = start
                + 2
                + source[start + 2..]
                    .find("?>")
                    .ok_or("packages.xml 声明不完整")?
                + 2;
            continue;
        }
        let mut at = start + 1;
        let mut quote = None;
        while at < bytes.len() {
            let b = bytes[at];
            if let Some(q) = quote {
                if b == q {
                    quote = None;
                }
            } else if b == b'\'' || b == b'"' {
                quote = Some(b);
            } else if b == b'>' {
                break;
            }
            at += 1;
        }
        if at == bytes.len() {
            return Err("packages.xml 标签不完整".into());
        }
        let body = source[start + 1..at].trim();
        offset = at + 1;
        if let Some(name) = body.strip_prefix('/') {
            if stack.pop().as_deref() != Some(name.trim()) {
                return Err("packages.xml 标签层级不匹配".into());
            }
            if stack.is_empty() {
                completed = true;
            }
            continue;
        }
        let short = body.ends_with('/');
        let body = if short {
            body[..body.len() - 1].trim()
        } else {
            body
        };
        let end = body.find(char::is_whitespace).unwrap_or(body.len());
        let name = &body[..end];
        if name.is_empty() || name.starts_with('!') || completed {
            return Err("packages.xml 根标签非法".into());
        }
        if stack.is_empty() {
            if root || name != "packages" {
                return Err("packages.xml 根标签非法".into());
            }
            root = true;
        }
        let attrs = attributes(&body[end..])?;
        if stack.len() == 1 && name == "package" {
            found |= attrs.iter().any(|(k, v)| k == "name" && v == package);
        }
        if !short {
            stack.push(name.into());
        } else if stack.is_empty() {
            completed = true;
        }
    }
    if !root || !completed || !stack.is_empty() {
        return Err("packages.xml 不完整".into());
    }
    Ok(found)
}
fn attributes(body: &str) -> Result<Vec<(String, String)>> {
    let mut rest = body.trim();
    let mut result = Vec::new();
    while !rest.is_empty() {
        let end = rest
            .find(|c: char| c.is_whitespace() || c == '=')
            .ok_or("XML 属性缺少值")?;
        let key = &rest[..end];
        if key.is_empty() {
            return Err("XML 属性名称为空".into());
        }
        rest = rest[end..]
            .trim_start()
            .strip_prefix('=')
            .ok_or("XML 属性缺少等号")?
            .trim_start();
        let quote = rest.chars().next().ok_or("XML 属性缺少引号")?;
        if quote != '\'' && quote != '"' {
            return Err("XML 属性缺少引号".into());
        }
        let tail = &rest[1..];
        let end = tail.find(quote).ok_or("XML 属性不完整")?;
        let value = &tail[..end];
        if result.iter().any(|(k, _)| k == key) {
            return Err("XML 属性重复".into());
        }
        result.push((
            key.into(),
            value
                .replace("&quot;", "\"")
                .replace("&apos;", "'")
                .replace("&lt;", "<")
                .replace("&gt;", ">")
                .replace("&amp;", "&"),
        ));
        rest = tail[end + 1..].trim_start();
    }
    Ok(result)
}
struct Reader<'a> {
    data: &'a [u8],
    offset: usize,
    strings: Vec<String>,
}
impl Reader<'_> {
    fn take(&mut self, size: usize) -> Result<&[u8]> {
        let end = self.offset.checked_add(size).ok_or("ABX 长度溢出")?;
        let data = self.data.get(self.offset..end).ok_or("ABX 记录不完整")?;
        self.offset = end;
        Ok(data)
    }
    fn byte(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }
    fn short(&mut self) -> Result<u16> {
        let b = self.take(2)?;
        Ok(u16::from_be_bytes([b[0], b[1]]))
    }
    fn utf(&mut self) -> Result<String> {
        let size = self.short()? as usize;
        Ok(String::from_utf8_lossy(self.take(size)?).into_owned())
    }
    fn interned(&mut self) -> Result<String> {
        let index = self.short()?;
        if index != u16::MAX {
            return self
                .strings
                .get(index as usize)
                .cloned()
                .ok_or_else(|| "ABX 字符串索引非法".into());
        }
        let value = self.utf()?;
        if self.strings.len() < u16::MAX as usize {
            self.strings.push(value.clone());
        }
        Ok(value)
    }
    fn value(&mut self, kind: u8) -> Result<Option<String>> {
        match kind {
            0x10 | 0xc0 | 0xd0 => Ok(None),
            0x20 => self.utf().map(Some),
            0x30 => self.interned().map(Some),
            0x40 | 0x50 => {
                let size = self.short()? as usize;
                self.take(size)?;
                Ok(None)
            }
            0x60 | 0x70 | 0xa0 => {
                self.take(4)?;
                Ok(None)
            }
            0x80 | 0x90 | 0xb0 => {
                self.take(8)?;
                Ok(None)
            }
            _ => Err("ABX 属性类型未知".into()),
        }
    }
}
fn binary(data: &[u8], package: &str) -> Result<bool> {
    let mut r = Reader {
        data,
        offset: 4,
        strings: Vec::new(),
    };
    let mut stack: Vec<String> = Vec::new();
    let mut root = false;
    let mut closed = false;
    let mut found = false;
    let mut attribute_allowed = false;
    while r.offset < data.len() {
        let event = r.byte()?;
        let token = event & 15;
        let kind = event & 0xf0;
        match token {
            0 => {
                if root {
                    return Err("ABX 文档头位置非法".into());
                }
                attribute_allowed = false;
            }
            1 => {
                if !closed || !stack.is_empty() || r.offset != data.len() {
                    return Err("ABX 文档尾不完整".into());
                }
                attribute_allowed = false;
            }
            2 => {
                if kind != 0x30 || closed {
                    return Err("ABX 标签类型非法".into());
                }
                let tag = r.interned()?;
                if stack.is_empty() {
                    if root || tag != "packages" {
                        return Err("ABX 根标签非法".into());
                    }
                    root = true;
                }
                if stack.len() >= 64 {
                    return Err("ABX 嵌套过深".into());
                }
                stack.push(tag);
                attribute_allowed = true;
            }
            3 => {
                if kind != 0x30 || stack.pop() != Some(r.interned()?) {
                    return Err("ABX 标签层级不匹配".into());
                }
                if stack.is_empty() {
                    closed = true;
                }
                attribute_allowed = false;
            }
            15 => {
                if !attribute_allowed {
                    return Err("ABX 属性位置非法".into());
                }
                let name = r.interned()?;
                let value = r.value(kind)?;
                if stack.len() == 2
                    && stack.last().is_some_and(|tag| tag == "package")
                    && name == "name"
                {
                    found |= value.as_deref() == Some(package);
                }
            }
            4..=10 => {
                r.value(kind)?;
                attribute_allowed = false;
            }
            _ => return Err("ABX token 未知".into()),
        }
    }
    if !root || !closed || !stack.is_empty() {
        return Err("ABX 注册表不完整".into());
    }
    Ok(found)
}
pub fn process_running_at(root: &Path, package: &str) -> Result<bool> {
    let entries = fs::read_dir(root).map_err(|e| format!("读取进程目录失败: {e}"))?;
    for entry in entries {
        let entry = entry.map_err(|e| format!("读取进程记录失败: {e}"))?;
        if !entry
            .file_name()
            .to_str()
            .is_some_and(|pid| !pid.is_empty() && pid.bytes().all(|b| b.is_ascii_digit()))
        {
            continue;
        }
        let mut bytes = Vec::new();
        match fs::File::open(entry.path().join("cmdline"))
            .and_then(|f| f.take(4096).read_to_end(&mut bytes))
        {
            Ok(_) => {}
            Err(_) => continue, // kernel tasks/exiting tasks/denied processes are not evidence.
        }
        let first = bytes.split(|b| *b == 0).next().unwrap_or(&[]);
        if first == package.as_bytes()
            || first
                .strip_prefix(package.as_bytes())
                .is_some_and(|rest| rest.first() == Some(&b':'))
        {
            return Ok(true);
        }
    }
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    const PACKAGE: &str = "com.omarea.vtools";
    #[test]
    fn text_requires_real_package_records_not_comments_backups_or_similar_names() {
        assert!(parse(br#"<?xml version="1.0"?><packages><package codePath="/data/app/x" name="com.omarea.vtools" /></packages>"#,PACKAGE).unwrap());
        assert!(!parse(br#"<packages><!-- <package name="com.omarea.vtools"/> --><updated-package name="com.omarea.vtools"/><package name="com.omarea.vtools.fake"/></packages>"#,PACKAGE).unwrap());
        assert!(!parse(
            br#"<packages><shared-user name="com.omarea.vtools"/></packages>"#,
            PACKAGE
        )
        .unwrap());
        for invalid in [
            b"<packages><package/>".as_slice(),
            b"<map/>",
            b"<packages><package></bad></packages>",
        ] {
            assert!(parse(invalid, PACKAGE).is_err());
        }
    }
    fn interned(bytes: &mut Vec<u8>, value: &str) {
        bytes.extend(u16::MAX.to_be_bytes());
        bytes.extend((value.len() as u16).to_be_bytes());
        bytes.extend(value.as_bytes());
    }
    fn abx(package: &str) -> Vec<u8> {
        let mut b = b"ABX\0".to_vec();
        b.push(0x10);
        b.push(0x32);
        interned(&mut b, "packages");
        b.push(0x32);
        interned(&mut b, "package");
        b.push(0x2f);
        interned(&mut b, "name");
        b.extend((package.len() as u16).to_be_bytes());
        b.extend(package.as_bytes());
        b.push(0x6f);
        interned(&mut b, "userId");
        b.extend(10000u32.to_be_bytes());
        b.push(0x33);
        b.extend(1u16.to_be_bytes());
        b.push(0x33);
        b.extend(0u16.to_be_bytes());
        b.push(0x11);
        b
    }
    #[test]
    fn abx_parses_interning_typed_attributes_and_rejects_truncated_or_wrong_indices() {
        let b = abx(PACKAGE);
        assert!(parse(&b, PACKAGE).unwrap());
        assert!(!parse(&abx("com.other.app"), PACKAGE).unwrap());
        for end in 0..b.len() - 3 {
            assert!(parse(&b[..end], PACKAGE).is_err(), "{end}");
        }
        let mut bad = b.clone();
        let n = bad.len();
        bad[n - 2] = 0xfe;
        assert!(parse(&bad, PACKAGE).is_err());
    }
    #[test]
    fn process_names_are_exact_and_stale_app_directories_are_not_installation_evidence() {
        let root = std::env::temp_dir().join(format!("nova-proc-{}", std::process::id()));
        fs::create_dir_all(root.join("42")).unwrap();
        fs::write(
            root.join("42/cmdline"),
            b"com.omarea.vtools.fake\0com.omarea.vtools\0",
        )
        .unwrap();
        assert!(!process_running_at(&root, PACKAGE).unwrap());
        fs::write(root.join("42/cmdline"), b"com.omarea.vtools:worker\0").unwrap();
        assert!(process_running_at(&root, PACKAGE).unwrap());
        fs::remove_dir_all(root).unwrap();
    }
}
