// SPDX-License-Identifier: AGPL-3.0-or-later
//! The app's language (docs/i18n.md): the choice in Settings, saved for this computer, or the
//! system's. Text set from Rust goes through `i18n::tr!`; text in the .slint files through
//! `@tr(...)`, from the same catalogs (bundled by build.rs).

use std::path::Path;

fn file(data: &Path) -> std::path::PathBuf {
    data.join("language")
}

/// The choice in Settings: "" follows the system.
pub fn choice(data: &Path) -> String {
    std::fs::read_to_string(file(data))
        .map(|s| s.trim().to_owned())
        .unwrap_or_default()
}

/// The language to use for this choice.
pub fn resolve(choice: &str) -> String {
    if !choice.is_empty() {
        return choice.to_owned();
    }
    let system: Vec<String> = sys_locale::get_locales().collect();
    i18n::best(system.iter().map(String::as_str)).to_owned()
}

/// Selects it for the text built in Rust and, once the window exists, for the .slint files.
pub fn apply(code: &str, window_exists: bool) {
    i18n::set_language(code);
    if window_exists {
        let _ = slint::select_bundled_translation(if code == "en" { "" } else { code });
    }
}

/// Saves the choice ("" = the system's) and applies it.
pub fn choose(data: &Path, choice: &str) {
    let _ = std::fs::write(file(data), choice);
    apply(&resolve(choice), true);
}
