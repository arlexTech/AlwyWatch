// SPDX-License-Identifier: AGPL-3.0-or-later
//! AlwyWatch accounts on the apps (docs/keep-protocol.md "AlwyWatch accounts"): signing in to
//! the AlwyWatch server without a media server (password, or Quick Connect approved from the
//! web or another device), the servers list kept in the synced settings, and QR codes for the
//! Quick Connect screen. The session token is the platform's to keep in secure storage, like
//! media tokens; the core keeps the rest.

use keep_service::KeepClient;
use keep_service::account::{self, SERVERS_SETTING, SavedServer};
use serde::{Deserialize, Serialize};

use crate::app::{AlwyWatch, Core};
use crate::error::{CoreError, Result};

const ACCOUNT_KEY: &str = "account.session";

/// The AlwyWatch account this device is signed in to (no secret: the token is the platform's).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, uniffi::Record)]
pub struct AlwyAccount {
    pub base_url: String,
    pub account_id: String,
    pub name: String,
    pub username: Option<String>,
    /// The person's key on the AlwyWatch server (rooms, profiles).
    pub user_key: String,
}

/// A sign-in: the platform stores `token` and passes it to `account_activate` at start.
#[derive(Debug, Clone, uniffi::Record)]
pub struct AccountSignedIn {
    pub account: AlwyAccount,
    pub token: String,
}

/// A server from the user's synced list (sign in to it on this device).
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct KnownServer {
    /// "jellyfin" or "plex".
    pub kind: String,
    pub url: String,
    pub name: String,
}

/// A QR code: `size`×`size` modules, row by row, true = dark. No quiet zone included.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct QrCode {
    pub size: u32,
    pub modules: Vec<bool>,
}

/// User-facing message for a failed account call.
fn explain(e: net::Error, signing_in: bool) -> CoreError {
    match e {
        net::Error::Network { .. } => e.into(),
        e => CoreError::Invalid {
            detail: account::explain(&e, signing_in),
        },
    }
}

impl Core {
    pub(crate) fn account_client(&self) -> Option<KeepClient> {
        self.account
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Who acts for me on the AlwyWatch server: the media sign-in linked to it, else the
    /// account session.
    pub(crate) fn person_client(&self) -> Result<KeepClient> {
        self.keep().or_else(|e| self.account_client().ok_or(e))
    }

    fn save_account(
        &self,
        base: &str,
        token: &str,
        v: &account::AccountView,
    ) -> Result<AlwyAccount> {
        let a = AlwyAccount {
            base_url: base.to_owned(),
            account_id: v.id.clone(),
            name: v.name.clone(),
            username: v.username.clone(),
            user_key: v.user_key.clone(),
        };
        self.store
            .set_setting(ACCOUNT_KEY, &serde_json::to_string(&a).unwrap_or_default())?;
        *self.account.write().unwrap_or_else(|e| e.into_inner()) =
            Some(KeepClient::account(self.http.clone(), base, token));
        Ok(a)
    }

    /// Adds this device's servers to the synced list (so the account's other devices offer
    /// them). True when the list changed.
    pub(crate) fn remember_servers(&self) -> Result<bool> {
        let mut list: Vec<SavedServer> = self
            .store
            .setting(SERVERS_SETTING)?
            .and_then(|v| serde_json::from_str(&v).ok())
            .unwrap_or_default();
        let before = list.len();
        for a in self.store.accounts::<crate::app::Account>()? {
            let kind = match a.kind {
                media_api::ServerKind::Jellyfin => "jellyfin",
                media_api::ServerKind::Plex => "plex",
            };
            if !list.iter().any(|s| s.kind == kind && s.url == a.base_url) {
                list.push(SavedServer {
                    kind: kind.into(),
                    url: a.base_url.clone(),
                    name: a.server_name.clone(),
                });
            }
        }
        if list.len() == before {
            return Ok(false);
        }
        self.store.set_setting(
            SERVERS_SETTING,
            &serde_json::to_string(&list).unwrap_or_default(),
        )?;
        Ok(true)
    }
}

/// What an AlwyWatch server offers (`account_options`).
#[derive(Debug, Clone, uniffi::Record)]
pub struct AccountOptions {
    /// Who may create an account: "invite", "open" or "closed".
    pub registration: String,
    pub passkeys: bool,
    pub pocketid: bool,
}

/// An invitation to create an account (`account_invite`).
#[derive(Debug, Clone, uniffi::Record)]
pub struct AccountInvite {
    pub code: String,
    /// The web page that signs up with it, when the server knows its public address.
    pub url: Option<String>,
    pub expires_in_days: i64,
    pub uses: i64,
}

#[uniffi::export]
impl AlwyWatch {
    /// Signs in to an AlwyWatch account with username and password.
    pub async fn account_login(
        &self,
        url: String,
        username: String,
        password: String,
        device: String,
    ) -> Result<AccountSignedIn> {
        self.run(|c| async move {
            let base = net::normalize_base_url(&url)?;
            let s = KeepClient::account_login(
                &c.http,
                &base,
                &account::LoginRequest {
                    username: username.trim().to_owned(),
                    password,
                    device,
                },
            )
            .await
            .map_err(|e| explain(e, true))?;
            let account = c.save_account(&base, &s.token, &s.account)?;
            Ok(AccountSignedIn {
                account,
                token: s.token,
            })
        })
        .await
    }

    /// Creates an AlwyWatch account (with an invitation code when the server asks for one) and
    /// signs this device in to it.
    pub async fn account_register(
        &self,
        url: String,
        username: String,
        password: String,
        invite: Option<String>,
        device: String,
    ) -> Result<AccountSignedIn> {
        self.run(|c| async move {
            let base = net::normalize_base_url(&url)?;
            let s = KeepClient::account_register(
                &c.http,
                &base,
                &account::RegisterRequest {
                    username: username.trim().to_owned(),
                    password,
                    invite: invite
                        .map(|i| i.trim().to_owned())
                        .filter(|i| !i.is_empty()),
                    device,
                },
            )
            .await
            .map_err(|e| explain(e, true))?;
            let account = c.save_account(&base, &s.token, &s.account)?;
            Ok(AccountSignedIn {
                account,
                token: s.token,
            })
        })
        .await
    }

    /// What an AlwyWatch server offers for sign-in and sign-up (no sign-in needed).
    pub async fn account_options(&self, url: String) -> Result<AccountOptions> {
        self.run(|c| async move {
            let base = net::normalize_base_url(&url)?;
            let o = KeepClient::account_options(&c.http, &base)
                .await
                .map_err(|e| explain(e, false))?;
            Ok(AccountOptions {
                registration: o.registration,
                passkeys: o.passkeys,
                pocketid: o.pocketid,
            })
        })
        .await
    }

    /// Admins: an invitation to create an account, valid `days` days for `uses` sign-ups.
    pub async fn account_invite(&self, days: i64, uses: i64) -> Result<AccountInvite> {
        // The AlwyWatch account's session, else the Jellyfin/Plex login used for AlwyWatch (the
        // server accepts any admin).
        let account = self.core.account_client();
        self.run(|c| async move {
            let client = match account {
                Some(a) => a,
                None => c.keep()?,
            };
            let i = client
                .account_invite(days, uses)
                .await
                .map_err(|e| explain(e, false))?;
            Ok(AccountInvite {
                code: i.code,
                url: i.url,
                expires_in_days: i.expires_in_days,
                uses: i.uses,
            })
        })
        .await
    }

    /// After Quick Connect gave this device an account token (`TransferReceived`): signs in
    /// with it. The platform stores the token.
    pub async fn account_use_token(&self, url: String, token: String) -> Result<AlwyAccount> {
        self.run(|c| async move {
            let base = net::normalize_base_url(&url)?;
            let v = KeepClient::account(c.http.clone(), &base, &token)
                .account_me()
                .await
                .map_err(|e| explain(e, false))?;
            c.save_account(&base, &token, &v)
        })
        .await
    }

    /// At start: re-attaches the account session with the token from secure storage. No
    /// network.
    pub fn account_activate(&self, token: String) -> Result<()> {
        let a = self.account().ok_or(CoreError::NotFound)?;
        *self.core.account.write().unwrap_or_else(|e| e.into_inner()) = Some(KeepClient::account(
            self.core.http.clone(),
            &a.base_url,
            &token,
        ));
        Ok(())
    }

    /// The AlwyWatch account this device is signed in to.
    pub fn account(&self) -> Option<AlwyAccount> {
        self.core
            .store
            .setting(ACCOUNT_KEY)
            .ok()
            .flatten()
            .and_then(|s| serde_json::from_str(&s).ok())
    }

    /// Signs this device out of the AlwyWatch account (the platform forgets the token).
    pub async fn account_sign_out(&self) -> Result<()> {
        let client = self.core.account_client();
        *self.core.account.write().unwrap_or_else(|e| e.into_inner()) = None;
        self.core.store.remove_setting(ACCOUNT_KEY)?;
        if let Some(k) = client {
            // Best effort: the session also ends by itself when unused.
            let _ = self
                .run(|_| async move { Ok(k.account_logout().await?) })
                .await;
        }
        Ok(())
    }

    /// Quick Connect from this device: signs the device showing `code` in to my account
    /// (with the account session or the linked media sign-in). Returns its name.
    pub async fn account_approve(&self, code: String) -> Result<String> {
        self.run(|c| async move {
            let k = c.person_client()?;
            Ok(k.transfer_approve(code.trim()).await?.device)
        })
        .await
    }

    /// After a Jellyfin/Plex sign-in while signed in to an AlwyWatch account: that sign-in
    /// joins the account (one person). `token` as `keep_connect` takes it. Servers the
    /// AlwyWatch server doesn't serve are skipped (false).
    pub async fn account_link_media(&self, account_id: String, token: String) -> Result<bool> {
        let Some(server_id) = self
            .accounts()?
            .into_iter()
            .find(|a| a.id == account_id)
            .map(|a| a.server_id)
        else {
            return Err(CoreError::NotFound);
        };
        self.run(|c| async move {
            let Some(k) = c.account_client() else {
                return Ok(false);
            };
            match k.account_link(&server_id, &token).await {
                Ok(v) => {
                    let base = k.base_url().to_owned();
                    c.save_account(&base, k.token(), &v)?;
                    Ok(true)
                }
                // Not a served server.
                Err(net::Error::Unauthorized) => Ok(false),
                Err(e) => Err(e.into()),
            }
        })
        .await
    }

    /// The servers of my other devices (synced list) not signed in here yet.
    pub fn known_servers(&self) -> Vec<KnownServer> {
        let list: Vec<SavedServer> = self
            .core
            .store
            .setting(SERVERS_SETTING)
            .ok()
            .flatten()
            .and_then(|v| serde_json::from_str(&v).ok())
            .unwrap_or_default();
        let here: Vec<String> = self
            .accounts()
            .unwrap_or_default()
            .into_iter()
            .map(|a| a.base_url)
            .collect();
        list.into_iter()
            .filter(|s| !here.contains(&s.url))
            .map(|s| KnownServer {
                kind: s.kind,
                url: s.url,
                name: s.name,
            })
            .collect()
    }
}

/// A QR code for `text` (medium error correction), e.g. the Quick Connect address.
#[uniffi::export]
pub fn qr_code(text: String) -> Result<QrCode> {
    let q = qrcodegen::QrCode::encode_text(&text, qrcodegen::QrCodeEcc::Medium).map_err(|_| {
        CoreError::Invalid {
            detail: i18n::tr!("too long for a QR code"),
        }
    })?;
    let n = q.size();
    let modules = (0..n)
        .flat_map(|y| (0..n).map(move |x| (x, y)))
        .map(|(x, y)| q.get_module(x, y))
        .collect();
    Ok(QrCode {
        size: n as u32,
        modules,
    })
}

/// The Quick Connect page for `code` on the AlwyWatch server at `url` (what the QR code holds).
#[uniffi::export]
pub fn quick_connect_url(url: String, code: String) -> String {
    format!("{}/q/{}", url.trim_end_matches('/'), code.trim())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn qr_codes_are_square_and_hold_the_address() {
        let q = qr_code(quick_connect_url(
            "https://watch.example/".into(),
            "123456".into(),
        ))
        .unwrap();
        assert_eq!(q.modules.len(), (q.size * q.size) as usize);
        assert!(q.size >= 21);
        // Finder pattern corner.
        assert!(q.modules[0]);
    }
}
