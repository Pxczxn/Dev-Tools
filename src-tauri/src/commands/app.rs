use crate::store::{AppState, DataPaths, Settings, SettingsPatch};
use std::fs;
use tauri::{AppHandle, State};

#[derive(Debug, Clone, serde::Serialize)]
pub struct HealthCheckResponse {
    pub app_version: String,
    pub ipc_status: String,
}

#[tauri::command]
pub fn health_check() -> HealthCheckResponse {
    HealthCheckResponse {
        app_version: env!("CARGO_PKG_VERSION").to_string(),
        ipc_status: "ONLINE".into(),
    }
}

#[tauri::command]
pub fn get_app_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

#[tauri::command]
pub fn get_settings(state: State<'_, AppState>) -> Result<Settings, String> {
    let data = state
        .data
        .lock()
        .map_err(|_| "Failed to open ConfigStore".to_string())?;
    Ok(data.settings.clone())
}

#[tauri::command]
pub fn update_settings(
    state: State<'_, AppState>,
    input: SettingsPatch,
) -> Result<Settings, String> {
    let mut data = state
        .data
        .lock()
        .map_err(|_| "Failed to open ConfigStore".to_string())?;
    data.settings
        .apply_patch(input)
        .map_err(|e| format!("Invalid settings input: {e}"))?;
    let settings = data.settings.clone();
    drop(data);
    state.commit()?;
    Ok(settings)
}

#[tauri::command]
pub fn get_data_paths(state: State<'_, AppState>) -> DataPaths {
    DataPaths {
        install_dir: state.install_dir.to_string_lossy().to_string(),
        data_root: state.data_root.to_string_lossy().to_string(),
        config_path: state.config_path().to_string_lossy().to_string(),
        default_data_root: state.default_data_root.to_string_lossy().to_string(),
        is_custom: state.is_custom_data_root(),
        migrated_files: Vec::new(),
        requires_restart: false,
    }
}

/// 把当前数据根目录整体迁移到 targetPath，并把 dataPath 写回设置。需重启生效。
#[tauri::command]
pub fn migrate_data_directory(
    state: State<'_, AppState>,
    target_path: String,
) -> Result<serde_json::Value, String> {
    if std::env::var("XCHEN_DATA_DIR").is_ok_and(|s| !s.trim().is_empty()) {
        return Err("当前由 XCHEN_DATA_DIR 指定数据目录，请先移除该环境变量再迁移".into());
    }
    move_data(&state, std::path::Path::new(&target_path), false)
}

/// Copy back the current data before removing the bootstrap pointer.
#[tauri::command]
pub fn reset_data_directory(state: State<'_, AppState>) -> Result<serde_json::Value, String> {
    if std::env::var("XCHEN_DATA_DIR").is_ok_and(|s| !s.trim().is_empty()) {
        return Err("当前由 XCHEN_DATA_DIR 指定数据目录，请先移除该环境变量再恢复默认目录".into());
    }
    move_data(&state, &state.default_data_root, true)
}

fn move_data(
    state: &AppState,
    target: &std::path::Path,
    reset: bool,
) -> Result<serde_json::Value, String> {
    if !target.is_absolute() {
        return Err("请选择绝对路径的数据目录".into());
    }
    fs::create_dir_all(&state.data_root).map_err(|e| e.to_string())?;
    fs::create_dir_all(target).map_err(|e| e.to_string())?;
    let source = state.data_root.canonicalize().map_err(|e| e.to_string())?;
    let target = target.canonicalize().map_err(|e| e.to_string())?;
    if source == target {
        return Ok(serde_json::json!({"migratedFiles": 0, "requiresRestart": false}));
    }
    if target.starts_with(&source) || source.starts_with(&target) {
        return Err("源目录和目标目录不能互相包含".into());
    }
    if !reset && target.join("config.json").exists() {
        return Err("目标目录已有配置，请选择空的数据目录".into());
    }
    let mut data = state
        .data
        .lock()
        .map_err(|_| "Failed to open ConfigStore".to_string())?;
    // Do not change the pointer until every file and the destination snapshot are saved.
    copy_dir_recursive(&source, &target).map_err(|e| format!("复制数据失败，尚未切换目录：{e}"))?;
    let mut snapshot = data.clone();
    snapshot.settings.data_path = if reset {
        None
    } else {
        Some(target.to_string_lossy().to_string())
    };
    crate::store::write_store(&target.join("config.json"), &snapshot)?;
    crate::store::write_store(&state.default_data_root.join("config.json"), &snapshot)?;
    *data = snapshot;
    Ok(
        serde_json::json!({"migratedFiles": fs::read_dir(&target).map_err(|e| e.to_string())?.count(), "requiresRestart": true}),
    )
}

/// 重启应用：重新拉起自身可执行文件后退出。
#[tauri::command]
pub fn restart_app(app: AppHandle) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| format!("Failed to resolve exe: {e}"))?;
    std::process::Command::new(exe)
        .spawn()
        .map_err(|e| format!("Failed to restart: {e}"))?;
    app.exit(0);
    Ok(())
}

#[tauri::command]
pub fn fetch_data_store_identifiers() -> Result<Vec<String>, String> {
    Err("NOT_IMPLEMENTED:fetch_data_store_identifiers".into())
}

#[tauri::command]
pub fn remove_data_store(_id: String) -> Result<(), String> {
    Err("NOT_IMPLEMENTED:remove_data_store".into())
}

fn copy_dir_recursive(src: &std::path::Path, dest: &std::path::Path) -> std::io::Result<()> {
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let to = dest.join(entry.file_name());
        if entry.file_type()?.is_symlink() {
            return Err(std::io::Error::other(
                "数据目录包含符号链接，请手动处理后再迁移",
            ));
        }
        if entry.path().is_dir() {
            fs::create_dir_all(&to)?;
            copy_dir_recursive(&entry.path(), &to)?;
        } else {
            fs::copy(entry.path(), &to)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn migration_and_reset_survive_restart_without_losing_services() {
        let temp = tempfile::tempdir().unwrap();
        let install = temp.path().join("install");
        let custom = temp.path().join("custom");
        let state = AppState::open_at(install.clone(), None);
        state
            .data
            .lock()
            .unwrap()
            .services
            .push(crate::store::Service {
                id: "service".into(),
                ..Default::default()
            });
        state.commit().unwrap();
        fs::create_dir_all(state.data_root.join("logs")).unwrap();
        fs::write(state.data_root.join("logs/a.txt"), "saved").unwrap();
        move_data(&state, &custom, false).unwrap();
        let reopened = AppState::open_at(install.clone(), None);
        assert_eq!(
            reopened.data_root.canonicalize().unwrap(),
            custom.canonicalize().unwrap()
        );
        assert_eq!(reopened.data.lock().unwrap().services[0].id, "service");
        assert_eq!(
            fs::read_to_string(custom.join("logs/a.txt")).unwrap(),
            "saved"
        );
        reopened.data.lock().unwrap().services[0].name = "new name".into();
        reopened.commit().unwrap();
        move_data(&reopened, &reopened.default_data_root, true).unwrap();
        let reset = AppState::open_at(install.clone(), None);
        assert!(!reset.is_custom_data_root());
        assert_eq!(reset.data.lock().unwrap().services[0].name, "new name");
        let override_path = temp.path().join("override");
        assert_eq!(
            AppState::open_at(install, Some(override_path.clone())).data_root,
            override_path
        );
    }
    #[test]
    fn migration_rejects_nested_paths_and_existing_configuration() {
        let temp = tempfile::tempdir().unwrap();
        let state = AppState::open_at(temp.path().join("app"), None);
        state.commit().unwrap();
        assert!(move_data(&state, &state.data_root.join("nested"), false).is_err());
        let occupied = temp.path().join("occupied");
        fs::create_dir(&occupied).unwrap();
        fs::write(occupied.join("config.json"), "keep").unwrap();
        assert!(move_data(&state, &occupied, false).is_err());
        assert_eq!(
            fs::read_to_string(occupied.join("config.json")).unwrap(),
            "keep"
        );
        assert!(state.data.lock().unwrap().settings.data_path.is_none());
    }
}
