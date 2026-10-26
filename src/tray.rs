//! Background agent (`equinox --tray`): tray icon and menu, global hotkeys,
//! and the loop that re-applies when rules, devices or the state file change.
//! Equalizer APO keeps the EQ running on its own; this only adds automation.

use std::time::{Duration, Instant, SystemTime};

use equinox::devices::{self, AudioDevice};
use equinox::engine;
use equinox::model::{Mode, State, Target};
use equinox::sys;
use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem, Submenu};
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
use windows::Win32::Foundation::WAIT_OBJECT_0;
use windows::Win32::System::Threading::CreateEventW;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    HOT_KEY_MODIFIERS, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, RegisterHotKey, UnregisterHotKey,
};
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, MSG, MsgWaitForMultipleObjects, PM_REMOVE, PeekMessageW, QS_ALLINPUT, TranslateMessage, WM_HOTKEY,
};
use windows::core::HSTRING;

const HK_TOGGLE: i32 = 1;
const HK_NEXT: i32 = 2;
const HK_PREV: i32 = 3;

struct Agent {
    st: State,
    mtime: Option<SystemTime>,
    devices: Vec<AudioDevice>,
    tray: TrayIcon,
    open: MenuItem,
    enabled: CheckMenuItem,
    profiles: Submenu,
    profile_items: Vec<(MenuId, u64, CheckMenuItem)>,
    profile_sig: Vec<(u64, String)>,
    startup: CheckMenuItem,
    quit: MenuItem,
    hotkeys: bool,
    last_bypass: Option<bool>,
}

pub fn run() {
    let Some(_guard) = sys::single_instance(sys::AGENT_MUTEX) else {
        return;
    };
    sys::com_init();
    // Signalled by `sys::stop_agent` (e.g. before updating the installed exe).
    let quit = unsafe { CreateEventW(None, true, false, &HSTRING::from(sys::scoped(sys::AGENT_QUIT_EVENT))) }.ok();
    let Ok(mut agent) = Agent::new() else { return };
    let mut last_tick = Instant::now() - Duration::from_secs(10);
    loop {
        unsafe {
            let mut msg = MSG::default();
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                if msg.message == WM_HOTKEY {
                    agent.hotkey(msg.wParam.0 as i32);
                }
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        while let Ok(e) = TrayIconEvent::receiver().try_recv() {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = e
            {
                sys::open_app();
            }
        }
        while let Ok(e) = MenuEvent::receiver().try_recv() {
            if !agent.menu(e.id) {
                agent.unregister_hotkeys();
                return;
            }
        }
        if last_tick.elapsed() >= Duration::from_millis(1000) {
            last_tick = Instant::now();
            if !agent.tick() {
                agent.unregister_hotkeys();
                return;
            }
        }
        unsafe {
            match quit {
                Some(h) => {
                    if MsgWaitForMultipleObjects(Some(&[h]), false, 100, QS_ALLINPUT) == WAIT_OBJECT_0 {
                        agent.unregister_hotkeys();
                        return;
                    }
                }
                None => {
                    MsgWaitForMultipleObjects(None, false, 100, QS_ALLINPUT);
                }
            }
        }
    }
}

fn mtime() -> Option<SystemTime> {
    std::fs::metadata(equinox::paths::state_file()).and_then(|m| m.modified()).ok()
}

fn icon(bypassed: bool) -> Icon {
    let tint = if bypassed { [90, 90, 96] } else { [125, 249, 255] };
    Icon::from_rgba(equinox::icon::icon_rgba_tinted(32, tint), 32, 32).expect("icon")
}

impl Agent {
    fn new() -> Result<Agent, Box<dyn std::error::Error>> {
        let st = State::load();
        let menu = Menu::new();
        let open = MenuItem::new("Open equinox", true, None);
        let enabled = CheckMenuItem::new("Equalizer on", true, !st.bypass, None);
        let profiles = Submenu::new("Profile", true);
        let startup = CheckMenuItem::new("Start with Windows", true, sys::autostart_enabled(), None);
        let quit = MenuItem::new("Quit tray (EQ stays applied)", true, None);
        menu.append(&open)?;
        menu.append(&PredefinedMenuItem::separator())?;
        menu.append(&enabled)?;
        menu.append(&profiles)?;
        menu.append(&PredefinedMenuItem::separator())?;
        menu.append(&startup)?;
        menu.append(&quit)?;
        let tray = TrayIconBuilder::new()
            .with_icon(icon(st.bypass))
            .with_tooltip("equinox")
            .with_menu(Box::new(menu))
            .with_menu_on_left_click(false)
            .build()?;
        let mut a = Agent {
            mtime: mtime(),
            devices: devices::list(),
            tray,
            open,
            enabled,
            profiles,
            profile_items: Vec::new(),
            profile_sig: Vec::new(),
            startup,
            quit,
            hotkeys: false,
            last_bypass: None,
            st,
        };
        a.sync_ui();
        Ok(a)
    }

    /// The chain the tray and hotkeys act on: whatever is playing right now.
    fn target(&self) -> Target {
        match (self.st.mode, self.devices.iter().find(|d| d.default)) {
            (Mode::PerDevice, Some(d)) => Target::Device(d.id.clone()),
            _ => Target::Global,
        }
    }

    fn save_apply(&mut self) {
        let _ = self.st.save();
        self.mtime = mtime();
        let ctx = engine::Context::capture(&self.st, &self.devices);
        engine::apply(&self.st, &self.devices, &ctx);
        self.sync_ui();
    }

    /// Returns false when the agent should exit.
    fn tick(&mut self) -> bool {
        let m = mtime();
        if m != self.mtime {
            self.mtime = m;
            self.st = State::load();
        }
        if !self.st.settings.agent {
            return false;
        }
        self.devices = devices::list();
        let ctx = engine::Context::capture(&self.st, &self.devices);
        // Only writes when rules, devices or state changed the rendered text.
        engine::apply(&self.st, &self.devices, &ctx);
        self.sync_ui();
        true
    }

    fn sync_ui(&mut self) {
        let ctx = engine::Context::capture(&self.st, &self.devices);
        let (eff, active) = engine::effective(&self.st, &ctx);
        let t = self.target();
        let ch = eff.chain(&t);
        let pname = ch
            .profile
            .and_then(|id| eff.profile(id))
            .map(|p| p.name.clone())
            .unwrap_or_else(|| "Flat".into());
        let mut tip = if eff.bypass {
            "equinox: bypassed".to_string()
        } else {
            format!("equinox: {pname}")
        };
        if let Some(c) = &ch.correction {
            tip += &format!(" + {}", c.name);
        }
        if !active.is_empty() {
            tip += " ⚡";
        }
        let _ = self.tray.set_tooltip(Some(tip));
        if self.last_bypass != Some(eff.bypass) {
            let _ = self.tray.set_icon(Some(icon(eff.bypass)));
            self.last_bypass = Some(eff.bypass);
        }
        self.enabled.set_checked(!self.st.bypass);
        self.startup.set_checked(sys::autostart_enabled());

        let sig: Vec<(u64, String)> = self.st.profiles.iter().map(|p| (p.id, p.name.clone())).collect();
        if sig != self.profile_sig {
            for (_, _, item) in self.profile_items.drain(..) {
                let _ = self.profiles.remove(&item);
            }
            for (id, name) in &sig {
                let item = CheckMenuItem::new(name, true, false, None);
                let _ = self.profiles.append(&item);
                self.profile_items.push((item.id().clone(), *id, item));
            }
            self.profile_sig = sig;
        }
        let current = self.st.chain(&self.target()).profile;
        for (_, id, item) in &self.profile_items {
            item.set_checked(Some(*id) == current);
        }

        if self.st.settings.hotkeys != self.hotkeys {
            if self.st.settings.hotkeys {
                self.register_hotkeys();
            } else {
                self.unregister_hotkeys();
            }
        }
    }

    /// Returns false on Quit.
    fn menu(&mut self, id: MenuId) -> bool {
        if id == *self.open.id() {
            sys::open_app();
        } else if id == *self.enabled.id() {
            self.st.bypass = !self.enabled.is_checked();
            self.save_apply();
        } else if id == *self.startup.id() {
            let _ = sys::set_autostart(self.startup.is_checked());
            self.sync_ui();
        } else if id == *self.quit.id() {
            return false;
        } else if let Some((_, pid, _)) = self.profile_items.iter().find(|(mid, _, _)| *mid == id) {
            let (pid, t) = (*pid, self.target());
            self.st.chain_mut(&t).profile = Some(pid);
            self.st.bypass = false;
            self.save_apply();
        }
        true
    }

    fn hotkey(&mut self, id: i32) {
        match id {
            HK_TOGGLE => self.st.bypass = !self.st.bypass,
            HK_NEXT | HK_PREV if !self.st.profiles.is_empty() => {
                let t = self.target();
                let n = self.st.profiles.len();
                let cur = self
                    .st
                    .chain(&t)
                    .profile
                    .and_then(|pid| self.st.profiles.iter().position(|p| p.id == pid))
                    .unwrap_or(0);
                let next = if id == HK_NEXT { (cur + 1) % n } else { (cur + n - 1) % n };
                let pid = self.st.profiles[next].id;
                self.st.chain_mut(&t).profile = Some(pid);
                self.st.bypass = false;
            }
            _ => return,
        }
        self.save_apply();
    }

    fn register_hotkeys(&mut self) {
        let mods = HOT_KEY_MODIFIERS(MOD_CONTROL.0 | MOD_ALT.0 | MOD_NOREPEAT.0);
        unsafe {
            // 'E', VK_OEM_PERIOD, VK_OEM_COMMA. Failure means another app owns the combo.
            let _ = RegisterHotKey(None, HK_TOGGLE, mods, 0x45);
            let _ = RegisterHotKey(None, HK_NEXT, mods, 0xBE);
            let _ = RegisterHotKey(None, HK_PREV, mods, 0xBC);
        }
        self.hotkeys = true;
    }

    fn unregister_hotkeys(&mut self) {
        unsafe {
            for id in [HK_TOGGLE, HK_NEXT, HK_PREV] {
                let _ = UnregisterHotKey(None, id);
            }
        }
        self.hotkeys = false;
    }
}
