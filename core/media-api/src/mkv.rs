// SPDX-License-Identifier: AGPL-3.0-or-later
//! Whether a Matroska file lists its tracks after its first cluster (some releases put the
//! track list at the very end, behind tens of MB of fonts). Android's player (Media3) needs
//! the tracks before the first cluster: it waits for ever on such a file, with no error, so
//! the platform plays the server's conversion instead. Two small range reads, no download.

use crate::model::HttpRequest;

const ID_EBML: u32 = 0x1A45_DFA3;
const ID_SEGMENT: u32 = 0x1853_8067;
const ID_TRACKS: u32 = 0x1654_AE6B;
const ID_CLUSTER: u32 = 0x1F43_B675;

/// An EBML element header at the start of `b`: (id, data size or None when unknown, length).
fn header(b: &[u8]) -> Option<(u32, Option<u64>, usize)> {
    let first = *b.first()?;
    let id_len = first.leading_zeros() as usize + 1;
    if id_len > 4 || b.len() < id_len {
        return None;
    }
    let id = b[..id_len]
        .iter()
        .fold(0u32, |v, x| (v << 8) | u32::from(*x));
    let s = &b[id_len..];
    let lead = *s.first()?;
    let size_len = lead.leading_zeros() as usize + 1;
    if size_len > 8 || s.len() < size_len {
        return None;
    }
    let mask = if size_len == 8 { 0 } else { 0xFFu8 >> size_len };
    let mut size = u64::from(lead & mask);
    for x in &s[1..size_len] {
        size = (size << 8) | u64::from(*x);
    }
    let unknown = size == (1u64 << (7 * size_len)) - 1;
    Some((id, (!unknown).then_some(size), id_len + size_len))
}

/// What the top-level elements read so far say: tracks first, clusters first, or more bytes
/// needed at this file position.
#[derive(Debug, PartialEq, Eq)]
pub enum Layout {
    TracksFirst,
    ClustersFirst,
    NeedAt(u64),
    Unknown,
}

/// Walks the top-level elements of `bytes` (the file from position `at`), starting at file
/// position `pos` (0: the file's start).
pub fn walk(bytes: &[u8], at: u64, mut pos: u64) -> Layout {
    for _ in 0..64 {
        let Some(off) = pos.checked_sub(at).map(|o| o as usize) else {
            return Layout::NeedAt(pos);
        };
        let Some((id, size, len)) = bytes.get(off..).and_then(header) else {
            return if off + 12 > bytes.len() {
                Layout::NeedAt(pos)
            } else {
                Layout::Unknown
            };
        };
        match id {
            ID_TRACKS => return Layout::TracksFirst,
            ID_CLUSTER => return Layout::ClustersFirst,
            // Into the segment: its children are the top-level elements.
            ID_SEGMENT => pos += len as u64,
            _ if pos == 0 && id != ID_EBML => return Layout::Unknown,
            _ => match size {
                Some(s) => pos += len as u64 + s,
                None => return Layout::Unknown,
            },
        }
    }
    Layout::Unknown
}

/// True when the file at `req` (a Matroska original) has its tracks after its first cluster.
/// None when it can't tell (not Matroska, no range support, network): play it as usual.
pub async fn tracks_after_clusters(http: &net::reqwest::Client, req: &HttpRequest) -> Option<bool> {
    let (mut at, mut pos) = (0u64, 0u64);
    let mut bytes = Vec::new();
    for _ in 0..4 {
        match walk(&bytes, at, pos) {
            Layout::TracksFirst => return Some(false),
            Layout::ClustersFirst => return Some(true),
            Layout::Unknown => return None,
            Layout::NeedAt(p) => {
                let mut r = http
                    .get(&req.url)
                    .header("Range", format!("bytes={p}-{}", p + 65_535));
                for h in &req.headers {
                    r = r.header(&h.name, &h.value);
                }
                let resp = r.send().await.ok()?;
                if resp.status().as_u16() != 206 {
                    return None;
                }
                bytes = resp.bytes().await.ok()?.to_vec();
                (at, pos) = (p, p);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn el(id: u32, data: &[u8]) -> Vec<u8> {
        let mut v = id
            .to_be_bytes()
            .iter()
            .copied()
            .skip_while(|b| *b == 0)
            .collect::<Vec<_>>();
        // 8-byte size, like many muxers write.
        v.push(0x01);
        v.extend_from_slice(&(data.len() as u64).to_be_bytes()[1..]);
        v.extend_from_slice(data);
        v
    }

    fn file(children: &[Vec<u8>]) -> Vec<u8> {
        let mut f = el(ID_EBML, &[0x42, 0x86, 0x81, 0x01]);
        f.extend(el(ID_SEGMENT, &children.concat()));
        f
    }

    #[test]
    fn tracks_behind_the_fonts_and_clusters_are_found() {
        let info = el(0x1549_A966, &[0; 10]);
        let fonts = el(0x1941_A469, &[0; 5000]);
        let tracks = el(ID_TRACKS, &[0; 20]);
        let cluster = el(ID_CLUSTER, &[0; 30]);
        let bad = file(&[info.clone(), fonts.clone(), cluster.clone(), tracks.clone()]);
        let good = file(&[info, tracks, fonts, cluster]);
        assert_eq!(walk(&bad, 0, 0), Layout::ClustersFirst);
        assert_eq!(walk(&good, 0, 0), Layout::TracksFirst);
        // Read in pieces: asks for the bytes it lacks, then decides from there.
        let Layout::NeedAt(p) = walk(&bad[..100], 0, 0) else {
            panic!("needs more")
        };
        assert_eq!(walk(&bad[p as usize..], p, 0), Layout::NeedAt(0));
        assert_eq!(walk(&bad, 0, p), Layout::ClustersFirst);
        assert_eq!(walk(b"RIFF....AVI LIST", 0, 0), Layout::Unknown);
    }
}
