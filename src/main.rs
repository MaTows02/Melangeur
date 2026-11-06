#![windows_subsystem = "windows"]

mod app;
mod engine;
mod ext;
mod menu;
mod model;
mod ui;
mod uia;

use windows::Win32::Foundation::{ERROR_ALREADY_EXISTS, GetLastError};
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext};
use windows::Win32::UI::WindowsAndMessaging::WM_QUIT;
use windows::core::w;

fn main() {
    // Lancé par le navigateur pour l'extension : simple relais, sans interface.
    if ext::is_host_invocation() {
        ext::run_host();
        return;
    }
    // Commandes de l'installateur.
    match std::env::args().nth(1).as_deref() {
        Some("--quit") => return app::quit_running_instance(),
        Some("--uninstall") => return app::uninstall(),
        _ => {}
    }
    unsafe {
        // Une seule instance : le mutex vit jusqu'à la fin du processus.
        let _instance = CreateMutexW(None, false, w!("Local\\Melangeur"));
        if GetLastError() == ERROR_ALREADY_EXISTS {
            return;
        }
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        ext::register_host();

        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        let engine = std::thread::spawn(move || engine::run(ready_tx));
        let Ok(engine_thread) = ready_rx.recv() else { return };

        ui::run(engine_thread);

        model::post_engine(engine_thread, WM_QUIT, 0, 0);
        let _ = engine.join();
    }
}
