//! Bounded file-resource queries through Windows Restart Manager. This is not
//! system-wide handle enumeration: directory handles/current directories are not covered.
use serde_json::{json, Value};
use std::path::PathBuf;
use std::time::{Duration, Instant};

const MAX_FILES: usize = 20_000;
const MAX_PATH_UNITS: usize = 2_000_000;

struct Resources {
    files: Vec<PathBuf>,
    skipped: usize,
    limited: bool,
}

fn collect(path: &str) -> Result<Resources, String> {
    collect_limited(path, MAX_FILES)
}

fn collect_limited(path: &str, max_files: usize) -> Result<Resources, String> {
    let root = std::fs::canonicalize(path).map_err(|e| format!("无法访问路径: {e}"))?;
    let mut result = Resources {
        files: Vec::new(),
        skipped: 0,
        limited: false,
    };
    let mut units = root.as_os_str().len();
    let mut stack = vec![root];
    let start = Instant::now();
    while let Some(path) = stack.pop() {
        if result.files.len() >= max_files || start.elapsed() > Duration::from_secs(5) {
            result.limited = true;
            break;
        }
        let metadata = match std::fs::symlink_metadata(&path) {
            Ok(value) => value,
            Err(_) => {
                result.skipped += 1;
                continue;
            }
        };
        // Do not follow junctions or symlinks outside the selected tree or into cycles.
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0 {
            result.skipped += 1;
            continue;
        }
        if metadata.is_file() {
            result.files.push(path);
        } else if metadata.is_dir() {
            match std::fs::read_dir(&path) {
                Ok(entries) => {
                    for entry in entries {
                        match entry {
                            Ok(entry) if stack.len() + result.files.len() < max_files => {
                                let child = entry.path();
                                let length = child.as_os_str().len();
                                if units + length > MAX_PATH_UNITS {
                                    result.limited = true;
                                    break;
                                }
                                units += length;
                                stack.push(child)
                            }
                            Ok(_) => {
                                result.limited = true;
                                break;
                            }
                            Err(_) => result.skipped += 1,
                        }
                        if start.elapsed() > Duration::from_secs(5) {
                            result.limited = true;
                            break;
                        }
                    }
                }
                Err(_) => result.skipped += 1,
            }
        }
    }
    Ok(result)
}

#[repr(C)]
#[derive(Clone, Copy, Default, PartialEq, Eq)]
struct FileTime {
    low: u32,
    high: u32,
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct UniqueProcess {
    pid: u32,
    started: FileTime,
}
#[repr(C)]
#[derive(Clone, Copy)]
struct ProcessInfo {
    process: UniqueProcess,
    name: [u16; 256],
    service: [u16; 64],
    kind: u32,
    status: u32,
    session: u32,
    restartable: i32,
}

#[link(name = "Rstrtmgr")]
unsafe extern "system" {
    fn RmStartSession(session: *mut u32, flags: u32, key: *mut u16) -> u32;
    fn RmEndSession(session: u32) -> u32;
    fn RmRegisterResources(
        session: u32,
        count: u32,
        files: *const *const u16,
        apps: u32,
        processes: *const UniqueProcess,
        services: u32,
        names: *const *const u16,
    ) -> u32;
    fn RmGetList(
        session: u32,
        needed: *mut u32,
        count: *mut u32,
        processes: *mut ProcessInfo,
        reasons: *mut u32,
    ) -> u32;
}
#[link(name = "kernel32")]
unsafe extern "system" {
    fn OpenProcess(access: u32, inherit: i32, pid: u32) -> *mut std::ffi::c_void;
    fn GetProcessTimes(
        handle: *mut std::ffi::c_void,
        created: *mut FileTime,
        exited: *mut FileTime,
        kernel: *mut FileTime,
        user: *mut FileTime,
    ) -> i32;
    fn TerminateProcess(handle: *mut std::ffi::c_void, code: u32) -> i32;
    fn CloseHandle(handle: *mut std::ffi::c_void) -> i32;
}
struct Session(u32);
impl Drop for Session {
    fn drop(&mut self) {
        // SAFETY: this object exclusively owns a successfully started RM session.
        unsafe {
            RmEndSession(self.0);
        }
    }
}
fn win_error(code: u32) -> String {
    format!(
        "Windows 占用查询失败: {} ({code})",
        std::io::Error::from_raw_os_error(code as i32)
    )
}

fn owners(files: &[PathBuf]) -> Result<Vec<ProcessInfo>, String> {
    if files.is_empty() {
        return Ok(Vec::new());
    }
    let mut session = 0;
    let mut key = [0u16; 33];
    // SAFETY: output pointers reference writable storage of the SDK-specified sizes.
    let code = unsafe { RmStartSession(&mut session, 0, key.as_mut_ptr()) };
    if code != 0 {
        return Err(win_error(code));
    }
    let session = Session(session);
    use std::os::windows::ffi::OsStrExt;
    for chunk in files.chunks(512) {
        let names: Vec<Vec<u16>> = chunk
            .iter()
            .map(|p| {
                // canonicalize produces extended DOS paths; RM expects absolute Win32 paths.
                let text = p.to_string_lossy();
                let normal = if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
                    format!(r"\\{rest}")
                } else {
                    text.strip_prefix(r"\\?\").unwrap_or(&text).to_owned()
                };
                std::ffi::OsStr::new(&normal)
                    .encode_wide()
                    .chain(Some(0))
                    .collect()
            })
            .collect();
        let pointers: Vec<*const u16> = names.iter().map(|name| name.as_ptr()).collect();
        // SAFETY: all strings are null-terminated and stay alive for this call;
        // the unused resource categories have zero counts and null pointers.
        let code = unsafe {
            RmRegisterResources(
                session.0,
                pointers.len() as u32,
                pointers.as_ptr(),
                0,
                std::ptr::null(),
                0,
                std::ptr::null(),
            )
        };
        if code != 0 {
            return Err(win_error(code));
        }
    }
    let mut needed = 0;
    let mut count = 0;
    let mut reasons = 0;
    // SAFETY: the first call requests the required array length with an empty buffer.
    let mut code = unsafe {
        RmGetList(
            session.0,
            &mut needed,
            &mut count,
            std::ptr::null_mut(),
            &mut reasons,
        )
    };
    for _ in 0..5 {
        if code == 0 && needed == 0 {
            return Ok(Vec::new());
        }
        if code != 234 {
            return Err(win_error(code));
        }
        if needed > 65_536 {
            return Err("占用进程过多，请缩小查询范围".into());
        }
        // SAFETY: ProcessInfo is a C POD containing only integer fields/arrays.
        let mut entries = vec![unsafe { std::mem::zeroed::<ProcessInfo>() }; needed as usize];
        count = entries.len() as u32;
        // SAFETY: capacity/count match; all output pointers remain valid during the call.
        code = unsafe {
            RmGetList(
                session.0,
                &mut needed,
                &mut count,
                entries.as_mut_ptr(),
                &mut reasons,
            )
        };
        if code == 0 {
            entries.truncate(count as usize);
            return Ok(entries);
        }
    }
    Err("占用进程持续变化，请重试".into())
}

fn display_name(info: &ProcessInfo) -> String {
    String::from_utf16_lossy(
        &info.name[..info
            .name
            .iter()
            .position(|c| *c == 0)
            .unwrap_or(info.name.len())],
    )
}
fn scan(path: &str) -> Result<(Resources, Vec<ProcessInfo>), String> {
    let resources = collect(path)?;
    let entries = owners_with_retry(&resources.files)?;
    Ok((resources, entries))
}

fn owners_with_retry(files: &[PathBuf]) -> Result<Vec<ProcessInfo>, String> {
    let mut last_error = None;
    for attempt in 0..2 {
        match owners(files) {
            Ok(entries) => return Ok(entries),
            Err(error) => {
                let transient = error.ends_with("(5)") || error.ends_with("(121)");
                if !transient || attempt == 1 {
                    if transient && files.len() > 1 {
                        let mut merged = Vec::new();
                        let mut successful_batch = false;
                        for chunk in files.chunks(128) {
                            if let Ok(entries) = owners_with_retry_once(chunk) {
                                successful_batch = true;
                                merged.extend(entries);
                            }
                        }
                        if successful_batch {
                            merged.sort_by_key(|entry| entry.process.pid);
                            merged.dedup_by_key(|entry| entry.process.pid);
                            return Ok(merged);
                        }
                    }
                    return Err(if error.ends_with("(5)") {
                        "Windows 占用查询被系统拒绝访问；请关闭正在扫描的临时程序后重试，或以管理员身份运行。".into()
                    } else {
                        error
                    });
                }
                last_error = Some(error);
                std::thread::sleep(Duration::from_millis(120));
            }
        }
    }
    Err(last_error.unwrap_or_else(|| "Windows 占用查询失败".into()))
}

fn owners_with_retry_once(files: &[PathBuf]) -> Result<Vec<ProcessInfo>, String> {
    for attempt in 0..2 {
        match owners(files) {
            Ok(entries) => return Ok(entries),
            Err(error) if (error.ends_with("(5)") || error.ends_with("(121)")) && attempt == 0 => {
                std::thread::sleep(Duration::from_millis(120));
            }
            Err(error) => return Err(error),
        }
    }
    unreachable!()
}
pub fn query(path: &str) -> Result<Value, String> {
    let start = Instant::now();
    let (resources, entries) = scan(path)?;
    let pids: Vec<_> = entries
        .iter()
        .map(|p| sysinfo::Pid::from_u32(p.process.pid))
        .collect();
    let mut system = sysinfo::System::new();
    if !pids.is_empty() {
        system.refresh_processes(sysinfo::ProcessesToUpdate::Some(&pids), true);
    }
    let occupations: Vec<Value> = entries
        .iter()
        .map(|info| {
            let process = system.process(sysinfo::Pid::from_u32(info.process.pid));
            json!({
                "pid": info.process.pid,
                "processName": process.map(|p| p.name().to_string_lossy().into_owned()).unwrap_or_else(|| display_name(info)),
                "processPath": process.and_then(|p| p.exe()).map(|p| p.to_string_lossy().into_owned()),
                // Compatibility fields for the recovered UI: RM reports process
                // associations, not individual handles. The UI labels these as records.
                "handleCount": 1, "handles": [],
            })
        })
        .collect();
    Ok(json!({ "occupations": occupations, "diagnostics": {
        "elapsedMs": start.elapsed().as_millis(), "filesScanned": resources.files.len(),
        "skipped": resources.skipped, "limited": resources.limited,
        "note": "查询文件资源占用；不含仅持有目录句柄或以该目录为工作目录的进程。"
    }}))
}

fn terminate(info: &ProcessInfo) -> Result<(), String> {
    if info.process.pid <= 4
        || info.process.pid == std::process::id()
        || info.kind == 1000
        || info.service[0] != 0
    {
        return Err("受保护的系统、服务或当前进程，拒绝终止".into());
    }
    // SAFETY: PID is from the fresh RM query; request only terminate/query rights.
    let handle = unsafe { OpenProcess(0x1001, 0, info.process.pid) };
    if handle.is_null() {
        return Err(std::io::Error::last_os_error().to_string());
    }
    let mut created = FileTime::default();
    let mut exited = FileTime::default();
    let mut kernel = FileTime::default();
    let mut user = FileTime::default();
    // SAFETY: handle remains open, output fields are valid. Compare creation time
    // before terminating through the SAME handle, so PID reuse cannot kill a replacement.
    let result = unsafe {
        if GetProcessTimes(handle, &mut created, &mut exited, &mut kernel, &mut user) == 0 {
            Err(std::io::Error::last_os_error().to_string())
        } else if created != info.process.started {
            Err("进程身份已变化，请重新查询".into())
        } else if TerminateProcess(handle, 1) == 0 {
            Err(std::io::Error::last_os_error().to_string())
        } else {
            Ok(())
        }
    };
    // SAFETY: this function owns the valid handle, and no code retains it.
    unsafe {
        CloseHandle(handle);
    }
    result
}
pub fn release(path: &str, pids: Option<Vec<u32>>) -> Result<Value, String> {
    let selected = pids
        .filter(|v| !v.is_empty())
        .ok_or("请选择需要释放的进程")?;
    let (_, entries) = scan(path)?;
    let selected: std::collections::BTreeSet<u32> = selected.into_iter().collect();
    Ok(Value::Array(selected.into_iter().map(|pid| {
        let result = entries.iter().find(|entry| entry.process.pid == pid)
            .ok_or_else(|| "该进程已不再占用查询范围内的文件，请刷新".to_string()).and_then(terminate);
        json!({ "pid": pid, "success": result.is_ok(), "message": result.err().unwrap_or_else(|| "已终止".into()) })
    }).collect()))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn directory_scan_is_bounded_and_reports_incomplete_results() {
        let dir = tempfile::tempdir().unwrap();
        for i in 0..12 {
            std::fs::write(dir.path().join(format!("{i}.txt")), "test").unwrap();
        }
        let result = collect_limited(dir.path().to_str().unwrap(), 4).unwrap();
        assert_eq!(result.files.len(), 4);
        assert!(result.limited);
        assert_eq!(result.skipped, 0);
    }
    #[test]
    fn empty_missing_and_unselected() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            query(dir.path().to_str().unwrap()).unwrap()["occupations"],
            json!([])
        );
        assert!(query(dir.path().join("missing").to_str().unwrap()).is_err());
        assert!(release(dir.path().to_str().unwrap(), None).is_err());
        let result = release(dir.path().to_str().unwrap(), Some(vec![std::process::id()])).unwrap();
        assert_eq!(result[0]["success"], false);
    }
    #[test]
    fn real_file_owner_and_release() {
        use std::process::{Command, Stdio};
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("占用 测试.txt");
        std::fs::write(&file, "test").unwrap();
        let ready = dir.path().join("ready");
        let script = format!("$f=[IO.File]::Open('{}','Open','ReadWrite','None'); [IO.File]::WriteAllText('{}','ready'); Start-Sleep -Seconds 30; $f.Dispose()",
            file.display().to_string().replace('\'', "''"), ready.display().to_string().replace('\'', "''"));
        let child = Command::new("powershell")
            .args(["-NoProfile", "-Command", &script])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        struct Child(std::process::Child);
        impl Drop for Child {
            fn drop(&mut self) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
        let mut child = Child(child);
        let start = Instant::now();
        while !ready.exists() && start.elapsed() < Duration::from_secs(10) {
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(ready.exists(), "fixture did not acquire file");
        let pid = child.0.id();
        let (_, owners) = scan(file.to_str().unwrap()).unwrap();
        let mut stale = *owners.iter().find(|p| p.process.pid == pid).unwrap();
        stale.process.started.low ^= 1;
        assert!(terminate(&stale).unwrap_err().contains("身份"));
        assert!(child.0.try_wait().unwrap().is_none());
        assert_eq!(
            release(file.to_str().unwrap(), Some(vec![std::process::id()])).unwrap()[0]["success"],
            false
        );
        for path in [&file, dir.path()] {
            let result = query(path.to_str().unwrap()).unwrap();
            assert!(
                result["occupations"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|p| p["pid"] == pid),
                "{result}"
            );
        }
        let result = release(file.to_str().unwrap(), Some(vec![pid])).unwrap();
        assert_eq!(result[0]["success"], true, "{result}");
        child.0.wait().unwrap();
        assert!(query(file.to_str().unwrap()).unwrap()["occupations"]
            .as_array()
            .unwrap()
            .is_empty());
    }
}
