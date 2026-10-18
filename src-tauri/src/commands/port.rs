use crate::net::{self, Connection};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::process::Command;

/// 与第二版一致的扫描忽略目录清单（workspace 与 port 探测共用）。
pub(crate) const IGNORE_DIRS: &[&str] = &[
    "node_modules",
    ".git",
    ".idea",
    ".vscode",
    "dist",
    "build",
    "out",
    "target",
    "coverage",
    "release",
    ".next",
    ".nuxt",
    ".cache",
    ".tmp",
    "logs",
];

/// 第二版前端消费的端口条目：仅 port/protocol/pid 三个字段。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PortEntry {
    pub port: u16,
    /// tcp | udp
    pub protocol: String,
    /// 未知归属时为 0
    pub pid: u32,
}

fn entries_from(conns: Vec<Connection>) -> Vec<PortEntry> {
    conns
        .into_iter()
        .filter(|c| c.local_port != 0)
        .map(|c| PortEntry {
            port: c.local_port,
            protocol: c.protocol,
            pid: c.pid,
        })
        .collect()
}

fn all_entries() -> Result<Vec<PortEntry>, String> {
    let mut out = entries_from(net::list_tcp()?);
    out.extend(entries_from(net::list_udp()?));
    Ok(out)
}

#[tauri::command]
pub fn query_single_port(port: u16, protocol: String) -> Result<Vec<PortEntry>, String> {
    if port == 0 {
        return Err("端口无效".into());
    }
    let proto = protocol.trim().to_lowercase();
    let conns = match proto.as_str() {
        "tcp" => net::list_tcp()?,
        "udp" => net::list_udp()?,
        _ => {
            let mut out = net::list_tcp()?;
            out.extend(net::list_udp()?);
            out
        }
    };
    Ok(entries_from(conns)
        .into_iter()
        .filter(|e| e.port == port)
        .collect())
}

#[tauri::command]
pub fn query_multiple_ports(ports: Vec<u16>) -> Result<Vec<PortEntry>, String> {
    let set: std::collections::HashSet<u16> = ports.into_iter().filter(|p| *p != 0).collect();
    Ok(all_entries()?
        .into_iter()
        .filter(|e| set.contains(&e.port))
        .collect())
}

#[tauri::command]
pub fn query_ports_range(start: u16, end: u16) -> Result<Vec<PortEntry>, String> {
    if start == 0 || end < start {
        return Err("端口范围无效".into());
    }
    Ok(all_entries()?
        .into_iter()
        .filter(|e| e.port >= start && e.port <= end)
        .collect())
}

#[tauri::command]
pub fn get_all_tcp_connections() -> Result<Vec<Connection>, String> {
    net::list_tcp()
}

#[tauri::command]
pub fn get_all_udp_listeners() -> Result<Vec<Connection>, String> {
    net::list_udp()
}

/// 释放端口占用：对所有监听该端口的进程执行 taskkill /F。
/// 返回逐 PID 结果（与 release_path 同构）。
#[tauri::command]
pub fn release_port(port: u16) -> Result<Vec<serde_json::Value>, String> {
    if port == 0 {
        return Err("端口无效".into());
    }
    let mut connections = net::list_tcp()?;
    connections.extend(net::list_udp()?);
    let pids = listener_pids(connections, port);
    Ok(pids
        .into_iter()
        .map(|pid| {
            let ok = taskkill(pid, false).is_ok();
            serde_json::json!({
                "success": ok,
                "pid": pid,
                "message": if ok { "已终止".to_string() } else { "终止失败".to_string() },
            })
        })
        .collect())
}

fn listener_pids(connections: Vec<Connection>, port: u16) -> std::collections::BTreeSet<u32> {
    connections
        .into_iter()
        .filter(|c| {
            c.local_port == port
                && c.pid != 0
                && (c.protocol == "udp" || matches!(c.state.as_str(), "LISTEN" | "LISTENING"))
        })
        .map(|c| c.pid)
        .collect()
}

/// 从常见前端/后端配置文件启发式探测端口。
/// 覆盖：根目录前端配置（vite/webpack/.env）、递归搜索的 Spring 配置
/// （application*.yml/properties、bootstrap*.yml，含多模块子目录与 profile 变体）。
pub(crate) fn detect_port_in_dir(dir: &Path) -> Option<u16> {
    let port_re = regex::Regex::new(r#"(?i)port["']?\s*[:=]\s*(\d{1,5})"#).unwrap();
    let server_re = regex::Regex::new(r#"(?i)server\.port\s*[:=]\s*(\d{1,5})"#).unwrap();

    // 第 1 层：根目录前端配置（vite/webpack/nuxt/.env）
    let root_names = [
        "vite.config.ts",
        "vite.config.js",
        "vite.config.mjs",
        "webpack.config.js",
        "nuxt.config.ts",
        ".env",
        ".env.local",
    ];
    for name in root_names {
        let file = dir.join(name);
        if let Ok(text) = std::fs::read_to_string(&file) {
            if let Some(m) = port_re.captures(&text) {
                if let Ok(port) = m[1].parse::<u16>() {
                    if port > 0 {
                        return Some(port);
                    }
                }
            }
        }
    }

    // 第 2 层：递归搜索 Spring/Boot 配置（application*.yml|yaml|properties、bootstrap*.yml），
    // 优先 server.port；跳过 target/node_modules 等构建产物目录。
    let config_re = regex::Regex::new(
        r#"(?i)^application.*\.(yml|yaml|properties)$|^bootstrap.*\.(yml|yaml|properties)$"#,
    )
    .unwrap();
    let mut configs = Vec::new();
    collect_config_files(dir, 5, &config_re, &mut configs);
    // server.port 优先
    for file in &configs {
        if let Ok(text) = std::fs::read_to_string(file) {
            if let Some(m) = server_re.captures(&text) {
                if let Ok(port) = m[1].parse::<u16>() {
                    if port > 0 {
                        return Some(port);
                    }
                }
            }
        }
    }
    for file in &configs {
        if let Ok(text) = std::fs::read_to_string(file) {
            if let Some(m) = port_re.captures(&text) {
                if let Ok(port) = m[1].parse::<u16>() {
                    if port > 0 {
                        return Some(port);
                    }
                }
            }
        }
    }
    None
}

fn collect_config_files(dir: &Path, depth: u32, pattern: &regex::Regex, out: &mut Vec<PathBuf>) {
    if depth == 0 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            let name = entry.file_name().to_string_lossy().to_string();
            if IGNORE_DIRS.contains(&name.as_str()) {
                continue;
            }
            collect_config_files(&path, depth - 1, pattern, out);
        } else if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
            if pattern.is_match(name) {
                out.push(path);
            }
        }
    }
}

pub(crate) fn taskkill(pid: u32, tree: bool) -> Result<(), String> {
    let mut cmd = Command::new("taskkill");
    cmd.arg("/PID").arg(pid.to_string());
    if tree {
        cmd.arg("/T");
    }
    cmd.arg("/F");
    let out = cmd
        .output()
        .map_err(|e| format!("Failed to execute taskkill: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!(
            "Failed to kill process {pid}: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn release_targets_unique_listeners_and_leaves_client_connections_out() {
        let connection = |pid, state: &str, protocol: &str| Connection {
            pid,
            local_port: 7779,
            state: state.into(),
            protocol: protocol.into(),
            local_address: "127.0.0.1".into(),
            remote_address: String::new(),
            remote_port: 0,
        };
        let pids = listener_pids(
            vec![
                connection(1, "LISTEN", "tcp"),
                connection(1, "LISTEN", "tcp"),
                connection(2, "ESTABLISHED", "tcp"),
                connection(3, "", "udp"),
                connection(0, "LISTEN", "tcp"),
            ],
            7779,
        );
        assert_eq!(pids.into_iter().collect::<Vec<_>>(), vec![1, 3]);
    }
    #[cfg(windows)]
    #[test]
    fn real_port_and_process_queries_find_this_test_process() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let entries = query_multiple_ports(vec![port]).unwrap();
        assert!(entries
            .iter()
            .any(|e| e.port == port && e.pid == std::process::id()));
        assert!(crate::commands::process::list_processes()
            .iter()
            .any(|p| p.pid == std::process::id()));
        assert!(query_ports_range(0, 1).is_err());
        assert!(query_ports_range(100, 1).is_err());
    }
}
