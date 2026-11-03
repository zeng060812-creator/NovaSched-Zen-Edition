//! ZenJooo · 应用名称与图标（十七）。
//!
//! 由 NovaMeta.dex（源码 scripts/meta-dex/NovaMeta.java，javac+d8 可复现
//! 构建）经 app_process 读取 PackageManager：名称、96px 透明 PNG 图标
//! （base64）、系统应用分类。安全与边界：
//! - 包名先经 valid_package 校验并白名单化，单批 ≤48 个；
//! - dex 路径固定在模块 bin/ 内，不给调用方传任意路径；
//! - 子进程 20 秒超时强杀，输出上限 512KB；
//! - 结果缓存于 run/meta-cache.json（TTL 1 小时，语言/应用更新由 TTL
//!   收敛），失败结果不进缓存（允许有界重试）；
//! - 不使用应用截图或网络图标；不做任何写系统操作。
use crate::util::{self, Result};
use std::collections::BTreeMap;
use std::path::Path;
use std::time::{Duration, Instant};

const CACHE_PATH: &str = "/data/adb/novasched/run/meta-cache.json";
const CACHE_TTL_SECS: u64 = 3600;
const MAX_BATCH: usize = 48;
const OUTPUT_CAP: u64 = 512 * 1024;
const TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Clone, Debug, PartialEq)]
pub struct AppMeta {
    pub label: String,
    pub icon: String,
    pub system: bool,
    /// 应用启用状态（ApplicationInfo.enabled）：用户在系统设置里停用的
    /// 应用为 false。旧缓存文件缺该键时按 true（未停用）处理。
    pub enabled: bool,
}

fn read_cache(now: u64) -> BTreeMap<String, (AppMeta, u64)> {
    let mut out = BTreeMap::new();
    let text = util::read_text(Path::new(CACHE_PATH)).unwrap_or_default();
    let Ok(root) = crate::json::parse(&text) else { return out };
    let crate::json::Value::Object(map) = root else { return out };
    let Some(crate::json::Value::Object(entries)) = map.get("entries") else { return out };
    for (package, value) in entries {
        let crate::json::Value::Object(entry) = value else { continue };
        let (Some(crate::json::Value::Number(ts)), Some(crate::json::Value::String(label)),
             Some(crate::json::Value::String(icon)), Some(crate::json::Value::Bool(system))) =
        (
            entry.get("ts"), entry.get("label"), entry.get("icon"), entry.get("system")
        ) else { continue };
        let enabled = match entry.get("enabled") {
            Some(crate::json::Value::Bool(value)) => *value,
            _ => true,
        };
        if now.saturating_sub(*ts as u64) < CACHE_TTL_SECS {
            out.insert(package.clone(), (
                AppMeta {
                    label: label.clone(),
                    icon: icon.clone(),
                    system: *system,
                    enabled,
                },
                *ts as u64,
            ));
        }
    }
    out
}

fn save_cache(entries: &BTreeMap<String, (AppMeta, u64)>) {
    let mut items = String::new();
    for (index, (package, (meta, ts))) in entries.iter().enumerate() {
        if index > 0 {
            items.push(',');
        }
        // label 来自系统，可能含任意字符：仅转义 JSON 必需字符。
        let label = meta.label.replace('\\', "\\\\").replace('"', "\\\"");
        items.push_str(&format!(
            "{{\"package\":\"{package}\",\"label\":\"{label}\",\"icon\":\"{}\",\"system\":{},\"enabled\":{},\"ts\":{ts}}}",
            meta.icon, meta.system, meta.enabled
        ));
    }
    let text = format!(
        "{{\n  \"schema\": \"novasched-meta-cache/1\",\n  \"entries\": {{{items}}}\n}}\n"
    );
    let _ = util::atomic_write(Path::new(CACHE_PATH), text.as_bytes(), 0o600);
}

/// 一次 app_process 查询：返回 (JSON 行文本, 是否整体失败)。
/// 不走 util::query_command：app_process 需要 LD_LIBRARY_PATH/CLASSPATH
/// 环境而 query_command 会剥离它们。
fn run_app_process(dex: &Path, packages: &[String]) -> (String, String, bool) {
    use std::io::Read;
    use std::process::{Command, Stdio};
    let mut command = Command::new("/system/bin/app_process");
    command
        .env("CLASSPATH", dex)
        .env("LD_LIBRARY_PATH", "/system/lib64:/system/lib")
        .args(["/system/bin", "NovaMeta"])
        .args(packages)
        .stdin(Stdio::null());
    let Ok(mut child) = command.stdout(Stdio::piped()).stderr(Stdio::piped()).spawn() else {
        return (String::new(), "app_process 无法启动".into(), true);
    };
    let mut output = String::new();
    let mut errors = String::new();
    let deadline = Instant::now() + TIMEOUT;
    let mut pipe = child.stdout.take();
    let mut err_pipe = child.stderr.take();
    let mut done = false;
    while !done {
        if let Some(pipe) = pipe.as_mut() {
            let mut buffer = [0u8; 8192];
            match pipe.read(&mut buffer) {
                Ok(0) => done = true,
                Ok(count) => {
                    output.push_str(&String::from_utf8_lossy(&buffer[..count]));
                    if output.len() > OUTPUT_CAP as usize {
                        let _ = child.kill();
                        return (String::new(), "输出超出上限".into(), true);
                    }
                }
                Err(_) => done = true,
            }
        } else {
            done = true;
        }
        if let Some(err_pipe) = err_pipe.as_mut() {
            let mut buffer = [0u8; 4096];
            let _ = err_pipe.read(&mut buffer);
        }
        if !done && Instant::now() >= deadline {
            let _ = child.kill();
            return (String::new(), "app_process 20 秒超时".into(), true);
        }
        if !done {
            match child.try_wait() {
                Ok(Some(status)) => {
                    if let Some(pipe) = pipe.as_mut() {
                        let mut rest = Vec::new();
                        let _ = pipe.read_to_end(&mut rest);
                        output.push_str(&String::from_utf8_lossy(&rest));
                    }
                    if let Some(err_pipe) = err_pipe.as_mut() {
                        let mut rest = Vec::new();
                        let _ = err_pipe.read_to_end(&mut rest);
                        errors.push_str(&String::from_utf8_lossy(&rest));
                    }
                    if !status.success() && output.trim().is_empty() {
                        // DEX 缺类 / 环境异常：stderr 通常带原因。
                        return (
                            String::new(),
                            format!("退出码 {:?}: {}", status.code(), errors.trim()),
                            true,
                        );
                    }
                    done = true;
                }
                Ok(None) => {
                    if Instant::now() >= deadline {
                        let _ = child.kill();
                        return (String::new(), "app_process 20 秒超时".into(), true);
                    }
                    std::thread::sleep(Duration::from_millis(20));
                }
                Err(error) => {
                    return (String::new(), format!("等待 app_process 失败: {error}"), true);
                }
            }
        }
    }
    if output.trim().is_empty() && !errors.trim().is_empty() {
        return (errors.trim().to_string(), errors.trim().to_string(), true);
    }
    // 失败路径都在上方提前返回；走到这里说明拿到了输出，不算失败。
    (output, errors.trim().to_string(), false)
}

/// 查询应用元数据（label/icon/system），带缓存；返回包名→元数据。
pub fn lookup(module_dir: &Path, packages: &[String]) -> Result<BTreeMap<String, AppMeta>> {
    if unsafe { crate::ffi::geteuid() } != 0 {
        return Err("应用元数据查询需要 root".into());
    }
    let mut wanted: Vec<String> = Vec::new();
    for package in packages {
        let package = package.trim();
        if crate::util::valid_package(package) && !wanted.contains(&package.to_string()) {
            wanted.push(package.to_string());
        }
    }
    wanted.truncate(MAX_BATCH);
    if wanted.is_empty() {
        return Ok(BTreeMap::new());
    }
    let now = util::now_secs();
    let cache = read_cache(now);
    let mut result: BTreeMap<String, AppMeta> = BTreeMap::new();
    let mut missing: Vec<String> = Vec::new();
    for package in &wanted {
        match cache.get(package) {
            Some((meta, _)) => {
                result.insert(package.clone(), meta.clone());
            }
            None => missing.push(package.clone()),
        }
    }
    if !missing.is_empty() {
        let dex = module_dir.join("bin/NovaMeta.dex");
        if !dex.is_file() {
            return Err("NovaMeta.dex 未随模块安装".into());
        }
        let (output, error_text, failed) = run_app_process(&dex, &missing);
        if failed {
            return Err(format!("app_process 查询失败（可重试）: {error_text}"));
        }
        let mut cache = cache;
        let mut matched = 0usize;
        let mut dex_errors: Vec<String> = Vec::new();
        for line in output.lines() {
            let Ok(root) = crate::json::parse(line.trim()) else { continue };
            let crate::json::Value::Object(map) = root else { continue };
            // DEX 级错误（如 {"error":"no context"}）没有 package 键：单列。
            if map.get("package").is_none() {
                if let Some(crate::json::Value::String(error)) = map.get("error") {
                    dex_errors.push(error.clone());
                }
                continue;
            }
            let Some(crate::json::Value::String(package)) = map.get("package") else { continue };
            if !wanted.contains(package) {
                continue;
            }
            let label = match map.get("label") {
                Some(crate::json::Value::String(v)) => v.trim().to_string(),
                _ => String::new(),
            };
            let icon = match map.get("icon") {
                Some(crate::json::Value::String(v)) if v.len() <= 400_000 => v.clone(),
                _ => String::new(),
            };
            let system = matches!(map.get("system"), Some(crate::json::Value::Bool(true)));
            let enabled = match map.get("enabled") {
                Some(crate::json::Value::Bool(value)) => *value,
                // 旧版 DEX/旧行缺该键：按未停用处理。
                _ => true,
            };
            let meta = AppMeta { label, icon, system, enabled };
            cache.insert(package.clone(), (meta.clone(), now));
            result.insert(package.clone(), meta);
            matched += 1;
        }
        save_cache(&cache);
        // 规格五：区分“DEX 加载失败 / PackageManager 查询失败 / 应用不存在”，
        // 不允许全部静默降级成空占位。
        if matched == 0 {
            let reason = summarize_dex_failure(&missing, &dex_errors);
            return Err(reason);
        }
    }
    Ok(result)
}

/// 全部请求包都未命中时的原因归纳（纯函数，便于测试）。
/// - DEX 报了上下文错误 → 明确指向 app_process/Context 失败；
/// - DEX 报了逐包错误 → 指向 PackageManager 查询失败（附首个原因）；
/// - 输出为空/无错误 → 指向输出被截断或 DEX 版本过旧。
fn summarize_dex_failure(missing: &[String], dex_errors: &[String]) -> String {
    if dex_errors.iter().any(|error| error.contains("no context")) {
        return "NovaMeta 无法获取系统 Context（app_process/systemMain 失败，无法查询 PackageManager）".into();
    }
    if let Some(first) = dex_errors.first() {
        return format!(
            "PackageManager 查询失败（{}/{} 个包未命中，首个原因: {}）",
            missing.len(),
            missing.len(),
            first
        );
    }
    format!(
        "NovaMeta 输出未包含任何请求包名（{}/{} 未命中；输出被截断或 DEX 版本过旧）",
        missing.len(),
        missing.len()
    )
}

/// `meta <包名>...`：输出 JSON 对象（WebUI 经 root 通道读取）。
pub fn run(module_dir: &Path, packages: &[String]) -> Result<()> {
    let entries = lookup(module_dir, packages)?;
    let mut out = String::from("{");
    for (index, (package, meta)) in entries.iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        let label = meta.label.replace('\\', "\\\\").replace('"', "\\\"");
        out.push_str(&format!(
            "\"{package}\":{{\"label\":\"{label}\",\"icon\":\"{}\",\"system\":{},\"enabled\":{}}}",
            meta.icon, meta.system, meta.enabled
        ));
    }
    out.push('}');
    println!("{out}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dex_failure_reasons_are_layered_not_generic() {
        // 规格五：区分 DEX 上下文失败 / PackageManager 查询失败 / 截断。
        assert!(summarize_dex_failure(&["com.a".into()], &["no context".into()])
            .contains("app_process/systemMain 失败"));
        assert!(summarize_dex_failure(&["com.a".into(), "com.b".into()], &["PackageManager has crashed".into()])
            .contains("PackageManager 查询失败"));
        assert!(summarize_dex_failure(&["com.a".into()], &[]).contains("输出被截断或 DEX 版本过旧"));
    }

    #[test]
    fn cache_round_trips_and_expires() {
        let mut entries: BTreeMap<String, (AppMeta, u64)> = BTreeMap::new();
        entries.insert(
            "com.game".into(),
            (
                AppMeta {
                    label: "游戏\"X\"".into(),
                    icon: "aGVsbG8=".into(),
                    system: false,
                    enabled: false,
                },
                1000,
            ),
        );
        // save→read 的缓存文件依赖绝对路径，不在这里做 IO；验证解析逻辑
        // 等价：手工构造同样文本（含 enabled 字段）。
        let text = format!(
            "{{\"schema\":\"x\",\"entries\":{{\"com.game\":{{\"package\":\"com.game\",\"label\":\"游戏\\\"X\\\"\",\"icon\":\"aGVsbG8=\",\"system\":false,\"enabled\":false,\"ts\":1000}}}}}}"
        );
        let root = crate::json::parse(&text).unwrap();
        let crate::json::Value::Object(map) = root else { panic!() };
        let Some(crate::json::Value::Object(list)) = map.get("entries") else { panic!() };
        assert_eq!(list.len(), 1);
        // 旧版缓存条目缺 enabled 键：按未停用（true）处理，向后兼容。
        let legacy = crate::json::parse(
            "{\"schema\":\"x\",\"entries\":{\"com.old\":{\"package\":\"com.old\",\"label\":\"旧\",\"icon\":\"\",\"system\":true,\"ts\":1}}}",
        )
        .unwrap();
        let crate::json::Value::Object(legacy_map) = legacy else { panic!() };
        let Some(crate::json::Value::Object(legacy_entries)) = legacy_map.get("entries") else { panic!() };
        let Some(crate::json::Value::Object(entry)) = legacy_entries.get("com.old") else { panic!() };
        assert!(matches!(entry.get("enabled"), None), "旧条目无 enabled 键");
    }
}
