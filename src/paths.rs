use std::path::PathBuf;

/// `%APPDATA%\equinox`, or `EQUINOX_HOME` when set (handy for testing).
pub fn dir() -> PathBuf {
    if let Some(p) = std::env::var_os("EQUINOX_HOME") {
        return PathBuf::from(p);
    }
    let base = std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    base.join("equinox")
}

pub fn state_file() -> PathBuf {
    dir().join("state.json")
}

pub fn cache_dir() -> PathBuf {
    dir().join("cache")
}

/// The GUI exe, also when called from `eqx.exe` sitting next to it.
pub fn app_exe() -> PathBuf {
    let me = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("equinox.exe"));
    match me.file_stem().and_then(|s| s.to_str()) {
        Some("equinox") => me,
        _ => me.with_file_name("equinox.exe"),
    }
}
