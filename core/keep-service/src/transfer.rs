// SPDX-License-Identifier: AGPL-3.0-or-later
//! Moving a user's sign-ins to a new device through the AlwyWatch server, without the server
//! being able to read them (like Jellyfin's Quick Connect, but end to end):
//!
//! 1. The new device makes a one-time X25519 key pair and registers its public key
//!    (`POST /v1/transfer`); it gets a short code to show and a handle to poll with.
//! 2. On a signed-in device the user types the code (`GET /v1/transfer/{code}`, signed in),
//!    compares the check number shown on both screens (it is derived from the public key, so a
//!    server swapping keys would show a different one), and sends its accounts sealed to that
//!    key (`PUT /v1/transfer/{code}`).
//! 3. The new device picks the sealed bundle up (`GET /v1/transfer/poll/{handle}`) and opens it.
//!
//! Sealing: an ephemeral X25519 key agreement with the recipient's key, HKDF-SHA256 over both
//! public keys, ChaCha20-Poly1305 with that single-use key. Output: ephemeral public key || ciphertext.

use ring::{aead, agreement, digest, hkdf, rand};
use serde::{Deserialize, Serialize};

const INFO: &[u8] = b"alwywatch-transfer v1";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CryptoError {
    Rng,
    BadKey,
    Seal,
    Open,
}

impl std::error::Error for CryptoError {}

impl std::fmt::Display for CryptoError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Rng => "no randomness available",
            Self::BadKey => "invalid public key",
            Self::Seal => "could not encrypt",
            Self::Open => "could not decrypt (wrong key or corrupted data)",
        })
    }
}

/// The new device's one-time key (kept in memory until the bundle arrives).
pub struct Recipient {
    secret: agreement::EphemeralPrivateKey,
    public: Vec<u8>,
}

impl Recipient {
    pub fn new() -> Result<Self, CryptoError> {
        let rng = rand::SystemRandom::new();
        let secret = agreement::EphemeralPrivateKey::generate(&agreement::X25519, &rng)
            .map_err(|_| CryptoError::Rng)?;
        let public = secret
            .compute_public_key()
            .map_err(|_| CryptoError::Rng)?
            .as_ref()
            .to_vec();
        Ok(Self { secret, public })
    }

    pub fn public_key(&self) -> &[u8] {
        &self.public
    }

    /// Opens a bundle sealed to this key (consumes the key: one transfer per key).
    pub fn open(self, sealed: &[u8]) -> Result<Vec<u8>, CryptoError> {
        if sealed.len() < 32 + aead::CHACHA20_POLY1305.tag_len() {
            return Err(CryptoError::Open);
        }
        let (eph, ct) = sealed.split_at(32);
        let peer = agreement::UnparsedPublicKey::new(&agreement::X25519, eph);
        let key = agreement::agree_ephemeral(self.secret, &peer, |shared| {
            derive(shared, eph, &self.public)
        })
        .map_err(|_| CryptoError::Open)??;
        let mut buf = ct.to_vec();
        let plain = key
            .open_in_place(
                aead::Nonce::assume_unique_for_key([0; 12]),
                aead::Aad::empty(),
                &mut buf,
            )
            .map_err(|_| CryptoError::Open)?;
        Ok(plain.to_vec())
    }
}

/// Seals `plain` to a recipient's public key.
pub fn seal(recipient_public: &[u8], plain: &[u8]) -> Result<Vec<u8>, CryptoError> {
    if recipient_public.len() != 32 {
        return Err(CryptoError::BadKey);
    }
    let rng = rand::SystemRandom::new();
    let eph = agreement::EphemeralPrivateKey::generate(&agreement::X25519, &rng)
        .map_err(|_| CryptoError::Rng)?;
    let eph_pub = eph
        .compute_public_key()
        .map_err(|_| CryptoError::Rng)?
        .as_ref()
        .to_vec();
    let peer = agreement::UnparsedPublicKey::new(&agreement::X25519, recipient_public);
    let key = agreement::agree_ephemeral(eph, &peer, |shared| {
        derive(shared, &eph_pub, recipient_public)
    })
    .map_err(|_| CryptoError::BadKey)??;
    let mut buf = plain.to_vec();
    // The key is used once (fresh ephemeral key per bundle), so a fixed nonce is safe.
    key.seal_in_place_append_tag(
        aead::Nonce::assume_unique_for_key([0; 12]),
        aead::Aad::empty(),
        &mut buf,
    )
    .map_err(|_| CryptoError::Seal)?;
    let mut out = eph_pub;
    out.extend(buf);
    Ok(out)
}

fn derive(
    shared: &[u8],
    eph_pub: &[u8],
    recipient_pub: &[u8],
) -> Result<aead::LessSafeKey, CryptoError> {
    let mut salt_bytes = eph_pub.to_vec();
    salt_bytes.extend_from_slice(recipient_pub);
    let prk = hkdf::Salt::new(hkdf::HKDF_SHA256, &salt_bytes).extract(shared);
    let okm = prk
        .expand(&[INFO], &aead::CHACHA20_POLY1305)
        .map_err(|_| CryptoError::Seal)?;
    let mut key = [0u8; 32];
    okm.fill(&mut key).map_err(|_| CryptoError::Seal)?;
    let unbound =
        aead::UnboundKey::new(&aead::CHACHA20_POLY1305, &key).map_err(|_| CryptoError::Seal)?;
    Ok(aead::LessSafeKey::new(unbound))
}

/// Four digits shown on both devices: they match when the sender encrypts to the key the new
/// device made (and not to one the server put in its place).
pub fn check_number(public_key: &[u8]) -> String {
    let h = digest::digest(&digest::SHA256, public_key);
    let n = u16::from_be_bytes([h.as_ref()[0], h.as_ref()[1]]) % 10_000;
    format!("{n:04}")
}

pub fn to_hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

pub fn from_hex(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok())
        .collect()
}

// ----- Wire format ----------------------------------------------------------------------------

/// `POST /v1/transfer` (no sign-in: the new device has none yet).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransferStart {
    /// X25519 public key, hex.
    pub public_key: String,
    /// Shown to the approving user ("Pixel 8").
    pub device: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransferStarted {
    /// Six digits, typed on the approving device.
    pub code: String,
    /// Secret, for the new device to poll with.
    pub handle: String,
    /// Seconds the code stays valid.
    pub expires_in: u64,
}

/// `GET /v1/transfer/{code}` (signed in).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransferPeer {
    pub public_key: String,
    pub device: String,
}

/// `PUT /v1/transfer/{code}` body and `GET /v1/transfer/poll/{handle}` answer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransferSealed {
    /// Hex of ephemeral public key || ciphertext. Empty when the approving device sent no
    /// sign-ins (approved from the web or an account-only device).
    #[serde(default)]
    pub sealed: String,
    /// A session of the approver's AlwyWatch account for the new device (poll answer only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_token: Option<String>,
}

/// Largest sealed bundle accepted (accounts and settings are a few KB).
pub const MAX_SEALED_HEX: usize = 256 * 1024;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seal_open_roundtrip_and_tamper() {
        let r = Recipient::new().unwrap();
        let pk = r.public_key().to_vec();
        let sealed = seal(&pk, b"secret tokens").unwrap();
        assert_eq!(r.open(&sealed).unwrap(), b"secret tokens");

        // Another key cannot open it; a flipped bit is rejected.
        let other = Recipient::new().unwrap();
        assert_eq!(other.open(&sealed), Err(CryptoError::Open));
        let r2 = Recipient::new().unwrap();
        let mut bad = seal(r2.public_key(), b"x").unwrap();
        let last = bad.len() - 1;
        bad[last] ^= 1;
        assert_eq!(r2.open(&bad), Err(CryptoError::Open));
    }

    #[test]
    fn check_numbers_and_hex() {
        let r = Recipient::new().unwrap();
        let c = check_number(r.public_key());
        assert_eq!(c.len(), 4);
        assert_eq!(c, check_number(r.public_key()));
        assert_eq!(
            from_hex(&to_hex(&[0, 1, 254, 255])).unwrap(),
            vec![0, 1, 254, 255]
        );
        assert!(from_hex("zz").is_none());
    }
}
