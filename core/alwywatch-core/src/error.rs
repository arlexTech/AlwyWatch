// SPDX-License-Identifier: AGPL-3.0-or-later
use net::Error as NetError;

/// Errors surfaced to the UIs, worded for the person in front of the screen (the UIs show
/// them as they are). Messages never contain tokens.
/// Flat for the bindings: Kotlin gets one exception class per variant whose `message` is the
/// text below (fields stay available to Rust callers such as the desktop app).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error, uniffi::Error)]
#[uniffi(flat_error)]
pub enum CoreError {
    #[error("The server refused: your sign-in may have ended. Sign in again in Settings.")]
    Unauthorized,
    #[error("This isn't on the server any more.")]
    NotFound,
    #[error("The server couldn't do this (HTTP {status}). Try again in a moment.")]
    Server { status: u16 },
    #[error("Can't reach the server. Check your connection.")]
    Network { detail: String },
    #[error("Still waiting for approval.")]
    Pending,
    #[error("{detail}")]
    Invalid { detail: String },
    #[error("This sign-in is no longer on this device. Sign in again in Settings.")]
    NotSignedIn { account_id: String },
    #[error("This device's storage had a problem: {detail}")]
    Storage { detail: String },
    #[error("Something went wrong in the app: {detail}")]
    Internal { detail: String },
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
