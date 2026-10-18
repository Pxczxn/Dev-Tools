use crate::runtime::RuntimeManager;
use crate::store::{new_id, now_iso, AppState, Service};
use serde::Deserialize;
use tauri::{AppHandle, State};

fn default_true() -> bool {
    true
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceInput {
    pub workspace_id: String,
    pub name: String,
    #[serde(default)]
    #[serde(rename = "type", alias = "kind")]
    pub kind: String,
    #[serde(default)]
    pub role: String,
    pub cwd: String,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub prepare: Vec<crate::launch::LaunchCommand>,
    #[serde(default)]
    pub package_manager: Option<String>,
    #[serde(default)]
    pub port: Option<u16>,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub dependencies: Vec<String>,
    #[serde(default)]
    pub startup_delay: u32,
    #[serde(default)]
    pub env: std::collections::BTreeMap<String, String>,
    #[serde(default)]
    pub env_file: Option<String>,
    #[serde(default)]
    pub shell_mode: bool,
    #[serde(default)]
    pub open_url: Option<String>,
    #[serde(default)]
    pub auto_open_browser: bool,
    #[serde(default)]
    pub health_check: Option<serde_json::Value>,
    #[serde(default)]
    pub discovery: Option<serde_json::Value>,
}

fn validate_service_input(input: &ServiceInput) -> Result<(), String> {
    if input.name.trim().is_empty() {
        return Err("Service name cannot be empty".into());
    }
    if input.cwd.trim().is_empty() {
        return Err("Service cwd cannot be empty".into());
    }
    if input.command.trim().is_empty() {
        return Err("Service command cannot be empty".into());
    }
    if let Some(port) = input.port {
        if port == 0 {
            return Err("Service port must be between 1 and 65535".into());
        }
    }
    for step in &input.prepare {
        if step.cwd.trim().is_empty() || step.command.trim().is_empty() {
            return Err("前置步骤的工作目录和命令不能为空".into());
        }
    }
    if input.startup_delay > 60000 {
        return Err(format!(
            "Service startupDelay must be between 0 and 60000, got {}",
            input.startup_delay
        ));
    }
    Ok(())
}

fn service_from_input(mut input: ServiceInput) -> Result<Service, String> {
    validate_service_input(&input)?;
    if input.command == "npm"
        && input.args.is_empty()
        && input
            .discovery
            .as_ref()
            .and_then(|d| d.get("managed"))
            .and_then(|v| v.as_bool())
            == Some(true)
    {
        return Err(format!(
            "{} 未发现可用的启动脚本，请手动配置启动命令后添加",
            input.name
        ));
    }
    input.workspace_id = input.workspace_id.trim().to_string();
    // 旧前端只传主命令；仅对与扫描推荐完全一致的托管服务补齐构建步骤。
    if input.prepare.is_empty()
        && input
            .discovery
            .as_ref()
            .and_then(|d| d.get("managed"))
            .and_then(|v| v.as_bool())
            == Some(true)
    {
        if let Some(plan) = crate::launch::java_options(std::path::Path::new(&input.cwd)).first() {
            if plan.launch.command == input.command && plan.launch.args == input.args {
                input.prepare = plan.prepare.clone();
            }
        }
    }
    Ok(Service {
        id: new_id(),
        workspace_id: input.workspace_id,
        name: input.name.trim().to_string(),
        kind: if input.kind.is_empty() {
            "generic".into()
        } else {
            input.kind
        },
        role: input.role,
        cwd: input.cwd.trim().to_string(),
        command: input.command.trim().to_string(),
        args: input.args,
        prepare: input.prepare,
        package_manager: input.package_manager,
        port: input.port,
        enabled: input.enabled,
        dependencies: input.dependencies,
        startup_delay: input.startup_delay,
        env: input.env,
        env_file: input.env_file,
        auto_open_browser: input.auto_open_browser,
        open_url: input.open_url,
        health_check: input.health_check,
        shell_mode: input.shell_mode,
        discovery: input.discovery,
        executable: None,
        created_at: now_iso(),
        updated_at: now_iso(),
    })
}

// ---------- service CRUD ----------

#[tauri::command]
pub fn create_service(state: State<'_, AppState>, input: ServiceInput) -> Result<Service, String> {
    let service = service_from_input(input)?;
    let mut data = state
        .data
        .lock()
        .map_err(|_| "Failed to open ConfigStore".to_string())?;
    data.services.push(service.clone());
    drop(data);
    state.commit()?;
    Ok(service)
}

/// 批量导入扫描发现的服务（第二版前端把候选转成 ServiceInput 数组后调用）。
#[tauri::command]
pub fn apply_discovery(
    state: State<'_, AppState>,
    workspace_id: String,
    services: Vec<ServiceInput>,
) -> Result<Vec<Service>, String> {
    let mut created = Vec::new();
    for mut input in services {
        input.workspace_id = workspace_id.clone();
        created.push(service_from_input(input)?);
    }
    let mut data = state
        .data
        .lock()
        .map_err(|_| "Failed to open ConfigStore".to_string())?;
    data.services.extend(created.iter().cloned());
    drop(data);
    state.commit()?;
    Ok(created)
}

#[tauri::command]
pub fn update_service(
    state: State<'_, AppState>,
    input: serde_json::Value,
) -> Result<Service, String> {
    let id = input
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or("Missing service id")?
        .to_string();
    let mut data = state
        .data
        .lock()
        .map_err(|_| "Failed to open ConfigStore".to_string())?;
    let service = data
        .services
        .iter_mut()
        .find(|s| s.id == id)
        .ok_or_else(|| format!("Service not found: {id}"))?;
    patch_service(service, &input)?;
    let updated = service.clone();
    drop(data);
    state.commit()?;
    Ok(updated)
}

fn patch_service(service: &mut Service, input: &serde_json::Value) -> Result<(), String> {
    let mut merged = serde_json::to_value(&*service).map_err(|e| e.to_string())?;
    for key in [
        "name",
        "cwd",
        "command",
        "role",
        "type",
        "args",
        "prepare",
        "packageManager",
        "port",
        "env",
        "envFile",
        "enabled",
        "dependencies",
        "startupDelay",
        "autoOpenBrowser",
        "openUrl",
        "healthCheck",
        "shellMode",
        "discovery",
        "executable",
    ] {
        if let Some(value) = input.get(key) {
            merged[key] = value.clone();
        }
    }
    let mut updated: Service = serde_json::from_value(merged.clone())
        .map_err(|e| format!("Invalid service input: {e}"))?;
    let validated: ServiceInput =
        serde_json::from_value(merged).map_err(|e| format!("Invalid service input: {e}"))?;
    validate_service_input(&validated)?;
    updated.name = updated.name.trim().to_string();
    updated.cwd = updated.cwd.trim().to_string();
    updated.command = updated.command.trim().to_string();
    updated.updated_at = now_iso();
    *service = updated;
    Ok(())
}

#[tauri::command]
pub fn delete_service(state: State<'_, AppState>, id: String) -> Result<(), String> {
    let mut data = state
        .data
        .lock()
        .map_err(|_| "Failed to open ConfigStore".to_string())?;
    let before = data.services.len();
    data.services.retain(|s| s.id != id);
    if data.services.len() == before {
        return Err(format!("Service not found: {id}"));
    }
    drop(data);
    state.commit()
}

/// workspaceId 缺省时返回全部服务（第二版首屏如此加载）。
#[tauri::command]
pub fn list_services(
    state: State<'_, AppState>,
    workspace_id: Option<String>,
) -> Result<Vec<Service>, String> {
    let data = state
        .data
        .lock()
        .map_err(|_| "Failed to open ConfigStore".to_string())?;
    Ok(match workspace_id {
        Some(id) if !id.is_empty() => data
            .services
            .iter()
            .filter(|s| s.workspace_id == id)
            .cloned()
            .collect(),
        _ => data.services.clone(),
    })
}

// ---------- 启停编排 ----------

fn max_log_lines(state: &AppState) -> usize {
    state
        .data
        .lock()
        .map(|d| d.settings.max_log_lines as usize)
        .unwrap_or(5000)
}

fn start_one(
    app: &AppHandle,
    state: &AppState,
    runtime: &RuntimeManager,
    service: &Service,
) -> Result<serde_json::Value, String> {
    let env = service_environment(service)?;
    let state_json = runtime.start(
        app,
        crate::runtime::StartSpec {
            service_id: &service.id,
            cwd: &service.cwd,
            command: &service.command,
            args: &service.args,
            shell_mode: service.shell_mode,
            env: &env,
            cap: max_log_lines(state),
            startup_delay: service.startup_delay,
            prepare: &service.prepare,
        },
    )?;
    if let Some(url) = service
        .open_url
        .clone()
        .filter(|_| service.auto_open_browser)
    {
        if let Some(session) = runtime.get(&service.id) {
            let port = service.port;
            std::thread::spawn(move || {
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(180);
                while std::time::Instant::now() < deadline {
                    let status = session.status();
                    if !matches!(status.as_str(), "starting" | "running") {
                        return;
                    }
                    let listening = port.is_none_or(|p| {
                        std::net::TcpStream::connect_timeout(
                            &std::net::SocketAddr::from(([127, 0, 0, 1], p)),
                            std::time::Duration::from_millis(100),
                        )
                        .is_ok()
                    });
                    if status == "running" && listening {
                        let _ = crate::commands::path::open_url(url);
                        return;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(250));
                }
            });
        }
    }
    Ok(serde_json::to_value(state_json).unwrap_or(serde_json::Value::Null))
}

#[tauri::command]
pub fn start_process(
    app: AppHandle,
    state: State<'_, AppState>,
    runtime: State<'_, RuntimeManager>,
    service_id: String,
) -> Result<serde_json::Value, String> {
    let service = state
        .data
        .lock()
        .map_err(|_| "Failed to open ConfigStore".to_string())?
        .services
        .iter()
        .find(|s| s.id == service_id)
        .cloned()
        .ok_or_else(|| format!("Service not found: {service_id}"))?;
    start_one(&app, &state, &runtime, &service)
}

#[tauri::command]
pub fn stop_process(
    app: AppHandle,
    runtime: State<'_, RuntimeManager>,
    service_id: String,
) -> Result<serde_json::Value, String> {
    runtime
        .stop(&app, &service_id)
        .and_then(|s| serde_json::to_value(s).map_err(|e| e.to_string()))
}

#[tauri::command]
pub async fn restart_process(
    app: AppHandle,
    state: State<'_, AppState>,
    runtime: State<'_, RuntimeManager>,
    service_id: String,
) -> Result<serde_json::Value, String> {
    let service = state
        .data
        .lock()
        .map_err(|_| "Failed to open ConfigStore".to_string())?
        .services
        .iter()
        .find(|s| s.id == service_id)
        .cloned()
        .ok_or_else(|| format!("Service not found: {service_id}"))?;
    if runtime.get(&service_id).is_some() {
        runtime.stop(&app, &service_id)?;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while runtime
            .get(&service_id)
            .is_some_and(|s| matches!(s.status().as_str(), "starting" | "running" | "stopping"))
        {
            if std::time::Instant::now() >= deadline {
                return Err("服务未能及时停止，已取消重启".into());
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }
    start_one(&app, &state, &runtime, &service)
}

#[tauri::command]
pub fn force_kill_process(
    _app: AppHandle,
    runtime: State<'_, RuntimeManager>,
    service_id: String,
) -> Result<serde_json::Value, String> {
    runtime
        .stop(&_app, &service_id)
        .and_then(|s| serde_json::to_value(s).map_err(|e| e.to_string()))
}

#[tauri::command]
pub fn stop_all_processes(
    app: AppHandle,
    runtime: State<'_, RuntimeManager>,
) -> Result<serde_json::Value, String> {
    runtime.cancel_all_workspace_launches();
    let mut stopped = 0;
    for id in runtime.running_service_ids() {
        if runtime.stop(&app, &id).is_ok() {
            stopped += 1;
        }
    }
    Ok(serde_json::json!({ "stopped": stopped }))
}

/// 一键启动工作区：parallel 模式全部并发；sequential 模式按依赖与顺序、
/// 服务间隔 settings.startupInterval 毫秒。
#[tauri::command]
pub async fn start_workspace(
    app: AppHandle,
    state: State<'_, AppState>,
    runtime: State<'_, RuntimeManager>,
    workspace_id: String,
) -> Result<serde_json::Value, String> {
    let (start_mode, interval) = {
        let data = state
            .data
            .lock()
            .map_err(|_| "Failed to open ConfigStore".to_string())?;
        let ws = data
            .workspaces
            .iter()
            .find(|w| w.id == workspace_id)
            .ok_or_else(|| format!("Workspace not found: {workspace_id}"))?;
        (ws.start_mode.clone(), data.settings.startup_interval)
    };
    let services: Vec<Service> = {
        let data = state
            .data
            .lock()
            .map_err(|_| "Failed to open ConfigStore".to_string())?;
        data.services
            .iter()
            .filter(|s| s.workspace_id == workspace_id && s.enabled)
            .cloned()
            .collect()
    };

    let ordered = order_services(&services)?;
    let launch = runtime.begin_workspace_launch(&workspace_id)?;

    let mut results = Vec::new();
    let mut failures = Vec::new();
    let parallel = start_mode != "sequential";
    for (index, service) in ordered.iter().enumerate() {
        if launch.is_cancelled() {
            break;
        }
        if !parallel && index > 0 {
            let deadline =
                std::time::Instant::now() + std::time::Duration::from_millis(interval as u64);
            while std::time::Instant::now() < deadline && !launch.is_cancelled() {
                std::thread::sleep(std::time::Duration::from_millis(25));
            }
        }
        if launch.is_cancelled() {
            break;
        }
        if let Err(error) = wait_for_dependencies(
            service,
            &services,
            &runtime,
            &launch,
            std::time::Duration::from_secs(180),
        ) {
            failures.push(format!("{}: {}", service.name, error));
            continue;
        }
        if launch.is_cancelled() {
            break;
        }
        match start_one(&app, &state, &runtime, service) {
            Ok(r) => results.push(r),
            Err(e) => failures.push(format!("{}: {}", service.name, e)),
        }
    }
    Ok(serde_json::json!({
        "started": results.len(),
        "failed": failures.len(),
        "results": results,
        "failures": failures,
        "cancelled": launch.is_cancelled(),
    }))
}

fn wait_for_dependencies(
    service: &Service,
    services: &[Service],
    runtime: &RuntimeManager,
    launch: &crate::runtime::WorkspaceLaunch<'_>,
    timeout: std::time::Duration,
) -> Result<(), String> {
    let deadline = std::time::Instant::now() + timeout;
    for id in &service.dependencies {
        let dependency = services
            .iter()
            .find(|s| &s.id == id)
            .ok_or_else(|| format!("依赖不存在：{id}"))?;
        loop {
            if launch.is_cancelled() {
                return Err("工作区批量启动已取消".into());
            }
            let session = runtime
                .get(id)
                .ok_or_else(|| format!("依赖 {} 尚未启动", dependency.name))?;
            let status = session.status();
            if !matches!(status.as_str(), "starting" | "running") {
                return Err(format!(
                    "依赖 {} 未运行（{status}），已取消启动",
                    dependency.name
                ));
            }
            let listening = dependency.port.is_none_or(|p| {
                std::net::TcpStream::connect_timeout(
                    &std::net::SocketAddr::from(([127, 0, 0, 1], p)),
                    std::time::Duration::from_millis(100),
                )
                .is_ok()
            });
            if status == "running" && listening {
                break;
            }
            if std::time::Instant::now() >= deadline {
                return Err(format!("等待依赖 {} 就绪超时", dependency.name));
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }
    Ok(())
}

/// Validate all dependencies before launching anything, then traverse the full graph.
fn order_services(services: &[Service]) -> Result<Vec<Service>, String> {
    fn visit(
        index: usize,
        services: &[Service],
        marks: &mut [u8],
        ordered: &mut Vec<Service>,
    ) -> Result<(), String> {
        if marks[index] == 2 {
            return Ok(());
        }
        if marks[index] == 1 {
            return Err(format!("服务依赖存在循环：{}", services[index].name));
        }
        marks[index] = 1;
        for dep in &services[index].dependencies {
            let target = services.iter().position(|s| &s.id == dep).ok_or_else(|| {
                format!(
                    "{} 的依赖 {} 不存在、已禁用或不在当前工作区",
                    services[index].name, dep
                )
            })?;
            visit(target, services, marks, ordered)?;
        }
        marks[index] = 2;
        ordered.push(services[index].clone());
        Ok(())
    }
    let mut ordered = Vec::new();
    let mut marks = vec![0; services.len()];
    for index in 0..services.len() {
        visit(index, services, &mut marks, &mut ordered)?;
    }
    Ok(ordered)
}

fn service_environment(
    service: &Service,
) -> Result<std::collections::BTreeMap<String, String>, String> {
    let mut env = service.env.clone();
    if let Some(file) = service.env_file.as_ref().filter(|s| !s.trim().is_empty()) {
        let path = std::path::Path::new(&service.cwd).join(file);
        let text = std::fs::read_to_string(&path)
            .map_err(|e| format!("无法读取环境变量文件 {}：{e}", path.display()))?;
        for raw in text.trim_start_matches('\u{feff}').lines() {
            let line = raw.trim().strip_prefix("export ").unwrap_or(raw.trim());
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some((key, value)) = line.split_once('=') {
                let key = key.trim();
                if key.is_empty() {
                    continue;
                }
                let value = value.trim();
                let value = if value.len() >= 2
                    && ((value.starts_with('"') && value.ends_with('"'))
                        || (value.starts_with('\'') && value.ends_with('\'')))
                {
                    &value[1..value.len() - 1]
                } else {
                    value
                };
                env.entry(key.into()).or_insert_with(|| value.into());
            }
        }
    }
    Ok(env)
}

fn workspace_running_ids(
    services: &[Service],
    workspace_id: &str,
    runtime: &RuntimeManager,
) -> Vec<String> {
    let running = runtime.running_service_ids();
    services
        .iter()
        .filter(|s| s.workspace_id == workspace_id && running.contains(&s.id))
        .map(|s| s.id.clone())
        .collect()
}

#[tauri::command]
pub fn stop_workspace(
    app: AppHandle,
    state: State<'_, AppState>,
    runtime: State<'_, RuntimeManager>,
    workspace_id: String,
) -> Result<serde_json::Value, String> {
    runtime.cancel_workspace_launch(&workspace_id);
    let services = state
        .data
        .lock()
        .map_err(|_| "Failed to open ConfigStore".to_string())?
        .services
        .clone();
    let mut stopped = 0;
    for id in workspace_running_ids(&services, &workspace_id, &runtime) {
        if runtime.stop(&app, &id).is_ok() {
            stopped += 1;
        }
    }
    Ok(serde_json::json!({ "stopped": stopped }))
}

#[tauri::command]
pub fn stop_all_workspaces(
    app: AppHandle,
    runtime: State<'_, RuntimeManager>,
) -> Result<serde_json::Value, String> {
    runtime.cancel_all_workspace_launches();
    let mut stopped = 0;
    for id in runtime.running_service_ids() {
        if runtime.stop(&app, &id).is_ok() {
            stopped += 1;
        }
    }
    Ok(serde_json::json!({ "stopped": stopped }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn service(id: &str, workspace_id: &str) -> Service {
        Service {
            id: id.into(),
            workspace_id: workspace_id.into(),
            name: "app".into(),
            cwd: ".".into(),
            command: "node".into(),
            ..Service::default()
        }
    }

    #[test]
    fn discovery_import_restores_prepare_steps_and_preserves_custom_commands() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let dir = root.join("api");
        std::fs::create_dir_all(dir.join("src/main/java")).unwrap();
        std::fs::write(
            root.join("pom.xml"),
            "<project><modules><module>api</module></modules></project>",
        )
        .unwrap();
        std::fs::write(dir.join("pom.xml"), "<project><build><plugins><plugin><artifactId>spring-boot-maven-plugin</artifactId></plugin></plugins></build></project>").unwrap();
        std::fs::write(
            dir.join("src/main/java/App.java"),
            "SpringApplication.run(App.class, args);",
        )
        .unwrap();
        let mut input = json!({"workspaceId":"one", "name":"api", "type":"java", "cwd":dir, "command":"mvn", "args":["spring-boot:run"], "discovery":{"managed":true}, "autoOpenBrowser":true});
        let service = service_from_input(serde_json::from_value(input.clone()).unwrap()).unwrap();
        assert_eq!(service.kind, "java");
        assert_eq!(service.prepare.len(), 1);
        assert!(service.auto_open_browser);
        input["args"] = json!(["custom:run"]);
        assert!(service_from_input(serde_json::from_value(input).unwrap())
            .unwrap()
            .prepare
            .is_empty());
        assert!(service_from_input(serde_json::from_value(json!({"workspaceId":"one", "name":"unknown", "cwd":root, "command":"npm", "discovery":{"managed":true}})).unwrap()).is_err());
    }

    #[test]
    fn dependent_launch_waits_for_preparation_and_rejects_failed_dependency() {
        let mut app = service("app", "one");
        app.dependencies = vec!["db".into()];
        let services = vec![app.clone(), service("db", "one")];
        let runtime = RuntimeManager::new();
        let launch = runtime.begin_workspace_launch("one").unwrap();
        let dependency = runtime.ensure_session("db", 500);
        dependency.set_status("starting");
        let ready = dependency.clone();
        let thread = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(30));
            ready.set_status("running");
        });
        assert!(wait_for_dependencies(
            &app,
            &services,
            &runtime,
            &launch,
            std::time::Duration::from_secs(1)
        )
        .is_ok());
        thread.join().unwrap();
        dependency.set_status("failed");
        assert!(wait_for_dependencies(
            &app,
            &services,
            &runtime,
            &launch,
            std::time::Duration::from_secs(1)
        )
        .is_err());
        dependency.set_status("starting");
        assert!(wait_for_dependencies(
            &app,
            &services,
            &runtime,
            &launch,
            std::time::Duration::from_millis(1)
        )
        .is_err());
    }

    #[test]
    fn dependency_order_is_recursive_unique_and_rejects_invalid_graphs() {
        let mut a = service("a", "one");
        a.dependencies = vec!["b".into(), "c".into()];
        let mut b = service("b", "one");
        b.dependencies = vec!["c".into()];
        let mut c = service("c", "one");
        assert_eq!(
            order_services(&[a.clone(), b.clone(), c.clone()])
                .unwrap()
                .iter()
                .map(|s| s.id.as_str())
                .collect::<Vec<_>>(),
            vec!["c", "b", "a"]
        );
        c.dependencies = vec!["a".into()];
        assert!(order_services(&[a.clone(), b.clone(), c]).is_err());
        assert!(order_services(&[a, b]).is_err());
    }

    #[test]
    fn environment_file_quotes_bom_and_overrides_are_preserved() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(
            temp.path().join(".env"),
            "\u{feff}export TOKEN=\"secret value\"\nPORT='7779'\nPORT=1\n# comment\n",
        )
        .unwrap();
        let mut s = service("a", "one");
        s.cwd = temp.path().to_string_lossy().into();
        s.env_file = Some(".env".into());
        s.env.insert("PORT".into(), "9000".into());
        let env = service_environment(&s).unwrap();
        assert_eq!(env["TOKEN"], "secret value");
        assert_eq!(env["PORT"], "9000");
        s.env_file = Some("missing.env".into());
        assert!(service_environment(&s).is_err());
    }

    #[test]
    fn stopping_workspace_selects_only_its_running_services() {
        let runtime = RuntimeManager::new();
        runtime.ensure_session("a", 500).set_status("running");
        runtime.ensure_session("b", 500).set_status("running");
        runtime.ensure_session("idle", 500);
        let services = vec![
            service("a", "one"),
            service("b", "two"),
            service("idle", "one"),
        ];
        assert_eq!(workspace_running_ids(&services, "one", &runtime), vec!["a"]);
        assert!(workspace_running_ids(&services, "missing", &runtime).is_empty());
    }

    #[test]
    fn editing_service_saves_launch_arguments_and_environment() {
        let mut service = service("a", "one");
        patch_service(&mut service, &json!({"args":["run", "dev"], "env":{"PORT":"3001"}, "envFile":".env.local", "shellMode":true, "dependencies":["db"], "startupDelay":200})).unwrap();
        assert_eq!(service.args, vec!["run", "dev"]);
        assert_eq!(service.env.get("PORT").map(String::as_str), Some("3001"));
        assert_eq!(service.env_file.as_deref(), Some(".env.local"));
        assert!(service.shell_mode);
        assert_eq!(service.dependencies, vec!["db"]);
        assert_eq!(service.startup_delay, 200);
    }

    #[test]
    fn invalid_service_patch_does_not_mutate_service() {
        for patch in [
            json!({"name":"changed", "port":65536}),
            json!({"name":"changed", "port":0}),
            json!({"name":"changed", "cwd":""}),
            json!({"name":"changed", "args":"invalid"}),
        ] {
            let mut service = service("a", "one");
            let before = serde_json::to_value(&service).unwrap();
            assert!(patch_service(&mut service, &patch).is_err());
            assert_eq!(serde_json::to_value(&service).unwrap(), before);
        }
    }
}
