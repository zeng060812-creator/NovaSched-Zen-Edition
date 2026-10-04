use std::fs;
use std::path::Path;
use std::sync::{Arc, Mutex};

use crate::util::{self, Result, OPTIONS_PATH};

#[derive(Clone, Debug, PartialEq, Eq)]
struct Data {
    extreme_powersave: bool,
    smooth_powersave: bool,
}

#[derive(Clone)]
pub struct Options {
    data: Arc<Mutex<Data>>,
}

impl Options {
    pub fn initialize(default_extreme: bool, default_smooth: bool) -> Result<Self> {
        let path = Path::new(OPTIONS_PATH);
        if let Some(parent) = path.parent() {
            util::create_dir(parent)?;
        }
        let data = match fs::read_to_string(path) {
            Ok(text) => parse(&text)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let data = Data {
                    extreme_powersave: default_extreme,
                    smooth_powersave: default_smooth,
                };
                write(&data)?;
                data
            }
            Err(error) => return Err(format!("读取选项失败: {error}")),
        };
        Ok(Self {
            data: Arc::new(Mutex::new(data)),
        })
    }

    pub fn reload(&self) -> Result<bool> {
        let next = parse(&util::read_text(Path::new(OPTIONS_PATH))?)?;
        let mut guard = self.data.lock().map_err(|_| "选项锁损坏".to_string())?;
        if *guard == next {
            return Ok(false);
        }
        *guard = next;
        Ok(true)
    }

    pub fn extreme_powersave(&self) -> bool {
        self.data
            .lock()
            .map(|data| data.extreme_powersave)
            .unwrap_or(false)
    }

    pub fn set_extreme_powersave(&self, enabled: bool) -> Result<()> {
        if enabled
            && crate::config::Config::load(Path::new(util::CONFIG_PATH))?
                .functions
                .extreme_powersave
                .cpuset_top_app
                .is_none()
        {
            return Err("当前处理器配置未提供极限节能参数".into());
        }
        let mut guard = self.data.lock().map_err(|_| "选项锁损坏".to_string())?;
        if guard.extreme_powersave == enabled {
            return Ok(());
        }
        let next = Data {
            extreme_powersave: enabled,
            smooth_powersave: guard.smooth_powersave,
        };
        write(&next)?;
        *guard = next;
        Ok(())
    }

    pub fn smooth_powersave(&self) -> bool {
        self.data
            .lock()
            .map(|data| data.smooth_powersave)
            .unwrap_or(false)
    }

    pub fn set_smooth_powersave(&self, enabled: bool) -> Result<()> {
        if enabled
            && crate::config::Config::load(Path::new(util::CONFIG_PATH))?
                .functions
                .smooth_powersave
                .as_ref()
                .map_or(true, |v| v.limits.cpuset_top_app.is_none())
        {
            return Err("当前配置缺少 SmoothPowerSave，请使用本版配置".into());
        }
        let mut guard = self.data.lock().map_err(|_| "选项锁损坏".to_string())?;
        if guard.smooth_powersave == enabled {
            return Ok(());
        }
        let next = Data {
            extreme_powersave: guard.extreme_powersave,
            smooth_powersave: enabled,
        };
        write(&next)?;
        *guard = next;
        Ok(())
    }
}

fn parse(text: &str) -> Result<Data> {
    let mut value = None;
    let mut smooth = false;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, raw)) = line.split_once('=') else {
            return Err("options.txt 格式错误".to_string());
        };
        match key.trim() {
            "extreme_powersave" => match raw.trim() {
                "0" => value = Some(false),
                "1" => value = Some(true),
                _ => return Err("extreme_powersave 只能为 0 或 1".to_string()),
            },
            "smooth_powersave" => match raw.trim() {
                "0" => smooth = false,
                "1" => smooth = true,
                _ => return Err("smooth_powersave 只能为 0 或 1".into()),
            },
            _ => return Err(format!("options.txt 包含未知选项: {}", key.trim())),
        }
    }
    Ok(Data {
        extreme_powersave: value.ok_or_else(|| "options.txt 缺少 extreme_powersave".to_string())?,
        smooth_powersave: smooth,
    })
}

fn write(data: &Data) -> Result<()> {
    util::atomic_write(
        Path::new(OPTIONS_PATH),
        format!(
            "# NovaSched Zen Edition 可选功能\n# 仅省电档生效；流畅省电优先于极限节能，两个开关新装默认关闭\nextreme_powersave={}\nsmooth_powersave={}\n",
            if data.extreme_powersave { 1 } else { 0 },
            if data.smooth_powersave { 1 } else { 0 }
        )
        .as_bytes(),
        0o664,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_accepts_boolean_switch() {
        assert!(parse("extreme_powersave=0\n").is_ok());
        assert!(parse("extreme_powersave=1\n").is_ok());
        assert!(parse("extreme_powersave=true\n").is_err());
    }

    #[test]
    fn legacy_options_keep_smooth_off_and_new_switch_is_strict() {
        assert!(
            !parse("extreme_powersave=1\n")
                .expect("legacy")
                .smooth_powersave
        );
        let both = parse("extreme_powersave=1\nsmooth_powersave=1\n").expect("both");
        assert!(both.extreme_powersave && both.smooth_powersave);
        assert!(parse("extreme_powersave=1\nsmooth_powersave=true\n").is_err());
    }
}
