use serde::Serialize;
use std::process::Command;

/// 运行时检测条目：name/available/version/path（第二版前端消费这四个字段）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeItem {
    pub name: String,
    pub available: bool,
    pub version: Option<String>,
    pub path: Option<String>,
}

/// 检测规格：与第二版一致——where.exe 定位 + 版本命令验证。
const RUNTIMES: &[(&str, &str, &[&str])] = &[
    // (名称, where 定位名, (程序, 版本参数))
    ("node", "node", &["node", "--version"]),
    ("npm", "npm", &["npm", "--version"]),
    ("pnpm", "pnpm", &["pnpm", "--version"]),
    ("yarn", "yarn", &["yarn", "--version"]),
    ("bun", "bun", &["bun", "--version"]),
    ("java", "java", &["java", "-version"]),
    ("mvn", "mvn", &["mvn", "--version"]),
    ("gradle", "gradle", &["gradle", "--version"]),
    ("git", "git", &["git", "--version"]),
    ("python", "python", &["python", "--version"]),
    ("go", "go", &["go", "version"]),
    ("rust", "rustc", &["rustc", "--version"]),
    ("docker", "docker", &["docker", "--version"]),
];

fn where_lookup(tool: &str) -> Option<String> {
    let out = Command::new("where.exe").arg(tool).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let lines: Vec<_> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect();
    lines
        .iter()
        .find(|s| s.to_lowercase().ends_with(".exe"))
        .or_else(|| {
            lines
                .iter()
                .find(|s| s.to_lowercase().ends_with(".cmd") || s.to_lowercase().ends_with(".bat"))
        })
        .or_else(|| lines.first())
        .cloned()
}

fn probe_version(spec: &[&str]) -> Option<String> {
    let (program, args) = spec.split_first()?;
    let cwd = std::env::current_dir().ok()?;
    let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    let env = Default::default();
    let launch = crate::runtime::StartSpec {
        service_id: "environment",
        cwd: cwd.to_str()?,
        command: program,
        args: &args,
        env: &env,
        shell_mode: false,
        cap: 0,
        startup_delay: 0,
        prepare: &[],
    };
    let mut command =
        crate::runtime::build_spawn_command(&launch, &crate::runtime::resolve_program(&launch));
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    let out = command.output().ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    let text = if text.trim().is_empty() {
        // java -version 输出在 stderr
        String::from_utf8_lossy(&out.stderr).to_string()
    } else {
        text
    };
    let ansi = regex::Regex::new(r"\x1b\[[0-?]*[ -/]*[@-~]").ok()?;
    let clean = ansi.replace_all(&text, "");
    let line = clean
        .lines()
        .map(str::trim)
        .find(|line| {
            !line.is_empty()
                && !line.starts_with("Picked up ")
                && !line.starts_with("NOTE: Picked up ")
        })?
        .to_string();
    if line.is_empty() {
        None
    } else {
        Some(line)
    }
}

fn detect(name: &str) -> RuntimeItem {
    let Some(spec) = RUNTIMES.iter().find(|(n, ..)| *n == name) else {
        return RuntimeItem {
            name: name.to_string(),
            available: false,
            version: None,
            path: None,
        };
    };
    let path = where_lookup(spec.1);
    let available = path.is_some();
    let version = if available {
        probe_version(spec.2)
    } else {
        None
    };
    RuntimeItem {
        name: name.to_string(),
        available: available && version.is_some(),
        version,
        path,
    }
}

#[tauri::command]
pub fn environment_detect_all() -> Result<Vec<RuntimeItem>, String> {
    Ok(RUNTIMES.iter().map(|(name, ..)| detect(name)).collect())
}

#[tauri::command]
pub fn environment_detect_single(name: String) -> Result<RuntimeItem, String> {
    Ok(detect(&name))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "requires installed local node, npm, java and mvn"]
    fn local_environment_tools() {
        for name in ["node", "npm", "java", "mvn"] {
            let item = detect(name);
            println!("{item:?}");
            assert!(item.available, "{name}");
        }
    }
    #[cfg(windows)]
    #[test]
    fn version_probe_executes_batch_tools_and_rejects_failure_output() {
        let temp = tempfile::tempdir().unwrap();
        let tool = temp.path().join("version tool.cmd");
        std::fs::write(
            &tool,
            "@echo off\r\necho Picked up JAVA_TOOL_OPTIONS: example\r\necho 1.2.3\r\n",
        )
        .unwrap();
        assert_eq!(
            probe_version(&[tool.to_str().unwrap(), "--version"]),
            Some("1.2.3".into())
        );
        std::fs::write(&tool, "@echo off\r\necho invalid command\r\nexit /b 1\r\n").unwrap();
        assert_eq!(probe_version(&[tool.to_str().unwrap(), "--version"]), None);
    }
}
