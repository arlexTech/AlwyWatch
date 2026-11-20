// SPDX-License-Identifier: AGPL-3.0-or-later
//! What the player offers at a given position: "Skip intro", "Next episode", the credits
//! countdown. Pure, so every player (Android, desktop) behaves the same.

use media_api::{Segment, SegmentKind};

/// Time before the end when "Next episode" is offered if the server marked no credits.
const NEXT_EPISODE_BEFORE_END_MS: u64 = 40_000;
/// No "Skip" in the very last second of a segment: there would be nothing left to skip.
const SKIP_MIN_LEFT_MS: u64 = 1_000;

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct PlaybackPrompt {
    /// The skippable segment being played (seek to its end), if any. Credits are left out
    /// when there is a next episode: "Next episode" replaces "Skip credits".
    pub skip: Option<Segment>,
    /// Offer the next episode (there is one, and we are in the credits or near the end).
    pub next_episode: bool,
    /// In the credits with a next episode: the player may count down and start it.
    pub credits: bool,
}

/// What to offer at `position_ms`. `segments` as returned by `segments()`.
#[uniffi::export]
pub fn playback_prompt(
    segments: Vec<Segment>,
    position_ms: u64,
    duration_ms: Option<u64>,
    has_next: bool,
) -> PlaybackPrompt {
    let current = segments
        .into_iter()
        .find(|s| position_ms >= s.start_ms && position_ms + SKIP_MIN_LEFT_MS < s.end_ms);
    let in_credits = current
        .as_ref()
        .is_some_and(|s| s.kind == SegmentKind::Credits);
    let near_end = duration_ms
        .filter(|d| *d > position_ms)
        .is_some_and(|d| d - position_ms <= NEXT_EPISODE_BEFORE_END_MS);
    PlaybackPrompt {
        skip: current.filter(|s| !(has_next && s.kind == SegmentKind::Credits)),
        next_episode: has_next && (in_credits || near_end),
        credits: has_next && in_credits,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seg(kind: SegmentKind, start_s: u64, end_s: u64) -> Segment {
        Segment {
            kind,
            start_ms: start_s * 1000,
            end_ms: end_s * 1000,
        }
    }

    fn segs() -> Vec<Segment> {
        vec![
            seg(SegmentKind::Intro, 60, 120),
            seg(SegmentKind::Credits, 2500, 2600),
        ]
    }

    const DUR: Option<u64> = Some(2_640_000);

    #[test]
    fn intro_is_skippable_until_its_last_second() {
        let p = playback_prompt(segs(), 61_000, DUR, true);
        assert_eq!(p.skip.map(|s| s.kind), Some(SegmentKind::Intro));
        assert!(!p.next_episode && !p.credits);
        assert_eq!(playback_prompt(segs(), 119_500, DUR, true).skip, None);
        assert_eq!(playback_prompt(segs(), 30_000, DUR, true).skip, None);
    }

    #[test]
    fn credits_offer_the_next_episode_instead_of_skipping() {
        let p = playback_prompt(segs(), 2_510_000, DUR, true);
        assert_eq!(p.skip, None);
        assert!(p.next_episode && p.credits);
    }

    #[test]
    fn credits_of_a_last_episode_can_be_skipped() {
        let p = playback_prompt(segs(), 2_510_000, DUR, false);
        assert_eq!(p.skip.map(|s| s.kind), Some(SegmentKind::Credits));
        assert!(!p.next_episode && !p.credits);
    }

    #[test]
    fn without_markers_next_episode_comes_in_the_last_40_seconds() {
        assert!(!playback_prompt(vec![], 2_590_000, DUR, true).next_episode);
        let p = playback_prompt(vec![], 2_610_000, DUR, true);
        assert!(p.next_episode && !p.credits);
        assert!(!playback_prompt(vec![], 2_610_000, None, true).next_episode);
        assert!(!playback_prompt(vec![], 2_610_000, DUR, false).next_episode);
    }
}
