//! 本机 MySQL / Redis 的 Windows 服务发现与启停。
//! 只接受服务名可识别，或可执行文件是 mysqld.exe / redis-server.exe 的服务。

use serde::Serialize;
use std::time::{Duration, Instant};
use windows::core::{PCWSTR, PWSTR};
use windows::Win32::System::Services::{
    CloseServiceHandle, ControlService, EnumServicesStatusExW, OpenSCManagerW, OpenServiceW,
    QueryServiceConfigW, QueryServiceStatusEx, StartServiceW, ENUM_SERVICE_STATUS_PROCESSW,
    QUERY_SERVICE_CONFIGW, SC_ENUM_PROCESS_INFO, SC_HANDLE, SC_MANAGER_CONNECT,
    SC_MANAGER_ENUMERATE_SERVICE, SC_STATUS_PROCESS_INFO, SERVICE_PAUSE_CONTINUE, SERVICE_QUERY_CONFIG,
    SERVICE_QUERY_STATUS, SERVICE_START, SERVICE_STATE_ALL, SERVICE_STATUS, SERVICE_STATUS_PROCESS,
    SERVICE_STOP, SERVICE_WIN32, SERVICE_CONTROL_CONTINUE, SERVICE_CONTROL_STOP,
};

const WAIT_LIMIT: Duration = Duration::from_secs(45);
const QUERY_ACCESS: u32 = SERVICE_QUERY_CONFIG | SERVICE_QUERY_STATUS;
const CONTROL_ACCESS: u32 = QUERY_ACCESS | SERVICE_START | SERVICE_STOP | SERVICE_PAUSE_CONTINUE;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct InfraService {
    pub name: String,
    pub display_name: String,
    pub kind: String,
    pub state: String,
    pub state_label: String,
    pub start_type: String,
    pub start_type_label: String,
    pub binary_path: String,
    pub pid: u32,
    pub message: String,
}

#[derive(Debug, Clone)]
struct Snapshot {
    name: String,
    display_name: String,
    state: u32,
    pid: u32,
    start_type: u32,
    binary_path: String,
}

pub fn is_safe_service_name(name: &str) -> bool {
    let name = name.trim();
    !name.is_empty()
        && name.len() <= 256
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | ' '))
}

/// 从服务启动命令里取出可执行文件路径。路径可能带引号和后续参数。
pub fn executable_path(binary_path: &str) -> String {
    let trimmed = binary_path.trim();
    if let Some(rest) = trimmed.strip_prefix('"') {
        return rest.split('"').next().unwrap_or(rest).to_string();
    }
    let lower = trimmed.to_ascii_lowercase();
    if let Some(index) = lower.find(".exe") {
        let end = index + 4;
        if end == trimmed.len() || trimmed.as_bytes().get(end).is_some_and(|byte| byte.is_ascii_whitespace()) {
            return trimmed[..end].to_string();
        }
    }
    trimmed.split_whitespace().next().unwrap_or(trimmed).to_string()
}

/// 从服务启动命令里取出可执行文件名。
pub fn executable_name(binary_path: &str) -> String {
    let body = executable_path(binary_path);
    body.rsplit(['\\', '/'])
        .next()
        .unwrap_or(&body)
        .to_ascii_lowercase()
}

/// 找到与本机程序是同一个可执行文件的 MySQL 或 Redis 服务。
pub fn program_service<'a>(
    services: &'a [InfraService],
    kind: &str,
    executable: &str,
) -> Option<&'a InfraService> {
    let wanted = executable_path(executable);
    if wanted.is_empty() {
        return None;
    }
    services.iter().find(|service| {
        service.kind == kind && executable_path(&service.binary_path).eq_ignore_ascii_case(&wanted)
    })
}

/// 返回 `mysql` 或 `redis`。名称不安全，且可执行文件也不是这两个服务时返回 None。
pub fn classify(name: &str, binary_path: &str) -> Option<&'static str> {
    if !is_safe_service_name(name) {
        return None;
    }
    match executable_name(binary_path).as_str() {
        "mysqld.exe" => return Some("mysql"),
        "redis-server.exe" => return Some("redis"),
        _ => {}
    }
    let normalized = name.trim().to_ascii_lowercase();
    if normalized == "redis" {
        return Some("redis");
    }
    if normalized == "mysql" {
        return Some("mysql");
    }
    let rest = normalized
        .strip_prefix("mysql")?
        .trim_start_matches(['.', '-', '_']);
    if !rest.is_empty()
        && rest.len() <= 8
        && rest.chars().all(|c| c.is_ascii_digit() || c == '.')
    {
        Some("mysql")
    } else {
        None
    }
}

#[cfg(test)]
fn is_candidate(name: &str, display_name: &str) -> bool {
    if !is_safe_service_name(name) {
        return false;
    }
    let blob = format!("{name}\n{display_name}").to_ascii_lowercase();
    blob.contains("mysql") || blob.contains("redis")
}

pub fn state_key(code: u32) -> &'static str {
    match code {
        1 => "stopped",
        2 => "start_pending",
        3 => "stop_pending",
        4 => "running",
        5 => "continue_pending",
        6 => "pause_pending",
        7 => "paused",
        _ => "unknown",
    }
}

pub fn state_label(code: u32) -> &'static str {
    match state_key(code) {
        "stopped" => "已停止",
        "start_pending" => "正在启动",
        "stop_pending" => "正在停止",
        "running" => "运行中",
        "continue_pending" => "正在继续",
        "pause_pending" => "正在暂停",
        "paused" => "已暂停",
        _ => "状态未知",
    }
}

pub fn start_type_key(code: u32) -> &'static str {
    match code {
        2 => "auto",
        3 => "manual",
        4 => "disabled",
        _ => "other",
    }
}

pub fn start_type_label(code: u32) -> &'static str {
    match start_type_key(code) {
        "auto" => "自动启动",
        "manual" => "手动启动",
        "disabled" => "已禁用",
        _ => "其他启动方式",
    }
}

/// 把一次启停动作展开成不会误杀其他服务的步骤。空列表表示无需向服务控制管理器发命令。
pub fn control_plan(action: &str, state: &str) -> Result<Vec<&'static str>, String> {
    match action {
        "start" => match state {
            "running" | "start_pending" => Ok(vec![]),
            "paused" | "pause_pending" => Ok(vec!["continue", "wait_running"]),
            "stopped" => Ok(vec!["start", "wait_running"]),
            "stop_pending" => Ok(vec!["wait_stopped", "start", "wait_running"]),
            _ => Err(format!("当前状态“{state}”不能启动")),
        },
        "stop" => match state {
            "stopped" => Ok(vec![]),
            "stop_pending" => Ok(vec!["wait_stopped"]),
            "running" | "start_pending" | "paused" | "pause_pending" | "continue_pending" => {
                Ok(vec!["stop", "wait_stopped"])
            }
            _ => Err(format!("当前状态“{state}”不能停止")),
        },
        "restart" => {
            let mut steps = Vec::new();
            if state != "stopped" && state != "stop_pending" {
                steps.push("stop");
            }
            if state != "stopped" {
                steps.push("wait_stopped");
            }
            steps.extend_from_slice(&["start", "wait_running"]);
            Ok(steps)
        }
        _ => Err("只支持启动、停止和重启".into()),
    }
}

pub fn authorize<'a>(name: &'a str, binary_path: &str, action: &str, state: &str) -> Result<(String, Vec<&'static str>), String> {
    let kind = classify(name, binary_path).ok_or_else(|| {
        "拒绝控制：只能操作系统识别为 MySQL 或 Redis 的服务".to_string()
    })?;
    let steps = control_plan(action, state)?;
    Ok((kind.to_string(), steps))
}

pub fn list_managed() -> Result<Vec<InfraService>, String> {
    let scm = open_manager()?;
    let enumerated = enum_services(scm.0)?;
    let mut services = Vec::new();
    for (name, display_name, state, pid) in enumerated {
        if !is_safe_service_name(&name) {
            continue;
        }
        let (binary_path, start_type, configured_display) = match open_service(scm.0, &name, QUERY_ACCESS) {
            Ok(handle) => query_config(handle.0).unwrap_or_default(),
            Err(_) => (String::new(), 0, String::new()),
        };
        let display_name = if configured_display.is_empty() { display_name } else { configured_display };
        if classify(&name, &binary_path).is_none() {
            continue;
        }
        services.push(to_service(Snapshot {
            name,
            display_name,
            state,
            pid,
            start_type,
            binary_path,
        }, String::new()));
    }
    services.sort_by(|a, b| a.kind.cmp(&b.kind).then(a.name.cmp(&b.name)));
    Ok(services)
}

pub fn control_managed(name: &str, action: &str) -> Result<InfraService, String> {
    if !is_safe_service_name(name) {
        return Err("拒绝控制：服务名包含不允许的字符".into());
    }
    let scm = open_manager()?;
    let handle = open_service(scm.0, name, CONTROL_ACCESS)?;
    let (binary_path, start_type, configured_display) = query_config(handle.0)?;
    let status = query_status(handle.0)?;
    let state = state_key(status.dwCurrentState.0);
    let (kind, steps) = authorize(name, &binary_path, action, state)?;
    let mut note = match (action, state) {
        ("start", "running" | "start_pending") => "已经在运行。".to_string(),
        ("stop", "stopped") => "已经停止。".to_string(),
        _ => String::new(),
    };
    for step in steps {
        match step {
            "start" => unsafe { StartServiceW(handle.0, None) }.map_err(explain)?,
            "stop" => control(handle.0, SERVICE_CONTROL_STOP)?,
            "continue" => control(handle.0, SERVICE_CONTROL_CONTINUE)?,
            "wait_stopped" => {
                let after = wait_until(handle.0, true)?;
                if after.dwCurrentState.0 != 1 {
                    note = "停止指令已提交，服务仍在停止。".into();
                }
            }
            "wait_running" => {
                let after = wait_until(handle.0, false)?;
                if after.dwCurrentState.0 == 1 {
                    note = "服务没有保持运行。若启动类型是“已禁用”，请先在 Windows 服务里改为手动或自动。".into();
                } else if after.dwCurrentState.0 != 4 {
                    note = "启动指令已提交，服务仍在启动。".into();
                }
            }
            _ => {}
        }
    }
    let status = query_status(handle.0)?;
    let display_name = if configured_display.is_empty() { name.to_string() } else { configured_display };
    let mut service = to_service(Snapshot {
        name: name.to_string(),
        display_name,
        state: status.dwCurrentState.0,
        pid: status.dwProcessId,
        start_type,
        binary_path,
    }, note);
    service.kind = kind;
    Ok(service)
}

fn to_service(snapshot: Snapshot, message: String) -> InfraService {
    let kind = classify(&snapshot.name, &snapshot.binary_path)
        .unwrap_or("unknown")
        .to_string();
    InfraService {
        name: snapshot.name,
        display_name: if snapshot.display_name.is_empty() {
            String::new()
        } else {
            snapshot.display_name
        },
        kind,
        state: state_key(snapshot.state).to_string(),
        state_label: state_label(snapshot.state).to_string(),
        start_type: start_type_key(snapshot.start_type).to_string(),
        start_type_label: start_type_label(snapshot.start_type).to_string(),
        binary_path: snapshot.binary_path,
        pid: snapshot.pid,
        message,
    }
}

struct Scm(SC_HANDLE);
impl Drop for Scm {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseServiceHandle(self.0);
        }
    }
}
struct Service(SC_HANDLE);
impl Drop for Service {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseServiceHandle(self.0);
        }
    }
}

fn open_manager() -> Result<Scm, String> {
    let handle = unsafe {
        OpenSCManagerW(
            PCWSTR::null(),
            PCWSTR::null(),
            SC_MANAGER_CONNECT | SC_MANAGER_ENUMERATE_SERVICE,
        )
    }
    .map_err(explain)?;
    Ok(Scm(handle))
}

fn open_service(scm: SC_HANDLE, name: &str, access: u32) -> Result<Service, String> {
    let wide: Vec<u16> = std::os::windows::ffi::OsStrExt::encode_wide(std::ffi::OsStr::new(name))
        .chain(Some(0))
        .collect();
    let handle = unsafe { OpenServiceW(scm, PCWSTR(wide.as_ptr()), access) }.map_err(explain)?;
    Ok(Service(handle))
}

fn enum_services(scm: SC_HANDLE) -> Result<Vec<(String, String, u32, u32)>, String> {
    let mut resume = 0u32;
    let mut capacity: usize = 64 * 1024;
    let mut listed = Vec::new();
    loop {
        let resume_before = resume;
        let mut needed = 0u32;
        let mut count = 0u32;
        let mut storage = vec![0u64; capacity.div_ceil(8)];
        let view = unsafe {
            std::slice::from_raw_parts_mut(storage.as_mut_ptr() as *mut u8, storage.len() * 8)
        };
        let result = unsafe {
            EnumServicesStatusExW(
                scm,
                SC_ENUM_PROCESS_INFO,
                SERVICE_WIN32,
                SERVICE_STATE_ALL,
                Some(view),
                &mut needed,
                &mut count,
                Some(&mut resume),
                PCWSTR::null(),
            )
        };
        match result {
            Ok(()) => {
                listed.extend(read_entries(view, count));
                break;
            }
            Err(err) if win32_code(&err) == 234 && count == 0 => {
                resume = resume_before;
                let next = (needed as usize).max(capacity.saturating_mul(2));
                if next <= capacity {
                    return Err(explain(err));
                }
                capacity = next;
            }
            Err(err) if win32_code(&err) == 234 => {
                listed.extend(read_entries(view, count));
                if resume == resume_before {
                    return Err(explain(err));
                }
            }
            Err(err) => return Err(explain(err)),
        }
    }
    Ok(listed)
}

fn read_entries(buf: &[u8], count: u32) -> Vec<(String, String, u32, u32)> {
    let entries = unsafe {
        std::slice::from_raw_parts(
            buf.as_ptr() as *const ENUM_SERVICE_STATUS_PROCESSW,
            count as usize,
        )
    };
    entries
        .iter()
        .map(|entry| {
            (
                wide_string(entry.lpServiceName),
                wide_string(entry.lpDisplayName),
                entry.ServiceStatusProcess.dwCurrentState.0,
                entry.ServiceStatusProcess.dwProcessId,
            )
        })
        .collect()
}

fn query_config(service: SC_HANDLE) -> Result<(String, u32, String), String> {
    let mut needed = 0u32;
    let _ = unsafe { QueryServiceConfigW(service, None, 0, &mut needed) };
    if needed == 0 {
        return Err("无法读取服务配置".into());
    }
    let mut storage = vec![0u64; (needed as usize).div_ceil(8) + 1];
    let size = (storage.len() * 8) as u32;
    unsafe {
        QueryServiceConfigW(
            service,
            Some(storage.as_mut_ptr() as *mut QUERY_SERVICE_CONFIGW),
            size,
            &mut needed,
        )
    }
    .map_err(explain)?;
    let config = unsafe { &*(storage.as_ptr() as *const QUERY_SERVICE_CONFIGW) };
    Ok((
        wide_string(config.lpBinaryPathName),
        config.dwStartType.0,
        wide_string(config.lpDisplayName),
    ))
}

fn query_status(service: SC_HANDLE) -> Result<SERVICE_STATUS_PROCESS, String> {
    let mut status = SERVICE_STATUS_PROCESS::default();
    let mut needed = 0u32;
    let view = unsafe {
        std::slice::from_raw_parts_mut(
            &mut status as *mut SERVICE_STATUS_PROCESS as *mut u8,
            std::mem::size_of::<SERVICE_STATUS_PROCESS>(),
        )
    };
    unsafe { QueryServiceStatusEx(service, SC_STATUS_PROCESS_INFO, Some(view), &mut needed) }
        .map_err(explain)?;
    Ok(status)
}

fn control(service: SC_HANDLE, code: u32) -> Result<(), String> {
    let mut status = SERVICE_STATUS::default();
    unsafe { ControlService(service, code, &mut status) }.map_err(explain)
}

fn wait_until(service: SC_HANDLE, want_stopped: bool) -> Result<SERVICE_STATUS_PROCESS, String> {
    let deadline = Instant::now() + WAIT_LIMIT;
    loop {
        let status = query_status(service)?;
        let code = status.dwCurrentState.0;
        if want_stopped && code == 1 {
            return Ok(status);
        }
        if !want_stopped && (code == 4 || code == 1) {
            return Ok(status);
        }
        if Instant::now() >= deadline {
            return Ok(status);
        }
        std::thread::sleep(Duration::from_millis(300));
    }
}

fn wide_string(value: PWSTR) -> String {
    if value.is_null() {
        return String::new();
    }
    unsafe { value.to_string() }.unwrap_or_default()
}

fn win32_code(err: &windows::core::Error) -> u32 {
    let code = err.code().0 as u32;
    if code & 0xFFFF_0000 == 0x8007_0000 {
        code & 0xFFFF
    } else {
        code
    }
}

fn explain(err: windows::core::Error) -> String {
    let text = err.to_string();
    match win32_code(&err) {
        5 => format!("{text} 请关闭 Dev-Tools 后，右键程序并选择“以管理员身份运行”，再操作 MySQL 或 Redis。"),
        1060 => format!("找不到这个 Windows 服务。{text}"),
        1058 => format!("{text} 该服务已被禁用，请先在 Windows“服务”里把启动类型改为手动或自动。"),
        _ => text,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_official_names_and_server_executables() {
        assert_eq!(classify("MySQL80", ""), Some("mysql"));
        assert_eq!(classify("MySQL8.0", ""), Some("mysql"));
        assert_eq!(classify("MySQL5.7", ""), Some("mysql"));
        assert_eq!(classify("Redis", ""), Some("redis"));
        assert_eq!(
            classify("phpstudy_mysql", r#""D:\phpstudy\MySQL\bin\mysqld.exe" --defaults-file=my.ini"#),
            Some("mysql")
        );
        assert_eq!(
            classify("cache", r"C:\Redis\redis-server.exe --service-run"),
            Some("redis")
        );
        assert_eq!(classify("Spooler", r"C:\Windows\System32\spoolsv.exe"), None);
        assert_eq!(classify("MySQLRouter", r"C:\MySQL\bin\mysqlrouter.exe"), None);
        assert_eq!(classify("MySQL80 & calc", ""), None);
        assert_eq!(executable_name(r#""C:\Program Files\MySQL\bin\mysqld.exe" --console"#), "mysqld.exe");
        let installed = r#""C:\Program Files\MySQL\MySQL Server 8.0\bin\mysqld.exe" --defaults-file="C:\ProgramData\MySQL\MySQL Server 8.0\my.ini" pxczxn"#;
        assert!(!is_candidate("pxczxn", "pxczxn"));
        assert_eq!(classify("pxczxn", installed), Some("mysql"));
        assert!(executable_path(installed).eq_ignore_ascii_case(
            r"C:\Program Files\MySQL\MySQL Server 8.0\bin\mysqld.exe"
        ));
    }

    #[test]
    fn program_service_matches_the_same_executable_ignoring_the_service_name() {
        let services = vec![InfraService {
            name: "pxczxn".into(),
            display_name: "pxczxn".into(),
            kind: "mysql".into(),
            state: "running".into(),
            state_label: "运行中".into(),
            start_type: "auto".into(),
            start_type_label: "自动启动".into(),
            binary_path: r#""C:\Program Files\MySQL\MySQL Server 8.0\bin\mysqld.exe" --defaults-file="C:\ProgramData\MySQL\MySQL Server 8.0\my.ini" pxczxn"#.into(),
            pid: 4,
            message: String::new(),
        }];
        let matched = program_service(
            &services,
            "mysql",
            r"C:\Program Files\MySQL\MySQL Server 8.0\bin\mysqld.exe",
        );
        assert_eq!(matched.map(|service| service.name.as_str()), Some("pxczxn"));
        assert!(program_service(&services, "redis", r"C:\Redis\redis-server.exe").is_none());
    }

    #[test]
    fn state_and_control_plan_cover_start_stop_restart() {
        assert_eq!(state_key(4), "running");
        assert_eq!(state_label(1), "已停止");
        assert_eq!(state_label(4), "运行中");
        assert_eq!(start_type_label(4), "已禁用");
        assert_eq!(control_plan("start", "stopped").unwrap(), vec!["start", "wait_running"]);
        assert!(control_plan("start", "running").unwrap().is_empty());
        assert_eq!(control_plan("stop", "running").unwrap(), vec!["stop", "wait_stopped"]);
        assert_eq!(
            control_plan("restart", "running").unwrap(),
            vec!["stop", "wait_stopped", "start", "wait_running"]
        );
        assert_eq!(
            control_plan("restart", "stopped").unwrap(),
            vec!["start", "wait_running"]
        );
        assert!(control_plan("delete", "running").is_err());
    }

    #[test]
    fn unsafe_service_names_are_rejected_before_opening_the_manager() {
        let err = control_managed("Spooler & calc", "stop").unwrap_err();
        assert!(err.contains("不允许"));
    }

    #[test]
    fn authorization_rejects_unrelated_services_before_any_control_step() {
        let err = authorize("Spooler", r"C:\Windows\System32\spoolsv.exe", "stop", "running").unwrap_err();
        assert!(err.contains("拒绝控制"));
        let (kind, steps) = authorize("MySQL80", "", "restart", "running").unwrap();
        assert_eq!(kind, "mysql");
        assert_eq!(steps, vec!["stop", "wait_stopped", "start", "wait_running"]);
        assert!(authorize("Redis", "", "launch", "stopped").is_err());
    }

    #[test]
    fn local_service_enumeration_returns_only_mysql_or_redis() {
        let services = list_managed().expect("service enumeration");
        assert!(services.iter().all(|service| service.kind == "mysql" || service.kind == "redis"));
        assert!(services.iter().all(|service| classify(&service.name, &service.binary_path).is_some()));
        let installed = std::path::Path::new(r"C:\Program Files\MySQL\MySQL Server 8.0\bin\mysqld.exe");
        if installed.is_file() {
            let expected = installed.to_str().unwrap();
            assert!(
                services.iter().any(|service| {
                    service.kind == "mysql"
                        && executable_path(&service.binary_path).eq_ignore_ascii_case(expected)
                }),
                "已安装的 mysqld 没有进入服务列表：{:?}",
                services.iter().map(|service| service.name.clone()).collect::<Vec<_>>()
            );
        }
    }
}
