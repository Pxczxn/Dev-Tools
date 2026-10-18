use std::path::Path;
use std::process::Command;

fn not_implemented(name: &str) -> String {
    format!("NOT_IMPLEMENTED:{name}")
}

#[tauri::command]
pub fn check_is_directory(path: String) -> Result<bool, String> {
    Ok(Path::new(&path).is_dir())
}

#[tauri::command]
pub fn check_is_file(path: String) -> Result<bool, String> {
    Ok(Path::new(&path).is_file())
}

/// 用资源管理器打开目录或文件（保持与第二版「打开目录」一致的行为）。
#[tauri::command]
pub fn open_path(path: String) -> Result<(), String> {
    let p = Path::new(&path);
    if !p.exists() {
        return Err(format!("路径不存在: {path}"));
    }
    Command::new("explorer")
        .arg(&path)
        .spawn()
        .map_err(|e| format!("Failed to open path: {e}"))?;
    Ok(())
}

#[tauri::command]
pub fn open_url(url: String) -> Result<(), String> {
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err(format!("不支持的 URL: {url}"));
    }
    spawn_no_window("cmd", &["/C", "start", "", &url])?;
    Ok(())
}

/// 在资源管理器中定位并选中该文件/目录。
#[tauri::command]
pub fn reveal_item_in_dir(path: String) -> Result<(), String> {
    if !Path::new(&path).exists() {
        return Err(format!("路径不存在: {path}"));
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        Command::new("explorer")
            .raw_arg(format!("/select,\"{path}\""))
            .creation_flags(0x0800_0000)
            .spawn()
            .map_err(|e| format!("Failed to open path: {e}"))?;
    }
    Ok(())
}

/// 释放被占用的目录/文件：枚举持有该路径句柄的进程并按 pids 终止。
/// 依赖句柄枚举（NtQuerySystemInformation），在进程工具模块就绪后接入。
#[tauri::command]
pub fn release_path(_path: String, _pids: Option<Vec<u32>>) -> Result<serde_json::Value, String> {
    Err(not_implemented("release_path"))
}

#[tauri::command]
pub fn query_file(_path: String) -> Result<serde_json::Value, String> {
    Err(not_implemented("query_file"))
}

#[tauri::command]
pub fn query_path(_path: String) -> Result<serde_json::Value, String> {
    Err(not_implemented("query_path"))
}

/// 输入校验：非空、不含 Windows 非法字符与控制字符。
#[tauri::command]
pub fn validate_path(path: String) -> Result<bool, String> {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return Ok(false);
    }
    let invalid = ['<', '>', '"', '|', '?', '*'];
    Ok(!trimmed.chars().any(|c| invalid.contains(&c) || c.is_control()))
}

pub(crate) fn spawn_no_window(program: &str, args: &[&str]) -> Result<(), String> {
    let mut cmd = Command::new(program);
    cmd.args(args);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    cmd.spawn().map_err(|e| format!("Failed to execute {program}: {e}"))?;
    Ok(())
}
