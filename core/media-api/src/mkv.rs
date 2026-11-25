// SPDX-License-Identifier: AGPL-3.0-or-later
//! Whether a Matroska file lists its tracks after its first cluster (some releases put the
//! track list at the very end, behind the whole picture). Android's player (Media3) needs the
//! tracks before the first cluster: it waits for ever on such a file, with no error. Usually
//! the list fits in the padding before the picture, and the player reads the original with
//! it moved there ([`fix`]); else the server's conversion plays. A few small range reads.

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

const ID_VOID: u32 = 0xEC;
const ID_INFO: u32 = 0x1549_A966;
const ID_CHAPTERS: u32 = 0x1043_A770;
const ID_TAGS: u32 = 0x1254_C367;
const ID_SEEKHEAD: u32 = 0x114D_9B74;
const ID_SEEK: u32 = 0x4DBB;
const ID_SEEK_ID: u32 = 0x53AB;
const ID_SEEK_POSITION: u32 = 0x53AC;

/// Largest track list (and padding region) worth moving; beyond it, the conversion.
const MAX_MOVE: u64 = 4 << 20;

/// Bytes to show the player in place of the file's own, from `offset` on (same length).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
pub struct BytePatch {
    pub offset: u64,
    pub bytes: Vec<u8>,
}

/// How a Matroska original can play on a player that needs its tracks first.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Fix {
    /// The tracks come first: plays as is.
    AsIs,
    /// The tracks come last, and fit in the padding before the picture: the player reads the
    /// file with these bytes in place (the track list moved there, its old place blanked).
    /// Nothing shifts, so the file's own index and seek table stay right.
    Patch(Vec<BytePatch>),
    /// The tracks come last and can't be moved: the server's conversion.
    Convert,
}

/// A Void element of exactly `n` bytes (0, or 2 and more).
fn void(n: usize) -> Option<Vec<u8>> {
    match n {
        0 => Some(Vec::new()),
        1 => None,
        2..=128 => {
            let mut v = vec![0u8; n];
            v[0] = ID_VOID as u8;
            v[1] = 0x80 | (n - 2) as u8;
            Some(v)
        }
        _ => {
            let mut v = vec![0u8; n];
            v[..9].copy_from_slice(&void_header(n as u64));
            Some(v)
        }
    }
}

/// The header of a Void element `total` bytes long (at least 9), 8-byte size.
fn void_header(total: u64) -> [u8; 9] {
    let mut h = [0u8; 9];
    h[0] = ID_VOID as u8;
    h[1] = 0x01;
    h[2..].copy_from_slice(&(total - 9).to_be_bytes()[1..]);
    h
}

/// The file, over HTTP ranges (or in memory, for tests).
enum Source<'a> {
    Http(&'a net::reqwest::Client, &'a HttpRequest),
    #[cfg(test)]
    Mem(&'a [u8]),
}

impl Source<'_> {
    async fn read(&self, pos: u64, len: u64) -> Option<Vec<u8>> {
        match self {
            Source::Http(http, req) => {
                let mut r = http
                    .get(&req.url)
                    .header("Range", format!("bytes={pos}-{}", pos + len - 1));
                for h in &req.headers {
                    r = r.header(&h.name, &h.value);
                }
                let resp = r.send().await.ok()?;
                if resp.status().as_u16() != 206 {
                    return None;
                }
                Some(resp.bytes().await.ok()?.to_vec())
            }
            #[cfg(test)]
            Source::Mem(b) => {
                let start = (pos as usize).min(b.len());
                let end = ((pos + len) as usize).min(b.len());
                Some(b[start..end].to_vec())
            }
        }
    }
}

/// Reads windows of the file, keeping the last one.
struct Window<'a> {
    src: Source<'a>,
    at: u64,
    bytes: Vec<u8>,
    reads: u32,
}

impl Window<'_> {
    /// The element header at `pos`: (id, size, header length).
    async fn header(&mut self, pos: u64) -> Option<(u32, u64, u64)> {
        if !(pos >= self.at && pos + 12 <= self.at + self.bytes.len() as u64) {
            self.reads += 1;
            if self.reads > 12 {
                return None;
            }
            self.bytes = self.src.read(pos, 65_536).await?;
            self.at = pos;
        }
        let (id, size, len) = header(&self.bytes[(pos - self.at) as usize..])?;
        Some((id, size?, len as u64))
    }

    /// `len` bytes at `pos`.
    async fn bytes(&mut self, pos: u64, len: u64) -> Option<Vec<u8>> {
        if pos >= self.at && pos + len <= self.at + self.bytes.len() as u64 {
            let o = (pos - self.at) as usize;
            return Some(self.bytes[o..o + len as usize].to_vec());
        }
        let b = self.src.read(pos, len).await?;
        (b.len() as u64 == len).then_some(b)
    }
}

/// The children of an EBML master element's data: (id, data).
fn children(mut b: &[u8]) -> Vec<(u32, &[u8])> {
    let mut out = Vec::new();
    while let Some((id, Some(size), len)) = header(b) {
        let end = len + size as usize;
        let Some(data) = b.get(len..end) else { break };
        out.push((id, data));
        b = &b[end..];
    }
    out
}

/// How the Matroska file at `req` plays on a player that needs its tracks first. None when
/// it can't tell (not Matroska, no range support, network): play it as usual.
pub async fn fix(http: &net::reqwest::Client, req: &HttpRequest) -> Option<Fix> {
    plan(Source::Http(http, req)).await
}

async fn plan(src: Source<'_>) -> Option<Fix> {
    let mut w = Window {
        src,
        at: 0,
        bytes: Vec::new(),
        reads: 0,
    };
    let (id, size, len) = w.header(0).await?;
    if id != ID_EBML {
        return None;
    }
    let (id, _, len2) = w.header(len + size).await?;
    if id != ID_SEGMENT {
        return None;
    }
    let seg = len + size + len2;
    // The segment's top-level elements up to the first cluster: (id, position, length).
    let mut top = Vec::new();
    let mut pos = seg;
    loop {
        let (id, size, len) = w.header(pos).await?;
        match id {
            ID_TRACKS => return Some(Fix::AsIs),
            ID_CLUSTER => break,
            _ => top.push((id, pos, len + size)),
        }
        pos += len + size;
    }
    // The track list, through the seek head.
    let mut tracks_at = None;
    for &(id, p, l) in &top {
        if id != ID_SEEKHEAD || l > 65_536 {
            continue;
        }
        let b = w.bytes(p, l).await?;
        let (_, _, hl) = header(&b)?;
        for (cid, seek) in children(&b[hl..]) {
            if cid != ID_SEEK {
                continue;
            }
            let fields = children(seek);
            let sid = fields.iter().find(|f| f.0 == ID_SEEK_ID).map(|f| f.1);
            let spos = fields.iter().find(|f| f.0 == ID_SEEK_POSITION).map(|f| f.1);
            if let (Some(sid), Some(spos)) = (sid, spos)
                && sid == ID_TRACKS.to_be_bytes()
            {
                tracks_at = Some(seg + spos.iter().fold(0u64, |v, x| (v << 8) | u64::from(*x)));
            }
        }
    }
    let Some(tracks_at) = tracks_at else {
        return Some(Fix::Convert);
    };
    let (id, size, len) = w.header(tracks_at).await?;
    let tracks_len = len + size;
    if id != ID_TRACKS || tracks_len > MAX_MOVE {
        return Some(Fix::Convert);
    }
    let tracks = w.bytes(tracks_at, tracks_len).await?;
    // A run of padding and small movable elements before the picture, long enough to take
    // the movable ones and the track list: rewritten in place, padding last.
    let movable = |id| matches!(id, ID_VOID | ID_INFO | ID_CHAPTERS | ID_TAGS);
    let mut i = 0;
    while i < top.len() {
        if !movable(top[i].0) {
            i += 1;
            continue;
        }
        let mut j = i;
        while j < top.len() && movable(top[j].0) {
            j += 1;
        }
        let run = &top[i..j];
        let start = run[0].1;
        let total: u64 = run.iter().map(|e| e.2).sum();
        let kept: u64 = run.iter().filter(|e| e.0 != ID_VOID).map(|e| e.2).sum();
        if let Some(rest) = total.checked_sub(kept + tracks_len)
            && total <= MAX_MOVE
            && let Some(filler) = void(rest as usize)
        {
            let old = w.bytes(start, total).await?;
            let mut new = Vec::with_capacity(total as usize);
            for e in run.iter().filter(|e| e.0 != ID_VOID) {
                let o = (e.1 - start) as usize;
                new.extend_from_slice(&old[o..o + e.2 as usize]);
            }
            new.extend_from_slice(&tracks);
            new.extend_from_slice(&filler);
            return Some(Fix::Patch(vec![
                BytePatch {
                    offset: start,
                    bytes: new,
                },
                // The old place: padding now (one track list only).
                BytePatch {
                    offset: tracks_at,
                    bytes: void_header(tracks_len).to_vec(),
                },
            ]));
        }
        i = j;
    }
    Some(Fix::Convert)
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

    fn seek(id: u32, pos: u64) -> Vec<u8> {
        let mut d = el(ID_SEEK_ID, &id.to_be_bytes());
        d.extend(el(ID_SEEK_POSITION, &pos.to_be_bytes()));
        el(ID_SEEK, &d)
    }

    fn apply(file: &[u8], fix: &Fix) -> Vec<u8> {
        let mut f = file.to_vec();
        let Fix::Patch(patches) = fix else {
            panic!("not a patch: {fix:?}")
        };
        for p in patches {
            let o = p.offset as usize;
            f[o..o + p.bytes.len()].copy_from_slice(&p.bytes);
        }
        f
    }

    fn block_on<F: std::future::Future>(f: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(f)
    }

    /// The release that prompted this: the track list at the end, its old slot left as
    /// padding a few bytes too small, info between two paddings.
    #[test]
    fn tracks_at_the_end_move_into_the_padding() {
        let info = el(ID_INFO, &[7; 10]);
        let fonts = el(0x1941_A469, &[0; 300]);
        let tracks = el(ID_TRACKS, &[9; 60]);
        let cluster = el(ID_CLUSTER, &[1; 30]);
        let cues = el(0x1C53_BB6B, &[2; 8]);
        // Positions relative to the segment's data; the seek head is first and fixed size.
        let build = |pad: usize| {
            let v1 = void(20).unwrap();
            let v2 = void(pad).unwrap();
            let head_len = el(
                ID_SEEKHEAD,
                &[seek(ID_TRACKS, 0), seek(0x1C53_BB6B, 0)].concat(),
            )
            .len();
            let before = head_len + v1.len() + info.len() + v2.len() + fonts.len() + cluster.len();
            let head = el(
                ID_SEEKHEAD,
                &[
                    seek(ID_TRACKS, (before + cues.len()) as u64),
                    seek(0x1C53_BB6B, before as u64),
                ]
                .concat(),
            );
            file(&[
                head,
                v1,
                info.clone(),
                v2,
                fonts.clone(),
                cluster.clone(),
                cues.clone(),
                tracks.clone(),
            ])
        };
        // Padding 6 bytes short of the track list: the run (void, info, void) takes it.
        let bad = build(tracks.len() - 6);
        assert_eq!(walk(&bad, 0, 0), Layout::ClustersFirst);
        let fix = block_on(plan(Source::Mem(&bad))).unwrap();
        let fixed = apply(&bad, &fix);
        assert_eq!(fixed.len(), bad.len());
        assert_eq!(walk(&fixed, 0, 0), Layout::TracksFirst);
        // The track list and info are there in full, the picture and index untouched.
        let at = |hay: &[u8], needle: &[u8]| hay.windows(needle.len()).position(|w| w == needle);
        assert!(at(&fixed, &tracks).unwrap() < at(&fixed, &cluster).unwrap());
        assert!(at(&fixed, &info).is_some());
        let tail = bad.len() - tracks.len();
        let k = at(&bad, &fonts).unwrap();
        assert_eq!(fixed[k..tail], bad[k..tail]);
        // The old track list is padding now: a single track list in the file.
        assert_eq!(fixed[tail], ID_VOID as u8);
        assert_eq!(
            fixed
                .windows(4)
                .filter(|w| *w == ID_TRACKS.to_be_bytes())
                .count(),
            2
        ); // seek id + element
        // Already in order: as is. Too little room: the conversion.
        assert_eq!(block_on(plan(Source::Mem(&fixed))), Some(Fix::AsIs));
        let tight = build(2);
        assert_eq!(block_on(plan(Source::Mem(&tight))), Some(Fix::Convert));
        assert_eq!(block_on(plan(Source::Mem(b"RIFF....AVI LIST"))), None);
    }
}
