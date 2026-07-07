mod app;
mod db;
mod download;
mod feed;
mod icon;
mod opml;
mod playback;
mod sync;
mod types;

use std::path::PathBuf;

use app::RuSStlyApp;

fn data_dir() -> PathBuf {
    let xdg = std::env::var("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
            PathBuf::from(home).join(".local/share")
        });
    xdg.join("russtly")
}

fn main() -> eframe::Result<()> {
    let data_path = data_dir();
    std::fs::create_dir_all(&data_path).ok();

    let rt = tokio::runtime::Runtime::new().expect("Failed to create tokio runtime");
    let _guard = rt.enter();

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_icon(icon::app_icon())
            .with_inner_size(egui::vec2(1100.0, 750.0))
            .with_title("RuSStly — Podcast Client"),
        ..Default::default()
    };

    eframe::run_native(
        "RuSStly",
        options,
        Box::new(|_cc| Box::new(RuSStlyApp::new())),
    )
}
