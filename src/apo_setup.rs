//! Installing Equalizer APO on output devices, without its Device Selector.
//!
//! It uses the same registry layout and test pipe as Equalizer APO 1.4 by
//! Jonas Thedering, so installs made here and by Device Selector are interchangeable:
//! back up the endpoint's effect CLSIDs under `SOFTWARE\EqualizerAPO\Child APOs\{guid}`,
//! point the endpoint at Equalizer APO's pre/post-mix APOs, restart the audio
//! service, then confirm over the APO's test pipe that it really loaded,
//! falling back through the install modes (SFX/EFX, SFX/MFX, LFX/GFX) if not.
//!
//! Everything here needs admin; the app runs it via `equinox.exe --apo-setup`
//! in an elevated child process ([`run_elevated`]), so there's one UAC prompt.

// FX slots are indexed in parallel with the backup arrays, mirroring the original.
#![allow(clippy::needless_range_loop)]

use std::collections::{HashMap, HashSet};
use std::io;
use std::path::Path;
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use winreg::RegKey;
use winreg::enums::{HKEY_CLASSES_ROOT, HKEY_LOCAL_MACHINE, KEY_READ, KEY_SET_VALUE, KEY_WRITE, RegType};

const RENDER: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\MMDevices\Audio\Render";
const APP: &str = r"SOFTWARE\EqualizerAPO";
const CHILD: &str = r"SOFTWARE\EqualizerAPO\Child APOs";
const PRE_MIX: &str = "{EACD2258-FCAC-4FF4-B36D-419E924A6D79}";
const POST_MIX: &str = "{EC1CC9CE-FAED-4822-828A-82A81A6F018F}";

const LFX: usize = 0;
const GFX: usize = 1;
const SFX: usize = 2;
const MFX: usize = 3;
const EFX: usize = 4;
const FX: [&str; 5] = [
    "{d04e05a6-594b-4fb6-a80d-01af5eed7d1d},1",
    "{d04e05a6-594b-4fb6-a80d-01af5eed7d1d},2",
    "{d04e05a6-594b-4fb6-a80d-01af5eed7d1d},5",
    "{d04e05a6-594b-4fb6-a80d-01af5eed7d1d},6",
    "{d04e05a6-594b-4fb6-a80d-01af5eed7d1d},7",
];
const MULTI_FX: [&str; 3] = [
    "{d04e05a6-594b-4fb6-a80d-01af5eed7d1d},13",
    "{d04e05a6-594b-4fb6-a80d-01af5eed7d1d},14",
    "{d04e05a6-594b-4fb6-a80d-01af5eed7d1d},15",
];
const SFX_MODES: &str = "{d3993a3f-99c2-4402-b5ec-a92a0367664b},5";
const MFX_MODES: &str = "{d3993a3f-99c2-4402-b5ec-a92a0367664b},6";
const EFX_MODES: &str = "{d3993a3f-99c2-4402-b5ec-a92a0367664b},7";
const DEFAULT_PROCESSING_MODE: &str = "{C18E2F7E-933D-4965-B7D1-1EEF228D2AF3}";
const DISABLE_ENHANCEMENTS: &str = "{1da5d803-d492-4edd-8c23-e0c0ffee7f0e},5";
const COMBINED_DEVICE: &str = "{b3f8fa53-0004-438e-9003-51a46e139bfc},41";
const NO_VALUE: &str = "!VALUE";
const NO_KEY: &str = "!KEY";
const INSTALL_VERSION: &str = "2";
const PIPE: &str = "EqualizerAPODeviceTest";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Mode {
    LfxGfx,
    SfxMfx,
    SfxEfx,
}

impl Mode {
    pub fn label(self) -> &'static str {
        match self {
            Mode::LfxGfx => "LFX/GFX",
            Mode::SfxMfx => "SFX/MFX",
            Mode::SfxEfx => "SFX/EFX",
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct DeviceResult {
    pub guid: String,
    pub installed: bool,
    /// Confirmed loading by the APO itself (None = couldn't test, e.g. unplugged).
    pub verified: Option<bool>,
    pub mode: Option<Mode>,
    pub message: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Report {
    pub devices: Vec<DeviceResult>,
    pub error: Option<String>,
    pub reboot_suggested: bool,
}

/// What we know about one endpoint's effect registration.
struct Dev {
    guid: String,
    /// Original CLSIDs per FX slot, or `!VALUE` / `!KEY`.
    originals: [String; 5],
    installed: bool,
    has_fx_key: bool,
    mode: Mode,
    active: bool,
}

fn hklm() -> RegKey {
    RegKey::predef(HKEY_LOCAL_MACHINE)
}

fn is_eapo(clsid: &str) -> bool {
    clsid.eq_ignore_ascii_case(PRE_MIX) || clsid.eq_ignore_ascii_case(POST_MIX)
}

fn get(key: &RegKey, name: &str) -> Option<String> {
    key.get_value::<String, _>(name).ok()
}

fn del(key: &RegKey, name: &str) -> io::Result<()> {
    match key.delete_value(name) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        r => r,
    }
}

fn set_multi(key: &RegKey, name: &str, value: &str) -> io::Result<()> {
    let bytes: Vec<u8> = value.encode_utf16().chain([0, 0]).flat_map(u16::to_le_bytes).collect();
    key.set_raw_value(
        name,
        &winreg::RegValue {
            bytes: bytes.into(),
            vtype: RegType::REG_MULTI_SZ,
        },
    )
}

fn load(guid: &str) -> io::Result<Dev> {
    let dev_key = hklm().open_subkey_with_flags(format!(r"{RENDER}\{guid}"), KEY_READ)?;
    let state: u32 = dev_key.get_value("DeviceState").unwrap_or(0);
    let active = state & 0xF == 1;
    let mut d = Dev {
        guid: guid.to_string(),
        originals: std::array::from_fn(|_| NO_KEY.to_string()),
        installed: false,
        has_fx_key: false,
        mode: Mode::LfxGfx,
        active,
    };
    let Ok(fx) = dev_key.open_subkey_with_flags("FxProperties", KEY_READ) else {
        return Ok(d);
    };
    d.has_fx_key = true;
    let mut found_at = [false; 5];
    for i in 0..5 {
        match get(&fx, FX[i]) {
            Some(v) if is_eapo(&v) => {
                found_at[i] = true;
                d.originals[i] = NO_VALUE.into();
            }
            Some(v) => d.originals[i] = v,
            None => d.originals[i] = NO_VALUE.into(),
        }
    }
    if found_at.iter().any(|&f| f) {
        d.installed = true;
        if let Ok(child) = hklm().open_subkey_with_flags(format!(r"{CHILD}\{guid}"), KEY_READ) {
            for i in 0..5 {
                if let Some(v) = get(&child, FX[i]) {
                    d.originals[i] = v;
                }
            }
        }
        d.mode = if found_at[LFX] || found_at[GFX] {
            Mode::LfxGfx
        } else if found_at[EFX] {
            Mode::SfxEfx
        } else {
            Mode::SfxMfx
        };
    } else {
        let has = |n: &str| get(&fx, n).is_some();
        let legacy_only = (has(FX[LFX]) || has(FX[GFX])) && ![FX[SFX], FX[MFX], FX[EFX]].iter().chain(&MULTI_FX).any(|n| has(n));
        let combined = dev_key
            .open_subkey_with_flags("Properties", KEY_READ)
            .is_ok_and(|p| p.get_raw_value(COMBINED_DEVICE).is_ok());
        d.mode = if legacy_only {
            Mode::LfxGfx
        } else if combined {
            // Combined Bluetooth endpoints on Windows 11: EFX doesn't work there.
            Mode::SfxMfx
        } else {
            Mode::SfxEfx
        };
    }
    Ok(d)
}

fn clean(g: &str) -> String {
    if g == NO_KEY || g == NO_VALUE {
        String::new()
    } else {
        g.to_string()
    }
}

/// The driver's own pre-mix APO that Equalizer APO should keep running as a child.
fn original_pre(d: &Dev, mode: Mode) -> String {
    let o = &d.originals;
    let g = match mode {
        Mode::LfxGfx if o[LFX] == NO_VALUE && o[GFX] == NO_VALUE => &o[SFX],
        Mode::LfxGfx => &o[LFX],
        Mode::SfxMfx if o[SFX] == NO_VALUE && o[MFX] == NO_VALUE => &o[LFX],
        Mode::SfxEfx if o[SFX] == NO_VALUE && o[EFX] == NO_VALUE => &o[LFX],
        _ => &o[SFX],
    };
    clean(g)
}

fn original_post(d: &Dev, mode: Mode) -> String {
    let o = &d.originals;
    let g = match mode {
        Mode::LfxGfx if o[LFX] == NO_VALUE && o[GFX] == NO_VALUE => &o[MFX],
        Mode::LfxGfx => &o[GFX],
        Mode::SfxMfx if o[SFX] == NO_VALUE && o[MFX] == NO_VALUE => &o[GFX],
        Mode::SfxMfx => &o[MFX],
        Mode::SfxEfx if o[SFX] == NO_VALUE && o[EFX] == NO_VALUE => &o[GFX],
        Mode::SfxEfx => &o[EFX],
    };
    clean(g)
}

fn install(d: &Dev, mode: Mode) -> io::Result<()> {
    if !d.has_fx_key {
        return Err(io::Error::other(
            "this device has no effects key; use Equalizer APO's Device Selector for it",
        ));
    }
    let (child, _) = hklm().create_subkey_with_flags(format!(r"{CHILD}\{}", d.guid), KEY_READ | KEY_WRITE)?;
    let fx = hklm().open_subkey_with_flags(format!(r"{RENDER}\{}\FxProperties", d.guid), KEY_READ | KEY_SET_VALUE)?;
    // Back up what's there now (on a fresh install, the driver's APOs).
    for name in FX {
        child.set_value(name, &get(&fx, name).unwrap_or_else(|| NO_VALUE.into()))?;
    }
    child.set_value("PreMixChild", &original_pre(d, mode))?;
    child.set_value("PostMixChild", &original_post(d, mode))?;
    child.set_value("AllowSilentBufferModification", &"false")?;
    del(&child, "DisableAutomaticAdjustment")?;
    child.set_value("Version", &INSTALL_VERSION)?;

    match mode {
        Mode::LfxGfx => {
            fx.set_value(FX[LFX], &PRE_MIX)?;
            fx.set_value(FX[GFX], &POST_MIX)?;
            for i in [SFX, MFX, EFX] {
                del(&fx, FX[i])?;
            }
        }
        Mode::SfxMfx | Mode::SfxEfx => {
            del(&fx, FX[LFX])?;
            del(&fx, FX[GFX])?;
            fx.set_value(FX[SFX], &PRE_MIX)?;
            if get(&fx, SFX_MODES).is_none() && fx.get_raw_value(SFX_MODES).is_err() {
                set_multi(&fx, SFX_MODES, DEFAULT_PROCESSING_MODE)?;
            }
            let (slot, modes) = if mode == Mode::SfxMfx { (MFX, MFX_MODES) } else { (EFX, EFX_MODES) };
            fx.set_value(FX[slot], &POST_MIX)?;
            if fx.get_raw_value(modes).is_err() {
                set_multi(&fx, modes, DEFAULT_PROCESSING_MODE)?;
            }
        }
    }
    // Equalizer APO only runs when "audio enhancements" are on.
    del(&fx, DISABLE_ENHANCEMENTS)
}

fn uninstall(d: &Dev) -> io::Result<()> {
    if d.has_fx_key {
        let fx = hklm().open_subkey_with_flags(format!(r"{RENDER}\{}\FxProperties", d.guid), KEY_READ | KEY_SET_VALUE)?;
        for i in 0..5 {
            let o = &d.originals[i];
            if o == NO_VALUE || o == NO_KEY {
                if get(&fx, FX[i]).is_some_and(|v| is_eapo(&v)) || o == NO_VALUE {
                    del(&fx, FX[i])?;
                }
            } else if !o.is_empty() {
                fx.set_value(FX[i], o)?;
            }
        }
    }
    let _ = hklm().delete_subkey_all(format!(r"{CHILD}\{}", d.guid));
    Ok(())
}

/// The registry fixes Device Selector applies on every run.
fn system_fixes(install_dir: &Path) -> bool {
    let mut changed = false;
    if let Ok((k, _)) = hklm().create_subkey(r"SOFTWARE\Microsoft\Windows\CurrentVersion\Audio")
        && k.get_value::<u32, _>("DisableProtectedAudioDG").ok() != Some(1)
    {
        let _ = k.set_value("DisableProtectedAudioDG", &1u32);
        changed = true;
    }
    let hkcr = RegKey::predef(HKEY_CLASSES_ROOT);
    let registered = [PRE_MIX, POST_MIX].iter().all(|g| {
        hkcr.open_subkey(format!(r"AudioEngine\AudioProcessingObjects\{g}")).is_ok() && hkcr.open_subkey(format!(r"CLSID\{g}")).is_ok()
    });
    if !registered {
        let dll = install_dir.join("EqualizerAPO.dll");
        let _ = std::process::Command::new("regsvr32.exe").arg("/s").arg(dll).status();
        changed = true;
    }
    changed
}

fn restart_audio_service() -> Result<(), String> {
    use windows::Win32::System::Services::*;
    use windows::core::w;
    unsafe {
        let scm = OpenSCManagerW(None, None, SC_MANAGER_CONNECT).map_err(|e| format!("service manager: {e}"))?;
        let svc = OpenServiceW(
            scm,
            w!("AudioSrv"),
            SERVICE_START | SERVICE_STOP | SERVICE_QUERY_STATUS | SERVICE_ENUMERATE_DEPENDENTS,
        )
        .map_err(|e| format!("AudioSrv: {e}"))?;
        // Running dependents must stop first and start again afterwards.
        let mut deps: Vec<SC_HANDLE> = Vec::new();
        let mut needed = 0u32;
        let mut count = 0u32;
        let _ = EnumDependentServicesW(svc, SERVICE_ACTIVE, None, 0, &mut needed, &mut count);
        if needed > 0 {
            let mut buf = vec![0u8; needed as usize];
            if EnumDependentServicesW(
                svc,
                SERVICE_ACTIVE,
                Some(buf.as_mut_ptr() as *mut ENUM_SERVICE_STATUSW),
                needed,
                &mut needed,
                &mut count,
            )
            .is_ok()
            {
                let list = std::slice::from_raw_parts(buf.as_ptr() as *const ENUM_SERVICE_STATUSW, count as usize);
                for s in list {
                    if let Ok(h) = OpenServiceW(scm, s.lpServiceName, SERVICE_START | SERVICE_STOP | SERVICE_QUERY_STATUS) {
                        deps.push(h);
                    }
                }
            }
        }
        let state = |h: SC_HANDLE| {
            let mut st = SERVICE_STATUS::default();
            QueryServiceStatus(h, &mut st).map(|_| st.dwCurrentState).unwrap_or(SERVICE_STOPPED)
        };
        let t = Instant::now();
        for &h in deps.iter().chain(std::iter::once(&svc)) {
            let mut st = SERVICE_STATUS::default();
            let _ = ControlService(h, SERVICE_CONTROL_STOP, &mut st);
            while state(h) != SERVICE_STOPPED {
                if t.elapsed() > Duration::from_secs(30) {
                    return Err("the audio service didn't stop".into());
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        }
        for &h in std::iter::once(&svc).chain(deps.iter()) {
            let _ = StartServiceW(h, None);
            let mut retry = t.elapsed() + Duration::from_secs(5);
            while state(h) != SERVICE_RUNNING {
                if t.elapsed() > Duration::from_secs(40) {
                    return Err("the audio service didn't start again; reboot to restore sound".into());
                }
                if t.elapsed() > retry {
                    let _ = StartServiceW(h, None);
                    retry = t.elapsed() + Duration::from_secs(5);
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        }
        for h in deps {
            let _ = CloseServiceHandle(h);
        }
        let _ = CloseServiceHandle(svc);
        let _ = CloseServiceHandle(scm);
    }
    Ok(())
}

#[derive(Clone, Copy, Default)]
struct Probe {
    pre: bool,
    post: bool,
    pre_child: bool,
    post_child: bool,
}

impl Probe {
    fn ok(&self) -> bool {
        self.pre && self.post && self.pre_child && self.post_child
    }
    fn score(&self) -> u32 {
        self.pre as u32 * 2 + self.post as u32 * 2 + self.pre_child as u32 + self.post_child as u32
    }
}

/// Pipe server that collects the APO's JSON messages until told to stop.
fn pipe_server() -> Receiver<String> {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::Security::{InitializeSecurityDescriptor, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES, SetSecurityDescriptorDacl};
    use windows::Win32::Storage::FileSystem::{PIPE_ACCESS_INBOUND, ReadFile};
    use windows::Win32::System::Pipes::*;
    use windows::core::HSTRING;
    let (tx, rx) = channel();
    std::thread::spawn(move || unsafe {
        // Null DACL: the audio engine (another account) must be able to connect.
        let mut sd = vec![0u8; 64];
        let psd = PSECURITY_DESCRIPTOR(sd.as_mut_ptr() as _);
        if InitializeSecurityDescriptor(psd, 1).is_err() || SetSecurityDescriptorDacl(psd, true, None, false).is_err() {
            return;
        }
        let sa = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: psd.0,
            bInheritHandle: false.into(),
        };
        let name = HSTRING::from(format!(r"\\.\pipe\{PIPE}"));
        loop {
            let pipe = CreateNamedPipeW(
                &name,
                PIPE_ACCESS_INBOUND,
                PIPE_TYPE_MESSAGE | PIPE_READMODE_MESSAGE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                PIPE_UNLIMITED_INSTANCES,
                0,
                1024,
                0,
                Some(&sa),
            );
            if pipe.is_invalid() {
                return;
            }
            let connected = ConnectNamedPipe(pipe, None).is_ok()
                || windows::Win32::Foundation::GetLastError() == windows::Win32::Foundation::ERROR_PIPE_CONNECTED;
            let mut msg = None;
            if connected {
                let mut buf = [0u8; 1024];
                let mut n = 0u32;
                if ReadFile(pipe, Some(&mut buf), Some(&mut n), None).is_ok() {
                    msg = Some(String::from_utf8_lossy(&buf[..n as usize]).into_owned());
                }
                let _ = DisconnectNamedPipe(pipe);
            }
            let _ = CloseHandle(pipe);
            match msg.as_deref() {
                Some("stop") => return,
                Some(m) if tx.send(m.to_string()).is_err() => return,
                _ => {}
            }
        }
    });
    rx
}

fn stop_pipe_server() {
    use windows::Win32::Foundation::{CloseHandle, GENERIC_WRITE};
    use windows::Win32::Storage::FileSystem::{CreateFileW, FILE_FLAGS_AND_ATTRIBUTES, FILE_SHARE_NONE, OPEN_EXISTING, WriteFile};
    use windows::core::HSTRING;
    let name = HSTRING::from(format!(r"\\.\pipe\{PIPE}"));
    for _ in 0..20 {
        unsafe {
            if let Ok(h) = CreateFileW(
                &name,
                GENERIC_WRITE.0,
                FILE_SHARE_NONE,
                None,
                OPEN_EXISTING,
                FILE_FLAGS_AND_ATTRIBUTES(0),
                None,
            ) {
                let _ = WriteFile(h, Some(b"stop"), None, None);
                let _ = CloseHandle(h);
                return;
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Open a shared-mode stream so the audio engine loads the endpoint's APOs.
fn poke(guid: &str) {
    use windows::Win32::Media::Audio::{AUDCLNT_SHAREMODE_SHARED, IAudioClient, IMMDeviceEnumerator, MMDeviceEnumerator};
    use windows::Win32::System::Com::{CLSCTX_ALL, CoCreateInstance, CoTaskMemFree};
    use windows::core::HSTRING;
    crate::sys::com_init();
    unsafe {
        let Ok(en) = CoCreateInstance::<_, IMMDeviceEnumerator>(&MMDeviceEnumerator, None, CLSCTX_ALL) else {
            return;
        };
        let Ok(dev) = en.GetDevice(&HSTRING::from(format!("{{0.0.0.00000000}}.{guid}"))) else {
            return;
        };
        let Ok(client) = dev.Activate::<IAudioClient>(CLSCTX_ALL, None) else {
            return;
        };
        let Ok(fmt) = client.GetMixFormat() else { return };
        let _ = client.Initialize(AUDCLNT_SHAREMODE_SHARED, 0, 1_000_000, 0, fmt, None);
        CoTaskMemFree(Some(fmt as _));
    }
}

/// Ask each device to load its APOs and collect what Equalizer APO reports.
fn test(devs: &[&Dev], modes: &HashMap<String, Mode>) -> HashMap<String, Probe> {
    let mut probes: HashMap<String, Probe> = devs
        .iter()
        .map(|d| {
            let m = modes[&d.guid];
            (
                d.guid.clone(),
                Probe {
                    pre_child: original_pre(d, m).is_empty(),
                    post_child: original_post(d, m).is_empty(),
                    ..Default::default()
                },
            )
        })
        .collect();
    let rx = pipe_server();
    let app = hklm().open_subkey_with_flags(APP, KEY_READ | KEY_WRITE);
    if let Ok(k) = &app {
        let _ = k.set_value("DeviceTestPipeName", &PIPE);
    }
    std::thread::sleep(Duration::from_millis(100));
    for d in devs {
        poke(&d.guid);
    }
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline && !probes.values().all(Probe::ok) {
        let Ok(m) = rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) else {
            break;
        };
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&m) else {
            continue;
        };
        let guid = v["deviceGuid"].as_str().unwrap_or("").to_ascii_lowercase();
        let (stage, phase) = (v["stage"].as_str().unwrap_or(""), v["phase"].as_str().unwrap_or(""));
        if let Some(p) = probes.get_mut(&guid) {
            match (stage, phase) {
                ("PreMix", "Initialize") => p.pre = true,
                ("PreMix", "ChildAPO") => p.pre_child = true,
                ("PostMix", "Initialize") => p.post = true,
                ("PostMix", "ChildAPO") => p.post_child = true,
                _ => {}
            }
        }
    }
    stop_pipe_server();
    if let Ok(k) = &app {
        let _ = k.delete_value("DeviceTestPipeName");
    }
    probes
}

/// The whole job: uninstall, install, restart audio, verify, auto-adjust modes.
pub fn setup(install_guids: &[String], uninstall_guids: &[String]) -> Report {
    let mut rep = Report::default();
    let Some(apo) = crate::apo::detect() else {
        rep.error = Some("Equalizer APO is not installed".into());
        return rep;
    };
    rep.reboot_suggested = system_fixes(&apo.install);

    for g in uninstall_guids {
        let mut r = DeviceResult {
            guid: g.clone(),
            ..Default::default()
        };
        match load(g).and_then(|d| if d.installed { uninstall(&d) } else { Ok(()) }) {
            Ok(()) => r.message = "removed".into(),
            Err(e) => r.message = format!("couldn't remove: {e}"),
        }
        r.installed = load(g).is_ok_and(|d| d.installed);
        rep.devices.push(r);
    }

    let mut modes: HashMap<String, Mode> = HashMap::new();
    let mut remaining: HashMap<String, Vec<Mode>> = HashMap::new();
    let mut testable = Vec::new();
    for g in install_guids {
        let mut r = DeviceResult {
            guid: g.clone(),
            ..Default::default()
        };
        let res = load(g).and_then(|d| {
            if !d.installed {
                install(&d, d.mode)?;
            }
            Ok(d)
        });
        match res {
            Ok(d) => {
                r.installed = true;
                r.mode = Some(d.mode);
                modes.insert(g.clone(), d.mode);
                remaining.insert(
                    g.clone(),
                    [Mode::SfxEfx, Mode::SfxMfx, Mode::LfxGfx]
                        .into_iter()
                        .filter(|m| *m != d.mode)
                        .collect(),
                );
                if d.active {
                    testable.push(g.clone());
                } else {
                    r.message = "installed; it'll be checked next time it's connected".into();
                }
            }
            Err(e) => r.message = format!("couldn't install: {e}"),
        }
        rep.devices.push(r);
    }

    if install_guids.is_empty() && uninstall_guids.is_empty() {
        return rep;
    }
    if let Err(e) = restart_audio_service() {
        rep.error = Some(e);
        rep.reboot_suggested = true;
        return rep;
    }

    // Verify, switching install mode for devices whose APO didn't load.
    let mut best: HashMap<String, (u32, Mode)> = HashMap::new();
    let mut pending: HashSet<String> = testable.iter().cloned().collect();
    while !pending.is_empty() {
        let devs: Vec<Dev> = pending.iter().filter_map(|g| load(g).ok()).collect();
        let refs: Vec<&Dev> = devs.iter().collect();
        let probes = test(&refs, &modes);
        let mut retry = false;
        for d in &devs {
            let p = probes.get(&d.guid).copied().unwrap_or_default();
            let mode = modes[&d.guid];
            let entry = rep.devices.iter_mut().find(|r| r.guid == d.guid).unwrap();
            if p.ok() {
                pending.remove(&d.guid);
                entry.verified = Some(true);
                entry.mode = Some(mode);
                entry.message = "working".into();
                continue;
            }
            if best.get(&d.guid).is_none_or(|b| p.score() > b.0) {
                best.insert(d.guid.clone(), (p.score(), mode));
            }
            let next = remaining.get_mut(&d.guid).and_then(|v| (!v.is_empty()).then(|| v.remove(0)));
            let (mode, last) = match next {
                Some(m) => (m, false),
                None => (best[&d.guid].1, true),
            };
            if mode != modes[&d.guid] {
                let _ = uninstall(d);
                if let Ok(fresh) = load(&d.guid) {
                    let _ = install(&fresh, mode);
                }
                modes.insert(d.guid.clone(), mode);
                retry = true;
            }
            entry.mode = Some(mode);
            if last {
                pending.remove(&d.guid);
                entry.verified = Some(best[&d.guid].0 >= 4);
                entry.message = if best[&d.guid].0 >= 4 {
                    "working (the driver's own effects couldn't be kept)".into()
                } else {
                    "installed but not confirmed; a reboot usually fixes this".into()
                };
                rep.reboot_suggested = true;
            }
        }
        if retry && let Err(e) = restart_audio_service() {
            rep.error = Some(e);
            rep.reboot_suggested = true;
            break;
        }
    }
    rep
}

/// Official Equalizer APO 1.4.2 installers and their SHA-256 (matching SourceForge's published MD5/SHA-1).
const EAPO_X64: (&str, &str) = (
    "https://sourceforge.net/projects/equalizerapo/files/1.4.2/EqualizerAPO-x64-1.4.2.exe/download",
    "7403be7427bbe1936a40dded082829b6e217fc4f5990fee5cba501f0ae055afa",
);
const EAPO_X86: (&str, &str) = (
    "https://sourceforge.net/projects/equalizerapo/files/1.4.2/EqualizerAPO-x86-1.4.2.exe/download",
    "15bdaba028b8a7805e17805e4596c27ab240381b5d5044cf247fa80717341b35",
);

/// Download and verify the Equalizer APO installer for this machine.
pub fn download_equalizer_apo() -> Result<std::path::PathBuf, String> {
    use sha2::{Digest, Sha256};
    use std::io::Read;
    let (url, sha) = match std::env::consts::ARCH {
        "x86_64" => EAPO_X64,
        "x86" => EAPO_X86,
        other => return Err(format!("Equalizer APO isn't available for {other} Windows")),
    };
    let dir = std::env::temp_dir().join("equinox-setup");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join(if sha == EAPO_X64.1 {
        "EqualizerAPO-x64-1.4.2.exe"
    } else {
        "EqualizerAPO-x86-1.4.2.exe"
    });
    let verify = |bytes: &[u8]| Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect::<String>() == sha;
    if std::fs::read(&path).is_ok_and(|b| verify(&b)) {
        return Ok(path);
    }
    let resp = ureq::get(url)
        .timeout(Duration::from_secs(300))
        .call()
        .map_err(|e| format!("download failed: {e}"))?;
    let mut bytes = Vec::new();
    resp.into_reader()
        .take(64 << 20)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("download failed: {e}"))?;
    if !verify(&bytes) {
        return Err("the downloaded Equalizer APO installer didn't match its checksum, so it wasn't run".into());
    }
    std::fs::write(&path, &bytes).map_err(|e| e.to_string())?;
    Ok(path)
}

/// Elevated entry point: `equinox.exe --apo-setup <report.json> +{guid} -{guid} ...`.
pub fn elevated_main(args: &[String]) {
    let Some(out) = args.first() else { return };
    if let Some(installer) = args[1..].iter().find_map(|a| a.strip_prefix('@')) {
        // Silent install of Equalizer APO itself, then carry on with the devices.
        let ok = std::process::Command::new(installer).arg("/S").status().is_ok_and(|s| s.success());
        if !ok || crate::apo::detect().is_none() {
            let rep = Report {
                error: Some("Equalizer APO's installer didn't finish".into()),
                ..Default::default()
            };
            let _ = std::fs::write(out, serde_json::to_string(&rep).unwrap_or_default());
            return;
        }
    }
    let install: Vec<String> = args[1..]
        .iter()
        .filter_map(|a| a.strip_prefix('+'))
        .map(str::to_ascii_lowercase)
        .collect();
    let remove: Vec<String> = args[1..]
        .iter()
        .filter_map(|a| a.strip_prefix('-'))
        .map(str::to_ascii_lowercase)
        .collect();
    let rep = setup(&install, &remove);
    let _ = std::fs::write(out, serde_json::to_string(&rep).unwrap_or_default());
}

/// Run [`setup`] in an elevated copy of the app (one UAC prompt) and wait for it.
pub fn run_elevated(install: &[String], uninstall: &[String], installer: Option<&Path>) -> Result<Report, String> {
    use windows::Win32::Foundation::{CloseHandle, ERROR_CANCELLED, WAIT_TIMEOUT};
    use windows::Win32::System::Threading::WaitForSingleObject;
    use windows::Win32::UI::Shell::{SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, ShellExecuteExW};
    use windows::Win32::UI::WindowsAndMessaging::SW_HIDE;
    use windows::core::{HSTRING, w};

    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let out = std::env::temp_dir().join(format!("equinox-apo-{}-{stamp}.json", std::process::id()));
    let _ = std::fs::remove_file(&out);
    let mut params = format!("--apo-setup \"{}\"", out.display());
    if let Some(p) = installer {
        params += &format!(" \"@{}\"", p.display());
    }
    for g in install {
        params += &format!(" +{g}");
    }
    for g in uninstall {
        params += &format!(" -{g}");
    }
    let exe = HSTRING::from(crate::paths::app_exe().as_os_str());
    let params = HSTRING::from(params);
    let mut info = SHELLEXECUTEINFOW {
        cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOCLOSEPROCESS,
        lpVerb: w!("runas"),
        lpFile: windows::core::PCWSTR(exe.as_ptr()),
        lpParameters: windows::core::PCWSTR(params.as_ptr()),
        nShow: SW_HIDE.0,
        ..Default::default()
    };
    unsafe {
        if let Err(e) = ShellExecuteExW(&mut info) {
            return Err(if e.code() == ERROR_CANCELLED.to_hresult() {
                "cancelled".into()
            } else {
                format!("couldn't start setup: {e}")
            });
        }
        let r = WaitForSingleObject(info.hProcess, 600_000);
        let _ = CloseHandle(info.hProcess);
        if r == WAIT_TIMEOUT {
            return Err("setup timed out".into());
        }
    }
    let text = std::fs::read_to_string(&out).map_err(|_| "setup didn't report back".to_string())?;
    let _ = std::fs::remove_file(&out);
    serde_json::from_str(&text).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dev(o: [&str; 5]) -> Dev {
        Dev {
            guid: "{x}".into(),
            originals: o.map(String::from),
            installed: false,
            has_fx_key: true,
            mode: Mode::SfxEfx,
            active: true,
        }
    }

    #[test]
    fn child_apo_selection_matches_device_selector() {
        // Driver with only SFX/MFX: both kept as children in SFX modes.
        let d = dev([NO_VALUE, NO_VALUE, "{S}", "{M}", NO_VALUE]);
        assert_eq!(original_pre(&d, Mode::SfxMfx), "{S}");
        assert_eq!(original_post(&d, Mode::SfxMfx), "{M}");
        assert_eq!(original_post(&d, Mode::SfxEfx), "");
        // LFX/GFX mode falls back to SFX/MFX when there are no legacy APOs.
        assert_eq!(original_pre(&d, Mode::LfxGfx), "{S}");
        assert_eq!(original_post(&d, Mode::LfxGfx), "{M}");
        // Legacy-only driver in SFX mode keeps the legacy APOs.
        let l = dev(["{L}", "{G}", NO_VALUE, NO_VALUE, NO_VALUE]);
        assert_eq!(original_pre(&l, Mode::SfxEfx), "{L}");
        assert_eq!(original_post(&l, Mode::SfxEfx), "{G}");
        let none = dev([NO_KEY; 5]);
        assert_eq!(original_pre(&none, Mode::SfxEfx), "");
    }
}

#[cfg(test)]
mod live {
    /// Read-only dump of what setup would do on this machine: `cargo test plan -- --ignored --nocapture`.
    /// Downloads (about 12 MB) and verifies the installer without running it.
    #[test]
    #[ignore]
    fn installer_download() {
        let p = super::download_equalizer_apo().unwrap();
        assert!(std::fs::metadata(p).unwrap().len() > 5_000_000);
    }

    #[test]
    #[ignore]
    fn plan() {
        for d in crate::devices::list().iter().filter(|d| d.active) {
            match super::load(&d.id) {
                Ok(x) => println!(
                    "{:<45} installed={} fx_key={} mode={:?} keep pre={:?} post={:?}",
                    d.label(),
                    x.installed,
                    x.has_fx_key,
                    x.mode,
                    super::original_pre(&x, x.mode),
                    super::original_post(&x, x.mode)
                ),
                Err(e) => println!("{}: {e}", d.label()),
            }
        }
    }
}
