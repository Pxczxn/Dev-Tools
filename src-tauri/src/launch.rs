//! 从工程文件生成可复核的启动方案，不执行扫描到的代码。
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LaunchCommand {
    pub cwd: String,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub shell_mode: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LaunchOption {
    pub id: String,
    pub label: String,
    pub description: String,
    #[serde(flatten)]
    pub launch: LaunchCommand,
    pub prepare: Vec<LaunchCommand>,
}

#[derive(Default)]
pub struct Pom {
    pub modules: Vec<String>,
    pub boot_plugin: bool,
    pub properties: BTreeMap<String, String>,
}

pub fn read_pom(dir: &Path) -> Pom {
    use quick_xml::{events::Event, Reader};
    let Ok(text) = std::fs::read_to_string(dir.join("pom.xml")) else {
        return Pom::default();
    };
    let mut reader = Reader::from_str(&text);
    let mut stack = Vec::new();
    let mut pom = Pom::default();
    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => stack.push(e.local_name().as_ref().to_string()),
            Ok(Event::End(_)) => {
                stack.pop();
            }
            Ok(Event::Text(e)) => {
                let value = e.as_ref().trim().to_string();
                match stack.join("/").as_str() {
                    "project/modules/module" if !value.is_empty() => pom.modules.push(value),
                    "project/build/plugins/plugin/artifactId"
                        if value == "spring-boot-maven-plugin" =>
                    {
                        pom.boot_plugin = true
                    }
                    _ if stack.len() == 3 && stack[1] == "properties" => {
                        pom.properties.insert(stack[2].clone(), value);
                    }
                    _ => {}
                }
            }
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
    }
    pom
}

pub fn has_boot_entry(dir: &Path) -> bool {
    fn walk(dir: &Path, depth: usize) -> bool {
        if depth == 0 {
            return false;
        }
        let Ok(entries) = std::fs::read_dir(dir) else {
            return false;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if entry.file_type().is_ok_and(|t| t.is_symlink()) {
                continue;
            }
            if path.is_dir() && walk(&path, depth - 1) {
                return true;
            }
            if path.extension().is_some_and(|e| e == "java" || e == "kt") {
                if let Ok(text) = std::fs::read_to_string(path) {
                    if text.contains("SpringApplication.run") || text.contains("runApplication<") {
                        return true;
                    }
                }
            }
        }
        false
    }
    walk(&dir.join("src/main/java"), 16) || walk(&dir.join("src/main/kotlin"), 16)
}

/// 只走 POM 声明的模块，避免把废弃目录、pluginManagement 或 target 当启动模块。
pub fn maven_projects(root: &Path) -> Vec<PathBuf> {
    fn collect(
        dir: &Path,
        root: &Path,
        depth: usize,
        visited: &mut HashSet<PathBuf>,
        out: &mut Vec<PathBuf>,
    ) {
        let Ok(canonical) = dir.canonicalize() else {
            return;
        };
        if depth == 0
            || !canonical.starts_with(root.canonicalize().unwrap_or_else(|_| root.to_path_buf()))
            || !visited.insert(canonical)
        {
            return;
        }
        let pom = read_pom(dir);
        if has_boot_entry(dir) || (pom.modules.is_empty() && dir == root) {
            out.push(dir.to_path_buf());
        }
        for module in pom.modules {
            collect(&dir.join(module), root, depth - 1, visited, out);
        }
    }
    let root = root.to_path_buf();
    let mut out = Vec::new();
    collect(&root, &root, 12, &mut HashSet::new(), &mut out);
    out
}

fn maven_root(dir: &Path) -> PathBuf {
    let mut root = dir.to_path_buf();
    for ancestor in dir.ancestors().skip(1).take(8) {
        if ancestor.join("pom.xml").is_file() && maven_projects(ancestor).iter().any(|p| p == dir) {
            root = ancestor.to_path_buf();
        }
    }
    root
}

pub fn package_manager(dir: &Path) -> String {
    let mut roots = vec![dir.to_path_buf()];
    for ancestor in dir.ancestors().skip(1).take(6) {
        let pkg = read_package(ancestor);
        if pkg.get("workspaces").is_some() || ancestor.join("pnpm-workspace.yaml").is_file() {
            roots.push(ancestor.to_path_buf());
        } else if ancestor.join("package.json").is_file() {
            break;
        }
        if ancestor.join(".git").exists() {
            break;
        }
    }
    for root in &roots {
        let pkg = read_package(root);
        if let Some(pm) = pkg
            .get("packageManager")
            .and_then(|v| v.as_str())
            .and_then(|s| s.split('@').next())
        {
            if ["npm", "pnpm", "yarn", "bun"].contains(&pm) {
                return pm.into();
            }
        }
    }
    for root in roots {
        for (file, pm) in [
            ("pnpm-lock.yaml", "pnpm"),
            ("yarn.lock", "yarn"),
            ("bun.lock", "bun"),
            ("bun.lockb", "bun"),
            ("package-lock.json", "npm"),
        ] {
            if root.join(file).is_file() {
                return pm.into();
            }
        }
    }
    "npm".into()
}

pub fn read_package(dir: &Path) -> serde_json::Value {
    std::fs::read_to_string(dir.join("package.json"))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

pub fn node_options(dir: &Path) -> Vec<LaunchOption> {
    let pkg = read_package(dir);
    let Some(scripts) = pkg.get("scripts").and_then(|s| s.as_object()) else {
        return vec![];
    };
    let mut keys: Vec<_> = scripts
        .iter()
        .filter(|(k, v)| {
            v.as_str().is_some_and(|s| !s.trim().is_empty())
                && (["dev", "start", "serve", "preview"].contains(&k.as_str())
                    || k.starts_with("dev:")
                    || k.starts_with("start:")
                    || k.starts_with("serve:"))
        })
        .map(|(k, _)| k.clone())
        .collect();
    keys.sort_by_key(|k| {
        (
            match k.as_str() {
                "dev" => 0,
                "start" => 1,
                "serve" => 2,
                "dev:h5" => 3,
                "dev:web" => 4,
                "dev:mp-weixin" => 5,
                "preview" => 9,
                _ => 6,
            },
            k.clone(),
        )
    });
    let pm = package_manager(dir);
    keys.into_iter()
        .map(|key| LaunchOption {
            id: format!("node:{key}"),
            label: format!("{pm} run {key}"),
            description: format!(
                "package.json scripts.{key}: {}",
                scripts[&key].as_str().unwrap_or_default()
            ),
            launch: LaunchCommand {
                cwd: dir.to_string_lossy().into(),
                command: pm.clone(),
                args: vec!["run".into(), key],
                shell_mode: false,
            },
            prepare: vec![],
        })
        .collect()
}

pub fn java_options(dir: &Path) -> Vec<LaunchOption> {
    if !has_boot_entry(dir) {
        return vec![];
    }
    let cwd = dir.to_string_lossy().into_owned();
    if dir.join("pom.xml").is_file() {
        let root = maven_root(dir);
        let plugin_declared = dir
            .ancestors()
            .take_while(|p| p.starts_with(&root))
            .any(|p| read_pom(p).boot_plugin);
        if !plugin_declared {
            return vec![];
        }
        let command = if root.join("mvnw.cmd").is_file() {
            root.join("mvnw.cmd").to_string_lossy().into_owned()
        } else {
            "mvn".into()
        };
        let main = LaunchCommand {
            cwd: cwd.clone(),
            command: command.clone(),
            args: vec!["spring-boot:run".into()],
            shell_mode: false,
        };
        let mut options = Vec::new();
        if root != dir {
            let module = dir
                .strip_prefix(&root)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            options.push(LaunchOption {
                id: "maven:build-run".into(), label: "构建依赖后启动（多模块推荐）".into(),
                description: "先在聚合根安装所需模块，再单独运行启动模块；任一步失败即停止。首次运行或依赖源码改动后使用。".into(),
                launch: main.clone(), prepare: vec![LaunchCommand { cwd: root.to_string_lossy().into(), command: command.clone(), args: vec!["-B".into(), "-pl".into(), module, "-am".into(), "install".into(), "-DskipTests".into()], shell_mode: false }],
            });
        }
        options.push(LaunchOption {
            id: "maven:run".into(),
            label: "直接启动 Spring Boot".into(),
            description: "依赖模块已安装到本地 Maven 仓库时使用，保留项目自身默认 profile。".into(),
            launch: main.clone(),
            prepare: vec![],
        });
        for profile in ["local", "dev", "test"] {
            if ["yml", "yaml", "properties"].iter().any(|ext| {
                dir.join(format!("src/main/resources/application-{profile}.{ext}"))
                    .is_file()
            }) {
                let mut launch = main.clone();
                launch
                    .args
                    .push(format!("-Dspring-boot.run.profiles={profile}"));
                options.push(LaunchOption {
                    id: format!("maven:profile:{profile}"),
                    label: format!("Spring Boot · {profile}"),
                    description: format!(
                        "明确选择 application-{profile}，不会自动切换到生产配置。"
                    ),
                    launch,
                    prepare: options
                        .first()
                        .map(|o| o.prepare.clone())
                        .unwrap_or_default(),
                });
            }
        }
        if let Ok(entries) = std::fs::read_dir(dir.join("target")) {
            let mut jars: Vec<_> = entries
                .flatten()
                .map(|e| e.path())
                .filter(|p| {
                    p.extension().is_some_and(|e| e == "jar")
                        && !p
                            .file_name()
                            .unwrap()
                            .to_string_lossy()
                            .contains("-sources")
                        && !p
                            .file_name()
                            .unwrap()
                            .to_string_lossy()
                            .contains("-javadoc")
                })
                .collect();
            jars.sort();
            for jar in jars {
                options.push(LaunchOption {
                    id: format!("jar:{}", jar.display()),
                    label: format!(
                        "运行现有 JAR · {}",
                        jar.file_name().unwrap().to_string_lossy()
                    ),
                    description: "仅当这是已打包的可执行 JAR 时使用；不会重新编译源码。".into(),
                    launch: LaunchCommand {
                        cwd: cwd.clone(),
                        command: "java".into(),
                        args: vec!["-jar".into(), jar.to_string_lossy().into()],
                        shell_mode: false,
                    },
                    prepare: vec![],
                });
            }
        }
        options
    } else {
        let Some(root) = dir
            .ancestors()
            .find(|p| {
                p.join("settings.gradle").is_file() || p.join("settings.gradle.kts").is_file()
            })
            .or(Some(dir))
        else {
            return vec![];
        };
        let build = std::fs::read_to_string(dir.join("build.gradle"))
            .or_else(|_| std::fs::read_to_string(dir.join("build.gradle.kts")))
            .unwrap_or_default();
        if !build.contains("org.springframework.boot") && !build.contains("spring-boot") {
            return vec![];
        }
        let task = if root == dir {
            "bootRun".into()
        } else {
            format!(
                ":{}:bootRun",
                dir.strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .replace(['\\', '/'], ":")
            )
        };
        let command = if root.join("gradlew.bat").is_file() {
            root.join("gradlew.bat").to_string_lossy().into_owned()
        } else {
            "gradle".into()
        };
        vec![LaunchOption {
            id: "gradle:bootRun".into(),
            label: format!("Gradle {task}"),
            description: "在 Gradle 根目录执行目标模块的 bootRun，保留工程依赖关系。".into(),
            launch: LaunchCommand {
                cwd: root.to_string_lossy().into(),
                command,
                args: vec![task],
                shell_mode: false,
            },
            prepare: vec![],
        }]
    }
}

pub fn gradle_projects(root: &Path) -> Vec<PathBuf> {
    let settings = std::fs::read_to_string(root.join("settings.gradle"))
        .or_else(|_| std::fs::read_to_string(root.join("settings.gradle.kts")))
        .unwrap_or_default();
    let mut out = Vec::new();
    if has_boot_entry(root) {
        out.push(root.to_path_buf());
    }
    let include = regex::Regex::new(r#"(?m)^\s*include\s*(?:\(([^)]*)\)|([^\n]+))"#).unwrap();
    let quoted = regex::Regex::new(r#"["']([^"']+)["']"#).unwrap();
    for line in include.captures_iter(&settings) {
        for module in quoted.captures_iter(line.get(1).or_else(|| line.get(2)).unwrap().as_str()) {
            let path = root.join(module[1].trim_start_matches(':').replace(':', "/"));
            if has_boot_entry(&path) && !out.contains(&path) {
                out.push(path);
            }
        }
    }
    if out.is_empty() {
        out.push(root.to_path_buf());
    }
    out
}

#[tauri::command]
pub fn get_launch_options(path: String) -> Result<Vec<LaunchOption>, String> {
    let dir = PathBuf::from(path);
    if !dir.is_dir() {
        return Err("项目目录不存在".into());
    }
    if dir.join("package.json").is_file() {
        Ok(node_options(&dir))
    } else if dir.join("pom.xml").is_file() {
        Ok(maven_projects(&dir)
            .iter()
            .flat_map(|p| java_options(p))
            .collect())
    } else if dir.join("settings.gradle").is_file() || dir.join("settings.gradle.kts").is_file() {
        Ok(gradle_projects(&dir)
            .iter()
            .flat_map(|p| java_options(p))
            .collect())
    } else {
        Ok(java_options(&dir))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn write(root: &Path, name: &str, text: &str) {
        let file = root.join(name);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, text).unwrap();
    }
    #[test]
    fn maven_wrapper_profile_and_nested_module_have_separate_build_and_run_steps() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        write(
            root,
            "pom.xml",
            "<project><modules><module>services</module></modules></project>",
        );
        write(
            root,
            "services/pom.xml",
            "<project><modules><module>api</module></modules></project>",
        );
        write(root, "services/api/pom.xml", "<project><build><plugins><plugin><artifactId>spring-boot-maven-plugin</artifactId></plugin></plugins></build></project>");
        write(
            root,
            "services/api/src/main/java/App.java",
            "SpringApplication.run(App.class, args);",
        );
        write(
            root,
            "services/api/src/main/resources/application-local.yaml",
            "server: {}",
        );
        write(root, "mvnw.cmd", "@echo off");
        let dir = root.join("services/api");
        let plans = java_options(&dir);
        assert_eq!(plans[0].launch.cwd, dir.to_string_lossy());
        assert!(plans[0].launch.command.ends_with("mvnw.cmd"));
        assert_eq!(plans[0].prepare[0].cwd, root.to_string_lossy());
        assert_eq!(
            plans[0].prepare[0].args,
            ["-B", "-pl", "services/api", "-am", "install", "-DskipTests"]
        );
        assert!(!plans[0].launch.args.contains(&"-am".into()));
        assert!(plans
            .iter()
            .find(|p| p.id == "maven:run")
            .unwrap()
            .prepare
            .is_empty());
        assert!(plans
            .iter()
            .find(|p| p.id == "maven:profile:local")
            .unwrap()
            .launch
            .args
            .contains(&"-Dspring-boot.run.profiles=local".into()));
    }
    #[test]
    fn gradle_kotlin_multiline_modules_run_at_wrapper_root() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        write(
            root,
            "settings.gradle.kts",
            "include(\n  \":apps:api\",\n  \":shared\"\n)",
        );
        write(root, "gradlew.bat", "@echo off");
        write(
            root,
            "apps/api/build.gradle.kts",
            "plugins { id(\"org.springframework.boot\") }",
        );
        write(
            root,
            "apps/api/src/main/kotlin/App.kt",
            "fun main(args: Array<String>) = runApplication<App>(*args)",
        );
        write(root, "shared/build.gradle.kts", "plugins { java }");
        assert_eq!(gradle_projects(root), vec![root.join("apps/api")]);
        let plan = java_options(&root.join("apps/api")).remove(0);
        assert_eq!(plan.launch.cwd, root.to_string_lossy());
        assert!(plan.launch.command.ends_with("gradlew.bat"));
        assert_eq!(plan.launch.args, vec![":apps:api:bootRun"]);
    }
    #[test]
    fn node_workspace_inherits_pinned_manager_and_exposes_actual_script_variants() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        write(
            root,
            "package.json",
            r#"{"private":true,"workspaces":["apps/*"],"packageManager":"pnpm@10.10.0"}"#,
        );
        write(root, "apps/web/package-lock.json", "{}");
        write(
            root,
            "apps/web/package.json",
            r#"{"scripts":{"dev:mp-weixin":"uni -p mp-weixin","dev:h5":"uni","build":"uni build","preview":"vite preview"}}"#,
        );
        let plans = node_options(&root.join("apps/web"));
        assert_eq!(
            plans
                .iter()
                .map(|p| p.launch.args[1].as_str())
                .collect::<Vec<_>>(),
            vec!["dev:h5", "dev:mp-weixin", "preview"]
        );
        assert!(plans.iter().all(|p| p.launch.command == "pnpm"));
    }
}
