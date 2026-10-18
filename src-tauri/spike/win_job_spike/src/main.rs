//! F003 spike: Windows Job Object coverage for cmd.exe launch chains.
//! Run: cargo run --manifest-path src-tauri/spike/win_job_spike/Cargo.toml

use std::ffi::c_void;
use std::mem::size_of;
use std::process::Command as OsCommand;
use std::thread;
use std::time::Duration;

use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, IsProcessInJob, JobObjectExtendedLimitInformation,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_BREAKAWAY_OK,
    JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, SetInformationJobObject,
};
use windows::Win32::System::Threading::{
    CreateProcessW, OpenProcess, PROCESS_INFORMATION, PROCESS_QUERY_INFORMATION, ResumeThread,
    STARTUPINFOW, CREATE_NO_WINDOW, CREATE_SUSPENDED,
};

fn main() {
    println!("=== F003 Windows Job Object Spike ===");
    println!("host={}", std::env::var("COMPUTERNAME").unwrap_or_default());
    println!("java_on_path={}", java_on_path());
    println!();

    for job_flags in [JobFlags::KillOnCloseOnly, JobFlags::KillOnCloseWithBreakaway] {
        println!("################################################################");
        println!("# Job flags: {}", job_flags.label());
        println!("################################################################\n");
        run_suite(job_flags);
        println!();
    }
}

#[derive(Clone, Copy)]
enum JobFlags {
    KillOnCloseOnly,
    KillOnCloseWithBreakaway,
}

impl JobFlags {
    fn label(self) -> &'static str {
        match self {
            JobFlags::KillOnCloseOnly => "KILL_ON_JOB_CLOSE only",
            JobFlags::KillOnCloseWithBreakaway => "KILL_ON_JOB_CLOSE | BREAKAWAY_OK",
        }
    }

    fn limit_flags(self) -> windows::Win32::System::JobObjects::JOB_OBJECT_LIMIT {
        match self {
            JobFlags::KillOnCloseOnly => JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
            JobFlags::KillOnCloseWithBreakaway => {
                JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE | JOB_OBJECT_LIMIT_BREAKAWAY_OK
            }
        }
    }
}

fn run_suite(job_flags: JobFlags) {
    let scenarios = [
        ("spawn_then_assign", SpawnMode::SpawnThenAssign),
        ("create_suspended_assign_resume", SpawnMode::CreateSuspendedAssignResume),
        ("spawn_delay_5ms_assign", SpawnMode::SpawnDelayAssign(5)),
        ("spawn_delay_50ms_assign", SpawnMode::SpawnDelayAssign(50)),
    ];

    let commands = [
        ("cmd_node_interval", "cmd /C node -e \"setInterval(()=>{}, 1e6)\""),
        ("cmd_npm_version", "cmd /C npm --version"),
        ("cmd_ping", "cmd /C ping -n 30 127.0.0.1"),
    ];

    for (mode_name, mode) in scenarios {
        println!("--- Mode: {} ---", mode_name);
        for (label, cmdline) in commands {
            match run_scenario(job_flags, mode, cmdline) {
                Ok(report) => print_report(label, &report),
                Err(e) => println!("[{}] ERR: {}", label, e),
            }
        }
        println!();
    }

    println!("--- Long-running npm chain (CREATE_SUSPENDED) ---");
    match run_scenario(
        job_flags,
        SpawnMode::CreateSuspendedAssignResume,
        "cmd /C npm exec --yes -- node -e \"setInterval(()=>{}, 1e6)\"",
    ) {
        Ok(r) => print_report("npm_exec_node", &r),
        Err(e) => println!("[npm_exec_node] ERR: {}", e),
    }

    println!();
    if java_on_path() {
        let long_cmd = match java_long_cmd() {
            Ok(cmd) => cmd,
            Err(e) => {
                println!("--- Java compile failed: {} ---", e);
                String::new()
            }
        };
        println!("--- Java: cmd /C java -version (short) ---");
        match run_scenario(
            job_flags,
            SpawnMode::CreateSuspendedAssignResume,
            "cmd /C java -version",
        ) {
            Ok(r) => print_report("cmd_java_version", &r),
            Err(e) => println!("[cmd_java_version] ERR: {}", e),
        }
        println!();
        if long_cmd.is_empty() {
            println!("--- Java: long-running SpikeSleep SKIPPED ---");
        } else {
            println!("--- Java: long-running SpikeSleep (CREATE_SUSPENDED) ---");
            match run_scenario_with_settle(
                job_flags,
                SpawnMode::CreateSuspendedAssignResume,
                &long_cmd,
                3000,
            ) {
                Ok(r) => print_report("cmd_java_sleep", &r),
                Err(e) => println!("[cmd_java_sleep] ERR: {}", e),
            }
        }
    } else {
        println!("--- Java: NOT ON PATH — long-running coverage UNVERIFIED ---");
    }

    println!();
    println!("--- Isolation: unrelated ping survives job close ---");
    match isolation_test(job_flags) {
        Ok(r) => println!(
            "unrelated_ping_survived={} job_root_killed={}",
            r.unrelated_survived, r.job_killed
        ),
        Err(e) => println!("isolation ERR: {}", e),
    }
}

#[derive(Clone, Copy)]
enum SpawnMode {
    SpawnThenAssign,
    CreateSuspendedAssignResume,
    SpawnDelayAssign(u64),
}

struct ScenarioReport {
    root_pid: u32,
    assign_ok: bool,
    assign_err: Option<String>,
    root_in_job: bool,
    descendant_pids: Vec<u32>,
    descendants_in_job: Vec<bool>,
    escaped_before_assign: bool,
    terminate_job_killed_all: bool,
}

struct IsolationReport {
    unrelated_survived: bool,
    job_killed: bool,
}

fn print_report(label: &str, r: &ScenarioReport) {
    let in_job = r.descendants_in_job.iter().filter(|b| **b).count();
    let total = r.descendants_in_job.len();
    println!(
        "[{}] root_pid={} assign_ok={} assign_err={} root_in_job={} descendants={}/{} in_job escaped_before_assign={} terminate_ok={}",
        label,
        r.root_pid,
        r.assign_ok,
        r.assign_err.as_deref().unwrap_or("-"),
        r.root_in_job,
        in_job,
        total,
        r.escaped_before_assign,
        r.terminate_job_killed_all
    );
    if !r.descendant_pids.is_empty() {
        println!(
            "  descendant_pids={:?} in_job={:?}",
            r.descendant_pids,
            r.descendants_in_job
        );
    }
}

fn run_scenario(
    job_flags: JobFlags,
    mode: SpawnMode,
    cmdline: &str,
) -> Result<ScenarioReport, String> {
    run_scenario_with_settle(job_flags, mode, cmdline, 800)
}

fn run_scenario_with_settle(
    job_flags: JobFlags,
    mode: SpawnMode,
    cmdline: &str,
    settle_ms: u64,
) -> Result<ScenarioReport, String> {
    let job = create_session_job(job_flags)?;
    let (root_pid, root_handle, thread_handle) = spawn_cmd(mode, cmdline)?;

    let pre_assign_descendants = if matches!(mode, SpawnMode::SpawnDelayAssign(_)) {
        Vec::new()
    } else if matches!(mode, SpawnMode::SpawnThenAssign) {
        thread::sleep(Duration::from_micros(100));
        collect_descendants(root_pid)
    } else {
        Vec::new()
    };

    if let SpawnMode::SpawnDelayAssign(ms) = mode {
        thread::sleep(Duration::from_millis(ms));
    }

    let assign_result = assign_to_job(job, root_handle);
    let assign_ok = assign_result.is_ok();
    let assign_err = assign_result.err();

    if let Some(th) = thread_handle {
        unsafe {
            let _ = ResumeThread(th);
            let _ = CloseHandle(th);
        }
    }
    unsafe {
        let _ = CloseHandle(root_handle);
    }

    thread::sleep(Duration::from_millis(settle_ms));

    let root_in_job = is_pid_in_job(root_pid, job);
    let descendant_pids = collect_descendants(root_pid);
    let descendants_in_job = descendant_pids
        .iter()
        .map(|pid| is_pid_in_job(*pid, job))
        .collect::<Vec<_>>();

    let escaped_before_assign = matches!(mode, SpawnMode::SpawnDelayAssign(_))
        && descendants_in_job.iter().any(|b| !*b)
        && !descendant_pids.is_empty();

    let pre_assign_escape = !pre_assign_descendants.is_empty()
        && pre_assign_descendants
            .iter()
            .any(|pid| !is_pid_in_job(*pid, job));

    let terminate_job_killed_all = terminate_job_and_verify(job, root_pid, &descendant_pids);

    Ok(ScenarioReport {
        root_pid,
        assign_ok,
        assign_err,
        root_in_job,
        descendant_pids,
        descendants_in_job,
        escaped_before_assign: escaped_before_assign || pre_assign_escape,
        terminate_job_killed_all,
    })
}

fn create_session_job(job_flags: JobFlags) -> Result<HANDLE, String> {
    let job = unsafe { CreateJobObjectW(None, None) }
        .map_err(|e| format!("CreateJobObjectW: {}", e))?;
    let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
    info.BasicLimitInformation.LimitFlags = job_flags.limit_flags();
    unsafe {
        SetInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            &info as *const _ as *const c_void,
            size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        )
    }
    .map_err(|e| format!("SetInformationJobObject: {}", e))?;
    Ok(job)
}

fn spawn_cmd(mode: SpawnMode, cmdline: &str) -> Result<(u32, HANDLE, Option<HANDLE>), String> {
    use std::os::windows::ffi::OsStrExt;
    let wide: Vec<u16> = std::ffi::OsStr::new(cmdline)
        .encode_wide()
        .chain([0])
        .collect();
    let mut si = STARTUPINFOW::default();
    si.cb = size_of::<STARTUPINFOW>() as u32;
    let mut pi = PROCESS_INFORMATION::default();
    let flags = match mode {
        SpawnMode::SpawnThenAssign | SpawnMode::SpawnDelayAssign(_) => CREATE_NO_WINDOW,
        SpawnMode::CreateSuspendedAssignResume => CREATE_SUSPENDED | CREATE_NO_WINDOW,
    };
    unsafe {
        CreateProcessW(
            None,
            Some(windows::core::PWSTR(wide.as_ptr() as *mut _)),
            None,
            None,
            false,
            flags,
            None,
            None,
            &mut si,
            &mut pi,
        )
    }
    .map_err(|e| format!("CreateProcessW: {}", e))?;

    let pid = pi.dwProcessId;
    let thread = match mode {
        SpawnMode::CreateSuspendedAssignResume => Some(pi.hThread),
        SpawnMode::SpawnThenAssign | SpawnMode::SpawnDelayAssign(_) => {
            unsafe {
                let _ = CloseHandle(pi.hThread);
            }
            None
        }
    };
    Ok((pid, pi.hProcess, thread))
}

fn assign_to_job(job: HANDLE, process: HANDLE) -> Result<(), String> {
    unsafe { AssignProcessToJobObject(job, process) }
        .map_err(|e| format!("AssignProcessToJobObject: {}", e))
}

fn is_pid_in_job(pid: u32, job: HANDLE) -> bool {
    let process = match unsafe { OpenProcess(PROCESS_QUERY_INFORMATION, false, pid) } {
        Ok(h) => h,
        Err(_) => return false,
    };
    let mut in_job = false.into();
    let ok = unsafe { IsProcessInJob(process, Some(job), &mut in_job) }.is_ok();
    unsafe {
        let _ = CloseHandle(process);
    }
    ok && in_job.as_bool()
}

fn collect_descendants(root_pid: u32) -> Vec<u32> {
    let mut system = System::new();
    system.refresh_processes_specifics(
        ProcessesToUpdate::All,
        true,
        ProcessRefreshKind::everything(),
    );
    let mut frontier = vec![root_pid];
    let mut seen = vec![root_pid];
    let mut descendants = Vec::new();
    for _ in 0..4 {
        let mut next = Vec::new();
        for parent in &frontier {
            for (pid, p) in system.processes() {
                let child = pid.as_u32();
                if p.parent().map(|p| p.as_u32()) == Some(*parent) && !seen.contains(&child) {
                    next.push(child);
                    descendants.push(child);
                    seen.push(child);
                }
            }
        }
        if next.is_empty() {
            break;
        }
        frontier = next;
    }
    descendants
}

fn terminate_job_and_verify(job: HANDLE, root_pid: u32, descendants: &[u32]) -> bool {
    unsafe {
        let _ = CloseHandle(job);
    }
    thread::sleep(Duration::from_millis(500));
    !is_pid_alive(root_pid) && descendants.iter().all(|pid| !is_pid_alive(*pid))
}

fn is_pid_alive(pid: u32) -> bool {
    let mut system = System::new();
    system.refresh_processes_specifics(
        ProcessesToUpdate::Some(&[Pid::from_u32(pid)]),
        true,
        ProcessRefreshKind::everything(),
    );
    system.process(Pid::from_u32(pid)).is_some()
}

fn java_on_path() -> bool {
    OsCommand::new("where")
        .arg("java")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn write_java_sleep_source() -> Result<std::path::PathBuf, String> {
    use std::io::Write;
    let path = std::env::temp_dir().join("SpikeSleep.java");
    let mut f = std::fs::File::create(&path).map_err(|e| e.to_string())?;
    write!(
        f,
        "public class SpikeSleep {{ public static void main(String[] a) throws Exception {{ Thread.sleep(120000); }} }}\n"
    )
    .map_err(|e| e.to_string())?;
    let status = OsCommand::new("javac")
        .arg(&path)
        .status()
        .map_err(|e| e.to_string())?;
    if !status.success() {
        return Err("javac SpikeSleep.java failed".to_string());
    }
    Ok(path)
}

fn java_long_cmd() -> Result<String, String> {
    write_java_sleep_source()?;
    let temp = std::env::temp_dir();
    Ok(format!(
        "cmd /C java -cp \"{}\" SpikeSleep",
        temp.display()
    ))
}

fn isolation_test(job_flags: JobFlags) -> Result<IsolationReport, String> {
    let job = create_session_job(job_flags)?;
    let (root_pid, root_handle, thread_handle) =
        spawn_cmd(SpawnMode::CreateSuspendedAssignResume, r"cmd /C ping -n 60 127.0.0.1")?;
    assign_to_job(job, root_handle)?;
    if let Some(th) = thread_handle {
        unsafe {
            let _ = ResumeThread(th);
            let _ = CloseHandle(th);
        }
    }
    unsafe {
        let _ = CloseHandle(root_handle);
    }

    let (unrelated_pid, unrelated_handle, unrelated_thread) =
        spawn_cmd(SpawnMode::SpawnThenAssign, r"cmd /C ping -n 60 127.0.0.1")?;
    if let Some(th) = unrelated_thread {
        unsafe {
            let _ = CloseHandle(th);
        }
    }
    unsafe {
        let _ = CloseHandle(unrelated_handle);
    }
    thread::sleep(Duration::from_millis(300));

    unsafe {
        let _ = CloseHandle(job);
    }
    thread::sleep(Duration::from_millis(500));

    Ok(IsolationReport {
        unrelated_survived: is_pid_alive(unrelated_pid),
        job_killed: !is_pid_alive(root_pid),
    })
}
