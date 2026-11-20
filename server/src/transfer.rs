// SPDX-License-Identifier: AGPL-3.0-or-later
//! Relay for moving sign-ins to a new device (see `keep_service::transfer`). The server only
//! passes an encrypted bundle from a signed-in device to the key a new device registered: it
//! cannot read it. Everything is in memory: pending transfers live ten minutes at most and the
//! bundle is dropped as soon as the new device picks it up.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use keep_service::transfer::{
    MAX_SEALED_HEX, TransferPeer, TransferSealed, TransferStart, TransferStarted,
};

const TTL: Duration = Duration::from_secs(600);
/// Pending transfers at once (codes are six digits: keep them sparse).
const MAX_PENDING: usize = 200;

struct Pending {
    handle: String,
    public_key: String,
    device: String,
    created: Instant,
    sealed: Option<String>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum TransferError {
    Invalid(&'static str),
    Busy,
    NotFound,
    AlreadySent,
}

pub enum Poll {
    Waiting,
    Ready(TransferSealed),
    Gone,
}

#[derive(Default)]
pub struct Transfers {
    by_code: Mutex<HashMap<String, Pending>>,
}

impl Transfers {
    fn purge(map: &mut HashMap<String, Pending>) {
        map.retain(|_, p| p.created.elapsed() < TTL);
    }

    pub fn start(&self, s: TransferStart) -> Result<TransferStarted, TransferError> {
        let key_ok =
            s.public_key.len() == 64 && s.public_key.bytes().all(|b| b.is_ascii_hexdigit());
        if !key_ok {
            return Err(TransferError::Invalid("public_key must be 32 bytes of hex"));
        }
        let device: String = s
            .device
            .chars()
            .filter(|c| !c.is_control())
            .take(80)
            .collect();
        let mut map = self.by_code.lock().unwrap_or_else(|e| e.into_inner());
        Self::purge(&mut map);
        if map.len() >= MAX_PENDING {
            return Err(TransferError::Busy);
        }
        let code = loop {
            let c = format!("{:06}", random_u64() % 1_000_000);
            if !map.contains_key(&c) {
                break c;
            }
        };
        let handle = format!(
            "{:016x}{:016x}{:016x}{:016x}",
            random_u64(),
            random_u64(),
            random_u64(),
            random_u64()
        );
        map.insert(
            code.clone(),
            Pending {
                handle: handle.clone(),
                public_key: s.public_key.to_ascii_lowercase(),
                device,
                created: Instant::now(),
                sealed: None,
            },
        );
        Ok(TransferStarted {
            code,
            handle,
            expires_in: TTL.as_secs(),
        })
    }

    /// A signed-in user looks the code up (only while nothing was sent yet).
    pub fn peer(&self, code: &str) -> Result<TransferPeer, TransferError> {
        let mut map = self.by_code.lock().unwrap_or_else(|e| e.into_inner());
        Self::purge(&mut map);
        match map.get(code) {
            Some(p) if p.sealed.is_none() => Ok(TransferPeer {
                public_key: p.public_key.clone(),
                device: p.device.clone(),
            }),
            Some(_) => Err(TransferError::AlreadySent),
            None => Err(TransferError::NotFound),
        }
    }

    pub fn send(&self, code: &str, sealed: TransferSealed) -> Result<(), TransferError> {
        if sealed.sealed.is_empty()
            || sealed.sealed.len() > MAX_SEALED_HEX
            || !sealed.sealed.bytes().all(|b| b.is_ascii_hexdigit())
        {
            return Err(TransferError::Invalid("sealed must be hex, at most 256 KB"));
        }
        let mut map = self.by_code.lock().unwrap_or_else(|e| e.into_inner());
        Self::purge(&mut map);
        let p = map.get_mut(code).ok_or(TransferError::NotFound)?;
        if p.sealed.is_some() {
            return Err(TransferError::AlreadySent);
        }
        p.sealed = Some(sealed.sealed);
        Ok(())
    }

    /// The new device waits for its bundle; picking it up ends the transfer.
    pub fn poll(&self, handle: &str) -> Poll {
        let mut map = self.by_code.lock().unwrap_or_else(|e| e.into_inner());
        Self::purge(&mut map);
        let Some(code) = map
            .iter()
            .find(|(_, p)| constant_time_eq(p.handle.as_bytes(), handle.as_bytes()))
            .map(|(c, _)| c.clone())
        else {
            return Poll::Gone;
        };
        if map[&code].sealed.is_none() {
            return Poll::Waiting;
        }
        let p = map.remove(&code).expect("present");
        Poll::Ready(TransferSealed {
            sealed: p.sealed.unwrap_or_default(),
        })
    }
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// OS-seeded randomness through std (see `seerr::random_password`): avoids a rand dependency.
fn random_u64() -> u64 {
    use std::hash::{BuildHasher, RandomState};
    RandomState::new().hash_one(Instant::now())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn start(t: &Transfers) -> TransferStarted {
        t.start(TransferStart {
            public_key: "ab".repeat(32),
            device: "Pixel\n8".into(),
        })
        .unwrap()
    }

    #[test]
    fn full_transfer_once() {
        let t = Transfers::default();
        let s = start(&t);
        assert_eq!(s.code.len(), 6);
        assert!(matches!(t.poll(&s.handle), Poll::Waiting));
        let peer = t.peer(&s.code).unwrap();
        assert_eq!(peer.device, "Pixel8", "control characters dropped");
        t.send(
            &s.code,
            TransferSealed {
                sealed: "00ff".into(),
            },
        )
        .unwrap();
        assert_eq!(
            t.send(
                &s.code,
                TransferSealed {
                    sealed: "00".into()
                }
            ),
            Err(TransferError::AlreadySent)
        );
        assert_eq!(t.peer(&s.code).err(), Some(TransferError::AlreadySent));
        assert!(matches!(t.poll("wrong"), Poll::Gone));
        match t.poll(&s.handle) {
            Poll::Ready(b) => assert_eq!(b.sealed, "00ff"),
            _ => panic!("ready expected"),
        }
        assert!(matches!(t.poll(&s.handle), Poll::Gone), "picked up once");
    }

    #[test]
    fn rejects_bad_input() {
        let t = Transfers::default();
        assert!(
            t.start(TransferStart {
                public_key: "zz".into(),
                device: "x".into()
            })
            .is_err()
        );
        let s = start(&t);
        assert_eq!(
            t.send(
                &s.code,
                TransferSealed {
                    sealed: "not hex".into()
                }
            ),
            Err(TransferError::Invalid("sealed must be hex, at most 256 KB"))
        );
        assert!(t.peer("000000").is_err());
    }
}
