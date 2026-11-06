//! Interface : icône de notification et flyout dessiné en GDI, sur le modèle d'EarTrumpet.
//! Ce thread ne touche jamais au COM audio : il poste ses demandes au moteur.

use std::ffi::c_void;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Dwm::{DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND, DwmSetWindowAttribute};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoUninitialize};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW};
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{HSTRING, PCWSTR, w};

use crate::app;
use crate::engine::PEAKS_QUEUED;
use crate::ext;
use crate::menu::{self, MenuItem, draw_text, fill, make_font, outline, rgb};
use crate::model::*;
use crate::uia;

const WIDTH: i32 = 360;
const TITLE: i32 = 40;
const DEVICE_ROW: i32 = 50;
const APP_ROW: i32 = 44;
const TAB_ROW: i32 = 34;
const EXTENSION_TAB_ROW: i32 = 50;
const FOOTER: i32 = 6;
const MARGIN: i32 = 12;

// Colonnes communes aux lignes, en points logiques.
const ICON_LEFT: i32 = 16;
const ICON_SIZE: i32 = 24;
const SLIDER_LEFT: i32 = 56;
const SLIDER_RIGHT: i32 = 278;
const NUMBER_RIGHT: i32 = 328;
const ARROW_LEFT: i32 = 330;
const ARROW_SIZE: i32 = 24;
const THUMB_WIDTH: i32 = 8;

const SHOW_TIMER: usize = 1;
const SHOW_FALLBACK_MS: u32 = 300;

const WM_MOUSELEAVE: u32 = 0x02A3;
const NIN_SELECT: u32 = WM_USER;
const NIN_KEYSELECT: u32 = WM_USER + 1;
const GLYPH_VOLUME: &str = "\u{E767}";
const GLYPH_MUTE: &str = "\u{E74F}";
const GLYPH_DOWN: &str = "\u{E70D}";
const GLYPH_UP: &str = "\u{E70E}";
const GLYPH_MICROPHONE: &str = "\u{E720}";

const BACKGROUND: u32 = rgb(0x3C, 0x3C, 0x3C);
const HOVER: u32 = rgb(0x50, 0x50, 0x50);
const TEXT: u32 = rgb(0xFF, 0xFF, 0xFF);
const DIM_TEXT: u32 = rgb(0xC4, 0xC4, 0xC4);
const TRACK: u32 = rgb(0xA8, 0xA8, 0xA8);
const BORDER: u32 = rgb(0x8C, 0x8C, 0x8C);
const METER: u32 = rgb(0xFF, 0xFF, 0xFF);
const DEFAULT_ACCENT: u32 = rgb(0x00, 0x78, 0xD4);

// Commandes du menu. Les périphériques commencent à MENU_DEVICE.
const MENU_OPEN: u32 = 1;
const MENU_QUIT: u32 = 2;
const MENU_MIXER: u32 = 10;
const MENU_PLAYBACK: u32 = 11;
const MENU_RECORDING: u32 = 12;
const MENU_SOUNDS: u32 = 13;
const MENU_SETTINGS: u32 = 14;
const MENU_APPS_VOLUME: u32 = 15;
const MENU_EXTENSION: u32 = 16;
const MENU_AUTOSTART: u32 = 17;
const MENU_ABOUT: u32 = 18;
const MENU_DEVICE: u32 = 100;

#[derive(Clone, Copy, PartialEq)]
enum Item {
    Device(usize),
    App(usize, usize),
    Tab(usize, usize, usize),
    Hint(usize, usize),
}

#[derive(Clone, Copy, PartialEq)]
enum Hit {
    None,
    Row(u32),
    Slider(u32),
    Mute(u32),
    Arrow(u32),
    Tab(usize, usize, usize),
    TabSlider(usize, usize, usize),
    TabMute(usize, usize, usize),
    Hint(usize, usize),
}

struct Ui {
    hwnd: HWND,
    engine: u32,
    taskbar_created: u32,
    tray_icon: HICON,
    devices: Vec<DeviceRow>,
    tabs: Vec<AppTabs>,
    scanned_tabs: Vec<AppTabs>,
    drag_tab: Option<(usize, i64)>,
    /// Navigateurs où l'extension n'est pas connectée : leurs onglets n'ont pas de curseur.
    uncovered: Vec<u32>,
    expanded: Option<u32>,
    icons: Vec<(u32, HICON)>,
    visible: bool,
    pending_show: bool,
    hidden_at: Option<Instant>,
    menu_point: POINT,
    dpi: u32,
    font: HFONT,
    bold_font: HFONT,
    small_font: HFONT,
    number_font: HFONT,
    big_number_font: HFONT,
    glyph_font: HFONT,
    big_glyph_font: HFONT,
    accent: u32,
    work: RECT,
    edge: u32,
    scroll: i32,
    hover: Hit,
    focus: usize,
    drag: Option<u32>,
}

pub fn run(engine: u32) {
    unsafe {
        let com_ok = CoInitializeEx(None, COINIT_APARTMENTTHREADED).is_ok();
        let instance: HINSTANCE = GetModuleHandleW(None).unwrap_or_default().into();
        let class = WNDCLASSW {
            style: CS_DROPSHADOW,
            lpfnWndProc: Some(window_proc),
            hInstance: instance,
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            lpszClassName: w!("MelangeurFlyout"),
            ..Default::default()
        };
        RegisterClassW(&class);

        let mut ui = Box::new(Ui {
            hwnd: HWND::default(),
            engine,
            taskbar_created: RegisterWindowMessageW(w!("TaskbarCreated")),
            tray_icon: make_tray_icon(),
            devices: Vec::new(),
            tabs: Vec::new(),
            scanned_tabs: Vec::new(),
            drag_tab: None,
            uncovered: Vec::new(),
            expanded: None,
            icons: Vec::new(),
            visible: false,
            pending_show: false,
            hidden_at: None,
            menu_point: POINT::default(),
            dpi: 96,
            font: HFONT::default(),
            bold_font: HFONT::default(),
            small_font: HFONT::default(),
            number_font: HFONT::default(),
            big_number_font: HFONT::default(),
            glyph_font: HFONT::default(),
            big_glyph_font: HFONT::default(),
            accent: DEFAULT_ACCENT,
            work: RECT::default(),
            edge: ABE_BOTTOM,
            scroll: 0,
            hover: Hit::None,
            focus: 0,
            drag: None,
        });

        let Ok(hwnd) = CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_LAYERED,
            w!("MelangeurFlyout"),
            w!("Melangeur"),
            WS_POPUP,
            0,
            0,
            WIDTH,
            TITLE,
            None,
            None,
            Some(instance),
            None,
        ) else {
            return;
        };
        ui.hwnd = hwnd;
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, ui.as_mut() as *mut Ui as isize);
        let corner = DWMWCP_ROUND;
        let _ = DwmSetWindowAttribute(hwnd, DWMWA_WINDOW_CORNER_PREFERENCE, &corner as *const _ as *const c_void, 4);
        let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), menu::WINDOW_ALPHA, LWA_ALPHA);
        ui.create_fonts(96);
        ui.add_tray_icon();
        post_engine(engine, WM_E_SET_UI, 0, hwnd.0 as isize);
        ext::start_server(hwnd.0 as isize);

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).0 > 0 {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        ui.free_resources();
        if com_ok {
            CoUninitialize();
        }
    }
}

unsafe extern "system" fn window_proc(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    unsafe {
        let ui = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Ui;
        if ui.is_null() {
            return DefWindowProcW(hwnd, msg, w, l);
        }
        (*ui).on_message(msg, w, l)
    }
}

impl Ui {
    fn px(&self, value: i32) -> i32 {
        value * self.dpi as i32 / 96
    }

    fn on_message(&mut self, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
        unsafe {
            if msg == self.taskbar_created {
                // L'icône a disparu avec l'ancienne barre des tâches : on la recrée.
                self.add_tray_icon();
                return LRESULT(0);
            }
            match msg {
                WM_U_TRAY => self.on_tray((l.0 & 0xFFFF) as u32),
                WM_U_SNAPSHOT => self.on_snapshot(Box::from_raw(l.0 as *mut Vec<DeviceRow>)),
                WM_U_TABS => self.on_tabs(Box::from_raw(l.0 as *mut Vec<AppTabs>)),
                WM_U_DEVICES => self.show_menu(*Box::from_raw(l.0 as *mut Vec<DeviceRow>)),
                WM_U_EXTENSION => {
                    if self.visible {
                        self.rebuild_tabs();
                        self.refresh_layout();
                    }
                }
                WM_U_PEAKS => {
                    PEAKS_QUEUED.store(false, Ordering::Release);
                    if self.visible {
                        self.invalidate();
                    }
                }
                WM_TIMER => {
                    if w.0 == SHOW_TIMER {
                        let _ = KillTimer(Some(self.hwnd), SHOW_TIMER);
                        self.finish_show();
                    }
                }
                WM_ACTIVATE => {
                    if (w.0 & 0xFFFF) as u32 == WA_INACTIVE {
                        self.hide();
                    }
                }
                WM_ERASEBKGND => return LRESULT(1),
                WM_PAINT => self.paint(),
                WM_MOUSEMOVE => self.on_mouse_move(point_from(l)),
                WM_MOUSELEAVE => self.set_hover(Hit::None),
                WM_LBUTTONDOWN => self.on_left_down(point_from(l)),
                WM_LBUTTONUP => {
                    if self.drag.take().is_some() || self.drag_tab.take().is_some() {
                        let _ = ReleaseCapture();
                    }
                }
                WM_MOUSEWHEEL => self.on_wheel(w, l),
                WM_KEYDOWN => self.on_key(VIRTUAL_KEY(w.0 as u16)),
                WM_DESTROY => {
                    self.remove_tray_icon();
                    PostQuitMessage(0);
                }
                _ => return DefWindowProcW(self.hwnd, msg, w, l),
            }
            LRESULT(0)
        }
    }

    // --- Icône de notification et menu ---

    fn tray_data(&self) -> NOTIFYICONDATAW {
        let mut data = NOTIFYICONDATAW {
            cbSize: size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: self.hwnd,
            uID: 1,
            uFlags: NIF_MESSAGE | NIF_ICON | NIF_TIP | NIF_SHOWTIP,
            uCallbackMessage: WM_U_TRAY,
            hIcon: self.tray_icon,
            ..Default::default()
        };
        for (slot, unit) in data.szTip.iter_mut().zip("Melangeur".encode_utf16()) {
            *slot = unit;
        }
        data
    }

    fn add_tray_icon(&self) {
        unsafe {
            let mut data = self.tray_data();
            let _ = Shell_NotifyIconW(NIM_ADD, &data);
            data.Anonymous.uVersion = NOTIFYICON_VERSION_4;
            let _ = Shell_NotifyIconW(NIM_SETVERSION, &data);
        }
    }

    fn remove_tray_icon(&self) {
        unsafe {
            let _ = Shell_NotifyIconW(NIM_DELETE, &self.tray_data());
        }
    }

    fn on_tray(&mut self, event: u32) {
        match event {
            NIN_SELECT | NIN_KEYSELECT => self.toggle(),
            WM_CONTEXTMENU => {
                unsafe {
                    let _ = GetCursorPos(&mut self.menu_point);
                }
                // La liste des périphériques vit dans le moteur : le menu s'ouvre à sa réponse.
                post_engine(self.engine, WM_E_LIST_DEVICES, 0, 0);
            }
            _ => {}
        }
    }

    fn toggle(&mut self) {
        // Cliquer sur l'icône retire d'abord le focus au flyout : ce clic doit le laisser fermé.
        let just_hidden = self.hidden_at.is_some_and(|t| t.elapsed() < Duration::from_millis(250));
        if self.visible {
            self.hide();
        } else if !just_hidden {
            self.show();
        }
    }

    fn show_menu(&mut self, devices: Vec<DeviceRow>) {
        let mut items: Vec<MenuItem> = devices.iter().map(|d| MenuItem::Command {
            id: MENU_DEVICE + d.id,
            text: d.name.clone(),
            checked: d.is_default,
        }).collect();
        if !items.is_empty() {
            items.push(MenuItem::Separator);
        }
        items.push(MenuItem::Submenu {
            text: "Windows Classique".to_string(),
            items: vec![
                command(MENU_MIXER, "Mélangeur de volume Windows"),
                command(MENU_PLAYBACK, "Périphériques de lecture"),
                command(MENU_RECORDING, "Périphériques d'enregistrement"),
                command(MENU_SOUNDS, "Sons"),
                command(MENU_SETTINGS, "Paramètres de son"),
                command(MENU_APPS_VOLUME, "Mélangeur de volume des paramètres"),
            ],
        });
        items.push(command(MENU_EXTENSION, "Installer l'extension (volume par onglet)"));
        let autostart = app::autostart_enabled();
        items.push(MenuItem::Command {
            id: MENU_AUTOSTART,
            text: "Lancer au démarrage de Windows".to_string(),
            checked: autostart,
        });
        items.push(MenuItem::Separator);
        items.push(command(MENU_OPEN, "Ouvrir le mélangeur"));
        items.push(command(MENU_ABOUT, "À propos"));
        items.push(command(MENU_QUIT, "Quitter"));

        let (work, dpi) = self.monitor_at_cursor();
        let choice = menu::run(&items, self.menu_point.x, self.menu_point.y, work, dpi);
        let browser = devices.iter().flat_map(|d| d.apps.iter()).find(|a| uia::is_browser(&a.path)).map(|a| a.path.clone());
        match choice {
            Some(MENU_QUIT) => unsafe {
                let _ = DestroyWindow(self.hwnd);
            },
            Some(MENU_OPEN) => self.show(),
            Some(MENU_EXTENSION) => ext::open_install(browser.as_deref()),
            Some(MENU_AUTOSTART) => app::set_autostart(!autostart),
            Some(MENU_ABOUT) => app::show_about(self.hwnd),
            Some(MENU_MIXER) => shell_open(w!("sndvol.exe"), PCWSTR::null()),
            Some(MENU_PLAYBACK) => control_panel("playback"),
            Some(MENU_RECORDING) => control_panel("recording"),
            Some(MENU_SOUNDS) => control_panel("sounds"),
            Some(MENU_SETTINGS) => shell_open(w!("ms-settings:sound"), PCWSTR::null()),
            Some(MENU_APPS_VOLUME) => shell_open(w!("ms-settings:apps-volume"), PCWSTR::null()),
            Some(id) if id >= MENU_DEVICE => {
                post_engine(self.engine, WM_E_SET_DEFAULT, (id - MENU_DEVICE) as usize, 0);
            }
            _ => {}
        }
    }

    fn monitor_at_cursor(&self) -> (RECT, u32) {
        unsafe {
            let monitor = MonitorFromPoint(self.menu_point, MONITOR_DEFAULTTONEAREST);
            let mut info = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
            let _ = GetMonitorInfoW(monitor, &mut info);
            let (mut dpi_x, mut dpi_y) = (96, 96);
            let _ = GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y);
            (info.rcWork, dpi_x)
        }
    }

    // --- Ouverture et fermeture ---

    /// Prépare l'ouverture. La fenêtre n'apparaît qu'avec les données, pour éviter un saut de taille.
    fn show(&mut self) {
        unsafe {
            let identifier = NOTIFYICONIDENTIFIER {
                cbSize: size_of::<NOTIFYICONIDENTIFIER>() as u32,
                hWnd: self.hwnd,
                uID: 1,
                ..Default::default()
            };
            let monitor = match Shell_NotifyIconGetRect(&identifier) {
                Ok(rect) => MonitorFromRect(&rect, MONITOR_DEFAULTTONEAREST),
                Err(_) => {
                    let mut point = POINT::default();
                    let _ = GetCursorPos(&mut point);
                    MonitorFromPoint(point, MONITOR_DEFAULTTONEAREST)
                }
            };
            let mut info = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
            let _ = GetMonitorInfoW(monitor, &mut info);
            self.work = info.rcWork;

            let mut appbar = APPBARDATA { cbSize: size_of::<APPBARDATA>() as u32, ..Default::default() };
            self.edge = if SHAppBarMessage(ABM_GETTASKBARPOS, &mut appbar) != 0 { appbar.uEdge } else { ABE_BOTTOM };

            let (mut dpi_x, mut dpi_y) = (96, 96);
            let _ = GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y);
            if dpi_x != self.dpi {
                self.create_fonts(dpi_x);
            }
            self.accent = read_accent_color();
            self.scroll = 0;
            self.focus = 0;
            self.visible = true;
            self.pending_show = true;
            ext::set_flyout_visible(true);
            SetTimer(Some(self.hwnd), SHOW_TIMER, SHOW_FALLBACK_MS, None);
        }
        post_engine(self.engine, WM_E_FLYOUT, 1, 0);
    }

    fn finish_show(&mut self) {
        if !self.pending_show {
            return;
        }
        self.pending_show = false;
        unsafe {
            let _ = KillTimer(Some(self.hwnd), SHOW_TIMER);
        }
        self.place_window();
        unsafe {
            let _ = ShowWindow(self.hwnd, SW_SHOW);
            let _ = SetForegroundWindow(self.hwnd);
            let _ = SetFocus(Some(self.hwnd));
        }
    }

    fn hide(&mut self) {
        if !self.visible {
            return;
        }
        self.visible = false;
        self.pending_show = false;
        ext::set_flyout_visible(false);
        self.hidden_at = Some(Instant::now());
        self.expanded = None;
        self.hover = Hit::None;
        self.drag = None;
        self.drag_tab = None;
        self.scanned_tabs = Vec::new();
        unsafe {
            let _ = KillTimer(Some(self.hwnd), SHOW_TIMER);
            let _ = ReleaseCapture();
            let _ = ShowWindow(self.hwnd, SW_HIDE);
        }
        // Libère titres et lignes : en veille, l'appli ne garde pas ces données.
        self.tabs = Vec::new();
        self.devices = Vec::new();
        post_engine(self.engine, WM_E_FLYOUT, 0, 0);
    }

    fn content_height(&self) -> i32 {
        let mut height = self.px(FOOTER);
        self.walk(|item, _| {
            height += self.item_height(item);
            false
        });
        height
    }

    fn item_height(&self, item: Item) -> i32 {
        match item {
            Item::Device(_) => self.px(TITLE + DEVICE_ROW),
            Item::App(..) => self.px(APP_ROW),
            Item::Tab(d, a, t) => {
                let has_slider = self.tabs_of(self.devices[d].apps[a].app)
                    .and_then(|tabs| tabs.get(t))
                    .is_some_and(|tab| tab.extension.is_some());
                self.px(if has_slider { EXTENSION_TAB_ROW } else { TAB_ROW })
            }
            Item::Hint(..) => self.px(TAB_ROW),
        }
    }

    fn place_window(&mut self) {
        let margin = self.px(MARGIN);
        let width = self.px(WIDTH);
        let available = self.work.bottom - self.work.top - margin * 2;
        let content = self.content_height().max(self.px(TITLE));
        let height = content.min(available);
        let x = if self.edge == ABE_LEFT { self.work.left + margin } else { self.work.right - width - margin };
        let y = if self.edge == ABE_TOP { self.work.top + margin } else { self.work.bottom - height - margin };
        self.scroll = self.scroll.clamp(0, (content - height).max(0));
        unsafe {
            let _ = SetWindowPos(self.hwnd, Some(HWND_TOPMOST), x, y, width, height, SWP_NOACTIVATE);
            let _ = InvalidateRect(Some(self.hwnd), None, false);
        }
    }

    // --- Données ---

    fn on_snapshot(&mut self, snapshot: Box<Vec<DeviceRow>>) {
        if !self.visible {
            return;
        }
        // Le curseur en cours de déplacement garde la valeur locale.
        let dragged = self.drag.and_then(|t| self.value_of(t).map(|(volume, _)| (t, volume)));
        self.devices = *snapshot;
        if let Some((target, volume)) = dragged {
            self.set_local_volume(target, volume);
        }
        for d in 0..self.devices.len() {
            for a in 0..self.devices[d].apps.len() {
                self.ensure_icon(d, a);
            }
        }
        self.rebuild_tabs();
        self.refresh_layout();
    }

    fn on_tabs(&mut self, tabs: Box<Vec<AppTabs>>) {
        if !self.visible {
            return;
        }
        self.scanned_tabs = *tabs;
        self.rebuild_tabs();
        self.refresh_layout();
    }

    fn refresh_layout(&mut self) {
        self.collapse_if_needed();
        if self.pending_show {
            self.finish_show();
        } else {
            self.place_window();
        }
    }

    /// L'extension, plus précise, remplace la lecture de la barre d'onglets là où elle est installée.
    fn rebuild_tabs(&mut self) {
        let connections = ext::snapshot();
        let mut result: Vec<AppTabs> = Vec::new();
        let mut uncovered = Vec::new();
        for app in self.devices.iter().flat_map(|d| d.apps.iter()) {
            if result.iter().any(|t| t.app == app.app) {
                continue;
            }
            let mut tabs = Vec::new();
            let mut covered = false;
            for (connection, browser, list) in &connections {
                if !browser.eq_ignore_ascii_case(&app.path) {
                    continue;
                }
                covered = true;
                tabs.extend(list.iter().map(|tab| TabItem {
                    hwnd: 0,
                    title: tab.title.clone(),
                    occurrence: 0,
                    uses_mic: tab.mic,
                    extension: Some(ExtensionTab {
                        connection: *connection,
                        id: tab.id,
                        volume: tab.volume,
                        muted: tab.muted,
                        audible: tab.audible || tab.call,
                        mic: tab.mic,
                    }),
                }));
            }
            if !covered {
                uncovered.push(app.app);
                if let Some(scanned) = self.scanned_tabs.iter().find(|t| t.app == app.app) {
                    tabs = scanned.tabs.clone();
                }
            }
            if !tabs.is_empty() {
                result.push(AppTabs { app: app.app, tabs });
            }
        }
        // Le curseur en cours de déplacement garde la valeur locale.
        if let Some((connection, id)) = self.drag_tab {
            let current = self.find_extension_tab(connection, id).map(|tab| tab.volume);
            if let Some(volume) = current {
                for tab in result.iter_mut().flat_map(|t| t.tabs.iter_mut()) {
                    if let Some(extension) = tab.extension.as_mut().filter(|e| e.connection == connection && e.id == id) {
                        extension.volume = volume;
                    }
                }
            }
        }
        self.tabs = result;
        self.uncovered = uncovered;
    }

    fn find_extension_tab(&self, connection: usize, id: i64) -> Option<ExtensionTab> {
        self.tabs.iter().flat_map(|t| t.tabs.iter())
            .filter_map(|t| t.extension)
            .find(|e| e.connection == connection && e.id == id)
    }

    fn extension_tab(&self, d: usize, a: usize, t: usize) -> Option<ExtensionTab> {
        self.tabs_of(self.devices.get(d)?.apps.get(a)?.app)?.get(t)?.extension
    }

    fn set_tab_volume(&mut self, connection: usize, id: i64, value: f32) {
        let value = value.clamp(0.0, 1.0);
        let mut changed = false;
        for tab in self.tabs.iter_mut().flat_map(|t| t.tabs.iter_mut()) {
            if let Some(extension) = tab.extension.as_mut().filter(|e| e.connection == connection && e.id == id) {
                changed = (extension.volume * 100.0).round() != (value * 100.0).round();
                if changed {
                    extension.volume = value;
                }
            }
        }
        if changed {
            ext::set_tab_volume(connection, id, value);
            self.invalidate();
        }
    }

    fn toggle_tab_mute(&mut self, connection: usize, id: i64) {
        for tab in self.tabs.iter_mut().flat_map(|t| t.tabs.iter_mut()) {
            if let Some(extension) = tab.extension.as_mut().filter(|e| e.connection == connection && e.id == id) {
                extension.muted = !extension.muted;
                ext::set_tab_muted(connection, id, extension.muted);
            }
        }
        self.invalidate();
    }

    /// Plus aucun onglet sonore, plus de flèche : la liste se replie.
    fn collapse_if_needed(&mut self) {
        if let Some(target) = self.expanded {
            if self.tabs_of(target_app(target)).is_none_or(|t| t.is_empty()) {
                self.expanded = None;
            }
        }
    }

    fn tabs_of(&self, app: u32) -> Option<&[TabItem]> {
        self.tabs.iter().find(|t| t.app == app).map(|t| t.tabs.as_slice())
    }

    fn has_arrow(&self, app: u32) -> bool {
        self.tabs_of(app).is_some_and(|t| !t.is_empty())
    }

    fn value_of(&self, target: u32) -> Option<(f32, bool)> {
        let device = self.devices.iter().find(|d| d.id == target_device(target))?;
        if target_app(target) == DEVICE_TARGET {
            return Some((device.volume, device.muted));
        }
        let app = device.apps.iter().find(|a| a.app == target_app(target))?;
        Some((app.volume, app.muted))
    }

    fn set_local_volume(&mut self, target: u32, volume: f32) {
        let Some(device) = self.devices.iter_mut().find(|d| d.id == target_device(target)) else { return };
        if target_app(target) == DEVICE_TARGET {
            device.volume = volume;
        } else if let Some(app) = device.apps.iter_mut().find(|a| a.app == target_app(target)) {
            app.volume = volume;
        }
    }

    fn ensure_icon(&mut self, device: usize, app: usize) {
        let row = &self.devices[device].apps[app];
        let key = row.app;
        if self.icons.iter().any(|(k, _)| *k == key) {
            return;
        }
        let icon = row.aumid.as_deref().map(package_icon).filter(|i| !i.is_invalid())
            .unwrap_or_else(|| file_icon(&row.path));
        self.icons.push((key, icon));
    }

    // --- Actions ---

    fn set_volume(&mut self, target: u32, value: f32) {
        let value = value.clamp(0.0, 1.0);
        let Some((current, _)) = self.value_of(target) else { return };
        if (current * 100.0).round() == (value * 100.0).round() {
            return;
        }
        self.set_local_volume(target, value);
        post_engine(self.engine, WM_E_SET_VOLUME, target as usize, value.to_bits() as isize);
        self.invalidate();
    }

    fn toggle_mute(&mut self, target: u32) {
        let Some((_, muted)) = self.value_of(target) else { return };
        let muted = !muted;
        if let Some(device) = self.devices.iter_mut().find(|d| d.id == target_device(target)) {
            if target_app(target) == DEVICE_TARGET {
                device.muted = muted;
            } else if let Some(app) = device.apps.iter_mut().find(|a| a.app == target_app(target)) {
                app.muted = muted;
            }
        }
        post_engine(self.engine, WM_E_SET_MUTE, target as usize, muted as isize);
        self.invalidate();
    }

    fn toggle_expand(&mut self, target: u32) {
        if target_app(target) == DEVICE_TARGET || !self.has_arrow(target_app(target)) {
            return;
        }
        self.expanded = if self.expanded == Some(target) { None } else { Some(target) };
        self.place_window();
    }

    fn activate_tab(&mut self, device: usize, app: usize, tab: usize) {
        let key = self.devices[device].apps[app].app;
        let Some(item) = self.tabs_of(key).and_then(|t| t.get(tab)) else { return };
        if let Some(extension) = item.extension {
            ext::activate_tab(extension.connection, extension.id);
            self.hide();
            return;
        }
        let hwnd = to_hwnd(item.hwnd);
        uia::select_tab_async(item.hwnd, item.title.clone(), item.occurrence);
        unsafe {
            if IsIconic(hwnd).as_bool() {
                let _ = ShowWindow(hwnd, SW_RESTORE);
            }
            let _ = SetForegroundWindow(hwnd);
        }
        self.hide();
    }

    fn slider_value(&self, x: i32) -> f32 {
        let (left, right) = (self.px(SLIDER_LEFT), self.px(SLIDER_RIGHT));
        (x - left) as f32 / (right - left) as f32
    }

    fn scroll_by(&mut self, delta: i32) {
        let mut client = RECT::default();
        unsafe {
            let _ = GetClientRect(self.hwnd, &mut client);
        }
        let limit = (self.content_height() - client.bottom).max(0);
        let scroll = (self.scroll + delta).clamp(0, limit);
        if scroll != self.scroll {
            self.scroll = scroll;
            self.invalidate();
        }
    }

    // --- Souris et clavier ---

    fn on_mouse_move(&mut self, point: POINT) {
        if let Some((connection, id)) = self.drag_tab {
            let value = self.slider_value(point.x);
            self.set_tab_volume(connection, id, value);
            return;
        }
        if let Some(target) = self.drag {
            let value = self.slider_value(point.x);
            self.set_volume(target, value);
            return;
        }
        unsafe {
            let mut track = TRACKMOUSEEVENT {
                cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
                dwFlags: TME_LEAVE,
                hwndTrack: self.hwnd,
                dwHoverTime: 0,
            };
            let _ = TrackMouseEvent(&mut track);
        }
        let hit = self.hit_test(point);
        self.set_hover(hit);
    }

    fn set_hover(&mut self, hit: Hit) {
        if self.hover != hit {
            self.hover = hit;
            self.invalidate();
        }
    }

    fn on_left_down(&mut self, point: POINT) {
        match self.hit_test(point) {
            Hit::Slider(target) => {
                self.drag = Some(target);
                unsafe { SetCapture(self.hwnd) };
                let value = self.slider_value(point.x);
                self.set_volume(target, value);
            }
            Hit::Mute(target) => self.toggle_mute(target),
            Hit::Arrow(target) => self.toggle_expand(target),
            Hit::Tab(device, app, tab) => self.activate_tab(device, app, tab),
            Hit::TabSlider(device, app, tab) => {
                if let Some(extension) = self.extension_tab(device, app, tab) {
                    self.drag_tab = Some((extension.connection, extension.id));
                    unsafe { SetCapture(self.hwnd) };
                    let value = self.slider_value(point.x);
                    self.set_tab_volume(extension.connection, extension.id, value);
                }
            }
            Hit::TabMute(device, app, tab) => {
                if let Some(extension) = self.extension_tab(device, app, tab) {
                    self.toggle_tab_mute(extension.connection, extension.id);
                }
            }
            Hit::Hint(device, app) => {
                let browser = self.devices.get(device).and_then(|d| d.apps.get(app)).map(|a| a.path.clone());
                self.hide();
                ext::open_install(browser.as_deref());
            }
            Hit::Row(_) | Hit::None => {}
        }
    }

    /// La molette règle le volume sous le curseur, comme EarTrumpet ; ailleurs, elle fait défiler.
    fn on_wheel(&mut self, w: WPARAM, l: LPARAM) {
        let delta = ((w.0 >> 16) & 0xFFFF) as u16 as i16 as i32;
        let mut point = point_from(l);
        unsafe {
            let _ = ScreenToClient(self.hwnd, &mut point);
        }
        match self.hit_test(point) {
            Hit::Slider(target) | Hit::Row(target) | Hit::Mute(target) | Hit::Arrow(target) => {
                if let Some((volume, _)) = self.value_of(target) {
                    self.set_volume(target, volume + 0.02 * delta as f32 / 120.0);
                }
            }
            Hit::TabSlider(d, a, t) | Hit::TabMute(d, a, t) => {
                if let Some(extension) = self.extension_tab(d, a, t) {
                    let value = extension.volume + 0.02 * delta as f32 / 120.0;
                    self.set_tab_volume(extension.connection, extension.id, value);
                }
            }
            _ => self.scroll_by(-delta * self.px(TAB_ROW) / 120),
        }
    }

    fn targets(&self) -> Vec<u32> {
        let mut targets = Vec::new();
        for device in &self.devices {
            targets.push(target(device.id, DEVICE_TARGET));
            targets.extend(device.apps.iter().map(|a| target(device.id, a.app)));
        }
        targets
    }

    fn on_key(&mut self, key: VIRTUAL_KEY) {
        let targets = self.targets();
        let current = targets.get(self.focus).copied();
        match key {
            VK_ESCAPE => self.hide(),
            VK_UP => {
                self.focus = self.focus.saturating_sub(1);
                self.invalidate();
            }
            VK_DOWN => {
                self.focus = (self.focus + 1).min(targets.len().saturating_sub(1));
                self.invalidate();
            }
            VK_LEFT | VK_RIGHT => {
                if let Some((target, (volume, _))) = current.and_then(|t| self.value_of(t).map(|v| (t, v))) {
                    self.set_volume(target, volume + if key == VK_LEFT { -0.02 } else { 0.02 });
                }
            }
            VK_M => {
                if let Some(target) = current {
                    self.toggle_mute(target);
                }
            }
            VK_RETURN | VK_SPACE => {
                if let Some(target) = current {
                    self.toggle_expand(target);
                }
            }
            _ => {}
        }
    }

    /// Parcourt les éléments dans l'ordre d'affichage, sans allocation.
    /// `visit` renvoie vrai pour arrêter le parcours.
    fn walk(&self, mut visit: impl FnMut(Item, i32) -> bool) {
        let mut y = -self.scroll;
        for (d, device) in self.devices.iter().enumerate() {
            if visit(Item::Device(d), y) {
                return;
            }
            y += self.item_height(Item::Device(d));
            for (a, app) in device.apps.iter().enumerate() {
                if visit(Item::App(d, a), y) {
                    return;
                }
                y += self.item_height(Item::App(d, a));
                if self.expanded == Some(target(device.id, app.app)) {
                    for t in 0..self.tabs_of(app.app).map_or(0, |tabs| tabs.len()) {
                        if visit(Item::Tab(d, a, t), y) {
                            return;
                        }
                        y += self.item_height(Item::Tab(d, a, t));
                    }
                    if self.uncovered.contains(&app.app) {
                        if visit(Item::Hint(d, a), y) {
                            return;
                        }
                        y += self.item_height(Item::Hint(d, a));
                    }
                }
            }
        }
    }

    fn hit_test(&self, point: POINT) -> Hit {
        let mut hit = Hit::None;
        self.walk(|item, top| {
            if point.y < top || point.y >= top + self.item_height(item) {
                return false;
            }
            let x = point.x;
            hit = match item {
                Item::Device(d) => {
                    let id = target(self.devices[d].id, DEVICE_TARGET);
                    if point.y < top + self.px(TITLE) {
                        Hit::None
                    } else {
                        self.row_hit(x, id, false)
                    }
                }
                Item::App(d, a) => {
                    let (device, app) = (&self.devices[d], &self.devices[d].apps[a]);
                    self.row_hit(x, target(device.id, app.app), self.has_arrow(app.app))
                }
                Item::Hint(d, a) => Hit::Hint(d, a),
                Item::Tab(d, a, t) => {
                    // Le titre ouvre l'onglet ; en dessous, le bouton muet et le curseur.
                    if self.extension_tab(d, a, t).is_none() || point.y < top + self.px(24) {
                        Hit::Tab(d, a, t)
                    } else if x < self.px(SLIDER_LEFT) - self.px(8) {
                        Hit::TabMute(d, a, t)
                    } else if x <= self.px(SLIDER_RIGHT) + self.px(8) {
                        Hit::TabSlider(d, a, t)
                    } else {
                        Hit::None
                    }
                }
            };
            true
        });
        hit
    }

    fn row_hit(&self, x: i32, target: u32, has_arrow: bool) -> Hit {
        if x < self.px(SLIDER_LEFT) - self.px(8) {
            Hit::Mute(target)
        } else if x <= self.px(SLIDER_RIGHT) + self.px(8) {
            Hit::Slider(target)
        } else if has_arrow && x >= self.px(ARROW_LEFT) {
            Hit::Arrow(target)
        } else {
            Hit::Row(target)
        }
    }

    fn invalidate(&self) {
        unsafe {
            let _ = InvalidateRect(Some(self.hwnd), None, false);
        }
    }

    // --- Dessin ---

    fn paint(&self) {
        unsafe {
            let mut ps = PAINTSTRUCT::default();
            let screen = BeginPaint(self.hwnd, &mut ps);
            let mut client = RECT::default();
            let _ = GetClientRect(self.hwnd, &mut client);

            // Double tampon, pour éviter le scintillement du vumètre.
            let hdc = CreateCompatibleDC(Some(screen));
            let bitmap = CreateCompatibleBitmap(screen, client.right, client.bottom);
            let old_bitmap = SelectObject(hdc, bitmap.into());
            let old_font = SelectObject(hdc, self.font.into());
            SetBkMode(hdc, TRANSPARENT);
            SelectObject(hdc, GetStockObject(DC_BRUSH));
            SelectObject(hdc, GetStockObject(NULL_PEN));
            fill(hdc, client, BACKGROUND);

            let targets = self.targets();
            let focused = targets.get(self.focus).copied();
            self.walk(|item, top| {
                let height = self.item_height(item);
                if top + height > 0 && top < client.bottom {
                    match item {
                        Item::Device(d) => self.paint_device(hdc, d, top),
                        Item::App(d, a) => self.paint_app(hdc, d, a, top, focused),
                        Item::Tab(d, a, t) => self.paint_tab(hdc, d, a, t, top),
                        Item::Hint(d, a) => self.paint_hint(hdc, d, a, top),
                    }
                }
                false
            });
            outline(hdc, client, BORDER);

            let _ = BitBlt(screen, 0, 0, client.right, client.bottom, Some(hdc), 0, 0, SRCCOPY);
            SelectObject(hdc, old_font);
            SelectObject(hdc, old_bitmap);
            let _ = DeleteObject(bitmap.into());
            let _ = DeleteDC(hdc);
            let _ = EndPaint(self.hwnd, &ps);
        }
    }

    /// Curseur EarTrumpet : piste fine, partie remplie, vumètre blanc épais, poignée verticale.
    fn paint_slider(&self, hdc: HDC, center: i32, volume: f32, peak: f32, muted: bool, on_accent: bool) {
        let (left, right) = (self.px(SLIDER_LEFT), self.px(SLIDER_RIGHT));
        let volume = volume.clamp(0.0, 1.0);
        let thumb_half = self.px(THUMB_WIDTH) / 2;
        let thumb_x = left + thumb_half + ((right - left - thumb_half * 2) as f32 * volume) as i32;
        let line = self.px(1).max(1);
        let thumb_color = lighten(self.accent, 0.22);
        let filled = if muted { DIM_TEXT } else { thumb_color };
        let track = if on_accent { rgb(0xE8, 0xE8, 0xE8) } else { TRACK };

        fill(hdc, rect(left, center - line, right, center + line), track);
        fill(hdc, rect(left, center - line, thumb_x, center + line), filled);

        // Comme EarTrumpet : la longueur du vumètre suit le pic et le volume réglé.
        if !muted && peak > 0.0 {
            let width = ((right - left) as f32 * peak.clamp(0.0, 1.0) * volume) as i32;
            let half = self.px(2).max(2);
            fill(hdc, rect(left, center - half, left + width, center + half), METER);
        }

        let height = self.px(if on_accent { 13 } else { 11 });
        let radius = self.px(4);
        unsafe {
            SetDCBrushColor(hdc, COLORREF(if muted { DIM_TEXT } else { thumb_color }));
            let _ = RoundRect(hdc, thumb_x - thumb_half, center - height, thumb_x + thumb_half + 1, center + height + 1, radius, radius);
        }
    }

    fn paint_device(&self, hdc: HDC, index: usize, top: i32) {
        let device = &self.devices[index];
        let width = self.px(WIDTH);
        let bottom = top + self.px(TITLE + DEVICE_ROW);
        fill(hdc, rect(0, top, width, bottom), self.accent);

        unsafe { SelectObject(hdc, self.bold_font.into()) };
        draw_text(
            hdc,
            &device.name,
            rect(self.px(14), top, width - self.px(12), top + self.px(TITLE)),
            TEXT,
            DT_LEFT,
        );

        let center = top + self.px(TITLE) + self.px(DEVICE_ROW) / 2 - self.px(2);
        unsafe { SelectObject(hdc, self.big_glyph_font.into()) };
        let glyph = if device.muted { GLYPH_MUTE } else { GLYPH_VOLUME };
        draw_text(
            hdc,
            glyph,
            rect(self.px(ICON_LEFT - 2), center - self.px(16), self.px(SLIDER_LEFT - 4), center + self.px(16)),
            TEXT,
            DT_LEFT,
        );

        let peak = f32::from_bits(device.peak.load(Ordering::Relaxed));
        self.paint_slider(hdc, center, device.volume, peak, device.muted, true);

        unsafe { SelectObject(hdc, self.big_number_font.into()) };
        draw_text(
            hdc,
            &percent(device.volume),
            rect(self.px(SLIDER_RIGHT + 4), center - self.px(16), self.px(NUMBER_RIGHT + 18), center + self.px(16)),
            TEXT,
            DT_RIGHT,
        );
        unsafe { SelectObject(hdc, self.font.into()) };
    }

    fn paint_app(&self, hdc: HDC, d: usize, a: usize, top: i32, focused: Option<u32>) {
        let device = &self.devices[d];
        let app = &device.apps[a];
        let id = target(device.id, app.app);
        let width = self.px(WIDTH);
        let bottom = top + self.px(APP_ROW);
        let hovered = matches!(self.hover, Hit::Row(t) | Hit::Slider(t) | Hit::Mute(t) | Hit::Arrow(t) if t == id);
        if hovered || (focused == Some(id) && self.drag.is_none() && self.focus > 0) {
            fill(hdc, rect(0, top, width, bottom), HOVER);
        }
        let center = top + self.px(APP_ROW) / 2 + self.px(3);

        // Icône, ou initiale si l'application n'en fournit pas.
        let icon = self.icons.iter().find(|(k, _)| *k == app.app).map(|(_, i)| *i).unwrap_or_default();
        let (icon_x, icon_y, icon_size) = (self.px(ICON_LEFT), center - self.px(ICON_SIZE) / 2, self.px(ICON_SIZE));
        unsafe {
            if !icon.is_invalid() {
                let _ = DrawIconEx(hdc, icon_x, icon_y, icon, icon_size, icon_size, 0, None, DI_NORMAL);
            } else {
                SelectObject(hdc, self.bold_font.into());
                let initial: String = app.name.chars().take(1).flat_map(char::to_uppercase).collect();
                draw_text(hdc, &initial, rect(icon_x, icon_y, icon_x + icon_size, icon_y + icon_size), DIM_TEXT, DT_CENTER);
            }
            if app.muted {
                // Pastille de mute sur l'icône.
                let badge = self.px(14);
                let (x, y) = (icon_x + icon_size - badge + self.px(3), icon_y + icon_size - badge + self.px(3));
                fill(hdc, rect(x, y, x + badge, y + badge), BACKGROUND);
                SelectObject(hdc, self.small_glyph_font());
                draw_text(hdc, GLYPH_MUTE, rect(x, y, x + badge, y + badge), TEXT, DT_CENTER);
            }
        }

        // Le nom n'apparaît qu'au survol, pour garder des lignes aussi sobres que sur EarTrumpet.
        if hovered {
            unsafe { SelectObject(hdc, self.small_font.into()) };
            draw_text(
                hdc,
                &app.name,
                rect(self.px(SLIDER_LEFT), top + self.px(1), self.px(NUMBER_RIGHT), top + self.px(15)),
                TEXT,
                DT_LEFT,
            );
        }

        let peak = f32::from_bits(app.peak.load(Ordering::Relaxed));
        self.paint_slider(hdc, center, app.volume, peak, app.muted, false);

        unsafe { SelectObject(hdc, self.number_font.into()) };
        draw_text(
            hdc,
            &percent(app.volume),
            rect(self.px(SLIDER_RIGHT + 4), center - self.px(14), self.px(NUMBER_RIGHT), center + self.px(14)),
            TEXT,
            DT_RIGHT,
        );

        // La flèche n'existe que si un onglet de cette appli joue du son.
        if self.has_arrow(app.app) {
            unsafe { SelectObject(hdc, self.glyph_font.into()) };
            let glyph = if self.expanded == Some(id) { GLYPH_UP } else { GLYPH_DOWN };
            let half = self.px(ARROW_SIZE) / 2;
            draw_text(
                hdc,
                glyph,
                rect(self.px(ARROW_LEFT), center - half, self.px(ARROW_LEFT + ARROW_SIZE), center + half),
                TEXT,
                DT_CENTER,
            );
        }
        unsafe { SelectObject(hdc, self.font.into()) };
    }

    fn paint_tab(&self, hdc: HDC, d: usize, a: usize, t: usize, top: i32) {
        let app = &self.devices[d].apps[a];
        let Some(tab) = self.tabs_of(app.app).and_then(|tabs| tabs.get(t)) else { return };
        let width = self.px(WIDTH);
        if let Some(extension) = tab.extension {
            self.paint_extension_tab(hdc, d, a, t, top, &tab.title, extension);
            return;
        }
        let bottom = top + self.px(TAB_ROW);
        if self.hover == Hit::Tab(d, a, t) {
            fill(hdc, rect(0, top, width, bottom), HOVER);
        }
        unsafe { SelectObject(hdc, self.small_glyph_font()) };
        draw_text(
            hdc,
            GLYPH_VOLUME,
            rect(self.px(ICON_LEFT + 8), top, self.px(SLIDER_LEFT), bottom - self.px(4)),
            DIM_TEXT,
            DT_LEFT,
        );
        unsafe { SelectObject(hdc, self.font.into()) };
        let title = if tab.title.is_empty() { "(sans titre)" } else { tab.title.as_str() };
        draw_text(
            hdc,
            title,
            rect(self.px(SLIDER_LEFT), top, width - self.px(if tab.uses_mic { 36 } else { 12 }), bottom - self.px(6)),
            TEXT,
            DT_LEFT,
        );
        if tab.uses_mic {
            unsafe { SelectObject(hdc, self.glyph_font.into()) };
            draw_text(
                hdc,
                GLYPH_MICROPHONE,
                rect(width - self.px(32), top, width - self.px(10), bottom - self.px(6)),
                TEXT,
                DT_CENTER,
            );
            unsafe { SelectObject(hdc, self.font.into()) };
        }

        // Windows ne donne qu'un niveau par appli : tous ses onglets sonores partagent ce niveau.
        let (left, right) = (self.px(SLIDER_LEFT), width - self.px(16));
        let y = bottom - self.px(6);
        let line = self.px(1).max(1);
        fill(hdc, rect(left, y, right, y + line), TRACK);
        if !app.muted {
            let peak = f32::from_bits(app.peak.load(Ordering::Relaxed));
            let level = ((right - left) as f32 * peak.clamp(0.0, 1.0) * app.volume.clamp(0.0, 1.0)) as i32;
            fill(hdc, rect(left, y - line, left + level, y + line * 2), METER);
        }
    }

    /// Sans extension, le navigateur ne laisse pas régler un onglet : on l'explique, cliquable.
    fn paint_hint(&self, hdc: HDC, d: usize, a: usize, top: i32) {
        let width = self.px(WIDTH);
        let bottom = top + self.px(TAB_ROW);
        if self.hover == Hit::Hint(d, a) {
            fill(hdc, rect(0, top, width, bottom), HOVER);
        }
        unsafe { SelectObject(hdc, self.small_font.into()) };
        draw_text(
            hdc,
            "Volume par onglet : cliquer pour installer l'extension",
            rect(self.px(SLIDER_LEFT), top, width - self.px(12), bottom),
            lighten(self.accent, 0.5),
            DT_LEFT,
        );
        unsafe { SelectObject(hdc, self.font.into()) };
    }

    /// Onglet géré par l'extension : titre cliquable, bouton muet, vrai curseur et micro éventuel.
    fn paint_extension_tab(&self, hdc: HDC, d: usize, a: usize, t: usize, top: i32, title: &str, tab: ExtensionTab) {
        let app = &self.devices[d].apps[a];
        let width = self.px(WIDTH);
        let bottom = top + self.px(EXTENSION_TAB_ROW);
        let hovered = matches!(self.hover, Hit::Tab(x, y, z) | Hit::TabSlider(x, y, z) | Hit::TabMute(x, y, z) if (x, y, z) == (d, a, t));
        if hovered {
            fill(hdc, rect(0, top, width, bottom), HOVER);
        }
        let center = top + self.px(35);

        unsafe { SelectObject(hdc, self.small_font.into()) };
        let title = if title.is_empty() { "(sans titre)" } else { title };
        draw_text(hdc, title, rect(self.px(SLIDER_LEFT), top + self.px(3), width - self.px(12), top + self.px(23)), TEXT, DT_LEFT);

        unsafe { SelectObject(hdc, self.glyph_font.into()) };
        draw_text(
            hdc,
            if tab.muted { GLYPH_MUTE } else { GLYPH_VOLUME },
            rect(self.px(ICON_LEFT + 10), center - self.px(12), self.px(SLIDER_LEFT - 6), center + self.px(12)),
            if tab.muted { DIM_TEXT } else { TEXT },
            DT_LEFT,
        );
        if tab.mic {
            draw_text(
                hdc,
                GLYPH_MICROPHONE,
                rect(self.px(ARROW_LEFT), center - self.px(12), self.px(ARROW_LEFT + ARROW_SIZE), center + self.px(12)),
                TEXT,
                DT_CENTER,
            );
        }

        // Windows ne donne qu'un niveau par appli : le vumètre suit ce niveau, réduit au volume de l'onglet.
        let peak = if tab.audible && !app.muted { f32::from_bits(app.peak.load(Ordering::Relaxed)) } else { 0.0 };
        self.paint_slider(hdc, center, tab.volume, peak, tab.muted, false);

        unsafe { SelectObject(hdc, self.font.into()) };
        draw_text(
            hdc,
            &percent(tab.volume),
            rect(self.px(SLIDER_RIGHT + 4), center - self.px(12), self.px(NUMBER_RIGHT), center + self.px(12)),
            DIM_TEXT,
            DT_RIGHT,
        );
    }

    // --- Ressources ---

    fn small_glyph_font(&self) -> HGDIOBJ {
        self.glyph_font.into()
    }

    fn create_fonts(&mut self, dpi: u32) {
        self.free_fonts();
        self.dpi = dpi;
        self.font = make_font(-self.px(14), 400, w!("Segoe UI"));
        self.bold_font = make_font(-self.px(15), 500, w!("Segoe UI"));
        self.small_font = make_font(-self.px(11), 400, w!("Segoe UI"));
        self.number_font = make_font(-self.px(17), 400, w!("Segoe UI"));
        self.big_number_font = make_font(-self.px(24), 400, w!("Segoe UI"));
        self.glyph_font = make_font(-self.px(11), 400, w!("Segoe MDL2 Assets"));
        self.big_glyph_font = make_font(-self.px(20), 400, w!("Segoe MDL2 Assets"));
    }

    fn free_fonts(&mut self) {
        let fonts = [
            self.font,
            self.bold_font,
            self.small_font,
            self.number_font,
            self.big_number_font,
            self.glyph_font,
            self.big_glyph_font,
        ];
        for font in fonts {
            if !font.is_invalid() {
                unsafe {
                    let _ = DeleteObject(font.into());
                }
            }
        }
    }

    fn free_resources(&mut self) {
        self.free_fonts();
        unsafe {
            for (_, icon) in &self.icons {
                if !icon.is_invalid() {
                    let _ = DestroyIcon(*icon);
                }
            }
            let _ = DestroyIcon(self.tray_icon);
        }
    }
}

fn command(id: u32, text: &str) -> MenuItem {
    MenuItem::Command { id, text: text.to_string(), checked: false }
}

fn shell_open(file: PCWSTR, parameters: PCWSTR) {
    unsafe {
        ShellExecuteW(None, w!("open"), file, parameters, PCWSTR::null(), SW_SHOWNORMAL);
    }
}

fn control_panel(page: &str) {
    let parameters = HSTRING::from(format!("shell32.dll,Control_RunDLL mmsys.cpl,,{page}"));
    shell_open(w!("rundll32.exe"), PCWSTR(parameters.as_ptr()));
}

fn percent(volume: f32) -> String {
    format!("{}", (volume * 100.0).round() as i32)
}

fn lighten(color: u32, amount: f32) -> u32 {
    let channel = |shift: u32| {
        let value = ((color >> shift) & 0xFF) as f32;
        ((value + (255.0 - value) * amount) as u32) << shift
    };
    channel(0) | channel(8) | channel(16)
}

fn point_from(l: LPARAM) -> POINT {
    POINT { x: (l.0 & 0xFFFF) as u16 as i16 as i32, y: ((l.0 >> 16) & 0xFFFF) as u16 as i16 as i32 }
}

fn rect(left: i32, top: i32, right: i32, bottom: i32) -> RECT {
    RECT { left, top, right, bottom }
}

fn file_icon(path: &str) -> HICON {
    // « Sons système » n'a pas d'exécutable : on prend l'icône du service audio, comme EarTrumpet.
    let source = if path.is_empty() { "%SystemRoot%\\System32\\AudioSrv.dll".to_string() } else { path.to_string() };
    let source = source.replace("%SystemRoot%", &std::env::var("SystemRoot").unwrap_or_default());
    let mut large = HICON::default();
    let extracted = unsafe { ExtractIconExW(&HSTRING::from(source), 0, Some(&mut large), None, 1) };
    if extracted > 0 { large } else { HICON::default() }
}

/// Icône d'une appli du Store : elle n'est pas dans l'exécutable, mais dans le paquet.
fn package_icon(aumid: &str) -> HICON {
    unsafe {
        let factory: windows::core::Result<IShellItemImageFactory> =
            SHCreateItemInKnownFolder(&FOLDERID_AppsFolder, KF_FLAG_DONT_VERIFY, &HSTRING::from(aumid));
        let Ok(factory) = factory else { return HICON::default() };
        let Ok(bitmap) = factory.GetImage(SIZE { cx: 48, cy: 48 }, SIIGBF_ICONONLY) else { return HICON::default() };
        let mask_bits = [0u8; 48 * 48 / 8];
        let mask = CreateBitmap(48, 48, 1, 1, Some(mask_bits.as_ptr() as *const c_void));
        let info = ICONINFO { fIcon: true.into(), hbmMask: mask, hbmColor: bitmap, ..Default::default() };
        let icon = CreateIconIndirect(&info).unwrap_or_default();
        let _ = DeleteObject(bitmap.into());
        let _ = DeleteObject(mask.into());
        icon
    }
}

fn read_dword(key: PCWSTR, name: PCWSTR) -> Option<u32> {
    let mut value = 0u32;
    let mut size = 4u32;
    let result = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key,
            name,
            RRF_RT_REG_DWORD,
            None,
            Some(&mut value as *mut u32 as *mut c_void),
            Some(&mut size),
        )
    };
    (result == ERROR_SUCCESS).then_some(value)
}

/// Couleur d'accentuation de Windows, stockée au format 0xAABBGGRR.
fn read_accent_color() -> u32 {
    read_dword(w!("Software\\Microsoft\\Windows\\DWM"), w!("AccentColor"))
        .map_or(DEFAULT_ACCENT, |color| color & 0x00FF_FFFF)
}

/// Dessine l'icône en mémoire (trois curseurs) : aucune ressource à embarquer.
fn make_tray_icon() -> HICON {
    const SIZE: i32 = 32;
    let light_taskbar = read_dword(
        w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"),
        w!("SystemUsesLightTheme"),
    )
    .unwrap_or(0)
        != 0;
    let color: u32 = if light_taskbar { 0xFF20_2020 } else { 0xFFFF_FFFF };
    unsafe {
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: SIZE,
                biHeight: -SIZE,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits: *mut c_void = std::ptr::null_mut();
        let Ok(color_bitmap) = CreateDIBSection(None, &info, DIB_RGB_COLORS, &mut bits, None, 0) else {
            return HICON::default();
        };
        let pixels = std::slice::from_raw_parts_mut(bits as *mut u32, (SIZE * SIZE) as usize);
        for (column_x, knob_y) in [(6, 20), (15, 9), (24, 15)] {
            for y in 3..29i32 {
                for x in (column_x - 1)..(column_x + 2) {
                    pixels[(y * SIZE + x) as usize] = color;
                }
            }
            for y in (knob_y - 3)..(knob_y + 4) {
                for x in (column_x - 4)..(column_x + 5) {
                    pixels[(y * SIZE + x) as usize] = color;
                }
            }
        }
        let mask_bits = [0u8; (SIZE * SIZE / 8) as usize];
        let mask_bitmap = CreateBitmap(SIZE, SIZE, 1, 1, Some(mask_bits.as_ptr() as *const c_void));
        let icon_info = ICONINFO { fIcon: true.into(), hbmMask: mask_bitmap, hbmColor: color_bitmap, ..Default::default() };
        let icon = CreateIconIndirect(&icon_info).unwrap_or_default();
        let _ = DeleteObject(color_bitmap.into());
        let _ = DeleteObject(mask_bitmap.into());
        icon
    }
}
