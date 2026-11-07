//! Intégration à Windows : démarrage automatique, À propos, arrêt et désinstallation.

use std::ffi::c_void;
use std::time::Duration;

use windows::Win32::Foundation::{ERROR_SUCCESS, HWND, LPARAM, WPARAM};
use windows::Win32::System::Registry::{HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_SZ, RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW};
use windows::Win32::UI::WindowsAndMessaging::{FindWindowW, IDYES, MB_ICONINFORMATION, MB_YESNO, MessageBoxW, PostMessageW, WM_CLOSE};
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
use windows::core::{HSTRING, PCWSTR, w};

use crate::ext;

const RUN_KEY: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
const RUN_VALUE: PCWSTR = w!("Melangeur");
const WEBSITE: PCWSTR = w!("https://github.com/MaTows02/Melangeur");

pub fn autostart_enabled() -> bool {
    unsafe { RegGetValueW(HKEY_CURRENT_USER, RUN_KEY, RUN_VALUE, RRF_RT_REG_SZ, None, None, None) == ERROR_SUCCESS }
}

pub fn set_autostart(enabled: bool) {
    unsafe {
        if !enabled {
            let _ = RegDeleteKeyValueW(HKEY_CURRENT_USER, RUN_KEY, RUN_VALUE);
            return;
        }
        let Ok(exe) = std::env::current_exe() else { return };
        let command: Vec<u16> = format!("\"{}\"", exe.display()).encode_utf16().chain(Some(0)).collect();
        let _ = RegSetKeyValueW(
            HKEY_CURRENT_USER,
            RUN_KEY,
            RUN_VALUE,
            REG_SZ.0,
            Some(command.as_ptr() as *const c_void),
            (command.len() * 2) as u32,
        );
    }
}

pub fn show_about(owner: HWND) {
    let text = HSTRING::from(format!(
        "Melangeur {}\nMélangeur de volume par application et par onglet.\n\n© 2026 MaTows. Tous droits réservés.\n\nOuvrir la page du projet ?",
        env!("CARGO_PKG_VERSION")
    ));
    unsafe {
        if MessageBoxW(Some(owner), &text, w!("À propos de Melangeur"), MB_YESNO | MB_ICONINFORMATION) == IDYES {
            ShellExecuteW(None, w!("open"), WEBSITE, PCWSTR::null(), PCWSTR::null(), SW_SHOWNORMAL);
        }
    }
}

/// Demande à l'instance en cours de se fermer, et attend qu'elle le fasse.
pub fn quit_running_instance() {
    unsafe {
        let find = || FindWindowW(w!("MelangeurFlyout"), w!("Melangeur")).ok();
        let Some(window) = find() else { return };
        let _ = PostMessageW(Some(window), WM_CLOSE, WPARAM(0), LPARAM(0));
        for _ in 0..30 {
            if find().is_none() {
                return;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

/// Appelé par le désinstallateur : retire tout ce que Melangeur a créé pour cet utilisateur.
pub fn uninstall() {
    quit_running_instance();
    set_autostart(false);
    ext::unregister_host();
    ext::stop_hosts();
    if let Some(base) = std::env::var_os("LOCALAPPDATA") {
        let _ = std::fs::remove_dir_all(std::path::Path::new(&base).join("Melangeur"));
    }
}
