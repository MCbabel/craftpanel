use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Serialize;

#[derive(Debug, thiserror::Error)]
#[error("{code}: {message}")]
pub struct Failure {
    status: StatusCode,
    code: &'static str,
    message: String,
    retry_after: Option<u64>,
    #[source]
    cause: Option<anyhow::Error>,
}

#[derive(Serialize)]
struct Body<'a> {
    error: &'a str,
    message: &'a str,
}

impl Failure {
    pub fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        Self { status, code, message: message.into(), retry_after: None, cause: None }
    }

    pub fn rate_limited(seconds: u64) -> Self {
        Self {
            retry_after: Some(seconds),
            ..Self::new(
                StatusCode::TOO_MANY_REQUESTS,
                "rate_limited",
                format!("too many requests; try again in {seconds} seconds"),
            )
        }
    }

    pub fn unauthenticated() -> Self {
        Self::new(StatusCode::UNAUTHORIZED, "unauthenticated", "sign in first")
    }

    pub fn forbidden() -> Self {
        Self::new(StatusCode::FORBIDDEN, "forbidden", "you may not do that")
    }

    pub fn invalid_request(message: impl Into<String>) -> Self {
        Self::bad_request("invalid_request", message)
    }

    pub fn bad_request(code: &'static str, message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, code, message)
    }

    pub fn not_found(code: &'static str, message: impl Into<String>) -> Self {
        Self::new(StatusCode::NOT_FOUND, code, message)
    }

    pub fn conflict(code: &'static str, message: impl Into<String>) -> Self {
        Self::new(StatusCode::CONFLICT, code, message)
    }

    pub fn internal(cause: impl Into<anyhow::Error>) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            code: "internal",
            message: "an internal error occurred".to_owned(),
            retry_after: None,
            cause: Some(cause.into()),
        }
    }

    pub fn code(&self) -> &'static str {
        self.code
    }

    pub fn status(&self) -> StatusCode {
        self.status
    }
}

impl IntoResponse for Failure {
    fn into_response(self) -> Response {
        if let Some(cause) = &self.cause {
            tracing::error!("{cause:#}");
        }
        let body = Body { error: self.code, message: &self.message };
        let mut response = (self.status, Json(body)).into_response();
        if let Some(seconds) = self.retry_after {
            response.headers_mut().insert(axum::http::header::RETRY_AFTER, seconds.into());
        }
        response
    }
}

impl From<sqlx::Error> for Failure {
    fn from(err: sqlx::Error) -> Self {
        Self::internal(err)
    }
}

impl From<anyhow::Error> for Failure {
    fn from(err: anyhow::Error) -> Self {
        Self::internal(err)
    }
}

pub type Result<T> = std::result::Result<T, Failure>;

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn the_body_carries_the_contract_code_and_nothing_else() {
        let response = Failure::conflict("username_taken", "anna is taken").into_response();
        assert_eq!(response.status(), StatusCode::CONFLICT);

        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(json["error"], "username_taken");
        assert_eq!(json["message"], "anna is taken");
        assert_eq!(json.as_object().unwrap().len(), 2, "1.7 allows exactly two fields");
    }

    #[tokio::test]
    async fn a_brake_says_how_long_to_wait() {
        let response = Failure::rate_limited(42).into_response();
        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(
            response.headers().get(axum::http::header::RETRY_AFTER).unwrap(),
            "42",
            "1.7 asks for Retry-After in seconds"
        );

        let plain = Failure::conflict("username_taken", "taken").into_response();
        assert!(!plain.headers().contains_key(axum::http::header::RETRY_AFTER));
    }

    #[tokio::test]
    async fn an_internal_error_keeps_its_cause_to_itself() {
        let failure = Failure::internal(anyhow::anyhow!("the password file is on fire"));
        let response = failure.into_response();
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);

        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let text = String::from_utf8(bytes.to_vec()).unwrap();
        assert!(!text.contains("on fire"), "the cause is for the log, not the caller: {text}");
    }
}
