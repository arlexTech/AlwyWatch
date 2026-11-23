// SPDX-License-Identifier: AGPL-3.0-or-later
//! The quality picker of the players: what plays now ("Original · 1080p · 12 Mbps") and the
//! sizes the server can convert a file to. Shared by every platform.

use crate::model::{MediaSource, Quality, StreamKind};
use i18n::tr;
use serde::{Deserialize, Serialize};

/// One line of the picker: None is the original file (or the server's own choice when the
/// device can't play it).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
pub struct QualityChoice {
    pub label: String,
    pub quality: Option<Quality>,
}

/// Conversions offered, from the largest: (height, bits per second).
const PRESETS: [(u32, u64); 6] = [
    (2160, 40_000_000),
    (1080, 20_000_000),
    (1080, 8_000_000),
    (720, 4_000_000),
    (480, 1_500_000),
    (360, 700_000),
];

/// The usual name of a picture size, from its width (a 2.39:1 film in 1080p is 1920×800).
fn class(width: Option<u32>, height: Option<u32>) -> Option<u32> {
    let by_width = width.map(|w| match w {
        3200.. => 2160,
        1800.. => 1080,
        1200.. => 720,
        700.. => 480,
        _ => 360,
    });
    by_width.or(height)
}

fn size_name(height: u32) -> String {
    if height >= 2000 {
        "4K".into()
    } else {
        format!("{height}p")
    }
}

fn mbps(bits: u64) -> String {
    let m = bits as f64 / 1_000_000.0;
    if m >= 10.0 {
        tr!("{n} Mbps", n = format!("{m:.0}"))
    } else {
        tr!("{n} Mbps", n = format!("{m:.1}"))
    }
}

/// A conversion size: "720p · 4.0 Mbps".
pub fn quality_name(q: &Quality) -> String {
    format!("{} · {}", size_name(q.max_height), mbps(q.max_bitrate))
}

fn video_class(source: &MediaSource) -> Option<u32> {
    let v = source
        .streams
        .iter()
        .find(|s| s.kind == StreamKind::Video)?;
    class(v.width, v.height)
}

/// What plays now: the original ("Original · 1080p · 12 Mbps"), a size picked in the menu
/// ("Converted · 720p · 4.0 Mbps"), or the server's own conversion.
pub fn quality_label(source: &MediaSource, converted: bool, quality: Option<Quality>) -> String {
    match (converted, quality) {
        (true, Some(q)) => tr!("Converted · {quality}", quality = quality_name(&q)),
        (true, None) => tr!("Converted by the server (this device can't play the original)"),
        (false, _) => {
            let mut parts = vec![tr!("Original")];
            parts.extend(video_class(source).map(size_name));
            parts.extend(source.bitrate.map(mbps));
            parts.join(" · ")
        }
    }
}

/// The picker: the original first, then every size smaller than it.
pub fn quality_choices(source: &MediaSource) -> Vec<QualityChoice> {
    let height = video_class(source);
    let mut out = vec![QualityChoice {
        label: quality_label(source, false, None),
        quality: None,
    }];
    out.extend(
        PRESETS
            .iter()
            .filter(|(h, b)| {
                height.is_none_or(|top| *h <= top) && source.bitrate.is_none_or(|top| *b < top)
            })
            .map(|&(max_height, max_bitrate)| {
                let q = Quality {
                    max_bitrate,
                    max_height,
                };
                QualityChoice {
                    label: quality_name(&q),
                    quality: Some(q),
                }
            }),
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{HttpRequest, MediaStream};

    fn source(width: u32, height: u32, bitrate: Option<u64>) -> MediaSource {
        let req = HttpRequest {
            url: String::new(),
            headers: vec![],
        };
        MediaSource {
            id: "s".into(),
            container: None,
            size: None,
            bitrate,
            duration_ms: None,
            file_name: None,
            streams: vec![MediaStream {
                index: 0,
                kind: StreamKind::Video,
                codec: "h264".into(),
                profile: None,
                language: None,
                title: None,
                display_title: None,
                channels: None,
                channel_layout: None,
                bit_depth: None,
                width: Some(width),
                height: Some(height),
                hdr: None,
                dv_profile: None,
                is_default: true,
                is_forced: false,
                is_external: false,
                external: None,
            }],
            direct: req.clone(),
            transcode: Some(req),
            supports_direct_play: true,
            default_audio_index: None,
            default_subtitle_index: None,
        }
    }

    #[test]
    fn a_wide_1080p_film_offers_the_smaller_sizes() {
        let s = source(1920, 800, Some(12_000_000));
        let labels: Vec<String> = quality_choices(&s).into_iter().map(|c| c.label).collect();
        assert_eq!(
            labels,
            [
                "Original · 1080p · 12 Mbps",
                "1080p · 8.0 Mbps",
                "720p · 4.0 Mbps",
                "480p · 1.5 Mbps",
                "360p · 0.7 Mbps"
            ]
        );
        let q = quality_choices(&s)[2].quality;
        assert_eq!(quality_label(&s, true, q), "Converted · 720p · 4.0 Mbps");
    }
}
