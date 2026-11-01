use crate::local_infra::LocalDatabases;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

/// 持久化根结构，对应 `<dataRoot>/config.json`，version = 2。
/// 字段与从第二版成品运行生成的真实文件一致。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistFile {
    pub version: u32,
    #[serde(default)]
    pub settings: Settings,
    #[serde(default)]
    pub workspaces: Vec<Workspace>,
    #[serde(default)]
    pub services: Vec<Service>,
    #[serde(default)]
    pub history: Vec<HistoryRecord>,
    #[serde(default)]
    pub favorites: Vec<FavoriteRecord>,
    #[serde(default)]
    pub local_databases: LocalDatabases,
}

impl Default for PersistFile {
    fn default() -> Self {
        Self {
            version: 2,
            settings: Settings::default(),
            workspaces: Vec::new(),
            services: Vec::new(),
            history: Vec::new(),
            favorites: Vec::new(),
            local_databases: LocalDatabases::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    /// light | dark | system
    pub theme: String,
    /// tray | quit | ask
    pub close_behavior: String,
    pub max_log_lines: u32,
    pub start_minimized: bool,
    pub auto_restore_last_session: bool,
    pub startup_interval: u32,
    pub show_timestamp: bool,
    pub default_browser: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data_path: Option<String>,
    pub auto_refresh: bool,
    pub refresh_interval: u32,
    pub confirm_dangerous_action: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: "dark".into(),
            close_behavior: "tray".into(),
            max_log_lines: 5000,
            start_minimized: false,
            auto_restore_last_session: false,
            startup_interval: 1000,
            show_timestamp: true,
            default_browser: "system".into(),
            data_path: None,
            auto_refresh: true,
            refresh_interval: 5,
            confirm_dangerous_action: true,
        }
    }
}

impl Settings {
    pub fn validate(&self) -> Result<(), String> {
        if self.max_log_lines < 500 || self.max_log_lines > 50000 {
            return Err(format!(
                "maxLogLines must be between 500 and 50000, got {}",
                self.max_log_lines
            ));
        }
        if self.startup_interval > 30000 {
            return Err(format!(
                "startupInterval must be between 0 and 30000, got {}",
                self.startup_interval
            ));
        }
        Ok(())
    }
}

/// 增量补丁：None 表示不修改。
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsPatch {
    pub theme: Option<String>,
    pub close_behavior: Option<String>,
    pub max_log_lines: Option<u32>,
    pub start_minimized: Option<bool>,
    pub auto_restore_last_session: Option<bool>,
    pub startup_interval: Option<u32>,
    pub show_timestamp: Option<bool>,
    pub default_browser: Option<String>,
    pub data_path: Option<String>,
    pub auto_refresh: Option<bool>,
    pub refresh_interval: Option<u32>,
    pub confirm_dangerous_action: Option<bool>,
}

impl Settings {
    pub fn apply_patch(&mut self, patch: SettingsPatch) -> Result<(), String> {
        let mut updated = self.clone();
        if let Some(v) = patch.max_log_lines {
            updated.max_log_lines = v;
        }
        if let Some(v) = patch.startup_interval {
            updated.startup_interval = v;
        }
        if let Some(v) = patch.theme {
            updated.theme = v;
        }
        if let Some(v) = patch.close_behavior {
            updated.close_behavior = v;
        }
        if let Some(v) = patch.start_minimized {
            updated.start_minimized = v;
        }
        if let Some(v) = patch.auto_restore_last_session {
            updated.auto_restore_last_session = v;
        }
        if let Some(v) = patch.show_timestamp {
            updated.show_timestamp = v;
        }
        if let Some(v) = patch.default_browser {
            updated.default_browser = v;
        }
        if let Some(v) = patch.data_path {
            updated.data_path = if v.is_empty() { None } else { Some(v) };
        }
        if let Some(v) = patch.auto_refresh {
            updated.auto_refresh = v;
        }
        if let Some(v) = patch.refresh_interval {
            updated.refresh_interval = v;
        }
        if let Some(v) = patch.confirm_dangerous_action {
            updated.confirm_dangerous_action = v;
        }
        updated.validate()?;
        *self = updated;
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Workspace {
    pub id: String,
    pub name: String,
    pub description: String,
    pub root_path: String,
    pub color: Option<String>,
    pub icon: Option<String>,
    /// 单数布尔（第二版前端如此消费）
    pub favorite: bool,
    /// parallel | sequential
    pub start_mode: String,
    pub created_at: String,
    pub updated_at: String,
}

impl Default for Workspace {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            description: String::new(),
            root_path: String::new(),
            color: None,
            icon: None,
            favorite: false,
            start_mode: "parallel".into(),
            created_at: String::new(),
            updated_at: String::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Service {
    pub id: String,
    pub workspace_id: String,
    pub name: String,
    /// frontend | node | java | generic
    #[serde(rename = "type")]
    pub kind: String,
    /// frontend | backend
    pub role: String,
    pub cwd: String,
    pub command: String,
    pub args: Vec<String>,
    pub prepare: Vec<crate::launch::LaunchCommand>,
    pub package_manager: Option<String>,
    pub port: Option<u16>,
    pub env: std::collections::BTreeMap<String, String>,
    pub env_file: Option<String>,
    pub enabled: bool,
    pub dependencies: Vec<String>,
    pub startup_delay: u32,
    pub auto_open_browser: bool,
    pub open_url: Option<String>,
    /// 第二版为对象，如 {"type":"http","target":"http://localhost:3000"}
    pub health_check: Option<serde_json::Value>,
    pub shell_mode: bool,
    pub discovery: Option<serde_json::Value>,
    pub executable: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

impl Default for Service {
    fn default() -> Self {
        Self {
            id: String::new(),
            workspace_id: String::new(),
            name: String::new(),
            kind: "generic".into(),
            role: String::new(),
            cwd: String::new(),
            command: String::new(),
            args: Vec::new(),
            prepare: Vec::new(),
            package_manager: None,
            port: None,
            env: Default::default(),
            env_file: None,
            enabled: true,
            dependencies: Vec::new(),
            startup_delay: 0,
            auto_open_browser: false,
            open_url: None,
            health_check: None,
            shell_mode: false,
            discovery: None,
            executable: None,
            created_at: String::new(),
            updated_at: String::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryRecord {
    pub id: String,
    /// 查询类型：port / path / process 等
    pub kind: String,
    pub query: String,
    pub timestamp: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FavoriteRecord {
    pub id: String,
    pub kind: String,
    pub path: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DataPaths {
    pub install_dir: String,
    pub data_root: String,
    pub config_path: String,
    pub default_data_root: String,
    pub is_custom: bool,
    pub migrated_files: Vec<String>,
    pub requires_restart: bool,
}

pub fn now_iso() -> String {
    chrono::Local::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

pub fn new_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// 应用共享状态：存储文件位置 + 内存中的持久化数据。
pub struct AppState {
    pub install_dir: PathBuf,
    pub default_data_root: PathBuf,
    pub data_root: PathBuf,
    pub data: std::sync::Mutex<PersistFile>,
}

impl AppState {
    /// 数据根目录解析：环境变量 XCHEN_DATA_DIR > dataPath 设置 > 默认（exe 同级 data/）。
    pub fn new() -> Self {
        let install_dir = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(Path::to_path_buf))
            .unwrap_or_else(|| PathBuf::from("."));
        Self::open_at(
            install_dir,
            std::env::var("XCHEN_DATA_DIR")
                .ok()
                .filter(|s| !s.trim().is_empty())
                .map(PathBuf::from),
        )
    }

    pub(crate) fn open_at(install_dir: PathBuf, env_root: Option<PathBuf>) -> Self {
        let default_data_root = install_dir.join("data");
        let configured = read_store(&default_data_root.join("config.json"))
            .ok()
            .and_then(|s| s.settings.data_path)
            .filter(|s| !s.trim().is_empty())
            .map(PathBuf::from);
        let is_custom_env = env_root.is_some();
        let data_root = env_root
            .or(configured)
            .unwrap_or_else(|| default_data_root.clone());
        let config_path = data_root.join("config.json");
        let data = read_store(&config_path).unwrap_or_default();
        let state = Self {
            install_dir,
            default_data_root,
            data_root,
            data: std::sync::Mutex::new(data),
        };
        if is_custom_env {
            // 环境变量覆盖记入设置，保持 dataPath 一致
            if let Some(s) = state.data.lock().ok().as_mut() {
                if s.settings.data_path.is_none() {
                    s.settings.data_path = Some(state.data_root.to_string_lossy().to_string());
                }
            }
        }
        state
    }

    pub fn config_path(&self) -> PathBuf {
        self.data_root.join("config.json")
    }

    pub fn is_custom_data_root(&self) -> bool {
        self.data_root != self.default_data_root
    }

    /// 原子提交：写 config.json.tmp 后替换。
    pub fn commit(&self) -> Result<(), String> {
        // Serialize and replace under one lock to prevent concurrent IPC saves
        // from racing on the shared temporary file or reverting a newer snapshot.
        let data = self
            .data
            .lock()
            .map_err(|_| "Failed to open ConfigStore".to_string())?;
        write_store(&self.config_path(), &data)
    }
}

pub(crate) fn write_store(path: &Path, data: &PersistFile) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("Failed to create data dir: {e}"))?;
    }
    let json = serde_json::to_string_pretty(data)
        .map_err(|e| format!("Failed to serialize config: {e}"))?;
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, json).map_err(|e| format!("Failed to write config: {e}"))?;
    fs::rename(&tmp, path).map_err(|e| format!("Failed to commit config: {e}"))
}

fn read_store(path: &Path) -> Result<PersistFile, String> {
    let raw = fs::read_to_string(path).map_err(|e| format!("Failed to read config: {e}"))?;
    let store: PersistFile =
        serde_json::from_str(&raw).map_err(|e| format!("Failed to deserialize store. {e}"))?;
    Ok(store)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejected_settings_patch_preserves_current_settings() {
        let mut settings = Settings::default();
        let before = serde_json::to_value(&settings).unwrap();
        assert!(settings
            .apply_patch(SettingsPatch {
                max_log_lines: Some(1),
                theme: Some("light".into()),
                ..SettingsPatch::default()
            })
            .is_err());
        assert_eq!(serde_json::to_value(&settings).unwrap(), before);
    }
}
