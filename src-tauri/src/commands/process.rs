use crate::runtime::RuntimeManager;
use serde::Serialize;
use sysinfo::{ProcessesToUpdate, System};
use tauri::State;

/// 进程列表条目：第二版前端仅消费 name/pid/path/memoryUsage/threads。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessInfo {
    pub name: String,
    pub pid: u32,
    pub path: Option<String>,
    /// 字节
    pub memory_usage: u64,
    pub threads: Option<u32>,
}

pub fn list_processes() -> Vec<ProcessInfo> {
    let mut sys = System::new();
    sys.refresh_processes(ProcessesToUpdate::All, true);
    let mut out: Vec<ProcessInfo> = sys
        .processes()
        .iter()
        .map(|(pid, process)| ProcessInfo {
            name: process.name().to_string_lossy().to_string(),
            pid: pid.as_u32(),
            path: process.exe().map(|p| p.to_string_lossy().to_string()),
            memory_usage: process.memory(),
            // sysinfo 0.33 无线程计数 API；第二版前端展示留空
            threads: None,
        })
        .collect();
    out.sort_by_key(|a| std::cmp::Reverse(a.memory_usage));
    out
}

#[tauri::command]
pub fn get_process_info() -> Result<Vec<ProcessInfo>, String> {
    Ok(list_processes())
}

/// 返回全部已知服务会话的运行状态（含已退出的终态）。
#[tauri::command]
pub fn get_process_runtime(
    runtime: State<'_, RuntimeManager>,
    service_id: Option<String>,
) -> Result<Vec<serde_json::Value>, String> {
    Ok(runtime
        .all_run_states()
        .into_iter()
        .filter(|s| service_id.as_ref().is_none_or(|id| id == &s.service_id))
        .map(|s| serde_json::to_value(s).unwrap_or(serde_json::Value::Null))
        .collect())
}

#[tauri::command]
pub fn kill_process_by_pid(pid: u32) -> Result<(), String> {
    crate::commands::port::taskkill(pid, false)
}

#[tauri::command]
pub fn kill_process_tree_by_pid(pid: u32) -> Result<(), String> {
    crate::commands::port::taskkill(pid, true)
}
