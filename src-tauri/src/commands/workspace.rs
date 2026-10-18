use crate::store::{new_id, now_iso, AppState, Workspace};
use serde::Deserialize;
use serde::Serialize;
use std::path::{Path, PathBuf};
use tauri::State;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceInput {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub root_path: String,
    #[serde(default)]
    pub color: Option<String>,
    #[serde(default)]
    pub favorite: bool,
    #[serde(default)]
    pub start_mode: String,
}

#[tauri::command]
pub fn create_workspace(
    state: State<'_, AppState>,
    input: WorkspaceInput,
) -> Result<Workspace, String> {
    if input.name.trim().is_empty() {
        return Err("Workspace name cannot be empty".into());
    }
    let workspace = Workspace {
        id: new_id(),
        name: input.name.trim().to_string(),
        description: input.description,
        root_path: input.root_path,
        color: input.color,
        icon: None,
        favorite: input.favorite,
        start_mode: if input.start_mode.is_empty() {
            "parallel".into()
        } else {
            input.start_mode
        },
        created_at: now_iso(),
        updated_at: now_iso(),
    };
    let mut data = state
        .data
        .lock()
        .map_err(|_| "Failed to open ConfigStore".to_string())?;
    data.workspaces.push(workspace.clone());
    drop(data);
    state.commit()?;
    Ok(workspace)
}

#[tauri::command]
pub fn update_workspace(
    state: State<'_, AppState>,
    input: serde_json::Value,
) -> Result<Workspace, String> {
    let id = input
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or("Missing workspace id")?
        .to_string();
    let mut data = state
        .data
        .lock()
        .map_err(|_| "Failed to open ConfigStore".to_string())?;
    let ws = data
        .workspaces
        .iter_mut()
        .find(|w| w.id == id)
        .ok_or_else(|| format!("Workspace not found: {id}"))?;
    if let Some(v) = input.get("name").and_then(|v| v.as_str()) {
        if v.trim().is_empty() {
            return Err("Workspace name cannot be empty".into());
        }
        ws.name = v.trim().to_string();
    }
    if let Some(v) = input.get("description").and_then(|v| v.as_str()) {
        ws.description = v.to_string();
    }
    if let Some(v) = input.get("rootPath").and_then(|v| v.as_str()) {
        ws.root_path = v.to_string();
    }
    if let Some(v) = input.get("color") {
        ws.color = v.as_str().map(String::from);
    }
    if let Some(v) = input.get("favorite").and_then(|v| v.as_bool()) {
        ws.favorite = v;
    }
    if let Some(v) = input.get("startMode").and_then(|v| v.as_str()) {
        ws.start_mode = v.to_string();
    }
    ws.updated_at = now_iso();
    let updated = ws.clone();
    drop(data);
    state.commit()?;
    Ok(updated)
}

/// 删除工作区并级联删除其下服务。
#[tauri::command]
pub fn delete_workspace(state: State<'_, AppState>, id: String) -> Result<(), String> {
    let mut data = state
        .data
        .lock()
        .map_err(|_| "Failed to open ConfigStore".to_string())?;
    let before = data.workspaces.len();
    data.workspaces.retain(|w| w.id != id);
    if data.workspaces.len() == before {
        return Err(format!("Workspace not found: {id}"));
    }
    data.services.retain(|s| s.workspace_id != id);
    drop(data);
    state.commit()
}

#[tauri::command]
pub fn list_workspaces(state: State<'_, AppState>) -> Result<Vec<Workspace>, String> {
    let data = state
        .data
        .lock()
        .map_err(|_| "Failed to open ConfigStore".to_string())?;
    Ok(data.workspaces.clone())
}

// ---------- 项目发现 ----------

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Evidence {
    detail: String,
    source: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveredProject {
    id: String,
    name: String,
    path: String,
    relative_path: String,
    /// frontend | node | java | generic
    project_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    suggested_service_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    suggested_role: Option<String>,
    suggested_selected: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    framework: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    package_manager: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    command: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    args: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    detected_port: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    is_library: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    has_startup_class: Option<bool>,
    confidence: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    evidence: Option<Vec<Evidence>>,
}

#[derive(Serialize)]
pub struct Warning {
    message: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveryResult {
    projects: Vec<DiscoveredProject>,
    warnings: Vec<Warning>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscoverOptions {
    #[serde(default = "default_depth")]
    max_depth: u32,
    #[serde(default)]
    include_library: bool,
}

fn default_depth() -> u32 {
    3
}

use crate::commands::port::IGNORE_DIRS;

#[tauri::command]
pub fn workspace_discover(
    root_path: String,
    _workspace_id: Option<String>,
    options: Option<DiscoverOptions>,
) -> Result<DiscoveryResult, String> {
    let root = PathBuf::from(&root_path);
    if !root.is_dir() {
        return Err(format!("路径不存在或不是目录: {root_path}"));
    }
    let max_depth = options.as_ref().map(|o| o.max_depth).unwrap_or(3);
    let include_library = options.as_ref().map(|o| o.include_library).unwrap_or(false);

    let mut projects = Vec::new();
    let mut warnings = Vec::new();
    walk(&root, &root, max_depth, include_library, &mut projects);

    if projects.is_empty() {
        warnings.push(Warning {
            message: "未发现可识别的项目".into(),
        });
    }
    Ok(DiscoveryResult { projects, warnings })
}

fn walk(
    dir: &Path,
    root: &Path,
    depth: u32,
    include_library: bool,
    out: &mut Vec<DiscoveredProject>,
) {
    if depth == 0 {
        return;
    }
    if std::fs::symlink_metadata(dir).is_ok_and(|m| m.file_type().is_symlink()) {
        return;
    }
    if dir.join("pom.xml").is_file() {
        let modules = crate::launch::maven_projects(dir);
        for module in modules {
            out.push(discover_java(&module, root, "maven", include_library));
        }
        return;
    }
    if dir.join("settings.gradle").is_file() || dir.join("settings.gradle.kts").is_file() {
        for module in crate::launch::gradle_projects(dir) {
            out.push(discover_java(&module, root, "gradle", include_library));
        }
        return;
    }
    let pkg = crate::launch::read_package(dir);
    let workspace_container =
        pkg.get("workspaces").is_some() || dir.join("pnpm-workspace.yaml").is_file();
    if let Some(project) = detect_project(dir, root, include_library) {
        if !workspace_container || project.command.is_some() {
            out.push(project);
        }
        if !workspace_container {
            return;
        }
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            let name = entry.file_name().to_string_lossy().to_string();
            if IGNORE_DIRS.contains(&name.as_str()) || name.starts_with('.') {
                continue;
            }
            walk(&path, root, depth - 1, include_library, out);
        }
    }
}

/// 对单个目录做技术栈识别：Node 系（package.json）、Java 系（pom.xml / build.gradle）、
/// 以及有工程痕迹但未识别的通用候选（丢给用户自己填启动方式）。
fn detect_project(dir: &Path, root: &Path, include_library: bool) -> Option<DiscoveredProject> {
    let package_json = dir.join("package.json");
    let pom = dir.join("pom.xml");
    let gradle = dir.join("build.gradle");

    if package_json.is_file() {
        return Some(discover_node(dir, root, include_library));
    }
    if pom.is_file() {
        return Some(discover_java(dir, root, "maven", include_library));
    }
    if gradle.is_file() || dir.join("build.gradle.kts").is_file() {
        return Some(discover_java(dir, root, "gradle", include_library));
    }
    detect_generic(dir, root)
}

/// 通用候选：非 Node/Java 技术栈（Rust/Python/Go/PHP 等），
/// 按技术栈给出默认启动命令，无法确定的留空由用户手填。
type StackCandidate<'a> = (&'a str, &'a str, Option<String>, Option<Vec<String>>);

fn detect_generic(dir: &Path, root: &Path) -> Option<DiscoveredProject> {
    let has = |f: &str| dir.join(f).is_file();
    let read = |f: &str| std::fs::read_to_string(dir.join(f)).unwrap_or_default();

    // (标记文件, 技术栈类型, 命令, 参数)
    let go_main = has("go.mod")
        && std::fs::read_dir(dir).ok().is_some_and(|entries| {
            entries.flatten().any(|e| {
                e.path().extension().is_some_and(|x| x == "go")
                    && std::fs::read_to_string(e.path())
                        .is_ok_and(|s| s.lines().any(|line| line.trim() == "package main"))
            })
        });
    let detected: Option<StackCandidate<'_>> = if has("Cargo.toml") {
        Some((
            "Cargo.toml",
            "rust",
            has("src/main.rs").then(|| "cargo".into()),
            has("src/main.rs").then(|| vec!["run".into()]),
        ))
    } else if has("go.mod") {
        Some((
            "go.mod",
            "go",
            go_main.then(|| "go".into()),
            go_main.then(|| vec!["run".into(), ".".into()]),
        ))
    } else if has("manage.py") {
        Some((
            "manage.py",
            "python",
            Some("python".into()),
            Some(vec!["manage.py".into(), "runserver".into()]),
        ))
    } else if has("pyproject.toml") || has("requirements.txt") {
        // Python Web 项目：uvicorn / flask / main.py / app.py 常见入口
        let pyproject = format!("{}\n{}", read("pyproject.toml"), read("requirements.txt"));
        let app = ["main.py", "app.py", "server.py"]
            .iter()
            .find(|f| has(f))
            .map(|f| f.to_string());
        if let Some(file) = app
            .as_ref()
            .filter(|_| pyproject.contains("uvicorn") || pyproject.contains("fastapi"))
        {
            let entry = format!("{}:app", file.trim_end_matches(".py"));
            Some((
                "pyproject.toml",
                "python",
                Some("python".into()),
                Some(vec!["-m".into(), "uvicorn".into(), entry]),
            ))
        } else if let Some(entry) = app {
            Some((
                "requirements.txt",
                "python",
                Some("python".into()),
                Some(vec![entry]),
            ))
        } else {
            Some(("pyproject.toml", "python", None, None))
        }
    } else if has("composer.json") {
        let artisan = dir.join("artisan").is_file();
        Some((
            "composer.json",
            "php",
            artisan.then(|| "php".into()),
            artisan.then(|| vec!["artisan".into(), "serve".into()]),
        ))
    } else if has("index.php") {
        Some(("index.php", "php", None, None))
    } else {
        None
    };

    let (marker, stack, command, args) = detected?;
    let mut evidence = vec![Evidence {
        detail: format!("检测到 {stack} 工程标记 {marker}"),
        source: marker.into(),
    }];
    if command.is_some() {
        evidence.push(Evidence {
            detail: "已按技术栈生成默认启动命令，可修改".into(),
            source: marker.into(),
        });
    }
    let has_command = command.is_some();
    let name = dir
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    Some(DiscoveredProject {
        id: relative_id(dir, root),
        name,
        path: dir.to_string_lossy().to_string(),
        relative_path: relative_id(dir, root),
        project_type: stack.into(),
        suggested_service_type: Some(stack.into()),
        suggested_role: Some("backend".into()),
        suggested_selected: has_command,
        framework: None,
        package_manager: None,
        command,
        args,
        detected_port: None,
        is_library: None,
        has_startup_class: None,
        confidence: if has_command { "medium" } else { "low" }.into(),
        evidence: Some(evidence),
    })
}

fn relative_id(dir: &Path, root: &Path) -> String {
    dir.strip_prefix(root)
        .map(|p| p.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|_| ".".to_string())
}

fn discover_node(dir: &Path, root: &Path, include_library: bool) -> DiscoveredProject {
    let mut evidence = Vec::new();
    let pkg_path = dir.join("package.json");
    evidence.push(Evidence {
        detail: "找到 package.json".into(),
        source: "package.json".into(),
    });
    let pkg: serde_json::Value = std::fs::read_to_string(&pkg_path)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or(serde_json::Value::Null);
    let name = pkg
        .get("name")
        .and_then(|v| v.as_str())
        .map(String::from)
        .unwrap_or_else(|| {
            dir.file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default()
        });

    let package_manager = Some(crate::launch::package_manager(dir));
    let options = crate::launch::node_options(dir);
    let launch = options.first();
    let dev_script = launch.map(|o| o.label.clone());
    let all_text = serde_json::to_string(&pkg).unwrap_or_default();
    let framework = [
        ("react-scripts", "react-scripts"),
        ("vue-cli-service", "vue-cli"),
        ("vite", "vite"),
        ("next", "next"),
        ("nuxt", "nuxt"),
    ]
    .iter()
    .find(|(sig, _)| {
        all_text.contains(sig) || dir.join("vite.config.ts").is_file() && *sig == "vite"
    })
    .map(|(_, fw)| fw.to_string());

    // uni-app 小程序：manifest.json + pages.json 是 HBuilderX 工程签名；
    // 依赖里出现 @dcloudio/* 或 uview 同样判定。小程序属前端。
    let is_uniapp = (dir.join("manifest.json").is_file() && dir.join("pages.json").is_file())
        || all_text.contains("@dcloudio")
        || all_text.contains("uview");
    if is_uniapp {
        evidence.push(Evidence {
            detail: "检测到 uni-app 小程序工程（manifest.json / pages.json）".into(),
            source: "manifest.json".into(),
        });
    }

    // 端口：vite/webpack 配置或 .env
    let detected_port = crate::commands::port::detect_port_in_dir(dir);

    let has_frontend_dep = ["vue", "react", "@dcloudio/uni-app", "uview-plus"]
        .iter()
        .any(|dep| pkg.get("dependencies").and_then(|d| d.get(*dep)).is_some());
    let is_library = dev_script.is_none() && !is_uniapp && pkg.get("main").is_some();
    let project_type =
        if is_uniapp || framework.is_some() || dev_script.is_some() || has_frontend_dep {
            "frontend"
        } else {
            "node"
        };
    if let Some(fw) = &framework {
        evidence.push(Evidence {
            detail: format!("检测到框架特征 {fw}"),
            source: "package.json".into(),
        });
    }
    if let Some(script) = &dev_script {
        evidence.push(Evidence {
            detail: format!("启动脚本可用：{script}"),
            source: "package.json".into(),
        });
    }
    let command = launch.map(|o| o.launch.command.clone());
    let args = launch.map(|o| o.launch.args.clone());

    let confidence = if framework.is_some() && command.is_some() {
        "high"
    } else if command.is_some() || is_uniapp {
        "medium"
    } else {
        "low"
    };

    DiscoveredProject {
        id: relative_id(dir, root),
        name,
        path: dir.to_string_lossy().to_string(),
        relative_path: relative_id(dir, root),
        project_type: project_type.into(),
        suggested_service_type: Some(project_type.into()),
        suggested_role: Some(if project_type == "frontend" {
            "frontend".into()
        } else {
            "backend".into()
        }),
        suggested_selected: command.is_some()
            && (confidence == "high" || (include_library && !is_library)),
        framework,
        package_manager,
        command,
        args,
        detected_port,
        is_library: Some(is_library),
        has_startup_class: None,
        confidence: confidence.into(),
        evidence: Some(evidence),
    }
}

fn discover_java(
    dir: &Path,
    root: &Path,
    _build_tool: &str,
    include_library: bool,
) -> DiscoveredProject {
    let has_startup_class = crate::launch::has_boot_entry(dir);
    let options = crate::launch::java_options(dir);
    let launch = options.first();
    let command = launch.map(|p| p.launch.command.clone());
    let args = launch.map(|p| p.launch.args.clone());
    let mut evidence = vec![Evidence {
        detail: if has_startup_class {
            "发现 Spring Boot 启动入口"
        } else {
            "未发现 Spring Boot 启动入口，请手动配置命令"
        }
        .into(),
        source: "src/main".into(),
    }];
    if let Some(plan) = launch {
        evidence.push(Evidence {
            detail: format!("{}：{}", plan.label, plan.description),
            source: if dir.join("pom.xml").is_file() {
                "pom.xml"
            } else {
                "build.gradle(.kts)"
            }
            .into(),
        });
    }
    let name = dir
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let selected = command.is_some() && (has_startup_class || include_library);
    DiscoveredProject {
        id: relative_id(dir, root),
        name,
        path: dir.to_string_lossy().to_string(),
        relative_path: relative_id(dir, root),
        project_type: "java".into(),
        suggested_service_type: Some("java".into()),
        suggested_role: Some("backend".into()),
        suggested_selected: selected,
        framework: has_startup_class.then(|| "spring-boot".into()),
        package_manager: None,
        command,
        args,
        detected_port: crate::commands::port::detect_port_in_dir(dir),
        is_library: Some(!has_startup_class),
        has_startup_class: Some(has_startup_class),
        confidence: if selected { "high" } else { "low" }.into(),
        evidence: Some(evidence),
    }
}

/// 供前端「应用发现结果」前快速探测端口。
#[tauri::command]
pub fn detect_project_port(path: String) -> Result<Option<u16>, String> {
    let dir = Path::new(&path);
    if !dir.is_dir() {
        return Ok(None);
    }
    Ok(crate::commands::port::detect_port_in_dir(dir))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn write(root: &Path, path: &str, text: &str) {
        let file = root.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, text).unwrap();
    }
    #[test]
    #[ignore = "requires XCHEN_TEST_WORKSPACE; only reads project files"]
    fn local_workspace_scan() {
        let root = std::env::var("XCHEN_TEST_WORKSPACE").expect("XCHEN_TEST_WORKSPACE");
        let scan = workspace_discover(
            root.clone(),
            None,
            Some(DiscoverOptions {
                max_depth: 6,
                include_library: false,
            }),
        )
        .unwrap();
        let backend = scan
            .projects
            .iter()
            .find(|p| p.name == "xingyu-starter")
            .expect("boot module");
        assert!(backend.suggested_selected);
        assert_eq!(backend.detected_port, Some(7779));
        let plans = crate::launch::get_launch_options(backend.path.clone()).unwrap();
        assert!(!plans[0].prepare.is_empty());
        assert!(plans[0].prepare[0].cwd.ends_with("xingyu-server"));
        assert_eq!(plans[0].launch.args, vec!["spring-boot:run"]);
        for name in ["xingyu-web-next", "xingyu-admin"] {
            let frontend = scan
                .projects
                .iter()
                .find(|p| p.name == name)
                .expect("frontend");
            assert_eq!(frontend.command.as_deref(), Some("npm"));
            assert_eq!(frontend.args, Some(vec!["run".into(), "dev".into()]));
        }
        let uni = scan
            .projects
            .iter()
            .find(|p| p.name == "xingyu-uniapp")
            .unwrap();
        assert!(uni.command.is_none());
        assert!(!uni.suggested_selected);
        let preview = serde_json::json!({ "scan": scan, "backendOptions": plans });
        std::fs::write(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target/workspace-smoke.json"),
            serde_json::to_string_pretty(&preview).unwrap(),
        )
        .unwrap();
        println!("Live read-only discovery verified: xingyu-starter, web, admin, uniapp; startup plans saved in target/workspace-smoke.json");
    }

    #[test]
    fn maven_aggregator_discovers_each_real_boot_module() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        write(root, "pom.xml", "<project><packaging>pom</packaging><modules><module>lib</module><module>apps/api</module><module>worker</module></modules></project>");
        write(root, "lib/pom.xml", "<project><build><pluginManagement><plugins><plugin><artifactId>spring-boot-maven-plugin</artifactId></plugin></plugins></pluginManagement></build></project>");
        for module in ["apps/api", "worker"] {
            write(root, &format!("{module}/pom.xml"), "<project><build><plugins><plugin><artifactId>spring-boot-maven-plugin</artifactId></plugin></plugins></build></project>");
            write(root, &format!("{module}/src/main/java/App.java"), "class App { public static void main(String[] a) { SpringApplication.run(App.class,a); } }");
        }
        let mut found = Vec::new();
        walk(root, root, 3, false, &mut found);
        assert_eq!(found.len(), 2);
        assert!(found
            .iter()
            .all(|p| p.has_startup_class == Some(true) && p.suggested_selected));
        assert!(found.iter().all(|p| !p.path.ends_with("lib")));
    }
    #[test]
    fn node_honors_pinned_manager_and_keeps_script_key() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        write(
            root,
            "package.json",
            r#"{"packageManager":"pnpm@10.10.0","scripts":{"dev":"vite","aaa":"vite"},"devDependencies":{"vite":"1"}}"#,
        );
        write(root, "package-lock.json", "{}");
        let project = discover_node(root, root, false);
        assert_eq!(project.package_manager.as_deref(), Some("pnpm"));
        assert_eq!(project.args, Some(vec!["run".into(), "dev".into()]));
    }
    #[test]
    fn non_spring_java_does_not_invent_boot_command() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        write(
            root,
            "pom.xml",
            "<project><artifactId>library</artifactId></project>",
        );
        let project = discover_java(root, root, "maven", false);
        assert!(project.command.is_none());
        assert!(!project.suggested_selected);
    }
    #[test]
    fn generic_libraries_do_not_get_invented_launch_commands() {
        for marker in ["Cargo.toml", "go.mod", "requirements.txt"] {
            let temp = tempfile::tempdir().unwrap();
            std::fs::write(temp.path().join(marker), "fastapi").unwrap();
            let project = detect_generic(temp.path(), temp.path()).unwrap();
            assert!(project.command.is_none(), "{marker}");
            assert!(!project.suggested_selected);
        }
    }

    #[test]
    fn python_uvicorn_has_module_and_application_target() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        write(
            root,
            "pyproject.toml",
            "[project]\ndependencies=['fastapi','uvicorn']",
        );
        write(
            root,
            "main.py",
            "from fastapi import FastAPI\napp = FastAPI()",
        );
        let project = detect_generic(root, root).unwrap();
        assert_eq!(
            project.args,
            Some(vec!["-m".into(), "uvicorn".into(), "main:app".into()])
        );
    }
}
