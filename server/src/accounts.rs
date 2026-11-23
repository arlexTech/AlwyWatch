// SPDX-License-Identifier: AGPL-3.0-or-later
//! AlwyWatch accounts: ways for a person to sign in to this server without a media-server
//! sign-in (username + password, passkeys, Pocket ID), and the sessions they get.
//!
//! An account belongs to a person (see `db.rs`): a person that only has an account is
//! `account:<id>`; linking a Jellyfin/Plex sign-in merges it like any other link, and the
//! account follows the merged person. A session token (`aws_…`) is only ever stored hashed.

use ring::digest;
use ring::rand::{SecureRandom, SystemRandom};
use rusqlite::{OptionalExtension, params};

use crate::db::{Db, now};

/// Session tokens start with this (media tokens never do), so a request says which it is.
pub use keep_service::account::SESSION_PREFIX;
/// A session unused for this long is gone.
const SESSION_IDLE: i64 = 180 * 86_400;
/// `last_seen` is written at most this often (a request per second must not write each time).
const TOUCH_EVERY: i64 = 3_600;
/// Sessions per account (the oldest goes).
const MAX_SESSIONS: i64 = 50;
pub const MIN_PASSWORD: usize = 8;
const MAX_PASSWORD: usize = 256;

pub fn random_hex(bytes: usize) -> String {
    let mut b = vec![0u8; bytes];
    // The OS generator does not fail in practice; a zeroed buffer would be caught by tests.
    let _ = SystemRandom::new().fill(&mut b);
    hex(&b)
}

pub fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

pub fn sha256_hex(s: &[u8]) -> String {
    hex(digest::digest(&digest::SHA256, s).as_ref())
}

pub fn new_token() -> String {
    format!("{SESSION_PREFIX}{}", random_hex(32))
}

/// Argon2id with the crate's defaults (19 MiB, 2 passes): about 30 ms on the server.
pub fn hash_password(password: &str) -> Result<String, AccountError> {
    use argon2::password_hash::{PasswordHasher, SaltString};
    use rand_core::OsRng;
    check_password(password)?;
    argon2::Argon2::default()
        .hash_password(password.as_bytes(), &SaltString::generate(&mut OsRng))
        .map(|h| h.to_string())
        .map_err(|_| AccountError::Invalid("could not store this password"))
}

pub fn verify_password(hash: &str, password: &str) -> bool {
    use argon2::password_hash::{PasswordHash, PasswordVerifier};
    PasswordHash::new(hash).is_ok_and(|h| {
        argon2::Argon2::default()
            .verify_password(password.as_bytes(), &h)
            .is_ok()
    })
}

/// Spends about the time of a real check, so a wrong username answers as slowly as a wrong
/// password (no way to tell which usernames exist).
pub fn verify_nothing(password: &str) {
    static DUMMY: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    let h = DUMMY.get_or_init(|| hash_password("not a real password").unwrap_or_default());
    let _ = verify_password(h, password);
}

pub fn check_password(p: &str) -> Result<(), AccountError> {
    if p.chars().count() < MIN_PASSWORD {
        return Err(AccountError::Invalid(
            "the password needs at least 8 characters",
        ));
    }
    if p.len() > MAX_PASSWORD {
        return Err(AccountError::Invalid("this password is too long"));
    }
    Ok(())
}

/// 3–32 letters, digits, `.`, `_` or `-`.
pub fn check_username(u: &str) -> Result<(), AccountError> {
    let ok = (3..=32).contains(&u.chars().count())
        && u.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'));
    if ok {
        Ok(())
    } else {
        Err(AccountError::Invalid(
            "a username has 3 to 32 letters, digits, dots, dashes or underscores",
        ))
    }
}

fn clean_device(d: &str) -> String {
    let d: String = d.chars().filter(|c| !c.is_control()).take(64).collect();
    let d = d.trim();
    if d.is_empty() {
        "Unknown device".into()
    } else {
        d.to_owned()
    }
}

#[derive(Debug, PartialEq)]
pub enum AccountError {
    Invalid(&'static str),
    UsernameTaken,
    Db(String),
}

impl From<rusqlite::Error> for AccountError {
    fn from(e: rusqlite::Error) -> Self {
        AccountError::Db(e.to_string())
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct AccountRow {
    pub id: String,
    pub person: String,
    pub username: Option<String>,
    pub has_password: bool,
    pub created: i64,
    /// The owner turned media sign-ins off for this account: a Jellyfin/Plex sign-in of the
    /// person still watches, votes and joins rooms, but no longer sets the account's
    /// username or password nor signs new devices in to it.
    pub media_locked: bool,
}

fn row_account(r: &rusqlite::Row<'_>) -> rusqlite::Result<AccountRow> {
    Ok(AccountRow {
        id: r.get(0)?,
        person: r.get(1)?,
        username: r.get(2)?,
        has_password: r.get::<_, Option<String>>(3)?.is_some(),
        created: r.get(4)?,
        media_locked: r.get::<_, i64>(5)? != 0,
    })
}

const ACCOUNT_COLS: &str = "id, person, username, password, created, media_locked";

#[derive(Debug, Clone, PartialEq)]
pub struct SessionRow {
    /// The first 12 characters of the token's hash: names a session (to sign it out).
    pub id: String,
    pub device: String,
    pub created: i64,
    pub last_seen: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PasskeyRow {
    pub cred_id: String,
    pub account: String,
    pub name: String,
    pub alg: i64,
    pub public_key: Vec<u8>,
    pub sign_count: u32,
    pub created: i64,
    pub last_used: Option<i64>,
}

impl Db {
    /// A new account. `person`: an existing person adding a way to sign in; None: a new
    /// person (`account:<id>`), named after the username.
    pub fn create_account(
        &self,
        username: Option<&str>,
        password_hash: Option<&str>,
        person: Option<&str>,
        name: &str,
    ) -> Result<AccountRow, AccountError> {
        let mut c = self.c();
        let tx = c.transaction()?;
        if let Some(u) = username {
            let taken: bool = tx.query_row(
                "SELECT COUNT(*) FROM accounts WHERE username = ?1",
                [u],
                |r| r.get::<_, i64>(0).map(|n| n > 0),
            )?;
            if taken {
                return Err(AccountError::UsernameTaken);
            }
        }
        let id = random_hex(16);
        let now = now();
        let person = match person {
            Some(p) => p.to_owned(),
            None => {
                let p = format!("account:{id}");
                tx.execute(
                    "INSERT INTO users(user_key, name, server_id, is_admin, last_seen)
                     VALUES (?1, ?2, '', 0, ?3)",
                    params![p, name, now],
                )?;
                p
            }
        };
        tx.execute(
            "INSERT INTO accounts(id, person, username, password, created)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![id, person, username, password_hash, now],
        )?;
        tx.commit()?;
        Ok(AccountRow {
            id,
            person,
            username: username.map(str::to_owned),
            has_password: password_hash.is_some(),
            created: now,
            media_locked: false,
        })
    }

    pub fn account(&self, id: &str) -> rusqlite::Result<Option<AccountRow>> {
        self.c()
            .query_row(
                &format!("SELECT {ACCOUNT_COLS} FROM accounts WHERE id = ?1"),
                [id],
                row_account,
            )
            .optional()
    }

    /// The account and its password hash.
    pub fn account_by_username(
        &self,
        username: &str,
    ) -> rusqlite::Result<Option<(AccountRow, Option<String>)>> {
        self.c()
            .query_row(
                &format!("SELECT {ACCOUNT_COLS} FROM accounts WHERE username = ?1"),
                [username],
                |r| Ok((row_account(r)?, r.get(3)?)),
            )
            .optional()
    }

    /// The person's account (the oldest, when two merged persons each had one).
    pub fn account_of_person(&self, person: &str) -> rusqlite::Result<Option<AccountRow>> {
        self.c()
            .query_row(
                &format!(
                    "SELECT {ACCOUNT_COLS} FROM accounts WHERE person = ?1 ORDER BY created LIMIT 1"
                ),
                [person],
                row_account,
            )
            .optional()
    }

    /// Deletes `account` with its sessions, passkeys and Pocket ID link. A person who only
    /// existed through the account (`account:<id>`, no media sign-in) goes with it: profile,
    /// synced settings, rights, votes; the person's key is then returned (its avatar file is
    /// the caller's to remove). A person with Jellyfin/Plex sign-ins stays, as they were
    /// before the account.
    pub fn delete_account(&self, account: &AccountRow) -> rusqlite::Result<Option<String>> {
        let mut c = self.c();
        let tx = c.transaction()?;
        for table in ["sessions", "passkeys", "oidc_links"] {
            tx.execute(
                &format!("DELETE FROM {table} WHERE account = ?1"),
                [&account.id],
            )?;
        }
        tx.execute("DELETE FROM accounts WHERE id = ?1", [&account.id])?;
        let p = &account.person;
        let still_used: i64 = tx.query_row(
            "SELECT (SELECT COUNT(*) FROM identities WHERE person = ?1)
                  + (SELECT COUNT(*) FROM accounts WHERE person = ?1)",
            [p],
            |r| r.get(0),
        )?;
        let gone = still_used == 0;
        if gone {
            for table in ["users", "rights", "settings", "votes", "seerr_links"] {
                tx.execute(&format!("DELETE FROM {table} WHERE user_key = ?1"), [p])?;
            }
            tx.execute("DELETE FROM together_pairs WHERE a = ?1 OR b = ?1", [p])?;
        }
        tx.commit()?;
        Ok(gone.then(|| p.clone()))
    }

    pub fn password_hash(&self, account: &str) -> rusqlite::Result<Option<String>> {
        Ok(self
            .c()
            .query_row(
                "SELECT password FROM accounts WHERE id = ?1",
                [account],
                |r| r.get(0),
            )
            .optional()?
            .flatten())
    }

    pub fn set_password(&self, account: &str, hash: Option<&str>) -> rusqlite::Result<()> {
        self.c().execute(
            "UPDATE accounts SET password = ?2 WHERE id = ?1",
            params![account, hash],
        )?;
        Ok(())
    }

    pub fn set_media_locked(&self, account: &str, locked: bool) -> rusqlite::Result<()> {
        self.c().execute(
            "UPDATE accounts SET media_locked = ?2 WHERE id = ?1",
            params![account, locked],
        )?;
        Ok(())
    }

    pub fn set_username(&self, account: &str, username: &str) -> Result<(), AccountError> {
        let c = self.c();
        let taken: bool = c.query_row(
            "SELECT COUNT(*) FROM accounts WHERE username = ?1 AND id != ?2",
            params![username, account],
            |r| r.get::<_, i64>(0).map(|n| n > 0),
        )?;
        if taken {
            return Err(AccountError::UsernameTaken);
        }
        c.execute(
            "UPDATE accounts SET username = ?2 WHERE id = ?1",
            params![account, username],
        )?;
        Ok(())
    }

    // ----- sessions ---------------------------------------------------------------------------

    /// A new session of `account`: its token (only its hash is stored).
    pub fn new_session(&self, account: &str, device: &str) -> rusqlite::Result<String> {
        let token = new_token();
        let now = now();
        let c = self.c();
        c.execute(
            "INSERT INTO sessions(token_hash, account, device, created, last_seen)
             VALUES (?1, ?2, ?3, ?4, ?4)",
            params![
                sha256_hex(token.as_bytes()),
                account,
                clean_device(device),
                now
            ],
        )?;
        c.execute(
            "DELETE FROM sessions WHERE account = ?1 AND token_hash NOT IN
               (SELECT token_hash FROM sessions WHERE account = ?1
                ORDER BY last_seen DESC LIMIT ?2)",
            params![account, MAX_SESSIONS],
        )?;
        Ok(token)
    }

    /// The account of a session token, and the session's id; None when unknown or expired.
    pub fn session(&self, token: &str) -> rusqlite::Result<Option<(AccountRow, String)>> {
        let hash = sha256_hex(token.as_bytes());
        let now = now();
        let c = self.c();
        let found: Option<(String, i64)> = c
            .query_row(
                "SELECT account, last_seen FROM sessions WHERE token_hash = ?1",
                [&hash],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let Some((account, last_seen)) = found else {
            return Ok(None);
        };
        if now - last_seen > SESSION_IDLE {
            c.execute("DELETE FROM sessions WHERE token_hash = ?1", [&hash])?;
            return Ok(None);
        }
        if now - last_seen > TOUCH_EVERY {
            c.execute(
                "UPDATE sessions SET last_seen = ?2 WHERE token_hash = ?1",
                params![hash, now],
            )?;
        }
        let acc = c
            .query_row(
                &format!("SELECT {ACCOUNT_COLS} FROM accounts WHERE id = ?1"),
                [&account],
                row_account,
            )
            .optional()?;
        Ok(acc.map(|a| (a, hash[..12].to_owned())))
    }

    pub fn sessions(&self, account: &str) -> rusqlite::Result<Vec<SessionRow>> {
        let c = self.c();
        let mut st = c.prepare(
            "SELECT token_hash, device, created, last_seen FROM sessions WHERE account = ?1
             ORDER BY last_seen DESC",
        )?;
        st.query_map([account], |r| {
            let h: String = r.get(0)?;
            Ok(SessionRow {
                id: h[..12].to_owned(),
                device: r.get(1)?,
                created: r.get(2)?,
                last_seen: r.get(3)?,
            })
        })?
        .collect()
    }

    /// Signs out the session `id` (see [`SessionRow::id`]) of `account`.
    pub fn end_session(&self, account: &str, id: &str) -> rusqlite::Result<bool> {
        if id.len() != 12 || !id.chars().all(|c| c.is_ascii_hexdigit()) {
            return Ok(false);
        }
        let n = self.c().execute(
            "DELETE FROM sessions WHERE account = ?1 AND substr(token_hash, 1, 12) = ?2",
            params![account, id],
        )?;
        Ok(n > 0)
    }

    /// Every session of `account` but `keep` (after a password change).
    pub fn end_other_sessions(&self, account: &str, keep: Option<&str>) -> rusqlite::Result<()> {
        self.c().execute(
            "DELETE FROM sessions WHERE account = ?1 AND substr(token_hash, 1, 12) != ?2",
            params![account, keep.unwrap_or("")],
        )?;
        Ok(())
    }

    // ----- passkeys ---------------------------------------------------------------------------

    pub fn add_passkey(&self, p: &PasskeyRow) -> rusqlite::Result<()> {
        self.c().execute(
            "INSERT INTO passkeys(cred_id, account, name, alg, public_key, sign_count, created)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                p.cred_id,
                p.account,
                p.name,
                p.alg,
                p.public_key,
                p.sign_count,
                p.created
            ],
        )?;
        Ok(())
    }

    pub fn passkey(&self, cred_id: &str) -> rusqlite::Result<Option<PasskeyRow>> {
        self.c()
            .query_row(
                "SELECT cred_id, account, name, alg, public_key, sign_count, created, last_used
                 FROM passkeys WHERE cred_id = ?1",
                [cred_id],
                row_passkey,
            )
            .optional()
    }

    pub fn passkeys(&self, account: &str) -> rusqlite::Result<Vec<PasskeyRow>> {
        let c = self.c();
        let mut st = c.prepare(
            "SELECT cred_id, account, name, alg, public_key, sign_count, created, last_used
             FROM passkeys WHERE account = ?1 ORDER BY created",
        )?;
        st.query_map([account], row_passkey)?.collect()
    }

    pub fn passkey_used(&self, cred_id: &str, sign_count: u32) -> rusqlite::Result<()> {
        self.c().execute(
            "UPDATE passkeys SET sign_count = ?2, last_used = ?3 WHERE cred_id = ?1",
            params![cred_id, sign_count, now()],
        )?;
        Ok(())
    }

    pub fn rename_passkey(
        &self,
        account: &str,
        cred_id: &str,
        name: &str,
    ) -> rusqlite::Result<bool> {
        let n = self.c().execute(
            "UPDATE passkeys SET name = ?3 WHERE account = ?1 AND cred_id = ?2",
            params![account, cred_id, name],
        )?;
        Ok(n > 0)
    }

    pub fn delete_passkey(&self, account: &str, cred_id: &str) -> rusqlite::Result<bool> {
        let n = self.c().execute(
            "DELETE FROM passkeys WHERE account = ?1 AND cred_id = ?2",
            params![account, cred_id],
        )?;
        Ok(n > 0)
    }

    // ----- Pocket ID (OIDC) ------------------------------------------------------------------

    pub fn oidc_account(&self, issuer: &str, sub: &str) -> rusqlite::Result<Option<String>> {
        self.c()
            .query_row(
                "SELECT account FROM oidc_links WHERE issuer = ?1 AND sub = ?2",
                params![issuer, sub],
                |r| r.get(0),
            )
            .optional()
    }

    pub fn link_oidc(&self, issuer: &str, sub: &str, account: &str) -> rusqlite::Result<()> {
        self.c().execute(
            "INSERT INTO oidc_links(issuer, sub, account, created) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(issuer, sub) DO UPDATE SET account = excluded.account",
            params![issuer, sub, account, now()],
        )?;
        Ok(())
    }

    pub fn has_oidc(&self, account: &str) -> rusqlite::Result<bool> {
        self.c().query_row(
            "SELECT COUNT(*) FROM oidc_links WHERE account = ?1",
            [account],
            |r| r.get::<_, i64>(0).map(|n| n > 0),
        )
    }

    pub fn unlink_oidc(&self, account: &str) -> rusqlite::Result<()> {
        self.c()
            .execute("DELETE FROM oidc_links WHERE account = ?1", [account])?;
        Ok(())
    }

    /// Ways left to sign in to `account` (password, passkeys, Pocket ID). Removing the last
    /// one is refused unless the person also has a media sign-in.
    pub fn sign_in_methods(&self, account: &str) -> rusqlite::Result<u32> {
        let c = self.c();
        c.query_row(
            "SELECT (SELECT COUNT(*) FROM accounts WHERE id = ?1 AND password IS NOT NULL)
                  + (SELECT COUNT(*) FROM passkeys WHERE account = ?1)
                  + (SELECT COUNT(*) FROM oidc_links WHERE account = ?1)",
            [account],
            |r| r.get::<_, u32>(0),
        )
    }

    // ----- invitations -----------------------------------------------------------------------

    /// A new invitation code (only its hash is stored).
    #[cfg(test)]
    pub fn create_invite(&self, by: &str, days: i64, uses: i64) -> rusqlite::Result<String> {
        self.create_invite_with(by, days, uses, &InviteExtra::default())
            .map(|(code, _)| code)
    }

    /// A new invitation: (its code, its id). Old ones are forgotten a month after they expire.
    pub fn create_invite_with(
        &self,
        by: &str,
        days: i64,
        uses: i64,
        extra: &InviteExtra,
    ) -> rusqlite::Result<(String, i64)> {
        let code = random_hex(12);
        let now = now();
        let c = self.c();
        c.execute(
            "DELETE FROM invites WHERE expires < ?1",
            [now - 30 * 86_400],
        )?;
        c.execute(
            "INSERT INTO invites(code_hash, created_by, created, expires, uses_left, email,
                                 jellyfin, like_user, note)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                sha256_hex(code.as_bytes()),
                by,
                now,
                now + days * 86_400,
                uses,
                extra.email,
                extra.jellyfin,
                extra.like_user,
                extra.note,
            ],
        )?;
        Ok((code, c.last_insert_rowid()))
    }

    /// The invitation behind `code`, while it still works.
    pub fn invite(&self, code: &str) -> rusqlite::Result<Option<InviteRow>> {
        self.c()
            .query_row(
                &format!(
                    "SELECT {INVITE_COLUMNS} FROM invites
                     WHERE code_hash = ?1 AND expires >= ?2 AND uses_left > 0"
                ),
                params![sha256_hex(code.trim().as_bytes()), now()],
                row_invite,
            )
            .optional()
    }

    /// Every invitation, newest first.
    pub fn invites(&self) -> rusqlite::Result<Vec<InviteRow>> {
        let c = self.c();
        let mut q = c.prepare(&format!(
            "SELECT {INVITE_COLUMNS} FROM invites ORDER BY created DESC LIMIT 100"
        ))?;
        q.query_map([], row_invite)?.collect()
    }

    /// Ends an invitation (it stays listed until forgotten).
    pub fn revoke_invite(&self, id: i64) -> rusqlite::Result<bool> {
        Ok(self.c().execute(
            "UPDATE invites SET uses_left = 0, expires = MIN(expires, ?2) WHERE rowid = ?1",
            params![id, now() - 1],
        )? > 0)
    }

    /// Ends the invitations still waiting for this email (a new one replaces them).
    pub fn revoke_invites_to(&self, email: &str) -> rusqlite::Result<()> {
        self.c().execute(
            "UPDATE invites SET uses_left = 0, expires = MIN(expires, ?2)
             WHERE lower(email) = lower(?1) AND uses_left > 0",
            params![email, now() - 1],
        )?;
        Ok(())
    }

    /// Uses one sign-up of `code`; false when unknown, expired or used up.
    #[cfg(test)]
    pub fn use_invite(&self, code: &str) -> rusqlite::Result<bool> {
        self.use_invite_for(code, "")
    }

    /// The same, noting the username it made.
    pub fn use_invite_for(&self, code: &str, username: &str) -> rusqlite::Result<bool> {
        let n = self.c().execute(
            "UPDATE invites SET uses_left = uses_left - 1,
                    used_by = CASE WHEN ?3 = '' THEN used_by
                                   WHEN used_by = '' THEN ?3 ELSE used_by || ',' || ?3 END
             WHERE code_hash = ?1 AND expires >= ?2 AND uses_left > 0",
            params![sha256_hex(code.trim().as_bytes()), now(), username],
        )?;
        Ok(n > 0)
    }

    /// The Jellyfin user an admin's last invitation to `server` copied access from.
    pub fn last_like(&self, by: &str, server: &str) -> rusqlite::Result<Option<String>> {
        self.c()
            .query_row(
                "SELECT like_user FROM invites
                 WHERE created_by = ?1 AND jellyfin = ?2 AND like_user IS NOT NULL
                 ORDER BY created DESC LIMIT 1",
                params![by, server],
                |r| r.get(0),
            )
            .optional()
    }

    pub fn person_name(&self, person: &str) -> rusqlite::Result<Option<String>> {
        self.c()
            .query_row(
                "SELECT COALESCE(custom_name, name) FROM users WHERE user_key = ?1",
                [person],
                |r| r.get(0),
            )
            .optional()
    }
}

#[derive(Debug, Clone, Default)]
pub struct InviteExtra {
    pub email: Option<String>,
    pub jellyfin: Option<String>,
    pub like_user: Option<String>,
    pub note: Option<String>,
}

#[derive(Debug, Clone)]
pub struct InviteRow {
    pub id: i64,
    pub created_by: String,
    pub created: i64,
    pub expires: i64,
    pub uses_left: i64,
    pub extra: InviteExtra,
    pub used_by: Vec<String>,
}

const INVITE_COLUMNS: &str =
    "rowid, created_by, created, expires, uses_left, email, jellyfin, like_user, note, used_by";

fn row_invite(r: &rusqlite::Row<'_>) -> rusqlite::Result<InviteRow> {
    let used: String = r.get(9)?;
    Ok(InviteRow {
        id: r.get(0)?,
        created_by: r.get(1)?,
        created: r.get(2)?,
        expires: r.get(3)?,
        uses_left: r.get(4)?,
        extra: InviteExtra {
            email: r.get(5)?,
            jellyfin: r.get(6)?,
            like_user: r.get(7)?,
            note: r.get(8)?,
        },
        used_by: used
            .split(',')
            .filter(|u| !u.is_empty())
            .map(str::to_owned)
            .collect(),
    })
}

fn row_passkey(r: &rusqlite::Row<'_>) -> rusqlite::Result<PasskeyRow> {
    Ok(PasskeyRow {
        cred_id: r.get(0)?,
        account: r.get(1)?,
        name: r.get(2)?,
        alg: r.get(3)?,
        public_key: r.get(4)?,
        sign_count: r.get(5)?,
        created: r.get(6)?,
        last_used: r.get(7)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passwords_hash_and_verify() {
        let h = hash_password("correct horse").unwrap();
        assert!(h.starts_with("$argon2id$"));
        assert!(verify_password(&h, "correct horse"));
        assert!(!verify_password(&h, "wrong horse"));
        assert!(hash_password("short").is_err());
    }

    #[test]
    fn usernames() {
        assert!(check_username("alex").is_ok());
        assert!(check_username("a.b-c_d").is_ok());
        assert!(check_username("ab").is_err());
        assert!(check_username("has space").is_err());
        assert!(check_username("é").is_err());
    }

    #[test]
    fn an_account_on_its_own_and_its_sessions() {
        let db = Db::open_in_memory().unwrap();
        let h = hash_password("password1").unwrap();
        let a = db
            .create_account(Some("Sam"), Some(&h), None, "Sam")
            .unwrap();
        assert!(a.person.starts_with("account:"));
        assert_eq!(db.person_name(&a.person).unwrap().as_deref(), Some("Sam"));
        // Usernames ignore case.
        assert_eq!(
            db.create_account(Some("sam"), None, None, "x"),
            Err(AccountError::UsernameTaken)
        );
        let (found, hash) = db.account_by_username("SAM").unwrap().unwrap();
        assert_eq!(found.id, a.id);
        assert!(verify_password(&hash.unwrap(), "password1"));

        let t = db.new_session(&a.id, "Pixel").unwrap();
        assert!(t.starts_with(SESSION_PREFIX));
        let (acc, sid) = db.session(&t).unwrap().unwrap();
        assert_eq!(acc.id, a.id);
        assert!(db.session("aws_nope").unwrap().is_none());
        let list = db.sessions(&a.id).unwrap();
        assert_eq!((list.len(), list[0].device.as_str()), (1, "Pixel"));
        assert!(db.end_session(&a.id, &sid).unwrap());
        assert!(db.session(&t).unwrap().is_none());
    }

    #[test]
    fn deleting_an_account_on_its_own_deletes_the_person() {
        let db = Db::open_in_memory().unwrap();
        let a = db.create_account(Some("Sam"), None, None, "Sam").unwrap();
        let t = db.new_session(&a.id, "Pixel").unwrap();
        assert_eq!(
            db.delete_account(&a).unwrap().as_deref(),
            Some(a.person.as_str())
        );
        assert!(db.session(&t).unwrap().is_none());
        assert!(db.account(&a.id).unwrap().is_none());
        assert_eq!(db.person_name(&a.person).unwrap(), None);
        // The username is free again.
        assert!(db.create_account(Some("Sam"), None, None, "Sam").is_ok());
    }

    #[test]
    fn invitations_are_single_use_and_hashed() {
        let db = Db::open_in_memory().unwrap();
        let code = db.create_invite("admin", 7, 1).unwrap();
        assert!(db.use_invite(&code).unwrap());
        assert!(!db.use_invite(&code).unwrap());
        assert!(!db.use_invite("nope").unwrap());
    }

    #[test]
    fn the_last_sign_in_method_is_counted() {
        let db = Db::open_in_memory().unwrap();
        let a = db.create_account(Some("lee"), None, None, "lee").unwrap();
        assert_eq!(db.sign_in_methods(&a.id).unwrap(), 0);
        db.link_oidc("https://id", "sub1", &a.id).unwrap();
        assert_eq!(db.sign_in_methods(&a.id).unwrap(), 1);
        assert_eq!(
            db.oidc_account("https://id", "sub1").unwrap().as_deref(),
            Some(a.id.as_str())
        );
    }
}
