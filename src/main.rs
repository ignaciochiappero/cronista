#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use cronista::engine::stt::current_ram_mb;
use cronista::ui::app::CronistaApp;

fn load_app_icon() -> Option<egui::IconData> {
    let bytes = include_bytes!("../assets/cronista-256.png");
    let img = image::load_from_memory(bytes).ok()?.to_rgba8();
    let (width, height) = img.dimensions();
    Some(egui::IconData {
        rgba: img.into_raw(),
        width,
        height,
    })
}

fn main() -> eframe::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--benchmark-ram") {
        let ram = current_ram_mb();
        println!("cronista-rs idle RAM (headless): {ram:.2} MB");
        return Ok(());
    }

    let mut viewport = egui::ViewportBuilder::default()
        .with_title("Cronista")
        .with_decorations(false)
        .with_inner_size([1140.0, 748.0])
        .with_min_inner_size([920.0, 600.0])
        .with_drag_and_drop(true);
    if let Some(icon) = load_app_icon() {
        viewport = viewport.with_icon(icon);
    }

    let options = eframe::NativeOptions {
        viewport,
        centered: true,
        persist_window: false,
        ..Default::default()
    };

    eframe::run_native(
        "Cronista",
        options,
        Box::new(|cc| Ok(Box::new(CronistaApp::new(cc).expect("init CronistaApp")))),
    )
}
