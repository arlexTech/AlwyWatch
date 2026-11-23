// SPDX-License-Identifier: AGPL-3.0-or-later
//! Playback preferences, kept in this browser: same names, defaults and rules as the apps'
//! Playback settings (skip intros, next episode countdown, subtitle mode and language).

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
        }
    }
}

pub fn load() -> PlayerPrefs {
    state::load(KEY).unwrap_or_default()
}

pub fn save(p: &PlayerPrefs) {
    state::save(KEY, p)
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

/// A subtitle track the player can show.
#[derive(Clone, Debug)]
pub struct SubTrack {
    pub index: i64,
    pub language: Option<String>,
    pub forced: bool,
}

/// The subtitle track to show at start (None: off), by the same rules as the apps: "auto"
/// shows subtitles in our language when the audio is in another one, else only forced ones
/// (signs, foreign dialogue).
pub fn pick_subtitle(
    subs: &[SubTrack],
    audio_lang: Option<&str>,
    mode: &str,
    sub_lang: &str,
) -> Option<i64> {
    let audio = audio_lang.and_then(lang3);
    let ours = lang3(sub_lang);
    let pick = |lang: Option<&String>, forced_only: bool| -> Option<i64> {
        let is = |t: &SubTrack| t.language.as_deref().and_then(lang3).as_ref() == lang;
        if forced_only {
            subs.iter().find(|t| t.forced && (lang.is_none() || is(t)))
        } else {
            subs.iter()
                .find(|t| !t.forced && is(t))
                .or_else(|| subs.iter().find(|t| is(t)))
        }
        .map(|t| t.index)
    };
    match mode {
        "off" => None,
        "always" => pick(ours.as_ref(), false),
        "forced" => pick(audio.as_ref(), true),
        _ => match (&audio, &ours) {
            (Some(a), Some(s)) if a != s => pick(Some(s), false).or_else(|| pick(Some(a), true)),
            _ => pick(audio.as_ref(), true),
        },
    }
}

/// Seconds left of the next episode countdown, started when the credits showed at
/// `from_ms` (playback time, so pausing pauses it too).
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
            <p class="muted small">{tr!("Only text subtitles can be shown in the browser. These settings are kept in this browser.")}</p>
        </section>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sub(index: i64, lang: &str, forced: bool) -> SubTrack {
        SubTrack {
            index,
            language: Some(lang.into()),
            forced,
        }
    }

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
    fn subtitles_follow_the_mode() {
        let subs = [
            sub(3, "eng", true),
            sub(4, "fre", false),
            sub(5, "eng", false),
        ];
        // English film, French viewer: French subtitles.
        assert_eq!(pick_subtitle(&subs, Some("eng"), "auto", "fr"), Some(4));
        // French film: only forced French ones (none here).
        assert_eq!(pick_subtitle(&subs, Some("fre"), "auto", "fra"), None);
        // English viewer of an English film: the forced track.
        assert_eq!(pick_subtitle(&subs, Some("eng"), "auto", "en"), Some(3));
        assert_eq!(pick_subtitle(&subs, Some("eng"), "always", "en"), Some(5));
        assert_eq!(pick_subtitle(&subs, Some("eng"), "forced", "fr"), Some(3));
        assert_eq!(pick_subtitle(&subs, Some("eng"), "off", "fr"), None);
        // No French subtitles: the forced ones of the audio's language.
        assert_eq!(
            pick_subtitle(&subs[..1], Some("eng"), "auto", "fr"),
            Some(3)
        );
    }

    #[test]
    fn countdown_runs_on_playback_time() {
        assert_eq!(countdown_left(10, 5_000, 5_000), 10);
        assert_eq!(countdown_left(10, 5_000, 9_200), 6);
        assert_eq!(countdown_left(10, 5_000, 30_000), 0);
        assert_eq!(countdown_left(10, 5_000, 1_000), 10);
    }
}
