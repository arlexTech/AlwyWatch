// SPDX-License-Identifier: AGPL-3.0-or-later
//! The browser's passkey prompts (`navigator.credentials`), as JSON in and out: the server's
//! options carry base64url strings where the browser wants bytes, and the answer goes back the
//! same way (see server/src/passkey.rs).

use wasm_bindgen::prelude::*;

#[wasm_bindgen(inline_js = r#"
const u2b = (s) => Uint8Array.from(atob(s.replace(/-/g, '+').replace(/_/g, '/') + '==='.slice((s.length + 3) % 4)), (c) => c.charCodeAt(0));
const b2u = (b) => { let s = ''; new Uint8Array(b).forEach((c) => { s += String.fromCharCode(c); }); return btoa(s).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, ''); };
export function passkeys_supported() { return !!(window.PublicKeyCredential && navigator.credentials); }
export async function passkey_create(json) {
  const pk = JSON.parse(json);
  pk.challenge = u2b(pk.challenge);
  pk.user.id = u2b(pk.user.id);
  (pk.excludeCredentials || []).forEach((c) => { c.id = u2b(c.id); });
  const c = await navigator.credentials.create({ publicKey: pk });
  return JSON.stringify({ id: c.id, rawId: b2u(c.rawId), type: c.type, response: {
    clientDataJSON: b2u(c.response.clientDataJSON), attestationObject: b2u(c.response.attestationObject) } });
}
export async function passkey_get(json) {
  const pk = JSON.parse(json);
  pk.challenge = u2b(pk.challenge);
  (pk.allowCredentials || []).forEach((c) => { c.id = u2b(c.id); });
  const c = await navigator.credentials.get({ publicKey: pk });
  return JSON.stringify({ id: c.id, rawId: b2u(c.rawId), type: c.type, response: {
    clientDataJSON: b2u(c.response.clientDataJSON), authenticatorData: b2u(c.response.authenticatorData),
    signature: b2u(c.response.signature) } });
}
"#)]
extern "C" {
    fn passkeys_supported() -> bool;
    #[wasm_bindgen(catch)]
    async fn passkey_create(options: String) -> Result<JsValue, JsValue>;
    #[wasm_bindgen(catch)]
    async fn passkey_get(options: String) -> Result<JsValue, JsValue>;
}

pub fn supported() -> bool {
    passkeys_supported()
}

async fn ask(
    f: impl std::future::Future<Output = Result<JsValue, JsValue>>,
) -> Result<serde_json::Value, String> {
    match f.await {
        Ok(v) => serde_json::from_str(&v.as_string().unwrap_or_default())
            .map_err(|_| "The browser's answer was unreadable.".to_owned()),
        // Cancelled, timed out, or no passkey for this site.
        Err(_) => Err("No passkey was used.".to_owned()),
    }
}

/// `navigator.credentials.create` with the server's `options`.
pub async fn create(options: &serde_json::Value) -> Result<serde_json::Value, String> {
    ask(passkey_create(options.to_string())).await
}

/// `navigator.credentials.get` with the server's `options`.
pub async fn get(options: &serde_json::Value) -> Result<serde_json::Value, String> {
    ask(passkey_get(options.to_string())).await
}
