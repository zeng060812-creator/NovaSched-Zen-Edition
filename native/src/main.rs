mod ai;
mod app_modes;
mod automatic;
mod boot_guard;
mod config;
mod config_v2;
mod daemon;
mod event_loop;
mod fas;
mod ffi;
mod fps_rules;
mod frame_ctrl;
mod game_diagnostics;
mod games;
mod graphics;
mod hardware;
mod health;
mod history;
mod input_boost;
mod installer;
mod json;
mod lifecycle;
mod logging;
mod meta;
mod package_registry;
mod process_identity;
mod profiles;
mod refresh;
mod root_bridge;
mod scene;
mod scheduler;
mod snapshot;
mod storage;
mod haptics;
mod soc;
mod telemetry;
mod uninstall;
mod util;
mod web_auth;
mod websocket;

use std::env;
use std::path::PathBuf;
use std::process;

use crate::util::Result;

fn main() {
    if let Err(error) = run() {
        eprintln!("novasched: {error}");
        process::exit(1);
    }
}

/// CLI 参数解析（纯函数，可单测）。
/// meta 的普通位置参数全部作为应用包名；--module-dir/--origin/--request
/// 与位置参数可任意交错。其他命令保持既有语义（首个位置参数 =
/// requested_mode，部分命令另有第二参数）。
#[derive(Default, Debug, PartialEq)]
struct ParsedArgs {
    requested_mode: Option<String>,
    second_arg: Option<String>,
    module_dir: Option<PathBuf>,
    webui_origin: Option<String>,
    webui_request: Option<String>,
    meta_packages: Vec<String>,
}

fn parse_args(command: &str, tokens: &[String]) -> Result<ParsedArgs> {
    const VALUE_ARG_COMMANDS: &[&str] = &[
        "scene-mode", "install-check", "haptic", "graphics-set", "graphics-remove",
        "fps-set", "fps-remove", "refresh-set", "refresh-remove", "game-add",
        "game-remove", "ai-mode", "ai-key", "ai-model", "ai-url", "ai-decide", "history",
    ];
    const SECOND_ARG_COMMANDS: &[&str] = &["graphics-set", "fps-set", "refresh-set", "ai-key"];
    let mut out = ParsedArgs::default();
    let mut iter = tokens.iter();
    if VALUE_ARG_COMMANDS.contains(&command) {
        out.requested_mode = iter.next().cloned();
    }
    if SECOND_ARG_COMMANDS.contains(&command) {
        out.second_arg = iter.next().cloned();
    }
    let option = |iter: &mut std::slice::Iter<String>, name: &str| -> Result<String> {
        iter.next()
            .cloned()
            .filter(|value| !value.starts_with("--"))
            .ok_or_else(|| format!("{name} 缺少值"))
    };    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--module-dir" => out.module_dir = Some(PathBuf::from(option(&mut iter, "--module-dir")?)),            "--origin" => out.webui_origin = Some(option(&mut iter, "--origin")?),
            "--request" => out.webui_request = Some(option(&mut iter, "--request")?),
            // meta：普通位置参数全部作为应用包名（不得提前消费，也不得
            // 吞掉后续 --module-dir——曾因预消费丢失首个包名并让模块目录
            // 回落默认值）。
            other if command == "meta" && !other.starts_with("--") => {
                out.meta_packages.push(other.to_string());
            }
            other => return Err(format!("未知参数: {other}\n{}", usage())),
        }
    }
    Ok(out)
}

fn run() -> Result<()> {
    let mut args = env::args().skip(1);
    let command = args.next().ok_or_else(|| usage().to_string())?;
    let tokens: Vec<String> = args.collect();
    let parsed = parse_args(&command, &tokens)?;
    let requested_mode = parsed.requested_mode;
    let graphics_backend = parsed.second_arg;
    let meta_packages = parsed.meta_packages;
    let module_dir = match parsed.module_dir {
        Some(path) => path,
        None => util::default_module_dir(),
    };
    let webui_origin = parsed.webui_origin;
    let webui_request = parsed.webui_request;

    if module_dir.join("disable").exists()
        && matches!(command.as_str(), "start" | "restart" | "supervise" | "daemon" | "reload" | "scene-mode")
    {
        return Err("模块已停用，请先查看启动保护和诊断记录".into());
    }

    if (!module_dir.is_dir()
        || module_dir.join(".uninstalling").exists()
        || module_dir.join("remove").exists())
        && matches!(
            command.as_str(),
            "start"
                | "restart"
                | "supervise"
                | "daemon"
                | "reload"
                | "scene-mode"
                | "scene-install"
                | "prepare-config"
                | "prepare-auto"
                | "webui-session"
                | "webui-rpc"
        )
    {
        return Err("模块正在卸载，已停止启动、回调与配置写入".into());
    }
    if matches!(command.as_str(), "start" | "restart" | "supervise" | "daemon" | "scene-mode" | "prepare-config" | "prepare-auto" | "scene-install") { storage::ensure_ready(std::path::Path::new(util::STATE_DIR))?; }
    if matches!(command.as_str(), "start" | "restart") { if let Err(e)=storage::cleanup_legacy(&module_dir,std::path::Path::new(util::STATE_DIR)){eprintln!("novasched: 部分旧文件未清理，继续启动: {e}");} }
    match command.as_str() {
        "prepare-storage" => { println!("storage_deferred={}",!storage::prepare(std::path::Path::new(util::STATE_DIR))?); Ok(()) },
        "haptic" => haptics::run(requested_mode.as_deref().unwrap_or("")),
        "boot-ready" => boot_guard::ensure_ready(),
        "version" | "--version" => {
            println!("NovaSched Zen Edition {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        "start" => lifecycle::start(&module_dir, false),
        "restart" => lifecycle::start(&module_dir, true),
        "supervise" => lifecycle::supervise(&module_dir),
        "diagnose" => lifecycle::diagnose(&module_dir),
        "history" => {
            let hours = requested_mode.as_deref().and_then(|v| v.parse::<u64>().ok()).unwrap_or(24);
            history::print_query(hours)?;
            Ok(())
        }
        "health-check" => health::run(&module_dir),
        "fault-report" => lifecycle::fault_report(&module_dir),
        "release-control" => daemon::release_control(&module_dir),
        "game-diagnose" => game_diagnostics::run(&module_dir),
        "meta" => {
            if meta_packages.is_empty() {
                // 测试 D：无包名时安全返回空结果（不崩溃、不误查）。
                println!("{{}}");
                println!("meta_packages=0（未提供包名）");
                return Ok(());
            }
            meta::run(&module_dir, &meta_packages)
        }
        "webui-session" => web_auth::print_current(
            &module_dir,
            webui_origin
                .as_deref()
                .ok_or_else(|| "webui-session 需要 --origin".to_string())?,
        ),
        "webui-rpc" => root_bridge::run(
            &module_dir,
            webui_origin.as_deref().ok_or("webui-rpc 需要 --origin")?,
            webui_request.as_deref().ok_or("webui-rpc 需要 --request")?,
        ),
        "self-test" => lifecycle::self_test(&module_dir),
        "install-check" => installer::run(&module_dir, requested_mode.as_deref().unwrap_or("")),
        "scene-mode" => app_modes::scene_callback(requested_mode.as_deref().unwrap_or("")),
        "daemon" => daemon::run_daemon(module_dir),
        "reload" => daemon::send_signal(&module_dir, ffi::SIGHUP),
        "stop" => lifecycle::stop(&module_dir),
        "status" => daemon::print_status(&module_dir),
        "restore" => daemon::restore_stock(&module_dir),
        "uninstall" => uninstall::run(&module_dir),
        "scene-install" => {
            println!("{}", scene::install_provider(&module_dir)?);
            Ok(())
        }
        "scene-restore" => {
            println!("{}", scene::restore_provider()?);
            Ok(())
        }
        "probe" => daemon::probe(&module_dir),
        "check-config" => daemon::check_config(&module_dir),
        "prepare-config" => {
            let hardware = scheduler::preflight()?;
            println!(
                "{}",
                profiles::initialize(
                    &module_dir,
                    &hardware,
                    std::path::Path::new(util::STATE_DIR)
                )?
            );
            Ok(())
        }
        "prepare-auto" => {
            let automatic =
                automatic::Automatic::initialize(std::path::Path::new(util::AUTO_POLICY_PATH))?;
            println!("automatic_enabled={}", automatic.enabled());
            Ok(())
        }
        "scene-status" => {
            let presence = scene::presence();
            let status = match presence.availability {
                scene::SceneAvailability::Available => "available",
                scene::SceneAvailability::Absent => "absent",
                scene::SceneAvailability::Unknown => "unknown",
            };
            println!(
                "scene_presence={status}\nscene_detection={}",
                presence.evidence
            );
            Ok(())
        }
        "fas-diagnose" => fas::diagnose(),
        "graphics-status" => {
            print!("{}", graphics::status());
            Ok(())
        }
        "fps-status" => {
            print!("{}", fps_rules::status());
            Ok(())
        }
        "fps-set" => {
            let package = requested_mode.as_deref().ok_or("fps-set 需要 <包名>")?;
            let fps = graphics_backend.as_deref().ok_or("fps-set 需要 <目标FPS>")?;
            let value = fps.parse::<u32>().map_err(|_| format!("目标 FPS 无效: {fps}"))?;
            fps_rules::set(package, value)?;
            println!("{}", fps_rules::status());
            Ok(())
        }
        "fps-remove" => {
            let package = requested_mode.as_deref().ok_or("fps-remove 需要 <包名>")?;
            fps_rules::set(package, 0)?;
            println!("{}", fps_rules::status());
            Ok(())
        }
        // ZenJooo · 按应用刷新率（Scene 同款：SurfaceFlinger 激活显示模式）。
        "refresh-status" => {
            print!("{}", refresh::status());
            Ok(())
        }
        "refresh-set" => {
            let package = requested_mode.as_deref().ok_or("refresh-set 需要 <包名>")?;
            let value = graphics_backend.as_deref().ok_or("refresh-set 需要 <目标Hz>")?;
            let hz = value.parse::<u32>().map_err(|_| format!("目标刷新率无效: {value}"))?;
            if hz != 0 && (hz < 30 || hz > 240) {
                return Err(format!("目标刷新率超出合理范围: {value}（30-240，0=跟随系统）"));
            }
            refresh::set(package, hz)?;
            println!("{}", refresh::status());
            Ok(())
        }
        "refresh-remove" => {
            let package = requested_mode.as_deref().ok_or("refresh-remove 需要 <包名>")?;
            refresh::set(package, 0)?;
            println!("{}", refresh::status());
            Ok(())
        }
        // ZenJooo · 运行时游戏目录（自有存档 + Scene 列表合成）。
        "games-status" => {
            print!("{}", games::status());
            Ok(())
        }
        // ZenJooo · DeepSeek 自适应引擎（三态：off/observe/auto）。
        "ai-status" => {
            print!("{}", ai::status(util::now_secs()));
            Ok(())
        }
        "ai-mode" => {
            let mode = requested_mode.as_deref().ok_or("ai-mode 需要 off|observe|auto")?;
            let mode = ai::Mode::parse(mode).ok_or(format!("AI 模式无效: {mode}（off/observe/auto）"))?;
            ai::set_mode(mode)?;
            println!("ai_mode={}\nai_mode_label={}", mode.as_str(), mode.label());
            Ok(())
        }
        "ai-key" => {
            match requested_mode.as_deref() {
                Some("set") => {
                    let key = graphics_backend.as_deref().ok_or("ai-key set 需要 <KEY>")?;
                    ai::set_key(key)?;
                    println!("API Key 已保存（0600 权限；仅存于设备本地）");
                    Ok(())
                }
                Some("clear") => {
                    ai::clear_key()?;
                    println!("API Key 已清除");
                    Ok(())
                }
                _ => Err("用法: ai-key set <KEY> | ai-key clear".to_string()),
            }
        }
        "ai-model" => {
            let model = requested_mode.as_deref().ok_or("ai-model 需要 <模型名>")?;
            ai::set_model(model)?;
            println!("模型已设置为 {model}");
            Ok(())
        }
        "ai-url" => {
            let url = requested_mode.as_deref().ok_or("ai-url 需要 <接口地址>")?;
            ai::set_base_url(url)?;
            println!("接口地址已更新（已通过主机安全校验）");
            Ok(())
        }
        "ai-test" => {
            println!("{}", ai::test_connection()?);
            Ok(())
        }
        "ai-decide" => {
            let event = requested_mode.as_deref().ok_or("ai-decide 需要 <事件名>")?;
            // 手动分析：诊断模式（只记录不写临时策略）；未给载荷时自采最小快照。
            let payload = match graphics_backend.as_deref() {
                Some(text) if !text.trim().is_empty() => text.to_string(),
                _ => ai::snapshot_from_device(),
            };
            let note = ai::consider_with(event, &payload, "manual", util::now_secs(), true)?;
            println!("{note}");
            Ok(())
        }
        "ai-log" => {
            for line in ai::log_entries(30) {
                println!("{line}");
            }
            Ok(())
        }
        "ai-restore" => {
            ai::restore_local()?;
            println!("已恢复本地调度（AI 临时策略已清除；AI 开关状态保持不变）");
            Ok(())
        }
        "game-add" => {
            let package = requested_mode.as_deref().ok_or("game-add 需要 <包名>")?;
            games::set_own(package, true)?;
            println!("已加入自有游戏存档: {package}\n{}", games::status());
            Ok(())
        }
        "game-remove" => {
            let package = requested_mode.as_deref().ok_or("game-remove 需要 <包名>")?;
            games::set_own(package, false)?;
            println!("已从自有游戏存档移除: {package}\n{}", games::status());
            Ok(())
        }
        "graphics-sync" => {
            let rules = graphics::Rules::load();
            println!("{}", graphics::sync(&rules));
            Ok(())
        }
        "graphics-set" => {
            let package = requested_mode.as_deref().ok_or("graphics-set 需要 <包名>")?;
            let backend = graphics_backend
                .as_deref()
                .ok_or("graphics-set 需要 <follow|auto|angle|native|vulkan>")?;
            let backend = graphics::BackendMode::parse(backend)
                .ok_or(format!("后端模式无效: {backend}（follow/auto/angle/native/vulkan）"))?;
            let mut rules = graphics::Rules::load();
            rules.set(package, backend)?;
            println!("{}", graphics::sync(&rules));
            Ok(())
        }
        "graphics-remove" => {
            let package = requested_mode.as_deref().ok_or("graphics-remove 需要 <包名>")?;
            let mut rules = graphics::Rules::load();
            rules.remove(package)?;
            println!("{}", graphics::sync(&rules));
            Ok(())
        }
        "graphics-revert" => {
            println!("{}", graphics::revert_owned());
            Ok(())
        }
        "scene-diagnose" => {
            println!(
                "module_dir={}\nmodule_version={}\nbinary_version={}",
                module_dir.display(),
                util::read_text(&module_dir.join("module.prop"))?
                    .lines()
                    .find_map(|line| line.strip_prefix("version="))
                    .unwrap_or("unknown"),
                env!("CARGO_PKG_VERSION")
            );
            let presence = scene::presence();
            let state = scene::detect();
            println!("scene_presence={:?}\nscene_detection={}\nscene_controller={}\nscene_linked={}\nscene_foreign={}",
                presence.availability, presence.evidence, state.controller(), state.linked(), state.controls_locked());
            for path in ["/data/powercfg.json", "/data/powercfg.sh"] {
                match std::fs::metadata(path) {
                    Ok(m) => println!("{path}=present:{} bytes", m.len()),
                    Err(e) => println!("{path}={e}"),
                }
            }
            let data = telemetry::inspect_scene();
            print!("{}", data.diagnostic_text());
            print!("{}", fas::inspect(std::path::Path::new("/")).report());
            Ok(())
        }
        _ => Err(usage().to_string()),
    }
}

fn usage() -> &'static str {
    "用法: novasched <start|restart|daemon|boot-ready|reload|stop|status|diagnose|health-check|fault-report|release-control|game-diagnose|meta 包名...|fas-diagnose|graphics-status|graphics-sync|graphics-set PKG follow|auto|angle|native|vulkan|graphics-remove PKG|graphics-revert|fps-status|fps-set PKG FPS|fps-remove PKG|refresh-status|refresh-set PKG HZ|refresh-remove PKG|games-status|game-add PKG|game-remove PKG|ai-status|ai-mode off|observe|auto|ai-key set KEY|ai-key clear|ai-model NAME|ai-url URL|ai-test|ai-decide EVENT|ai-log|ai-restore|webui-session|webui-rpc|restore|uninstall|scene-install|scene-restore|scene-status|scene-diagnose|probe|check-config|prepare-storage|prepare-config|prepare-auto|haptic ACTION|version|self-test|install-check PHASE> [--module-dir PATH] [--origin ORIGIN] [--request JSON]"
}

#[cfg(test)]
mod meta_args_tests {
    use super::*;
    use std::path::Path;

    fn tokens(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| value.to_string()).collect()
    }

    #[test]
    fn test_a_two_packages_then_module_dir() {
        // 测试 A：两个包名 + --module-dir——首个包名不得丢失（历史缺陷：
        // requested_mode 预消费吃掉第一个包，预循环又把 --module-dir 吞进
        // 包名列表导致模块目录回落默认值）。
        let parsed = parse_args(
            "meta",
            &tokens(&[
                "meta", "com.tencent.mm", "com.eg.android.AlipayGphone",
                "--module-dir", "/data/adb/modules/NovaSched_Zen_Edition",
            ])[1..],
        )
        .unwrap();
        assert_eq!(
            parsed.meta_packages,
            vec!["com.tencent.mm".to_string(), "com.eg.android.AlipayGphone".to_string()]
        );
        assert_eq!(
            parsed.module_dir.as_deref(),
            Some(Path::new("/data/adb/modules/NovaSched_Zen_Edition"))
        );
    }

    #[test]
    fn test_b_single_package_with_module_dir() {
        // 测试 B：只查询一个包。
        let parsed = parse_args(
            "meta",
            &tokens(&["com.tencent.mm", "--module-dir", "/data/adb/modules/NovaSched_Zen_Edition"]),
        )
        .unwrap();
        assert_eq!(parsed.meta_packages, vec!["com.tencent.mm".to_string()]);
        assert!(parsed.module_dir.is_some());
    }

    #[test]
    fn test_c_option_before_positional_still_parses() {
        // 测试 C：--module-dir 在包名之前（顺序合理变化）。
        let parsed = parse_args(
            "meta",
            &tokens(&["--module-dir", "/data/adb/modules/NovaSched_Zen_Edition", "com.tencent.mm"]),
        )
        .unwrap();
        assert_eq!(parsed.meta_packages, vec!["com.tencent.mm".to_string()]);
        assert_eq!(
            parsed.module_dir.as_deref(),
            Some(Path::new("/data/adb/modules/NovaSched_Zen_Edition"))
        );
    }

    #[test]
    fn test_d_empty_is_safe_and_options_are_rejected_as_packages() {
        // 测试 D：无包名 → 空列表（调用方安全返回空结果）。
        let parsed = parse_args("meta", &[]).unwrap();
        assert!(parsed.meta_packages.is_empty());
        // 带 -- 的未知选项不得进入包名列表。
        assert!(parse_args("meta", &tokens(&["--steam"])).is_err());
        // --module-dir 缺值必须报错而不是把后续选项当路径。
        assert!(parse_args("meta", &tokens(&["--module-dir", "--origin"])).is_err());
    }

    #[test]
    fn other_commands_keep_existing_arg_semantics() {
        // 不改变其他命令的参数语义：值参数命令首 token 仍是 requested_mode。
        let parsed = parse_args("fps-set", &tokens(&["com.game", "90", "--module-dir", "/x"])).unwrap();
        assert_eq!(parsed.requested_mode.as_deref(), Some("com.game"));
        assert_eq!(parsed.second_arg.as_deref(), Some("90"));
        assert_eq!(parsed.module_dir.as_deref(), Some(Path::new("/x")));
        assert!(parsed.meta_packages.is_empty());
        // meta 不再进入值参数命令列表：首 token 必须是包名而不是被消费。
        let parsed = parse_args("meta", &tokens(&["com.tencent.mm", "com.android.camera"])).unwrap();
        assert_eq!(parsed.requested_mode, None);
        assert_eq!(parsed.meta_packages.len(), 2);
    }
}
