// SPDX-License-Identifier: LicenseRef-CCBG-Commercial
// Copyright (c) 2026 walky

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    BadRequest,
    Unauthorized,
    NotFound,
    UpstreamUnavailable,
    Timeout,
    NotImplemented,
    Internal,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpErrorPayload {
    pub code: ErrorCode,
    pub message: String,
    pub retryable: bool,
}

impl McpErrorPayload {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            retryable: code.is_retryable(),
            message: message.into(),
        }
    }
}

impl ErrorCode {
    pub const fn is_retryable(self) -> bool {
        matches!(
            self,
            Self::UpstreamUnavailable | Self::Timeout | Self::Internal
        )
    }
}

#[derive(Debug, Error)]
pub enum ServerError {
    #[error("bad request: {0}")]
    BadRequest(String),
    #[error("unauthorized: {0}")]
    Unauthorized(String),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("upstream unavailable: {0}")]
    UpstreamUnavailable(String),
    #[error("timeout: {0}")]
    Timeout(String),
    #[error("not implemented: {0}")]
    NotImplemented(String),
    #[error("internal: {0}")]
    Internal(String),
}

impl ServerError {
    pub fn to_payload(&self) -> McpErrorPayload {
        match self {
            Self::BadRequest(msg) => McpErrorPayload::new(ErrorCode::BadRequest, msg),
            Self::Unauthorized(msg) => McpErrorPayload::new(ErrorCode::Unauthorized, msg),
            Self::NotFound(msg) => McpErrorPayload::new(ErrorCode::NotFound, msg),
            Self::UpstreamUnavailable(msg) => {
                McpErrorPayload::new(ErrorCode::UpstreamUnavailable, msg)
            }
            Self::Timeout(msg) => McpErrorPayload::new(ErrorCode::Timeout, msg),
            Self::NotImplemented(msg) => McpErrorPayload::new(ErrorCode::NotImplemented, msg),
            Self::Internal(msg) => McpErrorPayload::new(ErrorCode::Internal, msg),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ErrorCode, ServerError};

    #[test]
    fn retryable_flags_match_contract() {
        assert!(!ErrorCode::BadRequest.is_retryable());
        assert!(!ErrorCode::Unauthorized.is_retryable());
        assert!(!ErrorCode::NotFound.is_retryable());
        assert!(ErrorCode::UpstreamUnavailable.is_retryable());
        assert!(ErrorCode::Timeout.is_retryable());
        assert!(!ErrorCode::NotImplemented.is_retryable());
        assert!(ErrorCode::Internal.is_retryable());
    }

    #[test]
    fn server_error_mapping_is_machine_readable() {
        let mapped = ServerError::Timeout("deadline exceeded".into()).to_payload();
        assert_eq!(mapped.code, ErrorCode::Timeout);
        assert!(mapped.retryable);
        assert_eq!(mapped.message, "deadline exceeded");
    }
}
