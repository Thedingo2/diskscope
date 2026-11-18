use std::sync::{Mutex, OnceLock};

fn log_buffer() -> &'static Mutex<Vec<String>> {
    static LOGS: OnceLock<Mutex<Vec<String>>> = OnceLock::new();
    LOGS.get_or_init(|| Mutex::new(Vec::new()))
}

pub fn log(msg: impl Into<String>) {
    let line = msg.into();
    println!("{}", line);
    if let Ok(mut buf) = log_buffer().lock() {
        if buf.len() >= 500 {
            buf.remove(0);
        }
        buf.push(line);
    }
}

pub fn get_logs() -> Vec<String> {
    log_buffer()
        .lock()
        .map(|buf| buf.clone())
        .unwrap_or_default()
}

pub fn clear_logs() {
    if let Ok(mut buf) = log_buffer().lock() {
        buf.clear();
    }
}
