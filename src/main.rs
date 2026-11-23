// En release no abre una consola negra detrás de la ventana.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod collectors;
mod config;
mod gui;
mod gui_plans;
mod model;
mod portfolio;
mod pricing;
mod stats;

use eframe::egui;

pub const AUTHOR: &str = "Julio Carrizales";

fn main() -> eframe::Result<()> {
    let icon = eframe::icon_data::from_png_bytes(include_bytes!("../assets/app-icon.png")).expect("ícono válido");
    let snap_mode = std::env::var_os("TOKENSCOP_SNAP").is_some();
    let mut vp = egui::ViewportBuilder::default()
        .with_title("Tokenscopio")
        .with_icon(icon)
        .with_inner_size(if snap_mode { [1280.0, 680.0] } else { [1320.0, 880.0] })
        .with_min_inner_size([960.0, 640.0]);
    if !snap_mode {
        vp = vp.with_maximized(true);
    }
    let options = eframe::NativeOptions { viewport: vp, ..Default::default() };
    eframe::run_native("Tokenscopio", options, Box::new(|cc| Ok(Box::new(gui::App::new(cc)))))
}
