// SPDX-License-Identifier: AGPL-3.0-or-later
//! Passkeys (WebAuthn) for AlwyWatch accounts, checked with `ring` (already used by the server)
//! instead of a WebAuthn crate that would bring OpenSSL into the static image.
//!
//! Only what passkeys need: discoverable credentials, attestation "none" (the authenticator's
//! make is not checked, as with every consumer passkey site), ES256 / EdDSA / RS256 keys.
//! Registration: the browser's `navigator.credentials.create()` answer gives the credential id
//! and public key (in the authenticator data of the attestation object). Sign-in: the
//! signature over `authenticatorData || SHA-256(clientDataJSON)` is checked with that key,
//! after the challenge, origin, relying party and "user present" flag.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use ring::{digest, signature};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::accounts::random_hex;

pub const ES256: i64 = -7;
pub const EDDSA: i64 = -8;
pub const RS256: i64 = -257;

const FLOW_TTL: Duration = Duration::from_secs(300);
const MAX_FLOWS: usize = 500;
const FLAG_UP: u8 = 0x01;
const FLAG_AT: u8 = 0x40;

/// The relying party: this server's public address (`ALWYWATCH_PUBLIC_URL`).
#[derive(Debug, Clone)]
pub struct RelyingParty {
    /// "watch.example.org"
    pub id: String,
    /// "https://watch.example.org"
    pub origin: String,
}

impl RelyingParty {
    pub fn from_url(url: &str) -> Option<Self> {
        let origin = url.trim().trim_end_matches('/').to_owned();
        let rest = origin
            .strip_prefix("https://")
            .or_else(|| origin.strip_prefix("http://"))?;
        let host = rest.split(['/', ':']).next()?.to_ascii_lowercase();
        if host.is_empty() || rest.contains('/') {
            return None;
        }
        Some(Self { id: host, origin })
    }
}

#[derive(Debug, PartialEq)]
pub enum PasskeyError {
    /// Expired or unknown flow (start again).
    Expired,
    /// Something in the answer doesn't check out.
    Invalid(&'static str),
    UnknownCredential,
}

enum Flow {
    Register { account: String },
    Login,
}

type Pending = (Flow, Vec<u8>, Instant);

/// Pending challenges (in memory: five minutes at most).
#[derive(Default)]
pub struct Flows(Mutex<HashMap<String, Pending>>);

impl Flows {
    fn put(&self, flow: Flow) -> (String, Vec<u8>) {
        let id = random_hex(16);
        let challenge = hex_bytes(&random_hex(32));
        let mut m = self.0.lock().unwrap_or_else(|e| e.into_inner());
        m.retain(|_, (_, _, at)| at.elapsed() < FLOW_TTL);
        if m.len() >= MAX_FLOWS
            && let Some(oldest) = m
                .iter()
                .min_by_key(|(_, (_, _, at))| *at)
                .map(|(k, _)| k.clone())
        {
            m.remove(&oldest);
        }
        m.insert(id.clone(), (flow, challenge.clone(), Instant::now()));
        (id, challenge)
    }

    fn take(&self, id: &str) -> Option<(Flow, Vec<u8>)> {
        let mut m = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let (f, c, at) = m.remove(id)?;
        (at.elapsed() < FLOW_TTL).then_some((f, c))
    }

    /// Options for `navigator.credentials.create()` (`publicKey`), and the flow id.
    pub fn register_start(
        &self,
        rp: &RelyingParty,
        account: &str,
        user_name: &str,
        display: &str,
        exclude: &[String],
    ) -> Value {
        let (flow, challenge) = self.put(Flow::Register {
            account: account.to_owned(),
        });
        json!({
            "flow_id": flow,
            "options": {
                "rp": { "id": rp.id, "name": "AlwyWatch" },
                "user": { "id": b64url(account.as_bytes()), "name": user_name, "displayName": display },
                "challenge": b64url(&challenge),
                "pubKeyCredParams": [
                    { "type": "public-key", "alg": ES256 },
                    { "type": "public-key", "alg": EDDSA },
                    { "type": "public-key", "alg": RS256 }
                ],
                "timeout": 120000,
                "attestation": "none",
                "authenticatorSelection": { "residentKey": "required", "requireResidentKey": true, "userVerification": "preferred" },
                "excludeCredentials": exclude.iter().map(|id| json!({ "type": "public-key", "id": id })).collect::<Vec<_>>()
            }
        })
    }

    /// Options for `navigator.credentials.get()` (`publicKey`), and the flow id.
    pub fn login_start(&self, rp: &RelyingParty) -> Value {
        let (flow, challenge) = self.put(Flow::Login);
        json!({
            "flow_id": flow,
            "options": {
                "challenge": b64url(&challenge),
                "rpId": rp.id,
                "timeout": 120000,
                "userVerification": "preferred",
                "allowCredentials": []
            }
        })
    }

    /// Checks a registration answer: (account, the new key).
    pub fn register_finish(
        &self,
        rp: &RelyingParty,
        flow_id: &str,
        cred: &RegisterCredential,
    ) -> Result<(String, NewKey), PasskeyError> {
        let (flow, challenge) = self.take(flow_id).ok_or(PasskeyError::Expired)?;
        let Flow::Register { account } = flow else {
            return Err(PasskeyError::Expired);
        };
        let client = unb64(&cred.response.client_data_json)?;
        check_client(&client, "webauthn.create", &challenge, rp)?;
        let att = unb64(&cred.response.attestation_object)?;
        let (obj, _) = cbor::decode(&att).ok_or(PasskeyError::Invalid("attestation"))?;
        let auth = obj
            .get_text("authData")
            .and_then(cbor::Value::bytes)
            .ok_or(PasskeyError::Invalid("attestation"))?;
        let flags = check_auth_data(auth, rp)?;
        if flags & FLAG_AT == 0 || auth.len() < 55 {
            return Err(PasskeyError::Invalid("no credential in the answer"));
        }
        let sign_count = u32::from_be_bytes([auth[33], auth[34], auth[35], auth[36]]);
        let id_len = u16::from_be_bytes([auth[53], auth[54]]) as usize;
        let id = auth
            .get(55..55 + id_len)
            .ok_or(PasskeyError::Invalid("credential id"))?;
        let cred_id = b64url(id);
        if cred_id != cred.raw_id.trim_end_matches('=') {
            return Err(PasskeyError::Invalid("credential id"));
        }
        let (cose, _) =
            cbor::decode(&auth[55 + id_len..]).ok_or(PasskeyError::Invalid("public key"))?;
        let (alg, public_key) = cose_key(&cose)?;
        Ok((
            account,
            NewKey {
                cred_id,
                alg,
                public_key,
                sign_count,
            },
        ))
    }

    /// Checks a sign-in answer against the stored key of its credential (`lookup`): the
    /// credential id and its new signature counter.
    pub fn login_finish(
        &self,
        rp: &RelyingParty,
        flow_id: &str,
        cred: &LoginCredential,
        lookup: impl FnOnce(&str) -> Option<(i64, Vec<u8>, u32)>,
    ) -> Result<(String, u32), PasskeyError> {
        let (flow, challenge) = self.take(flow_id).ok_or(PasskeyError::Expired)?;
        if !matches!(flow, Flow::Login) {
            return Err(PasskeyError::Expired);
        }
        let cred_id = cred.raw_id.trim_end_matches('=').to_owned();
        let (alg, key, stored_count) = lookup(&cred_id).ok_or(PasskeyError::UnknownCredential)?;
        let client = unb64(&cred.response.client_data_json)?;
        check_client(&client, "webauthn.get", &challenge, rp)?;
        let auth = unb64(&cred.response.authenticator_data)?;
        check_auth_data(&auth, rp)?;
        let sig = unb64(&cred.response.signature)?;
        let mut signed = auth.clone();
        signed.extend_from_slice(digest::digest(&digest::SHA256, &client).as_ref());
        verify(alg, &key, &signed, &sig)?;
        let count = u32::from_be_bytes([auth[33], auth[34], auth[35], auth[36]]);
        // A counter that went back: a cloned authenticator (synced passkeys always send 0).
        if count != 0 && stored_count != 0 && count <= stored_count {
            return Err(PasskeyError::Invalid("this passkey's counter went back"));
        }
        Ok((cred_id, count))
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct NewKey {
    pub cred_id: String,
    pub alg: i64,
    pub public_key: Vec<u8>,
    pub sign_count: u32,
}

/// The browser's `PublicKeyCredential` from `create()`, fields base64url as sent by the apps.
#[derive(Debug, Deserialize)]
pub struct RegisterCredential {
    #[serde(rename = "rawId")]
    pub raw_id: String,
    pub response: RegisterResponse,
}

#[derive(Debug, Deserialize)]
pub struct RegisterResponse {
    #[serde(rename = "clientDataJSON")]
    pub client_data_json: String,
    #[serde(rename = "attestationObject")]
    pub attestation_object: String,
}

#[derive(Debug, Deserialize)]
pub struct LoginCredential {
    #[serde(rename = "rawId")]
    pub raw_id: String,
    pub response: LoginResponse,
}

#[derive(Debug, Deserialize)]
pub struct LoginResponse {
    #[serde(rename = "clientDataJSON")]
    pub client_data_json: String,
    #[serde(rename = "authenticatorData")]
    pub authenticator_data: String,
    pub signature: String,
}

fn check_client(
    client: &[u8],
    kind: &str,
    challenge: &[u8],
    rp: &RelyingParty,
) -> Result<(), PasskeyError> {
    #[derive(Deserialize)]
    struct ClientData {
        #[serde(rename = "type")]
        kind: String,
        challenge: String,
        origin: String,
    }
    let c: ClientData =
        serde_json::from_slice(client).map_err(|_| PasskeyError::Invalid("client data"))?;
    if c.kind != kind {
        return Err(PasskeyError::Invalid("client data type"));
    }
    if c.challenge.trim_end_matches('=') != b64url(challenge) {
        return Err(PasskeyError::Invalid("challenge"));
    }
    if c.origin.trim_end_matches('/') != rp.origin {
        return Err(PasskeyError::Invalid("origin"));
    }
    Ok(())
}

/// Relying party hash and "user present": the flags.
fn check_auth_data(auth: &[u8], rp: &RelyingParty) -> Result<u8, PasskeyError> {
    if auth.len() < 37 {
        return Err(PasskeyError::Invalid("authenticator data"));
    }
    if auth[..32] != *digest::digest(&digest::SHA256, rp.id.as_bytes()).as_ref() {
        return Err(PasskeyError::Invalid("relying party"));
    }
    let flags = auth[32];
    if flags & FLAG_UP == 0 {
        return Err(PasskeyError::Invalid("user not present"));
    }
    Ok(flags)
}

/// A COSE public key in the form `verify` takes: ES256 0x04‖x‖y, EdDSA x, RS256 len(n)‖n‖e.
fn cose_key(k: &cbor::Value) -> Result<(i64, Vec<u8>), PasskeyError> {
    let bad = PasskeyError::Invalid("public key");
    let alg = k.get_int(3).and_then(cbor::Value::int).ok_or(bad)?;
    let b = |label: i64| {
        k.get_int(label)
            .and_then(cbor::Value::bytes)
            .ok_or(PasskeyError::Invalid("public key"))
    };
    match alg {
        ES256 => {
            let (x, y) = (b(-2)?, b(-3)?);
            if x.len() != 32 || y.len() != 32 {
                return Err(PasskeyError::Invalid("public key"));
            }
            Ok((alg, [&[4u8][..], x, y].concat()))
        }
        EDDSA => {
            let x = b(-2)?;
            if x.len() != 32 {
                return Err(PasskeyError::Invalid("public key"));
            }
            Ok((alg, x.to_vec()))
        }
        RS256 => {
            let (n, e) = (b(-1)?, b(-2)?);
            let len = u32::try_from(n.len()).map_err(|_| PasskeyError::Invalid("public key"))?;
            Ok((alg, [&len.to_be_bytes()[..], n, e].concat()))
        }
        _ => Err(PasskeyError::Invalid("unsupported key type")),
    }
}

fn verify(alg: i64, key: &[u8], msg: &[u8], sig: &[u8]) -> Result<(), PasskeyError> {
    let ok = match alg {
        ES256 => signature::UnparsedPublicKey::new(&signature::ECDSA_P256_SHA256_ASN1, key)
            .verify(msg, sig)
            .is_ok(),
        EDDSA => signature::UnparsedPublicKey::new(&signature::ED25519, key)
            .verify(msg, sig)
            .is_ok(),
        RS256 if key.len() > 4 => {
            let n_len = u32::from_be_bytes([key[0], key[1], key[2], key[3]]) as usize;
            match (key.get(4..4 + n_len), key.get(4 + n_len..)) {
                (Some(n), Some(e)) => signature::RsaPublicKeyComponents { n, e }
                    .verify(&signature::RSA_PKCS1_2048_8192_SHA256, msg, sig)
                    .is_ok(),
                _ => false,
            }
        }
        _ => false,
    };
    if ok {
        Ok(())
    } else {
        Err(PasskeyError::Invalid("signature"))
    }
}

fn unb64(s: &str) -> Result<Vec<u8>, PasskeyError> {
    b64url_decode(s).ok_or(PasskeyError::Invalid("encoding"))
}

fn hex_bytes(h: &str) -> Vec<u8> {
    (0..h.len() / 2)
        .filter_map(|i| u8::from_str_radix(&h[2 * i..2 * i + 2], 16).ok())
        .collect()
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

/// Base64url without padding (WebAuthn's encoding).
pub fn b64url(b: &[u8]) -> String {
    let mut out = String::with_capacity(b.len().div_ceil(3) * 4);
    for chunk in b.chunks(3) {
        let n = (u32::from(chunk[0]) << 16)
            | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
            | u32::from(*chunk.get(2).unwrap_or(&0));
        for i in 0..=chunk.len() {
            out.push(B64[(n >> (18 - 6 * i) & 63) as usize] as char);
        }
    }
    out
}

/// Base64url or standard base64, padded or not.
pub fn b64url_decode(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let (mut acc, mut bits) = (0u32, 0u32);
    for c in s.bytes().filter(|c| *c != b'=') {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'-' | b'+' => 62,
            b'_' | b'/' => 63,
            _ => return None,
        };
        acc = (acc << 6) | u32::from(v);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Some(out)
}

/// The little CBOR WebAuthn uses (attestation object, COSE keys).
pub mod cbor {
    #[derive(Debug, Clone, PartialEq)]
    pub enum Value {
        Int(i64),
        Bytes(Vec<u8>),
        Text(String),
        Array(Vec<Value>),
        Map(Vec<(Value, Value)>),
        Simple(u8),
    }

    impl Value {
        pub fn get_text(&self, key: &str) -> Option<&Value> {
            match self {
                Value::Map(m) => m
                    .iter()
                    .find(|(k, _)| matches!(k, Value::Text(t) if t == key))
                    .map(|(_, v)| v),
                _ => None,
            }
        }
        pub fn get_int(&self, key: i64) -> Option<&Value> {
            match self {
                Value::Map(m) => m
                    .iter()
                    .find(|(k, _)| *k == Value::Int(key))
                    .map(|(_, v)| v),
                _ => None,
            }
        }
        pub fn bytes(&self) -> Option<&[u8]> {
            match self {
                Value::Bytes(b) => Some(b),
                _ => None,
            }
        }
        pub fn int(&self) -> Option<i64> {
            match self {
                Value::Int(i) => Some(*i),
                _ => None,
            }
        }
    }

    /// One value and how many bytes it took; None on bad or too deep input.
    pub fn decode(b: &[u8]) -> Option<(Value, usize)> {
        value(b, 0)
    }

    fn arg(b: &[u8], info: u8) -> Option<(u64, usize)> {
        Some(match info {
            0..=23 => (u64::from(info), 1),
            24 => (u64::from(*b.get(1)?), 2),
            25 => (
                u64::from(u16::from_be_bytes(b.get(1..3)?.try_into().ok()?)),
                3,
            ),
            26 => (
                u64::from(u32::from_be_bytes(b.get(1..5)?.try_into().ok()?)),
                5,
            ),
            27 => (u64::from_be_bytes(b.get(1..9)?.try_into().ok()?), 9),
            _ => return None,
        })
    }

    fn value(b: &[u8], depth: u32) -> Option<(Value, usize)> {
        if depth > 16 {
            return None;
        }
        let first = *b.first()?;
        let (major, info) = (first >> 5, first & 31);
        let (n, head) = arg(b, info)?;
        let len = usize::try_from(n).ok();
        match major {
            0 => Some((Value::Int(i64::try_from(n).ok()?), head)),
            1 => Some((Value::Int(-1 - i64::try_from(n).ok()?), head)),
            2 => {
                let end = head.checked_add(len?)?;
                Some((Value::Bytes(b.get(head..end)?.to_vec()), end))
            }
            3 => {
                let end = head.checked_add(len?)?;
                let t = std::str::from_utf8(b.get(head..end)?).ok()?;
                Some((Value::Text(t.to_owned()), end))
            }
            4 => {
                let mut at = head;
                let mut items = Vec::new();
                for _ in 0..len.filter(|l| *l <= 1024)? {
                    let (v, used) = value(b.get(at..)?, depth + 1)?;
                    items.push(v);
                    at += used;
                }
                Some((Value::Array(items), at))
            }
            5 => {
                let mut at = head;
                let mut items = Vec::new();
                for _ in 0..len.filter(|l| *l <= 1024)? {
                    let (k, used) = value(b.get(at..)?, depth + 1)?;
                    at += used;
                    let (v, used) = value(b.get(at..)?, depth + 1)?;
                    at += used;
                    items.push((k, v));
                }
                Some((Value::Map(items), at))
            }
            7 if info < 24 => Some((Value::Simple(info), 1)),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ring::rand::SystemRandom;
    use ring::signature::{ECDSA_P256_SHA256_ASN1_SIGNING, EcdsaKeyPair, KeyPair};

    fn rp() -> RelyingParty {
        RelyingParty::from_url("https://watch.example.org/").unwrap()
    }

    // A tiny CBOR writer for the tests.
    fn head(major: u8, n: usize, out: &mut Vec<u8>) {
        if n < 24 {
            out.push(major << 5 | n as u8);
        } else if n < 256 {
            out.extend([major << 5 | 24, n as u8]);
        } else {
            out.push(major << 5 | 25);
            out.extend((n as u16).to_be_bytes());
        }
    }
    fn int(i: i64, out: &mut Vec<u8>) {
        if i >= 0 {
            head(0, i as usize, out)
        } else {
            head(1, (-1 - i) as usize, out)
        }
    }
    fn bytes(b: &[u8], out: &mut Vec<u8>) {
        head(2, b.len(), out);
        out.extend(b);
    }
    fn text(t: &str, out: &mut Vec<u8>) {
        head(3, t.len(), out);
        out.extend(t.as_bytes());
    }

    struct Authenticator {
        key: EcdsaKeyPair,
        cred_id: Vec<u8>,
    }

    impl Authenticator {
        fn new() -> Self {
            let rng = SystemRandom::new();
            let pkcs8 =
                EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, &rng).unwrap();
            let key =
                EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, pkcs8.as_ref(), &rng)
                    .unwrap();
            Self {
                key,
                cred_id: vec![7; 20],
            }
        }

        fn auth_data(&self, rp_id: &str, flags: u8, count: u32, with_key: bool) -> Vec<u8> {
            let mut a = digest::digest(&digest::SHA256, rp_id.as_bytes())
                .as_ref()
                .to_vec();
            a.push(flags);
            a.extend(count.to_be_bytes());
            if with_key {
                a.extend([0u8; 16]);
                a.extend((self.cred_id.len() as u16).to_be_bytes());
                a.extend(&self.cred_id);
                let p = self.key.public_key().as_ref();
                let mut k = Vec::new();
                head(5, 5, &mut k);
                int(1, &mut k);
                int(2, &mut k);
                int(3, &mut k);
                int(ES256, &mut k);
                int(-1, &mut k);
                int(1, &mut k);
                int(-2, &mut k);
                bytes(&p[1..33], &mut k);
                int(-3, &mut k);
                bytes(&p[33..65], &mut k);
                a.extend(k);
            }
            a
        }

        fn create(&self, options: &Value, origin: &str) -> RegisterCredential {
            let challenge = options["options"]["challenge"].as_str().unwrap();
            let client = format!(
                r#"{{"type":"webauthn.create","challenge":"{challenge}","origin":"{origin}"}}"#
            );
            let mut att = Vec::new();
            head(5, 3, &mut att);
            text("fmt", &mut att);
            text("none", &mut att);
            text("attStmt", &mut att);
            head(5, 0, &mut att);
            text("authData", &mut att);
            bytes(
                &self.auth_data(&rp().id, FLAG_UP | FLAG_AT, 0, true),
                &mut att,
            );
            RegisterCredential {
                raw_id: b64url(&self.cred_id),
                response: RegisterResponse {
                    client_data_json: b64url(client.as_bytes()),
                    attestation_object: b64url(&att),
                },
            }
        }

        fn get(&self, options: &Value, count: u32) -> LoginCredential {
            let challenge = options["options"]["challenge"].as_str().unwrap();
            let client = format!(
                r#"{{"type":"webauthn.get","challenge":"{challenge}","origin":"https://watch.example.org"}}"#
            );
            let auth = self.auth_data(&rp().id, FLAG_UP, count, false);
            let mut signed = auth.clone();
            signed.extend(digest::digest(&digest::SHA256, client.as_bytes()).as_ref());
            let sig = self.key.sign(&SystemRandom::new(), &signed).unwrap();
            LoginCredential {
                raw_id: b64url(&self.cred_id),
                response: LoginResponse {
                    client_data_json: b64url(client.as_bytes()),
                    authenticator_data: b64url(&auth),
                    signature: b64url(sig.as_ref()),
                },
            }
        }
    }

    #[test]
    fn base64url_round_trips() {
        for n in 0..10 {
            let b: Vec<u8> = (0..n).map(|i: u8| i.wrapping_mul(37)).collect();
            assert_eq!(b64url_decode(&b64url(&b)).unwrap(), b);
        }
        assert_eq!(b64url(b"hi?"), "aGk_");
    }

    #[test]
    fn a_passkey_registers_then_signs_in() {
        let flows = Flows::default();
        let a = Authenticator::new();
        let opts = flows.register_start(&rp(), "acc1", "sam", "Sam", &[]);
        let flow = opts["flow_id"].as_str().unwrap().to_owned();
        let (account, key) = flows
            .register_finish(&rp(), &flow, &a.create(&opts, "https://watch.example.org"))
            .unwrap();
        assert_eq!(account, "acc1");
        assert_eq!(key.cred_id, b64url(&a.cred_id));
        assert_eq!(key.alg, ES256);
        // A flow is used once.
        assert_eq!(
            flows
                .register_finish(&rp(), &flow, &a.create(&opts, "https://watch.example.org"))
                .unwrap_err(),
            PasskeyError::Expired
        );

        let stored = (key.alg, key.public_key.clone(), 0u32);
        let opts = flows.login_start(&rp());
        let flow = opts["flow_id"].as_str().unwrap();
        let (id, count) = flows
            .login_finish(&rp(), flow, &a.get(&opts, 5), |id| {
                (id == key.cred_id).then(|| stored.clone())
            })
            .unwrap();
        assert_eq!((id, count), (key.cred_id.clone(), 5));

        // Signed for another challenge: refused.
        let other = flows.login_start(&rp());
        let opts = flows.login_start(&rp());
        let flow = opts["flow_id"].as_str().unwrap();
        assert_eq!(
            flows
                .login_finish(&rp(), flow, &a.get(&other, 6), |_| Some(stored.clone()))
                .unwrap_err(),
            PasskeyError::Invalid("challenge")
        );
        // A counter that went back: refused.
        let opts = flows.login_start(&rp());
        let flow = opts["flow_id"].as_str().unwrap();
        assert_eq!(
            flows
                .login_finish(&rp(), flow, &a.get(&opts, 3), |_| Some((
                    stored.0,
                    stored.1.clone(),
                    5
                )))
                .unwrap_err(),
            PasskeyError::Invalid("this passkey's counter went back")
        );
        // Another key: the signature fails.
        let b = Authenticator::new();
        let opts = flows.login_start(&rp());
        let flow = opts["flow_id"].as_str().unwrap();
        assert_eq!(
            flows
                .login_finish(&rp(), flow, &b.get(&opts, 9), |_| Some(stored.clone()))
                .unwrap_err(),
            PasskeyError::Invalid("signature")
        );
    }

    #[test]
    fn another_site_cannot_register() {
        let flows = Flows::default();
        let a = Authenticator::new();
        let opts = flows.register_start(&rp(), "acc1", "sam", "Sam", &[]);
        let flow = opts["flow_id"].as_str().unwrap();
        assert_eq!(
            flows
                .register_finish(&rp(), flow, &a.create(&opts, "https://evil.example"))
                .unwrap_err(),
            PasskeyError::Invalid("origin")
        );
    }

    #[test]
    fn relying_party_from_the_public_address() {
        let r = RelyingParty::from_url("https://watch.example.org").unwrap();
        assert_eq!(
            (r.id.as_str(), r.origin.as_str()),
            ("watch.example.org", "https://watch.example.org")
        );
        assert!(RelyingParty::from_url("watch.example.org").is_none());
        assert!(RelyingParty::from_url("https://x.org/path").is_none());
    }

    #[test]
    fn cbor_rejects_truncated_input() {
        assert!(cbor::decode(&[0x58, 10, 1, 2]).is_none());
        assert!(cbor::decode(&[0xa1]).is_none());
        assert_eq!(cbor::decode(&[0x20]).unwrap().0, cbor::Value::Int(-1));
    }
}
