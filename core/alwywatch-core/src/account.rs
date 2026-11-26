// SPDX-License-Identifier: AGPL-3.0-or-later
//! AlwyWatch accounts on the apps (docs/keep-protocol.md "AlwyWatch accounts"): signing in to
//! the AlwyWatch server without a media server (password, or Quick Connect approved from the
//! web or another device), the servers list kept in the synced settings, and QR codes for the
//! Quick Connect screen. The session token is the platform's to keep in secure storage, like
//! media tokens; the core keeps the rest.

use keep_service::KeepClient;
use keep_service::account::{self, SERVERS_SETTING, SavedServer};
use serde::{Deserialize, Serialize};

use keep_service::ServerType;
use media_api::jellyfin::JellyfinAuth;
use media_api::plex::{PlexAuth, PlexConnection, PlexServerResource, PlexUser};

use crate::app::{AlwyWatch, Core, SignedIn};
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
    /// The Jellyfin login an invitation made (same username and password), already signed in
    /// on this device, at sign-up or a later sign-in: store its token like after any Jellyfin
    /// sign-in.
    pub jellyfin: Option<crate::app::SignedIn>,
}

/// A server from the user's synced list (sign in to it on this device).
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct KnownServer {
    /// "jellyfin" or "plex".
    pub kind: String,
    pub url: String,
    pub name: String,
}

/// A media-server user linked to my AlwyWatch account, on a served server this device is not
/// signed in to: [`AlwyWatch::account_sign_in_server`] signs it in without typing anything.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct LinkedServer {
    pub server_id: String,
    /// "jellyfin" or "plex".
    pub kind: String,
    pub server_name: String,
    /// My user name there.
    pub user_name: String,
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
    /// Where it was emailed (`account_invite_send` with an email).
    pub sent_to: Option<String>,
}

/// What an admin's invitation can do on this AlwyWatch server, and its invitations
/// (`account_invitations`).
#[derive(Debug, Clone, uniffi::Record)]
pub struct Invitations {
    /// Emails can be sent (Server settings → Email).
    pub mail: bool,
    /// This site's address is set (needed for the links).
    pub public_url: bool,
    /// Jellyfin servers where an invitation can also make a login.
    pub jellyfin: Vec<JellyfinTarget>,
    /// Newest first, as the server lists them.
    pub invites: Vec<InviteItem>,
}

/// A Jellyfin server an invitation can make a login on.
#[derive(Debug, Clone, uniffi::Record)]
pub struct JellyfinTarget {
    pub server_id: String,
    pub name: String,
    /// Users whose access (libraries, ratings…) a new login can copy.
    pub users: Vec<JellyfinUser>,
    /// The user copied last time, to offer first.
    pub last_like: Option<String>,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct JellyfinUser {
    pub id: String,
    pub name: String,
}

/// An invitation in the admin's list.
#[derive(Debug, Clone, uniffi::Record)]
pub struct InviteItem {
    pub id: i64,
    /// Who made it.
    pub created_by: String,
    /// Unix seconds.
    pub created: i64,
    /// Unix seconds.
    pub expires: i64,
    pub uses_left: i64,
    /// Where it was emailed; none for a link.
    pub email: Option<String>,
    /// The Jellyfin server name, when it also makes a login there.
    pub jellyfin: Option<String>,
    /// Usernames of the accounts made with it.
    pub used_by: Vec<String>,
}

/// What an invitation gives, for the sign-up screen (`account_invite_info`).
#[derive(Debug, Clone, uniffi::Record)]
pub struct InviteInfo {
    /// Who invited.
    pub inviter: String,
    /// The Jellyfin server the account also gets a login on (its name).
    pub jellyfin: Option<String>,
    /// Who it was sent to: offered as the account's email (for "forgot password").
    pub email: Option<String>,
    /// Unix seconds.
    pub expires: i64,
}

impl From<account::Invite> for AccountInvite {
    fn from(i: account::Invite) -> Self {
        Self {
            code: i.code,
            url: i.url,
            expires_in_days: i.expires_in_days,
            uses: i.uses,
            sent_to: i.sent_to,
        }
    }
}

impl From<account::Invitations> for Invitations {
    fn from(v: account::Invitations) -> Self {
        Self {
            mail: v.mail,
            public_url: v.public_url,
            jellyfin: v
                .jellyfin
                .into_iter()
                .map(|t| JellyfinTarget {
                    server_id: t.server_id,
                    name: t.name,
                    users: t
                        .users
                        .into_iter()
                        .map(|u| JellyfinUser {
                            id: u.id,
                            name: u.name,
                        })
                        .collect(),
                    last_like: t.last_like,
                })
                .collect(),
            invites: v
                .invites
                .into_iter()
                .map(|i| InviteItem {
                    id: i.id,
                    created_by: i.created_by,
                    created: i.created,
                    expires: i.expires,
                    uses_left: i.uses_left,
                    email: i.email,
                    jellyfin: i.jellyfin,
                    used_by: i.used_by,
                })
                .collect(),
        }
    }
}

/// Trimmed, `None` when empty.
fn given(s: Option<String>) -> Option<String> {
    s.map(|s| s.trim().to_owned()).filter(|s| !s.is_empty())
}

impl AlwyWatch {
    /// Runs an admin call as the AlwyWatch account's session, else the Jellyfin/Plex login used
    /// for AlwyWatch (the server accepts any admin). `NotFound` when there is neither.
    async fn admin_call<T, F, Fut>(&self, f: F) -> Result<T>
    where
        T: Send + 'static,
        F: FnOnce(KeepClient) -> Fut + Send + 'static,
        Fut: std::future::Future<Output = Result<T>> + Send + 'static,
    {
        let account = self.core.account_client();
        self.run(|c| async move {
            let client = match account {
                Some(a) => a,
                None => c.keep()?,
            };
            f(client).await
        })
        .await
    }
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
        let pw = password.clone();
        let (mut out, login) = self
            .run(|c| async move {
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
                Ok((
                    AccountSignedIn {
                        account,
                        token: s.token,
                        jellyfin: None,
                    },
                    s.jellyfin,
                ))
            })
            .await?;
        // The Jellyfin login an invitation made with this username and password: signed in
        // too, so a new device goes straight to the library (quietly skipped if it fails).
        if let Some(j) = login {
            out.jellyfin = self.jellyfin_sign_in(j.url, j.username, pw).await.ok();
        }
        Ok(out)
    }

    /// Creates an AlwyWatch account (with an invitation code when the server asks for one) and
    /// signs this device in to it.
    pub async fn account_register(
        &self,
        url: String,
        username: String,
        password: String,
        invite: Option<String>,
        email: Option<String>,
        device: String,
    ) -> Result<AccountSignedIn> {
        let pw = password.clone();
        let (mut out, login) = self
            .run(|c| async move {
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
                        email: email.map(|e| e.trim().to_owned()).filter(|e| !e.is_empty()),
                        device,
                    },
                )
                .await
                .map_err(|e| explain(e, true))?;
                let account = c.save_account(&base, &s.token, &s.account)?;
                Ok((
                    AccountSignedIn {
                        account,
                        token: s.token,
                        jellyfin: None,
                    },
                    s.jellyfin,
                ))
            })
            .await?;
        // The Jellyfin login the invitation made: signed in right away, same password.
        if let Some(j) = login {
            out.jellyfin = self.jellyfin_sign_in(j.url, j.username, pw).await.ok();
        }
        Ok(out)
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
        self.account_invite_send(None, None, None, None, days, uses)
            .await
    }

    /// Admins: an invitation, emailed to `email` (one use, in this app's language) or a link
    /// for `uses` people, valid `days` days. With `jellyfin` (a server id from
    /// `account_invitations`) it also makes a Jellyfin login there, with the access of the
    /// Jellyfin user `like` (Jellyfin's defaults when none). `note`: a few words in the email.
    pub async fn account_invite_send(
        &self,
        email: Option<String>,
        jellyfin: Option<String>,
        like: Option<String>,
        note: Option<String>,
        days: i64,
        uses: i64,
    ) -> Result<AccountInvite> {
        let request = account::InviteRequest {
            days,
            uses,
            email: given(email),
            jellyfin: given(jellyfin),
            like: given(like),
            note: given(note),
            lang: Some(i18n::language().to_owned()),
        };
        let emailing = request.email.is_some();
        self.admin_call(move |k| async move {
            k.account_invite_with(&request)
                .await
                .map(AccountInvite::from)
                .map_err(|e| match e {
                    net::Error::Status { status: 501 } if emailing => CoreError::Invalid {
                        detail: i18n::tr!(
                            "Set up email and this site's address in Server settings (web app) first."
                        ),
                    },
                    net::Error::Status { status: 502 } if emailing => CoreError::Invalid {
                        detail: i18n::tr!(
                            "The email could not be sent. Check the email settings in Server settings (web app)."
                        ),
                    },
                    e => explain(e, false),
                })
        })
        .await
    }

    /// Admins: what an invitation can do here (email, Jellyfin logins) and the invitations.
    pub async fn account_invitations(&self) -> Result<Invitations> {
        self.admin_call(|k| async move {
            k.account_invites()
                .await
                .map(Invitations::from)
                .map_err(|e| explain(e, false))
        })
        .await
    }

    /// Admins: ends an invitation (`InviteItem::id`); it no longer works.
    pub async fn account_revoke_invite(&self, id: i64) -> Result<()> {
        self.admin_call(move |k| async move {
            k.account_revoke_invite(id)
                .await
                .map_err(|e| explain(e, false))
        })
        .await
    }

    /// "Forgot password" on the AlwyWatch server at `url`: emails a link to set a new one to
    /// the account `login` (username or email) names, if it has an email. The answer is the
    /// same whether or not one does; an error only when the server can't send emails.
    pub async fn account_forgot_password(&self, url: String, login: String) -> Result<()> {
        self.run(|c| async move {
            let base = net::normalize_base_url(&url)?;
            KeepClient::password_forgot(
                &c.http,
                &base,
                &account::ForgotRequest {
                    login: login.trim().to_owned(),
                    lang: i18n::language().to_owned(),
                },
            )
            .await
            .map_err(|e| explain(e, false))
        })
        .await
    }

    /// Before signing up with an invitation `code` on the AlwyWatch server at `url` (no
    /// sign-in): who invited, and whether it also makes a Jellyfin login. An `Invalid` error
    /// saying so when the code is unknown, used or expired.
    pub async fn account_invite_info(&self, url: String, code: String) -> Result<InviteInfo> {
        self.run(|c| async move {
            let base = net::normalize_base_url(&url)?;
            let i = KeepClient::account_invite_info(&c.http, &base, &code)
                .await
                .map_err(|e| explain(e, false))?;
            Ok(InviteInfo {
                inviter: i.inviter,
                jellyfin: i.jellyfin,
                email: i.email,
                expires: i.expires,
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

    /// My linked sign-ins on served servers this device is not signed in to (empty without an
    /// AlwyWatch account).
    pub async fn account_linked_servers(&self) -> Result<Vec<LinkedServer>> {
        let here: Vec<String> = self.accounts()?.into_iter().map(|a| a.server_id).collect();
        self.run(|c| async move {
            let Some(k) = c.account_client() else {
                return Ok(vec![]);
            };
            let v = k.account_me().await.map_err(|e| explain(e, false))?;
            let served = KeepClient::servers(&c.http, k.base_url()).await?;
            let mut out: Vec<LinkedServer> = vec![];
            for l in v.linked {
                let Some(srv) = served.iter().find(|x| x.id == l.server_id) else {
                    continue;
                };
                if here.contains(&l.server_id) || out.iter().any(|o| o.server_id == l.server_id) {
                    continue;
                }
                out.push(LinkedServer {
                    server_id: l.server_id,
                    kind: l.kind,
                    server_name: srv.name.clone(),
                    user_name: l.name,
                });
            }
            Ok(out)
        })
        .await
    }

    /// Signs this device in to `server_id` as my linked user there, through my AlwyWatch
    /// account (nothing to type: Jellyfin approves this device's Quick Connect code, Plex hands
    /// over my server token). Store the token like after any sign-in; `plex_account_token` is
    /// none (the Plex watchlist needs a plex.tv sign-in).
    pub async fn account_sign_in_server(&self, server_id: String) -> Result<SignedIn> {
        self.run(|c| async move {
            let k = c.account_client().ok_or(CoreError::NotSignedIn {
                account_id: "account".into(),
            })?;
            let served = KeepClient::servers(&c.http, k.base_url()).await?;
            let srv = served
                .into_iter()
                .find(|x| x.id == server_id)
                .ok_or(CoreError::NotFound)?;
            let url = net::normalize_base_url(&srv.url)?;
            match srv.kind {
                ServerType::Jellyfin => {
                    let auth = JellyfinAuth::new(c.http.clone(), c.client.clone());
                    let qc = auth.quick_connect_start(&url).await?;
                    k.server_sign_in(&server_id, Some(&qc.code))
                        .await
                        .map_err(|e| explain(e, false))?;
                    // Approved already: the first poll normally signs in.
                    let mut tries = 0;
                    loop {
                        match auth.quick_connect_poll(&url, &qc.secret).await {
                            Ok(session) => return c.signed_in(session),
                            Err(media_api::Error::Pending) if tries < 5 => {
                                tries += 1;
                                tokio::time::sleep(std::time::Duration::from_millis(600)).await;
                            }
                            Err(e) => return Err(e.into()),
                        }
                    }
                }
                ServerType::Plex => {
                    let r = k
                        .server_sign_in(&server_id, None)
                        .await
                        .map_err(|e| explain(e, false))?;
                    let token = r.token.ok_or(CoreError::NotFound)?;
                    let server = PlexServerResource {
                        name: r.server_name,
                        machine_id: r.server_id,
                        owned: r.is_admin,
                        access_token: token,
                        connections: vec![PlexConnection {
                            uri: net::normalize_base_url(&r.url)?,
                            local: false,
                            relay: false,
                        }],
                    };
                    let user = PlexUser {
                        id: r.user_id,
                        username: r.user_name.clone(),
                        title: r.user_name,
                    };
                    let session = PlexAuth::new(c.http.clone(), c.client.clone())
                        .connect(&server, &user, None)
                        .await?;
                    c.signed_in(session)
                }
            }
        })
        .await
    }

    /// Right after signing in to my AlwyWatch account on a new device: signs in to every linked
    /// server too ([`Self::account_sign_in_server`]). The ones that fail are left out (the
    /// user can still sign in to them by hand).
    pub async fn account_sign_in_linked(&self) -> Result<Vec<SignedIn>> {
        let mut out = vec![];
        for l in self.account_linked_servers().await? {
            if let Ok(s) = self.account_sign_in_server(l.server_id).await {
                out.push(s);
            }
        }
        Ok(out)
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

/// The sign-in code in a camera frame showing another device's "Sign in with a code" QR code:
/// `luma` is the frame's brightness plane (`width` × `height`, `row_stride` bytes a row).
/// None until a readable code of ours is in view.
#[uniffi::export]
pub fn qr_sign_in_code(luma: Vec<u8>, width: u32, height: u32, row_stride: u32) -> Option<String> {
    let (w, h, stride) = (width as usize, height as usize, row_stride as usize);
    if w == 0 || h == 0 || stride < w || luma.len() < stride * (h - 1) + w {
        return None;
    }
    let mut img = rqrr::PreparedImage::prepare_from_greyscale(w, h, |x, y| luma[y * stride + x]);
    img.detect_grids()
        .into_iter()
        .filter_map(|g| g.decode().ok().map(|(_, text)| text))
        .find_map(|t| sign_in_code_of(&t))
}

/// `…/q/123456` (the address in the QR code), or six digits on their own.
fn sign_in_code_of(text: &str) -> Option<String> {
    let t = text.trim();
    let tail = t.rsplit_once("/q/").map_or(t, |(_, c)| c);
    let code: String = tail
        .chars()
        .take_while(|c| !matches!(c, '?' | '#' | '/'))
        .collect();
    (code.len() == 6 && code.bytes().all(|b| b.is_ascii_digit())).then_some(code)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sign_in_qr_code_is_read_back_from_a_frame() {
        let q = qr_code("https://watch.example.org/q/058405".into()).unwrap();
        // Drawn as a camera would see it: 4 px a module, a quiet zone, inside a wider row.
        let (scale, quiet) = (4usize, 4usize);
        let side = (q.size as usize + 2 * quiet) * scale;
        let stride = side + 16;
        let mut luma = vec![255u8; stride * side];
        for y in 0..side {
            for x in 0..side {
                let (mx, my) = (x / scale, y / scale);
                let n = q.size as usize;
                let dark = (quiet..quiet + n).contains(&mx)
                    && (quiet..quiet + n).contains(&my)
                    && q.modules[(my - quiet) * n + (mx - quiet)];
                if dark {
                    luma[y * stride + x] = 0;
                }
            }
        }
        assert_eq!(
            qr_sign_in_code(luma, side as u32, side as u32, stride as u32).as_deref(),
            Some("058405")
        );
        assert_eq!(qr_sign_in_code(vec![255; 100], 10, 10, 10), None);
        assert_eq!(sign_in_code_of("123456").as_deref(), Some("123456"));
        assert_eq!(sign_in_code_of("https://x/q/12345"), None);
        assert_eq!(sign_in_code_of("https://evil/other"), None);
    }

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
