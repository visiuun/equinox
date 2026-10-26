//! Everything equinox persists, in one serde tree (`state.json`).
//!
//! Profiles are a shared library of sounds. A chain is what one output gets:
//! a profile, an optional headphone correction, preamp and balance. The global
//! chain covers every device; in per-device mode each device can have its own.

use std::path::Path;

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum FilterKind {
    Peak,
    LowShelf,
    HighShelf,
    LowPass,
    HighPass,
    Notch,
    BandPass,
}

impl FilterKind {
    pub const ALL: [FilterKind; 7] = [
        FilterKind::Peak,
        FilterKind::LowShelf,
        FilterKind::HighShelf,
        FilterKind::LowPass,
        FilterKind::HighPass,
        FilterKind::Notch,
        FilterKind::BandPass,
    ];

    pub fn label(self) -> &'static str {
        match self {
            FilterKind::Peak => "Peak",
            FilterKind::LowShelf => "Low shelf",
            FilterKind::HighShelf => "High shelf",
            FilterKind::LowPass => "Low pass",
            FilterKind::HighPass => "High pass",
            FilterKind::Notch => "Notch",
            FilterKind::BandPass => "Band pass",
        }
    }

    /// Equalizer APO filter code.
    pub fn code(self) -> &'static str {
        match self {
            FilterKind::Peak => "PK",
            FilterKind::LowShelf => "LSC",
            FilterKind::HighShelf => "HSC",
            FilterKind::LowPass => "LPQ",
            FilterKind::HighPass => "HPQ",
            FilterKind::Notch => "NO",
            FilterKind::BandPass => "BP",
        }
    }

    pub fn from_name(s: &str) -> Option<FilterKind> {
        Some(match s.to_ascii_lowercase().as_str() {
            "pk" | "peq" | "peak" | "peaking" | "bell" => FilterKind::Peak,
            "ls" | "lsc" | "lowshelf" | "low-shelf" | "low_shelf" => FilterKind::LowShelf,
            "hs" | "hsc" | "highshelf" | "high-shelf" | "high_shelf" => FilterKind::HighShelf,
            "lp" | "lpq" | "lowpass" | "low-pass" | "lpf" => FilterKind::LowPass,
            "hp" | "hpq" | "highpass" | "high-pass" | "hpf" => FilterKind::HighPass,
            "no" | "notch" => FilterKind::Notch,
            "bp" | "bandpass" | "band-pass" => FilterKind::BandPass,
            _ => return None,
        })
    }

    pub fn has_gain(self) -> bool {
        matches!(self, FilterKind::Peak | FilterKind::LowShelf | FilterKind::HighShelf)
    }
}

fn yes() -> bool {
    true
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Band {
    pub kind: FilterKind,
    pub freq: f32,
    #[serde(default)]
    pub gain: f32,
    pub q: f32,
    #[serde(default = "yes")]
    pub enabled: bool,
}

impl Band {
    pub fn new(kind: FilterKind, freq: f32, gain: f32, q: f32) -> Band {
        Band {
            kind,
            freq,
            gain,
            q,
            enabled: true,
        }
    }

    pub fn clamp(&mut self) {
        self.freq = self.freq.clamp(10.0, 22_000.0);
        self.gain = self.gain.clamp(-30.0, 30.0);
        self.q = self.q.clamp(0.1, 30.0);
    }
}

/// The four "simple mode" macros. Each is a fixed filter whose gain is the slider.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Tone {
    #[serde(default)]
    pub bass: f32,
    #[serde(default)]
    pub warmth: f32,
    #[serde(default)]
    pub clarity: f32,
    #[serde(default)]
    pub air: f32,
}

impl Tone {
    pub const NAMES: [&'static str; 4] = ["Bass", "Warmth", "Clarity", "Air"];
    pub const HINTS: [&'static str; 4] = ["Low shelf · 105 Hz", "Peak · 250 Hz", "Peak · 2.8 kHz", "High shelf · 10 kHz"];

    pub fn get(&self, i: usize) -> f32 {
        [self.bass, self.warmth, self.clarity, self.air][i]
    }

    pub fn get_mut(&mut self, i: usize) -> &mut f32 {
        match i {
            0 => &mut self.bass,
            1 => &mut self.warmth,
            2 => &mut self.clarity,
            _ => &mut self.air,
        }
    }

    pub fn filters(&self) -> Vec<Band> {
        let defs = [
            (FilterKind::LowShelf, 105.0, self.bass, 0.7),
            (FilterKind::Peak, 250.0, self.warmth, 0.8),
            (FilterKind::Peak, 2800.0, self.clarity, 0.9),
            (FilterKind::HighShelf, 10_000.0, self.air, 0.7),
        ];
        defs.into_iter()
            .filter(|d| d.2.abs() >= 0.05)
            .map(|(k, f, g, q)| Band::new(k, f, g, q))
            .collect()
    }

    pub fn is_flat(&self) -> bool {
        (0..4).all(|i| self.get(i).abs() < 0.05)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Profile {
    pub id: u64,
    pub name: String,
    #[serde(default)]
    pub tone: Tone,
    #[serde(default)]
    pub bands: Vec<Band>,
}

impl Profile {
    /// Tone macros plus enabled bands, in processing order.
    pub fn filters(&self) -> Vec<Band> {
        let mut f = self.tone.filters();
        f.extend(self.bands.iter().filter(|b| b.enabled).cloned());
        f
    }
}

/// A measured headphone correction (AutoEq or imported). Applied before the profile.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Correction {
    pub name: String,
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub filters: Vec<Band>,
    /// GraphicEQ points (Hz, dB) for corrections that come as a curve.
    #[serde(default)]
    pub graphic: Vec<(f32, f32)>,
}

impl Correction {
    /// The correction blended toward flat: 1.0 is the full measurement-based fix.
    pub fn scaled(&self, strength: f32) -> (Vec<Band>, Vec<(f32, f32)>) {
        let s = strength.clamp(0.0, 1.5);
        let filters = self
            .filters
            .iter()
            .filter(|b| b.enabled)
            .map(|b| {
                let mut b = b.clone();
                if b.kind.has_gain() {
                    b.gain *= s;
                }
                b
            })
            .filter(|b| !b.kind.has_gain() || b.gain.abs() >= 0.01)
            .collect();
        let graphic = if s <= 0.0 {
            Vec::new()
        } else {
            self.graphic.iter().map(|&(f, g)| (f, g * s)).collect()
        };
        (filters, graphic)
    }
}

fn one() -> f32 {
    1.0
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Chain {
    pub profile: Option<u64>,
    #[serde(default)]
    pub correction: Option<Correction>,
    #[serde(default = "one")]
    pub strength: f32,
    #[serde(default = "yes")]
    pub auto_preamp: bool,
    /// Used when `auto_preamp` is off.
    #[serde(default)]
    pub preamp: f32,
    /// dB; positive pulls the image right by attenuating the left channel.
    #[serde(default)]
    pub balance: f32,
}

impl Default for Chain {
    fn default() -> Self {
        Chain {
            profile: None,
            correction: None,
            strength: 1.0,
            auto_preamp: true,
            preamp: 0.0,
            balance: 0.0,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DeviceEntry {
    /// Endpoint GUID, lowercase with braces.
    pub id: String,
    /// Last seen display name, so rules and the CLI read well when unplugged.
    pub name: String,
    /// When true the device uses the global chain.
    #[serde(default = "yes")]
    pub follow_global: bool,
    #[serde(default)]
    pub chain: Chain,
    /// AutoEq suggestion the user dismissed, so it isn't offered again.
    #[serde(default)]
    pub dismissed_suggestion: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Mode {
    #[default]
    Global,
    PerDevice,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Target {
    Global,
    Device(String),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Trigger {
    /// The device is the Windows default output.
    DefaultDevice { device: String },
    /// Local time window, "HH:MM"; wraps past midnight when end < start.
    TimeRange { start: String, end: String },
    /// A process with this exe name is running (case-insensitive).
    AppRunning { exe: String },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Action {
    UseProfile { target: Target, profile: u64 },
    Bypass { on: bool },
    Strength { target: Target, value: f32 },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Rule {
    pub id: u64,
    pub name: String,
    #[serde(default = "yes")]
    pub enabled: bool,
    pub trigger: Trigger,
    pub action: Action,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Keep the tray agent running (needed for rules and hotkeys).
    pub agent: bool,
    pub hotkeys: bool,
    /// Bypass keeps the preamp, so A/B compares EQ rather than loudness.
    pub level_matched_bypass: bool,
    pub analyzer: bool,
    pub band_curves: bool,
    /// Extra safety margin on top of the auto preamp, dB.
    pub headroom: f32,
    pub accent: [u8; 3],
    pub advanced: bool,
    /// First-launch setup (start with Windows) has run.
    pub setup_done: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            agent: true,
            hotkeys: true,
            level_matched_bypass: true,
            analyzer: true,
            band_curves: true,
            headroom: 0.0,
            accent: [125, 249, 255],
            advanced: false,
            setup_done: false,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct State {
    #[serde(default)]
    pub mode: Mode,
    #[serde(default)]
    pub bypass: bool,
    #[serde(default)]
    pub global: Chain,
    #[serde(default)]
    pub devices: Vec<DeviceEntry>,
    #[serde(default)]
    pub profiles: Vec<Profile>,
    #[serde(default)]
    pub rules: Vec<Rule>,
    #[serde(default)]
    pub settings: Settings,
    #[serde(default)]
    pub next_id: u64,
}

impl Default for State {
    fn default() -> Self {
        let mut s = State {
            mode: Mode::Global,
            bypass: false,
            global: Chain::default(),
            devices: Vec::new(),
            profiles: Vec::new(),
            rules: Vec::new(),
            settings: Settings::default(),
            next_id: 1,
        };
        for (name, tone, bands) in crate::presets::builtin() {
            let id = s.new_id();
            s.profiles.push(Profile {
                id,
                name: name.to_string(),
                tone,
                bands,
            });
        }
        s.global.profile = s.profiles.first().map(|p| p.id);
        s
    }
}

impl State {
    pub fn new_id(&mut self) -> u64 {
        let used = self
            .profiles
            .iter()
            .map(|p| p.id)
            .chain(self.rules.iter().map(|r| r.id))
            .max()
            .unwrap_or(0);
        self.next_id = self.next_id.max(used + 1);
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    pub fn profile(&self, id: u64) -> Option<&Profile> {
        self.profiles.iter().find(|p| p.id == id)
    }

    pub fn profile_mut(&mut self, id: u64) -> Option<&mut Profile> {
        self.profiles.iter_mut().find(|p| p.id == id)
    }

    pub fn device(&self, id: &str) -> Option<&DeviceEntry> {
        self.devices.iter().find(|d| d.id.eq_ignore_ascii_case(id))
    }

    /// The device entry, created (following global) on first touch.
    pub fn device_mut(&mut self, id: &str, name: &str) -> &mut DeviceEntry {
        if let Some(i) = self.devices.iter().position(|d| d.id.eq_ignore_ascii_case(id)) {
            if !name.is_empty() {
                self.devices[i].name = name.to_string();
            }
            return &mut self.devices[i];
        }
        self.devices.push(DeviceEntry {
            id: id.to_ascii_lowercase(),
            name: name.to_string(),
            follow_global: true,
            chain: self.global.clone(),
            dismissed_suggestion: None,
        });
        self.devices.last_mut().unwrap()
    }

    /// The chain actually used for a target (devices may follow global).
    pub fn chain(&self, target: &Target) -> &Chain {
        match target {
            Target::Device(id) if self.mode == Mode::PerDevice => match self.device(id) {
                Some(d) if !d.follow_global => &d.chain,
                _ => &self.global,
            },
            _ => &self.global,
        }
    }

    /// Whether edits to `target` land on its own chain or on the global one.
    pub fn owns_chain(&self, target: &Target) -> bool {
        match target {
            Target::Global => true,
            Target::Device(id) => self.mode == Mode::PerDevice && self.device(id).is_some_and(|d| !d.follow_global),
        }
    }

    /// Mutable chain for a target; device targets that follow global edit global.
    pub fn chain_mut(&mut self, target: &Target) -> &mut Chain {
        if let Target::Device(id) = target
            && self.owns_chain(target)
            && let Some(i) = self.devices.iter().position(|d| d.id.eq_ignore_ascii_case(id))
        {
            return &mut self.devices[i].chain;
        }
        &mut self.global
    }

    /// Give a device its own chain, starting from a copy of global.
    pub fn detach_device(&mut self, id: &str, name: &str) {
        self.mode = Mode::PerDevice;
        let global = self.global.clone();
        let d = self.device_mut(id, name);
        if d.follow_global {
            d.follow_global = false;
            d.chain = global;
        }
    }

    /// The profile a target's chain points at, creating "Custom" if it has none.
    pub fn ensure_profile(&mut self, target: &Target) -> u64 {
        if let Some(id) = self.chain(target).profile
            && self.profile(id).is_some()
        {
            return id;
        }
        let id = self.new_id();
        let name = self.unique_name("Custom");
        self.profiles.push(Profile {
            id,
            name,
            tone: Tone::default(),
            bands: Vec::new(),
        });
        self.chain_mut(target).profile = Some(id);
        id
    }

    pub fn unique_name(&self, base: &str) -> String {
        if !self.profiles.iter().any(|p| p.name.eq_ignore_ascii_case(base)) {
            return base.to_string();
        }
        (2..)
            .map(|n| format!("{base} {n}"))
            .find(|n| !self.profiles.iter().any(|p| p.name.eq_ignore_ascii_case(n)))
            .unwrap()
    }

    pub fn delete_profile(&mut self, id: u64) {
        self.profiles.retain(|p| p.id != id);
        let clear = |c: &mut Chain| {
            if c.profile == Some(id) {
                c.profile = None;
            }
        };
        clear(&mut self.global);
        for d in &mut self.devices {
            clear(&mut d.chain);
        }
        self.rules
            .retain(|r| !matches!(&r.action, Action::UseProfile { profile, .. } if *profile == id));
    }

    /// Case-insensitive lookup by id, exact name, then unique prefix/substring.
    pub fn find_profile(&self, query: &str) -> Option<u64> {
        let q = query.trim().to_lowercase();
        if let Ok(id) = q.parse::<u64>()
            && self.profile(id).is_some()
        {
            return Some(id);
        }
        if let Some(p) = self.profiles.iter().find(|p| p.name.to_lowercase() == q) {
            return Some(p.id);
        }
        for pred in [
            &(|n: &str| n.starts_with(&q)) as &dyn Fn(&str) -> bool,
            &(|n: &str| n.contains(&q)) as &dyn Fn(&str) -> bool,
        ] {
            let hits: Vec<_> = self.profiles.iter().filter(|p| pred(&p.name.to_lowercase())).collect();
            if hits.len() == 1 {
                return Some(hits[0].id);
            }
        }
        None
    }

    pub fn load() -> State {
        let path = crate::paths::state_file();
        match std::fs::read_to_string(&path) {
            Ok(text) => match serde_json::from_str::<State>(&text) {
                Ok(s) => s,
                Err(_) => {
                    // Keep the unreadable file for the user instead of silently losing it.
                    let _ = std::fs::rename(&path, path.with_extension("json.broken"));
                    State::default()
                }
            },
            Err(_) => State::default(),
        }
    }

    pub fn save(&self) -> std::io::Result<()> {
        let path = crate::paths::state_file();
        write_atomic(&path, serde_json::to_string_pretty(self).unwrap_or_default().as_bytes())
    }
}

/// Write via a temp file and rename, so readers never see half a file.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)
}
