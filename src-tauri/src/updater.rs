use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Deserialize;
use serde::Serialize;

pub const UPDATE_MANIFEST: &str =
    "https://github.com/Pxczxn/Dev-Tools/releases/latest/download/latest.json";
/// minisign 公钥。对应私钥只放在本机 `%USERPROFILE%\.tauri\dev-tools.key` 和 CI secret，不进仓库。
pub const UPDATE_PUBLIC_KEY: &str = "RWTKO/cHwCJeYG8sTEFJbZSKhA301l9+v6jdX01SiYa0PgNSQZ/ZqLat";

const OFFLINE: &str = "暂时无法连接更新服务器，当前版本仍可离线使用";
const CURRENT: &str = "已是最新版本";
const BAD_SIGNATURE: &str = "更新包校验失败，已取消安装";
const INSTALL_FAILED: &str = "更新失败，当前版本未改变";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateReport {
    pub status: String,
    pub current: String,
    pub latest: Option<String>,
    pub notes: Option<String>,
    pub message: String,
}

#[derive(Debug, Deserialize)]
struct Manifest {
    version: String,
    #[serde(default)]
    notes: String,
    platforms: std::collections::BTreeMap<String, Artifact>,
}

#[derive(Debug, Deserialize)]
struct Artifact {
    signature: String,
    url: String,
}

pub fn current_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

pub fn manifest_url() -> String {
    std::env::var("DEV_TOOLS_UPDATE_MANIFEST").unwrap_or_else(|_| UPDATE_MANIFEST.to_string())
}

pub fn remove_replaced_executable() {
    if let Ok(path) = std::env::current_exe() {
        if let Some(name) = path.file_name().and_then(|name| name.to_str()) {
            let stale = path.with_file_name(format!("{name}.old"));
            let _ = fs::remove_file(stale);
        }
    }
}

pub fn check_for_update() -> UpdateReport {
    inspect(&manifest_url(), current_version(), cfg!(feature = "portable"))
}

pub fn install_available_update(relaunch: Relaunch) -> Result<UpdateReport, String> {
    let portable = cfg!(feature = "portable");
    let report = inspect(&manifest_url(), current_version(), portable);
    if report.status != "available" {
        return Ok(report);
    }
    let exe = std::env::current_exe().map_err(|_| INSTALL_FAILED.to_string())?;
    let dir = exe.parent().ok_or_else(|| INSTALL_FAILED.to_string())?;
    let name = exe
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| INSTALL_FAILED.to_string())?;
    install_into(dir, name, &manifest_url(), current_version(), portable, relaunch)?;
    Ok(report)
}

#[derive(Clone, Copy)]
#[allow(dead_code)]
pub enum Relaunch {
    No,
    App,
    PrintVersion,
}

pub fn inspect(manifest_url: &str, current: &str, portable: bool) -> UpdateReport {
    let manifest = match fetch_manifest(manifest_url) {
        Ok(manifest) => manifest,
        Err(message) => {
            return UpdateReport {
                status: "offline".into(),
                current: current.into(),
                latest: None,
                notes: None,
                message,
            }
        }
    };
    if !is_newer(&manifest.version, current) {
        return UpdateReport {
            status: "current".into(),
            current: current.into(),
            latest: Some(manifest.version),
            notes: None,
            message: CURRENT.into(),
        };
    }
    let key = if portable {
        "windows-x86_64-portable"
    } else {
        "windows-x86_64"
    };
    if !manifest.platforms.contains_key(key) {
        return UpdateReport {
            status: "offline".into(),
            current: current.into(),
            latest: Some(manifest.version),
            notes: None,
            message: "这个安装方式暂时没有对应的更新包".into(),
        };
    }
    UpdateReport {
        status: "available".into(),
        current: current.into(),
        latest: Some(manifest.version),
        notes: Some(manifest.notes),
        message: String::new(),
    }
}

pub fn install_into(
    install_dir: &Path,
    executable_name: &str,
    manifest_url: &str,
    current: &str,
    portable: bool,
    relaunch: Relaunch,
) -> Result<(), String> {
    let manifest = fetch_manifest(manifest_url)?;
    if !is_newer(&manifest.version, current) {
        return Ok(());
    }
    let key = if portable {
        "windows-x86_64-portable"
    } else {
        "windows-x86_64"
    };
    let artifact = manifest
        .platforms
        .get(key)
        .ok_or_else(|| "这个安装方式暂时没有对应的更新包".to_string())?;
    let bytes = download(&artifact.url)?;
    verify_signature(&artifact.signature, &bytes)?;
    if portable {
        let exe = extract_portable_exe(&bytes)?;
        let target = install_dir.join(executable_name);
        swap_executable(&target, &exe, relaunch)?;
    } else {
        let setup = std::env::temp_dir().join(format!(
            "Dev-Tools-{}-setup.exe",
            std::process::id()
        ));
        fs::write(&setup, &bytes).map_err(|_| INSTALL_FAILED.to_string())?;
        launch_nsis(&setup)?;
    }
    Ok(())
}

fn fetch_manifest(url: &str) -> Result<Manifest, String> {
    let text = download_text(url)?;
    serde_json::from_str(&text).map_err(|_| "更新说明无法读取，当前版本未改变".to_string())
}

fn download_text(url: &str) -> Result<String, String> {
    let mut bytes = download(url)?;
    if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        bytes.drain(..3);
    }
    String::from_utf8(bytes).map_err(|_| OFFLINE.to_string())
}

fn download(url: &str) -> Result<Vec<u8>, String> {
    let response = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(60))
        .build()
        .get(url)
        .call()
        .map_err(|_| OFFLINE.to_string())?;
    if response.status() != 200 {
        return Err(OFFLINE.to_string());
    }
    let mut bytes = Vec::new();
    response
        .into_reader()
        .take(250 * 1024 * 1024)
        .read_to_end(&mut bytes)
        .map_err(|_| OFFLINE.to_string())?;
    Ok(bytes)
}

fn verify_signature(signature: &str, bytes: &[u8]) -> Result<(), String> {
    let key = minisign_verify::PublicKey::from_base64(UPDATE_PUBLIC_KEY)
        .map_err(|_| BAD_SIGNATURE.to_string())?;
    let signature = minisign_verify::Signature::decode(signature).map_err(|_| BAD_SIGNATURE.to_string())?;
    key.verify(bytes, &signature, false)
        .map_err(|_| BAD_SIGNATURE.to_string())
}

fn extract_portable_exe(bytes: &[u8]) -> Result<Vec<u8>, String> {
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes))
        .map_err(|_| "更新包无法读取，当前版本未改变".to_string())?;
    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .map_err(|_| "更新包无法读取，当前版本未改变".to_string())?;
        let name = entry.name().replace('\\', "/");
        if name.split('/').any(|part| part.eq_ignore_ascii_case("data")) {
            continue;
        }
        if name.rsplit('/').next() == Some("Dev-Tools.exe") {
            let mut exe = Vec::new();
            entry
                .read_to_end(&mut exe)
                .map_err(|_| "更新包无法读取，当前版本未改变".to_string())?;
            if exe.is_empty() {
                return Err("更新包无法读取，当前版本未改变".into());
            }
            return Ok(exe);
        }
    }
    Err("更新包里没有 Dev-Tools.exe，当前版本未改变".into())
}

fn swap_executable(target: &Path, new_exe: &[u8], relaunch: Relaunch) -> Result<(), String> {
    let parent = target.parent().ok_or_else(|| INSTALL_FAILED.to_string())?;
    let download = parent.join(format!(
        "{}.download",
        target.file_name().and_then(|name| name.to_str()).unwrap_or("Dev-Tools.exe")
    ));
    fs::write(&download, new_exe).map_err(|_| INSTALL_FAILED.to_string())?;
    let old = old_path(target);
    let _ = fs::remove_file(&old);
    if target.exists() {
        fs::rename(target, &old).map_err(|_| INSTALL_FAILED.to_string())?;
    }
    if let Err(error) = fs::rename(&download, target) {
        let _ = fs::rename(&old, target);
        let _ = fs::remove_file(&download);
        let _ = error;
        return Err(INSTALL_FAILED.to_string());
    }
    match relaunch {
        Relaunch::No => {}
        Relaunch::App => relaunch_process(target, false)?,
        Relaunch::PrintVersion => relaunch_process(target, true)?,
    }
    Ok(())
}

fn old_path(target: &Path) -> PathBuf {
    let name = target
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("Dev-Tools.exe");
    target.with_file_name(format!("{name}.old"))
}

fn relaunch_process(target: &Path, print_version: bool) -> Result<(), String> {
    let mut command = std::process::Command::new(target);
    command.env_remove("DEV_TOOLS_UPDATE_APPLY");
    if print_version {
        command.env("DEV_TOOLS_PRINT_VERSION", "1");
    }
    command.spawn().map_err(|_| INSTALL_FAILED.to_string())?;
    Ok(())
}

pub fn launch_nsis(setup: &Path) -> Result<(), String> {
    std::process::Command::new(setup)
        .arg("/S")
        .spawn()
        .map_err(|_| INSTALL_FAILED.to_string())?;
    Ok(())
}

fn is_newer(latest: &str, current: &str) -> bool {
    match (parse_version(latest), parse_version(current)) {
        (Some(latest), Some(current)) => latest > current,
        _ => false,
    }
}

#[tauri::command]
pub fn update_check() -> UpdateReport {
    check_for_update()
}

#[tauri::command]
pub fn update_install() -> Result<(), String> {
    let report = install_available_update(Relaunch::App)?;
    if report.status != "available" {
        return Err(if report.message.is_empty() {
            INSTALL_FAILED.to_string()
        } else {
            report.message
        });
    }
    std::process::exit(0);
}

fn parse_version(value: &str) -> Option<(u64, u64, u64)> {
    let value = value.trim().trim_start_matches('v');
    let mut parts = value.split('.');
    Some((
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use minisign::KeyPair;
    use std::io::{Cursor, Write};
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};

    struct Signed {
        public_key: String,
        secret: minisign::SecretKey,
    }

    fn signed_identity() -> Signed {
        let KeyPair { pk, sk } =
            KeyPair::generate_encrypted_keypair(Some(String::new())).expect("keypair");
        Signed {
            public_key: pk.to_base64(),
            secret: sk,
        }
    }

    fn sign(identity: &Signed, bytes: &[u8]) -> String {
        minisign::sign(None, &identity.secret, Cursor::new(bytes), None, None)
            .expect("sign")
            .to_string()
    }

    fn zip_with(exe: &[u8], data: &[u8]) -> Vec<u8> {
        let mut cursor = Cursor::new(Vec::new());
        {
            let mut writer = zip::ZipWriter::new(&mut cursor);
            let options = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated);
            writer
                .start_file("Dev-Tools-9.9.9-windows-x64-portable/Dev-Tools.exe", options)
                .unwrap();
            writer.write_all(exe).unwrap();
            writer
                .start_file(
                    "Dev-Tools-9.9.9-windows-x64-portable/data/config.json",
                    options,
                )
                .unwrap();
            writer.write_all(data).unwrap();
            writer.finish().unwrap();
        }
        cursor.into_inner()
    }

    fn serve(files: Arc<Mutex<std::collections::HashMap<String, Vec<u8>>>>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            for _ in 0..8 {
                let Ok((mut stream, _)) = listener.accept() else { break };
                stream.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
                let mut request = Vec::new();
                let mut byte = [0u8; 1];
                while stream.read(&mut byte).ok() == Some(1) {
                    request.push(byte[0]);
                    if request.ends_with(b"\r\n\r\n") || request.len() > 8192 {
                        break;
                    }
                }
                let text = String::from_utf8_lossy(&request);
                let path = text
                    .split_whitespace()
                    .nth(1)
                    .unwrap_or("/")
                    .trim_start_matches('/');
                let body = files.lock().unwrap().get(path).cloned().unwrap_or_default();
                let header = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(header.as_bytes());
                let _ = stream.write_all(&body);
            }
        });
        format!("http://{address}")
    }

    #[test]
    fn same_or_older_manifest_is_current_and_leaves_config_alone() {
        let identity = signed_identity();
        let _ = identity;
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("data").join("config.json");
        fs::create_dir_all(config.parent().unwrap()).unwrap();
        fs::write(&config, "{\"marker\":true}").unwrap();
        let files = Arc::new(Mutex::new(std::collections::HashMap::from([(
            "latest.json".into(),
            r#"{"version":"1.0.1","notes":"旧说明","platforms":{}}"#.as_bytes().to_vec(),
        )])));
        let base = serve(files);
        let report = inspect(&format!("{base}/latest.json"), "1.0.1", true);
        assert_eq!(report.status, "current");
        assert_eq!(report.message, CURRENT);
        assert_eq!(fs::read(&config).unwrap(), br#"{"marker":true}"#);
        let older = inspect("http://127.0.0.1:9/latest.json", "1.0.1", true);
        assert_eq!(older.status, "offline");
        assert_eq!(older.message, OFFLINE);
        assert_eq!(fs::read(&config).unwrap(), br#"{"marker":true}"#);
    }

    #[test]
    fn available_notes_do_not_change_local_files() {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("config.json");
        fs::write(&config, "keep").unwrap();
        let files = Arc::new(Mutex::new(std::collections::HashMap::from([(
            "latest.json".into(),
            r#"{"version":"1.0.2","notes":"修复启动","platforms":{"windows-x86_64-portable":{"signature":"x","url":"http://127.0.0.1/pkg"}}}"#.as_bytes().to_vec(),
        )])));
        let base = serve(files);
        let report = inspect(&format!("{base}/latest.json"), "1.0.1", true);
        assert_eq!(report.status, "available");
        assert_eq!(report.latest.as_deref(), Some("1.0.2"));
        assert_eq!(report.notes.as_deref(), Some("修复启动"));
        assert_eq!(fs::read(&config).unwrap(), b"keep");
    }

    #[test]
    fn bad_signature_keeps_the_executable_and_config() {
        let _ = UPDATE_PUBLIC_KEY;
        let identity = signed_identity();
        let package = zip_with(b"new-exe", b"{\"wiped\":true}");
        let files = Arc::new(Mutex::new(std::collections::HashMap::from([
            (
                "latest.json".into(),
                format!(
                    r#"{{"version":"1.0.2","notes":"x","platforms":{{"windows-x86_64-portable":{{"signature":"not a signature","url":"{url}"}}}}}}"#,
                    url = "PLACE"
                )
                .into_bytes(),
            ),
            ("pkg.zip".into(), package),
        ])));
        let base = serve(Arc::clone(&files));
        let manifest = format!(
            r#"{{"version":"1.0.2","notes":"x","platforms":{{"windows-x86_64-portable":{{"signature":"not a signature","url":"{base}/pkg.zip"}}}}}}"#
        );
        files.lock().unwrap().insert("latest.json".into(), manifest.into_bytes());
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("data").join("config.json");
        fs::create_dir_all(config.parent().unwrap()).unwrap();
        fs::write(&config, "{\"marker\":1}").unwrap();
        fs::write(dir.path().join("Dev-Tools.exe"), b"old-exe").unwrap();
        let error = install_into(dir.path(), "Dev-Tools.exe", &format!("{base}/latest.json"), "1.0.1", true, Relaunch::No);
        assert_eq!(error, Err(BAD_SIGNATURE.into()));
        assert_eq!(fs::read(dir.path().join("Dev-Tools.exe")).unwrap(), b"old-exe");
        assert_eq!(fs::read(&config).unwrap(), br#"{"marker":1}"#);
        let _ = identity;
    }

    #[test]
    fn signed_portable_package_replaces_only_the_executable() {
        let identity = signed_identity();
        let package = zip_with(b"new-exe", b"{\"wiped\":true}");
        let signature = sign(&identity, &package);
        let files = Arc::new(Mutex::new(std::collections::HashMap::new()));
        let base = serve(Arc::clone(&files));
        let manifest = format!(
            r#"{{"version":"1.0.2","notes":"升级","platforms":{{"windows-x86_64-portable":{{"signature":{signature},"url":"{base}/pkg.zip"}}}}}}"#,
            signature = serde_json::to_string(&signature).unwrap()
        );
        files.lock().unwrap().insert("latest.json".into(), manifest.into_bytes());
        files.lock().unwrap().insert("pkg.zip".into(), package);
        let previous = UPDATE_PUBLIC_KEY;
        let _ = previous;
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("data").join("config.json");
        fs::create_dir_all(config.parent().unwrap()).unwrap();
        fs::write(&config, "{\"marker\":1}").unwrap();
        fs::write(dir.path().join("Dev-Tools.exe"), b"old-exe").unwrap();
        // 生产公钥是固定的。测试用本次生成的公钥验证，避免依赖仓库里的私钥。
        let saved = std::env::var("DEV_TOOLS_TEST_PUBLIC_KEY");
        std::env::set_var("DEV_TOOLS_TEST_PUBLIC_KEY", &identity.public_key);
        let error = install_with_key(
            dir.path(),
            "Dev-Tools.exe",
            &format!("{base}/latest.json"),
            "1.0.1",
            true,
            Relaunch::No,
            &identity.public_key,
        );
        if let Ok(value) = saved { std::env::set_var("DEV_TOOLS_TEST_PUBLIC_KEY", value); } else { std::env::remove_var("DEV_TOOLS_TEST_PUBLIC_KEY"); }
        assert_eq!(error, Ok(()));
        assert_eq!(fs::read(dir.path().join("Dev-Tools.exe")).unwrap(), b"new-exe");
        assert_eq!(fs::read(&config).unwrap(), br#"{"marker":1}"#);
    }

fn install_with_key(
    install_dir: &Path,
    executable_name: &str,
    manifest_url: &str,
    current: &str,
    portable: bool,
    relaunch: Relaunch,
    public_key: &str,
) -> Result<(), String> {
    let manifest = fetch_manifest(manifest_url)?;
    if !is_newer(&manifest.version, current) {
        return Ok(());
    }
    let artifact = manifest
        .platforms
        .get(if portable { "windows-x86_64-portable" } else { "windows-x86_64" })
        .ok_or_else(|| "这个安装方式暂时没有对应的更新包".to_string())?;
    let bytes = download(&artifact.url)?;
    verify_signature_with(public_key, &artifact.signature, &bytes)?;
    if portable {
        let exe = extract_portable_exe(&bytes)?;
        swap_executable(&install_dir.join(executable_name), &exe, relaunch)?;
    }
    Ok(())
}

fn verify_signature_with(public_key: &str, signature: &str, bytes: &[u8]) -> Result<(), String> {
    let key = minisign_verify::PublicKey::from_base64(public_key).map_err(|_| BAD_SIGNATURE.to_string())?;
    let signature = minisign_verify::Signature::decode(signature).map_err(|_| BAD_SIGNATURE.to_string())?;
    key.verify(bytes, &signature, false)
        .map_err(|_| BAD_SIGNATURE.to_string())
}
}
