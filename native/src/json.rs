use std::collections::BTreeMap;

use crate::util::Result;

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Number(i64),
    String(String),
    Array(Vec<Value>),
    Object(BTreeMap<String, Value>),
}

impl Value {
    pub fn get(&self, key: &str) -> Result<&Value> {
        match self {
            Value::Object(map) => map.get(key).ok_or_else(|| format!("JSON 缺少字段: {key}")),
            _ => Err(format!("JSON 节点不是对象，无法读取: {key}")),
        }
    }

    pub fn as_str(&self) -> Result<&str> {
        match self {
            Value::String(value) => Ok(value),
            _ => Err("JSON 字段不是字符串".to_string()),
        }
    }

    pub fn as_i64(&self) -> Result<i64> {
        match self {
            Value::Number(value) => Ok(*value),
            _ => Err("JSON 字段不是整数".to_string()),
        }
    }

    pub fn as_bool(&self) -> Result<bool> {
        match self {
            Value::Bool(value) => Ok(*value),
            _ => Err("JSON 字段不是布尔值".to_string()),
        }
    }
}

pub fn parse(text: &str) -> Result<Value> {
    if text.len() > 1024 * 1024 {
        return Err("JSON 超过 1 MiB".into());
    }
    let mut parser = Parser {
        bytes: text.as_bytes(),
        pos: 0,
        depth: 0,
    };
    let value = parser.value()?;
    parser.space();
    if parser.pos != parser.bytes.len() {
        return Err(format!("JSON 末尾存在多余内容，偏移 {}", parser.pos));
    }
    Ok(value)
}

/// Serialize parsed configuration without dropping unknown user fields.
pub fn stringify(value: &Value) -> Result<String> {
    fn quoted(value: &str, out: &mut String) {
        out.push('"');
        for c in value.chars() {
            match c {
                '"' => out.push_str("\\\""),
                '\\' => out.push_str("\\\\"),
                '\n' => out.push_str("\\n"),
                '\r' => out.push_str("\\r"),
                '\t' => out.push_str("\\t"),
                c if c < ' ' => out.push_str(&format!("\\u{:04x}", c as u32)),
                c => out.push(c),
            }
        }
        out.push('"');
    }
    fn write(value: &Value, out: &mut String, depth: usize) -> Result<()> {
        if depth >= 64 {
            return Err("JSON 序列化嵌套超过 64 层".into());
        }
        match value {
            Value::Null => out.push_str("null"),
            Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            Value::Number(n) => out.push_str(&n.to_string()),
            Value::String(s) => quoted(s, out),
            Value::Array(items) => {
                out.push('[');
                for (index, item) in items.iter().enumerate() {
                    if index > 0 {
                        out.push(',');
                    }
                    out.push('\n');
                    out.push_str(&"  ".repeat(depth + 1));
                    write(item, out, depth + 1)?;
                }
                if !items.is_empty() {
                    out.push('\n');
                    out.push_str(&"  ".repeat(depth));
                }
                out.push(']');
            }
            Value::Object(items) => {
                out.push('{');
                for (index, (key, item)) in items.iter().enumerate() {
                    if index > 0 {
                        out.push(',');
                    }
                    out.push('\n');
                    out.push_str(&"  ".repeat(depth + 1));
                    quoted(key, out);
                    out.push_str(": ");
                    write(item, out, depth + 1)?;
                }
                if !items.is_empty() {
                    out.push('\n');
                    out.push_str(&"  ".repeat(depth));
                }
                out.push('}');
            }
        }
        if out.len() > 1024 * 1024 {
            return Err("JSON 序列化超过 1 MiB".into());
        }
        Ok(())
    }
    let mut out = String::new();
    write(value, &mut out, 0)?;
    out.push('\n');
    Ok(out)
}

struct Parser<'a> {
    bytes: &'a [u8],
    pos: usize,
    depth: usize,
}

impl Parser<'_> {
    fn value(&mut self) -> Result<Value> {
        if self.depth >= 64 {
            return Err("JSON 嵌套超过 64 层".into());
        }
        self.depth += 1;
        let result = self.value_inner();
        self.depth -= 1;
        result
    }
    fn value_inner(&mut self) -> Result<Value> {
        self.space();
        let Some(byte) = self.peek() else {
            return Err("JSON 意外结束".to_string());
        };
        match byte {
            b'{' => self.object(),
            b'[' => self.array(),
            b'"' => self.string().map(Value::String),
            b't' => {
                self.literal(b"true")?;
                Ok(Value::Bool(true))
            }
            b'f' => {
                self.literal(b"false")?;
                Ok(Value::Bool(false))
            }
            b'n' => {
                self.literal(b"null")?;
                Ok(Value::Null)
            }
            b'-' | b'0'..=b'9' => self.number().map(Value::Number),
            _ => Err(format!("JSON 非法字符 0x{byte:02x}，偏移 {}", self.pos)),
        }
    }

    fn object(&mut self) -> Result<Value> {
        self.expect(b'{')?;
        let mut map = BTreeMap::new();
        self.space();
        if self.consume(b'}') {
            return Ok(Value::Object(map));
        }
        loop {
            self.space();
            let key = self.string()?;
            self.space();
            self.expect(b':')?;
            let value = self.value()?;
            map.insert(key, value);
            self.space();
            if self.consume(b'}') {
                break;
            }
            self.expect(b',')?;
        }
        Ok(Value::Object(map))
    }

    fn array(&mut self) -> Result<Value> {
        self.expect(b'[')?;
        let mut values = Vec::new();
        self.space();
        if self.consume(b']') {
            return Ok(Value::Array(values));
        }
        loop {
            values.push(self.value()?);
            self.space();
            if self.consume(b']') {
                break;
            }
            self.expect(b',')?;
        }
        Ok(Value::Array(values))
    }

    fn string(&mut self) -> Result<String> {
        self.expect(b'"')?;
        let mut output = String::new();
        let mut start = self.pos;
        while self.pos < self.bytes.len() {
            match self.bytes[self.pos] {
                b'"' => {
                    output.push_str(
                        std::str::from_utf8(&self.bytes[start..self.pos])
                            .map_err(|e| format!("JSON 字符串 UTF-8 错误: {e}"))?,
                    );
                    self.pos += 1;
                    return Ok(output);
                }
                b'\\' => {
                    output.push_str(
                        std::str::from_utf8(&self.bytes[start..self.pos])
                            .map_err(|e| format!("JSON 字符串 UTF-8 错误: {e}"))?,
                    );
                    self.pos += 1;
                    let escaped = self.next().ok_or_else(|| "JSON 转义意外结束".to_string())?;
                    match escaped {
                        b'"' => output.push('"'),
                        b'\\' => output.push('\\'),
                        b'/' => output.push('/'),
                        b'b' => output.push('\u{0008}'),
                        b'f' => output.push('\u{000c}'),
                        b'n' => output.push('\n'),
                        b'r' => output.push('\r'),
                        b't' => output.push('\t'),
                        b'u' => {
                            let first = self.hex4()?;
                            let scalar = if (0xD800..=0xDBFF).contains(&first) {
                                self.expect(b'\\')?;
                                self.expect(b'u')?;
                                let second = self.hex4()?;
                                if !(0xDC00..=0xDFFF).contains(&second) {
                                    return Err("JSON Unicode 代理对无效".to_string());
                                }
                                0x10000
                                    + (((first - 0xD800) as u32) << 10)
                                    + (second - 0xDC00) as u32
                            } else {
                                first as u32
                            };
                            output.push(
                                char::from_u32(scalar)
                                    .ok_or_else(|| "JSON Unicode 码点无效".to_string())?,
                            );
                        }
                        _ => return Err("JSON 转义字符无效".to_string()),
                    }
                    start = self.pos;
                }
                0x00..=0x1f => return Err("JSON 字符串包含控制字符".to_string()),
                _ => self.pos += 1,
            }
        }
        Err("JSON 字符串缺少结束引号".to_string())
    }

    fn hex4(&mut self) -> Result<u16> {
        let mut value = 0u16;
        for _ in 0..4 {
            let byte = self
                .next()
                .ok_or_else(|| "JSON Unicode 转义不完整".to_string())?;
            let digit = match byte {
                b'0'..=b'9' => byte - b'0',
                b'a'..=b'f' => byte - b'a' + 10,
                b'A'..=b'F' => byte - b'A' + 10,
                _ => return Err("JSON Unicode 转义无效".to_string()),
            };
            value = (value << 4) | digit as u16;
        }
        Ok(value)
    }

    fn number(&mut self) -> Result<i64> {
        let start = self.pos;
        if self.consume(b'-') {}
        if self.consume(b'0') {
        } else {
            self.take_digits();
        }
        if matches!(self.peek(), Some(b'.' | b'e' | b'E')) {
            return Err("本配置仅支持整数".to_string());
        }
        let text = std::str::from_utf8(&self.bytes[start..self.pos]).map_err(|e| e.to_string())?;
        text.parse::<i64>()
            .map_err(|e| format!("JSON 整数无效: {e}"))
    }

    fn take_digits(&mut self) {
        while matches!(self.peek(), Some(b'0'..=b'9')) {
            self.pos += 1;
        }
    }

    fn literal(&mut self, expected: &[u8]) -> Result<()> {
        if self.bytes.get(self.pos..self.pos + expected.len()) == Some(expected) {
            self.pos += expected.len();
            Ok(())
        } else {
            Err(format!("JSON 字面量无效，偏移 {}", self.pos))
        }
    }

    fn space(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\n' | b'\r' | b'\t')) {
            self.pos += 1;
        }
    }

    fn expect(&mut self, expected: u8) -> Result<()> {
        if self.consume(expected) {
            Ok(())
        } else {
            Err(format!(
                "JSON 期望 '{}'，偏移 {}",
                expected as char, self.pos
            ))
        }
    }

    fn consume(&mut self, expected: u8) -> bool {
        if self.peek() == Some(expected) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }
    fn next(&mut self) -> Option<u8> {
        let out = self.peek();
        if out.is_some() {
            self.pos += 1;
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_nested_json() {
        let root = parse(r#"{"a":1,"b":true,"c":"小米\n14","d":[null,-2]}"#).unwrap();
        assert_eq!(root.get("a").unwrap().as_i64().unwrap(), 1);
        assert!(root.get("b").unwrap().as_bool().unwrap());
        assert_eq!(root.get("c").unwrap().as_str().unwrap(), "小米\n14");
    }

    #[test]
    fn serialization_preserves_unknown_fields_unicode_and_control_characters() {
        let value =
            parse(r#"{"note":"Zen \"你好\" \\ \u0001\n\t","custom":[1,true,null,{"x":-2}]}"#)
                .unwrap();
        assert_eq!(parse(&stringify(&value).unwrap()).unwrap(), value);
        assert!(stringify(&Value::String("x".repeat(1024 * 1024))).is_err());
    }
}

#[cfg(test)]
mod regression_tests {
    use super::*;
    #[test]
    fn overly_nested_input_is_an_error_not_stack_overflow() {
        let text = format!("{}0{}", "[".repeat(256), "]".repeat(256));
        assert!(parse(&text).is_err());
    }
}
