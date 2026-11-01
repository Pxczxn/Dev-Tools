//! Serves the prebuilt `frontend/` directory during development.
//! Windows resolves `http://tauri.localhost` to 127.0.0.1, so a debug build
//! without this server shows a connection-refused page and hides the toolbar.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

pub const DEV_FRONTEND_ADDR: &str = "127.0.0.1:1430";

pub fn frontend_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../frontend")
}

pub fn ensure_started() {
    if !tauri::is_dev() {
        return;
    }
    let Ok(listener) = TcpListener::bind(DEV_FRONTEND_ADDR) else {
        // `tauri dev` already started its static file server on this port.
        return;
    };
    let root = frontend_root();
    thread::spawn(move || serve(listener, &root));
}

pub fn serve(listener: TcpListener, root: &Path) {
    for connection in listener.incoming() {
        let Ok(mut stream) = connection else {
            continue;
        };
        let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
        let _ = handle(&mut stream, root);
    }
}

pub fn resolve_file(root: &Path, request_path: &str) -> Option<PathBuf> {
    let path = request_path.split(['?', '#']).next().unwrap_or("/");
    let relative = path.trim_start_matches('/');
    if relative.contains('\0') || relative.contains(':') {
        return None;
    }
    let relative = if relative.is_empty() {
        "index.html"
    } else {
        relative
    };
    let root = root.canonicalize().ok()?;
    let candidate = root.join(relative);
    let file = candidate.canonicalize().ok()?;
    if file.starts_with(&root) && file.is_file() {
        Some(file)
    } else {
        None
    }
}

fn handle(stream: &mut TcpStream, root: &Path) -> std::io::Result<()> {
    let mut buffer = [0u8; 8192];
    let read = stream.read(&mut buffer)?;
    let request = String::from_utf8_lossy(&buffer[..read]);
    let mut lines = request.lines();
    let Some(start) = lines.next() else {
        return Ok(());
    };
    let mut parts = start.split_whitespace();
    let method = parts.next().unwrap_or("");
    let target = parts.next().unwrap_or("/");
    if method != "GET" && method != "HEAD" {
        return write_response(stream, 405, "text/plain", b"Method Not Allowed", method == "HEAD");
    }
    let Some(file) = resolve_file(root, target) else {
        return write_response(stream, 404, "text/plain", b"Not Found", method == "HEAD");
    };
    let bytes = std::fs::read(&file)?;
    let content_type = mime_type(&file);
    write_response(stream, 200, content_type, &bytes, method == "HEAD")
}

fn write_response(
    stream: &mut TcpStream,
    status: u16,
    content_type: &str,
    body: &[u8],
    head: bool,
) -> std::io::Result<()> {
    let reason = if status == 200 { "OK" } else if status == 404 { "Not Found" } else { "Error" };
    write!(
        stream,
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )?;
    if !head {
        stream.write_all(body)?;
    }
    Ok(())
}

fn mime_type(path: &Path) -> &'static str {
    match path.extension().and_then(|ext| ext.to_str()).unwrap_or("") {
        "html" => "text/html; charset=utf-8",
        "js" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "svg" => "image/svg+xml",
        "json" => "application/json",
        "png" => "image/png",
        "ico" => "image/x-icon",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    #[test]
    fn resolve_rejects_paths_outside_the_frontend_root() {
        let root = frontend_root();
        assert!(resolve_file(&root, "/").unwrap().ends_with("index.html"));
        assert!(resolve_file(&root, "/tools/infra.js").is_some());
        assert!(resolve_file(&root, "/../src-tauri/Cargo.toml").is_none());
        assert!(resolve_file(&root, "/..\\..\\Cargo.toml").is_none());
    }

    #[test]
    fn dev_server_returns_the_toolbar_page() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let root = frontend_root();
        thread::spawn(move || serve(listener, &root));
        let mut stream = TcpStream::connect(address).unwrap();
        stream.write_all(b"GET / HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n").unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        assert!(response.contains("200 OK"));
        assert!(response.contains("MySQL / Redis"));
        assert!(response.contains("text/html"));
    }
}
