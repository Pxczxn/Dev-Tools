//! 用户自己指定的 MySQL / Redis 程序。只接受 mysqld.exe 与 redis-server.exe，
//! 缺失或非法路径在启动前拒绝，不会交给系统去执行。

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;
use sysinfo::{Pid, ProcessesToUpdate, System};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LocalDatabase {
    pub executable: String,
    pub port: u16,
    pub data_dir: String,
    pub config_file: String,
    pub password: String,
}

impl Default for LocalDatabase {
    fn default() -> Self {
        Self {
            executable: String::new(),
            port: 3306,
            data_dir: String::new(),
            config_file: String::new(),
            password: String::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LocalDatabases {
    pub mysql: LocalDatabase,
    pub redis: LocalDatabase,
}

impl Default for LocalDatabases {
    fn default() -> Self {
        Self {
            mysql: LocalDatabase {
                port: 3306,
                ..LocalDatabase::default()
            },
            redis: LocalDatabase {
                port: 6379,
                ..LocalDatabase::default()
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchPlan {
    pub executable: PathBuf,
    pub args: Vec<String>,
}

#[derive(Default)]
pub struct LocalInfraRuntime {
    mysql: Mutex<Option<u32>>,
    redis: Mutex<Option<u32>>,
}

impl LocalInfraRuntime {
    fn slot(&self, kind: &str) -> Result<&Mutex<Option<u32>>, String> {
        match kind {
            "mysql" => Ok(&self.mysql),
            "redis" => Ok(&self.redis),
            _ => Err("只能配置 MySQL 或 Redis".into()),
        }
    }

    pub fn remember(&self, kind: &str, pid: u32) -> Result<(), String> {
        *self.slot(kind)?.lock().map_err(|_| "无法记录进程".to_string())? = Some(pid);
        Ok(())
    }

    pub fn clear(&self, kind: &str) -> Result<(), String> {
        *self.slot(kind)?.lock().map_err(|_| "无法记录进程".to_string())? = None;
        Ok(())
    }

    /// 只承认本程序启动、且进程名仍然匹配的 PID。进程退出后会清掉记录。
    pub fn observed_pid(&self, kind: &str) -> Result<Option<u32>, String> {
        let expected = expected_exe(kind)?;
        let slot = self.slot(kind)?;
        let mut guard = slot.lock().map_err(|_| "无法记录进程".to_string())?;
        let Some(pid) = *guard else {
            return Ok(None);
        };
        if process_matches(pid, expected) {
            Ok(Some(pid))
        } else {
            *guard = None;
            Ok(None)
        }
    }
}

pub fn expected_exe(kind: &str) -> Result<&'static str, String> {
    match kind {
        "mysql" => Ok("mysqld.exe"),
        "redis" => Ok("redis-server.exe"),
        _ => Err("只能配置 MySQL 或 Redis".into()),
    }
}

pub fn normalize(kind: &str, config: &LocalDatabase) -> Result<LocalDatabase, String> {
    let expected = expected_exe(kind)?;
    if config.port == 0 {
        return Err("端口需要在 1 到 65535 之间".into());
    }
    let executable = clean_path(&config.executable)?;
    let data_dir = clean_path(&config.data_dir)?;
    let config_file = clean_path(&config.config_file)?;
    if !executable.is_empty() {
        let name = Path::new(&executable)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("");
        if !name.eq_ignore_ascii_case(expected) {
            return Err(format!("程序必须是 {expected}"));
        }
    }
    let password = config.password.trim().to_string();
    if password.chars().any(|c| c.is_control()) {
        return Err("密码不能包含控制字符".into());
    }
    if kind == "mysql" && !password.is_empty() {
        return Err("这里不修改 MySQL 密码".into());
    }
    Ok(LocalDatabase {
        executable,
        port: config.port,
        data_dir,
        config_file,
        password,
    })
}

/// 官方安装器的默认目录。版本号高的优先。
pub fn default_mysql_paths() -> Vec<PathBuf> {
    let mut found = Vec::new();
    for root in [r"C:\Program Files\MySQL", r"C:\Program Files (x86)\MySQL"] {
        let Ok(entries) = std::fs::read_dir(root) else {
            continue;
        };
        for entry in entries.flatten() {
            let executable = entry.path().join("bin").join("mysqld.exe");
            if executable.is_file() {
                found.push(executable);
            }
        }
    }
    found.sort_by(|left, right| mysql_version_key(right).cmp(&mysql_version_key(left)));
    found
}

pub fn choose_mysql_executable(defaults: &[PathBuf], custom: &str) -> (&'static str, String) {
    let mut found: Vec<&PathBuf> = defaults
        .iter()
        .filter(|path| {
            path.is_file()
                && path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.eq_ignore_ascii_case("mysqld.exe"))
        })
        .collect();
    found.sort_by(|left, right| mysql_version_key(right).cmp(&mysql_version_key(left)));
    if let Some(path) = found.first() {
        return ("default", path.to_string_lossy().to_string());
    }
    let custom = custom.trim();
    if custom.is_empty() {
        ("unset", String::new())
    } else {
        ("custom", custom.to_string())
    }
}

fn mysql_version_key(path: &Path) -> Vec<u32> {
    let name = path
        .parent()
        .and_then(|bin| bin.parent())
        .and_then(|server| server.file_name())
        .and_then(|name| name.to_str())
        .unwrap_or("");
    name.split(|ch: char| !ch.is_ascii_digit())
        .filter(|part| !part.is_empty())
        .filter_map(|part| part.parse().ok())
        .collect()
}

fn prepare_for_launch(
    kind: &str,
    config: &LocalDatabase,
    defaults: &[PathBuf],
) -> Result<LocalDatabase, String> {
    let mut saved = normalize(kind, config)?;
    if kind == "mysql" {
        let (source, executable) = choose_mysql_executable(defaults, &saved.executable);
        if source == "unset" {
            return Err("请先填写 mysqld.exe 的绝对路径".into());
        }
        saved.executable = executable;
    }
    apply_located(kind, &mut saved);
    Ok(saved)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocatedFiles {
    pub config_file: String,
    pub data_dir: String,
    pub message: String,
}

pub fn locate_files(kind: &str, executable: &str) -> LocatedFiles {
    let path = Path::new(executable.trim());
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("");
    let located = if !path.is_file() {
        None
    } else if kind == "mysql" && name.eq_ignore_ascii_case("mysqld.exe") {
        Some(locate_mysql(path))
    } else if kind == "redis" && name.eq_ignore_ascii_case("redis-server.exe") {
        Some(locate_redis(path))
    } else {
        None
    };
    let (config_file, data_dir) = located.unwrap_or_default();
    let message = match (config_file.is_empty(), data_dir.is_empty()) {
        (false, false) => "已在这个程序旁边找到配置文件和数据目录。".into(),
        (false, true) => "已在这个程序旁边找到配置文件，没有找到数据目录。".into(),
        (true, false) => "已在这个程序旁边找到数据目录，没有找到配置文件。".into(),
        (true, true) => "在这个程序旁边没有找到配置文件或数据目录。".into(),
    };
    LocatedFiles {
        config_file,
        data_dir,
        message,
    }
}

fn locate_mysql(executable: &Path) -> (String, String) {
    let root = install_root(executable);
    let mut configs = vec![
        root.join("my.ini"),
        root.join("my.cnf"),
        root.join("bin").join("my.ini"),
    ];
    if let Some(name) = root.file_name() {
        configs.push(
            PathBuf::from(r"C:\ProgramData\MySQL")
                .join(name)
                .join("my.ini"),
        );
    }
    let config = configs.into_iter().find(|path| path.is_file());
    let configured_dir = config.as_ref().and_then(|path| {
        let text = std::fs::read_to_string(path).unwrap_or_default();
        ini_value(&text, "datadir").map(|value| resolve_beside(&root, &value))
    });
    let data = configured_dir
        .and_then(existing_dir)
        .or_else(|| [root.join("data"), root.join("Data")].into_iter().find_map(existing_dir));
    (
        config.map(|path| path.to_string_lossy().to_string()).unwrap_or_default(),
        data.map(|path| path.to_string_lossy().to_string()).unwrap_or_default(),
    )
}

fn locate_redis(executable: &Path) -> (String, String) {
    let root = install_root(executable);
    let config = ["redis.windows.conf", "redis.windows-service.conf", "redis.conf"]
        .into_iter()
        .map(|name| root.join(name))
        .find(|path| path.is_file());
    let configured_dir = config.as_ref().and_then(|path| {
        let text = std::fs::read_to_string(path).unwrap_or_default();
        ini_value(&text, "dir").map(|value| resolve_beside(&root, &value))
    });
    let data = configured_dir
        .and_then(existing_dir)
        .or_else(|| existing_dir(root.join("data")));
    (
        config.map(|path| path.to_string_lossy().to_string()).unwrap_or_default(),
        data.map(|path| path.to_string_lossy().to_string()).unwrap_or_default(),
    )
}

fn install_root(executable: &Path) -> PathBuf {
    let parent = executable.parent().unwrap_or(executable);
    if parent
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.eq_ignore_ascii_case("bin"))
    {
        parent.parent().unwrap_or(parent).to_path_buf()
    } else {
        parent.to_path_buf()
    }
}

fn existing_dir(path: PathBuf) -> Option<PathBuf> {
    path.is_dir()
        .then(|| path.canonicalize().unwrap_or(path))
}

fn resolve_beside(root: &Path, value: &str) -> PathBuf {
    let path = PathBuf::from(value.trim());
    if path.is_absolute() {
        path
    } else {
        root.join(path)
    }
}

fn ini_value(text: &str, key: &str) -> Option<String> {
    text.lines().find_map(|line| {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            return None;
        }
        let (name, value) = if let Some(pair) = line.split_once('=') {
            pair
        } else {
            line.split_once(char::is_whitespace)?
        };
        if !name.trim().eq_ignore_ascii_case(key) {
            return None;
        }
        let value = value.trim().trim_matches('"').trim_matches('\'');
        (!value.is_empty()).then(|| value.to_string())
    })
}

fn apply_located(kind: &str, saved: &mut LocalDatabase) {
    if saved.executable.is_empty() || (!saved.config_file.is_empty() && !saved.data_dir.is_empty()) {
        return;
    }
    let found = locate_files(kind, &saved.executable);
    if saved.config_file.is_empty() {
        saved.config_file = found.config_file;
    }
    if saved.data_dir.is_empty() {
        saved.data_dir = found.data_dir;
    }
}

pub fn mysql_resolution(config: &LocalDatabase) -> (&'static str, String) {
    choose_mysql_executable(&default_mysql_paths(), &config.executable)
}

pub fn launch_plan(kind: &str, config: &LocalDatabase) -> Result<LaunchPlan, String> {
    launch_with_defaults(kind, config, &default_mysql_paths())
}

fn launch_with_defaults(
    kind: &str,
    config: &LocalDatabase,
    defaults: &[PathBuf],
) -> Result<LaunchPlan, String> {
    let saved = prepare_for_launch(kind, config, defaults)?;
    let expected = expected_exe(kind)?;
    if saved.executable.is_empty() {
        return Err(format!("请先填写 {expected} 的绝对路径"));
    }
    let executable = PathBuf::from(&saved.executable);
    if !executable.is_file() {
        return Err(format!("找不到程序：{}", saved.executable));
    }
    if !saved.config_file.is_empty() {
        let config_path = PathBuf::from(&saved.config_file);
        if !config_path.is_file() {
            return Err(format!("找不到配置文件：{}", saved.config_file));
        }
        let args = if kind == "mysql" {
            vec![
                format!("--defaults-file=\"{}\"", saved.config_file),
                "--console".into(),
            ]
        } else {
            vec![saved.config_file]
        };
        return Ok(LaunchPlan { executable, args });
    }
    if saved.data_dir.is_empty() {
        return Err("请填写数据目录，或改填配置文件".into());
    }
    if !Path::new(&saved.data_dir).is_dir() {
        return Err(format!("数据目录不存在：{}", saved.data_dir));
    }
    let args = if kind == "mysql" {
        vec![
            "--no-defaults".into(),
            "--bind-address=127.0.0.1".into(),
            format!("--port={}", saved.port),
            format!("--datadir=\"{}\"", saved.data_dir),
            "--console".into(),
        ]
    } else {
        let mut args = vec![
            "--bind".into(),
            "127.0.0.1".into(),
            "--port".into(),
            saved.port.to_string(),
            "--dir".into(),
            saved.data_dir,
            "--protected-mode".into(),
            "yes".into(),
        ];
        if !saved.password.is_empty() {
            args.push("--requirepass".into());
            args.push(saved.password);
        }
        args
    };
    Ok(LaunchPlan { executable, args })
}

pub fn spawn_plan(plan: &LaunchPlan) -> Result<u32, String> {
    let mut cmd = Command::new(&plan.executable);
    cmd.args(&plan.args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if let Some(dir) = plan.executable.parent() {
        cmd.current_dir(dir);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x08000000);
    }
    let child = cmd.spawn().map_err(|error| format!("启动失败：{error}"))?;
    Ok(child.id())
}

fn clean_path(value: &str) -> Result<String, String> {
    let value = value.trim();
    if value.is_empty() {
        return Ok(String::new());
    }
    if value.chars().any(|c| c.is_control() || c == '"') {
        return Err("路径里不能包含引号或控制字符".into());
    }
    if !Path::new(value).is_absolute() {
        return Err("请填写绝对路径".into());
    }
    Ok(value.to_string())
}

fn process_matches(pid: u32, expected_exe: &str) -> bool {
    let mut sys = System::new();
    let pid = Pid::from_u32(pid);
    sys.refresh_processes(ProcessesToUpdate::Some(&[pid]), true);
    sys.process(pid)
        .and_then(|process| process.exe())
        .and_then(|path| path.file_name())
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.eq_ignore_ascii_case(expected_exe))
}

fn executable_key(path: &Path) -> String {
    let mut text = path.to_string_lossy().trim().to_string();
    if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
        text = format!(r"\\{rest}");
    } else if let Some(rest) = text.strip_prefix(r"\\?\") {
        text = rest.to_string();
    }
    text.replace('/', "\\").trim_end_matches('\\').to_ascii_lowercase()
}

pub fn same_executable(left: &Path, right: &Path) -> bool {
    let left = executable_key(left);
    let right = executable_key(right);
    !left.is_empty() && left == right
}

/// 找出可执行文件路径与配置一致的进程。只认完整路径，不认同名的另一个程序。
pub fn pids_for_executable<'a>(
    executable: &str,
    processes: impl IntoIterator<Item = (u32, &'a Path)>,
) -> Vec<u32> {
    let wanted = PathBuf::from(executable.trim());
    if wanted.as_os_str().is_empty() {
        return Vec::new();
    }
    processes
        .into_iter()
        .filter(|(_, path)| same_executable(path, &wanted))
        .map(|(pid, _)| pid)
        .collect()
}

pub fn scan_executable(executable: &str) -> Vec<u32> {
    if executable.trim().is_empty() {
        return Vec::new();
    }
    let mut sys = System::new();
    sys.refresh_processes(ProcessesToUpdate::All, true);
    let found: Vec<(u32, PathBuf)> = sys
        .processes()
        .iter()
        .filter_map(|(pid, process)| process.exe().map(|exe| (pid.as_u32(), exe.to_path_buf())))
        .collect();
    pids_for_executable(executable, found.iter().map(|(pid, path)| (*pid, path.as_path())))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{write_store, PersistFile};
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn scratch() -> PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("xchen-local-infra-{stamp}"));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn saved_config_roundtrips_and_rejects_missing_or_illegal_programs() {
        let mut file = PersistFile::default();
        file.local_databases.mysql.executable = r"D:\db\bin\mysqld.exe".into();
        file.local_databases.mysql.port = 3307;
        file.local_databases.redis.password = "local-pass".into();
        let raw = serde_json::to_string(&file).unwrap();
        let loaded: PersistFile = serde_json::from_str(&raw).unwrap();
        assert_eq!(loaded.local_databases.mysql.port, 3307);
        assert_eq!(loaded.local_databases.redis.password, "local-pass");
        let legacy: PersistFile = serde_json::from_str(r#"{"version":2}"#).unwrap();
        assert_eq!(legacy.local_databases.mysql.port, 3306);
        assert_eq!(legacy.local_databases.redis.port, 6379);

        let dir = scratch();
        let saved_path = dir.join("config.json");
        write_store(&saved_path, &loaded).unwrap();
        let from_disk = fs::read_to_string(&saved_path).unwrap();
        assert!(from_disk.contains("mysqld.exe"));
        assert!(from_disk.contains("3307"));

        let missing = LocalDatabase {
            executable: r"D:\missing\mysqld.exe".into(),
            port: 3306,
            data_dir: dir.to_string_lossy().to_string(),
            ..LocalDatabase::default()
        };
        assert!(launch_with_defaults("mysql", &missing, &[]).is_err());

        let illegal = LocalDatabase {
            executable: r"C:\Windows\System32\cmd.exe".into(),
            port: 3306,
            data_dir: dir.to_string_lossy().to_string(),
            ..LocalDatabase::default()
        };
        assert!(normalize("mysql", &illegal).is_err());
        assert!(launch_plan("mysql", &illegal).is_err());
        assert!(normalize(
            "redis",
            &LocalDatabase {
                executable: r"redis-server.exe".into(),
                port: 6379,
                ..LocalDatabase::default()
            },
        )
        .is_err());

        let program = dir.join("mysqld.exe");
        let config = dir.join("my.ini");
        fs::write(&program, b"").unwrap();
        fs::write(&config, b"[mysqld]\n").unwrap();
        let planned = launch_plan(
            "mysql",
            &LocalDatabase {
                executable: program.to_string_lossy().to_string(),
                port: 3308,
                data_dir: dir.to_string_lossy().to_string(),
                config_file: config.to_string_lossy().to_string(),
                password: String::new(),
            },
        )
        .unwrap();
        assert_eq!(planned.args.len(), 2);
        assert!(planned.args[0].starts_with("--defaults-file=\""));
        assert!(planned.args[0].contains("my.ini"));
        assert!(planned.args[0].ends_with('"'));
        assert!(!planned.args.iter().any(|arg| arg.contains("3308")));

        let redis_program = dir.join("redis-server.exe");
        fs::write(&redis_program, b"").unwrap();
        let data_dir = dir.to_string_lossy().to_string();
        let redis = launch_plan(
            "redis",
            &LocalDatabase {
                executable: redis_program.to_string_lossy().to_string(),
                port: 6380,
                data_dir: data_dir.clone(),
                password: "secret".into(),
                ..LocalDatabase::default()
            },
        )
        .unwrap();
        assert_eq!(
            redis.args,
            vec![
                "--bind".to_string(),
                "127.0.0.1".to_string(),
                "--port".to_string(),
                "6380".to_string(),
                "--dir".to_string(),
                data_dir,
                "--protected-mode".to_string(),
                "yes".to_string(),
                "--requirepass".to_string(),
                "secret".to_string(),
            ]
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn mysql_prefers_the_highest_default_install_over_a_custom_path() {
        let dir = scratch();
        let older = dir.join("MySQL Server 5.7").join("bin");
        let newer = dir.join("MySQL Server 8.4").join("bin");
        fs::create_dir_all(&older).unwrap();
        fs::create_dir_all(&newer).unwrap();
        let older_exe = older.join("mysqld.exe");
        let newer_exe = newer.join("mysqld.exe");
        fs::write(&older_exe, b"").unwrap();
        fs::write(&newer_exe, b"").unwrap();
        let custom = dir.join("custom").join("mysqld.exe");
        fs::create_dir_all(custom.parent().unwrap()).unwrap();
        fs::write(&custom, b"").unwrap();
        let (source, chosen) = choose_mysql_executable(
            &[older_exe, newer_exe.clone()],
            custom.to_str().unwrap(),
        );
        assert_eq!(source, "default");
        assert_eq!(chosen, newer_exe.to_string_lossy());
        let (source, chosen) = choose_mysql_executable(&[], custom.to_str().unwrap());
        assert_eq!(source, "custom");
        assert_eq!(chosen, custom.to_string_lossy());
        let (source, chosen) = choose_mysql_executable(&[], "  ");
        assert_eq!(source, "unset");
        assert!(chosen.is_empty());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn selected_program_finds_the_config_and_data_directory_beside_it() {
        let dir = scratch();
        let bin = dir.join("bin");
        let data = dir.join("data");
        fs::create_dir_all(&bin).unwrap();
        fs::create_dir_all(&data).unwrap();
        let executable = bin.join("mysqld.exe");
        fs::write(&executable, b"").unwrap();
        fs::write(dir.join("my.ini"), format!("datadir={}\n", data.display())).unwrap();
        let found = locate_files("mysql", &executable.to_string_lossy());
        assert!(found.config_file.ends_with("my.ini"));
        assert_eq!(found.data_dir, data.canonicalize().unwrap().to_string_lossy());
        assert!(found.message.contains("配置文件和数据目录"));

        let redis = dir.join("redis-server.exe");
        let redis_data = dir.join("redis-data");
        fs::create_dir_all(&redis_data).unwrap();
        fs::write(&redis, b"").unwrap();
        fs::write(dir.join("redis.windows.conf"), "dir ./redis-data\n").unwrap();
        let found = locate_files("redis", &redis.to_string_lossy());
        assert!(found.config_file.ends_with("redis.windows.conf"));
        assert_eq!(found.data_dir, redis_data.canonicalize().unwrap().to_string_lossy());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn running_program_matches_the_configured_path_only() {
        let configured = Path::new(r"D:\Coding\software\redis\redis-server.exe");
        let processes = [
            (8948, Path::new(r"\\?\D:\Coding\software\redis\redis-server.exe")),
            (12, Path::new(r"D:\other\redis-server.exe")),
            (13, Path::new(r"D:\Coding\software\redis\redis-cli.exe")),
        ];
        assert_eq!(pids_for_executable(configured.to_str().unwrap(), processes), vec![8948]);
        assert!(pids_for_executable("  ", processes).is_empty());
        assert!(same_executable(
            Path::new(r"D:/Coding/software/redis/redis-server.exe"),
            configured,
        ));
    }

    #[test]
    fn scan_finds_the_current_process_by_its_own_executable() {
        let executable = std::env::current_exe().unwrap();
        assert!(scan_executable(&executable.to_string_lossy()).contains(&std::process::id()));
        assert!(scan_executable("").is_empty());
    }
}
