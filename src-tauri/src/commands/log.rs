use crate::runtime::RuntimeManager;
use serde_json::json;
use tauri::State;

// ---------- log.rs 语义：按 serviceId 的输出缓冲 ----------

/// 第二版前端对全局事件 log:batch 自行监听；subscribe/unsubscribe 仅用于
/// 通知后端维持会话缓冲，返回值被忽略。
#[tauri::command]
pub fn log_subscribe(_service_id: String) -> Result<(), String> {
    Ok(())
}

#[tauri::command]
pub fn log_unsubscribe(_service_id: String) -> Result<(), String> {
    Ok(())
}

#[tauri::command]
pub fn log_history(
    runtime: State<'_, RuntimeManager>,
    service_id: String,
    limit: Option<u32>,
) -> Result<serde_json::Value, String> {
    match runtime.get(&service_id) {
        Some(session) => Ok(json!(session.history(limit))),
        None => Ok(json!([])),
    }
}

#[tauri::command]
pub fn log_clear(runtime: State<'_, RuntimeManager>, service_id: String) -> Result<(), String> {
    if let Some(session) = runtime.get(&service_id) {
        session.clear_logs();
    }
    Ok(())
}

/// 导出会话输出。savePath 缺省时写入数据目录 logs/ 下。
#[tauri::command]
pub fn log_export(
    state: tauri::State<'_, crate::store::AppState>,
    runtime: State<'_, RuntimeManager>,
    service_id: String,
    save_path: Option<String>,
) -> Result<serde_json::Value, String> {
    let session = runtime
        .get(&service_id)
        .ok_or_else(|| format!("Service not found: {service_id}"))?;
    let entries = session.history(None);
    let content = entries
        .iter()
        .map(|e| format!("{} [{}] {}", e.timestamp, e.stream, e.text))
        .collect::<Vec<_>>()
        .join("\n");
    let path = match save_path {
        Some(p) if !p.trim().is_empty() => std::path::PathBuf::from(p),
        _ => {
            let dir = state.data_root.join("logs");
            std::fs::create_dir_all(&dir)
                .map_err(|e| format!("Failed to create data dir: {e}"))?;
            dir.join(format!(
                "{}-{}.log",
                service_id,
                chrono::Local::now().format("%Y%m%d-%H%M%S")
            ))
        }
    };
    std::fs::write(&path, content).map_err(|e| format!("Failed to write log export: {e}"))?;
    Ok(json!({ "path": path.to_string_lossy(), "lines": entries.len() }))
}
