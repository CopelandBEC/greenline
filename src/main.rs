mod annot;
mod app;
mod fonts;
mod render;
mod sidecar;

fn main() -> eframe::Result {
    let path = std::env::args_os().nth(1).map(std::path::PathBuf::from);
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_title("Redline").with_app_id("redline").with_inner_size([1400.0, 950.0]).with_drag_and_drop(true),
        ..Default::default()
    };
    eframe::run_native("Redline", options, Box::new(move |cc| Ok(Box::new(app::App::new(cc, path)))))
}
