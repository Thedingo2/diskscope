use rayon::prelude::*;
use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::{self, JoinHandle};
use std::time::Duration;
use tiny_http::{Header, Method, Request, Response, Server};

use crate::debug;
use crate::scanner::{
    get_available_drives, get_drive_space, scan_subtree_in_memory, FileItem, ScanStatus,
};

const EMBEDDED_WEB_UI_HTML: &str = include_str!("resource/server/web_ui.html");

fn resolve_resource_file(rel_parts: &[&str]) -> Option<PathBuf> {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let mut candidate = dir.join("resource");
            for part in rel_parts {
                candidate = candidate.join(part);
            }
            if candidate.exists() {
                return Some(candidate);
            }
            if let Some(contents) = dir.parent() {
                let mut res_candidate = contents.join("Resources").join("resource");
                for part in rel_parts {
                    res_candidate = res_candidate.join(part);
                }
                if res_candidate.exists() {
                    return Some(res_candidate);
                }
            }
        }
    }
    let mut local = PathBuf::from("resource");
    for part in rel_parts {
        local = local.join(part);
    }
    if local.exists() {
        Some(local)
    } else {
        None
    }
}

fn load_web_ui_html() -> String {
    if let Some(path) = resolve_resource_file(&["server", "web_ui.html"]) {
        if let Ok(html) = fs::read_to_string(&path) {
            return html;
        }
    }
    EMBEDDED_WEB_UI_HTML.to_string()
}

struct WebScanState {
    is_scanning: AtomicBool,
    items_processed: AtomicUsize,
    target_path: Mutex<String>,
    status_text: Mutex<String>,
    live_items: Mutex<HashMap<String, (ScanStatus, u64)>>,
    scanning_dirs: Mutex<HashSet<String>>,
}

fn scan_state() -> &'static WebScanState {
    static STATE: OnceLock<WebScanState> = OnceLock::new();
    STATE.get_or_init(|| WebScanState {
        is_scanning: AtomicBool::new(false),
        items_processed: AtomicUsize::new(0),
        target_path: Mutex::new(String::new()),
        status_text: Mutex::new("Ready".to_string()),
        live_items: Mutex::new(HashMap::new()),
        scanning_dirs: Mutex::new(HashSet::new()),
    })
}

pub struct ServerHandle {
    stop_flag: Arc<AtomicBool>,
    join_handle: Option<JoinHandle<()>>,
    pub port: u16,
}

impl ServerHandle {
    pub fn stop(&mut self) {
        self.stop_flag.store(true, Ordering::SeqCst);
        if let Some(handle) = self.join_handle.take() {
            let _ = handle.join();
        }
        debug::log(format!("[DiskScope Server] Stopped server on port {}", self.port));
    }
}

impl Drop for ServerHandle {
    fn drop(&mut self) {
        self.stop();
    }
}

pub fn start_background(port: u16, pin: String) -> Result<ServerHandle, String> {
    let addr = format!("0.0.0.0:{}", port);
    let server = Server::http(&addr).map_err(|e| {
        format!(
            "Port {} is not free or unavailable ({}). Try changing the port number.",
            port, e
        )
    })?;

    debug::log(format!(
        "[DiskScope Server] Listening on http://0.0.0.0:{} (http://localhost:{})",
        port, port
    ));

    let stop_flag = Arc::new(AtomicBool::new(false));
    let thread_stop = Arc::clone(&stop_flag);

    let join_handle = thread::spawn(move || {
        serve_loop(server, thread_stop, pin);
    });

    Ok(ServerHandle {
        stop_flag,
        join_handle: Some(join_handle),
        port,
    })
}

pub fn run_blocking(port: u16, pin: String) -> Result<(), String> {
    let addr = format!("0.0.0.0:{}", port);
    let server = Server::http(&addr).map_err(|e| {
        format!(
            "Port {} is not free or unavailable ({}). Try passing --port <number>.",
            port, e
        )
    })?;

    println!(
        "[DiskScope Server] Headless Server Mode v0.03 active on http://0.0.0.0:{} (http://localhost:{})",
        port, port
    );
    let stop_flag = Arc::new(AtomicBool::new(false));
    serve_loop(server, stop_flag, pin);
    Ok(())
}

fn serve_loop(server: Server, stop_flag: Arc<AtomicBool>, pin: String) {
    while !stop_flag.load(Ordering::SeqCst) {
        match server.recv_timeout(Duration::from_millis(250)) {
            Ok(Some(request)) => {
                let pin_clone = pin.clone();
                thread::spawn(move || {
                    handle_request(request, &pin_clone);
                });
            }
            Ok(None) => {}
            Err(e) => {
                debug::log(format!("[DiskScope Server] Accept error: {}", e));
                break;
            }
        }
    }
}

fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {}
            c => out.push(c),
        }
    }
    out
}

fn status_str(status: ScanStatus) -> &'static str {
    match status {
        ScanStatus::Pending => "pending",
        ScanStatus::Scanning => "scanning",
        ScanStatus::Done => "done",
    }
}

fn items_to_json(items: &[FileItem]) -> String {
    let mut out = String::from("[");
    for (i, item) in items.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        let indexed = item.status == ScanStatus::Done;
        out.push_str(&format!(
            r#"{{"name":"{}","path":"{}","size":{},"is_dir":{},"indexed":{},"status":"{}"}}"#,
            json_escape(&item.name),
            json_escape(&item.path),
            item.size,
            item.is_dir,
            indexed,
            status_str(item.status)
        ));
    }
    out.push(']');
    out
}

fn url_decode(s: &str) -> String {
    let mut bytes = Vec::with_capacity(s.len());
    let raw = s.as_bytes();
    let mut i = 0;
    while i < raw.len() {
        if raw[i] == b'%' && i + 2 < raw.len() {
            if let Ok(hex) = std::str::from_utf8(&raw[i + 1..i + 3]) {
                if let Ok(val) = u8::from_str_radix(hex, 16) {
                    bytes.push(val);
                    i += 3;
                    continue;
                }
            }
        } else if raw[i] == b'+' {
            bytes.push(b' ');
            i += 1;
            continue;
        }
        bytes.push(raw[i]);
        i += 1;
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

fn query_param(url: &str, key: &str) -> String {
    let Some((_, query)) = url.split_once('?') else {
        return String::new();
    };
    for pair in query.split('&') {
        if let Some((k, v)) = pair.split_once('=') {
            if k == key {
                return url_decode(v);
            }
        }
    }
    String::new()
}

fn extract_json_str(body: &str, key: &str) -> String {
    let needle = format!("\"{}\"", key);
    let Some(pos) = body.find(&needle) else { return String::new() };
    let rest = &body[pos + needle.len()..];
    let Some(colon) = rest.find(':') else { return String::new() };
    let after_colon = rest[colon + 1..].trim_start();
    if !after_colon.starts_with('"') {
        return String::new();
    }
    let mut out = String::new();
    let mut chars = after_colon[1..].chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            if let Some(next) = chars.next() {
                out.push(next);
            }
        } else if c == '"' {
            break;
        } else {
            out.push(c);
        }
    }
    out
}

fn extract_json_u64(body: &str, key: &str) -> u64 {
    let needle = format!("\"{}\"", key);
    let Some(pos) = body.find(&needle) else { return 0 };
    let rest = &body[pos + needle.len()..];
    let Some(colon) = rest.find(':') else { return 0 };
    let digits: String = rest[colon + 1..]
        .trim_start()
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    digits.parse().unwrap_or(0)
}

fn normalize_path(target: &str) -> &str {
    if target.len() > 3 && (target.ends_with('\\') || target.ends_with('/')) {
        &target[..target.len() - 1]
    } else {
        target
    }
}

fn load_children_with_fallback(target: &str) -> Vec<FileItem> {
    let trimmed = normalize_path(target);
    let live_snapshot: HashMap<String, (ScanStatus, u64)> = scan_state()
        .live_items
        .lock()
        .map(|g| g.clone())
        .unwrap_or_default();

    let scanning_set: HashSet<String> = scan_state()
        .scanning_dirs
        .lock()
        .map(|g| g.clone())
        .unwrap_or_default();

    let mut items = crate::db::get_children(trimmed);
    if items.is_empty() && trimmed != target {
        items = crate::db::get_children(target);
    }

    if items.is_empty() {
        let cached = crate::db::get_cached_sizes(trimmed);
        if let Ok(entries) = fs::read_dir(trimmed) {
            for entry in entries.flatten() {
                let p = entry.path();
                let path_str = p.to_string_lossy().into_owned();
                let is_dir = entry.file_type().map(|ft| ft.is_dir()).unwrap_or(false);
                let (size, status) = if let Some(&(st, sz)) = live_snapshot.get(&path_str) {
                    (sz, st)
                } else if scanning_set.contains(&path_str) {
                    (0, ScanStatus::Scanning)
                } else if let Some(&sz) = cached.get(&path_str) {
                    (sz, ScanStatus::Done)
                } else if !is_dir {
                    (
                        entry.metadata().map(|m| m.len()).unwrap_or(0),
                        ScanStatus::Done,
                    )
                } else {
                    (0, ScanStatus::Pending)
                };

                items.push(FileItem {
                    name: entry.file_name().to_string_lossy().into_owned(),
                    path: path_str,
                    size,
                    is_dir,
                    status,
                });
            }
        }
    } else {
        for item in &mut items {
            if let Some(&(st, sz)) = live_snapshot.get(&item.path) {
                item.status = st;
                if sz > 0 {
                    item.size = sz;
                }
            } else if scanning_set.contains(&item.path) {
                item.status = ScanStatus::Scanning;
            }
        }
    }

    items.sort_by(|a, b| {
        if a.is_dir != b.is_dir {
            b.is_dir.cmp(&a.is_dir)
        } else {
            b.size.cmp(&a.size)
        }
    });
    items
}

fn fallback_live_search(root: &str, query: &str, filter: &str, limit: usize) -> Vec<FileItem> {
    let q_lower = query.trim().to_ascii_lowercase();
    let category_exts: Option<&[&str]> = match filter {
        "Videos" => Some(&["mp4", "mkv", "mov", "avi", "webm", "wmv", "flv", "m4v"]),
        "Archives" => Some(&["zip", "7z", "rar", "tar", "gz", "xz", "iso", "dmg"]),
        "Images" => Some(&["png", "jpg", "jpeg", "gif", "webp", "bmp", "psd", "tga", "svg"]),
        "Audio" => Some(&["mp3", "wav", "flac", "ogg", "m4a", "aac", "opus"]),
        "Executables" => Some(&["exe", "msi", "app", "pkg", "deb", "rpm", "dmg", "bin"]),
        _ => None,
    };

    let mut results = Vec::new();
    let mut stack = vec![PathBuf::from(root)];
    let mut visited = 0usize;

    while let Some(dir) = stack.pop() {
        if results.len() >= limit || visited > 15_000 {
            break;
        }
        let Ok(entries) = fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            visited += 1;
            if results.len() >= limit || visited > 15_000 {
                break;
            }
            let Ok(ft) = entry.file_type() else { continue };
            if ft.is_symlink() {
                continue;
            }
            let p = entry.path();
            let is_dir = ft.is_dir();
            if is_dir {
                stack.push(p.clone());
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            if !q_lower.is_empty() && !name.to_ascii_lowercase().contains(&q_lower) {
                continue;
            }
            if filter != "All" && !filter.is_empty() {
                if is_dir {
                    continue;
                }
                let ext = p
                    .extension()
                    .and_then(|e| e.to_str())
                    .map(|e| e.to_ascii_lowercase())
                    .unwrap_or_default();
                if let Some(exts) = category_exts {
                    if !exts.contains(&ext.as_str()) {
                        continue;
                    }
                } else if ext != filter.trim_start_matches('.').to_ascii_lowercase() {
                    continue;
                }
            }
            let size = if is_dir {
                0
            } else {
                entry.metadata().map(|m| m.len()).unwrap_or(0)
            };
            results.push(FileItem {
                name,
                path: p.to_string_lossy().into_owned(),
                size,
                is_dir,
                status: ScanStatus::Done,
            });
        }
    }

    results.sort_by(|a, b| b.size.cmp(&a.size));
    results
}

fn spawn_background_scan(target: String) {
    let state = scan_state();
    if state
        .is_scanning
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return;
    }

    state.items_processed.store(0, Ordering::SeqCst);
    if let Ok(mut t) = state.target_path.lock() {
        *t = target.clone();
    }
    if let Ok(mut s) = state.status_text.lock() {
        *s = format!("Scanning {}...", target);
    }

    thread::spawn(move || {
        let normalized = normalize_path(&target).to_string();

        let mut top_items = Vec::new();
        if let Ok(entries) = fs::read_dir(&normalized) {
            if let Ok(mut sdirs) = scan_state().scanning_dirs.lock() {
                sdirs.clear();
                sdirs.insert(normalized.clone());
                for entry in entries.flatten() {
                    let p = entry.path();
                    let p_str = p.to_string_lossy().into_owned();
                    let is_dir = entry.file_type().map(|ft| ft.is_dir()).unwrap_or(false);
                    if is_dir {
                        sdirs.insert(p_str.clone());
                    }
                    top_items.push(FileItem {
                        name: entry.file_name().to_string_lossy().into_owned(),
                        path: p_str,
                        size: 0,
                        is_dir,
                        status: if is_dir {
                            ScanStatus::Scanning
                        } else {
                            ScanStatus::Done
                        },
                    });
                }
            }
        }

        if let Ok(mut map) = scan_state().live_items.lock() {
            map.clear();
            for item in &top_items {
                map.insert(item.path.clone(), (item.status, item.size));
            }
        }

        let db_write_lock = Mutex::new(());

        top_items.par_iter_mut().for_each(|item| {
            let p = PathBuf::from(&item.path);
            if item.is_dir {
                let (sz, sub_rows) = scan_subtree_in_memory(&p);
                item.size = sz;
                item.status = ScanStatus::Done;

                scan_state()
                    .items_processed
                    .fetch_add(sub_rows.len() + 1, Ordering::Relaxed);

                if let Ok(_guard) = db_write_lock.lock() {
                    if let Some(mut conn) = crate::db::open_conn() {
                        crate::db::insert_rows(&mut conn, &sub_rows);
                    }
                    crate::db::save_items_to_cache(std::slice::from_ref(item));
                    crate::db::mark_indexed(&item.path);
                }

                if let Ok(mut sdirs) = scan_state().scanning_dirs.lock() {
                    sdirs.remove(&item.path);
                }
                if let Ok(mut map) = scan_state().live_items.lock() {
                    map.insert(item.path.clone(), (ScanStatus::Done, sz));
                }
            } else if let Ok(meta) = p.metadata() {
                item.size = meta.len();
                item.status = ScanStatus::Done;
                scan_state().items_processed.fetch_add(1, Ordering::Relaxed);
                if let Ok(mut map) = scan_state().live_items.lock() {
                    map.insert(item.path.clone(), (ScanStatus::Done, item.size));
                }
            }
        });

        crate::db::save_items_to_cache(&top_items);
        crate::db::mark_indexed(&normalized);

        if Path::new(&normalized).parent().is_some() {
            let total_sz: u64 = top_items.iter().map(|i| i.size).sum();
            let name = Path::new(&normalized)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| normalized.clone());
            crate::db::save_items_to_cache(&[FileItem {
                name,
                path: normalized.clone(),
                size: total_sz,
                is_dir: true,
                status: ScanStatus::Done,
            }]);
        }

        if let Ok(mut sdirs) = scan_state().scanning_dirs.lock() {
            sdirs.clear();
        }
        if let Ok(mut map) = scan_state().live_items.lock() {
            map.clear();
        }

        let total_count = scan_state().items_processed.load(Ordering::Relaxed);
        if let Ok(mut s) = scan_state().status_text.lock() {
            *s = format!("Done scanning ({} items indexed)", total_count);
        }
        scan_state().is_scanning.store(false, Ordering::SeqCst);
        debug::log(format!(
            "[DiskScope Server] Done scanning '{}' ({} items)",
            normalized, total_count
        ));
    });
}

fn serve_slime_png(request: Request, file_name: &str) {
    if let Some(path) = resolve_resource_file(&["theme", "slimes", file_name]) {
        if let Ok(bytes) = fs::read(&path) {
            let header = Header::from_bytes(&b"Content-Type"[..], &b"image/png"[..]).unwrap();
            let _ = request.respond(Response::from_data(bytes).with_header(header));
            return;
        }
    }
    let _ = request.respond(Response::from_string("Not Found").with_status_code(404));
}

fn media_mime_type(path: &Path) -> &'static str {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        // Video
        "mp4" | "m4v" => "video/mp4",
        "webm" => "video/webm",
        "mkv" => "video/x-matroska",
        "mov" => "video/quicktime",
        "avi" => "video/x-msvideo",
        "wmv" => "video/x-ms-wmv",
        "flv" => "video/x-flv",
        // Audio
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "flac" => "audio/flac",
        "ogg" | "opus" => "audio/ogg",
        "m4a" | "aac" => "audio/mp4",
        // Images
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        "svg" => "image/svg+xml",
        _ => "application/octet-stream",
    }
}

fn parse_byte_range(range_hdr: &str, total_len: u64) -> Option<(u64, u64)> {
    let spec = range_hdr.strip_prefix("bytes=")?;
    let (start_str, end_str) = spec.split_once('-')?;
    if start_str.is_empty() {
        let suffix: u64 = end_str.parse().ok()?;
        let start = total_len.saturating_sub(suffix);
        Some((start, total_len.saturating_sub(1)))
    } else {
        let start: u64 = start_str.parse().ok()?;
        let end: u64 = if end_str.is_empty() {
            total_len.saturating_sub(1)
        } else {
            end_str.parse::<u64>().ok()?.min(total_len.saturating_sub(1))
        };
        if start <= end && start < total_len {
            Some((start, end))
        } else {
            None
        }
    }
}

/// Streams audio, video, and image files with HTTP 206 Byte-Range support
/// so browser <video> and <audio> players can seek instantly.
fn serve_media_file(request: Request, file_path: &str) {
    let path = Path::new(file_path);
    let Ok(meta) = fs::metadata(path) else {
        let _ = request.respond(Response::from_string("File Not Found").with_status_code(404));
        return;
    };
    if !meta.is_file() {
        let _ = request.respond(Response::from_string("Not a file").with_status_code(400));
        return;
    }

    let total_len = meta.len();
    let mime = media_mime_type(path);
    let range_header = request
        .headers()
        .iter()
        .find(|h| h.field.equiv("Range"))
        .map(|h| h.value.as_str().to_string());

    let Ok(mut file) = File::open(path) else {
        let _ = request.respond(Response::from_string("Cannot open file").with_status_code(500));
        return;
    };

    let ct_header = Header::from_bytes(&b"Content-Type"[..], mime.as_bytes()).unwrap();
    let accept_ranges = Header::from_bytes(&b"Accept-Ranges"[..], &b"bytes"[..]).unwrap();

    if let Some(rh) = range_header {
        if let Some((start, end)) = parse_byte_range(&rh, total_len) {
            let chunk_len = (end - start + 1).min(4 * 1024 * 1024);
            let actual_end = start + chunk_len - 1;
            let mut buf = vec![0u8; chunk_len as usize];
            if file.seek(SeekFrom::Start(start)).is_ok() && file.read_exact(&mut buf).is_ok() {
                let content_range = Header::from_bytes(
                    &b"Content-Range"[..],
                    format!("bytes {}-{}/{}", start, actual_end, total_len).as_bytes(),
                )
                .unwrap();
                let _ = request.respond(
                    Response::from_data(buf)
                        .with_status_code(206)
                        .with_header(ct_header)
                        .with_header(accept_ranges)
                        .with_header(content_range),
                );
                return;
            }
        }
    }

    let response = Response::from_file(file)
        .with_header(ct_header)
        .with_header(accept_ranges);
    let _ = request.respond(response);
}

fn handle_request(mut request: Request, pin: &str) {
    let url = request.url().to_string();
    let path_only = url.split('?').next().unwrap_or("/");

    if path_only.starts_with("/api/") && !pin.is_empty() {
        let header_pin = request
            .headers()
            .iter()
            .find(|h| h.field.equiv("X-DiskScope-Pin"))
            .map(|h| h.value.as_str())
            .unwrap_or("");
        let query_pin = query_param(&url, "pin");
        if header_pin != pin && query_pin != pin {
            let header = Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..]).unwrap();
            let _ = request.respond(
                Response::from_string(r#"{"error":"Unauthorized"}"#)
                    .with_status_code(401)
                    .with_header(header),
            );
            return;
        }
    }

    match (request.method(), path_only) {
        (&Method::Get, "/") => {
            let header =
                Header::from_bytes(&b"Content-Type"[..], &b"text/html; charset=utf-8"[..]).unwrap();
            let _ = request.respond(Response::from_string(load_web_ui_html()).with_header(header));
        }
        (&Method::Get, "/slimes/happy.png") => serve_slime_png(request, "happy.png"),
        (&Method::Get, "/slimes/shiver.png") => serve_slime_png(request, "shiver.png"),
        (&Method::Get, "/slimes/dead.png") => serve_slime_png(request, "dead.png"),
        (&Method::Get, "/api/media") => {
            let target = query_param(&url, "path");
            serve_media_file(request, &target);
        }
        (&Method::Get, "/api/status") => {
            let st = scan_state();
            let scanning = st.is_scanning.load(Ordering::SeqCst);
            let count = st.items_processed.load(Ordering::Relaxed);
            let target = st.target_path.lock().map(|g| g.clone()).unwrap_or_default();
            let status_text = st.status_text.lock().map(|g| g.clone()).unwrap_or_default();
            let body = format!(
                r#"{{"version":"0.03","scanning":{},"items_processed":{},"target":"{}","status":"{}"}}"#,
                scanning,
                count,
                json_escape(&target),
                json_escape(&status_text)
            );
            let header = Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..]).unwrap();
            let _ = request.respond(Response::from_string(body).with_header(header));
        }
        (&Method::Get, "/api/drives") => {
            let drives = get_available_drives();
            let mut out = String::from("[");
            for (i, d) in drives.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                let (total, free) = get_drive_space(d);
                out.push_str(&format!(
                    r#"{{"path":"{}","total":{},"free":{}}}"#,
                    json_escape(d),
                    total,
                    free
                ));
            }
            out.push(']');
            let header = Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..]).unwrap();
            let _ = request.respond(Response::from_string(out).with_header(header));
        }
        (&Method::Get, "/api/children") => {
            let target = query_param(&url, "path");
            let items = load_children_with_fallback(&target);
            let header = Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..]).unwrap();
            let _ =
                request.respond(Response::from_string(items_to_json(&items)).with_header(header));
        }
        (&Method::Get, "/api/search") => {
            let drive = query_param(&url, "drive");
            let current = query_param(&url, "path");
            let q = query_param(&url, "q");
            let filter = query_param(&url, "type");
            let search_root = if !drive.is_empty() {
                drive
            } else {
                current.clone()
            };

            let mut items = crate::db::search_items(&search_root, &q, &filter, 250);
            if items.is_empty() && !current.is_empty() && current != search_root {
                items = crate::db::search_items(&current, &q, &filter, 250);
            }
            if items.is_empty() {
                let fallback_root = if !current.is_empty() {
                    &current
                } else {
                    &search_root
                };
                items = fallback_live_search(fallback_root, &q, &filter, 150);
            }

            let header = Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..]).unwrap();
            let _ =
                request.respond(Response::from_string(items_to_json(&items)).with_header(header));
        }
        (&Method::Get, "/api/top") => {
            let drive = query_param(&url, "drive");
            let items = crate::db::get_top_files(&drive, 10);
            let header = Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..]).unwrap();
            let _ =
                request.respond(Response::from_string(items_to_json(&items)).with_header(header));
        }
        (&Method::Post, "/api/scan") => {
            let mut body = String::new();
            let _ = request.as_reader().read_to_string(&mut body);
            let target = extract_json_str(&body, "path");
            spawn_background_scan(target.clone());

            let header = Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..]).unwrap();
            let msg = format!(
                r#"{{"ok":true,"scanning":true,"message":"Scanning {}..."}}"#,
                json_escape(&target)
            );
            let _ = request.respond(Response::from_string(msg).with_header(header));
        }
        (&Method::Post, "/api/delete") => {
            let mut body = String::new();
            let _ = request.as_reader().read_to_string(&mut body);
            let target = extract_json_str(&body, "path");
            let size = extract_json_u64(&body, "size");
            let header = Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..]).unwrap();

            match trash::delete(&target) {
                Ok(()) => {
                    crate::db::remove_path(&target, size);
                    debug::log(format!("[DiskScope Server] Moved to Trash: {}", target));
                    let msg = format!(
                        r#"{{"ok":true,"message":"Moved to Trash: {}"}}"#,
                        json_escape(&target)
                    );
                    let _ = request.respond(Response::from_string(msg).with_header(header));
                }
                Err(e) => {
                    let msg = format!(
                        r#"{{"ok":false,"message":"Delete failed: {}"}}"#,
                        json_escape(&e.to_string())
                    );
                    let _ = request.respond(
                        Response::from_string(msg)
                            .with_status_code(400)
                            .with_header(header),
                    );
                }
            }
        }
        _ => {
            let header = Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..]).unwrap();
            let _ = request.respond(
                Response::from_string(r#"{"error":"Not Found"}"#)
                    .with_status_code(404)
                    .with_header(header),
            );
        }
    }
}