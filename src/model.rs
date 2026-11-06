//! Données et messages échangés entre les threads. Aucun appel COM ici.

use std::sync::Arc;
use std::sync::atomic::AtomicU32;

use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, PostThreadMessageW, WM_APP};

// Messages vers le thread moteur (file de messages du thread, sans fenêtre).
pub const WM_E_SET_UI: u32 = WM_APP + 1; // lParam = HWND de l'interface
pub const WM_E_SESSION_CREATED: u32 = WM_APP + 2; // wParam = id du périphérique, lParam = IAudioSessionControl brut
pub const WM_E_SESSION_DIRTY: u32 = WM_APP + 3; // wParam = id de session
pub const WM_E_DEVICES_CHANGED: u32 = WM_APP + 4;
pub const WM_E_PROCESS_EXIT: u32 = WM_APP + 5; // wParam = pid
pub const WM_E_SET_VOLUME: u32 = WM_APP + 6; // wParam = cible, lParam = bits du f32
pub const WM_E_SET_MUTE: u32 = WM_APP + 7; // wParam = cible, lParam = 0 ou 1
pub const WM_E_FLYOUT: u32 = WM_APP + 8; // wParam = 1 ouvert, 0 fermé
pub const WM_E_DEVICE_DIRTY: u32 = WM_APP + 9; // wParam = id du périphérique
pub const WM_E_LIST_DEVICES: u32 = WM_APP + 10;
pub const WM_E_SET_DEFAULT: u32 = WM_APP + 11; // wParam = id du périphérique

// Messages vers l'interface.
pub const WM_U_SNAPSHOT: u32 = WM_APP + 20; // lParam = Box<Vec<DeviceRow>>
pub const WM_U_TABS: u32 = WM_APP + 21; // lParam = Box<Vec<AppTabs>>
pub const WM_U_TRAY: u32 = WM_APP + 22;
pub const WM_U_PEAKS: u32 = WM_APP + 23;
pub const WM_U_DEVICES: u32 = WM_APP + 24; // lParam = Box<Vec<DeviceRow>>, pour le menu
pub const WM_U_EXTENSION: u32 = WM_APP + 25; // L'extension a envoyé un nouvel état

/// Partie « appli » d'une cible désignant le volume du périphérique lui-même.
pub const DEVICE_TARGET: u32 = 0xFFFF;

/// Une cible de volume : un périphérique, ou une appli sur ce périphérique.
pub const fn target(device: u32, app: u32) -> u32 {
    (device << 16) | (app & 0xFFFF)
}
pub const fn target_device(target: u32) -> u32 {
    target >> 16
}
pub const fn target_app(target: u32) -> u32 {
    target & 0xFFFF
}

/// Une ligne du mélangeur : toutes les sessions d'un même exécutable sur un périphérique.
pub struct AppRow {
    pub app: u32,
    pub name: String,
    pub path: String,
    pub aumid: Option<String>,
    pub volume: f32,
    pub muted: bool,
    pub peak: Arc<AtomicU32>,
}

pub struct DeviceRow {
    pub id: u32,
    pub name: String,
    pub is_default: bool,
    pub volume: f32,
    pub muted: bool,
    pub peak: Arc<AtomicU32>,
    pub apps: Vec<AppRow>,
}

/// Un onglet qui joue du son actuellement.
#[derive(Clone)]
pub struct TabItem {
    pub hwnd: isize,
    pub title: String,
    /// Rang parmi les onglets sonores de même titre dans cette fenêtre.
    pub occurrence: usize,
    /// L'onglet utilise le micro : souvent un appel, même si personne ne parle à cet instant.
    pub uses_mic: bool,
    /// Présent si l'extension gère cet onglet : son volume est alors réglable.
    pub extension: Option<ExtensionTab>,
}

#[derive(Clone, Copy)]
pub struct ExtensionTab {
    pub connection: usize,
    pub id: i64,
    pub volume: f32,
    pub muted: bool,
    pub audible: bool,
    pub mic: bool,
}

#[derive(Clone)]
pub struct AppTabs {
    pub app: u32,
    pub tabs: Vec<TabItem>,
}

pub fn to_hwnd(value: isize) -> HWND {
    HWND(value as *mut core::ffi::c_void)
}

pub fn post_engine(thread_id: u32, msg: u32, w: usize, l: isize) -> bool {
    unsafe { PostThreadMessageW(thread_id, msg, WPARAM(w), LPARAM(l)).is_ok() }
}

pub fn post_ui(ui: isize, msg: u32) -> bool {
    unsafe { PostMessageW(Some(to_hwnd(ui)), msg, WPARAM(0), LPARAM(0)).is_ok() }
}

/// Transfère un objet à l'interface ; il est libéré si le message n'a pas pu partir.
pub fn post_ui_box<T>(ui: isize, msg: u32, value: Box<T>) {
    let raw = Box::into_raw(value);
    let sent = unsafe { PostMessageW(Some(to_hwnd(ui)), msg, WPARAM(0), LPARAM(raw as isize)).is_ok() };
    if !sent {
        drop(unsafe { Box::from_raw(raw) });
    }
}
