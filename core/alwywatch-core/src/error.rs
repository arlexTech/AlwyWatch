// SPDX-License-Identifier: AGPL-3.0-or-later
use net::Error as NetError;

/// Errors surfaced to the UIs, worded for the person in front of the screen in the selected
/// language (the UIs show them as they are). Messages never contain tokens.
/// Flat for the bindings: Kotlin gets one exception class per variant whose `message` is the
/// text below (fields stay available to Rust callers such as the desktop app). Apps that
/// read the status back look for "HTTP <status>", which every translation keeps.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Error)]
#[uniffi(flat_error)]
pub enum CoreError {
    Unauthorized,
    /// Signed in, but the media server doesn't allow this account to do it (HTTP 403).
    Forbidden,
    NotFound,
    Server {
        status: u16,
    },
    Network {
        detail: String,
    },
    Pending,
    Invalid {
        detail: String,
    },
    NotSignedIn {
        account_id: String,
    },
    Storage {
        detail: String,
    },
    Internal {
        detail: String,
    },
}

impl std::fmt::Display for CoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        use i18n::tr;
        let text = match self {
            CoreError::Unauthorized => {
                tr!("The server refused: your sign-in may have ended. Sign in again in Settings.")
            }
            CoreError::Forbidden => tr!(
                "Your account on this server isn't allowed to do this (HTTP 403). Its administrator can change that."
            ),
            CoreError::NotFound => tr!("This isn't on the server any more."),
            CoreError::Server { status } => tr!(
                "The server couldn't do this (HTTP {status}). Try again in a moment.",
                status = status
            ),
            // A certificate "not valid yet" or "expired" is almost always this device's clock
            // (TV boxes often have it wrong): "check your connection" would mislead.
            CoreError::Network { detail } if clock_problem(detail) => tr!(
                "The server's security certificate doesn't match this device's date and time. Check the date and time in this device's settings."
            ),
            CoreError::Network { .. } => tr!("Can't reach the server. Check your connection."),
            CoreError::Pending => tr!("Still waiting for approval."),
            CoreError::Invalid { detail } => detail.clone(),
            CoreError::NotSignedIn { .. } => {
                tr!("This sign-in is no longer on this device. Sign in again in Settings.")
            }
            CoreError::Storage { detail } => {
                tr!(
                    "This device's storage had a problem: {detail}",
                    detail = detail
                )
            }
            CoreError::Internal { detail } => {
                tr!("Something went wrong in the app: {detail}", detail = detail)
            }
        };
        f.write_str(&text)
    }
}

impl std::error::Error for CoreError {}

/// A TLS failure that comes from the date (rustls: `NotValidYet`, `Expired`, or their
/// "not valid yet" / "expired" wording).
fn clock_problem(detail: &str) -> bool {
    let d = detail.to_ascii_lowercase();
    d.contains("notvalidyet")
        || d.contains("not valid yet")
        || (d.contains("certificate") && d.contains("expired"))
}

impl CoreError {
    pub fn is_transient(&self) -> bool {
        match self {
            CoreError::Network { .. } => true,
            CoreError::Server { status } => *status >= 500 || *status == 429 || *status == 408,
            _ => false,
        }
    }
}

impl From<NetError> for CoreError {
    fn from(e: NetError) -> Self {
        match e {
            NetError::Unauthorized => CoreError::Unauthorized,
            NetError::Forbidden => CoreError::Forbidden,
            NetError::NotFound => CoreError::NotFound,
            NetError::Status { status } => CoreError::Server { status },
            NetError::Network { message } => CoreError::Network { detail: message },
            NetError::Decode { message } => CoreError::Invalid {
                detail: format!("unexpected server response: {message}"),
            },
            NetError::Invalid { message } => CoreError::Invalid { detail: message },
            NetError::Pending => CoreError::Pending,
        }
    }
}

impl From<store::Error> for CoreError {
    fn from(e: store::Error) -> Self {
        CoreError::Storage {
            detail: e.to_string(),
        }
    }
}

pub type Result<T, E = CoreError> = std::result::Result<T, E>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_wrong_clock_is_named_as_such() {
        let tls = |m: &str| CoreError::Network {
            detail: format!(
                "error sending request: client error (Connect): invalid peer certificate: {m}"
            ),
        };
        assert!(tls("NotValidYet").to_string().contains("date and time"));
        assert!(tls("Expired").to_string().contains("date and time"));
        assert!(!tls("UnknownIssuer").to_string().contains("date and time"));
        let refused = CoreError::Network {
            detail: "error sending request: Connection refused".into(),
        };
        assert!(refused.to_string().contains("Check your connection"));
    }
}
