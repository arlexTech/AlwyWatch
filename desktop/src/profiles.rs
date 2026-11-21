// SPDX-License-Identifier: AGPL-3.0-or-later
//! The device's profiles (several people on one computer), same model as Android's
//! `ProfileStore`: "default" is the profile that existed before profiles (its data stays where it
//! was), every other profile gets its own data directory (database and tokens) under
//! `profiles/<id>`. Downloads are shared by all profiles.
//!
//! A PIN is stored as a salted PBKDF2-HMAC-SHA256 hash (same parameters as Android): it keeps
//! people out of each other's profile on a shared computer; tokens stay protected as before.

use std::num::NonZeroU32;
use std::path::{Path, PathBuf};

use ring::rand::SecureRandom;
use serde_json::{Value, json};

pub const DEFAULT: &str = "default";
const ITERATIONS: u32 = 20_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Profile {
    pub id: String,
    pub name: String,
    pub has_pin: bool,
}

pub struct ProfileStore {
    file: PathBuf,
    data: PathBuf,
}

impl ProfileStore {
    pub fn new(data_dir: &Path) -> Self {
        Self {
            file: data_dir.join("profiles.json"),
            data: data_dir.to_owned(),
        }
    }

    fn raw(&self) -> Value {
        let mut v = std::fs::read_to_string(&self.file)
            .ok()
            .and_then(|s| serde_json::from_str::<Value>(&s).ok())
            .filter(Value::is_object)
            .unwrap_or_else(|| json!({}));
        if !v["list"].is_array() {
            v["list"] = json!([{ "id": DEFAULT, "name": "Me" }]);
        }
        v
    }

    fn save(&self, v: &Value) {
        let _ = std::fs::create_dir_all(&self.data);
        let tmp = self.file.with_extension("tmp");
        if std::fs::write(&tmp, v.to_string()).is_ok() {
            let _ = std::fs::rename(tmp, &self.file);
        }
    }

    fn edit(&self, f: impl FnOnce(&mut Value)) {
        let mut v = self.raw();
        f(&mut v);
        self.save(&v);
    }

    fn entries(v: &mut Value) -> &mut Vec<Value> {
        v["list"].as_array_mut().expect("list is an array (raw)")
    }

    pub fn list(&self) -> Vec<Profile> {
        self.raw()["list"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|o| {
                Some(Profile {
                    id: o["id"].as_str()?.to_owned(),
                    name: o["name"].as_str().unwrap_or("Profile").to_owned(),
                    has_pin: o["pin"].is_object(),
                })
            })
            .collect()
    }

    /// The profile in use (the last one picked), "default" when unknown.
    pub fn current(&self) -> String {
        let v = self.raw();
        v["current"]
            .as_str()
            .filter(|id| self.list().iter().any(|p| p.id == *id))
            .unwrap_or(DEFAULT)
            .to_owned()
    }

    pub fn set_current(&self, id: &str) {
        self.edit(|v| v["current"] = json!(id));
    }

    /// Where a profile keeps its database and tokens.
    pub fn data_dir(&self, id: &str) -> PathBuf {
        if id == DEFAULT {
            self.data.clone()
        } else {
            self.data.join("profiles").join(id)
        }
    }

    pub fn add(&self, name: &str) -> String {
        let mut b = [0u8; 4];
        let _ = ring::rand::SystemRandom::new().fill(&mut b);
        let id = hex(&b);
        let name = clean_name(name);
        self.edit(|v| Self::entries(v).push(json!({ "id": id, "name": name })));
        id
    }

    pub fn rename(&self, id: &str, name: &str) {
        let name = clean_name(name);
        self.edit(|v| {
            for o in Self::entries(v) {
                if o["id"] == id {
                    o["name"] = json!(name);
                }
            }
        });
    }

    /// Removes a profile from the list (never "default"); its files are the caller's.
    pub fn remove(&self, id: &str) {
        if id == DEFAULT {
            return;
        }
        self.edit(|v| Self::entries(v).retain(|o| o["id"] != id));
    }

    /// Sets (or with `None`, removes) a profile's PIN.
    pub fn set_pin(&self, id: &str, pin: Option<&str>) {
        let stored = pin.map(|p| {
            let mut salt = [0u8; 16];
            let _ = ring::rand::SystemRandom::new().fill(&mut salt);
            json!({ "salt": hex(&salt), "hash": hex(&hash(p, &salt)) })
        });
        self.edit(|v| {
            for o in Self::entries(v) {
                if o["id"] != id {
                    continue;
                }
                match &stored {
                    Some(s) => o["pin"] = s.clone(),
                    None => {
                        if let Some(m) = o.as_object_mut() {
                            m.remove("pin");
                        }
                    }
                }
            }
        });
    }

    /// True when `pin` opens the profile (always for a profile without a PIN).
    pub fn check_pin(&self, id: &str, pin: &str) -> bool {
        let v = self.raw();
        let Some(o) = v["list"]
            .as_array()
            .and_then(|l| l.iter().find(|o| o["id"] == id))
        else {
            return false;
        };
        if !o["pin"].is_object() {
            return true;
        }
        let (Some(salt), Some(expected)) = (
            o["pin"]["salt"].as_str().and_then(unhex),
            o["pin"]["hash"].as_str().and_then(unhex),
        ) else {
            return false;
        };
        ring::pbkdf2::verify(
            ring::pbkdf2::PBKDF2_HMAC_SHA256,
            iterations(),
            &salt,
            pin.as_bytes(),
            &expected,
        )
        .is_ok()
    }

    /// Several profiles: open the last one used at start instead of asking who's watching.
    pub fn auto_sign_in(&self) -> bool {
        self.raw()["auto_sign_in"].as_bool().unwrap_or(false)
    }

    pub fn set_auto_sign_in(&self, on: bool) {
        self.edit(|v| v["auto_sign_in"] = json!(on));
    }

    /// Read once at the next start (after a switch whose PIN was already checked), then cleared.
    pub fn take_unlocked_once(&self) -> Option<String> {
        let id = self.raw()["unlocked_once"].as_str().map(str::to_owned)?;
        self.edit(|v| {
            if let Some(m) = v.as_object_mut() {
                m.remove("unlocked_once");
            }
        });
        Some(id)
    }

    pub fn set_unlocked_once(&self, id: &str) {
        self.edit(|v| v["unlocked_once"] = json!(id));
    }

    /// Whether the app opens straight into the current profile, or asks first ("Who's
    /// watching?" or the PIN), with the same rules as Android. Consumes the one-time unlock.
    pub fn opens_directly(&self) -> bool {
        let profiles = self.list();
        let current = self.current();
        let has_pin = profiles.iter().any(|p| p.id == current && p.has_pin);
        let unlocked = self.take_unlocked_once().as_deref() == Some(current.as_str());
        unlocked || (self.auto_sign_in() && !has_pin) || (profiles.len() < 2 && !has_pin)
    }
}

/// A PIN is exactly 4 digits.
pub fn valid_pin(pin: &str) -> bool {
    pin.len() == 4 && pin.bytes().all(|b| b.is_ascii_digit())
}

fn clean_name(name: &str) -> String {
    let n: String = name.trim().chars().take(24).collect();
    if n.is_empty() { "Profile".into() } else { n }
}

fn iterations() -> NonZeroU32 {
    NonZeroU32::new(ITERATIONS).expect("non-zero")
}

fn hash(pin: &str, salt: &[u8]) -> [u8; 32] {
    let mut out = [0u8; 32];
    ring::pbkdf2::derive(
        ring::pbkdf2::PBKDF2_HMAC_SHA256,
        iterations(),
        salt,
        pin.as_bytes(),
        &mut out,
    );
    out
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn unhex(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (ProfileStore, PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "alwywatch-profiles-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        (ProfileStore::new(&dir), dir)
    }

    #[test]
    fn starts_with_the_default_profile() {
        let (s, dir) = store();
        assert_eq!(
            s.list(),
            vec![Profile {
                id: DEFAULT.into(),
                name: "Me".into(),
                has_pin: false
            }]
        );
        assert_eq!(s.current(), DEFAULT);
        assert_eq!(s.data_dir(DEFAULT), dir);
        assert!(s.opens_directly());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn add_rename_remove_and_switch() {
        let (s, dir) = store();
        let id = s.add("  Kids  ");
        assert_eq!(id.len(), 8);
        assert_eq!(s.list()[1].name, "Kids");
        assert_eq!(s.data_dir(&id), dir.join("profiles").join(&id));
        s.rename(&id, "");
        assert_eq!(s.list()[1].name, "Profile");
        // Two profiles: who's watching is asked, unless automatic sign-in.
        assert!(!s.opens_directly());
        s.set_auto_sign_in(true);
        assert!(s.opens_directly());
        s.set_auto_sign_in(false);
        // A switch whose PIN was checked opens directly, once.
        s.set_current(&id);
        s.set_unlocked_once(&id);
        assert!(s.opens_directly());
        assert!(!s.opens_directly());
        s.remove(&id);
        s.remove(DEFAULT);
        assert_eq!(s.list().len(), 1);
        // The removed current profile falls back to the default one.
        assert_eq!(s.current(), DEFAULT);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn pin_is_hashed_checked_and_removable() {
        let (s, dir) = store();
        assert!(s.check_pin(DEFAULT, "0000"));
        s.set_pin(DEFAULT, Some("1234"));
        let raw = std::fs::read_to_string(dir.join("profiles.json")).unwrap();
        assert!(!raw.contains("1234"));
        assert!(s.list()[0].has_pin);
        assert!(s.check_pin(DEFAULT, "1234"));
        assert!(!s.check_pin(DEFAULT, "4321"));
        assert!(!s.check_pin("nobody", "1234"));
        // One profile with a PIN: the PIN is asked at start, even with automatic sign-in.
        s.set_auto_sign_in(true);
        assert!(!s.opens_directly());
        s.set_pin(DEFAULT, None);
        assert!(!s.list()[0].has_pin);
        assert!(s.check_pin(DEFAULT, ""));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn pin_format() {
        assert!(valid_pin("0042"));
        assert!(!valid_pin("042"));
        assert!(!valid_pin("12a4"));
        assert!(!valid_pin("12345"));
        assert_eq!(unhex(&hex(&[0, 255, 16])), Some(vec![0, 255, 16]));
        assert_eq!(unhex("abc"), None);
    }
}
