// SPDX-License-Identifier: AGPL-3.0-or-later
use net::Error as NetError;

/// Errors surfaced to the UIs. Messages never contain tokens.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error, uniffi::Error)]
pub enum CoreError {
    #[error("sign in again: the session expired or was revoked")]
    Unauthorized,
    #[error("not found")]
    NotFound,
    #[error("server error (HTTP {status})")]
    Server { status: u16 },
    #[error("network unavailable: {detail}")]
    Network { detail: String },
    #[error("waiting for authorization")]
    Pending,
    #[error("{detail}")]
    Invalid { detail: String },
    #[error("account {account_id} is not signed in on this device")]
    NotSignedIn { account_id: String },
    #[error("local storage error: {detail}")]
    Storage { detail: String },
    #[error("internal error: {detail}")]
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
