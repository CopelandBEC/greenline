// No console window behind the app on Windows release builds.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod annot;
mod app;
mod fonts;
mod render;
mod sidecar;
mod stamps;

fn main() -> eframe::Result {
    let path = std::env::args_os().nth(1).map(std::path::PathBuf::from);
    let icon = egui::IconData { rgba: include_bytes!("../assets/icon128.rgba").to_vec(), width: 128, height: 128 };
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Redline")
            .with_app_id("redline")
            .with_icon(icon)
            .with_inner_size([1400.0, 950.0])
            .with_drag_and_drop(true),
        ..Default::default()
    };
    eframe::run_native("Redline", options, Box::new(move |cc| Ok(Box::new(app::App::new(cc, path)))))
}
