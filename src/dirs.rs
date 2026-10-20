//! Where things are, the XDG way: the home, the cache, the state, the
//! Music folder (as `xdg-user-dirs` names it); and the local time, for
//! names stamped with it.

use std::path::PathBuf;

pub fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/"))
}

fn xdg(var: &str, fallback: &[&str]) -> PathBuf {
    std::env::var_os(var)
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| fallback.iter().fold(home(), |p, part| p.join(part)))
}

pub fn cache() -> PathBuf {
    xdg("XDG_CACHE_HOME", &[".cache"])
}

pub fn state() -> PathBuf {
    xdg("XDG_STATE_HOME", &[".local", "state"])
}

pub fn config() -> PathBuf {
    xdg("XDG_CONFIG_HOME", &[".config"])
}

pub fn runtime() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(std::env::temp_dir)
}

/// The Music folder: `XDG_MUSIC_DIR` in `user-dirs.dirs`, else `~/Music`.
pub fn music() -> PathBuf {
    let dirs = std::fs::read_to_string(config().join("user-dirs.dirs")).unwrap_or_default();
    for line in dirs.lines() {
        let Some(value) = line.trim().strip_prefix("XDG_MUSIC_DIR=") else { continue };
        let value = value.trim().trim_matches('"');
        let path = match value.strip_prefix("$HOME") {
            Some(rest) => home().join(rest.trim_start_matches('/')),
            None => PathBuf::from(value),
        };
        // the home itself means none, as xdg-user-dirs has it
        if path.is_absolute() && path != home() {
            return path;
        }
    }
    home().join("Music")
}

/// The local time in strftime's `format`.
pub fn now(format: &str) -> String {
    let format = std::ffi::CString::new(format).unwrap_or_default();
    let mut buf = [0u8; 64];
    unsafe {
        let t = libc::time(std::ptr::null_mut());
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&t, &mut tm);
        let n = libc::strftime(buf.as_mut_ptr().cast(), buf.len(), format.as_ptr(), &tm);
        String::from_utf8_lossy(&buf[..n]).into_owned()
    }
}
