// SPDX-License-Identifier: AGPL-3.0-or-later
//! Playback preferences, kept in this browser: same names, defaults and rules as the apps'
//! Playback settings (skip intros, next episode countdown, skip steps, resuming, "Are you
//! still watching?", languages and the subtitle look).

use i18n::{tr, trn};
use leptos::prelude::*;
use serde::{Deserialize, Serialize};

use crate::state;

const KEY: &str = "aw.player";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PlayerPrefs {
    /// Jump over intros and recaps without asking (when the server marked them).
    pub auto_skip_intro: bool,
    /// When the credits start, play the next episode after a short countdown.
    pub auto_next: bool,
    /// Seconds of that countdown.
    pub countdown_s: u32,
    /// "auto" (when the audio is not in the subtitle language), "forced", "always", "off".
    pub sub_mode: String,
    /// Subtitle language (ISO 639-2), or "" for the browser's language.
    pub sub_lang: String,
    /// What others may do in watch together rooms created here.
    pub together: keep_service::together::TogetherRights,
    /// Audio language (ISO 639-2), or "" for the file's own.
    pub audio_lang: String,
    /// Seconds ←/→ (and a double tap on a side) jump.
    pub seek_back_s: u32,
    pub seek_forward_s: u32,
    /// Seconds played again when resuming.
    pub resume_rewind_s: u32,
    /// Hours without a press before playback pauses and asks (0: never).
    pub still_watching_h: u32,
    /// Subtitle colour: "white" or "yellow".
    pub sub_color: String,
    /// A dark box behind subtitles (else an outline).
    pub sub_bg: bool,
    /// The menu's libraries: their order and the hidden ones (library ids, as the apps).
    pub menu_libs: Vec<String>,
    pub menu_hidden: Vec<String>,
}

impl Default for PlayerPrefs {
    fn default() -> Self {
        Self {
            auto_skip_intro: false,
            auto_next: true,
            countdown_s: 10,
            sub_mode: "auto".into(),
            sub_lang: String::new(),
            together: Default::default(),
            audio_lang: String::new(),
            seek_back_s: 10,
            seek_forward_s: 30,
            resume_rewind_s: 0,
            still_watching_h: 2,
            sub_color: "white".into(),
            sub_bg: false,
            menu_libs: vec![],
            menu_hidden: vec![],
        }
    }
}

pub fn load() -> PlayerPrefs {
    state::load(KEY).unwrap_or_default()
}

/// Values taken from the other devices (not shared back as changes made here).
pub fn save_quietly(p: &PlayerPrefs) {
    state::save(KEY, p);
}

pub fn save(p: &PlayerPrefs) {
    let before = load();
    state::save(KEY, p);
    // Shared with the other devices (a moment later).
    crate::sync::changed(&before, p);
}

/// The languages offered, as on the apps: ISO 639-2/T, 639-1, 639-2/B (names: `language_label`).
const LANGUAGES: &[(&str, &str, &str)] = &[
    ("eng", "en", "eng"),
    ("fra", "fr", "fre"),
    ("nld", "nl", "dut"),
    ("deu", "de", "ger"),
    ("spa", "es", "spa"),
    ("ita", "it", "ita"),
    ("por", "pt", "por"),
    ("jpn", "ja", "jpn"),
    ("kor", "ko", "kor"),
    ("zho", "zh", "chi"),
    ("ara", "ar", "ara"),
    ("rus", "ru", "rus"),
    ("pol", "pl", "pol"),
    ("swe", "sv", "swe"),
    ("tur", "tr", "tur"),
    ("hin", "hi", "hin"),
];

/// A language's name in the selected language (codes from `LANGUAGES`).
fn language_label(code: &str) -> Option<String> {
    Some(match code {
        "eng" => tr!("English"),
        "fra" => tr!("French"),
        "nld" => tr!("Dutch"),
        "deu" => tr!("German"),
        "spa" => tr!("Spanish"),
        "ita" => tr!("Italian"),
        "por" => tr!("Portuguese"),
        "jpn" => tr!("Japanese"),
        "kor" => tr!("Korean"),
        "zho" => tr!("Chinese"),
        "ara" => tr!("Arabic"),
        "rus" => tr!("Russian"),
        "pol" => tr!("Polish"),
        "swe" => tr!("Swedish"),
        "tur" => tr!("Turkish"),
        "hin" => tr!("Hindi"),
        _ => return None,
    })
}

/// Any language code ("fr", "fre", "fra", "fr-BE") as ISO 639-2/T; unknown ones as given.
pub fn lang3(code: &str) -> Option<String> {
    let c = code
        .trim()
        .split(['-', '_'])
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    if c.is_empty() {
        return None;
    }
    Some(
        LANGUAGES
            .iter()
            .find(|(t, one, b)| c == *t || c == *one || c == *b)
            .map_or(c, |(t, ..)| (*t).to_owned()),
    )
}

fn language_name(code: &str) -> String {
    let c = lang3(code).unwrap_or_default();
    language_label(&c).unwrap_or(c)
}

/// The browser's language ("fr-BE").
pub fn browser_language() -> String {
    web_sys::window()
        .and_then(|w| w.navigator().language())
        .unwrap_or_default()
}

/// Seconds left of the next episode countdown, started when the credits showed at
/// `from_ms` (playback time, so pausing pauses it too).
fn seconds(n: u32) -> String {
    match n {
        0 => tr!("Off"),
        60 => trn!("{n} minute", "{n} minutes", 1),
        n => trn!("{n} second", "{n} seconds", n),
    }
}

/// The menu's libraries in the user's order (new ones last), without the hidden ones unless
/// `all`. `id` gives a library's id.
pub fn arrange<T>(mut libs: Vec<T>, id: impl Fn(&T) -> &str, p: &PlayerPrefs, all: bool) -> Vec<T> {
    libs.sort_by_key(|l| {
        p.menu_libs
            .iter()
            .position(|o| o == id(l))
            .unwrap_or(usize::MAX)
    });
    libs.retain(|l| all || !p.menu_hidden.iter().any(|h| h == id(l)));
    libs
}

/// Shared by the menu and Settings → Servers, so a change shows at once.
#[derive(Clone, Copy)]
pub struct MenuPrefs(pub RwSignal<PlayerPrefs>);

pub fn countdown_left(countdown_s: u32, from_ms: u64, pos_ms: u64) -> u32 {
    let gone = (pos_ms.saturating_sub(from_ms) / 1000) as u32;
    countdown_s.saturating_sub(gone)
}

/// The Playback part of Settings.
#[component]
pub fn PlaybackSettings() -> impl IntoView {
    let prefs = RwSignal::new(load());
    let change = move |f: &dyn Fn(&mut PlayerPrefs)| {
        prefs.update(|p| f(p));
        save(&prefs.get_untracked());
    };
    view! {
        <section class="group">
            <h2>{tr!("Intros and credits")}</h2>
            <label class="row toggle">
                <div>{tr!("Skip intros automatically")}<div class="muted small">{tr!("Otherwise a Skip intro button appears.")}</div></div>
                <input type="checkbox" prop:checked=move || prefs.with(|p| p.auto_skip_intro)
                    on:change=move |ev| { let on = event_target_checked(&ev); change(&|p| p.auto_skip_intro = on) } />
            </label>
            <label class="row toggle">
                <div>{tr!("Play the next episode automatically")}<div class="muted small">{tr!("A countdown during the credits, which Cancel stops. Off: playback stops at the end of the episode.")}</div></div>
                <input type="checkbox" prop:checked=move || prefs.with(|p| p.auto_next)
                    on:change=move |ev| { let on = event_target_checked(&ev); change(&|p| p.auto_next = on) } />
            </label>
            <div class="row">
                <span>{tr!("Next episode countdown")}</span>
                <select class="chip" aria-label=tr!("Next episode countdown")
                    prop:value=move || prefs.with(|p| p.countdown_s.to_string())
                    on:change=move |ev| { let s: u32 = event_target_value(&ev).parse().unwrap_or(10); change(&|p| p.countdown_s = s) }>
                    {[5u32, 10, 15, 30].map(|s| view! { <option value=s.to_string()>{trn!("{n} second", "{n} seconds", s)}</option> }).collect_view()}
                </select>
            </div>
        </section>
        <section class="group">
            <h2>{tr!("Skipping and resuming")}</h2>
            <div class="row">
                <div>{tr!("Skip back")}<div class="muted small">{tr!("Left arrow, or a double tap on the left of the picture.")}</div></div>
                <select class="chip" aria-label=tr!("Skip back")
                    prop:value=move || prefs.with(|p| p.seek_back_s.to_string())
                    on:change=move |ev| { let s: u32 = event_target_value(&ev).parse().unwrap_or(10); change(&|p| p.seek_back_s = s) }>
                    {[5u32, 10, 15, 30].map(|s| view! { <option value=s.to_string()>{seconds(s)}</option> }).collect_view()}
                </select>
            </div>
            <div class="row">
                <div>{tr!("Skip forward")}<div class="muted small">{tr!("Right arrow, or a double tap on the right of the picture.")}</div></div>
                <select class="chip" aria-label=tr!("Skip forward")
                    prop:value=move || prefs.with(|p| p.seek_forward_s.to_string())
                    on:change=move |ev| { let s: u32 = event_target_value(&ev).parse().unwrap_or(30); change(&|p| p.seek_forward_s = s) }>
                    {[10u32, 15, 30, 60].map(|s| view! { <option value=s.to_string()>{seconds(s)}</option> }).collect_view()}
                </select>
            </div>
            <div class="row">
                <span>{tr!("Rewind when resuming")}</span>
                <select class="chip" aria-label=tr!("Rewind when resuming")
                    prop:value=move || prefs.with(|p| p.resume_rewind_s.to_string())
                    on:change=move |ev| { let s: u32 = event_target_value(&ev).parse().unwrap_or(0); change(&|p| p.resume_rewind_s = s) }>
                    {[0u32, 5, 10, 30].map(|s| view! { <option value=s.to_string()>{seconds(s)}</option> }).collect_view()}
                </select>
            </div>
            <div class="row">
                <div>{tr!("Are you still watching?")}<div class="muted small">{tr!("Pauses when no key or player button has been pressed for that long.")}</div></div>
                <select class="chip" aria-label=tr!("Are you still watching?")
                    prop:value=move || prefs.with(|p| p.still_watching_h.to_string())
                    on:change=move |ev| { let h: u32 = event_target_value(&ev).parse().unwrap_or(2); change(&|p| p.still_watching_h = h) }>
                    <option value="0">{tr!("Never")}</option>
                    {[2u32, 3, 4].map(|h| view! { <option value=h.to_string()>{trn!("After {n} hour", "After {n} hours", h)}</option> }).collect_view()}
                </select>
            </div>
        </section>
        <section class="group">
            <h2>{tr!("Watch together")}</h2>
            <p class="muted small">{tr!("In rooms you create, what the others may do. You can always do everything.")}</p>
            <label class="row toggle">
                <div>{tr!("Others can change what's playing")}<div class="muted small">{tr!("Next episode, another title.")}</div></div>
                <input type="checkbox" prop:checked=move || prefs.with(|p| p.together.change_title)
                    on:change=move |ev| { let on = event_target_checked(&ev); change(&|p| p.together.change_title = on) } />
            </label>
            <label class="row toggle">
                <div>{tr!("Others can pause and play")}</div>
                <input type="checkbox" prop:checked=move || prefs.with(|p| p.together.play_pause)
                    on:change=move |ev| { let on = event_target_checked(&ev); change(&|p| p.together.play_pause = on) } />
            </label>
            <label class="row toggle">
                <div>{tr!("Others can move forward or back")}</div>
                <input type="checkbox" prop:checked=move || prefs.with(|p| p.together.seek)
                    on:change=move |ev| { let on = event_target_checked(&ev); change(&|p| p.together.seek = on) } />
            </label>
        </section>
    }
}

/// The Audio & subtitles part of Settings.
#[component]
pub fn AudioSettings() -> impl IntoView {
    let prefs = RwSignal::new(load());
    let change = move |f: &dyn Fn(&mut PlayerPrefs)| {
        prefs.update(|p| f(p));
        save(&prefs.get_untracked());
    };
    let browser = language_name(&browser_language());
    view! {
        <section class="group">
            <h2>{tr!("Languages")}</h2>
            <div class="row">
                <span>{tr!("Audio language")}</span>
                <select class="chip" aria-label=tr!("Audio language")
                    prop:value=move || prefs.with(|p| p.audio_lang.clone())
                    on:change=move |ev| { let l = event_target_value(&ev); change(&|p| p.audio_lang = l.clone()) }>
                    <option value="">{tr!("Original")}</option>
                    {LANGUAGES.iter().map(|(code, ..)| view! { <option value=*code>{language_label(code)}</option> }).collect_view()}
                </select>
            </div>
            <div class="row">
                <div>{tr!("Subtitles")}<div class="muted small">{tr!("Automatic: when the audio is not in your subtitle language.")}</div></div>
                <select class="chip" aria-label=tr!("Subtitles")
                    prop:value=move || prefs.with(|p| p.sub_mode.clone())
                    on:change=move |ev| { let m = event_target_value(&ev); change(&|p| p.sub_mode = m.clone()) }>
                    <option value="auto">{tr!("Automatic")}</option>
                    <option value="forced">{tr!("Only foreign-language parts")}</option>
                    <option value="always">{tr!("Always")}</option>
                    <option value="off">{tr!("Off")}</option>
                </select>
            </div>
            <div class="row">
                <span>{tr!("Subtitle language")}</span>
                <select class="chip" aria-label=tr!("Subtitle language")
                    prop:value=move || prefs.with(|p| p.sub_lang.clone())
                    on:change=move |ev| { let l = event_target_value(&ev); change(&|p| p.sub_lang = l.clone()) }>
                    <option value="">{tr!("Browser language ({language})", language = browser)}</option>
                    {LANGUAGES.iter().map(|(code, ..)| view! { <option value=*code>{language_label(code)}</option> }).collect_view()}
                </select>
            </div>
            <p class="muted small">{tr!("Subtitles the browser can't draw (styled or image ones) are added to the picture by the server. A movie's or show's own choice, made here or in the apps, comes first.")}</p>
        </section>
        <section class="group">
            <h2>{tr!("Subtitle look")}</h2>
            <div class="row">
                <span>{tr!("Colour")}</span>
                <select class="chip" aria-label=tr!("Colour")
                    prop:value=move || prefs.with(|p| p.sub_color.clone())
                    on:change=move |ev| { let c = event_target_value(&ev); change(&|p| p.sub_color = c.clone()) }>
                    <option value="white">{tr!("White")}</option>
                    <option value="yellow">{tr!("Yellow")}</option>
                </select>
            </div>
            <label class="row toggle">
                <div>{tr!("Background box")}<div class="muted small">{tr!("Otherwise a dark outline keeps them readable.")}</div></div>
                <input type="checkbox" prop:checked=move || prefs.with(|p| p.sub_bg)
                    on:change=move |ev| { let on = event_target_checked(&ev); change(&|p| p.sub_bg = on) } />
            </label>
            <p class="muted small">{tr!("The size follows the browser's own caption settings.")}</p>
        </section>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_the_apps_and_old_values_load() {
        let p: PlayerPrefs = serde_json::from_str(r#"{"auto_next":false}"#).unwrap();
        assert!(!p.auto_next);
        assert!(!p.auto_skip_intro);
        assert_eq!(p.countdown_s, 10);
        assert_eq!(p.sub_mode, "auto");
        assert_eq!(p.sub_lang, "");
    }

    #[test]
    fn language_codes_normalise() {
        assert_eq!(lang3("fr-BE").as_deref(), Some("fra"));
        assert_eq!(lang3("fre").as_deref(), Some("fra"));
        assert_eq!(lang3("ger").as_deref(), Some("deu"));
        assert_eq!(lang3("xyz").as_deref(), Some("xyz"));
        assert_eq!(lang3(""), None);
    }

    #[test]
    fn menu_libraries_follow_the_apps_order_and_hiding() {
        let p = PlayerPrefs {
            menu_libs: vec!["b".into(), "a".into()],
            menu_hidden: vec!["a".into()],
            ..Default::default()
        };
        let libs = vec!["a", "new", "b"];
        assert_eq!(arrange(libs.clone(), |l| l, &p, true), ["b", "a", "new"]);
        assert_eq!(arrange(libs, |l| l, &p, false), ["b", "new"]);
    }

    #[test]
    fn countdown_runs_on_playback_time() {
        assert_eq!(countdown_left(10, 5_000, 5_000), 10);
        assert_eq!(countdown_left(10, 5_000, 9_200), 6);
        assert_eq!(countdown_left(10, 5_000, 30_000), 0);
        assert_eq!(countdown_left(10, 5_000, 1_000), 10);
    }
}
