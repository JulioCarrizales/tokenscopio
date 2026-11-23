// En release no abre una consola negra detrás de la ventana.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod collectors;
mod gui;
mod model;
mod stats;

use eframe::egui;

pub const AUTHOR: &str = "Julio Carrizales";

fn main() -> eframe::Result<()> {
    let icon = eframe::icon_data::from_png_bytes(include_bytes!("../assets/app-icon.png")).expect("ícono válido");
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Tokenscopio")
            .with_icon(icon)
            .with_inner_size([1320.0, 880.0])
            .with_maximized(true)
            .with_min_inner_size([960.0, 640.0]),
        ..Default::default()
    };
    eframe::run_native("Tokenscopio", options, Box::new(|cc| Ok(Box::new(gui::App::new(cc)))))
}
