use crate::local_infra::{
    expected_exe, launch_plan, normalize, scan_executable, spawn_plan, LocalDatabase, LocalInfraRuntime,
};
use crate::store::{write_store, AppState};
use serde::Serialize;
use tauri::State;

use crate::infra::{control_managed, list_managed, program_service, InfraService};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalDatabaseStatus {
    pub kind: String,
    pub executable: String,
    pub resolved_executable: String,
    pub executable_source: String,
    pub port: u16,
    pub data_dir: String,
    pub config_file: String,
    pub password: String,
    pub state: String,
    pub state_label: String,
    pub pid: Option<u32>,
    pub message: String,
}

#[tauri::command]
pub fn locate_database_files(kind: String, executable: String) -> crate::local_infra::LocatedFiles {
    crate::local_infra::locate_files(&kind, &executable)
}

#[tauri::command]
pub fn list_infra_services() -> Result<Vec<InfraService>, String> {
    list_managed()
}

#[tauri::command]
pub fn control_infra_service(name: String, action: String) -> Result<InfraService, String> {
    control_managed(&name, &action)
}

#[tauri::command]
pub fn get_local_databases(
    state: State<'_, AppState>,
    runtime: State<'_, LocalInfraRuntime>,
) -> Result<Vec<LocalDatabaseStatus>, String> {
    let data = state
        .data
        .lock()
        .map_err(|_| "Failed to open ConfigStore".to_string())?;
    let services = list_managed().unwrap_or_default();
    Ok(vec![
        status_of("mysql", &data.local_databases.mysql, &runtime, "", &services)?,
        status_of("redis", &data.local_databases.redis, &runtime, "", &services)?,
    ])
}

#[tauri::command]
pub fn save_local_database(
    state: State<'_, AppState>,
    runtime: State<'_, LocalInfraRuntime>,
    kind: String,
    executable: String,
    port: u32,
    data_dir: String,
    config_file: String,
    password: String,
) -> Result<LocalDatabaseStatus, String> {
    let port = u16::try_from(port).map_err(|_| "端口需要在 1 到 65535 之间".to_string())?;
    let saved = normalize(
        &kind,
        &LocalDatabase {
            executable,
            port,
            data_dir,
            config_file,
            password,
        },
    )?;
    let message = {
        let mut data = state
            .data
            .lock()
            .map_err(|_| "Failed to open ConfigStore".to_string())?;
        match kind.as_str() {
            "mysql" => data.local_databases.mysql = saved.clone(),
            "redis" => data.local_databases.redis = saved.clone(),
            _ => return Err("只能配置 MySQL 或 Redis".into()),
        }
        write_store(&state.config_path(), &data)?;
        let executable = target_executable(&kind, &saved);
        let services = list_managed().unwrap_or_default();
        let service_busy = program_service(&services, &kind, &executable)
            .is_some_and(|service| service.state != "stopped");
        if service_busy || !live_pids(&runtime, &kind, &executable)?.is_empty() {
            "配置已保存。正在运行的进程仍使用启动时的配置，重启后才会换成新配置。"
        } else {
            "配置已保存。"
        }
    };
    let data = state
        .data
        .lock()
        .map_err(|_| "Failed to open ConfigStore".to_string())?;
    let config = match kind.as_str() {
        "mysql" => &data.local_databases.mysql,
        "redis" => &data.local_databases.redis,
        _ => return Err("只能配置 MySQL 或 Redis".into()),
    };
    status_of(&kind, config, &runtime, message, &list_managed().unwrap_or_default())
}

#[tauri::command]
pub fn control_local_database(
    state: State<'_, AppState>,
    runtime: State<'_, LocalInfraRuntime>,
    kind: String,
    action: String,
) -> Result<LocalDatabaseStatus, String> {
    let config = {
        let data = state
            .data
            .lock()
            .map_err(|_| "Failed to open ConfigStore".to_string())?;
        match kind.as_str() {
            "mysql" => data.local_databases.mysql.clone(),
            "redis" => data.local_databases.redis.clone(),
            _ => return Err("只能配置 MySQL 或 Redis".into()),
        }
    };
    let message = match action.as_str() {
        "start" => start_saved(&runtime, &kind, &config)?,
        "stop" => stop_saved(&runtime, &kind, &config)?,
        "restart" => {
            stop_saved(&runtime, &kind, &config)?;
            start_saved(&runtime, &kind, &config)?
        }
        _ => return Err("操作只能是启动、停止或重启".into()),
    };
    status_of(&kind, &config, &runtime, &message, &list_managed().unwrap_or_default())
}

fn service_note(name: &str, message: &str, done: &str) -> String {
    if message.is_empty() {
        format!("已通过 Windows 服务 {name} {done}")
    } else {
        format!("Windows 服务 {name}：{message}")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StartChoice {
    Service,
    AlreadyRunning,
    Spawn,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StopChoice {
    Service,
    Kill,
    AlreadyStopped,
}

fn target_executable(kind: &str, config: &LocalDatabase) -> String {
    if kind == "mysql" {
        crate::local_infra::mysql_resolution(config).1
    } else {
        config.executable.trim().to_string()
    }
}

fn live_pids(runtime: &LocalInfraRuntime, kind: &str, executable: &str) -> Result<Vec<u32>, String> {
    let mut pids = Vec::new();
    if let Some(pid) = runtime.observed_pid(kind)? {
        pids.push(pid);
    }
    for pid in scan_executable(executable) {
        if !pids.contains(&pid) {
            pids.push(pid);
        }
    }
    Ok(pids)
}

/// 服务正在运行时继续交给服务管理器。没有服务、但配置路径上已有进程时，不再另起一个。
fn choose_start(service_busy: bool, has_service: bool, pids: &[u32]) -> StartChoice {
    if service_busy {
        StartChoice::Service
    } else if !pids.is_empty() {
        StartChoice::AlreadyRunning
    } else if has_service {
        StartChoice::Service
    } else {
        StartChoice::Spawn
    }
}

/// 服务正在运行时只停服务。服务已停、进程还在时，结束这个进程。
fn choose_stop(service_busy: bool, pids: &[u32]) -> StopChoice {
    if service_busy {
        StopChoice::Service
    } else if !pids.is_empty() {
        StopChoice::Kill
    } else {
        StopChoice::AlreadyStopped
    }
}

fn interpret_status(
    owned: Option<u32>,
    service_running: bool,
    service_pid: Option<u32>,
    scanned: &[u32],
) -> (bool, Option<u32>) {
    let pid = owned
        .or_else(|| service_running.then_some(service_pid).flatten())
        .or_else(|| scanned.first().copied());
    let running = service_running || owned.is_some() || !scanned.is_empty();
    (running, pid)
}

fn start_saved(
    runtime: &LocalInfraRuntime,
    kind: &str,
    config: &LocalDatabase,
) -> Result<String, String> {
    let expected = expected_exe(kind)?;
    let executable = target_executable(kind, config);
    let services = list_managed().unwrap_or_default();
    let service = program_service(&services, kind, &executable).cloned();
    let pids = live_pids(runtime, kind, &executable)?;
    let busy = service.as_ref().is_some_and(|item| item.state != "stopped");
    match choose_start(busy, service.is_some(), &pids) {
        StartChoice::AlreadyRunning => {
            Err(format!("{expected} 已经在运行。要使用新配置，请先停止或重启。"))
        }
        StartChoice::Service => {
            let service = service.expect("service choice has a service");
            let updated = control_managed(&service.name, "start")?;
            runtime.clear(kind)?;
            Ok(service_note(&service.name, &updated.message, "启动"))
        }
        StartChoice::Spawn => {
            let plan = launch_plan(kind, config)?;
            let pid = spawn_plan(&plan)?;
            runtime.remember(kind, pid)?;
            let mut message = format!("已启动 {expected}，PID {pid}");
            if kind == "redis"
                && !config.config_file.trim().is_empty()
                && !config.password.trim().is_empty()
            {
                message.push_str("。填写了配置文件，启动时不另加密码");
            }
            Ok(message)
        }
    }
}

fn stop_saved(runtime: &LocalInfraRuntime, kind: &str, config: &LocalDatabase) -> Result<String, String> {
    let expected = expected_exe(kind)?;
    let executable = target_executable(kind, config);
    let services = list_managed().unwrap_or_default();
    let service = program_service(&services, kind, &executable).cloned();
    let busy = service.as_ref().is_some_and(|item| item.state != "stopped");
    let pids = live_pids(runtime, kind, &executable)?;
    match choose_stop(busy, &pids) {
        StopChoice::Service => {
            let service = service.expect("service choice has a service");
            let updated = control_managed(&service.name, "stop")?;
            runtime.clear(kind)?;
            Ok(service_note(&service.name, &updated.message, "停止"))
        }
        StopChoice::AlreadyStopped => Ok(format!("{expected} 已经停止")),
        StopChoice::Kill => {
            for pid in &pids {
                crate::commands::port::taskkill(*pid, true)?;
            }
            runtime.clear(kind)?;
            Ok(format!("已停止 {expected}"))
        }
    }
}

fn status_of(
    kind: &str,
    config: &LocalDatabase,
    runtime: &LocalInfraRuntime,
    message: &str,
    services: &[InfraService],
) -> Result<LocalDatabaseStatus, String> {
    let owned = runtime.observed_pid(kind)?;
    let (executable_source, resolved_executable) = if kind == "mysql" {
        let (source, path) = crate::local_infra::mysql_resolution(config);
        (source.to_string(), path)
    } else if config.executable.trim().is_empty() {
        ("unset".into(), String::new())
    } else {
        ("custom".into(), config.executable.clone())
    };
    let mut data_dir = config.data_dir.clone();
    let mut config_file = config.config_file.clone();
    let search_at = if resolved_executable.is_empty() {
        config.executable.clone()
    } else {
        resolved_executable.clone()
    };
    if !search_at.is_empty() && (data_dir.is_empty() || config_file.is_empty()) {
        let found = crate::local_infra::locate_files(kind, &search_at);
        if config_file.is_empty() {
            config_file = found.config_file;
        }
        if data_dir.is_empty() {
            data_dir = found.data_dir;
        }
    }
    let service = program_service(services, kind, &search_at);
    let service_running = service.is_some_and(|item| {
        matches!(item.state.as_str(), "running" | "start_pending" | "paused" | "pause_pending" | "continue_pending")
    });
    let scanned = scan_executable(&search_at);
    let (running, pid) = interpret_status(
        owned,
        service_running,
        service.and_then(|item| (item.pid > 0).then_some(item.pid)),
        &scanned,
    );
    Ok(LocalDatabaseStatus {
        kind: kind.to_string(),
        executable: config.executable.clone(),
        resolved_executable,
        executable_source,
        port: config.port,
        data_dir,
        config_file,
        password: config.password.clone(),
        state: if running { "running" } else { "stopped" }.into(),
        state_label: if running { "运行中" } else { "已停止" }.into(),
        pid,
        message: message.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::process::Command;
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

    #[test]
    fn external_process_is_running_without_an_owned_pid_or_service() {
        let (running, pid) = interpret_status(None, false, None, &[8948]);
        assert!(running);
        assert_eq!(pid, Some(8948));
        let (running, pid) = interpret_status(None, false, None, &[]);
        assert!(!running);
        assert_eq!(pid, None);
    }

    #[test]
    fn a_busy_service_stays_with_the_service_manager() {
        assert_eq!(choose_stop(true, &[8948]), StopChoice::Service);
        assert_eq!(choose_start(true, true, &[8948]), StartChoice::Service);
        assert_eq!(choose_stop(false, &[8948]), StopChoice::Kill);
        assert_eq!(choose_start(false, false, &[8948]), StartChoice::AlreadyRunning);
        assert_eq!(choose_start(false, true, &[8948]), StartChoice::AlreadyRunning);
        assert_eq!(choose_start(false, true, &[]), StartChoice::Service);
        assert_eq!(choose_start(false, false, &[]), StartChoice::Spawn);
        assert_eq!(choose_stop(false, &[]), StopChoice::AlreadyStopped);
    }

    #[test]
    fn already_running_program_is_shown_refused_and_then_stopped() {
        let stamp = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let dir = std::env::temp_dir().join(format!("xchen-redis-status-{stamp}"));
        fs::create_dir_all(&dir).unwrap();
        let executable = dir.join("redis-server.exe");
        fs::copy(r"C:\Windows\System32\ping.exe", &executable).unwrap();
        let mut command = Command::new(&executable);
        command.args(["-n", "40", "127.0.0.1"]);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x08000000);
        }
        let child = command.spawn().unwrap();
        let pid = child.id();
        let _reap = Reap(pid);
        let path = executable.to_string_lossy().to_string();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !scan_executable(&path).contains(&pid) {
            assert!(Instant::now() < deadline, "spawned program did not appear");
            std::thread::sleep(Duration::from_millis(50));
        }

        let runtime = LocalInfraRuntime::default();
        let config = LocalDatabase {
            executable: path.clone(),
            port: 6379,
            data_dir: dir.to_string_lossy().to_string(),
            config_file: dir.join("redis.windows.conf").to_string_lossy().to_string(),
            password: String::new(),
        };
        let status = status_of("redis", &config, &runtime, "", &[]).unwrap();
        assert_eq!(status.state, "running");
        assert_eq!(status.pid, Some(pid));

        let error = start_saved(&runtime, "redis", &config).unwrap_err();
        assert!(error.contains("已经在运行"));
        assert_eq!(scan_executable(&path), vec![pid]);

        let stopped = stop_saved(&runtime, "redis", &config).unwrap();
        assert!(stopped.contains("已停止"));
        let deadline = Instant::now() + Duration::from_secs(5);
        while scan_executable(&path).contains(&pid) {
            assert!(Instant::now() < deadline, "program was still running after stop");
            std::thread::sleep(Duration::from_millis(50));
        }
        let status = status_of("redis", &config, &runtime, "", &[]).unwrap();
        assert_eq!(status.state, "stopped");
        assert_eq!(status.pid, None);
        let _ = fs::remove_dir_all(&dir);
    }

    struct Reap(u32);
    impl Drop for Reap {
        fn drop(&mut self) {
            let _ = crate::commands::port::taskkill(self.0, true);
        }
    }
}
