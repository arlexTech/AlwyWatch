// SPDX-License-Identifier: AGPL-3.0-or-later
//! Languages for the native apps (the `i18n` crate behind UniFFI). The desktop app calls the
//! crate directly.

#[derive(Debug, Clone, uniffi::Record)]
pub struct Language {
    /// "fr", or "en" for the text in the code.
    pub code: String,
    /// In the language itself: "Français".
    pub name: String,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct Translation {
    /// The English text (its key).
    pub id: String,
    /// One translation, or one per plural form.
    pub forms: Vec<String>,
}

/// Every language the apps are translated into, English first.
#[uniffi::export]
pub fn i18n_languages() -> Vec<Language> {
    i18n::languages()
        .into_iter()
        .map(|(code, name)| Language {
            code: code.into(),
            name: name.into(),
        })
        .collect()
}

/// The best language for the system's locales, most preferred first ("fr-BE", "en-US").
#[uniffi::export]
pub fn i18n_best(locales: Vec<String>) -> String {
    i18n::best(locales.iter().map(String::as_str)).into()
}

/// Selects the language of the text the core itself produces (error messages…).
#[uniffi::export]
pub fn i18n_set_language(code: String) {
    i18n::set_language(&code);
}

/// The whole catalog of a language, for the app's own lookup table (empty for English).
#[uniffi::export]
pub fn i18n_table(code: String) -> Vec<Translation> {
    i18n::entries(&code)
        .into_iter()
        .map(|(id, forms)| Translation {
            id: id.into(),
            forms: forms.iter().map(|f| (*f).into()).collect(),
        })
        .collect()
}

/// Which plural form `n` takes in the selected language.
#[uniffi::export]
pub fn i18n_plural_index(n: u64) -> u32 {
    i18n::plural_index(n) as u32
}
