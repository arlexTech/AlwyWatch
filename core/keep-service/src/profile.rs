// SPDX-License-Identifier: AGPL-3.0-or-later
//! Account profile: the display name and avatar an AlwyWatch user shows to others.
//!
//! - `PUT /v1/me/name` with [`DisplayNameRequest`] → `Me`; `DELETE /v1/me/name` → `Me` (back
//!   to the media-server name).
//! - `PUT /v1/me/avatar` with [`AvatarChoice`] (JSON) or a raw picture (`image/jpeg`,
//!   `image/png`, `image/webp`, 512 KiB at most) → `Me`; `DELETE /v1/me/avatar` → `Me`.
//! - `GET /v1/users/{user_key}/avatar?v={custom_version}` → the custom picture.
//!
//! Built-in avatars are drawn by the apps from [`avatar_presets`]; the default one (no choice)
//! is [`avatar_initial`] on [`default_avatar_color`]. Details: `docs/keep-protocol.md`.

use serde::{Deserialize, Serialize};

macro_rules! record {
    ($(#[$m:meta])* pub struct $name:ident { $($body:tt)* }) => {
        $(#[$m])*
        #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
        #[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
        pub struct $name { $($body)* }
    };
}

/// Largest custom picture accepted (bytes).
pub const AVATAR_MAX_BYTES: usize = 512 * 1024;
/// Largest side of a custom picture the apps upload (pixels, square).
pub const AVATAR_MAX_SIDE: u32 = 512;
/// Display names: characters at most, after trimming.
pub const DISPLAY_NAME_MAX: usize = 32;
/// Picture types accepted for custom avatars.
pub const AVATAR_TYPES: [&str; 3] = ["image/jpeg", "image/png", "image/webp"];

record! {
    /// How to draw someone. Both `None`: the default ([`avatar_initial`] on
    /// [`default_avatar_color`] of their name).
    #[derive(Default)]
    pub struct Avatar {
        /// A built-in avatar id (see [`avatar_presets`]).
        #[serde(default)]
        pub preset: Option<String>,
        /// A custom picture: fetch `GET /v1/users/{user_key}/avatar?v={custom_version}` (signed
        /// in) and cache it by this version (it changes on every change).
        #[serde(default)]
        pub custom_version: Option<u32>,
    }
}

record! {
    /// A built-in avatar: a disc of `background` with `shape` drawn in `foreground` (see
    /// `docs/keep-protocol.md` for the geometry of every shape).
    pub struct AvatarPreset {
        pub id: String,
        /// `#RRGGBB`.
        pub background: String,
        /// `#RRGGBB`.
        pub foreground: String,
        /// One of: circle, ring, square, diamond, triangle, plus, cross, star, hexagon, bars,
        /// half, dots.
        pub shape: String,
    }
}

record! {
    /// `PUT /v1/me/avatar` body choosing a built-in avatar.
    pub struct AvatarChoice {
        pub preset: String,
    }
}

record! {
    /// `PUT /v1/me/name` body.
    pub struct DisplayNameRequest {
        pub name: String,
    }
}

const PRESETS: [(&str, &str, &str); 12] = [
    ("p01", "#C62828", "circle"),
    ("p02", "#AD1457", "ring"),
    ("p03", "#6A1B9A", "square"),
    ("p04", "#4527A0", "diamond"),
    ("p05", "#1565C0", "triangle"),
    ("p06", "#00838F", "plus"),
    ("p07", "#2E7D32", "cross"),
    ("p08", "#9E6A00", "star"),
    ("p09", "#D84315", "hexagon"),
    ("p10", "#4E342E", "bars"),
    ("p11", "#37474F", "half"),
    ("p12", "#00695C", "dots"),
];

/// Colours of default avatars, picked by [`default_avatar_color`].
const DEFAULT_COLORS: [&str; 8] = [
    "#C62828", "#AD1457", "#6A1B9A", "#1565C0", "#00838F", "#2E7D32", "#D84315", "#4E342E",
];

/// The built-in avatars, in display order.
pub fn avatar_presets() -> Vec<AvatarPreset> {
    PRESETS
        .iter()
        .map(|(id, bg, shape)| AvatarPreset {
            id: (*id).into(),
            background: (*bg).into(),
            foreground: "#FFFFFF".into(),
            shape: (*shape).into(),
        })
        .collect()
}

pub fn is_avatar_preset(id: &str) -> bool {
    PRESETS.iter().any(|(p, _, _)| *p == id)
}

/// Background of someone's default avatar: FNV-1a (32 bit) of the name's UTF-8 bytes, modulo
/// the 8 colours.
pub fn default_avatar_color(name: &str) -> String {
    let mut h: u32 = 0x811c_9dc5;
    for b in name.bytes() {
        h ^= u32::from(b);
        h = h.wrapping_mul(0x0100_0193);
    }
    DEFAULT_COLORS[(h % DEFAULT_COLORS.len() as u32) as usize].into()
}

/// Letter of someone's default avatar: the name's first letter or digit, uppercased ("?" when
/// there is none), drawn in white.
pub fn avatar_initial(name: &str) -> String {
    name.chars()
        .find(|c| c.is_alphanumeric())
        .map(|c| c.to_uppercase().collect())
        .unwrap_or_else(|| "?".into())
}

/// A display name as stored: trimmed, inner whitespace collapsed to one space, 1 to 32
/// characters, no control characters or line breaks. The error says what is wrong in plain
/// words.
pub fn clean_display_name(raw: &str) -> Result<String, String> {
    if raw.chars().any(char::is_control) {
        return Err("the name cannot contain line breaks or control characters".into());
    }
    let name = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    let n = name.chars().count();
    if n == 0 {
        return Err("the name cannot be empty".into());
    }
    if n > DISPLAY_NAME_MAX {
        return Err(format!(
            "the name is too long ({DISPLAY_NAME_MAX} characters at most)"
        ));
    }
    Ok(name)
}

/// Whether `bytes` really are a picture of `content_type` (magic numbers).
pub fn avatar_bytes_match(content_type: &str, bytes: &[u8]) -> bool {
    match content_type {
        "image/jpeg" => bytes.starts_with(&[0xFF, 0xD8, 0xFF]),
        "image/png" => bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]),
        "image/webp" => bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP",
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_trimmed_collapsed_and_bounded() {
        assert_eq!(clean_display_name("  Ana   Maria ").unwrap(), "Ana Maria");
        assert!(clean_display_name("   ").is_err());
        assert!(clean_display_name("a\nb").is_err());
        assert!(clean_display_name("a\u{7}b").is_err());
        assert!(clean_display_name(&"é".repeat(32)).is_ok());
        assert!(clean_display_name(&"é".repeat(33)).is_err());
    }

    #[test]
    fn presets_and_defaults_are_stable() {
        let p = avatar_presets();
        assert_eq!(p.len(), 12);
        assert!(p.iter().all(|p| is_avatar_preset(&p.id)));
        assert!(!is_avatar_preset("p13"));
        // FNV-1a of "" is the offset basis: 0x811c9dc5 % 8 == 5.
        assert_eq!(default_avatar_color(""), "#2E7D32");
        assert_eq!(default_avatar_color("ana"), default_avatar_color("ana"));
        assert_eq!(avatar_initial("  élodie"), "É");
        assert_eq!(avatar_initial("--"), "?");
    }

    #[test]
    fn magic_numbers() {
        assert!(avatar_bytes_match("image/jpeg", &[0xFF, 0xD8, 0xFF, 0xE0]));
        assert!(!avatar_bytes_match("image/png", &[0xFF, 0xD8, 0xFF, 0xE0]));
        assert!(avatar_bytes_match("image/webp", b"RIFF\0\0\0\0WEBPVP8 "));
        assert!(!avatar_bytes_match("image/gif", b"GIF89a"));
    }
}
