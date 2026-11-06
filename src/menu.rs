//! Menu du clic droit, dessiné comme le flyout. Une boucle de messages locale le rend modal.

use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::{SetFocus, TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent, VK_ESCAPE};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{PCWSTR, w};

pub enum MenuItem {
    Command { id: u32, text: String, checked: bool },
    Submenu { text: String, items: Vec<MenuItem> },
    Separator,
}

const WIDTH: i32 = 390;
const ROW: i32 = 34;
const SEPARATOR: i32 = 13;
const PADDING: i32 = 6;
const WM_MOUSELEAVE: u32 = 0x02A3;

pub const TEXT: u32 = rgb(0xF4, 0xF4, 0xF4);
pub const BACKGROUND: u32 = rgb(0x4A, 0x4A, 0x4A);
pub const HOVER: u32 = rgb(0x62, 0x62, 0x62);
pub const BORDER: u32 = rgb(0xB0, 0xB0, 0xB0);
pub const WINDOW_ALPHA: u8 = 238;

pub const fn rgb(r: u32, g: u32, b: u32) -> u32 {
    r | (g << 8) | (b << 16)
}

struct MenuState {
    items: *const [MenuItem],
    hwnd: HWND,
    dpi: i32,
    work: RECT,
    font: HFONT,
    glyphs: HFONT,
    hover: Option<usize>,
    result: Option<u32>,
    done: bool,
    in_submenu: bool,
}

pub fn make_font(height: i32, weight: i32, face: PCWSTR) -> HFONT {
    unsafe {
        CreateFontW(
            height, 0, 0, 0, weight, 0, 0, 0, DEFAULT_CHARSET, OUT_DEFAULT_PRECIS, CLIP_DEFAULT_PRECIS,
            CLEARTYPE_QUALITY, 0, face,
        )
    }
}

pub fn fill(hdc: HDC, area: RECT, color: u32) {
    // Le pinceau DC est partagé : aucune création d'objet GDI par rectangle.
    unsafe {
        SetDCBrushColor(hdc, COLORREF(color));
        FillRect(hdc, &area, HBRUSH(GetStockObject(DC_BRUSH).0));
    }
}

pub fn draw_text(hdc: HDC, text: &str, mut area: RECT, color: u32, align: DRAW_TEXT_FORMAT) {
    let mut wide: Vec<u16> = text.encode_utf16().collect();
    unsafe {
        SetTextColor(hdc, COLORREF(color));
        DrawTextW(hdc, &mut wide, &mut area, align | DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS | DT_NOPREFIX);
    }
}

pub fn outline(hdc: HDC, area: RECT, color: u32) {
    fill(hdc, RECT { bottom: area.top + 1, ..area }, color);
    fill(hdc, RECT { top: area.bottom - 1, ..area }, color);
    fill(hdc, RECT { right: area.left + 1, ..area }, color);
    fill(hdc, RECT { left: area.right - 1, ..area }, color);
}

fn scale(value: i32, dpi: i32) -> i32 {
    value * dpi / 96
}

fn item_height(item: &MenuItem, dpi: i32) -> i32 {
    match item {
        MenuItem::Separator => scale(SEPARATOR, dpi),
        _ => scale(ROW, dpi),
    }
}

fn register_class() {
    unsafe {
        let class = WNDCLASSW {
            style: CS_DROPSHADOW,
            lpfnWndProc: Some(menu_proc),
            hInstance: GetModuleHandleW(None).unwrap_or_default().into(),
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            lpszClassName: w!("MelangeurMenu"),
            ..Default::default()
        };
        RegisterClassW(&class);
    }
}

/// Affiche le menu avec son coin bas-droit vers (x, y), et renvoie l'id de la commande choisie.
pub fn run(items: &[MenuItem], x: i32, y: i32, work: RECT, dpi: u32) -> Option<u32> {
    register_class();
    let dpi = dpi as i32;
    let width = scale(WIDTH, dpi);
    let height: i32 = items.iter().map(|i| item_height(i, dpi)).sum::<i32>() + scale(PADDING, dpi) * 2;
    let left = (x - width).clamp(work.left, (work.right - width).max(work.left));
    let top = (y - height).clamp(work.top, (work.bottom - height).max(work.top));

    let mut state = Box::new(MenuState {
        items: items as *const [MenuItem],
        hwnd: HWND::default(),
        dpi,
        work,
        font: make_font(-scale(15, dpi), 400, w!("Segoe UI")),
        glyphs: make_font(-scale(12, dpi), 400, w!("Segoe MDL2 Assets")),
        hover: None,
        result: None,
        done: false,
        in_submenu: false,
    });

    unsafe {
        let instance: HINSTANCE = GetModuleHandleW(None).unwrap_or_default().into();
        let Ok(hwnd) = CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_LAYERED,
            w!("MelangeurMenu"),
            w!("Menu"),
            WS_POPUP,
            left,
            top,
            width,
            height,
            None,
            None,
            Some(instance),
            None,
        ) else {
            free_fonts(&state);
            return None;
        };
        state.hwnd = hwnd;
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, state.as_mut() as *mut MenuState as isize);
        let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), WINDOW_ALPHA, LWA_ALPHA);
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = SetForegroundWindow(hwnd);
        let _ = SetFocus(Some(hwnd));

        let mut msg = MSG::default();
        while !state.done {
            let code = GetMessageW(&mut msg, None, 0, 0).0;
            if code <= 0 {
                PostQuitMessage(msg.wParam.0 as i32);
                break;
            }
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
        let _ = DestroyWindow(hwnd);
    }
    free_fonts(&state);
    state.result
}

fn free_fonts(state: &MenuState) {
    unsafe {
        let _ = DeleteObject(state.font.into());
        let _ = DeleteObject(state.glyphs.into());
    }
}

unsafe extern "system" fn menu_proc(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    unsafe {
        let state = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut MenuState;
        if state.is_null() {
            return DefWindowProcW(hwnd, msg, w, l);
        }
        let state = &mut *state;
        match msg {
            WM_ERASEBKGND => return LRESULT(1),
            WM_PAINT => paint(state),
            WM_ACTIVATE => {
                if (w.0 & 0xFFFF) as u32 == WA_INACTIVE && !state.in_submenu {
                    state.done = true;
                }
            }
            WM_KEYDOWN => {
                if w.0 as u16 == VK_ESCAPE.0 {
                    state.done = true;
                }
            }
            WM_MOUSEMOVE => {
                let mut track = TRACKMOUSEEVENT {
                    cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
                    dwFlags: TME_LEAVE,
                    hwndTrack: hwnd,
                    dwHoverTime: 0,
                };
                let _ = TrackMouseEvent(&mut track);
                let hit = hit_test(state, ((l.0 >> 16) & 0xFFFF) as u16 as i16 as i32);
                set_hover(state, hit);
            }
            WM_MOUSELEAVE => set_hover(state, None),
            WM_LBUTTONUP => on_click(state, ((l.0 >> 16) & 0xFFFF) as u16 as i16 as i32),
            _ => return DefWindowProcW(hwnd, msg, w, l),
        }
        LRESULT(0)
    }
}

fn set_hover(state: &mut MenuState, hit: Option<usize>) {
    if state.hover != hit {
        state.hover = hit;
        unsafe {
            let _ = InvalidateRect(Some(state.hwnd), None, false);
        }
    }
}

fn hit_test(state: &MenuState, y: i32) -> Option<usize> {
    let items = unsafe { &*state.items };
    let mut top = scale(PADDING, state.dpi);
    for (i, item) in items.iter().enumerate() {
        let height = item_height(item, state.dpi);
        if y >= top && y < top + height {
            return (!matches!(item, MenuItem::Separator)).then_some(i);
        }
        top += height;
    }
    None
}

fn on_click(state: &mut MenuState, y: i32) {
    let items = unsafe { &*state.items };
    let Some(index) = hit_test(state, y) else { return };
    match &items[index] {
        MenuItem::Command { id, .. } => {
            state.result = Some(*id);
            state.done = true;
        }
        MenuItem::Submenu { items: children, .. } => {
            let mut rect = RECT::default();
            unsafe {
                let _ = GetWindowRect(state.hwnd, &mut rect);
            }
            let mut top = rect.top + scale(PADDING, state.dpi);
            for item in items.iter().take(index) {
                top += item_height(item, state.dpi);
            }
            let height: i32 = children.iter().map(|i| item_height(i, state.dpi)).sum::<i32>() + scale(PADDING, state.dpi) * 2;
            state.in_submenu = true;
            let choice = run(children, rect.left, top + height, state.work, state.dpi as u32);
            state.in_submenu = false;
            match choice {
                Some(id) => {
                    state.result = Some(id);
                    state.done = true;
                }
                None => {
                    // Fermé par un clic ailleurs : le menu parent se ferme aussi.
                    let foreground = unsafe { GetForegroundWindow() };
                    if foreground != state.hwnd {
                        state.done = true;
                    }
                }
            }
        }
        MenuItem::Separator => {}
    }
}

fn paint(state: &MenuState) {
    let items = unsafe { &*state.items };
    unsafe {
        let mut ps = PAINTSTRUCT::default();
        let screen = BeginPaint(state.hwnd, &mut ps);
        let mut client = RECT::default();
        let _ = GetClientRect(state.hwnd, &mut client);
        let hdc = CreateCompatibleDC(Some(screen));
        let bitmap = CreateCompatibleBitmap(screen, client.right, client.bottom);
        let old_bitmap = SelectObject(hdc, bitmap.into());
        let old_font = SelectObject(hdc, state.font.into());
        SetBkMode(hdc, TRANSPARENT);
        fill(hdc, client, BACKGROUND);

        let mut top = scale(PADDING, state.dpi);
        let margin = scale(16, state.dpi);
        let text_left = scale(44, state.dpi);
        for (i, item) in items.iter().enumerate() {
            let height = item_height(item, state.dpi);
            let row = RECT { left: 0, top, right: client.right, bottom: top + height };
            match item {
                MenuItem::Separator => {
                    let y = top + height / 2;
                    fill(hdc, RECT { left: margin, top: y, right: client.right - margin, bottom: y + 1 }, BORDER);
                }
                MenuItem::Command { text, checked, .. } => {
                    if state.hover == Some(i) {
                        fill(hdc, row, HOVER);
                    }
                    if *checked {
                        SelectObject(hdc, state.glyphs.into());
                        draw_text(hdc, "\u{E73E}", RECT { left: margin, right: text_left, ..row }, TEXT, DT_LEFT);
                        SelectObject(hdc, state.font.into());
                    }
                    draw_text(hdc, text, RECT { left: text_left, right: client.right - margin, ..row }, TEXT, DT_LEFT);
                }
                MenuItem::Submenu { text, .. } => {
                    if state.hover == Some(i) {
                        fill(hdc, row, HOVER);
                    }
                    draw_text(hdc, text, RECT { left: text_left, right: client.right - margin * 2, ..row }, TEXT, DT_LEFT);
                    SelectObject(hdc, state.glyphs.into());
                    draw_text(hdc, "\u{E76C}", RECT { left: text_left, right: client.right - margin, ..row }, TEXT, DT_RIGHT);
                    SelectObject(hdc, state.font.into());
                }
            }
            top += height;
        }
        outline(hdc, client, BORDER);

        let _ = BitBlt(screen, 0, 0, client.right, client.bottom, Some(hdc), 0, 0, SRCCOPY);
        SelectObject(hdc, old_font);
        SelectObject(hdc, old_bitmap);
        let _ = DeleteObject(bitmap.into());
        let _ = DeleteDC(hdc);
        let _ = EndPaint(state.hwnd, &ps);
    }
}
