//! Thread moteur : MTA, propriétaire de tous les objets audio.
//! Il dort dans GetMessageW tant qu'aucun événement n'arrive.
//! Les callbacks COM ne font que poster un message : ni attente, ni désabonnement.
//! Les seules minuteries (vumètres, onglets) n'existent que flyout ouvert.

use std::ffi::c_void;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::Sender;

use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Dwm::{DWMWA_CLOAKED, DwmGetWindowAttribute};
use windows::Win32::Media::Audio::Endpoints::*;
use windows::Win32::Media::Audio::*;
use windows::Win32::Storage::FileSystem::{GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW};
use windows::Win32::Storage::Packaging::Appx::GetApplicationUserModelId;
use windows::Win32::System::Com::*;
use windows::Win32::System::Threading::*;
use windows::Win32::UI::Shell::{FOLDERID_AppsFolder, IShellItem, KF_FLAG_DONT_VERIFY, SHCreateItemInKnownFolder, SIGDN_NORMALDISPLAY};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{BOOL, GUID, HRESULT, HSTRING, IUnknown, IUnknown_Vtbl, Interface, PCWSTR, PWSTR, Ref, Result, implement, interface};

use crate::model::*;
use crate::uia;

/// Identifie nos propres réglages, pour ignorer leur écho dans les notifications.
const SELF_CONTEXT: GUID = GUID::from_u128(0x6d3f_1a52_7c1e_4b0a_9f27_53a1_8e44_c0d1);
const NO_APP: u32 = u32::MAX;
const PUBLISH_DELAY_MS: u32 = 30;
const REBUILD_DELAY_MS: u32 = 200;
const METER_PERIOD_MS: u32 = 33;
const TAB_SCAN_PERIOD_MS: u32 = 1500;

static ENGINE_THREAD: AtomicU32 = AtomicU32::new(0);
pub static PEAKS_QUEUED: AtomicBool = AtomicBool::new(false);

fn engine_thread() -> u32 {
    ENGINE_THREAD.load(Ordering::Relaxed)
}

// Interface non documentée, utilisée par EarTrumpet pour changer le périphérique par défaut.
#[interface("F8679F50-850A-41CF-9C72-430F290290C8")]
unsafe trait IPolicyConfig: IUnknown {
    fn unused1(&self) -> HRESULT;
    fn unused2(&self) -> HRESULT;
    fn unused3(&self) -> HRESULT;
    fn unused4(&self) -> HRESULT;
    fn unused5(&self) -> HRESULT;
    fn unused6(&self) -> HRESULT;
    fn unused7(&self) -> HRESULT;
    fn unused8(&self) -> HRESULT;
    fn get_property_value(&self, id: PCWSTR, key: *const c_void, value: *mut c_void) -> HRESULT;
    fn set_property_value(&self, id: PCWSTR, key: *const c_void, value: *mut c_void) -> HRESULT;
    fn set_default_endpoint(&self, id: PCWSTR, role: ERole) -> HRESULT;
}
const POLICY_CONFIG_CLIENT: GUID = GUID::from_u128(0x870af99c_171d_4f9e_af0d_e63df40c2bc9);

/// État écrit par le callback et lu par le moteur, sans verrou.
struct SinkState {
    id: u32,
    volume_bits: AtomicU32,
    muted: AtomicBool,
    expired: AtomicBool,
    active: AtomicBool,
    queued: AtomicBool,
}

impl SinkState {
    /// Regroupe les rafales : un seul message en attente par session.
    fn signal(&self, message: u32) {
        if !self.queued.swap(true, Ordering::AcqRel) && !post_engine(engine_thread(), message, self.id as usize, 0) {
            self.queued.store(false, Ordering::Release);
        }
    }
}

#[implement(IAudioSessionEvents)]
struct SessionSink {
    state: Arc<SinkState>,
}

impl IAudioSessionEvents_Impl for SessionSink_Impl {
    fn OnDisplayNameChanged(&self, _: &PCWSTR, _: *const GUID) -> Result<()> {
        Ok(())
    }
    fn OnIconPathChanged(&self, _: &PCWSTR, _: *const GUID) -> Result<()> {
        Ok(())
    }
    fn OnSimpleVolumeChanged(&self, volume: f32, mute: BOOL, context: *const GUID) -> Result<()> {
        if !context.is_null() && unsafe { *context } == SELF_CONTEXT {
            return Ok(());
        }
        self.state.volume_bits.store(volume.to_bits(), Ordering::Relaxed);
        self.state.muted.store(mute.as_bool(), Ordering::Relaxed);
        self.state.signal(WM_E_SESSION_DIRTY);
        Ok(())
    }
    fn OnChannelVolumeChanged(&self, _: u32, _: *const f32, _: u32, _: *const GUID) -> Result<()> {
        Ok(())
    }
    fn OnGroupingParamChanged(&self, _: *const GUID, _: *const GUID) -> Result<()> {
        Ok(())
    }
    fn OnStateChanged(&self, state: AudioSessionState) -> Result<()> {
        self.state.active.store(state == AudioSessionStateActive, Ordering::Relaxed);
        if state == AudioSessionStateExpired {
            self.state.expired.store(true, Ordering::Relaxed);
        }
        self.state.signal(WM_E_SESSION_DIRTY);
        Ok(())
    }
    fn OnSessionDisconnected(&self, _: AudioSessionDisconnectReason) -> Result<()> {
        self.state.expired.store(true, Ordering::Relaxed);
        self.state.signal(WM_E_SESSION_DIRTY);
        Ok(())
    }
}

#[implement(IAudioSessionNotification)]
struct SessionNotifier {
    device: u32,
}

impl IAudioSessionNotification_Impl for SessionNotifier_Impl {
    fn OnSessionCreated(&self, session: Ref<IAudioSessionControl>) -> Result<()> {
        if let Some(session) = session.as_ref() {
            // Garde une référence et la transfère au moteur, qui fera l'abonnement hors callback.
            let raw = session.clone().into_raw();
            if !post_engine(engine_thread(), WM_E_SESSION_CREATED, self.device as usize, raw as isize) {
                drop(unsafe { IAudioSessionControl::from_raw(raw) });
            }
        }
        Ok(())
    }
}

#[implement(IAudioEndpointVolumeCallback)]
struct EndpointSink {
    state: Arc<SinkState>,
}

impl IAudioEndpointVolumeCallback_Impl for EndpointSink_Impl {
    fn OnNotify(&self, data: *mut AUDIO_VOLUME_NOTIFICATION_DATA) -> Result<()> {
        if let Some(data) = unsafe { data.as_ref() } {
            if data.guidEventContext != SELF_CONTEXT {
                self.state.volume_bits.store(data.fMasterVolume.to_bits(), Ordering::Relaxed);
                self.state.muted.store(data.bMuted.as_bool(), Ordering::Relaxed);
                self.state.signal(WM_E_DEVICE_DIRTY);
            }
        }
        Ok(())
    }
}

#[implement(IMMNotificationClient)]
struct DeviceClient;

impl DeviceClient_Impl {
    fn changed(&self) {
        post_engine(engine_thread(), WM_E_DEVICES_CHANGED, 0, 0);
    }
}

impl IMMNotificationClient_Impl for DeviceClient_Impl {
    fn OnDeviceStateChanged(&self, _: &PCWSTR, _: DEVICE_STATE) -> Result<()> {
        self.changed();
        Ok(())
    }
    fn OnDeviceAdded(&self, _: &PCWSTR) -> Result<()> {
        self.changed();
        Ok(())
    }
    fn OnDeviceRemoved(&self, _: &PCWSTR) -> Result<()> {
        self.changed();
        Ok(())
    }
    fn OnDefaultDeviceChanged(&self, flow: EDataFlow, role: ERole, _: &PCWSTR) -> Result<()> {
        if flow == eRender && role == eMultimedia {
            self.changed();
        }
        Ok(())
    }
    fn OnPropertyValueChanged(&self, _: &PCWSTR, _: &PROPERTYKEY) -> Result<()> {
        Ok(())
    }
}

unsafe extern "system" fn on_process_exit(context: *mut c_void, _timed_out: bool) {
    post_engine(engine_thread(), WM_E_PROCESS_EXIT, context as usize, 0);
}

struct Device {
    id: u32,
    hardware_id: String,
    name: String,
    endpoint: IAudioEndpointVolume,
    endpoint_sink: IAudioEndpointVolumeCallback,
    meter: IAudioMeterInformation,
    manager: IAudioSessionManager2,
    notifier: IAudioSessionNotification,
    state: Arc<SinkState>,
    peak: Arc<AtomicU32>,
}

struct Session {
    id: u32,
    pid: u32,
    device: u32,
    app: u32,
    instance_id: String,
    control: IAudioSessionControl,
    volume: ISimpleAudioVolume,
    meter: Option<IAudioMeterInformation>,
    sink: IAudioSessionEvents,
    state: Arc<SinkState>,
}

/// Une appli identifiée par le chemin de son exécutable, comme dans EarTrumpet.
struct App {
    key: u32,
    path: String,
    name: String,
    aumid: Option<String>,
}

/// Attente sans délai sur la fin d'un processus : aucun réveil tant qu'il vit.
struct ProcessWatch {
    pid: u32,
    process: HANDLE,
    wait: HANDLE,
}

impl ProcessWatch {
    fn stop(self) {
        unsafe {
            let _ = UnregisterWaitEx(self.wait, Some(INVALID_HANDLE_VALUE));
            let _ = CloseHandle(self.process);
        }
    }
}

struct RowPeak {
    device: u32,
    app: u32,
    peak: Arc<AtomicU32>,
}

struct Engine {
    ui: isize,
    enumerator: IMMDeviceEnumerator,
    device_client: IMMNotificationClient,
    devices: Vec<Device>,
    default_device: String,
    next_device_id: u32,
    next_session_id: u32,
    sessions: Vec<Session>,
    apps: Vec<App>,
    watches: Vec<ProcessWatch>,
    row_peaks: Vec<RowPeak>,
    flyout_visible: bool,
    publish_timer: usize,
    rebuild_timer: usize,
    meter_timer: usize,
    scan_timer: usize,
    last_scan_had_jobs: bool,
    window_buffer: Vec<isize>,
    pid_cache: Vec<(u32, u32)>,
}

pub fn run(ready: Sender<u32>) {
    unsafe {
        // Crée la file de messages avant de communiquer l'id du thread.
        let mut msg = MSG::default();
        let _ = PeekMessageW(&mut msg, None, WM_USER, WM_USER, PM_NOREMOVE);
        let thread_id = GetCurrentThreadId();
        ENGINE_THREAD.store(thread_id, Ordering::Relaxed);

        let com_ok = CoInitializeEx(None, COINIT_MULTITHREADED).is_ok();
        let mut engine = Engine::new();
        let _ = ready.send(thread_id);

        if let Some(engine) = engine.as_mut() {
            engine.rebuild_devices();
        }
        while GetMessageW(&mut msg, None, 0, 0).0 > 0 {
            if let Some(engine) = engine.as_mut() {
                engine.handle(&msg);
            }
        }
        if let Some(mut engine) = engine.take() {
            engine.shutdown();
        }
        if com_ok {
            CoUninitialize();
        }
    }
}

impl Engine {
    fn new() -> Option<Self> {
        unsafe {
            let enumerator: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).ok()?;
            let device_client: IMMNotificationClient = DeviceClient.into();
            enumerator.RegisterEndpointNotificationCallback(&device_client).ok()?;
            Some(Self {
                ui: 0,
                enumerator,
                device_client,
                devices: Vec::with_capacity(4),
                default_device: String::new(),
                next_device_id: 1,
                next_session_id: 1,
                sessions: Vec::with_capacity(32),
                apps: Vec::with_capacity(32),
                watches: Vec::with_capacity(32),
                row_peaks: Vec::new(),
                flyout_visible: false,
                publish_timer: 0,
                rebuild_timer: 0,
                meter_timer: 0,
                scan_timer: 0,
                last_scan_had_jobs: false,
                window_buffer: Vec::new(),
                pid_cache: Vec::new(),
            })
        }
    }

    fn handle(&mut self, msg: &MSG) {
        let (w, l) = (msg.wParam.0, msg.lParam.0);
        match msg.message {
            WM_E_SET_UI => self.ui = l,
            WM_E_SESSION_CREATED => {
                let session = unsafe { IAudioSessionControl::from_raw(l as *mut c_void) };
                // Un périphérique retiré entre-temps ne doit plus recevoir de session.
                if self.devices.iter().any(|d| d.id == w as u32) {
                    self.add_session(w as u32, session);
                    self.schedule_publish();
                }
            }
            WM_E_SESSION_DIRTY => self.on_session_dirty(w as u32),
            WM_E_DEVICE_DIRTY => {
                if let Some(device) = self.devices.iter().find(|d| d.state.id == w as u32) {
                    device.state.queued.store(false, Ordering::Release);
                }
                self.schedule_publish();
            }
            WM_E_DEVICES_CHANGED => {
                // Plusieurs notifications arrivent d'un coup : une seule reconstruction suffit.
                if self.rebuild_timer == 0 {
                    self.rebuild_timer = unsafe { SetTimer(None, 0, REBUILD_DELAY_MS, None) };
                }
            }
            WM_E_PROCESS_EXIT => {
                self.remove_sessions_where(|s| s.pid == w as u32);
                self.schedule_publish();
            }
            WM_E_SET_VOLUME => self.set_volume(w as u32, f32::from_bits(l as u32)),
            WM_E_SET_MUTE => self.set_mute(w as u32, l != 0),
            WM_E_FLYOUT => {
                if w != 0 {
                    self.on_flyout_shown();
                } else {
                    self.on_flyout_hidden();
                }
            }
            WM_E_LIST_DEVICES => {
                if self.ui != 0 {
                    post_ui_box(self.ui, WM_U_DEVICES, Box::new(self.device_rows(true)));
                }
            }
            WM_E_SET_DEFAULT => self.set_default(w as u32),
            WM_TIMER => self.on_timer(w),
            _ => {}
        }
    }

    fn on_timer(&mut self, id: usize) {
        if id == self.meter_timer {
            self.update_meters();
        } else if id == self.scan_timer {
            self.scan_tabs();
        } else if id == self.publish_timer {
            unsafe {
                let _ = KillTimer(None, id);
            }
            self.publish_timer = 0;
            self.publish();
        } else if id == self.rebuild_timer {
            unsafe {
                let _ = KillTimer(None, id);
            }
            self.rebuild_timer = 0;
            self.rebuild_devices();
            self.publish();
        }
    }

    // --- Périphériques ---

    fn rebuild_devices(&mut self) {
        self.detach_all();
        self.default_device = unsafe {
            self.enumerator
                .GetDefaultAudioEndpoint(eRender, eMultimedia)
                .ok()
                .and_then(|device| device.GetId().ok())
                .map(take_co_string)
                .unwrap_or_default()
        };
        let collection = unsafe { self.enumerator.EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE) };
        let Ok(collection) = collection else { return };
        for i in 0..unsafe { collection.GetCount() }.unwrap_or(0) {
            if let Ok(device) = unsafe { collection.Item(i) } {
                let _ = self.attach_device(&device);
            }
        }
    }

    fn attach_device(&mut self, device: &IMMDevice) -> Result<()> {
        unsafe {
            let id = self.next_device_id;
            self.next_device_id = (id + 1) & 0x7FFF;
            let hardware_id = take_co_string(device.GetId()?);
            let store = device.OpenPropertyStore(STGM_READ)?;
            let name = store.GetValue(&PKEY_Device_FriendlyName).map(|v| v.to_string()).unwrap_or_default();

            let endpoint: IAudioEndpointVolume = device.Activate(CLSCTX_ALL, None)?;
            let meter: IAudioMeterInformation = device.Activate(CLSCTX_ALL, None)?;
            let manager: IAudioSessionManager2 = device.Activate(CLSCTX_ALL, None)?;

            let state = Arc::new(SinkState {
                id,
                volume_bits: AtomicU32::new(endpoint.GetMasterVolumeLevelScalar()?.to_bits()),
                muted: AtomicBool::new(endpoint.GetMute()?.as_bool()),
                expired: AtomicBool::new(false),
                active: AtomicBool::new(false),
                queued: AtomicBool::new(false),
            });
            let endpoint_sink: IAudioEndpointVolumeCallback = EndpointSink { state: state.clone() }.into();
            endpoint.RegisterControlChangeNotify(&endpoint_sink)?;

            let notifier: IAudioSessionNotification = SessionNotifier { device: id }.into();
            manager.RegisterSessionNotification(&notifier)?;

            self.devices.push(Device {
                id,
                hardware_id,
                name,
                endpoint,
                endpoint_sink,
                meter,
                manager: manager.clone(),
                notifier,
                state,
                peak: Arc::new(AtomicU32::new(0)),
            });

            let list = manager.GetSessionEnumerator()?;
            // Microsoft : GetCount déclenche la réception des notifications.
            for i in 0..list.GetCount()? {
                if let Ok(control) = list.GetSession(i) {
                    self.add_session(id, control);
                }
            }
        }
        Ok(())
    }

    fn detach_all(&mut self) {
        self.remove_sessions_where(|_| true);
        self.row_peaks.clear();
        for device in self.devices.drain(..) {
            unsafe {
                let _ = device.manager.UnregisterSessionNotification(&device.notifier);
                let _ = device.endpoint.UnregisterControlChangeNotify(&device.endpoint_sink);
            }
        }
    }

    /// Même réglage qu'EarTrumpet, sur les rôles multimédia et console.
    fn set_default(&self, id: u32) {
        let Some(device) = self.devices.iter().find(|d| d.id == id) else { return };
        unsafe {
            let Ok(policy) = CoCreateInstance::<_, IPolicyConfig>(&POLICY_CONFIG_CLIENT, None, CLSCTX_ALL) else { return };
            let wide = HSTRING::from(device.hardware_id.as_str());
            let _ = policy.set_default_endpoint(PCWSTR(wide.as_ptr()), eMultimedia);
            let _ = policy.set_default_endpoint(PCWSTR(wide.as_ptr()), eConsole);
        }
    }

    // --- Sessions ---

    fn add_session(&mut self, device: u32, control: IAudioSessionControl) {
        let _ = self.try_add_session(device, control);
    }

    fn try_add_session(&mut self, device: u32, control: IAudioSessionControl) -> Result<()> {
        unsafe {
            let session_state = control.GetState()?;
            if session_state == AudioSessionStateExpired {
                return Ok(());
            }
            let control2: IAudioSessionControl2 = control.cast()?;
            let volume: ISimpleAudioVolume = control.cast()?;

            // Une session peut arriver à la fois par l'énumération et par OnSessionCreated.
            let instance_id = take_co_string(control2.GetSessionInstanceIdentifier()?);
            if self.sessions.iter().any(|s| s.instance_id == instance_id) {
                return Ok(());
            }

            // AUDCLNT_S_NO_SINGLE_PROCESS est un succès : le pid est alors celui du créateur.
            let pid = control2.GetProcessId()?;
            let is_system = control2.IsSystemSoundsSession() == S_OK;
            let app = self.app_for(pid, is_system);

            // Hérite du mute de l'appli, pour qu'un nouveau flux ne sorte pas à plein volume.
            let app_muted = self.sessions.iter().find(|s| s.app == app && s.device == device)
                .is_some_and(|s| s.state.muted.load(Ordering::Relaxed));
            let mut muted = volume.GetMute()?.as_bool();
            if app_muted && !muted {
                let _ = volume.SetMute(true, &SELF_CONTEXT);
                muted = true;
            }

            let id = self.next_session_id;
            self.next_session_id = id.wrapping_add(1);
            let state = Arc::new(SinkState {
                id,
                volume_bits: AtomicU32::new(volume.GetMasterVolume()?.to_bits()),
                muted: AtomicBool::new(muted),
                expired: AtomicBool::new(false),
                active: AtomicBool::new(session_state == AudioSessionStateActive),
                queued: AtomicBool::new(false),
            });
            let sink: IAudioSessionEvents = SessionSink { state: state.clone() }.into();
            control.RegisterAudioSessionNotification(&sink)?;
            if !is_system && pid != 0 {
                self.watch_process(pid);
            }
            let meter = control.cast().ok();
            self.sessions.push(Session { id, pid, device, app, instance_id, control, volume, meter, sink, state });
        }
        Ok(())
    }

    fn watch_process(&mut self, pid: u32) {
        if self.watches.iter().any(|w| w.pid == pid) {
            return;
        }
        unsafe {
            let Ok(process) = OpenProcess(PROCESS_SYNCHRONIZE, false, pid) else { return };
            let mut wait = HANDLE::default();
            if RegisterWaitForSingleObject(
                &mut wait,
                process,
                Some(on_process_exit),
                Some(pid as usize as *const c_void),
                INFINITE,
                WT_EXECUTEONLYONCE,
            )
            .is_ok()
            {
                self.watches.push(ProcessWatch { pid, process, wait });
            } else {
                let _ = CloseHandle(process);
            }
        }
    }

    fn app_for(&mut self, pid: u32, is_system: bool) -> u32 {
        let path = if is_system {
            String::new()
        } else {
            image_path(pid).unwrap_or_else(|| format!("pid:{pid}"))
        };
        if let Some(app) = self.apps.iter().find(|a| a.path == path) {
            return app.key;
        }
        let key = self.apps.len() as u32;
        let aumid = if is_system { None } else { package_id(pid) };
        let name = if is_system {
            "Sons système".to_string()
        } else {
            aumid.as_deref().and_then(package_name).unwrap_or_else(|| display_name(&path))
        };
        self.apps.push(App { key, path, name, aumid });
        key
    }

    fn on_session_dirty(&mut self, id: u32) {
        let Some(session) = self.sessions.iter().find(|s| s.id == id) else { return };
        session.state.queued.store(false, Ordering::Release);
        if session.state.expired.load(Ordering::Relaxed) {
            self.remove_sessions_where(|s| s.id == id);
        }
        self.schedule_publish();
    }

    /// Le désabonnement a lieu ici, hors de tout callback, comme l'exige Microsoft.
    fn remove_sessions_where(&mut self, mut predicate: impl FnMut(&Session) -> bool) {
        let mut i = 0;
        while i < self.sessions.len() {
            if predicate(&self.sessions[i]) {
                let session = self.sessions.swap_remove(i);
                let _ = unsafe { session.control.UnregisterAudioSessionNotification(&session.sink) };
            } else {
                i += 1;
            }
        }
        // Arrête les attentes des processus qui n'ont plus de session.
        let mut i = 0;
        while i < self.watches.len() {
            let pid = self.watches[i].pid;
            if self.sessions.iter().any(|s| s.pid == pid) {
                i += 1;
            } else {
                self.watches.swap_remove(i).stop();
            }
        }
    }

    // --- Réglages ---

    fn set_volume(&mut self, target: u32, value: f32) {
        let (device, app) = (target_device(target), target_app(target));
        if app == DEVICE_TARGET {
            if let Some(d) = self.devices.iter().find(|d| d.id == device) {
                if unsafe { d.endpoint.SetMasterVolumeLevelScalar(value, &SELF_CONTEXT) }.is_ok() {
                    d.state.volume_bits.store(value.to_bits(), Ordering::Relaxed);
                }
            }
            return;
        }
        for session in self.sessions.iter().filter(|s| s.device == device && s.app == app) {
            if unsafe { session.volume.SetMasterVolume(value, &SELF_CONTEXT) }.is_ok() {
                session.state.volume_bits.store(value.to_bits(), Ordering::Relaxed);
            }
        }
    }

    fn set_mute(&mut self, target: u32, muted: bool) {
        let (device, app) = (target_device(target), target_app(target));
        if app == DEVICE_TARGET {
            if let Some(d) = self.devices.iter().find(|d| d.id == device) {
                if unsafe { d.endpoint.SetMute(muted, &SELF_CONTEXT) }.is_ok() {
                    d.state.muted.store(muted, Ordering::Relaxed);
                }
            }
            return;
        }
        for session in self.sessions.iter().filter(|s| s.device == device && s.app == app) {
            if unsafe { session.volume.SetMute(muted, &SELF_CONTEXT) }.is_ok() {
                session.state.muted.store(muted, Ordering::Relaxed);
            }
        }
    }

    // --- Flyout ouvert : vumètres et onglets ---

    fn on_flyout_shown(&mut self) {
        if self.flyout_visible {
            return;
        }
        self.flyout_visible = true;
        self.publish();
        self.update_meters();
        self.scan_tabs();
        unsafe {
            self.meter_timer = SetTimer(None, 0, METER_PERIOD_MS, None);
            self.scan_timer = SetTimer(None, 0, TAB_SCAN_PERIOD_MS, None);
        }
    }

    fn on_flyout_hidden(&mut self) {
        self.flyout_visible = false;
        for timer in [&mut self.publish_timer, &mut self.meter_timer, &mut self.scan_timer] {
            if *timer != 0 {
                unsafe {
                    let _ = KillTimer(None, *timer);
                }
                *timer = 0;
            }
        }
        self.window_buffer = Vec::new();
        self.pid_cache = Vec::new();
        self.last_scan_had_jobs = false;
    }

    fn update_meters(&mut self) {
        let mut changed = false;
        for device in &self.devices {
            let value = unsafe { device.meter.GetPeakValue() }.unwrap_or(0.0).to_bits();
            changed |= device.peak.swap(value, Ordering::Relaxed) != value;
        }
        for row in &self.row_peaks {
            let mut peak = 0f32;
            for session in self.sessions.iter().filter(|s| s.device == row.device && s.app == row.app) {
                if let Some(meter) = &session.meter {
                    peak = peak.max(unsafe { meter.GetPeakValue() }.unwrap_or(0.0));
                }
            }
            changed |= row.peak.swap(peak.to_bits(), Ordering::Relaxed) != peak.to_bits();
        }
        // Rien ne bouge, donc rien à redessiner.
        if changed && self.ui != 0 && !PEAKS_QUEUED.swap(true, Ordering::AcqRel) && !post_ui(self.ui, WM_U_PEAKS) {
            PEAKS_QUEUED.store(false, Ordering::Release);
        }
    }

    /// Ne lit que les navigateurs qui ont une session audio, et seulement flyout ouvert.
    /// Elle peut être inactive : dans un appel où personne ne parle, le micro reste à signaler.
    fn scan_tabs(&mut self) {
        if self.ui == 0 {
            return;
        }
        let browsers: Vec<u32> = self.apps.iter()
            .filter(|a| uia::is_browser(&a.path) && !crate::ext::covers(&a.path))
            .filter(|a| self.sessions.iter().any(|s| s.app == a.key))
            .map(|a| a.key)
            .collect();
        let jobs = if browsers.is_empty() { Vec::new() } else { self.browser_windows(&browsers) };
        if jobs.is_empty() {
            if self.last_scan_had_jobs {
                self.last_scan_had_jobs = false;
                post_ui_box(self.ui, WM_U_TABS, Box::new(Vec::<AppTabs>::new()));
            }
            return;
        }
        self.last_scan_had_jobs = true;
        uia::scan_async(self.ui, jobs);
    }

    fn browser_windows(&mut self, browsers: &[u32]) -> Vec<(u32, isize)> {
        self.window_buffer.clear();
        unsafe {
            let _ = EnumWindows(Some(collect_app_window), LPARAM(&mut self.window_buffer as *mut Vec<isize> as isize));
        }
        let mut jobs = Vec::new();
        for i in 0..self.window_buffer.len() {
            let hwnd = self.window_buffer[i];
            let mut pid = 0;
            unsafe { GetWindowThreadProcessId(to_hwnd(hwnd), Some(&mut pid)) };
            let app = match self.pid_cache.iter().find(|(p, _)| *p == pid) {
                Some((_, app)) => *app,
                None => {
                    let app = image_path(pid)
                        .and_then(|path| self.apps.iter().find(|a| a.path == path).map(|a| a.key))
                        .unwrap_or(NO_APP);
                    self.pid_cache.push((pid, app));
                    app
                }
            };
            if browsers.contains(&app) {
                jobs.push((app, hwnd));
            }
        }
        jobs
    }

    // --- Publication ---

    fn schedule_publish(&mut self) {
        if self.flyout_visible && self.publish_timer == 0 {
            self.publish_timer = unsafe { SetTimer(None, 0, PUBLISH_DELAY_MS, None) };
        }
    }

    /// Alloue un instantané seulement si le flyout est visible.
    fn publish(&mut self) {
        if !self.flyout_visible || self.ui == 0 {
            return;
        }
        let rows = self.device_rows(false);
        post_ui_box(self.ui, WM_U_SNAPSHOT, Box::new(rows));
    }

    fn row_peak(&mut self, device: u32, app: u32) -> Arc<AtomicU32> {
        if let Some(row) = self.row_peaks.iter().find(|r| r.device == device && r.app == app) {
            return row.peak.clone();
        }
        let peak = Arc::new(AtomicU32::new(0));
        self.row_peaks.push(RowPeak { device, app, peak: peak.clone() });
        peak
    }

    /// Le périphérique par défaut d'abord, puis ceux où une appli joue, comme le veut un mélangeur.
    /// Avec `all`, renvoie tous les périphériques actifs, pour le menu.
    fn device_rows(&mut self, all: bool) -> Vec<DeviceRow> {
        let mut rows: Vec<DeviceRow> = Vec::with_capacity(self.devices.len());
        for d in 0..self.devices.len() {
            let (id, is_default) = (self.devices[d].id, self.devices[d].hardware_id == self.default_device);
            let mut apps = Vec::new();
            for a in 0..self.apps.len() {
                let key = self.apps[a].key;
                let Some(session) = self.sessions.iter().find(|s| s.device == id && s.app == key) else { continue };
                let volume = f32::from_bits(session.state.volume_bits.load(Ordering::Relaxed));
                let muted = session.state.muted.load(Ordering::Relaxed);
                let peak = self.row_peak(id, key);
                let app = &self.apps[a];
                apps.push(AppRow {
                    app: key,
                    name: app.name.clone(),
                    path: app.path.clone(),
                    aumid: app.aumid.clone(),
                    volume,
                    muted,
                    peak,
                });
            }
            let has_real_app = apps.iter().any(|a| !a.path.is_empty());
            if !all && !is_default && !has_real_app {
                continue;
            }
            apps.sort_by_cached_key(|r| (!r.path.is_empty(), r.name.to_lowercase()));
            let device = &self.devices[d];
            rows.push(DeviceRow {
                id,
                name: device.name.clone(),
                is_default,
                volume: f32::from_bits(device.state.volume_bits.load(Ordering::Relaxed)),
                muted: device.state.muted.load(Ordering::Relaxed),
                peak: device.peak.clone(),
                apps,
            });
        }
        rows.sort_by_cached_key(|r| (!r.is_default, r.name.to_lowercase()));
        rows
    }

    fn shutdown(&mut self) {
        self.on_flyout_hidden();
        if self.rebuild_timer != 0 {
            unsafe {
                let _ = KillTimer(None, self.rebuild_timer);
            }
        }
        self.detach_all();
        let _ = unsafe { self.enumerator.UnregisterEndpointNotificationCallback(&self.device_client) };
    }
}

/// Copie une chaîne allouée par COM et la libère.
fn take_co_string(value: PWSTR) -> String {
    unsafe {
        let text = value.to_string().unwrap_or_default();
        CoTaskMemFree(Some(value.as_ptr() as *const c_void));
        text
    }
}

/// Fenêtre principale : premier niveau, visible, sans propriétaire, avec un titre, non masquée.
unsafe extern "system" fn collect_app_window(hwnd: HWND, lparam: LPARAM) -> BOOL {
    unsafe {
        if IsWindowVisible(hwnd).as_bool()
            && GetWindow(hwnd, GW_OWNER).is_err()
            && (GetWindowLongW(hwnd, GWL_EXSTYLE) as u32 & WS_EX_TOOLWINDOW.0) == 0
            && GetWindowTextLengthW(hwnd) > 0
        {
            let mut cloaked = 0u32;
            let result = DwmGetWindowAttribute(hwnd, DWMWA_CLOAKED, &mut cloaked as *mut u32 as *mut c_void, 4);
            if result.is_err() || cloaked == 0 {
                (*(lparam.0 as *mut Vec<isize>)).push(hwnd.0 as isize);
            }
        }
    }
    true.into()
}

pub fn image_path(pid: u32) -> Option<String> {
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buffer = [0u16; 1024];
        let mut length = buffer.len() as u32;
        let result = QueryFullProcessImageNameW(process, PROCESS_NAME_WIN32, PWSTR(buffer.as_mut_ptr()), &mut length);
        let _ = CloseHandle(process);
        result.ok()?;
        Some(String::from_utf16_lossy(&buffer[..length as usize]))
    }
}

/// Identifiant d'une appli du Store ; aucun pour une appli de bureau classique.
fn package_id(pid: u32) -> Option<String> {
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buffer = [0u16; 512];
        let mut length = buffer.len() as u32;
        let result = GetApplicationUserModelId(process, &mut length, Some(PWSTR(buffer.as_mut_ptr())));
        let _ = CloseHandle(process);
        if result != ERROR_SUCCESS || length == 0 {
            return None;
        }
        Some(String::from_utf16_lossy(&buffer[..length as usize - 1]))
    }
}

fn package_name(aumid: &str) -> Option<String> {
    unsafe {
        let item: IShellItem =
            SHCreateItemInKnownFolder(&FOLDERID_AppsFolder, KF_FLAG_DONT_VERIFY, &HSTRING::from(aumid)).ok()?;
        let name = take_co_string(item.GetDisplayName(SIGDN_NORMALDISPLAY).ok()?);
        (!name.is_empty() && name != aumid).then_some(name)
    }
}

fn display_name(path: &str) -> String {
    file_description(path).unwrap_or_else(|| {
        let file = path.rsplit('\\').next().unwrap_or(path);
        file.strip_suffix(".exe").or(file.strip_suffix(".EXE")).unwrap_or(file).to_string()
    })
}

fn file_description(path: &str) -> Option<String> {
    unsafe {
        let wide = HSTRING::from(path);
        let size = GetFileVersionInfoSizeW(&wide, None);
        if size == 0 {
            return None;
        }
        let mut data = vec![0u8; size as usize];
        GetFileVersionInfoW(&wide, None, size, data.as_mut_ptr() as *mut c_void).ok()?;

        let mut pointer: *mut c_void = std::ptr::null_mut();
        let mut length = 0u32;
        if !VerQueryValueW(
            data.as_ptr() as *const c_void,
            &HSTRING::from("\\VarFileInfo\\Translation"),
            &mut pointer,
            &mut length,
        )
        .as_bool()
            || length < 4
        {
            return None;
        }
        let language = *(pointer as *const u16);
        let codepage = *(pointer as *const u16).add(1);
        let query = HSTRING::from(format!("\\StringFileInfo\\{language:04x}{codepage:04x}\\FileDescription"));
        if !VerQueryValueW(data.as_ptr() as *const c_void, &query, &mut pointer, &mut length).as_bool() || length == 0 {
            return None;
        }
        let text = std::slice::from_raw_parts(pointer as *const u16, length as usize);
        let end = text.iter().position(|c| *c == 0).unwrap_or(text.len());
        let name = String::from_utf16_lossy(&text[..end]).trim().to_string();
        (!name.is_empty()).then_some(name)
    }
}
