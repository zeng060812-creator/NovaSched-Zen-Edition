mod app_modes;
mod config;
mod config_v2;
mod daemon;
mod event_loop;
mod ffi;
mod game_diagnostics;
mod hardware;
mod json;
mod lifecycle;
mod logging;
mod options;
mod package_registry;
mod process_identity;
mod profiles;
mod root_bridge;
mod scene;
mod scheduler;
mod snapshot;
mod soc;
mod telemetry;
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

fn run() -> Result<()> {
    let mut args = env::args().skip(1);
    let command = args.next().ok_or_else(|| usage().to_string())?;
    let requested_mode = if command == "scene-mode" {
        args.next()
    } else {
        None
    };
    let mut module_dir = util::default_module_dir();
    let mut webui_origin = None;
    let mut webui_request = None;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--module-dir" => {
                module_dir = PathBuf::from(
                    args.next()
                        .ok_or_else(|| "--module-dir 缺少路径".to_string())?,
                );
            }
            "--origin" => {
                webui_origin = Some(args.next().ok_or_else(|| "--origin 缺少来源".to_string())?);
            }
            "--request" => {
                webui_request = Some(
                    args.next()
                        .ok_or_else(|| "--request 缺少 JSON".to_string())?,
                );
            }
            _ => return Err(format!("未知参数: {arg}\n{}", usage())),
        }
    }

    match command.as_str() {
        "version" | "--version" => {
            println!("NovaSched Zen Edition {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        "start" => lifecycle::start(&module_dir, false),
        "restart" => lifecycle::start(&module_dir, true),
        "supervise" => lifecycle::supervise(&module_dir),
        "diagnose" => lifecycle::diagnose(&module_dir),
        "game-diagnose" => game_diagnostics::run(&module_dir),
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
        "scene-mode" => app_modes::scene_callback(requested_mode.as_deref().unwrap_or("")),
        "daemon" => daemon::run_daemon(module_dir),
        "reload" => daemon::send_signal(&module_dir, ffi::SIGHUP),
        "stop" => lifecycle::stop(&module_dir),
        "status" => daemon::print_status(&module_dir),
        "restore" => daemon::restore_stock(&module_dir),
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
            Ok(())
        }
        _ => Err(usage().to_string()),
    }
}

fn usage() -> &'static str {
    "用法: novasched <start|restart|daemon|reload|stop|status|diagnose|game-diagnose|webui-session|webui-rpc|restore|scene-install|scene-restore|scene-diagnose|probe|check-config|prepare-config|version|self-test> [--module-dir PATH] [--origin ORIGIN] [--request JSON]"
}
