// SPDX-License-Identifier: AGPL-3.0-or-later
//! Translations for every AlwyWatch app.
//!
//! The text in the code is English and is its own key (gettext style): `tr("Sign in")`. Each
//! language is one file, `/i18n/<code>.po`, compiled into the binary by build.rs; a missing or
//! untranslated entry shows the English text. Adding a language is adding a file: see
//! docs/i18n.md.
//!
//! Placeholders are named, `{days}`, filled with [`tr!`]; plurals use `{n}` for the count
//! ([`trn`]). The language is process-wide ([`set_language`]), chosen by the app from its
//! setting or the system's locales ([`best`]).

use std::borrow::Cow;
use std::sync::atomic::{AtomicUsize, Ordering};

#[doc(hidden)]
pub mod po;

pub struct Catalog {
    pub code: &'static str,
    /// The language's own name ("Français"), from the `X-Language-Name` header.
    pub name: &'static str,
    plural: &'static str,
    /// Sorted by msgid: (msgid, [msgstr] or [msgstr[0], msgstr[1]…]).
    entries: &'static [(&'static str, &'static [&'static str])],
}

include!(concat!(env!("OUT_DIR"), "/catalogs.rs"));

/// 0: English (the text in the code); i: `CATALOGS[i - 1]`.
static CURRENT: AtomicUsize = AtomicUsize::new(0);

fn current() -> Option<&'static Catalog> {
    match CURRENT.load(Ordering::Relaxed) {
        0 => None,
        i => CATALOGS.get(i - 1),
    }
}

/// Every language available, English first: (code, own name).
pub fn languages() -> Vec<(&'static str, &'static str)> {
    std::iter::once(("en", "English"))
        .chain(CATALOGS.iter().map(|c| (c.code, c.name)))
        .collect()
}

/// The best language for these locales, most preferred first (`fr-BE`, `fr_FR.UTF-8`, `en`):
/// the first one with a catalog, English otherwise.
pub fn best<'a>(locales: impl IntoIterator<Item = &'a str>) -> &'static str {
    for l in locales {
        let l = l.trim().to_ascii_lowercase().replace('_', "-");
        let l = l.split(['.', '@']).next().unwrap_or("");
        let base = l.split('-').next().unwrap_or("");
        if base == "en" {
            return "en";
        }
        // Exact (pt-br) first, then the base language (fr-be → fr).
        for want in [l, base] {
            if let Some(c) = CATALOGS.iter().find(|c| c.code.eq_ignore_ascii_case(want)) {
                return c.code;
            }
        }
    }
    "en"
}

/// Selects the language by code; English for "", "en" or a code without a catalog.
pub fn set_language(code: &str) {
    let i = CATALOGS
        .iter()
        .position(|c| c.code.eq_ignore_ascii_case(code.trim()))
        .map_or(0, |i| i + 1);
    CURRENT.store(i, Ordering::Relaxed);
}

/// The code of the selected language ("en" when English).
pub fn language() -> &'static str {
    current().map_or("en", |c| c.code)
}

fn find(c: &Catalog, msgid: &str) -> Option<&'static [&'static str]> {
    c.entries
        .binary_search_by(|(k, _)| (*k).cmp(msgid))
        .ok()
        .map(|i| c.entries[i].1)
}

/// The translation of `text` in the selected language, or `text` itself.
pub fn tr<'a>(text: &'a str) -> Cow<'a, str> {
    match current()
        .and_then(|c| find(c, text))
        .and_then(|s| s.first())
    {
        Some(t) => Cow::Borrowed(t),
        None => Cow::Borrowed(text),
    }
}

/// The translation of `text` in the language `code` (not the selected one: for a server that
/// writes to several people), or `text` itself.
pub fn tr_in<'a>(code: &str, text: &'a str) -> Cow<'a, str> {
    match CATALOGS
        .iter()
        .find(|c| c.code.eq_ignore_ascii_case(code.trim()))
        .and_then(|c| find(c, text))
        .and_then(|s| s.first())
    {
        Some(t) => Cow::Borrowed(t),
        None => Cow::Borrowed(text),
    }
}

/// Which plural form `n` takes in the selected language (English: 0 for 1, else 1).
pub fn plural_index(n: u64) -> usize {
    match current() {
        None => usize::from(n != 1),
        Some(c) => po::plural_index(c.plural, n).unwrap_or(usize::from(n != 1)),
    }
}

/// The form of `singular`/`plural` for `n`, with `{n}` replaced by `n`.
pub fn trn(singular: &str, plural: &str, n: u64) -> String {
    let form = current()
        .and_then(|c| find(c, singular))
        .and_then(|s| s.get(plural_index(n)).or(s.last()).copied());
    let text = form.unwrap_or(if n == 1 { singular } else { plural });
    text.replace("{n}", &n.to_string())
}

/// Replaces each `{name}` with its value.
pub fn fill(text: &str, args: &[(&str, &dyn std::fmt::Display)]) -> String {
    let mut out = text.to_owned();
    for (k, v) in args {
        out = out.replace(&format!("{{{k}}}"), &v.to_string());
    }
    out
}

/// Every entry of a language: (msgid, forms). For apps that keep their own lookup table
/// (Android loads it once).
pub fn entries(code: &str) -> Vec<(&'static str, &'static [&'static str])> {
    CATALOGS
        .iter()
        .find(|c| c.code.eq_ignore_ascii_case(code))
        .map(|c| c.entries.to_vec())
        .unwrap_or_default()
}

/// `tr!("Sign in")` → String; `tr!("Leaves in {days} days", days = d)` fills placeholders.
#[macro_export]
macro_rules! tr {
    ($text:expr) => {
        $crate::tr($text).into_owned()
    };
    ($text:expr, $($name:ident = $value:expr),+ $(,)?) => {
        $crate::fill(&$crate::tr($text), &[$((stringify!($name), &$value as &dyn ::std::fmt::Display)),+])
    };
}

/// [`trn`] in the language `code`.
pub fn trn_in(code: &str, singular: &str, plural: &str, n: u64) -> String {
    let c = CATALOGS
        .iter()
        .find(|c| c.code.eq_ignore_ascii_case(code.trim()));
    let index = |c: &Catalog| po::plural_index(c.plural, n).unwrap_or(usize::from(n != 1));
    let form =
        c.and_then(|c| find(c, singular).and_then(|s| s.get(index(c)).or(s.last()).copied()));
    form.unwrap_or(if n == 1 { singular } else { plural })
        .replace("{n}", &n.to_string())
}

/// `tr_in!(lang, "Hello {name}", name = n)`: [`tr_in`] with placeholders filled.
#[macro_export]
macro_rules! tr_in {
    ($lang:expr, $text:expr) => {
        $crate::tr_in($lang, $text).into_owned()
    };
    ($lang:expr, $text:expr, $($name:ident = $value:expr),+ $(,)?) => {
        $crate::fill(&$crate::tr_in($lang, $text), &[$((stringify!($name), &$value as &dyn ::std::fmt::Display)),+])
    };
}

/// `trn!("{n} title", "{n} titles", count)`, with more named placeholders after the count.
#[macro_export]
macro_rules! trn {
    ($one:expr, $many:expr, $n:expr) => {
        $crate::trn($one, $many, ($n) as u64)
    };
    ($one:expr, $many:expr, $n:expr, $($name:ident = $value:expr),+ $(,)?) => {
        $crate::fill(&$crate::trn($one, $many, ($n) as u64), &[$((stringify!($name), &$value as &dyn ::std::fmt::Display)),+])
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn po_files_and_plural_rules() {
        let f = po::parse(
            "msgid \"\"\nmsgstr \"\"\n\"Language: fr\\n\"\n\"Plural-Forms: nplurals=2; plural=(n > 1);\\n\"\n\n\
             #: a.rs:1\nmsgid \"Sign in\"\nmsgstr \"Se connecter\"\n\n\
             #, fuzzy\nmsgid \"Old\"\nmsgstr \"Vieux\"\n\n\
             msgid \"{n} title\"\nmsgid_plural \"{n} titles\"\nmsgstr[0] \"{n} titre\"\nmsgstr[1] \"{n} \"\n\"titres\"\n\n\
             #~ msgid \"Gone\"\n#~ msgstr \"Parti\"\n",
        )
        .unwrap();
        assert_eq!(f.header("language"), Some("fr"));
        assert_eq!(f.entries.len(), 3);
        assert!(f.entries[1].fuzzy && !f.entries[0].fuzzy && !f.entries[2].fuzzy);
        assert_eq!(f.entries[2].msgstr, ["{n} titre", "{n} titres"]);

        assert_eq!(po::plural_index("(n > 1)", 0), Ok(0));
        assert_eq!(po::plural_index("(n > 1)", 2), Ok(1));
        assert_eq!(po::plural_index("n != 1", 0), Ok(1));
        let slavic =
            "(n%10==1 && n%100!=11 ? 0 : n%10>=2 && n%10<=4 && (n%100<10 || n%100>=20) ? 1 : 2)";
        let forms: Vec<_> = [1, 2, 5, 11, 22, 25]
            .iter()
            .map(|&n| po::plural_index(slavic, n).unwrap())
            .collect();
        assert_eq!(forms, [0, 1, 2, 2, 1, 2]);
        assert!(po::plural_index("n +", 1).is_err());
    }

    #[test]
    fn english_is_the_fallback() {
        set_language("xx");
        assert_eq!(language(), "en");
        assert_eq!(
            tr("Something nobody translated"),
            "Something nobody translated"
        );
        assert_eq!(trn!("{n} title", "{n} titles", 1), "1 title");
        assert_eq!(trn!("{n} title", "{n} titles", 3), "3 titles");
        assert_eq!(tr!("Leaves in {days} days", days = 4), "Leaves in 4 days");
        assert_eq!(best(["de-DE", "en-US"]), "en");
        assert_eq!(languages()[0], ("en", "English"));

        // The language is process-wide: checked in this one test, not in parallel ones.
        if CATALOGS.iter().any(|c| c.code == "fr") {
            assert_eq!(tr_in("fr", "Sign in"), "Se connecter");
            assert_eq!(tr_in("xx", "Sign in"), "Sign in");
            set_language("fr");
            assert_eq!(best(["fr_BE.UTF-8"]), "fr");
            assert_eq!(tr("Sign in"), "Se connecter");
            assert_eq!(trn!("{n} title", "{n} titles", 0), "0 titre");
            assert_eq!(trn!("{n} title", "{n} titles", 2), "2 titres");
            assert_eq!(
                tr("Something nobody translated"),
                "Something nobody translated"
            );
            set_language("en");
        }
    }

    #[test]
    fn every_catalog_is_usable() {
        for c in CATALOGS {
            assert!(po::plural_index(c.plural, 1).is_ok(), "{}", c.code);
            assert_eq!(best([format!("{}-XX", c.code).as_str()]), c.code);
            // Sorted, so lookups find everything.
            assert!(c.entries.windows(2).all(|w| w[0].0 < w[1].0), "{}", c.code);
            for (id, forms) in c.entries {
                assert!(!forms.is_empty(), "{}: {id}", c.code);
            }
        }
    }
}
