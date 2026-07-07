use image::GenericImageView;

pub fn app_icon() -> egui::IconData {
    let img = image::load_from_memory(include_bytes!("../res/icon.png"))
        .expect("Failed to decode icon.png");
    let (w, h) = img.dimensions();
    let rgba = img.into_rgba8().into_raw();
    egui::IconData { rgba, width: w, height: h }
}

pub fn tray_icon() -> tray_icon::Icon {
    let img = image::load_from_memory(include_bytes!("../res/icon.png"))
        .expect("Failed to decode icon.png");
    let (w, h) = img.dimensions();
    let rgba = img.into_rgba8().into_raw();
    tray_icon::Icon::from_rgba(rgba, w, h).expect("Failed to create tray icon")
}
