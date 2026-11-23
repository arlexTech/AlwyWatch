// SPDX-License-Identifier: AGPL-3.0-or-later
//! Invitation emails, through the admin's SMTP server (Settings → Admin → Server settings →
//! Email; a server set up with Vaultwarden-style `SMTP_*` variables gets them imported).

use lettre::message::{Mailbox, MultiPart};
use lettre::transport::smtp::authentication::Credentials;
use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};

#[derive(Clone)]
pub struct Mailer {
    transport: AsyncSmtpTransport<Tokio1Executor>,
    from: lettre::Address,
    /// The sender's name when no person is named ("AlwyWatch").
    pub from_name: String,
    /// For the startup summary: host and port, no secrets.
    pub describe: String,
}

impl std::fmt::Debug for Mailer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.describe)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Security {
    /// TLS from the first byte (port 465).
    Tls,
    /// Plain, then STARTTLS (port 587).
    StartTls,
    /// No encryption (a relay on the same machine only).
    Off,
}

impl Mailer {
    pub fn from_stored(m: &crate::settings::StoredSmtp) -> Result<Self, String> {
        let host = m.host.trim();
        let security = match m.security.trim() {
            "force_tls" | "tls" | "ssl" => Security::Tls,
            "starttls" => Security::StartTls,
            "off" | "none" => Security::Off,
            "" if m.port == Some(465) => Security::Tls,
            "" => Security::StartTls,
            v => {
                return Err(format!(
                    "Email security must be force_tls, starttls or off, got '{v}'"
                ));
            }
        };
        let port = m.port.unwrap_or(match security {
            Security::Tls => 465,
            Security::StartTls => 587,
            Security::Off => 25,
        });
        let from = m.from.trim();
        if from.is_empty() {
            return Err("Email: the address emails are sent from is missing".into());
        }
        let from: lettre::Address = from
            .parse()
            .map_err(|_| format!("Email: '{from}' is not an email address"))?;
        let mut b = match security {
            Security::Tls => AsyncSmtpTransport::<Tokio1Executor>::relay(host),
            Security::StartTls => AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(host),
            Security::Off => Ok(AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(
                host,
            )),
        }
        .map_err(|e| format!("Email server '{host}': {e}"))?
        .port(port)
        .timeout(Some(std::time::Duration::from_secs(20)));
        let user = m
            .username
            .as_deref()
            .map(str::trim)
            .filter(|v| !v.is_empty());
        let pass = m.password.as_deref().filter(|v| !v.is_empty());
        match (user, pass) {
            (Some(u), Some(p)) => b = b.credentials(Credentials::new(u.into(), p.into())),
            (None, None) => {}
            _ => return Err("Email: the username and the password go together".into()),
        }
        Ok(Self {
            transport: b.build(),
            from,
            from_name: m
                .from_name
                .clone()
                .filter(|n| !n.trim().is_empty())
                .unwrap_or_else(|| "AlwyWatch".into()),
            describe: format!("{host}:{port}"),
        })
    }

    /// Connects and signs in, sending nothing (the Test button).
    pub async fn test(&self) -> Result<(), String> {
        match self.transport.test_connection().await {
            Ok(true) => Ok(()),
            Ok(false) => Err("the email server did not answer".into()),
            Err(e) => Err(e.to_string()),
        }
    }

    /// Sends one email with a plain-text and an HTML version. `sender`: shown as the sender's
    /// name ("Alex via AlwyWatch").
    pub async fn send(
        &self,
        to: &str,
        sender: &str,
        subject: &str,
        text: String,
        html: String,
    ) -> Result<(), String> {
        let to: lettre::Address = to.parse().map_err(|_| "not an email address".to_owned())?;
        let m = Message::builder()
            .from(Mailbox::new(Some(sender.to_owned()), self.from.clone()))
            .to(Mailbox::new(None, to))
            .subject(subject)
            .multipart(MultiPart::alternative_plain_html(text, html))
            .map_err(|e| e.to_string())?;
        self.transport
            .send(m)
            .await
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
}

/// A plausible email address (the mail server has the last word).
pub fn valid_address(s: &str) -> bool {
    s.len() <= 254 && s.parse::<lettre::Address>().is_ok()
}

/// HTML-escapes text written into the email.
pub fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}
