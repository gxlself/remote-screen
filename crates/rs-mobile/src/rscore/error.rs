//! Errors shared by every RsCore uniffi export.

use std::fmt::Display;

use super::store::StoreError;

#[derive(Debug, Clone, PartialEq, thiserror::Error, uniffi::Error)]
pub enum CoreError {
    #[error("network: {message}")]
    Network { message: String },
    #[error("http {status}: {message}")]
    Http {
        status: u16,
        hub_code: Option<String>,
        message: String,
    },
    #[error("conflict: {message}")]
    Conflict {
        message: String,
        current_json: Option<String>,
    },
    #[error("unsupported: {message}")]
    Unsupported { message: String },
    #[error("cancelled: {message}")]
    Cancelled { message: String },
    #[error("internal: {message}")]
    Internal { message: String },
}

impl CoreError {
    pub fn internal(message: impl Display) -> Self {
        Self::Internal {
            message: message.to_string(),
        }
    }

    pub fn network(message: impl Display) -> Self {
        Self::Network {
            message: message.to_string(),
        }
    }

    pub fn unsupported(message: impl Display) -> Self {
        Self::Unsupported {
            message: message.to_string(),
        }
    }

    pub fn cancelled(message: impl Display) -> Self {
        Self::Cancelled {
            message: message.to_string(),
        }
    }
}

impl From<StoreError> for CoreError {
    fn from(error: StoreError) -> Self {
        Self::internal(error)
    }
}

impl From<serde_json::Error> for CoreError {
    fn from(error: serde_json::Error) -> Self {
        Self::internal(format!("bad json: {error}"))
    }
}

impl From<std::io::Error> for CoreError {
    fn from(error: std::io::Error) -> Self {
        Self::internal(error)
    }
}
