// En release no debe abrirse una consola detrás de la ventana.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    localify_app::run();
}
