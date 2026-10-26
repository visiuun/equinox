//! Equalizer APO is the system-wide audio hook: it runs inside the Windows
//! audio engine and re-reads its config folder on every change. equinox owns
//! one file there (`equinox.txt`) and points `config.txt` at it.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use winreg::RegKey;
use winreg::enums::{HKEY_LOCAL_MACHINE, KEY_READ};

pub const OUR_FILE: &str = "equinox.txt";
const BACKUP: &str = "config.before-equinox.txt";
const INCLUDE: &str = "Include: equinox.txt";

#[derive(Clone, Debug)]
pub struct Apo {
    pub install: PathBuf,
    pub config: PathBuf,
    pub version: String,
}

pub fn detect() -> Option<Apo> {
    let key = RegKey::predef(HKEY_LOCAL_MACHINE)
        .open_subkey_with_flags(r"SOFTWARE\EqualizerAPO", KEY_READ)
        .ok()?;
    let install: String = key.get_value("InstallPath").ok()?;
    let config: String = key.get_value("ConfigPath").unwrap_or_else(|_| format!(r"{install}\config"));
    let version: String = key
        .get_value::<String, _>("Start Menu Folder")
        .ok()
        .and_then(|s| s.rsplit(' ').next().map(str::to_string))
        .unwrap_or_default();
    let apo = Apo {
        install: install.into(),
        config: config.into(),
        version,
    };
    apo.config.is_dir().then_some(apo)
}

/// Endpoint GUIDs (lowercase) that Equalizer APO is installed on.
pub fn installed_endpoints() -> HashSet<String> {
    RegKey::predef(HKEY_LOCAL_MACHINE)
        .open_subkey_with_flags(r"SOFTWARE\EqualizerAPO\Child APOs", KEY_READ)
        .map(|k| k.enum_keys().flatten().map(|s| s.to_ascii_lowercase()).collect())
        .unwrap_or_default()
}

impl Apo {
    pub fn main_config(&self) -> PathBuf {
        self.config.join("config.txt")
    }

    pub fn our_file(&self) -> PathBuf {
        self.config.join(OUR_FILE)
    }

    /// Equalizer APO's own installer for picking devices (needs admin).
    pub fn device_selector(&self) -> PathBuf {
        let ds = self.install.join("DeviceSelector.exe");
        if ds.exists() { ds } else { self.install.join("Configurator.exe") }
    }

    /// config.txt includes our file on an active line.
    pub fn hooked(&self) -> bool {
        std::fs::read_to_string(self.main_config()).is_ok_and(|t| active_lines(&t).any(|l| l.eq_ignore_ascii_case(INCLUDE)))
    }

    /// Other active lines in config.txt (another EQ app would stack with ours).
    pub fn foreign_lines(&self) -> Vec<String> {
        std::fs::read_to_string(self.main_config())
            .map(|t| {
                active_lines(&t)
                    .filter(|l| !l.eq_ignore_ascii_case(INCLUDE))
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Make config.txt load only equinox, keeping a backup of what was there.
    pub fn take_control(&self) -> std::io::Result<()> {
        let main = self.main_config();
        let backup = self.config.join(BACKUP);
        if main.exists() && !backup.exists() {
            std::fs::copy(&main, &backup)?;
        }
        if !self.our_file().exists() {
            std::fs::write(self.our_file(), "")?;
        }
        std::fs::write(
            &main,
            format!("# Managed by equinox. Your previous config is saved as {BACKUP};\n# `eqx apo release` puts it back.\n{INCLUDE}\n"),
        )
    }

    /// Restore the config.txt from before equinox took over.
    pub fn release(&self) -> std::io::Result<()> {
        let main = self.main_config();
        let backup = self.config.join(BACKUP);
        if backup.exists() {
            std::fs::copy(&backup, &main)?;
            std::fs::remove_file(&backup)?;
        } else {
            let text = std::fs::read_to_string(&main).unwrap_or_default();
            let kept: Vec<&str> = text.lines().filter(|l| !l.trim().eq_ignore_ascii_case(INCLUDE)).collect();
            std::fs::write(&main, kept.join("\n") + "\n")?;
        }
        Ok(())
    }

    /// Write our config if it changed. Returns whether a write happened.
    pub fn write(&self, text: &str) -> std::io::Result<bool> {
        write_if_changed(&self.our_file(), text)
    }
}

fn active_lines(text: &str) -> impl Iterator<Item = &str> {
    text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#'))
}

fn write_if_changed(path: &Path, text: &str) -> std::io::Result<bool> {
    if std::fs::read_to_string(path).is_ok_and(|old| old == text) {
        return Ok(false);
    }
    std::fs::write(path, text)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn take_control_and_release_round_trip() {
        let dir = std::env::temp_dir().join(format!("equinox-apo-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let original = "Preamp: -3 dB\nInclude: peace.txt\n";
        std::fs::write(dir.join("config.txt"), original).unwrap();
        let a = Apo {
            install: dir.clone(),
            config: dir.clone(),
            version: "test".into(),
        };

        assert!(!a.hooked());
        assert_eq!(a.foreign_lines(), vec!["Preamp: -3 dB", "Include: peace.txt"]);
        a.take_control().unwrap();
        assert!(a.hooked());
        assert!(a.foreign_lines().is_empty());
        assert!(a.write("Filter: ON PK Fc 100 Hz Gain 1 dB Q 1\n").unwrap());
        assert!(
            !a.write("Filter: ON PK Fc 100 Hz Gain 1 dB Q 1\n").unwrap(),
            "unchanged text is not rewritten"
        );
        // A second take-over must not overwrite the original backup.
        a.take_control().unwrap();
        a.release().unwrap();
        assert_eq!(std::fs::read_to_string(dir.join("config.txt")).unwrap(), original);
        assert!(!a.hooked());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
