use crate::runtime::{LogEntry, RuntimeManager};
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
) -> Result<Vec<LogEntry>, String> {
    match runtime.get(&service_id) {
        Some(session) => Ok(session.history(limit)),
        None => Ok(Vec::new()),
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
    let path = match save_path {
        Some(p) if !p.trim().is_empty() => std::path::PathBuf::from(p),
        _ => {
            let dir = state.data_root.join("logs");
            std::fs::create_dir_all(&dir).map_err(|e| format!("Failed to create data dir: {e}"))?;
            dir.join(format!(
                "{}-{}.log",
                service_id,
                chrono::Local::now().format("%Y%m%d-%H%M%S")
            ))
        }
    };
    let file =
        std::fs::File::create(&path).map_err(|e| format!("Failed to write log export: {e}"))?;
    let mut writer = std::io::BufWriter::new(file);
    write_log_entries(&mut writer, &entries)
        .map_err(|e| format!("Failed to write log export: {e}"))?;
    Ok(json!({ "path": path.to_string_lossy(), "lines": entries.len() }))
}

fn write_log_entries(
    writer: &mut impl std::io::Write,
    entries: &[LogEntry],
) -> std::io::Result<()> {
    for (index, entry) in entries.iter().enumerate() {
        if index > 0 {
            writer.write_all(b"\n")?;
        }
        write!(
            writer,
            "{} [{}] {}",
            entry.timestamp, entry.stream, entry.text
        )?;
    }
    writer.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn streamed_export_preserves_format_unicode_and_empty_history() {
        let entries = vec![
            LogEntry {
                timestamp: "now".into(),
                stream: "stdout".into(),
                text: "中文 SQL".into(),
            },
            LogEntry {
                timestamp: "later".into(),
                stream: "stderr".into(),
                text: "错误\n详情".into(),
            },
        ];
        let mut output = Vec::new();
        write_log_entries(&mut output, &entries).unwrap();
        assert_eq!(
            String::from_utf8(output).unwrap(),
            "now [stdout] 中文 SQL\nlater [stderr] 错误\n详情"
        );
        let mut empty = Vec::new();
        write_log_entries(&mut empty, &[]).unwrap();
        assert!(empty.is_empty());
    }

    #[test]
    fn streamed_export_reports_buffer_flush_failure() {
        struct FailedDisk;
        impl std::io::Write for FailedDisk {
            fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
                Err(std::io::ErrorKind::StorageFull.into())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let mut writer = std::io::BufWriter::new(FailedDisk);
        let entries = [LogEntry {
            timestamp: "now".into(),
            stream: "stdout".into(),
            text: "log".into(),
        }];
        assert_eq!(
            write_log_entries(&mut writer, &entries).unwrap_err().kind(),
            std::io::ErrorKind::StorageFull
        );
    }
}
