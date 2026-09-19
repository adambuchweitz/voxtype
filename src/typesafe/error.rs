//! Error types for the TypeSafe System One client
//!
//! Mirrors the error taxonomy documented for TypeSafe's official SDKs so that
//! callers can branch on the same cases: authentication, validation, rate
//! limiting, overload, and transport failures.

use std::time::Duration;
use thiserror::Error;

/// Errors returned by the TypeSafe client
#[derive(Debug, Error)]
pub enum TypeSafeError {
    #[error(
        "TypeSafe API key not found. Set TYPESAFE_API_KEY, or pass one to ClientConfig::new().\n  Keys are issued at https://console.typesafe.ai/keys"
    )]
    MissingApiKey,

    #[error("TypeSafe base URL must start with http:// or https://, got: {0}")]
    InvalidBaseUrl(String),

    #[error("Could not build the TypeSafe HTTP client: {0}")]
    ClientBuild(String),

    /// 401. The key is missing, malformed, or revoked.
    #[error("TypeSafe rejected the API key (401). Check TYPESAFE_API_KEY.{}", detail_suffix(.0))]
    Authentication(Option<String>),

    /// 403. The key is valid but not entitled to this call.
    #[error("TypeSafe denied this request (403). The key may lack access to this model or endpoint.{}", detail_suffix(.0))]
    PermissionDenied(Option<String>),

    /// 404.
    #[error("TypeSafe endpoint not found (404). Check the base URL.{}", detail_suffix(.0))]
    NotFound(Option<String>),

    /// 400.
    #[error("TypeSafe rejected the request as malformed (400).{}", detail_suffix(.0))]
    BadRequest(Option<String>),

    /// 422. The request body failed validation; retrying will not help.
    #[error("TypeSafe rejected the request body (422). A question is malformed or a required field is missing.{}", detail_suffix(.0))]
    UnprocessableEntity(Option<String>),

    /// 429. Over the token-per-second or request-per-minute limit.
    #[error("TypeSafe rate limit exceeded (429).{}{}", retry_after_suffix(.retry_after), detail_suffix(.detail))]
    RateLimit {
        retry_after: Option<Duration>,
        detail: Option<String>,
    },

    /// 529. TypeSafe is temporarily overloaded.
    #[error("TypeSafe is overloaded (529). Retry after a short delay.{}", detail_suffix(.0))]
    Overloaded(Option<String>),

    /// Any other 5xx.
    #[error("TypeSafe server error ({status}).{}", detail_suffix(.detail))]
    InternalServer { status: u16, detail: Option<String> },

    /// A status we do not model explicitly.
    #[error("Unexpected TypeSafe response ({status}).{}", detail_suffix(.detail))]
    UnexpectedStatus { status: u16, detail: Option<String> },

    #[error("Could not reach TypeSafe: {0}")]
    Connection(String),

    #[error("TypeSafe request timed out after {0:?}")]
    Timeout(Duration),

    #[error("Could not decode the TypeSafe response: {0}")]
    Decode(String),

    #[error("Could not encode the TypeSafe request: {0}")]
    Encode(String),

    /// A request with no questions would be billed for nothing.
    #[error("A TypeSafe request needs at least one question")]
    EmptyRequest,

    /// A question that cannot be valid, caught before the round-trip.
    #[error("Question '{id}' is invalid: {reason}")]
    InvalidQuestion { id: String, reason: String },

    /// No answer came back under the id the caller asked about.
    #[error("No answer for question id '{0}' in the TypeSafe response")]
    MissingAnswer(String),

    /// An answer came back under a different type than the caller expected.
    #[error("Question '{id}' returned a {actual} answer, expected {expected}")]
    AnswerTypeMismatch {
        id: String,
        expected: &'static str,
        actual: &'static str,
    },
}

impl TypeSafeError {
    /// Whether retrying this error could plausibly succeed.
    ///
    /// Validation failures (422) and auth failures (401/403) are permanent:
    /// the same request will fail the same way. Rate limits, overload, 5xx,
    /// and transport errors are worth another attempt.
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            Self::RateLimit { .. }
                | Self::Overloaded(_)
                | Self::InternalServer { .. }
                | Self::Connection(_)
                | Self::Timeout(_)
        )
    }

    /// The delay the server asked for, when it sent one.
    pub fn retry_after(&self) -> Option<Duration> {
        match self {
            Self::RateLimit { retry_after, .. } => *retry_after,
            _ => None,
        }
    }

    /// Build the right variant for an HTTP status and response body.
    pub(crate) fn from_status(
        status: u16,
        retry_after: Option<Duration>,
        detail: Option<String>,
    ) -> Self {
        match status {
            400 => Self::BadRequest(detail),
            401 => Self::Authentication(detail),
            403 => Self::PermissionDenied(detail),
            404 => Self::NotFound(detail),
            422 => Self::UnprocessableEntity(detail),
            429 => Self::RateLimit {
                retry_after,
                detail,
            },
            529 => Self::Overloaded(detail),
            s if (500..600).contains(&s) => Self::InternalServer { status: s, detail },
            s => Self::UnexpectedStatus { status: s, detail },
        }
    }
}

fn detail_suffix(detail: &Option<String>) -> String {
    match detail {
        Some(d) if !d.trim().is_empty() => format!("\n  {}", d.trim()),
        _ => String::new(),
    }
}

fn retry_after_suffix(retry_after: &Option<Duration>) -> String {
    match retry_after {
        Some(d) => format!(" Retry after {}s.", d.as_secs()),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_documented_statuses() {
        assert!(matches!(
            TypeSafeError::from_status(401, None, None),
            TypeSafeError::Authentication(_)
        ));
        assert!(matches!(
            TypeSafeError::from_status(422, None, None),
            TypeSafeError::UnprocessableEntity(_)
        ));
        assert!(matches!(
            TypeSafeError::from_status(429, None, None),
            TypeSafeError::RateLimit { .. }
        ));
        assert!(matches!(
            TypeSafeError::from_status(529, None, None),
            TypeSafeError::Overloaded(_)
        ));
        assert!(matches!(
            TypeSafeError::from_status(503, None, None),
            TypeSafeError::InternalServer { status: 503, .. }
        ));
    }

    #[test]
    fn validation_and_auth_are_not_retryable() {
        assert!(!TypeSafeError::from_status(422, None, None).is_retryable());
        assert!(!TypeSafeError::from_status(401, None, None).is_retryable());
        assert!(TypeSafeError::from_status(429, None, None).is_retryable());
        assert!(TypeSafeError::from_status(529, None, None).is_retryable());
    }

    #[test]
    fn detail_is_appended_when_present() {
        let err = TypeSafeError::from_status(422, None, Some("questions.urgency: missing".into()));
        assert!(err.to_string().contains("questions.urgency: missing"));
    }
}
