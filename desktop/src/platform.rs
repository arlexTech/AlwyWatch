// SPDX-License-Identifier: AGPL-3.0-or-later
//! Platform paths and token storage.
//!
//! Tokens: Windows encrypts them with DPAPI (bound to the Windows user). Linux stores them in a
//! file readable only by the user (0600) in the data directory, like other media clients do;
//! a Secret Service integration would pull a whole D-Bus stack for one value.

use std::collections::BTreeMap;
use std::path::PathBuf;

pub struct Paths {
    pub data: PathBuf,
    pub cache: PathBuf,
    /// Downloaded media: in the user's Videos folder so they are easy to find and manage.
    pub downloads: PathBuf,
}

pub fn paths() -> Paths {
    #[cfg(windows)]
    {
        let env = |k: &str| std::env::var_os(k).map(PathBuf::from);
        let roaming = env("APPDATA").unwrap_or_else(|| PathBuf::from("."));
        let local = env("LOCALAPPDATA").unwrap_or_else(|| roaming.clone());
        let videos = env("USERPROFILE")
            .map(|p| p.join("Videos"))
            .unwrap_or_else(|| local.clone());
        Paths {
            data: roaming.join("AlwyWatch"),
            cache: local.join("AlwyWatch").join("cache"),
            downloads: videos.join("AlwyWatch"),
        }
    }
    #[cfg(not(windows))]
    {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));
        let xdg = |k: &str, fallback: &str| {
            std::env::var_os(k)
                .map(PathBuf::from)
                .filter(|p| p.is_absolute())
                .unwrap_or_else(|| home.join(fallback))
        };
        // XDG_VIDEOS_DIR lives in user-dirs.dirs; the plain default is good enough here.
        let videos = std::env::var_os("XDG_VIDEOS_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join("Videos"));
        Paths {
            data: xdg("XDG_DATA_HOME", ".local/share").join("alwywatch"),
            cache: xdg("XDG_CACHE_HOME", ".cache").join("alwywatch"),
            downloads: videos.join("AlwyWatch"),
        }
    }
}

pub struct Tokens {
    file: PathBuf,
}

impl Tokens {
    pub fn new(data_dir: &std::path::Path) -> Self {
        Self {
            file: data_dir.join("tokens"),
        }
    }

    fn load(&self) -> BTreeMap<String, String> {
        std::fs::read_to_string(&self.file)
            .ok()
            .map(|s| {
                s.lines()
                    .filter_map(|l| l.split_once('\t'))
                    .map(|(k, v)| (k.to_owned(), v.to_owned()))
                    .collect()
            })
            .unwrap_or_default()
    }

    fn save(&self, map: &BTreeMap<String, String>) -> std::io::Result<()> {
        if let Some(dir) = self.file.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let body: String = map.iter().map(|(k, v)| format!("{k}\t{v}\n")).collect();
        let tmp = self.file.with_extension("tmp");
        write_private(&tmp, body.as_bytes())?;
        std::fs::rename(tmp, &self.file)
    }

    pub fn get(&self, account: &str) -> Option<String> {
        self.load().get(account).and_then(|v| unprotect(v))
    }

    pub fn put(&self, account: &str, token: &str) -> std::io::Result<()> {
        let mut map = self.load();
        map.insert(account.to_owned(), protect(token));
        self.save(&map)
    }

    pub fn remove(&self, account: &str) -> std::io::Result<()> {
        let mut map = self.load();
        map.remove(account);
        self.save(&map)
    }
}

#[cfg(unix)]
fn write_private(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    f.write_all(bytes)
}

#[cfg(not(unix))]
fn write_private(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    std::fs::write(path, bytes)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex(s: &str) -> Option<Vec<u8>> {
    (0..s.len())
        .step_by(2)
        .map(|i| s.get(i..i + 2).and_then(|h| u8::from_str_radix(h, 16).ok()))
        .collect()
}

#[cfg(not(windows))]
fn protect(token: &str) -> String {
    hex(token.as_bytes())
}

#[cfg(not(windows))]
fn unprotect(stored: &str) -> Option<String> {
    String::from_utf8(unhex(stored)?).ok()
}

#[cfg(windows)]
fn protect(token: &str) -> String {
    dpapi(token.as_bytes(), true)
        .map(|b| hex(&b))
        .unwrap_or_default()
}

#[cfg(windows)]
fn unprotect(stored: &str) -> Option<String> {
    String::from_utf8(dpapi(&unhex(stored)?, false)?).ok()
}

#[cfg(windows)]
fn dpapi(input: &[u8], encrypt: bool) -> Option<Vec<u8>> {
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Cryptography::{
        CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData, CryptUnprotectData,
    };
    let in_blob = CRYPT_INTEGER_BLOB {
        cbData: input.len() as u32,
        pbData: input.as_ptr() as *mut u8,
    };
    let mut out = CRYPT_INTEGER_BLOB {
        cbData: 0,
        pbData: std::ptr::null_mut(),
    };
    // SAFETY: blobs point to valid memory; DPAPI allocates `out` with LocalAlloc.
    let ok = unsafe {
        if encrypt {
            CryptProtectData(
                &in_blob,
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut out,
            )
        } else {
            CryptUnprotectData(
                &in_blob,
                std::ptr::null_mut(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut out,
            )
        }
    };
    if ok == 0 || out.pbData.is_null() {
        return None;
    }
    // SAFETY: DPAPI returned `cbData` bytes at `pbData`, freed right after copying.
    let v = unsafe { std::slice::from_raw_parts(out.pbData, out.cbData as usize) }.to_vec();
    unsafe { LocalFree(out.pbData.cast()) };
    Some(v)
}

/// Opens a URL in the default browser (Plex sign-in).
pub fn open_url(url: &str) {
    #[cfg(windows)]
    let r = std::process::Command::new("rundll32")
        .args(["url.dll,FileProtocolHandler", url])
        .spawn();
    #[cfg(not(windows))]
    let r = std::process::Command::new("xdg-open").arg(url).spawn();
    if let Err(e) = r {
        eprintln!("could not open browser: {e}");
    }
}

/// The desktop's language ("fr" from LANG=fr_BE.UTF-8), for subtitles in "our" language;
/// empty when unknown (Windows without LANG: only explicit settings apply).
pub fn system_language() -> String {
    ["LC_ALL", "LC_MESSAGES", "LANG"]
        .iter()
        .filter_map(|k| std::env::var(k).ok())
        .find(|v| !v.is_empty() && v != "C" && v != "POSIX")
        .map(|v| {
            v.split(['_', '.', '-'])
                .next()
                .unwrap_or_default()
                .to_owned()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_roundtrip_and_private_file() {
        let dir = std::env::temp_dir().join(format!("aw-tokens-{}", std::process::id()));
        let t = Tokens::new(&dir);
        t.put("jellyfin:s:u", "secret").unwrap();
        t.put("plex:m:1", "other").unwrap();
        assert_eq!(t.get("jellyfin:s:u").as_deref(), Some("secret"));
        t.remove("jellyfin:s:u").unwrap();
        assert_eq!(t.get("jellyfin:s:u"), None);
        assert_eq!(t.get("plex:m:1").as_deref(), Some("other"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(dir.join("tokens"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn hex_roundtrip() {
        assert_eq!(unhex(&hex(b"\x00\xffab")).unwrap(), b"\x00\xffab");
        assert!(unhex("zz").is_none());
    }
}
