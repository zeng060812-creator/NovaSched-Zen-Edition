//! ZenJooo · 按需健康检查：聚合现有只读检查，不做任何修改。
//!
//! 每个子项输出 status（ok / unsupported / permission / unavailable /
//! failed / unknown）、证据与下一步。默认全部只读；“修复”动作只有明确
//! 列出的项才提供，且不触碰温控、其它模块或共享系统配置。
use crate::config::Config;
use crate::util::{self, Result, STATE_DIR};
use std::path::Path;

fn section(name: &str, status: &str, evidence: &str, next: &str) {
    println!("[{name}] status={status}\n{evidence}next={next}\n");
}

pub fn run(module_dir: &Path) -> Result<()> {
    if unsafe { crate::ffi::geteuid() } != 0 {
        return Err("健康检查需要 root（只读，不修改任何配置）".into());
    }
    println!(
        "NovaSched Zen Edition {} 健康检查\ntime={}\n全部子项只读；本命令不修改温控、其它模块或共享系统配置\n",
        env!("CARGO_PKG_VERSION"),
        util::local_timestamp()
    );

    // 守护进程与版本。
    let status_text = match crate::daemon::health(module_dir) {
        Ok(text) => {
            let running = text.contains("running=true") && text.contains("verified=true");
            let phase = text
                .lines()
                .find_map(|line| line.strip_prefix("phase="))
                .unwrap_or("unknown")
                .to_string();
            let error_line = text
                .lines()
                .find(|line| line.starts_with("error="))
                .unwrap_or("")
                .trim()
                .to_string();
            section(
                "daemon",
                if running && phase == "ready" { "ok" } else if running { "unavailable" } else { "failed" },
                format!("phase={phase}\n{error_line}\n").as_str(),
                if running && phase == "ready" { "无需处理" } else { "WebUI 状态卡 → 启动服务，或执行 novasched start" },
            );
            text
        }
        Err(error) => {
            section(
                "daemon",
                "failed",
                format!("{error}\n").as_str(),
                "执行 novasched start；反复失败时执行 novasched diagnose",
            );
            String::new()
        }
    };

    // 配置与应用规则。
    let config_verdict = (|| -> Result<String> {
        let config = Config::load(Path::new(util::CONFIG_PATH))?;
        let rules = crate::app_modes::rule_count(Path::new(util::APP_MODES_PATH))?;
        Ok(format!(
            "config_version={} profile_soc={:?} policy={},{},{},{} rules={}\n",
            config.meta.version,
            config.meta.profile_soc,
            config.policy[0],
            config.policy[1],
            config.policy[2],
            config.policy[3],
            rules
        ))
    })();
    match config_verdict {
        Ok(evidence) => section("config", "ok", &evidence, "无需处理"),
        Err(error) => section(
            "config",
            "failed",
            format!("{error}\n").as_str(),
            "启动时守护会自动回滚到上一份可用配置；也可在 WebUI 重新选择处理器模板",
        ),
    }

    // 处理器识别与拓扑。
    match crate::hardware::detect() {
        Ok(hardware) => section(
            "soc",
            "ok",
            format!(
                "soc={} evidence={} policies={}\n",
                hardware.soc.name(),
                hardware.evidence,
                hardware
                    .policies
                    .iter()
                    .map(|p| format!("policy{}({}核)", p.id, p.cpus.len()))
                    .collect::<Vec<_>>()
                    .join(" ")
            )
            .as_str(),
            "无需处理",
        ),
        Err(error) => section(
            "soc",
            "failed",
            format!("{error}\n").as_str(),
            "确认设备是否在支持列表（docs/processor-adaptation.md）",
        ),
    }

    // 调度请求 vs 实际生效。
    let pending = status_text
        .lines()
        .find_map(|line| line.strip_prefix("pending_controls="))
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(0);
    section(
        "controls",
        if pending == 0 { "ok" } else { "unavailable" },
        format!("pending_controls={pending}（温控/厂商服务占用的上限会在 WebUI 显示原因）\n").as_str(),
        if pending == 0 { "无需处理" } else { "game-diagnose 的 vendor-limit 段可确认限制来源；本模块不与温控抢占" },
    );

    // Scene 与 fas-rs。
    let presence = crate::scene::presence();
    let scene_state = crate::scene::detect();
    section(
        "scene",
        match presence.availability {
            crate::scene::SceneAvailability::Available => "ok",
            crate::scene::SceneAvailability::Absent => "unsupported",
            crate::scene::SceneAvailability::Unknown => "unknown",
        },
        format!("{} controller={} linked={} locked={}\n", presence.evidence, scene_state.controller(), scene_state.linked(), scene_state.controls_locked()).as_str(),
        "无需处理；接管关系随 Scene 配置自动协商",
    );
    let fas = crate::fas::inspect(Path::new("/"));
    section(
        "fas",
        if fas.available() { "ok" } else { "unsupported" },
        format!("{}\n", fas.report().replace('\n', " ")).as_str(),
        "无需处理；fas-rs 未安装时 NovaFAS 独立工作",
    );

    // WebUI 桥接。
    let port = status_text
        .lines()
        .find_map(|line| line.strip_prefix("port="))
        .map(str::to_string)
        .unwrap_or_else(|| "unknown".into());
    section(
        "webui",
        if port != "unknown" && !port.is_empty() { "ok" } else { "unknown" },
        format!("port={port}\n").as_str(),
        "若页面无法连接：锁屏解锁后重开 WebUI，或查看 novasched.log",
    );

    // 触感能力（只读探测，不振动）。
    match crate::haptics::capability() {
        Ok(report) => section(
            "haptics",
            "ok",
            format!("{report}\n").as_str(),
            "无需处理；可在 WebUI 外观设置使用「测试触感」",
        ),
        Err(error) => section(
            "haptics",
            "unsupported",
            format!("{error}\n").as_str(),
            "WebUI 将回退到设备振动；无需处理",
        ),
    }

    // 图形后端规则。
    println!("[graphics]");
    print!("{}", crate::graphics::status());
    println!("next=安装 ANGLE 驱动包后可用；见 graphics-status\n");

    // 应用名元数据：由 WebUI 宿主桥（getPackagesInfo）承担，本命令无法
    // 探测宿主能力，以规则列表实际显示为准。
    section(
        "app-metadata",
        "unknown",
        "应用名称读取由 WebUI 宿主桥承担（getPackagesInfo），守护进程侧无法探测宿主能力\n",
        "以规则列表实际显示为准；图标与名称的 app_process 辅助为已记录边界（docs/v2.12.1-report.md）",
    );

    // 历史轨迹。
    let history_dir = Path::new("/data/adb/novasched/history");
    let history_files = std::fs::read_dir(history_dir)
        .map(|entries| entries.flatten().count())
        .unwrap_or(0);
    section(
        "history",
        if history_files > 0 { "ok" } else { "unavailable" },
        format!("history_files={history_files}（按天滚动，保留 7 天 / 8MB 上限；本机数据不上传）\n").as_str(),
        if history_files > 0 { "WebUI「历史」页查看；novasched history <小时> 导出 JSON" } else { "尚无轨迹：模块运行一段时间后自动生成" },
    );

    // 温度（只读）。
    let thermal = crate::game_diagnostics::vendor_limit_report(Path::new("/"));
    let thermal_line = thermal
        .lines()
        .find(|line| line.contains("sconfig="))
        .unwrap_or("thermal sconfig 不可读（只读采集失败时如实留空）");
    section(
        "thermal",
        "ok",
        format!("{thermal_line}\n（只读；本模块不修改温控）\n").as_str(),
        "无需处理",
    );
    let _ = STATE_DIR;
    Ok(())
}
