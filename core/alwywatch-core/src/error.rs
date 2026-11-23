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
    NotFound,
    Server { status: u16 },
    Network { detail: String },
    Pending,
    Invalid { detail: String },
    NotSignedIn { account_id: String },
    Storage { detail: String },
    Internal { detail: String },
}

impl std::fmt::Display for CoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        use i18n::tr;
        let text = match self {
            CoreError::Unauthorized => {
                tr!("The server refused: your sign-in may have ended. Sign in again in Settings.")
            }
            CoreError::NotFound => tr!("This isn't on the server any more."),
            CoreError::Server { status } => tr!(
                "The server couldn't do this (HTTP {status}). Try again in a moment.",
                status = status
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
