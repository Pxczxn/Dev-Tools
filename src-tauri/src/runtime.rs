//! 服务运行时：会话管理、日志环形缓冲、`log:batch` / `runtime:changed` 事件流。
//! 状态机与第二版一致：stopped → starting → running → stopping → exited/failed。

use serde::Serialize;
use std::collections::{HashMap, VecDeque};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter};

// Line count alone does not bound memory for SQL/JSON output.
const MAX_LOG_BYTES: usize = 8 * 1024 * 1024;
const MAX_LOG_LINE_BYTES: usize = 64 * 1024;
const TRUNCATED_LOG: &str = " …[单行超过 64KB，后续内容已截断]";

#[derive(Default)]
struct LogBuffer {
    entries: VecDeque<LogEntry>,
    bytes: usize,
}

impl LogEntry {
    fn retained_bytes(&self) -> usize {
        std::mem::size_of::<Self>() + self.timestamp.len() + self.stream.len() + self.text.len()
    }
}

/// 启动参数聚合（避免长参数列表）。
#[derive(Clone, Copy)]
pub struct StartSpec<'a> {
    pub service_id: &'a str,
    pub cwd: &'a str,
    pub command: &'a str,
    pub args: &'a [String],
    pub shell_mode: bool,
    pub env: &'a std::collections::BTreeMap<String, String>,
    pub cap: usize,
    pub startup_delay: u32,
    pub prepare: &'a [crate::launch::LaunchCommand],
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LogEntry {
    pub timestamp: String,
    /// stdout | stderr
    pub stream: String,
    pub text: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunState {
    pub service_id: String,
    pub status: String,
    pub pid: Option<u32>,
    pub ready: bool,
    pub started_at: Option<String>,
    pub stopped_at: Option<String>,
    pub exit_code: Option<i32>,
    pub error: Option<String>,
}

pub struct Session {
    pub service_id: String,
    control: Mutex<()>,
    status: Mutex<String>,
    pid: Mutex<Option<u32>>,
    exit_code: Mutex<Option<i32>>,
    error: Mutex<Option<String>>,
    started_at: Mutex<Option<String>>,
    stopped_at: Mutex<Option<String>>,
    stopping: AtomicBool,
    logs: Mutex<LogBuffer>,
    cap: Mutex<usize>,
}

impl Session {
    fn new(service_id: &str, cap: usize) -> Self {
        Self {
            service_id: service_id.to_string(),
            control: Mutex::new(()),
            status: Mutex::new("stopped".into()),
            pid: Mutex::new(None),
            exit_code: Mutex::new(None),
            error: Mutex::new(None),
            started_at: Mutex::new(None),
            stopped_at: Mutex::new(None),
            stopping: AtomicBool::new(false),
            logs: Mutex::new(LogBuffer::default()),
            cap: Mutex::new(cap),
        }
    }

    pub fn set_status(&self, status: &str) {
        *self.status.lock().unwrap() = status.to_string();
    }

    pub fn status(&self) -> String {
        self.status.lock().unwrap().clone()
    }

    pub fn set_pid(&self, pid: Option<u32>) {
        *self.pid.lock().unwrap() = pid;
    }

    pub fn pid(&self) -> Option<u32> {
        *self.pid.lock().unwrap()
    }

    pub fn mark_stopping(&self) -> bool {
        // 幂等：已在停止流程中则返回 false
        !self.stopping.swap(true, Ordering::SeqCst)
    }

    pub fn reset_stopping(&self) {
        self.stopping.store(false, Ordering::SeqCst);
    }

    fn finish(&self, exit_code: Option<i32>, error: Option<String>) {
        self.set_pid(None);
        *self.exit_code.lock().unwrap() = exit_code;
        *self.error.lock().unwrap() = error;
        *self.stopped_at.lock().unwrap() = Some(crate::store::now_iso());
        let status = if self.stopping.load(Ordering::SeqCst) {
            "stopped"
        } else if exit_code == Some(0) {
            "exited"
        } else {
            "failed"
        };
        self.set_status(status);
    }

    pub fn run_state(&self) -> RunState {
        RunState {
            service_id: self.service_id.clone(),
            status: self.status(),
            pid: self.pid(),
            ready: self.status() == "running",
            started_at: self.started_at.lock().unwrap().clone(),
            stopped_at: self.stopped_at.lock().unwrap().clone(),
            exit_code: *self.exit_code.lock().unwrap(),
            error: self.error.lock().unwrap().clone(),
        }
    }

    pub fn push_log(&self, stream: &str, text: &str) -> Option<LogEntry> {
        let clean = plain_log_text(text);
        // A standalone terminal reset must not create a visible empty log row.
        if clean != text && clean.trim().is_empty() {
            return None;
        }
        let mut logs = self.logs.lock().unwrap();
        let entry = LogEntry {
            timestamp: crate::store::now_iso(),
            stream: stream.to_string(),
            text: clean.into_owned(),
        };
        logs.bytes += entry.retained_bytes();
        logs.entries.push_back(entry.clone());
        let cap = *self.cap.lock().unwrap();
        while logs.entries.len() > cap || logs.bytes > MAX_LOG_BYTES {
            if let Some(old) = logs.entries.pop_front() {
                logs.bytes -= old.retained_bytes();
            }
        }
        Some(entry)
    }

    pub fn history(&self, limit: Option<u32>) -> Vec<LogEntry> {
        let logs = self.logs.lock().unwrap();
        match limit {
            Some(n) if (n as usize) < logs.entries.len() => logs
                .entries
                .iter()
                .skip(logs.entries.len() - n as usize)
                .cloned()
                .collect(),
            _ => logs.entries.iter().cloned().collect(),
        }
    }

    pub fn clear_logs(&self) {
        *self.logs.lock().unwrap() = LogBuffer::default();
    }
}

/// The log viewer and exports are plain text, not terminal emulators.
fn plain_log_text(text: &str) -> std::borrow::Cow<'_, str> {
    if !text.contains('\u{1b}') {
        return std::borrow::Cow::Borrowed(text);
    }
    static TERMINAL_SEQUENCES: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    TERMINAL_SEQUENCES
        .get_or_init(|| {
            regex::Regex::new(r"\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)|\x1b\[[0-?]*[ -/]*[@-~]")
                .expect("valid ANSI sequence pattern")
        })
        .replace_all(text, "")
}

#[derive(Default)]
pub struct RuntimeManager {
    sessions: Mutex<HashMap<String, Arc<Session>>>,
    workspace_launches: Mutex<HashMap<String, Arc<AtomicBool>>>,
}

pub struct WorkspaceLaunch<'a> {
    runtime: &'a RuntimeManager,
    id: String,
    cancelled: Arc<AtomicBool>,
}
impl WorkspaceLaunch<'_> {
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }
}
impl Drop for WorkspaceLaunch<'_> {
    fn drop(&mut self) {
        self.runtime
            .workspace_launches
            .lock()
            .unwrap()
            .remove(&self.id);
    }
}

impl RuntimeManager {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn begin_workspace_launch(&self, id: &str) -> Result<WorkspaceLaunch<'_>, String> {
        let mut launches = self.workspace_launches.lock().unwrap();
        if launches.contains_key(id) {
            return Err("该工作区正在批量启动，请等待完成或停止后再试".into());
        }
        let cancelled = Arc::new(AtomicBool::new(false));
        launches.insert(id.into(), cancelled.clone());
        Ok(WorkspaceLaunch {
            runtime: self,
            id: id.into(),
            cancelled,
        })
    }
    pub fn cancel_workspace_launch(&self, id: &str) {
        if let Some(cancelled) = self.workspace_launches.lock().unwrap().get(id) {
            cancelled.store(true, Ordering::SeqCst);
        }
    }
    pub fn cancel_all_workspace_launches(&self) {
        for cancelled in self.workspace_launches.lock().unwrap().values() {
            cancelled.store(true, Ordering::SeqCst);
        }
    }

    /// 取既有会话；没有则创建（stopped 状态），保证 get_process_runtime 能看到全部已知服务。
    pub fn ensure_session(&self, service_id: &str, cap: usize) -> Arc<Session> {
        let mut sessions = self.sessions.lock().unwrap();
        sessions
            .entry(service_id.to_string())
            .or_insert_with(|| Arc::new(Session::new(service_id, cap)))
            .clone()
    }

    pub fn all_run_states(&self) -> Vec<RunState> {
        let sessions = self.sessions.lock().unwrap();
        sessions.values().map(|s| s.run_state()).collect()
    }

    pub fn running_service_ids(&self) -> Vec<String> {
        let sessions = self.sessions.lock().unwrap();
        sessions
            .iter()
            .filter(|(_, s)| matches!(s.status().as_str(), "starting" | "running" | "stopping"))
            .map(|(id, _)| id.clone())
            .collect()
    }

    pub fn get(&self, service_id: &str) -> Option<Arc<Session>> {
        self.sessions.lock().unwrap().get(service_id).cloned()
    }

    /// 启动服务进程：创建会话、拉起子进程、挂接输出捕获与退出监听线程。
    /// command + args 分离；shell_mode 时经 cmd /C 执行。
    pub fn start(&self, app: &AppHandle, spec: StartSpec<'_>) -> Result<RunState, String> {
        let service_id = spec.service_id;
        let session = self.ensure_session(service_id, spec.cap);
        let _control = session.control.lock().unwrap();
        if matches!(
            session.status().as_str(),
            "running" | "starting" | "stopping"
        ) {
            return Err(format!(
                "Service {service_id} is already running or stopping"
            ));
        }
        *session.cap.lock().unwrap() = spec.cap;

        session.reset_stopping();
        session.set_status("starting");
        session.clear_logs();
        *session.started_at.lock().unwrap() = Some(crate::store::now_iso());
        *session.exit_code.lock().unwrap() = None;
        *session.error.lock().unwrap() = None;
        *session.stopped_at.lock().unwrap() = None;

        let mut steps = spec.prepare.to_vec();
        steps.push(crate::launch::LaunchCommand {
            cwd: spec.cwd.into(),
            command: spec.command.into(),
            args: spec.args.to_vec(),
            shell_mode: spec.shell_mode,
        });
        let env = spec.env.clone();
        let session_thread = session.clone();
        let state_app = app.clone();
        let logs_app = app.clone();
        let sid = service_id.to_string();
        let on_state: StateCallback = Arc::new(move |state| {
            let _ = state_app.emit(
                "runtime:changed",
                serde_json::json!({"serviceId": state.service_id, "runtime": state}),
            );
        });
        let on_logs: LogCallback = Arc::new(move |entries| {
            let _ = logs_app.emit(
                "log:batch",
                serde_json::json!({"serviceId": sid, "entries": entries}),
            );
        });
        on_state(session.run_state());
        let delay = std::time::Duration::from_millis(spec.startup_delay as u64);
        std::thread::spawn(move || {
            let deadline = std::time::Instant::now() + delay;
            while std::time::Instant::now() < deadline
                && !session_thread.stopping.load(Ordering::SeqCst)
            {
                std::thread::sleep(std::time::Duration::from_millis(25));
            }
            run_launch_steps(session_thread, steps, env, on_state, on_logs);
        });
        Ok(session.run_state())
    }

    /// 停止服务：标记 stopping → taskkill /T /F → 等待线程回写 stopped。
    pub fn stop(&self, app: &AppHandle, service_id: &str) -> Result<RunState, String> {
        let session = self
            .get(service_id)
            .ok_or_else(|| format!("Service not found: {service_id}"))?;
        let _control = session.control.lock().unwrap();
        let status = session.status();
        if status == "stopped" || status == "exited" || status == "failed" {
            return Ok(session.run_state());
        }
        if !session.mark_stopping() {
            return Err(format!(
                "Service {service_id} is already running or stopping"
            ));
        }
        session.set_status("stopping");
        let _ = app.emit(
            "runtime:changed",
            serde_json::json!({ "serviceId": service_id, "runtime": session.run_state() }),
        );
        if let Some(pid) = session.pid() {
            if let Err(error) = crate::commands::port::taskkill(pid, true) {
                if session.pid().is_some() && session.status() == "stopping" {
                    session.reset_stopping();
                    session.set_status(&status);
                    let _ = app.emit("runtime:changed", serde_json::json!({"serviceId": service_id, "runtime": session.run_state()}));
                    return Err(error);
                }
            }
        }
        // taskkill 后由退出监听线程回写终态；这里直接返回当前状态
        Ok(session.run_state())
    }
}

/// Windows 下的程序解析：Command::new 只解析 .exe，而 mvn/npm/pnpm 等是
/// .cmd 批处理——先用 where.exe 按 PATHEXT 解析出真实路径，批处理再经 cmd /C 执行。
pub(crate) fn resolve_program(spec: &StartSpec<'_>) -> String {
    let command = spec.command;
    // 1) 相对服务 cwd 的 wrapper（如 mvnw.cmd）
    let cwd_path = Path::new(spec.cwd).join(command);
    if cwd_path.is_file() {
        return cwd_path.to_string_lossy().to_string();
    }
    // 2) 已是绝对/存在的路径
    if Path::new(command).is_file() {
        return command.to_string();
    }
    // 3) PATH 解析（覆盖 .exe/.cmd/.bat）；where 可能同时返回无扩展名的
    //    bash 脚本与 .cmd，优先可被 Windows 直接执行/包装执行的条目
    if let Ok(out) = std::process::Command::new("where.exe")
        .arg(command)
        .envs(spec.env)
        .current_dir(spec.cwd)
        .output()
    {
        if out.status.success() {
            let lines: Vec<String> = String::from_utf8_lossy(&out.stdout)
                .lines()
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .map(String::from)
                .collect();
            let pick = |ext: &[&str]| {
                lines
                    .iter()
                    .find(|l| ext.iter().any(|e| l.to_lowercase().ends_with(e)))
                    .cloned()
            };
            if let Some(p) = pick(&[".exe"]) {
                return p;
            }
            if let Some(p) = pick(&[".cmd", ".bat"]) {
                return p;
            }
            if let Some(p) = lines.first() {
                return p.clone();
            }
        }
    }
    command.to_string()
}

fn is_batch(program: &str) -> bool {
    let lower = program.to_lowercase();
    lower.ends_with(".cmd") || lower.ends_with(".bat")
}

pub(crate) fn build_spawn_command(spec: &StartSpec<'_>, program: &str) -> std::process::Command {
    // Corepack 保留项目指定的 pnpm/yarn 版本，不回退成 npm。
    if program == spec.command && ["pnpm", "yarn"].contains(&program) {
        let core_spec = StartSpec {
            command: "corepack",
            ..*spec
        };
        let core = resolve_program(&core_spec);
        if core != "corepack" {
            let mut args = vec![program.to_string()];
            args.extend_from_slice(spec.args);
            return build_spawn_command(
                &StartSpec {
                    args: &args,
                    ..core_spec
                },
                &core,
            );
        }
    }
    let mut cmd = if spec.shell_mode || is_batch(program) {
        let mut c = std::process::Command::new("cmd");
        c.args(["/D", "/S", "/C"]);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            // /S /C 删除最外层双引号；额外一层保护带空格的程序路径。
            let quote = |s: &str| format!("\"{}\"", s.replace('"', "\\\""));
            let line = std::iter::once(quote(program))
                .chain(spec.args.iter().map(|a| quote(a)))
                .collect::<Vec<_>>()
                .join(" ");
            c.raw_arg(format!("\"{line}\""));
        }
        #[cfg(not(windows))]
        {
            c.arg(program).args(spec.args);
        }
        c
    } else {
        let mut c = std::process::Command::new(program);
        c.args(spec.args);
        c
    };
    cmd.current_dir(spec.cwd);
    for (k, v) in spec.env {
        cmd.env(k, v);
    }
    cmd
}

type StateCallback = Arc<dyn Fn(RunState) + Send + Sync>;
type LogCallback = Arc<dyn Fn(Vec<LogEntry>) + Send + Sync>;

fn run_launch_steps(
    session: Arc<Session>,
    steps: Vec<crate::launch::LaunchCommand>,
    env: std::collections::BTreeMap<String, String>,
    on_state: StateCallback,
    on_logs: LogCallback,
) {
    let cap = *session.cap.lock().unwrap();
    for (index, step) in steps.iter().enumerate() {
        let last = index + 1 == steps.len();
        let spawn = {
            let _control = session.control.lock().unwrap();
            if session.stopping.load(Ordering::SeqCst) {
                session.finish(Some(0), None);
                on_state(session.run_state());
                return;
            }
            if let Some(entry) = session.push_log(
                "stdout",
                &format!(
                    "[启动] {}：{} {}（目录：{}）",
                    if last { "运行服务" } else { "前置构建" },
                    step.command,
                    step.args.join(" "),
                    step.cwd
                ),
            ) {
                on_logs(vec![entry]);
            }
            let spec = StartSpec {
                service_id: &session.service_id,
                cwd: &step.cwd,
                command: &step.command,
                args: &step.args,
                shell_mode: step.shell_mode,
                env: &env,
                cap,
                startup_delay: 0,
                prepare: &[],
            };
            let mut cmd = build_spawn_command(&spec, &resolve_program(&spec));
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                cmd.creation_flags(0x0800_0000);
            }
            match cmd
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
            {
                Ok(child) => {
                    session.set_pid(Some(child.id()));
                    session.set_status(if last { "running" } else { "starting" });
                    on_state(session.run_state());
                    Ok(child)
                }
                Err(e) => Err(e),
            }
        };
        let mut child = match spawn {
            Ok(child) => child,
            Err(e) => {
                let message = format!("无法执行 {}：{e}", step.command);
                if let Some(entry) = session.push_log("stderr", &format!("[ERROR] {message}")) {
                    on_logs(vec![entry]);
                }
                session.finish(None, Some(message));
                on_state(session.run_state());
                return;
            }
        };
        let mut readers = Vec::new();
        if let Some(stdout) = child.stdout.take() {
            readers.push(spawn_reader(
                session.clone(),
                stdout,
                "stdout".into(),
                on_logs.clone(),
            ));
        }
        if let Some(stderr) = child.stderr.take() {
            readers.push(spawn_reader(
                session.clone(),
                stderr,
                "stderr".into(),
                on_logs.clone(),
            ));
        }
        let status = child.wait();
        {
            let _control = session.control.lock().unwrap();
            session.set_pid(None);
        }
        for reader in readers {
            let _ = reader.join();
        }
        match status {
            Ok(code) if !last && code.success() && !session.stopping.load(Ordering::SeqCst) => {
                continue
            }
            Ok(code) => {
                let error = (!last && !code.success())
                    .then(|| format!("前置构建失败（退出码 {:?}），未执行启动命令", code.code()));
                if let Some(message) = &error {
                    if let Some(entry) = session.push_log("stderr", &format!("[ERROR] {message}")) {
                        on_logs(vec![entry]);
                    }
                }
                session.finish(code.code(), error);
            }
            Err(e) => session.finish(None, Some(e.to_string())),
        }
        on_state(session.run_state());
        return;
    }
}

fn spawn_reader(
    session: Arc<Session>,
    stream: impl std::io::Read + Send + 'static,
    name: String,
    on_logs: LogCallback,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        read_log_batches(stream, |lines| {
            let entries: Vec<_> = lines
                .iter()
                .filter_map(|text| session.push_log(&name, text))
                .collect();
            on_logs(entries);
        });
    })
}

fn read_log_batches(
    stream: impl std::io::Read + Send + 'static,
    mut emit: impl FnMut(Vec<String>),
) {
    use std::io::{BufRead, BufReader};
    use std::sync::mpsc::{self, RecvTimeoutError};
    use std::time::{Duration, Instant};

    // 独立读取线程可能阻塞在管道上；接收端负责定时刷新未满的批次。
    // 有界队列限制大量输出时的额外内存占用。
    let (sender, receiver) = mpsc::sync_channel(32);
    std::thread::spawn(move || {
        let mut reader = BufReader::new(stream);
        let mut line = Vec::new();
        let mut truncated = false;
        while let Ok(bytes) = reader.fill_buf() {
            let eof = bytes.is_empty();
            let newline = bytes.iter().position(|byte| *byte == b'\n');
            let len = newline.unwrap_or(bytes.len());
            let keep = len.min(MAX_LOG_LINE_BYTES.saturating_sub(line.len()));
            line.extend_from_slice(&bytes[..keep]);
            truncated |= keep < len;
            reader.consume(len + usize::from(newline.is_some()));
            if newline.is_some() || (eof && !line.is_empty()) {
                if !truncated && newline.is_some() && line.last() == Some(&b'\r') {
                    line.pop();
                }
                // A UTF-8 codepoint may straddle the byte limit.
                if truncated {
                    if let Err(error) = std::str::from_utf8(&line) {
                        if error.error_len().is_none() {
                            line.truncate(error.valid_up_to());
                        }
                    }
                }
                let mut text = match String::from_utf8(std::mem::take(&mut line)) {
                    Ok(text) => text,
                    Err(_) => break,
                };
                if truncated {
                    text.push_str(TRUNCATED_LOG);
                }
                if sender.send(text).is_err() {
                    break;
                }
                truncated = false;
            }
            if eof {
                break;
            }
        }
    });
    let interval = Duration::from_millis(100);
    let mut deadline = Instant::now() + interval;
    let mut batch = Vec::new();
    loop {
        match receiver.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            Ok(text) => batch.push(text),
            Err(RecvTimeoutError::Timeout) => {
                if !batch.is_empty() {
                    emit(std::mem::take(&mut batch));
                }
                deadline = Instant::now() + interval;
            }
            Err(RecvTimeoutError::Disconnected) => break,
        }
        if batch.len() >= 20 {
            emit(std::mem::take(&mut batch));
            deadline = Instant::now() + interval;
        }
    }
    if !batch.is_empty() {
        emit(batch);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Duration;

    #[test]
    #[ignore = "local log memory benchmark"]
    fn log_cache_memory_profile() {
        let pid = sysinfo::get_current_pid().unwrap();
        let mut system = sysinfo::System::new();
        let mut memory = || {
            system.refresh_processes(sysinfo::ProcessesToUpdate::Some(&[pid]), true);
            system.process(pid).unwrap().memory()
        };
        let before = memory();
        let sessions: Vec<_> = (0..3).map(|n| Session::new(&n.to_string(), 5000)).collect();
        let payload = "x".repeat(8192);
        for session in &sessions {
            for n in 0..5000 {
                session.push_log("stdout", &format!("{n:08}{payload}"));
            }
        }
        let retained: usize = sessions
            .iter()
            .map(|s| {
                s.logs
                    .lock()
                    .unwrap()
                    .entries
                    .iter()
                    .map(|e| e.text.len())
                    .sum::<usize>()
            })
            .sum();
        println!(
            "MEMORY_PROFILE before={} loaded={} retained_text={}",
            before,
            memory(),
            retained
        );
        for session in &sessions {
            session.clear_logs();
        }
        println!("MEMORY_PROFILE cleared={}", memory());
    }

    #[test]
    fn log_cache_bounds_bytes_preserves_tail_and_releases_clear_capacity() {
        let session = Session::new("large", 5000);
        let text = "中".repeat(3000);
        for n in 0..5000 {
            session.push_log("stdout", &format!("{n}:{text}"));
        }
        let logs = session.logs.lock().unwrap();
        assert!(logs.bytes <= MAX_LOG_BYTES);
        assert!(logs.entries.len() < 5000);
        assert!(logs.entries.back().unwrap().text.starts_with("4999:"));
        assert_eq!(
            logs.bytes,
            logs.entries
                .iter()
                .map(LogEntry::retained_bytes)
                .sum::<usize>()
        );
        drop(logs);
        session.clear_logs();
        let logs = session.logs.lock().unwrap();
        assert_eq!(logs.bytes, 0);
        assert_eq!(logs.entries.capacity(), 0);
    }

    #[test]
    fn oversized_line_is_bounded_utf8_safe_and_following_lines_survive() {
        let input = format!("{}\r\nnext\r\n\nlast", "中".repeat(MAX_LOG_LINE_BYTES));
        let mut lines = Vec::new();
        read_log_batches(std::io::Cursor::new(input.into_bytes()), |batch| {
            lines.extend(batch)
        });
        assert_eq!(lines.len(), 4);
        assert!(lines[0].ends_with(TRUNCATED_LOG));
        assert!(!lines[0].contains('\u{fffd}'));
        assert!(lines[0].len() <= MAX_LOG_LINE_BYTES + TRUNCATED_LOG.len());
        assert_eq!(&lines[1..], &["next", "", "last"]);
    }

    struct LiveOutput {
        first: std::io::Cursor<Vec<u8>>,
        close: mpsc::Receiver<()>,
    }
    impl std::io::Read for LiveOutput {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            let n = std::io::Read::read(&mut self.first, buf)?;
            if n == 0 {
                let _ = self.close.recv();
            }
            Ok(n)
        }
    }

    #[cfg(windows)]
    #[test]
    fn batch_wrapper_in_directory_with_spaces_preserves_arguments() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("path with spaces");
        std::fs::create_dir_all(&dir).unwrap();
        let script = dir.join("echo tool.cmd");
        std::fs::write(&script, "@echo off\r\necho [%~1] [%~2]\r\n").unwrap();
        let cwd = dir.to_string_lossy();
        let command = script.to_string_lossy();
        let args = vec!["hello world".into(), "second".into()];
        let env = Default::default();
        let spec = StartSpec {
            service_id: "test",
            cwd: &cwd,
            command: &command,
            args: &args,
            shell_mode: false,
            env: &env,
            cap: 500,
            startup_delay: 0,
            prepare: &[],
        };
        let out = build_spawn_command(&spec, &resolve_program(&spec))
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&out.stdout).trim(),
            "[hello world] [second]"
        );
    }

    #[cfg(windows)]
    fn step(cwd: &str, command: &str) -> crate::launch::LaunchCommand {
        crate::launch::LaunchCommand {
            cwd: cwd.into(),
            command: "cmd.exe".into(),
            args: vec!["/D".into(), "/C".into(), command.into()],
            shell_mode: false,
        }
    }

    #[cfg(windows)]
    #[test]
    fn preparation_finishes_before_main_process_and_failure_aborts_launch() {
        let temp = tempfile::tempdir().unwrap();
        let cwd = temp.path().to_string_lossy();
        let session = Arc::new(Session::new("test", 500));
        run_launch_steps(
            session.clone(),
            vec![
                step(&cwd, "echo prepared>ready.txt"),
                step(
                    &cwd,
                    "if not exist ready.txt (exit /b 7) else (echo launched>started.txt)",
                ),
            ],
            Default::default(),
            Arc::new(|_| {}),
            Arc::new(|_| {}),
        );
        assert_eq!(session.status(), "exited");
        assert!(temp.path().join("started.txt").is_file());
        assert_eq!(session.pid(), None);
        let failed = Arc::new(Session::new("failed", 500));
        run_launch_steps(
            failed.clone(),
            vec![
                step(&cwd, "exit /b 9"),
                step(&cwd, "echo wrong>unexpected.txt"),
            ],
            Default::default(),
            Arc::new(|_| {}),
            Arc::new(|_| {}),
        );
        assert_eq!(failed.status(), "failed");
        assert!(!temp.path().join("unexpected.txt").exists());
        assert!(failed.run_state().error.unwrap().contains("前置构建失败"));
    }

    #[cfg(windows)]
    #[test]
    fn cancellation_prevents_next_launch_step() {
        let temp = tempfile::tempdir().unwrap();
        let cwd = temp.path().to_string_lossy();
        let session = Arc::new(Session::new("test", 500));
        session.mark_stopping();
        run_launch_steps(
            session.clone(),
            vec![step(&cwd, "echo wrong>unexpected.txt")],
            Default::default(),
            Arc::new(|_| {}),
            Arc::new(|_| {}),
        );
        assert_eq!(session.status(), "stopped");
        assert!(!temp.path().join("unexpected.txt").exists());
    }

    /// Explicit opt-in: exercises the real project via the same process supervisor as IPC.
    #[cfg(windows)]
    #[test]
    #[ignore = "requires XCHEN_TEST_WORKSPACE and local project dependencies"]
    fn local_project_launch_http_smoke() {
        use std::io::{Read, Write};
        use std::net::{TcpListener, TcpStream};
        use std::time::Instant;
        let root = std::env::var("XCHEN_TEST_WORKSPACE").expect("set XCHEN_TEST_WORKSPACE");
        let mode = std::env::var("XCHEN_TEST_KIND").unwrap_or_else(|_| "web".into());
        let backend = mode.starts_with("backend");
        let dir = Path::new(&root).join(if backend {
            "xingyu-server/xingyu-starter"
        } else if mode == "admin" {
            "xingyu-admin"
        } else {
            "xingyu-web"
        });
        let port = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let mut plans = if backend {
            crate::launch::java_options(&dir)
        } else {
            crate::launch::node_options(&dir)
        };
        let mut plan = plans.remove(if mode == "backend-fast" { 1 } else { 0 });
        let mut env = std::collections::BTreeMap::new();
        if backend {
            env.insert("SERVER_PORT".into(), port.to_string());
        } else {
            plan.launch.args.extend([
                "--".into(),
                "--host".into(),
                "127.0.0.1".into(),
                "--port".into(),
                port.to_string(),
                "--strictPort".into(),
            ]);
        }
        let mut steps = plan.prepare;
        steps.push(plan.launch);
        let session = Arc::new(Session::new("local-smoke", 5000));
        session.set_status("starting");
        let worker_session = session.clone();
        let worker = std::thread::spawn(move || {
            run_launch_steps(
                worker_session,
                steps,
                env,
                Arc::new(|_| {}),
                Arc::new(|_| {}),
            )
        });
        let deadline = Instant::now() + Duration::from_secs(if backend { 240 } else { 45 });
        let mut response = String::new();
        while Instant::now() < deadline
            && !["failed", "exited"].contains(&session.status().as_str())
        {
            if let Ok(mut stream) = TcpStream::connect_timeout(
                &format!("127.0.0.1:{port}").parse().unwrap(),
                Duration::from_millis(200),
            ) {
                stream
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                stream
                    .write_all(b"GET / HTTP/1.0\r\nHost: localhost\r\nConnection: close\r\n\r\n")
                    .unwrap();
                let _ = stream.read_to_string(&mut response);
                if response.starts_with("HTTP/") {
                    break;
                }
            }
            std::thread::sleep(Duration::from_millis(250));
        }
        // Kill only the root PID created by this test, including its child process tree.
        {
            let _guard = session.control.lock().unwrap();
            session.mark_stopping();
            if let Some(pid) = session.pid() {
                let _ = crate::commands::port::taskkill(pid, true);
            }
        }
        worker.join().unwrap();
        let logs = session
            .history(None)
            .iter()
            .map(|e| e.text.clone())
            .collect::<Vec<_>>()
            .join("\n");
        std::fs::write(
            Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("target/launch-{mode}-smoke.log")),
            &logs,
        )
        .unwrap();
        assert!(
            response.starts_with("HTTP/"),
            "No HTTP response. Last logs: {}",
            logs.chars()
                .rev()
                .take(6000)
                .collect::<String>()
                .chars()
                .rev()
                .collect::<String>()
        );
        assert_eq!(
            response.lines().next().unwrap().split_whitespace().nth(1),
            Some("200")
        );
        println!(
            "{mode}: port={port}, {}, final={}, pid={:?}",
            response.lines().next().unwrap(),
            session.status(),
            session.pid()
        );
        assert_eq!(session.status(), "stopped");
        assert_eq!(session.pid(), None);
    }

    #[test]
    fn workspace_launch_cancellation_is_scoped_and_blocks_duplicate_runs() {
        let runtime = RuntimeManager::new();
        let one = runtime.begin_workspace_launch("one").unwrap();
        let two = runtime.begin_workspace_launch("two").unwrap();
        assert!(runtime.begin_workspace_launch("one").is_err());
        runtime.cancel_workspace_launch("one");
        assert!(one.is_cancelled());
        assert!(!two.is_cancelled());
        runtime.cancel_all_workspace_launches();
        assert!(two.is_cancelled());
        drop(one);
        assert!(runtime.begin_workspace_launch("one").is_ok());
    }

    #[test]
    fn colored_sql_is_plain_in_live_entries_history_and_serialization() {
        let session = Session::new("sql", 100);
        let sql = "SQL: \u{1b}[94m\u{1b}[96mSELECT\u{1b}[0m\u{1b}[94m * \u{1b}[96mFROM\u{1b}[0m\u{1b}[94m reliable_event WHERE status IN (\u{1b}[32m'PENDING'\u{1b}[0m, 'FAILED') AND attempt_count < max_attempts\u{1b}[0m";
        let plain = "SQL: SELECT * FROM reliable_event WHERE status IN ('PENDING', 'FAILED') AND attempt_count < max_attempts";
        let input =
            format!("\u{1b}[92m ✅ 正常执行 │ 耗时: 3ms\u{1b}[0m\r\n{sql}\r\n\u{1b}[0m\r\n");
        let mut live = Vec::new();
        read_log_batches(std::io::Cursor::new(input.into_bytes()), |batch| {
            live.extend(
                batch
                    .iter()
                    .filter_map(|line| session.push_log("stdout", line)),
            );
        });
        assert_eq!(live.len(), 2);
        assert_eq!(live[0].text, " ✅ 正常执行 │ 耗时: 3ms");
        assert_eq!(live[1].text, plain);
        let history = session.history(None);
        assert_eq!(history[1].text, live[1].text);
        let json = serde_json::to_string(&history).unwrap();
        assert!(!json.contains("\\u001b"));
        assert!(json.contains(plain));
    }

    #[test]
    fn terminal_metadata_is_removed_without_changing_log_content() {
        let session = Session::new("stderr", 100);
        let entry = session
            .push_log("stderr", "\u{1b}]0;Build window\u{7}  \t\u{1b}[31m错误: \u{1b}]8;;https://example.test\u{1b}\\详细信息\u{1b}]8;;\u{1b}\\ <tag> &amp; [FAILED]\u{1b}[0m\u{1b}[K")
            .unwrap();
        assert_eq!(entry.stream, "stderr");
        assert_eq!(entry.text, "  \t错误: 详细信息 <tag> &amp; [FAILED]");
        let ordinary = "  SQL: SELECT * FROM t WHERE value < 20 AND name = '中文'";
        assert_eq!(session.push_log("stdout", ordinary).unwrap().text, ordinary);
        assert_eq!(session.history(Some(1))[0].text, ordinary);
    }

    #[test]
    fn sparse_logs_are_delivered_before_process_exits() {
        let (close, wait) = mpsc::channel();
        let (sent, received) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            read_log_batches(
                LiveOutput {
                    first: std::io::Cursor::new(b"server ready\n".to_vec()),
                    close: wait,
                },
                |batch| {
                    let _ = sent.send(batch);
                },
            )
        });
        let batch = received.recv_timeout(Duration::from_secs(2));
        close.send(()).unwrap();
        worker.join().unwrap();
        assert_eq!(batch.unwrap(), vec!["server ready"]);
    }

    #[test]
    fn log_batches_preserve_order_and_flush_at_eof() {
        let text = (0..45).map(|i| format!("line {i}\n")).collect::<String>();
        let mut batches = Vec::new();
        read_log_batches(std::io::Cursor::new(text.into_bytes()), |batch| {
            batches.push(batch)
        });
        assert_eq!(
            batches.iter().map(Vec::len).collect::<Vec<_>>(),
            vec![20, 20, 5]
        );
        assert_eq!(
            batches.into_iter().flatten().collect::<Vec<_>>(),
            (0..45).map(|i| format!("line {i}")).collect::<Vec<_>>()
        );
    }
}
