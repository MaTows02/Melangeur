//! Lien avec l'extension de navigateur.
//!
//! Le navigateur lance `melangeur.exe` comme hôte de messagerie native (mode `run_host`).
//! Cet hôte relaie les messages vers l'instance principale par un canal nommé local.
//! Côté principal, un thread par connexion dort sur une lecture : aucun réveil sans événement.

use std::ffi::c_void;
use std::io::{Read, Write};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::time::Duration;

use serde_json::{Value, json};
use windows::Win32::Foundation::*;
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAG_OVERLAPPED, FILE_FLAGS_AND_ATTRIBUTES, FILE_GENERIC_READ, FILE_GENERIC_WRITE, FILE_SHARE_NONE,
    OPEN_EXISTING, PIPE_ACCESS_DUPLEX, ReadFile, WriteFile,
};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::IO::{GetOverlappedResult, OVERLAPPED};
use windows::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES, PIPE_WAIT,
};
use windows::Win32::System::Registry::{HKEY_CURRENT_USER, REG_SZ, RegDeleteKeyW, RegSetKeyValueW};
use windows::Win32::System::Threading::{CreateEventW, GetCurrentProcessId, OpenProcess, PROCESS_TERMINATE, TerminateProcess};
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
use windows::core::{HSTRING, PCWSTR, w};

use crate::model::*;

pub const EXTENSION_ID: &str = "deagjjohcecifdjdandbplddmfmpikjg";
/// Identifiant attribué par le Chrome Web Store, à renseigner après le premier envoi.
const STORE_EXTENSION_ID: Option<&str> = None;
const HOST_NAME: &str = "com.melangeur.host";
const PIPE_NAME: PCWSTR = w!("\\\\.\\pipe\\melangeur-extension");
const BUFFER_SIZE: u32 = 65536;
const MAX_FRAME: usize = 1024 * 1024;
/// L'extension voyage dans le .exe : elle ne dépend pas de l'emplacement du projet.
const EXTENSION_FILES: [(&str, &[u8]); 7] = [
    ("manifest.json", include_bytes!("../extension/manifest.json")),
    ("background.js", include_bytes!("../extension/background.js")),
    ("bridge.js", include_bytes!("../extension/bridge.js")),
    ("page.js", include_bytes!("../extension/page.js")),
    ("icon16.png", include_bytes!("../extension/icon16.png")),
    ("icon48.png", include_bytes!("../extension/icon48.png")),
    ("icon128.png", include_bytes!("../extension/icon128.png")),
];
const REGISTRY_ROOTS: [&str; 3] = [
    "Software\\Google\\Chrome\\NativeMessagingHosts",
    "Software\\BraveSoftware\\Brave-Browser\\NativeMessagingHosts",
    "Software\\Microsoft\\Edge\\NativeMessagingHosts",
];

#[derive(Clone)]
pub struct ExtTab {
    pub id: i64,
    pub title: String,
    pub audible: bool,
    pub muted: bool,
    pub volume: f32,
    pub mic: bool,
    pub call: bool,
}

struct Connection {
    id: usize,
    handle: isize,
    browser: String,
    tabs: Vec<ExtTab>,
}

static CONNECTIONS: Mutex<Vec<Connection>> = Mutex::new(Vec::new());
static UI_WINDOW: AtomicIsize = AtomicIsize::new(0);
static FLYOUT_VISIBLE: AtomicBool = AtomicBool::new(false);

fn to_handle(value: isize) -> HANDLE {
    HANDLE(value as *mut c_void)
}

/// Lecture ou écriture bloquante sur un canal ouvert en mode asynchrone,
/// pour pouvoir lire et écrire en même temps sur le même canal.
fn transfer(handle: HANDLE, buffer: &mut [u8], write: bool) -> Option<usize> {
    unsafe {
        let event = CreateEventW(None, true, false, None).ok()?;
        let mut overlapped = OVERLAPPED { hEvent: event, ..Default::default() };
        let result = if write {
            WriteFile(handle, Some(buffer), None, Some(&mut overlapped))
        } else {
            ReadFile(handle, Some(buffer), None, Some(&mut overlapped))
        };
        let pending = matches!(&result, Err(e) if e.code() == ERROR_IO_PENDING.to_hresult());
        let mut count = 0u32;
        let done = (result.is_ok() || pending) && GetOverlappedResult(handle, &overlapped, &mut count, true).is_ok();
        let _ = CloseHandle(event);
        done.then_some(count as usize)
    }
}

fn read_exact(handle: HANDLE, buffer: &mut [u8]) -> bool {
    let mut offset = 0;
    while offset < buffer.len() {
        match transfer(handle, &mut buffer[offset..], false) {
            Some(count) if count > 0 => offset += count,
            _ => return false,
        }
    }
    true
}

fn write_all(handle: HANDLE, data: &[u8]) -> bool {
    let mut copy = data.to_vec();
    let mut offset = 0;
    while offset < copy.len() {
        match transfer(handle, &mut copy[offset..], true) {
            Some(count) if count > 0 => offset += count,
            _ => return false,
        }
    }
    true
}

fn read_frame(handle: HANDLE) -> Option<Vec<u8>> {
    let mut size = [0u8; 4];
    if !read_exact(handle, &mut size) {
        return None;
    }
    let size = u32::from_le_bytes(size) as usize;
    if size > MAX_FRAME {
        return None;
    }
    let mut data = vec![0u8; size];
    read_exact(handle, &mut data).then_some(data)
}

fn write_frame(handle: HANDLE, data: &[u8]) -> bool {
    let mut frame = Vec::with_capacity(data.len() + 4);
    frame.extend_from_slice(&(data.len() as u32).to_le_bytes());
    frame.extend_from_slice(data);
    write_all(handle, &frame)
}

// --- Instance principale ---

pub fn start_server(ui: isize) {
    UI_WINDOW.store(ui, Ordering::Relaxed);
    std::thread::spawn(|| {
        let mut next_id = 1;
        loop {
            let pipe = unsafe {
                CreateNamedPipeW(
                    PIPE_NAME,
                    FILE_FLAGS_AND_ATTRIBUTES(PIPE_ACCESS_DUPLEX.0 | FILE_FLAG_OVERLAPPED.0),
                    PIPE_TYPE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                    PIPE_UNLIMITED_INSTANCES,
                    BUFFER_SIZE,
                    BUFFER_SIZE,
                    0,
                    None,
                )
            };
            if pipe.is_invalid() {
                return;
            }
            if !wait_for_client(pipe) {
                unsafe {
                    let _ = CloseHandle(pipe);
                }
                continue;
            }
            let id = next_id;
            next_id += 1;
            let raw = pipe.0 as isize;
            std::thread::spawn(move || serve_connection(id, raw));
        }
    });
}

fn wait_for_client(pipe: HANDLE) -> bool {
    unsafe {
        let Ok(event) = CreateEventW(None, true, false, None) else { return false };
        let mut overlapped = OVERLAPPED { hEvent: event, ..Default::default() };
        let result = ConnectNamedPipe(pipe, Some(&mut overlapped));
        let mut connected = result.is_ok();
        if let Err(error) = &result {
            if error.code() == ERROR_PIPE_CONNECTED.to_hresult() {
                connected = true;
            } else if error.code() == ERROR_IO_PENDING.to_hresult() {
                let mut ignored = 0;
                connected = GetOverlappedResult(pipe, &overlapped, &mut ignored, true).is_ok();
            }
        }
        let _ = CloseHandle(event);
        connected
    }
}

fn serve_connection(id: usize, raw: isize) {
    let handle = to_handle(raw);
    if let Ok(mut connections) = CONNECTIONS.lock() {
        connections.push(Connection { id, handle: raw, browser: String::new(), tabs: Vec::new() });
    }
    while let Some(frame) = read_frame(handle) {
        let Ok(message) = serde_json::from_slice::<Value>(&frame) else { continue };
        if let Ok(mut connections) = CONNECTIONS.lock() {
            let Some(connection) = connections.iter_mut().find(|c| c.id == id) else { break };
            if let Some(browser) = message["browser"].as_str() {
                connection.browser = browser.to_string();
            }
            if let Some(tabs) = message["tabs"].as_array() {
                connection.tabs = tabs.iter().filter_map(parse_tab).collect();
            }
        }
        notify_ui();
    }
    if let Ok(mut connections) = CONNECTIONS.lock() {
        connections.retain(|c| c.id != id);
    }
    unsafe {
        let _ = CloseHandle(handle);
    }
    notify_ui();
}

fn parse_tab(value: &Value) -> Option<ExtTab> {
    Some(ExtTab {
        id: value["id"].as_i64()?,
        title: value["title"].as_str().unwrap_or_default().to_string(),
        audible: value["audible"].as_bool().unwrap_or(false),
        muted: value["muted"].as_bool().unwrap_or(false),
        volume: value["volume"].as_f64().unwrap_or(1.0) as f32,
        mic: value["mic"].as_bool().unwrap_or(false),
        call: value["call"].as_bool().unwrap_or(false),
    })
}

fn notify_ui() {
    let ui = UI_WINDOW.load(Ordering::Relaxed);
    if ui != 0 && FLYOUT_VISIBLE.load(Ordering::Relaxed) {
        post_ui(ui, WM_U_EXTENSION);
    }
}

pub fn set_flyout_visible(visible: bool) {
    FLYOUT_VISIBLE.store(visible, Ordering::Relaxed);
}

/// Les onglets signalés, par connexion : (id de connexion, navigateur, onglets).
pub fn snapshot() -> Vec<(usize, String, Vec<ExtTab>)> {
    CONNECTIONS.lock().map_or(Vec::new(), |connections| {
        connections.iter().map(|c| (c.id, c.browser.clone(), c.tabs.clone())).collect()
    })
}

/// Vrai si l'extension couvre ce navigateur : inutile alors de lire sa barre d'onglets.
pub fn covers(path: &str) -> bool {
    CONNECTIONS.lock().is_ok_and(|connections| connections.iter().any(|c| c.browser.eq_ignore_ascii_case(path)))
}

fn send(connection: usize, command: Value) {
    let Ok(data) = serde_json::to_vec(&command) else { return };
    if let Ok(connections) = CONNECTIONS.lock() {
        if let Some(found) = connections.iter().find(|c| c.id == connection) {
            write_frame(to_handle(found.handle), &data);
        }
    }
}

pub fn set_tab_volume(connection: usize, tab: i64, value: f32) {
    send(connection, json!({ "cmd": "volume", "tab": tab, "value": value }));
}

pub fn set_tab_muted(connection: usize, tab: i64, muted: bool) {
    send(connection, json!({ "cmd": "mute", "tab": tab, "value": muted }));
}

pub fn activate_tab(connection: usize, tab: i64) {
    send(connection, json!({ "cmd": "activate", "tab": tab }));
}

/// Déclare l'hôte aux navigateurs, pour cet utilisateur uniquement (aucun droit administrateur).
pub fn register_host() {
    let Ok(exe) = std::env::current_exe() else { return };
    let Some(base) = std::env::var_os("LOCALAPPDATA") else { return };
    let folder = std::path::Path::new(&base).join("Melangeur");
    if std::fs::create_dir_all(&folder).is_err() {
        return;
    }
    let extension = folder.join("extension");
    if std::fs::create_dir_all(&extension).is_ok() {
        for (name, content) in EXTENSION_FILES {
            let _ = std::fs::write(extension.join(name), content);
        }
    }
    let manifest_path = folder.join(format!("{HOST_NAME}.json"));
    let mut origins = vec![format!("chrome-extension://{EXTENSION_ID}/")];
    origins.extend(STORE_EXTENSION_ID.map(|id| format!("chrome-extension://{id}/")));
    let manifest = json!({
        "name": HOST_NAME,
        "description": "Lien entre Melangeur et son extension",
        "path": exe,
        "type": "stdio",
        "allowed_origins": origins,
    });
    let Ok(text) = serde_json::to_vec_pretty(&manifest) else { return };
    if std::fs::write(&manifest_path, text).is_err() {
        return;
    }
    let value: Vec<u16> = manifest_path.as_os_str().to_string_lossy().encode_utf16().chain(Some(0)).collect();
    for root in REGISTRY_ROOTS {
        let key = HSTRING::from(format!("{root}\\{HOST_NAME}"));
        unsafe {
            let _ = RegSetKeyValueW(
                HKEY_CURRENT_USER,
                &key,
                PCWSTR::null(),
                REG_SZ.0,
                Some(value.as_ptr() as *const c_void),
                (value.len() * 2) as u32,
            );
        }
    }
}

/// Ouvre ce qu'il faut pour charger l'extension : son dossier, et la page d'extensions du navigateur.
pub fn open_install(browser: Option<&str>) {
    // Une fois l'extension publiée, sa page du store suffit : un clic sur « Ajouter ».
    if let Some(id) = STORE_EXTENSION_ID {
        let page = HSTRING::from(format!("https://chromewebstore.google.com/detail/{id}"));
        unsafe {
            ShellExecuteW(None, w!("open"), &page, PCWSTR::null(), PCWSTR::null(), SW_SHOWNORMAL);
        }
        return;
    }
    let Some(base) = std::env::var_os("LOCALAPPDATA") else { return };
    let folder = std::path::Path::new(&base).join("Melangeur").join("extension");
    unsafe {
        ShellExecuteW(
            None,
            w!("open"),
            &HSTRING::from(folder.as_os_str()),
            PCWSTR::null(),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        );
        if let Some(browser) = browser {
            let page = if browser.to_ascii_lowercase().ends_with("msedge.exe") { "edge://extensions" } else { "chrome://extensions" };
            ShellExecuteW(None, w!("open"), &HSTRING::from(browser), &HSTRING::from(page), PCWSTR::null(), SW_SHOWNORMAL);
        }
    }
}

/// Arrête les relais lancés par les navigateurs : ils verrouillent le .exe à désinstaller.
pub fn stop_hosts() {
    unsafe {
        let Ok(snapshot) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else { return };
        let own = GetCurrentProcessId();
        let mut entry = PROCESSENTRY32W { dwSize: size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
        let mut more = Process32FirstW(snapshot, &mut entry).is_ok();
        while more {
            let end = entry.szExeFile.iter().position(|c| *c == 0).unwrap_or(entry.szExeFile.len());
            let name = String::from_utf16_lossy(&entry.szExeFile[..end]);
            if entry.th32ProcessID != own && name.eq_ignore_ascii_case("melangeur.exe") {
                if let Ok(process) = OpenProcess(PROCESS_TERMINATE, false, entry.th32ProcessID) {
                    let _ = TerminateProcess(process, 0);
                    let _ = CloseHandle(process);
                }
            }
            more = Process32NextW(snapshot, &mut entry).is_ok();
        }
        let _ = CloseHandle(snapshot);
    }
}

/// Retire les déclarations faites par `register_host`, à la désinstallation.
pub fn unregister_host() {
    for root in REGISTRY_ROOTS {
        let key = HSTRING::from(format!("{root}\\{HOST_NAME}"));
        unsafe {
            let _ = RegDeleteKeyW(HKEY_CURRENT_USER, &key);
        }
    }
}

// --- Mode hôte, lancé par le navigateur ---

static HOST_PIPE: AtomicIsize = AtomicIsize::new(0);

pub fn is_host_invocation() -> bool {
    std::env::args().nth(1).is_some_and(|argument| argument.starts_with("chrome-extension://"))
}

pub fn run_host() {
    std::thread::spawn(forward_stdin);
    let browser = find_browser_path();
    let mut stdout = std::io::stdout();
    loop {
        let pipe = unsafe {
            CreateFileW(
                PIPE_NAME,
                (FILE_GENERIC_READ | FILE_GENERIC_WRITE).0,
                FILE_SHARE_NONE,
                None,
                OPEN_EXISTING,
                FILE_FLAG_OVERLAPPED,
                None,
            )
        };
        let Ok(pipe) = pipe else {
            // Melangeur n'est pas lancé : on attend qu'il revienne.
            std::thread::sleep(Duration::from_secs(3));
            continue;
        };
        HOST_PIPE.store(pipe.0 as isize, Ordering::Release);
        let hello = serde_json::to_vec(&json!({ "browser": browser })).unwrap_or_default();
        write_frame(pipe, &hello);
        // L'extension renvoie son état à chaque nouvelle connexion.
        write_stdout(&mut stdout, br#"{"cmd":"hello"}"#);
        while let Some(frame) = read_frame(pipe) {
            write_stdout(&mut stdout, &frame);
        }
        HOST_PIPE.store(0, Ordering::Release);
        unsafe {
            let _ = CloseHandle(pipe);
        }
    }
}

fn write_stdout(stdout: &mut std::io::Stdout, data: &[u8]) {
    let mut lock = stdout.lock();
    let ok = lock.write_all(&(data.len() as u32).to_le_bytes()).is_ok()
        && lock.write_all(data).is_ok()
        && lock.flush().is_ok();
    if !ok {
        std::process::exit(0);
    }
}

/// Le navigateur ferme stdin quand il n'a plus besoin de l'hôte : on s'arrête alors.
fn forward_stdin() {
    let mut stdin = std::io::stdin();
    loop {
        let mut size = [0u8; 4];
        if stdin.read_exact(&mut size).is_err() {
            std::process::exit(0);
        }
        let size = u32::from_le_bytes(size) as usize;
        if size > MAX_FRAME {
            std::process::exit(0);
        }
        let mut data = vec![0u8; size];
        if stdin.read_exact(&mut data).is_err() {
            std::process::exit(0);
        }
        let pipe = HOST_PIPE.load(Ordering::Acquire);
        if pipe != 0 {
            write_frame(to_handle(pipe), &data);
        }
    }
}

/// Remonte les processus parents jusqu'au navigateur : Chromium lance l'hôte via cmd.exe.
fn find_browser_path() -> String {
    unsafe {
        let Ok(snapshot) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else { return String::new() };
        let mut entries: Vec<(u32, u32)> = Vec::new();
        let mut entry = PROCESSENTRY32W { dwSize: size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
        let mut more = Process32FirstW(snapshot, &mut entry).is_ok();
        while more {
            entries.push((entry.th32ProcessID, entry.th32ParentProcessID));
            more = Process32NextW(snapshot, &mut entry).is_ok();
        }
        let _ = CloseHandle(snapshot);

        let mut pid = GetCurrentProcessId();
        for _ in 0..6 {
            let Some((_, parent)) = entries.iter().find(|(p, _)| *p == pid) else { break };
            pid = *parent;
            if let Some(path) = crate::engine::image_path(pid) {
                let file = path.rsplit('\\').next().unwrap_or("").to_ascii_lowercase();
                if file != "cmd.exe" && file != "conhost.exe" {
                    return path;
                }
            }
        }
        String::new()
    }
}
