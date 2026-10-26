//! Small Win32 helpers: default output device, running processes, single
//! instance, start with Windows, PATH and window focus.

use std::collections::HashSet;

use windows::Win32::Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE};
use windows::Win32::Media::Audio::{IMMDeviceEnumerator, MMDeviceEnumerator, eConsole, eRender};
use windows::Win32::System::Com::{CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::{FindWindowW, IsIconic, SW_RESTORE, SW_SHOWNORMAL, SetForegroundWindow, ShowWindow};
use windows::core::{HSTRING, PCWSTR, w};
use winreg::RegKey;
use winreg::enums::{HKEY_CURRENT_USER, KEY_READ, KEY_WRITE};

pub const WINDOW_TITLE: &str = "equinox";
const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const RUN_VALUE: &str = "equinox";

/// COM on the calling thread; harmless if the thread already has it (any model).
pub fn com_init() {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }
}

/// GUID of the default render endpoint ("{...}", lowercase).
pub fn default_output() -> Option<String> {
    com_init();
    unsafe {
        let en: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).ok()?;
        let dev = en.GetDefaultAudioEndpoint(eRender, eConsole).ok()?;
        let id = dev.GetId().ok()?;
        let s = id.to_string().ok();
        CoTaskMemFree(Some(id.0 as _));
        s.map(|s| endpoint_guid(&s))
    }
}

/// "{0.0.0.00000000}.{guid}" -> "{guid}".
pub fn endpoint_guid(id: &str) -> String {
    match id.rfind('{') {
        Some(i) => id[i..].to_ascii_lowercase(),
        None => id.to_ascii_lowercase(),
    }
}

/// Lowercase exe names of every running process.
pub fn processes() -> HashSet<String> {
    let mut out = HashSet::new();
    unsafe {
        let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else {
            return out;
        };
        let mut e = PROCESSENTRY32W {
            dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        if Process32FirstW(snap, &mut e).is_ok() {
            loop {
                let len = e.szExeFile.iter().position(|&c| c == 0).unwrap_or(e.szExeFile.len());
                out.insert(String::from_utf16_lossy(&e.szExeFile[..len]).to_lowercase());
                if Process32NextW(snap, &mut e).is_err() {
                    break;
                }
            }
        }
        let _ = CloseHandle(snap);
    }
    out
}

/// Holds a named mutex for the life of the process. `None` if another instance has it.
pub struct Instance(#[allow(dead_code)] HANDLE);

/// Kernel object name, made unique per data folder when `EQUINOX_HOME` is set,
/// so a test copy can run next to the real app.
pub fn scoped(name: &str) -> String {
    match std::env::var_os("EQUINOX_HOME") {
        Some(home) => {
            use std::hash::{Hash, Hasher};
            let mut h = std::collections::hash_map::DefaultHasher::new();
            home.hash(&mut h);
            format!("{name}-{:x}", h.finish())
        }
        None => name.to_string(),
    }
}

pub fn single_instance(name: &str) -> Option<Instance> {
    unsafe {
        let h = CreateMutexW(None, true, &HSTRING::from(scoped(name))).ok()?;
        if GetLastError() == ERROR_ALREADY_EXISTS {
            let _ = CloseHandle(h);
            return None;
        }
        Some(Instance(h))
    }
}

pub fn agent_running() -> bool {
    match single_instance(AGENT_MUTEX) {
        Some(i) => {
            unsafe {
                let _ = CloseHandle(i.0);
            }
            false
        }
        None => true,
    }
}

pub const AGENT_MUTEX: &str = r"Local\equinox-agent";
pub const APP_MUTEX: &str = r"Local\equinox-app";

/// Bring an already open equinox window to the front. True if one was found.
pub fn focus_app_window() -> bool {
    unsafe {
        let Ok(hwnd) = FindWindowW(PCWSTR::null(), &HSTRING::from(WINDOW_TITLE)) else {
            return false;
        };
        if hwnd.is_invalid() {
            return false;
        }
        if IsIconic(hwnd).as_bool() {
            let _ = ShowWindow(hwnd, SW_RESTORE);
        }
        let _ = SetForegroundWindow(hwnd);
        true
    }
}

pub fn open_app() {
    if !focus_app_window() {
        let _ = std::process::Command::new(crate::paths::app_exe()).spawn();
    }
}

pub fn start_agent() {
    if !agent_running() {
        let _ = std::process::Command::new(crate::paths::app_exe()).arg("--tray").spawn();
    }
}

/// Run something elevated (UAC prompt), e.g. the Equalizer APO device selector.
pub fn run_elevated(path: &std::path::Path) -> bool {
    unsafe {
        let r = ShellExecuteW(
            None,
            w!("runas"),
            &HSTRING::from(path.as_os_str()),
            PCWSTR::null(),
            // Qt tools (Device Selector) find their plugins relative to the working dir.
            &HSTRING::from(path.parent().map(|p| p.as_os_str()).unwrap_or_default()),
            SW_SHOWNORMAL,
        );
        r.0 as usize > 32
    }
}

pub fn open_path(path: &std::path::Path) {
    unsafe {
        ShellExecuteW(
            None,
            w!("open"),
            &HSTRING::from(path.as_os_str()),
            PCWSTR::null(),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        );
    }
}

pub fn autostart_enabled() -> bool {
    RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey_with_flags(RUN_KEY, KEY_READ)
        .and_then(|k| k.get_value::<String, _>(RUN_VALUE))
        .is_ok()
}

/// Starts the tray agent at sign-in. Enabling copies equinox.exe and eqx.exe to
/// `%LOCALAPPDATA%\equinox` first, so the Run entry survives moving or rebuilding the originals.
pub fn set_autostart(on: bool) -> std::io::Result<()> {
    let key = RegKey::predef(HKEY_CURRENT_USER).open_subkey_with_flags(RUN_KEY, KEY_READ | KEY_WRITE)?;
    if on {
        let exe = install()?;
        key.set_value(RUN_VALUE, &format!("\"{}\" --tray", exe.display()))
    } else {
        match key.delete_value(RUN_VALUE) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            r => r,
        }
    }
}

pub fn install_dir() -> std::path::PathBuf {
    let base = std::env::var_os("LOCALAPPDATA")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    base.join("equinox")
}

/// Copy both executables into the install dir (stopping an agent that runs from
/// there) and return the installed app path.
fn install() -> std::io::Result<std::path::PathBuf> {
    let src = crate::paths::app_exe();
    let dir = install_dir();
    let dst = dir.join("equinox.exe");
    if same_file(&src, &dst) {
        return Ok(dst);
    }
    std::fs::create_dir_all(&dir)?;
    let mut stopped = false;
    for name in ["equinox.exe", "eqx.exe"] {
        let from = src.with_file_name(name);
        if !from.exists() {
            continue;
        }
        let to = dir.join(name);
        if std::fs::copy(&from, &to).is_err() {
            // Probably the installed agent holding the file: stop it and retry.
            if !stopped {
                stop_agent();
                stopped = true;
            }
            std::fs::copy(&from, &to)?;
        }
    }
    Ok(dst)
}

fn same_file(a: &std::path::Path, b: &std::path::Path) -> bool {
    matches!((a.canonicalize(), b.canonicalize()), (Ok(a), Ok(b)) if a == b)
}

pub const AGENT_QUIT_EVENT: &str = r"Local\equinox-agent-quit";

/// Ask the tray agent to exit and wait (up to 3 s) until it has.
pub fn stop_agent() {
    use windows::Win32::System::Threading::{EVENT_MODIFY_STATE, OpenEventW, SetEvent};
    unsafe {
        if let Ok(h) = OpenEventW(EVENT_MODIFY_STATE, false, &HSTRING::from(scoped(AGENT_QUIT_EVENT))) {
            let _ = SetEvent(h);
            let _ = CloseHandle(h);
        }
    }
    let t = std::time::Instant::now();
    while agent_running() && t.elapsed() < std::time::Duration::from_secs(3) {
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

/// Whether `dir` is on the user's PATH.
pub fn on_user_path(dir: &std::path::Path) -> bool {
    let want = dir.to_string_lossy().trim_end_matches('\\').to_lowercase();
    let path: String = RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey("Environment")
        .and_then(|k| k.get_value("Path"))
        .unwrap_or_default();
    path.split(';').any(|p| p.trim().trim_end_matches('\\').to_lowercase() == want)
}

/// Append `dir` to the user PATH (keeping REG_EXPAND_SZ) and tell running shells.
pub fn add_to_user_path(dir: &std::path::Path) -> std::io::Result<()> {
    use windows::Win32::Foundation::{LPARAM, WPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{HWND_BROADCAST, SMTO_ABORTIFHUNG, SendMessageTimeoutW, WM_SETTINGCHANGE};
    use winreg::enums::RegType;
    if on_user_path(dir) {
        return Ok(());
    }
    let (env, _) = RegKey::predef(HKEY_CURRENT_USER).create_subkey("Environment")?;
    let cur: String = env.get_value("Path").unwrap_or_default();
    let new = if cur.trim().is_empty() {
        dir.display().to_string()
    } else {
        format!("{};{}", cur.trim_end_matches(';'), dir.display())
    };
    let bytes: Vec<u8> = new.encode_utf16().chain(std::iter::once(0)).flat_map(u16::to_le_bytes).collect();
    env.set_raw_value(
        "Path",
        &winreg::RegValue {
            bytes: bytes.into(),
            vtype: RegType::REG_EXPAND_SZ,
        },
    )?;
    let what = HSTRING::from("Environment");
    unsafe {
        SendMessageTimeoutW(
            HWND_BROADCAST,
            WM_SETTINGCHANGE,
            WPARAM(0),
            LPARAM(what.as_ptr() as isize),
            SMTO_ABORTIFHUNG,
            2000,
            None,
        );
    }
    Ok(())
}

/// Minutes since local midnight.
pub fn local_minutes() -> u32 {
    let t = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
    t.wHour as u32 * 60 + t.wMinute as u32
}
