#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod db;
mod debug;
mod health;
mod pie_chart;
mod scanner;
mod server;
mod theme;

use app::DiskScopeApp;
use eframe::egui;

fn print_help() {
    println!(
        "DiskScope v2.0.0\n\
         Usage:\n\
           diskscope_v2                        Launch Desktop GUI\n\
           diskscope_v2 --server [OPTIONS]     Launch Headless Web Server Mode\n\n\
         Options:\n\
           --port <PORT>   HTTP listen port (default: 8080)\n\
           --pin <PIN>     Optional PIN required via X-DiskScope-Pin header\n\
           --help          Show this help message"
    );
}

fn main() -> eframe::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let mut headless = false;
    let mut port: u16 = 8080;
    let mut pin = String::new();

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--help" | "-h" => {
                print_help();
                return Ok(());
            }
            "--server" | "--headless" => {
                headless = true;
            }
            "--port" => {
                if i + 1 < args.len() {
                    if let Ok(p) = args[i + 1].parse::<u16>() {
                        port = p;
                    }
                    i += 1;
                }
            }
            "--pin" => {
                if i + 1 < args.len() {
                    pin = args[i + 1].clone();
                    i += 1;
                }
            }
            _ => {}
        }
        i += 1;
    }

    if headless {
        db::init_db();
        if let Err(err) = server::run_blocking(port, pin) {
            eprintln!("[DiskScope Error] {}", err);
            std::process::exit(1);
        }
        return Ok(());
    }

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1180.0, 740.0])
            .with_min_inner_size([840.0, 540.0])
            .with_title("DiskScope v2"),
        ..Default::default()
    };

    eframe::run_native(
        "DiskScope v2",
        options,
        Box::new(|cc| Ok(Box::new(DiskScopeApp::new(cc)))),
    )
}
