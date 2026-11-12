//! The one error type, with the status it answers.
//!
//! A handler returns an `AppError`; the response layer in `web::security`
//! turns it into an HTML error page, or into `{"error": "<message>"}` under
//! `/api/`. A form that fails validation is a [`FormError`], which the form's
//! page shows again with status 422.

use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};

/// A form field that broke a rule, and the rule in words.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormError {
    pub field: String,
    pub rule: String,
}

impl FormError {
    pub fn new(field: impl Into<String>, rule: impl Into<String>) -> Self {
        Self {
            field: field.into(),
            rule: rule.into(),
        }
    }
}

/// An error with the HTTP status it answers and a message safe to show.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{status}: {message}")]
pub struct AppError {
    pub status: StatusCode,
    pub message: String,
    /// Whole seconds for a `Retry-After` header, on 429 and 503.
    pub retry_after: Option<u64>,
    /// Set for a form that failed validation.
    pub form: Option<FormError>,
}

impl AppError {
    pub fn new(status: StatusCode, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
            retry_after: None,
            form: None,
        }
    }

    pub fn not_found() -> Self {
        Self::new(StatusCode::NOT_FOUND, "There is nothing at this address.")
    }

    pub fn bad_request(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, message)
    }

    pub fn forbidden(message: impl Into<String>) -> Self {
        Self::new(StatusCode::FORBIDDEN, message)
    }

    /// A limit was passed; the message names it and when it resets.
    pub fn too_many_requests(message: impl Into<String>, retry_after: u64) -> Self {
        let mut e = Self::new(StatusCode::TOO_MANY_REQUESTS, message);
        e.retry_after = Some(retry_after.max(1));
        e
    }

    /// The database had no free turn within the wait.
    pub fn busy() -> Self {
        Self::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "The server is busy. Try again in a moment.",
        )
    }

    /// An unexpected failure. The detail goes to the log, never to the page.
    pub fn internal(detail: impl std::fmt::Display) -> Self {
        tracing::error!("internal error: {detail}");
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Something went wrong on our side.",
        )
    }

    /// A form that failed validation: status 422 and the rule that failed.
    pub fn form(error: FormError) -> Self {
        let mut e = Self::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            format!("{}: {}", error.field, error.rule),
        );
        e.form = Some(error);
        e
    }

    /// The error's name, shown as the error page's heading.
    pub fn name(&self) -> String {
        self.status
            .canonical_reason()
            .unwrap_or("Error")
            .to_string()
    }
}

impl From<rusqlite::Error> for AppError {
    fn from(e: rusqlite::Error) -> Self {
        AppError::internal(e)
    }
}

impl From<anyhow::Error> for AppError {
    fn from(e: anyhow::Error) -> Self {
        AppError::internal(format!("{e:#}"))
    }
}

/// An `AppError` leaves a handler as a bare response that carries the error
/// itself. The response layer renders it, because only the layer knows
/// whether the request was for the API or for a page.
impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let mut response = Response::new(axum::body::Body::empty());
        *response.status_mut() = self.status;
        if let Some(secs) = self.retry_after {
            if let Ok(value) = HeaderValue::from_str(&secs.to_string()) {
                response.headers_mut().insert(header::RETRY_AFTER, value);
            }
        }
        response.extensions_mut().insert(self);
        response
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn form_error_is_422_with_its_rule() {
        let e = AppError::form(FormError::new("title", "must be 1 to 120 characters"));
        assert_eq!(e.status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(e.message, "title: must be 1 to 120 characters");
        assert_eq!(e.form.as_ref().map(|f| f.field.as_str()), Some("title"));
    }

    #[test]
    fn retry_after_is_at_least_one_second() {
        let e = AppError::too_many_requests("slow down", 0);
        assert_eq!(e.retry_after, Some(1));
        let response = e.into_response();
        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(response.headers()[header::RETRY_AFTER], "1");
        assert!(response.extensions().get::<AppError>().is_some());
    }

    #[test]
    fn internal_errors_hide_their_detail() {
        let e = AppError::internal("secret path /srv/db");
        assert!(!e.message.contains("secret"));
        assert_eq!(e.name(), "Internal Server Error");
    }
}
