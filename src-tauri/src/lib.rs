mod commands;
mod dev_server;
mod infra;
mod launch;
mod local_infra;
mod net;
mod path_usage;
mod runtime;
mod store;

use commands::{app, environment, history, infra as infra_cmd, log, path, port, process, service, workspace};
use local_infra::LocalInfraRuntime;
use runtime::RuntimeManager;
use store::AppState;
use tauri::{Emitter, Manager};

pub fn run() {
    let context = tauri::generate_context!();
    #[cfg(feature = "portable")]
    let context = {
        let mut context = context;
        // Tauri resolves this root relative to the executable, independent of cwd.
        // Keep WebView storage, caches and framework logs inside the portable folder.
        context.config_mut().app.app_directories_override = Some(
            tauri::utils::config::AppDirectoriesOverride::Root("data/webview".into()),
        );
        context
    };
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .manage(AppState::new())
        .manage(RuntimeManager::new())
        .manage(LocalInfraRuntime::default())
        .setup(|app| {
            // 调试构建没有 Vite。先在 127.0.0.1:1430 提供 frontend/，
            // 窗口的 devUrl 才能打开带 MySQL / Redis 工具条的页面。
            dev_server::ensure_started();
            // 自动刷新心跳：按设置的 autoRefresh/refreshInterval 周期广播
            // 全部会话状态（runtime:changed），前端监听器据此更新服务列表，
            // 进程自尽/外部终止也能在下一拍反映出来。
            let handle = app.handle().clone();
            std::thread::spawn(move || loop {
                let (auto, interval) = {
                    let state = handle.state::<AppState>();
                    let data = state.data.lock().unwrap();
                    (data.settings.auto_refresh, data.settings.refresh_interval)
                };
                std::thread::sleep(std::time::Duration::from_secs(interval.max(1) as u64));
                if !auto {
                    continue;
                }
                let runtime = handle.state::<RuntimeManager>();
                for s in runtime.all_run_states() {
                    let _ = handle.emit(
                        "runtime:changed",
                        serde_json::json!({ "serviceId": s.service_id, "runtime": s }),
                    );
                }
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            // app
            app::health_check,
            app::get_app_version,
            app::get_settings,
            app::update_settings,
            app::get_data_paths,
            app::reset_data_directory,
            app::migrate_data_directory,
            app::restart_app,
            app::fetch_data_store_identifiers,
            app::remove_data_store,
            // workspace
            workspace::create_workspace,
            workspace::update_workspace,
            workspace::delete_workspace,
            workspace::list_workspaces,
            service::start_workspace,
            service::stop_workspace,
            service::stop_all_workspaces,
            workspace::workspace_discover,
            workspace::detect_project_port,
            crate::launch::get_launch_options,
            // service
            service::create_service,
            service::update_service,
            service::delete_service,
            service::list_services,
            service::apply_discovery,
            service::start_process,
            service::stop_process,
            service::restart_process,
            service::force_kill_process,
            service::stop_all_processes,
            // process
            process::get_process_info,
            process::get_process_runtime,
            process::kill_process_by_pid,
            process::kill_process_tree_by_pid,
            // port
            port::release_port,
            port::query_single_port,
            port::query_multiple_ports,
            port::query_ports_range,
            port::get_all_tcp_connections,
            port::get_all_udp_listeners,
            // path
            path::check_is_directory,
            path::check_is_file,
            path::validate_path,
            path::open_path,
            path::open_url,
            path::reveal_item_in_dir,
            path::release_path,
            path::query_file,
            path::query_path,
            // history + favorites
            history::add_history,
            history::list_history,
            history::delete_history,
            history::clear_history,
            history::add_favorite,
            history::list_favorites,
            history::delete_favorite,
            // log
            log::log_subscribe,
            log::log_unsubscribe,
            log::log_history,
            log::log_clear,
            log::log_export,
            // environment
            environment::environment_detect_all,
            environment::environment_detect_single,
            // MySQL / Redis Windows services
            infra_cmd::list_infra_services,
            infra_cmd::control_infra_service,
            infra_cmd::get_local_databases,
            infra_cmd::save_local_database,
            infra_cmd::control_local_database,
            infra_cmd::locate_database_files,
        ])
        .run(context)
        .expect("error while running tauri application");
}

/// 前端契约测试：从第二版 bundle 提取的全部 32 个命令必须都已注册。
#[cfg(test)]
mod tests {
    const FRONTEND_COMMANDS: &[&str] = &[
        "add_favorite",
        "add_history",
        "check_is_directory",
        "check_is_file",
        "create_service",
        "create_workspace",
        "delete_favorite",
        "delete_history",
        "delete_service",
        "delete_workspace",
        "get_all_tcp_connections",
        "get_all_udp_listeners",
        "get_app_version",
        "get_data_paths",
        "get_process_info",
        "get_process_runtime",
        "get_settings",
        "health_check",
        "kill_process_by_pid",
        "kill_process_tree_by_pid",
        "list_favorites",
        "list_history",
        "list_services",
        "list_workspaces",
        "open_path",
        "release_path",
        "release_port",
        "start_process",
        "start_workspace",
        "stop_all_processes",
        "stop_process",
        "stop_workspace",
    ];

    #[test]
    fn all_frontend_commands_are_registered() {
        let source = include_str!("lib.rs");
        for cmd in FRONTEND_COMMANDS {
            assert!(
                source.contains(&format!("{cmd},")),
                "command `{cmd}` is called by the v2 frontend but not registered"
            );
        }
    }
}
