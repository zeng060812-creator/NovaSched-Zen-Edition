//! 信息流视频应用锁帧：前台命中清单内应用时，把屏幕刷新率限制到
//! fps Hz；应用切走或守护停止时恢复原值。
//!
//! 通道优先级：
//! 1. MI mi_display sysfs 节点（HyperOS/MIUI 面板直控，探测候选列表取
//!    第一个可写节点，写入前保存原值）；
//! 2. HyperOS/AOSP 全局刷新率 settings（min + peak 双写，root 下用
//!    `settings` 命令，恢复时对原值为 null 的键执行 delete）。
//! 两个通道都不可用时功能如实失效（只记一条日志），不猜测节点。
use crate::logging::Logger;

const NODE_BASE: &str = "/sys/class/mi_display";
const NODE_CANDIDATES: &[&str] = &[
    "disp-DSI-0/max_fps_clone",
    "dsi-0-mi_display/max_fps_clone",
    "disp-DSI-0/dynamic_fps",
    "dsi-0-mi_display/dynamic_fps",
];
const SETTING_KEYS: &[&str] = &["min_refresh_rate", "peak_refresh_rate"];

#[derive(Default)]
pub struct FeedFpsState {
    pub engaged: bool,
    node: Option<(std::path::PathBuf, String)>,
    settings: Vec<(&'static str, String)>,
}

/// 崩溃恢复记录：engage 成功后把原始值落盘，守护异常死亡（SIGKILL 且监督
/// 重启失败）后，下次启动按记录恢复屏幕刷新率，避免永久锁在低刷新率。
fn saved_path() -> std::path::PathBuf {
    crate::storage::path(std::path::Path::new(crate::util::STATE_DIR), "feed-fps.saved")
}

fn persist(state: &FeedFpsState, logger: &Logger) {
    let mut text = String::new();
    if let Some((node, original)) = &state.node {
        text.push_str(&format!("node\t{}\t{}\n", node.display(), original));
    }
    for (key, value) in &state.settings {
        text.push_str(&format!("setting\t{key}\t{value}\n"));
    }
    if let Err(error) = crate::util::atomic_write(&saved_path(), text.as_bytes(), 0o600) {
        logger.warn(format!("信息流锁帧的恢复记录写入失败: {error}"));
    }
}

fn discard_saved() {
    let _ = crate::util::remove_if_exists(&saved_path());
}

/// 守护启动时调用：若上次会话遗留锁帧记录（进程被杀没来得及恢复），
/// 按记录恢复原值并删除记录。无记录则什么都不做。
pub fn recover_pending(logger: &Logger) {
    let path = saved_path();
    let Ok(text) = crate::util::read_text(&path) else {
        return;
    };
    let mut restored = 0u32;
    for line in text.lines() {
        let mut fields = line.split('\t');
        match (fields.next(), fields.next(), fields.next()) {
            (Some("node"), Some(node), Some(original)) => {
                if std::fs::write(node, original.as_bytes()).is_ok() {
                    restored += 1;
                }
            }
            (Some("setting"), Some(key), Some(value)) => {
                if value == "null" || value.is_empty() {
                    let _ = crate::util::run_command("settings", &["delete", "system", key]);
                } else {
                    let _ = crate::util::run_command("settings", &["put", "system", key, value]);
                }
                restored += 1;
            }
            _ => {}
        }
    }
    discard_saved();
    if restored > 0 {
        logger.warn(format!(
            "检测到上次异常退出遗留的 {restored} 项信息流锁帧设置，已恢复原刷新率"
        ));
    }
}

pub fn engage(fps: u32, logger: &Logger) -> FeedFpsState {
    let value = fps.to_string();
    for candidate in NODE_CANDIDATES {
        let node = std::path::Path::new(NODE_BASE).join(candidate);
        let Ok(original) = crate::util::read_trimmed(&node) else {
            continue;
        };
        if original == value {
            let state = FeedFpsState {
                engaged: true,
                node: Some((node, original)),
                settings: Vec::new(),
            };
            persist(&state, logger);
            return state;
        }
        if std::fs::write(&node, value.as_bytes()).is_err() {
            continue;
        }
        if crate::util::read_trimmed(&node).ok().as_deref() != Some(value.as_str()) {
            continue;
        }
        logger.info(format!(
            "信息流锁帧：屏幕刷新率已限制为 {fps} Hz（{}）",
            node.display()
        ));
        let state = FeedFpsState {
            engaged: true,
            node: Some((node, original)),
            settings: Vec::new(),
        };
        persist(&state, logger);
        return state;
    }
    let mut saved = Vec::new();
    for key in SETTING_KEYS {
        let old = crate::util::run_command("settings", &["get", "system", key])
            .unwrap_or_default()
            .trim()
            .to_string();
        if crate::util::run_command("settings", &["put", "system", key, &value]).is_err() {
            logger.debug(format!("信息流锁帧：settings put {key} 失败，放弃 settings 通道"));
            restore_settings(&saved);
            return FeedFpsState::default();
        }
        saved.push((*key, old));
    }
    if saved.is_empty() {
        return FeedFpsState::default();
    }
    logger.info(format!("信息流锁帧：全局刷新率已限制为 {fps} Hz"));
    let state = FeedFpsState {
        engaged: true,
        node: None,
        settings: saved,
    };
    persist(&state, logger);
    state
}

pub fn release(state: &mut FeedFpsState, logger: &Logger) {
    if !state.engaged {
        return;
    }
    if let Some((node, original)) = state.node.take() {
        let _ = std::fs::write(&node, original.as_bytes());
    }
    restore_settings(&state.settings);
    state.settings.clear();
    state.engaged = false;
    discard_saved();
    logger.info("信息流锁帧已解除，刷新率恢复原值");
}

fn restore_settings(saved: &[(&'static str, String)]) {
    for (key, value) in saved {
        if value == "null" || value.is_empty() {
            let _ = crate::util::run_command("settings", &["delete", "system", key]);
        } else {
            let _ = crate::util::run_command("settings", &["put", "system", key, value]);
        }
    }
}
