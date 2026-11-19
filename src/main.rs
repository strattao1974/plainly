#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod collect;
mod dns;
mod eventlog;
mod http;
mod icon_art;
mod ipc;
mod journal;
mod kb;
mod net;
mod netwatch;
mod perf;
mod pulse;
mod settings;
mod shared;
mod slow;
mod snapshot;
mod theme;
mod tray;
mod update;
mod watcher;
mod winutil;

use eframe::egui;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use tray::TrayCmd;

// Two roles from one exe: by default the tiny background watcher that owns the tray icon;
// with --window, the window, which fully exits when closed so its memory goes back to Windows.
fn main() -> eframe::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let page = args.iter().position(|a| a == "--page").and_then(|i| args.get(i + 1)).cloned();
    let dir = winutil::data_dir();
    let _ = std::fs::create_dir_all(&dir);

    if args.iter().any(|a| a == "--window") {
        return run_window(dir, page);
    }

    let background = args.iter().any(|a| a == "--background");
    if !winutil::claim_single_instance("Local\\Plainly.Agent") {
        if !background {
            ipc::send_to_agent("open");
        }
        return Ok(());
    }
    let shared = Arc::new(shared::Shared::load(dir));
    watcher::spawn(shared.clone());
    pulse::spawn(shared.clone());
    netwatch::spawn(shared.clone());
    update::spawn(shared.clone());
    if !background {
        ipc::open_window(page.as_deref());
    }
    let s = shared.clone();
    tray::run(move |cmd| match cmd {
        TrayCmd::Open => ipc::open_window(None),
        TrayCmd::Timeline => ipc::open_window(Some("timeline")),
        TrayCmd::Slow => ipc::open_window(Some("slow")),
        TrayCmd::Internet => ipc::open_window(Some("internet")),
        TrayCmd::Scan => s.scan_now.store(true, Ordering::SeqCst),
        TrayCmd::Hint => tray::notify(
            "Plainly is still keeping watch",
            "It lives by the clock now. Click the icon any time to open it, or right-click to quit.",
        ),
        TrayCmd::Quit => {
            tray::remove();
            std::process::exit(0);
        }
    });
    Ok(())
}

fn run_window(dir: PathBuf, page: Option<String>) -> eframe::Result<()> {
    if !winutil::claim_single_instance("Local\\Plainly.Window") {
        ipc::send_to_window(&page.map(|p| format!("page:{p}")).unwrap_or_else(|| "open".into()));
        return Ok(());
    }
    let icon = egui::IconData { rgba: icon_art::render(64), width: 64, height: 64 };
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Plainly")
            .with_inner_size([1060.0, 740.0])
            .with_min_inner_size([780.0, 540.0])
            .with_icon(Arc::new(icon)),
        ..Default::default()
    };
    eframe::run_native("Plainly", options, Box::new(move |cc| Ok(Box::new(app::PlainlyApp::new(cc, dir, page)))))
}
