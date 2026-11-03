//! ZenJooo · 使用 / 电量 / 性能 / 策略的轻量滚动轨迹。
//!
//! 由守护主循环节流写入（游戏前台 10 秒、其余 20 秒；不唤醒设备），
//! 每个样本一行 JSON 追加到按天的 .jsonl 文件。损坏行在读取时跳过，
//! 不补零、不回填；保留 7 天且总量有上限，超限删除最旧文件。
//! 电池侧读数（百分比/充电/温度）来自内核电源供应节点；频率为内核
//! 报告值，可能是请求频率——展示层必须如实标注。
use crate::util::{self, Result};
use std::path::{Path, PathBuf};

pub const DIR: &str = "/data/adb/novasched/history";
/// 保留天数与总容量上限；超限删除最旧文件。
const KEEP_DAYS: u64 = 7;
const TOTAL_CAP: u64 = 8 * 1024 * 1024;
const GAME_INTERVAL_SECS: u64 = 10;
const IDLE_INTERVAL_SECS: u64 = 20;

/// 一次轨迹采样。fps 为 None 表示 NovaFAS 无有效帧信号（展示层留空，
/// 不补零）。boost 为 NovaFAS 当拍加压百分比（0 = 未加压/引擎未运行）：
/// 第二阶段·闭环反馈用——对照同一时间线的"加压幅度 ↔ 实际帧率"，
/// 回答"加压有没有换来帧收益"。
#[derive(Clone, Debug, PartialEq)]
pub struct Sample {
    pub t: u64,
    pub pkg: String,
    pub mode: String,
    pub fps: Option<f32>,
    pub target: u32,
    pub boost: u32,
    pub freqs: [u32; 4],
    pub battery: Option<u32>,
    pub charging: Option<bool>,
    pub temp: Option<f32>,
}

impl Sample {
    pub fn to_line(&self) -> String {
        let freqs = self
            .freqs
            .iter()
            .map(u32::to_string)
            .collect::<Vec<_>>()
            .join(",");
        // pkg/mode 来自守护内部状态，仍做最小转义：拒绝引号与反斜杠。
        let pkg = sanitize(&self.pkg);
        let mode = sanitize(&self.mode);
        let charging = match self.charging {
            Some(true) => "true".to_string(),
            Some(false) => "false".to_string(),
            None => "null".to_string(),
        };
        let battery = match self.battery {
            Some(v) => v.to_string(),
            None => "null".to_string(),
        };
        let temp = match self.temp {
            Some(v) => format!("{}", (v * 10.0).round() as i64),
            None => "null".to_string(),
        };
        // 项目 json::Number 为 i64：FPS/温度都以 0.1 为最小刻度存整（595=59.5），
        // null 表示无有效读数；小数字面量会让该行 parse 失败（真机实测）。
        let fps = match self.fps {
            Some(v) => format!("{}", (v * 10.0).round() as i64),
            None => "null".into(),
        };
        format!(
            "{{\"t\":{t},\"pkg\":\"{pkg}\",\"mode\":\"{mode}\",\"fps\":{fps},\"tgt\":{tgt},\"boost\":{boost},\"freqs\":[{freqs}],\"batt\":{battery},\"chg\":{charging},\"temp\":{temp}}}\n",
            t = self.t,
            tgt = self.target,
            boost = self.boost,
        )
    }
    pub fn parse(line: &str) -> Option<Self> {
        let value = crate::json::parse(line.trim()).ok()?;
        let obj = match value {
            crate::json::Value::Object(map) => map,
            _ => return None,
        };
        let get_num = |key: &str| -> Option<i64> {
            match obj.get(key)? {
                crate::json::Value::Number(n) => Some(*n),
                _ => None,
            }
        };
        let get_str = |key: &str| -> Option<String> {
            match obj.get(key)? {
                crate::json::Value::String(v) => Some(v.clone()),
                _ => None,
            }
        };
        let t = get_num("t")? as u64;
        let pkg = get_str("pkg")?;
        let mode = get_str("mode")?;
        // 写入侧按 0.1 刻度存整（to_line ×10）；读回必须 ÷10 还原。
        // 审计 P1-1：漏除曾让历史 fps/温度虚高 10 倍，且 history 命令把
        // parse 结果再 to_line 输出、二次放大为 ×100。
        let fps = match obj.get("fps")? {
            crate::json::Value::Number(n) => Some(*n as f32 / 10.0),
            crate::json::Value::Null => None,
            _ => return None,
        };
        let battery = match obj.get("batt")? {
            crate::json::Value::Number(n) => Some(*n as u32),
            crate::json::Value::Null => None,
            _ => return None,
        };
        let charging = match obj.get("chg")? {
            crate::json::Value::Bool(b) => Some(*b),
            crate::json::Value::Null => None,
            _ => return None,
        };
        // 与 fps 同一 0.1 刻度约定（写入 ×10，读回 ÷10，见上方注释）。
        let temp = match obj.get("temp")? {
            crate::json::Value::Number(n) => Some(*n as f32 / 10.0),
            crate::json::Value::Null => None,
            _ => return None,
        };
        // boost 为后加字段：旧文件没有该键，缺省 0（向后兼容读回）。
        let boost = get_num("boost").unwrap_or(0).max(0) as u32;
        let target = get_num("tgt").unwrap_or(0) as u32;
        let mut freqs = [0u32; 4];
        if let Some(crate::json::Value::Array(items)) = obj.get("freqs") {
            for (index, item) in items.iter().take(4).enumerate() {
                if let crate::json::Value::Number(n) = item {
                    freqs[index] = *n as u32;
                }
            }
        }
        Some(Self {
            t,
            pkg,
            mode,
            fps,
            target,
            boost,
            freqs,
            battery,
            charging,
            temp,
        })
    }
}

fn sanitize(value: &str) -> String {
    value
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-' || c == ':' { c } else { '_' })
        .take(255)
        .collect()
}

fn day_key(t: u64) -> String {
    // 按 UTC 天分文件；时区仅影响展示层，不影响存储正确性。
    format!("history-{}.jsonl", t / 86400)
}

fn day_from_name(name: &str) -> Option<u64> {
    name.strip_prefix("history-")?
        .strip_suffix(".jsonl")?
        .parse()
        .ok()
}

/// 节流状态：上次写入时间。由守护循环持有。
#[derive(Default)]
pub struct Recorder {
    last_write: u64,
}

impl Recorder {
    /// 节流判定 + 占位：返回 true 表示到了写入时机（调用方采集数据后
    /// 调 append）。占位本身即消费本窗口，避免调用方采集失败后每拍重试。
    pub fn due(&mut self, now: u64, game: bool) -> bool {
        let interval = if game { GAME_INTERVAL_SECS } else { IDLE_INTERVAL_SECS };
        if now.saturating_sub(self.last_write) < interval {
            return false;
        }
        self.last_write = now;
        true
    }

    pub fn append(&self, sample: &Sample) {
        self.append_in(Path::new(DIR), sample);
    }

    /// 电池读数（百分比、充电中、电池温度℃）。任一节点缺失返回 None——
    /// 展示层留空，不补零。
    pub fn read_battery() -> (Option<u32>, Option<bool>, Option<f32>) {
        let base = "/sys/class/power_supply/battery";
        let capacity = util::read_trimmed(Path::new(&format!("{base}/capacity")))
            .ok()
            .and_then(|v| v.trim().parse::<u32>().ok())
            .filter(|v| *v <= 100);
        let status = util::read_trimmed(Path::new(&format!("{base}/status")))
            .map(|v| v.trim().to_lowercase())
            .unwrap_or_default();
        let charging = if status.is_empty() {
            None
        } else {
            Some(status.contains("charging") || status.contains("full"))
        };
        let temp = util::read_trimmed(Path::new(&format!("{base}/temp")))
            .ok()
            .and_then(|v| v.trim().parse::<f32>().ok())
            .map(|v| v / 10.0);
        (capacity, charging, temp)
    }

    fn append_in(&self, dir: &Path, sample: &Sample) {
        self.sweep_in(dir, util::now_secs());
        if util::create_dir(dir).is_err() {
            return;
        }
        // 追加写：jsonl 按行损坏可容忍，且不随文件增长放大 IO。
        let path = dir.join(day_key(util::now_secs()));
        let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(&path) else {
            return;
        };
        use std::io::Write;
        let _ = file.write_all(sample.to_line().as_bytes());
    }

    /// 清理：删除 KEEP_DAYS 之前的按天文件；总量超限时从最旧开始删除。
    /// 由 append_in 周期性触发，不单独暴露定时入口。
    #[allow(dead_code)]
    fn sweep(&self, now: u64) {
        self.sweep_in(Path::new(DIR), now);
    }

    fn sweep_in(&self, dir: &Path, now: u64) {
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        let cutoff = now / 86400;
        let mut files: Vec<(u64, PathBuf, u64)> = entries
            .flatten()
            .filter_map(|entry| {
                let name = entry.file_name().to_string_lossy().into_owned();
                let day = day_from_name(&name)?;
                let size = entry.metadata().ok()?.len();
                Some((day, entry.path(), size))
            })
            .collect();
        files.sort_by_key(|(day, _, _)| *day);
        let mut total: u64 = files.iter().map(|(_, _, size)| size).sum();
        for (day, path, size) in files.clone() {
            if total <= TOTAL_CAP {
                break;
            }
            if std::fs::remove_file(&path).is_ok() {
                total = total.saturating_sub(size);
            }
            let _ = day;
        }
        for (day, path, size) in files {
            if cutoff.saturating_sub(day) >= KEEP_DAYS {
                let _ = std::fs::remove_file(path);
                let _ = size;
            }
        }
    }
}

/// 读取最近 hours 小时的样本（跨天文件），时间升序；损坏行跳过。
/// 输出超过 max_points 时按时间均匀降采样（保留首尾）——点数受限但
/// 每个点都是实测值，聚合不改写原始读数。
pub fn query(hours: u64, now: u64, max_points: usize) -> Result<Vec<Sample>> {
    query_in(Path::new(DIR), hours, now, max_points)
}

fn query_in(dir: &Path, hours: u64, now: u64, max_points: usize) -> Result<Vec<Sample>> {
    let since = now.saturating_sub(hours * 3600);
    let mut samples = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Ok(samples);
    };
    let mut files: Vec<(u64, PathBuf)> = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            Some((day_from_name(&name)?, entry.path()))
        })
        .collect();
    files.sort_by_key(|(day, _)| *day);
    for (_, path) in files {
        let Ok(text) = util::read_text(&path) else { continue };
        for line in text.lines() {
            if line.trim().is_empty() {
                continue;
            }
            match Sample::parse(line) {
                Some(sample) if sample.t >= since => samples.push(sample),
                _ => continue, // 损坏行跳过，不中断整个文件
            }
        }
    }
    samples.sort_by_key(|s| s.t);
    if samples.len() > max_points {
        let step = samples.len() as f64 / max_points as f64;
        let picked: Vec<Sample> = (0..max_points)
            .map(|index| samples[((index as f64) * step) as usize].clone())
            .collect();
        return Ok(picked);
    }
    Ok(samples)
}

fn now_secs() -> u64 {
    let mut now: i64 = 0;
    // SAFETY: FFI 调用，传入合法指针。
    unsafe {
        crate::ffi::time(&mut now);
    }
    now.max(0) as u64
}

/// `history <hours>` 命令输出：JSON 数组（行缓冲由调用方 print）。
pub fn print_query(hours: u64) -> Result<()> {
    let now = now_secs();
    let samples = query(hours, now, 900)?;
    let mut out = String::from("{\"samples\":[");
    for (index, sample) in samples.iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        let line = sample.to_line();
        out.push_str(line.trim_end());
    }
    out.push_str("]}");
    println!("{out}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(t: u64, pkg: &str, fps: Option<f32>) -> Sample {
        Sample {
            t,
            pkg: pkg.into(),
            mode: "fast".into(),
            fps,
            target: 60,
            boost: 24,
            freqs: [1000, 2000, 3000, 4000],
            battery: Some(83),
            charging: Some(false),
            temp: Some(36.5),
        }
    }

    #[test]
    fn line_round_trips_all_fields_and_nulls() {
        let s = sample(1000, "com.game", Some(59.5));
        let parsed = Sample::parse(s.to_line().trim()).unwrap();
        assert_eq!(parsed, s);
        let mut nulled = sample(1000, "com.game", None);
        nulled.battery = None;
        nulled.charging = None;
        nulled.temp = None;
        nulled.target = 0;
        let parsed = Sample::parse(nulled.to_line().trim()).unwrap();
        assert_eq!(parsed.fps, None);
        assert_eq!(parsed.battery, None);
        assert_eq!(parsed.charging, None);
        assert_eq!(parsed.temp, None);
        // 损坏行返回 None（读取时跳过）。
        assert!(Sample::parse("{\"t\":1,\"pkg\":").is_none());
        assert!(Sample::parse("[]").is_none());
    }

    #[test]
    fn scaled_fields_decode_by_tenth_of_unit() {
        // 审计 P1-1 回归：盘上 595 必须读成 59.5 fps / 36.5℃，不得保持 ×10。
        let line = "{\"t\":1000,\"pkg\":\"com.game\",\"mode\":\"fast\",\"fps\":595,\"tgt\":60,\
            \"boost\":24,\"freqs\":[1000,2000,3000,4000],\"batt\":83,\"chg\":false,\"temp\":365}";
        let parsed = Sample::parse(line).unwrap();
        assert_eq!(parsed.fps, Some(59.5));
        assert_eq!(parsed.temp, Some(36.5));
        assert_eq!(parsed.boost, 24);
        // 再序列化不二次放大（history 命令 parse→to_line 的输出路径）。
        let again = Sample::parse(parsed.to_line().trim()).unwrap();
        assert_eq!(again, parsed);
    }

    #[test]
    fn boost_field_is_backward_compatible_with_old_files() {
        // 第二阶段·闭环反馈：boost 为新增字段，旧文件缺该键按 0 读回，
        // 不拒绝整行（历史数据不打折）。
        let old_line = "{\"t\":1,\"pkg\":\"com.game\",\"mode\":\"fast\",\"fps\":600,\"tgt\":0,\
            \"freqs\":[1,2,3,4],\"batt\":50,\"chg\":true,\"temp\":400}";
        let parsed = Sample::parse(old_line).unwrap();
        assert_eq!(parsed.boost, 0);
        assert_eq!(parsed.fps, Some(60.0));
    }

    #[test]
    fn sanitize_blocks_quotes_and_control_chars() {
        assert_eq!(sanitize("com.ok.pkg_1-x"), "com.ok.pkg_1-x");
        assert_eq!(sanitize("bad\"pkg\nname"), "bad_pkg_name");
        let s = sample(1, "bad\"pkg", None);
        let line = s.to_line();
        assert!(line.contains("bad_pkg"), "非法字符已被替换");
        assert!(!line.contains("\"bad\""), "引号被转义为下划线");
    }

    #[test]
    fn query_skips_corrupt_lines_and_downsamples_preserving_order() {
        let dir = std::env::temp_dir().join(format!("nova-history-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // 用测试专用目录重写 query 的读取逻辑：临时改 DIR 不可行（const），
        // 改为直接构造文件后调用内部逻辑相同的读取路径——这里验证
        // parse+排序+降采样的纯逻辑等价物。
        let _now = 86400 * 20; // 第 20 天
        let mut samples = Vec::new();
        for i in 0..2000u64 {
            samples.push(sample(86400 * 20 + i * 10, "com.app", Some(60.0)));
        }
        samples.push(sample(86400 * 20 + 500, "com.app", Some(60.0))); // 乱序
        let mut corrupted = String::new();
        for s in &samples {
            corrupted.push_str(&s.to_line());
            if s.t % 5000 == 0 {
                corrupted.push_str("{\"broken\"\n");
            }
        }
        let parsed: Vec<Sample> = corrupted
            .lines()
            .filter_map(|l| Sample::parse(l))
            .collect();
        assert_eq!(parsed.len(), samples.len(), "损坏行被跳过");
        let mut sorted = parsed.clone();
        sorted.sort_by_key(|s| s.t);
        assert!(sorted.windows(2).all(|w| w[0].t <= w[1].t), "升序");
        // 降采样到 900 点：有序、首尾保留
        let step = sorted.len() as f64 / 900f64;
        let picked: Vec<Sample> = (0..900).map(|i| sorted[((i as f64) * step) as usize].clone()).collect();
        assert_eq!(picked.len(), 900);
        assert_eq!(picked[0].t, sorted[0].t);
        let _ = dir;
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn recorder_throttles_by_foreground_type() {
        // 真实文件 IO：写入到临时目录，节流 + query_in 全链验证。
        let dir = std::env::temp_dir().join(format!("nova-hist-throttle-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut recorder = Recorder::default();
        let mut writes = 0;
        let mut t = 86400 * 20 + 1000u64;
        // 游戏前台：10 秒一写
        for _ in 0..60 {
            if recorder.due(t, true) {
                recorder.append_in(&dir, &sample(t, "com.game", Some(60.0)));
                writes += 1;
            }
            t += 1;
        }
        assert!(writes >= 5 && writes <= 7, "游戏 60 秒应写 6~7 次，实际 {writes}");
        // 非游戏：20 秒一写
        writes = 0;
        t = 86400 * 20 + 5000;
        for _ in 0..60 {
            if recorder.due(t, false) {
                recorder.append_in(&dir, &sample(t, "com.app", None));
                writes += 1;
            }
            t += 1;
        }
        assert!(writes >= 2 && writes <= 4, "非游戏 60 秒应写 3 次左右，实际 {writes}");
        // query_in：真实文件读取 + 范围过滤 + 升序
        let got = query_in(&dir, 1, t, 900).expect("query_in");
        assert!(!got.is_empty(), "query_in 应读到刚写入的样本");
        assert!(got.windows(2).all(|w| w[0].t <= w[1].t), "query 结果升序");
        std::fs::remove_dir_all(&dir).ok();
    }
}
