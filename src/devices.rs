//! Output devices, read straight from the MMDevices registry (fast, no COM,
//! and it includes unplugged endpoints so per-device settings survive replugs).

use winreg::RegKey;
use winreg::enums::{HKEY_LOCAL_MACHINE, KEY_READ};

const RENDER: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\MMDevices\Audio\Render";
const P_NAME: &str = "{a45c254e-df1c-4efd-8020-67d146a850e0},2";
const P_IFACE: &str = "{b3f8fa53-0004-438e-9003-51a46e139bfc},6";
const P_FORM: &str = "{1da5d803-d492-4edd-8c23-e0c0ffee7f0e},0";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Form {
    Speakers,
    Headphones,
    Headset,
    Digital,
    Display,
    Other,
}

impl Form {
    fn from_reg(v: u32) -> Form {
        match v {
            1 => Form::Speakers,
            3 => Form::Headphones,
            5 | 6 => Form::Headset,
            7 | 8 => Form::Digital,
            9 => Form::Display,
            _ => Form::Other,
        }
    }

    pub fn glyph(self) -> &'static str {
        match self {
            Form::Headphones | Form::Headset => "🎧",
            Form::Speakers => "🔊",
            Form::Display => "🖵",
            _ => "♪",
        }
    }

    /// Worth looking up a headphone measurement for.
    pub fn is_headphone(self) -> bool {
        matches!(self, Form::Headphones | Form::Headset)
    }
}

#[derive(Clone, Debug)]
pub struct AudioDevice {
    /// Endpoint GUID, lowercase with braces.
    pub id: String,
    /// "Headphones", "Speakers"...
    pub name: String,
    /// Adapter or product: "Realtek(R) Audio", "WH-1000XM4".
    pub interface: String,
    pub active: bool,
    pub form: Form,
    /// Equalizer APO is installed on this endpoint.
    pub apo: bool,
    pub default: bool,
}

impl AudioDevice {
    pub fn label(&self) -> String {
        if self.interface.is_empty() {
            self.name.clone()
        } else {
            format!("{} ({})", self.name, self.interface)
        }
    }

    /// Short label for chips: the product name when it's more telling than "Headphones".
    pub fn short(&self) -> String {
        let generic = [
            "headphones",
            "speakers",
            "headset",
            "headset earphone",
            "digital output",
            "realtek digital output",
        ];
        if !self.interface.is_empty() && generic.contains(&self.name.to_lowercase().as_str()) {
            let iface = self.interface.replace("(R)", "").replace("High Definition Audio", "HD Audio");
            format!("{} · {}", self.name, iface.trim())
        } else {
            self.name.clone()
        }
    }
}

/// Active and unplugged outputs (disabled and removed ones are skipped),
/// default first, then active, then by name.
pub fn list() -> Vec<AudioDevice> {
    let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
    let Ok(render) = hklm.open_subkey_with_flags(RENDER, KEY_READ) else {
        return Vec::new();
    };
    let apo = crate::apo::installed_endpoints();
    let default = crate::sys::default_output();
    let mut out = Vec::new();
    for guid in render.enum_keys().flatten() {
        let Ok(key) = render.open_subkey_with_flags(&guid, KEY_READ) else {
            continue;
        };
        let state: u32 = key.get_value("DeviceState").unwrap_or(0);
        let active = match state & 0xF {
            1 => true,
            8 => false,
            _ => continue,
        };
        let Ok(props) = key.open_subkey_with_flags("Properties", KEY_READ) else {
            continue;
        };
        let name: String = props.get_value(P_NAME).unwrap_or_default();
        if name.is_empty() {
            continue;
        }
        let id = guid.to_ascii_lowercase();
        out.push(AudioDevice {
            interface: props.get_value(P_IFACE).unwrap_or_default(),
            form: Form::from_reg(props.get_value(P_FORM).unwrap_or(10)),
            apo: apo.contains(&id) || fx_has_apo(&key),
            default: default.as_deref() == Some(id.as_str()),
            active,
            name,
            id,
        });
    }
    out.sort_by(|a, b| {
        b.default
            .cmp(&a.default)
            .then(b.active.cmp(&a.active))
            .then(a.label().cmp(&b.label()))
    });
    out
}

/// The endpoint's effect chain references Equalizer APO's pre- or post-mix class.
fn fx_has_apo(endpoint: &RegKey) -> bool {
    const CLSIDS: [&str; 2] = ["{eacd2258-fcac-4ff4-b36d-419e924a6d79}", "{ec1cc9ce-faed-4822-828a-82a81a6f018f}"];
    let Ok(fx) = endpoint.open_subkey_with_flags("FxProperties", KEY_READ) else {
        return false;
    };
    fx.enum_values().flatten().any(|(_, v)| {
        let s = v.to_string().to_ascii_lowercase();
        CLSIDS.iter().any(|c| s.contains(c))
    })
}

/// Resolve user input to a device: "default", a GUID fragment or part of the name.
pub fn find<'a>(devices: &'a [AudioDevice], query: &str) -> Option<&'a AudioDevice> {
    let q = query.trim().to_lowercase();
    if q == "default" {
        return devices.iter().find(|d| d.default);
    }
    if let Some(d) = devices
        .iter()
        .find(|d| d.id.trim_matches(['{', '}']).starts_with(q.trim_matches(['{', '}'])) && q.len() >= 4)
    {
        return Some(d);
    }
    let hits: Vec<_> = devices.iter().filter(|d| d.label().to_lowercase().contains(&q)).collect();
    // Prefer active endpoints when the same name exists unplugged too.
    hits.iter().find(|d| d.active).or(hits.first()).copied()
}
