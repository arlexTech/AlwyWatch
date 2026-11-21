// SPDX-License-Identifier: AGPL-3.0-or-later
//! Audio and subtitle choice, like Plex: one per movie or per show (its episodes follow), picked
//! on the title page or in the player; otherwise the languages and subtitle mode of the settings.
//!
//! A season or a show is usually packed by one person with the same naming and track order, so
//! besides the language we remember the track's title and its position among the tracks of that
//! language: "the second French track, titled VFF" finds the same track in every episode, and
//! a file laid out differently still gets the right language.
//!
//! Pure functions over a neutral track list, so every player (Media3, mpv) behaves the same.

use crate::app::AlwyWatch;
use crate::error::Result;
use media_api::{Item, ItemKind};

/// The remembered choice for a movie or a show.
#[derive(Debug, Clone, Default, PartialEq, Eq, uniffi::Record)]
pub struct TrackPref {
    /// Audio language (ISO 639-2/T, "fra"), its track title and position among that language.
    pub audio: Option<String>,
    pub audio_title: Option<String>,
    pub audio_nth: Option<u32>,
    /// None: the settings decide; "off"; or a language, with `forced` for signs-only tracks.
    pub subs: Option<String>,
    pub forced: bool,
    pub subs_title: Option<String>,
    pub subs_nth: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum TrackKind {
    Audio,
    Subtitle,
}

/// A playable track as the player sees it, in file order. `id` is the player's own handle
/// (an index into its list), returned as is.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct TrackInfo {
    pub id: u32,
    pub kind: TrackKind,
    pub language: Option<String>,
    pub title: Option<String>,
    pub forced: bool,
}

/// The settings that apply when the title has no choice of its own.
#[derive(Debug, Clone, Default, PartialEq, Eq, uniffi::Record)]
pub struct TrackRules {
    /// Preferred audio language, any code ("fr", "fre", "fra"); empty: the file's default.
    pub audio_language: String,
    /// "auto" (subtitles when the audio isn't in our language, else forced ones), "forced",
    /// "always" or "off".
    pub subtitle_mode: String,
    /// Our language for subtitles (the caller falls back to the device language).
    pub subtitle_language: String,
    /// The audio track the player picked by itself, used when no audio rule matches.
    pub default_audio: Option<u32>,
}

/// What to select: the audio track (None: keep the player's) and the subtitle track (None: off).
#[derive(Debug, Clone, Default, PartialEq, Eq, uniffi::Record)]
pub struct TrackChoice {
    pub audio: Option<u32>,
    pub subtitle: Option<u32>,
}

/// ISO 639-1 → 639-2/T, and bibliographic 639-2/B → T (what servers often send).
const CODES: &[(&str, &str)] = &[
    ("aa", "aar"),
    ("af", "afr"),
    ("am", "amh"),
    ("ar", "ara"),
    ("as", "asm"),
    ("az", "aze"),
    ("be", "bel"),
    ("bg", "bul"),
    ("bn", "ben"),
    ("bo", "bod"),
    ("bs", "bos"),
    ("ca", "cat"),
    ("cs", "ces"),
    ("cy", "cym"),
    ("da", "dan"),
    ("de", "deu"),
    ("el", "ell"),
    ("en", "eng"),
    ("eo", "epo"),
    ("es", "spa"),
    ("et", "est"),
    ("eu", "eus"),
    ("fa", "fas"),
    ("fi", "fin"),
    ("fo", "fao"),
    ("fr", "fra"),
    ("ga", "gle"),
    ("gd", "gla"),
    ("gl", "glg"),
    ("gu", "guj"),
    ("he", "heb"),
    ("hi", "hin"),
    ("hr", "hrv"),
    ("hu", "hun"),
    ("hy", "hye"),
    ("id", "ind"),
    ("is", "isl"),
    ("it", "ita"),
    ("ja", "jpn"),
    ("jv", "jav"),
    ("ka", "kat"),
    ("kk", "kaz"),
    ("km", "khm"),
    ("kn", "kan"),
    ("ko", "kor"),
    ("ku", "kur"),
    ("ky", "kir"),
    ("la", "lat"),
    ("lb", "ltz"),
    ("lo", "lao"),
    ("lt", "lit"),
    ("lv", "lav"),
    ("mk", "mkd"),
    ("ml", "mal"),
    ("mn", "mon"),
    ("mr", "mar"),
    ("ms", "msa"),
    ("mt", "mlt"),
    ("my", "mya"),
    ("nb", "nob"),
    ("ne", "nep"),
    ("nl", "nld"),
    ("nn", "nno"),
    ("no", "nor"),
    ("pa", "pan"),
    ("pl", "pol"),
    ("ps", "pus"),
    ("pt", "por"),
    ("ro", "ron"),
    ("ru", "rus"),
    ("si", "sin"),
    ("sk", "slk"),
    ("sl", "slv"),
    ("so", "som"),
    ("sq", "sqi"),
    ("sr", "srp"),
    ("sv", "swe"),
    ("sw", "swa"),
    ("ta", "tam"),
    ("te", "tel"),
    ("tg", "tgk"),
    ("th", "tha"),
    ("tl", "tgl"),
    ("tr", "tur"),
    ("uk", "ukr"),
    ("ur", "urd"),
    ("uz", "uzb"),
    ("vi", "vie"),
    ("wa", "wln"),
    ("yi", "yid"),
    ("zh", "zho"),
    ("zu", "zul"),
    ("alb", "sqi"),
    ("arm", "hye"),
    ("baq", "eus"),
    ("bur", "mya"),
    ("chi", "zho"),
    ("cze", "ces"),
    ("dut", "nld"),
    ("fre", "fra"),
    ("geo", "kat"),
    ("ger", "deu"),
    ("gre", "ell"),
    ("ice", "isl"),
    ("mac", "mkd"),
    ("mao", "mri"),
    ("may", "msa"),
    ("per", "fas"),
    ("rum", "ron"),
    ("slo", "slk"),
    ("tib", "bod"),
    ("wel", "cym"),
];

/// Any language code ("fr", "fre", "fra", "fr-FR", "pt_BR") → ISO 639-2/T ("fra"); None when
/// unknown ("und") or empty.
#[uniffi::export]
pub fn lang3(code: Option<String>) -> Option<String> {
    let c = code?.trim().to_ascii_lowercase();
    let c = c.split(['-', '_']).next().unwrap_or_default();
    if c.is_empty() || c == "und" || c == "unknown" || c == "mis" || c == "zxx" {
        return None;
    }
    Some(
        CODES
            .iter()
            .find(|(from, _)| *from == c)
            .map_or(c, |(_, to)| to)
            .to_owned(),
    )
}

fn same_title(a: Option<&str>, b: Option<&str>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => a.trim().eq_ignore_ascii_case(b.trim()),
        _ => false,
    }
}

/// Among `same` (one language and kind, file order): same title, else same position, else
/// the first.
fn like_remembered<'a>(
    same: &[&'a TrackInfo],
    title: Option<&str>,
    nth: Option<u32>,
) -> Option<&'a TrackInfo> {
    same.iter()
        .find(|t| same_title(t.title.as_deref(), title))
        .or_else(|| nth.and_then(|n| same.get(n as usize)))
        .or_else(|| same.first())
        .copied()
}

fn of_kind(tracks: &[TrackInfo], kind: TrackKind) -> impl Iterator<Item = &TrackInfo> {
    tracks.iter().filter(move |t| t.kind == kind)
}

fn audio_like<'a>(tracks: &'a [TrackInfo], lang: &str) -> Vec<&'a TrackInfo> {
    of_kind(tracks, TrackKind::Audio)
        .filter(|t| lang3(t.language.clone()).as_deref() == Some(lang))
        .collect()
}

fn subs_like<'a>(tracks: &'a [TrackInfo], lang: Option<&str>, forced: bool) -> Vec<&'a TrackInfo> {
    of_kind(tracks, TrackKind::Subtitle)
        .filter(|t| lang3(t.language.clone()).as_deref() == lang && t.forced == forced)
        .collect()
}

/// Picks audio and subtitles once the file's tracks are known: the title's choice, else the
/// settings.
#[uniffi::export]
pub fn choose_tracks(
    pref: Option<TrackPref>,
    rules: TrackRules,
    tracks: Vec<TrackInfo>,
) -> TrackChoice {
    let pref = pref.unwrap_or_default();
    let mut choice = TrackChoice::default();

    // Audio.
    let want = pref
        .audio
        .clone()
        .or_else(|| lang3(Some(rules.audio_language.clone())));
    let mut audio_lang = None;
    if let Some(want) = want {
        let same = audio_like(&tracks, &want);
        let hit = if pref.audio.is_some() {
            like_remembered(&same, pref.audio_title.as_deref(), pref.audio_nth)
        } else {
            same.first().copied()
        };
        if let Some(t) = hit {
            choice.audio = Some(t.id);
            audio_lang = Some(want);
        }
    }
    if audio_lang.is_none() {
        audio_lang = rules.default_audio.and_then(|id| {
            of_kind(&tracks, TrackKind::Audio)
                .find(|t| t.id == id)
                .and_then(|t| lang3(t.language.clone()))
        });
    }

    // Subtitles.
    let sub_lang = lang3(Some(rules.subtitle_language.clone()));
    let pick = |lang: Option<&str>, forced_only: bool| -> Option<u32> {
        let subs = || of_kind(&tracks, TrackKind::Subtitle);
        let lang_is = |t: &TrackInfo| lang3(t.language.clone()).as_deref() == lang;
        if forced_only {
            subs().find(|t| t.forced && (lang.is_none() || lang_is(t)))
        } else {
            subs()
                .find(|t| !t.forced && lang_is(t))
                .or_else(|| subs().find(|t| lang_is(t)))
        }
        .map(|t| t.id)
    };
    let audio_lang = audio_lang.as_deref();
    choice.subtitle = match (pref.subs.as_deref(), rules.subtitle_mode.as_str()) {
        (Some("off"), _) => None,
        (Some(lang), _) => {
            let same = subs_like(&tracks, Some(lang), pref.forced);
            like_remembered(&same, pref.subs_title.as_deref(), pref.subs_nth)
                .map(|t| t.id)
                .or_else(|| pick(Some(lang), pref.forced))
        }
        (None, "off") => None,
        (None, "always") => pick(sub_lang.as_deref(), false),
        (None, "forced") => pick(audio_lang, true),
        // Automatic: the audio is in another language than ours → subtitles in ours; else only
        // forced ones (signs, foreign dialogue).
        (None, _) => match (audio_lang, sub_lang.as_deref()) {
            (Some(a), Some(s)) if a != s => pick(Some(s), false).or_else(|| pick(Some(a), true)),
            _ => pick(audio_lang, true),
        },
    };
    choice
}

/// The track the title's choice points to in this file (for the title page), if any.
#[uniffi::export]
pub fn remembered_track(pref: TrackPref, kind: TrackKind, tracks: Vec<TrackInfo>) -> Option<u32> {
    match kind {
        TrackKind::Audio => {
            let lang = pref.audio.as_deref()?;
            like_remembered(
                &audio_like(&tracks, lang),
                pref.audio_title.as_deref(),
                pref.audio_nth,
            )
        }
        TrackKind::Subtitle => {
            let lang = pref.subs.as_deref().filter(|s| *s != "off")?;
            like_remembered(
                &subs_like(&tracks, Some(lang), pref.forced),
                pref.subs_title.as_deref(),
                pref.subs_nth,
            )
        }
    }
    .map(|t| t.id)
}

/// The choice after the viewer picked `picked` (None: subtitles off) among `tracks`.
#[uniffi::export]
pub fn pick_track(
    pref: Option<TrackPref>,
    kind: TrackKind,
    tracks: Vec<TrackInfo>,
    picked: Option<u32>,
) -> TrackPref {
    let old = pref.unwrap_or_default();
    let Some(t) = picked.and_then(|id| tracks.iter().find(|t| t.id == id && t.kind == kind)) else {
        return match kind {
            TrackKind::Audio => old,
            TrackKind::Subtitle => TrackPref {
                subs: Some("off".into()),
                forced: false,
                subs_title: None,
                subs_nth: None,
                ..old
            },
        };
    };
    let lang = lang3(t.language.clone());
    let nth = |same: Vec<&TrackInfo>| same.iter().position(|s| s.id == t.id).map(|p| p as u32);
    match kind {
        TrackKind::Audio => TrackPref {
            audio_nth: lang.as_deref().and_then(|l| nth(audio_like(&tracks, l))),
            audio: lang,
            audio_title: t.title.clone(),
            ..old
        },
        TrackKind::Subtitle => TrackPref {
            subs_nth: nth(subs_like(&tracks, lang.as_deref(), t.forced)),
            subs: Some(lang.unwrap_or_else(|| "off".into())),
            forced: t.forced,
            subs_title: t.title.clone(),
            ..old
        },
    }
}

/// Episodes share their show's choice.
#[uniffi::export]
pub fn track_title_key(item: Item) -> String {
    match item.kind {
        ItemKind::Episode => item.series_id.unwrap_or(item.id),
        _ => item.id,
    }
}

const SEP: char = '\u{1f}';

fn encode(p: &TrackPref) -> String {
    let s = |o: &Option<String>| o.clone().unwrap_or_default();
    let n = |o: Option<u32>| o.map(|n| n.to_string()).unwrap_or_default();
    // Field order kept from the first Android version (stored values stay readable).
    [
        s(&p.audio),
        s(&p.audio_title),
        s(&p.subs),
        if p.forced { "1".into() } else { String::new() },
        n(p.audio_nth),
        s(&p.subs_title),
        n(p.subs_nth),
    ]
    .join(&SEP.to_string())
}

fn decode(v: &str) -> Option<TrackPref> {
    let p: Vec<&str> = v.split(SEP).collect();
    if p.len() < 4 {
        return None;
    }
    let s = |i: usize| p.get(i).filter(|x| !x.is_empty()).map(|x| (*x).to_owned());
    let n = |i: usize| p.get(i).and_then(|x| x.parse().ok());
    Some(TrackPref {
        audio: s(0),
        audio_title: s(1),
        subs: s(2),
        forced: p[3] == "1",
        audio_nth: n(4),
        subs_title: s(5),
        subs_nth: n(6),
    })
}

#[uniffi::export]
impl AlwyWatch {
    /// The audio/subtitle choice of a movie or show (`title_key` from `track_title_key`).
    pub fn track_pref(&self, account_id: String, title_key: String) -> Result<Option<TrackPref>> {
        Ok(self
            .store()
            .setting(&format!("tracks.{account_id}.{title_key}"))?
            .as_deref()
            .and_then(decode))
    }

    pub fn set_track_pref(
        &self,
        account_id: String,
        title_key: String,
        pref: TrackPref,
    ) -> Result<()> {
        Ok(self
            .store()
            .set_setting(&format!("tracks.{account_id}.{title_key}"), &encode(&pref))?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(id: u32, kind: TrackKind, lang: &str, title: &str, forced: bool) -> TrackInfo {
        TrackInfo {
            id,
            kind,
            language: Some(lang.into()),
            title: (!title.is_empty()).then(|| title.into()),
            forced,
        }
    }
    use TrackKind::{Audio, Subtitle};

    /// An episode of a French pack: VFQ then VFF, English; French full, SDH and forced subs.
    fn episode(vff_title: &str) -> Vec<TrackInfo> {
        vec![
            t(0, Audio, "fre", "VFQ", false),
            t(1, Audio, "fre", vff_title, false),
            t(2, Audio, "eng", "VO", false),
            t(3, Subtitle, "fre", "Full", false),
            t(4, Subtitle, "fre", "SDH", false),
            t(5, Subtitle, "fre", "Forced", true),
            t(6, Subtitle, "eng", "", false),
        ]
    }

    fn rules() -> TrackRules {
        TrackRules {
            audio_language: String::new(),
            subtitle_mode: "auto".into(),
            subtitle_language: "fr".into(),
            default_audio: Some(0),
        }
    }

    #[test]
    fn codes_normalise() {
        assert_eq!(lang3(Some("fr".into())).as_deref(), Some("fra"));
        assert_eq!(lang3(Some("fre".into())).as_deref(), Some("fra"));
        assert_eq!(lang3(Some("fr-CA".into())).as_deref(), Some("fra"));
        assert_eq!(lang3(Some("pt_BR".into())).as_deref(), Some("por"));
        assert_eq!(lang3(Some("und".into())), None);
        assert_eq!(lang3(Some(" ".into())), None);
    }

    #[test]
    fn a_pick_follows_to_the_next_episode_by_title_then_position() {
        let ep1 = episode("VFF");
        let pref = pick_track(None, Audio, ep1.clone(), Some(1));
        let pref = pick_track(Some(pref), Subtitle, ep1, Some(4));
        assert_eq!(pref.audio.as_deref(), Some("fra"));
        assert_eq!(pref.audio_nth, Some(1));
        assert_eq!(pref.subs_nth, Some(1));

        // Same naming: same tracks.
        let c = choose_tracks(Some(pref.clone()), rules(), episode("VFF"));
        assert_eq!((c.audio, c.subtitle), (Some(1), Some(4)));
        // Renamed in this episode ("VFF 5.1"): the position still finds it.
        let c = choose_tracks(Some(pref.clone()), rules(), episode("VFF 5.1"));
        assert_eq!(c.audio, Some(1));
        // Only one French track here: the language decides.
        let single = vec![t(7, Audio, "fra", "", false), t(8, Audio, "eng", "", false)];
        assert_eq!(choose_tracks(Some(pref), rules(), single).audio, Some(7));
    }

    #[test]
    fn a_title_match_wins_over_the_position() {
        let pref = pick_track(None, Audio, episode("VFF"), Some(1));
        // Next file lists VFF first.
        let swapped = vec![
            t(0, Audio, "fra", "vff", false),
            t(1, Audio, "fra", "VFQ", false),
        ];
        assert_eq!(choose_tracks(Some(pref), rules(), swapped).audio, Some(0));
    }

    #[test]
    fn settings_apply_without_a_choice() {
        // French audio, French us: forced subtitles only.
        let c = choose_tracks(None, rules(), episode("VFF"));
        assert_eq!((c.audio, c.subtitle), (None, Some(5)));
        // English audio preferred: French full subtitles.
        let r = TrackRules {
            audio_language: "en".into(),
            ..rules()
        };
        let c = choose_tracks(None, r, episode("VFF"));
        assert_eq!((c.audio, c.subtitle), (Some(2), Some(3)));
        // Off.
        let r = TrackRules {
            subtitle_mode: "off".into(),
            ..rules()
        };
        assert_eq!(choose_tracks(None, r, episode("VFF")).subtitle, None);
    }

    #[test]
    fn off_and_title_page_lookups() {
        let ep = episode("VFF");
        let off = pick_track(None, Subtitle, ep.clone(), None);
        assert_eq!(off.subs.as_deref(), Some("off"));
        assert_eq!(
            choose_tracks(Some(off.clone()), rules(), ep.clone()).subtitle,
            None
        );
        assert_eq!(remembered_track(off, Subtitle, ep.clone()), None);
        let p = pick_track(None, Subtitle, ep.clone(), Some(5));
        assert!(p.forced);
        assert_eq!(remembered_track(p, Subtitle, ep), Some(5));
    }

    #[test]
    fn stored_values_round_trip_and_old_ones_read() {
        let p = TrackPref {
            audio: Some("fra".into()),
            audio_title: Some("VFF".into()),
            audio_nth: Some(1),
            subs: Some("eng".into()),
            forced: true,
            subs_title: None,
            subs_nth: Some(0),
        };
        assert_eq!(decode(&encode(&p)), Some(p));
        // First Android version: four fields.
        let old = decode("fra\u{1f}VFQ\u{1f}off\u{1f}").unwrap();
        assert_eq!(old.subs.as_deref(), Some("off"));
        assert_eq!(old.audio_nth, None);
    }
}
