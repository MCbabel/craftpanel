use std::borrow::Cow;

use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

#[derive(Debug)]
pub struct Fault {
    status: StatusCode,
    code: &'static str,
    message: Cow<'static, str>,
}

pub type Answer<T> = Result<T, Fault>;

impl Fault {
    pub fn new(
        status: StatusCode,
        code: &'static str,
        message: impl Into<Cow<'static, str>>,
    ) -> Self {
        Self { status, code, message: message.into() }
    }

    pub fn unauthenticated() -> Self {
        Self::new(StatusCode::UNAUTHORIZED, "unauthenticated", "no or expired session")
    }

    pub fn forbidden() -> Self {
        Self::new(StatusCode::FORBIDDEN, "forbidden", "you are missing a permission for this")
    }

    pub fn server_not_found() -> Self {
        Self::new(StatusCode::NOT_FOUND, "server_not_found", "no such server")
    }

    pub fn not_found(code: &'static str, message: impl Into<Cow<'static, str>>) -> Self {
        Self::new(StatusCode::NOT_FOUND, code, message)
    }

    pub fn invalid_request(message: impl Into<Cow<'static, str>>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, "invalid_request", message)
    }

    pub fn conflict(code: &'static str, message: impl Into<Cow<'static, str>>) -> Self {
        Self::new(StatusCode::CONFLICT, code, message)
    }

    pub fn code(&self) -> &'static str {
        self.code
    }

    pub fn status(&self) -> StatusCode {
        self.status
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl From<sqlx::Error> for Fault {
    fn from(err: sqlx::Error) -> Self {
        Self::from(anyhow::Error::from(err))
    }
}

impl From<anyhow::Error> for Fault {
    fn from(err: anyhow::Error) -> Self {
        tracing::error!("{err:#}");
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, "internal", "an internal error occurred")
    }
}

impl IntoResponse for Fault {
    fn into_response(self) -> Response {
        let body = serde_json::json!({ "error": self.code, "message": self.message });
        (self.status, axum::Json(body)).into_response()
    }
}

pub struct Params<T>(pub T);

impl<T, S> FromRequestParts<S> for Params<T>
where
    T: serde::de::DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = Fault;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Answer<Self> {
        axum::extract::Query::<T>::from_request_parts(parts, state)
            .await
            .map(|axum::extract::Query(query)| Self(query))
            .map_err(|rejection| Fault::invalid_request(rejection.body_text()))
    }
}

pub struct Route<T>(pub T);

impl<T, S> FromRequestParts<S> for Route<T>
where
    T: serde::de::DeserializeOwned + Send,
    S: Send + Sync,
{
    type Rejection = Fault;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Answer<Self> {
        axum::extract::Path::<T>::from_request_parts(parts, state)
            .await
            .map(|axum::extract::Path(path)| Self(path))
            .map_err(|rejection| Fault::invalid_request(rejection.body_text()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;

    #[tokio::test]
    async fn the_envelope_has_the_two_fields_of_1_7_and_nothing_else() {
        let response = Fault::conflict("server_busy", "a backup is being created").into_response();
        assert_eq!(response.status(), StatusCode::CONFLICT);

        let bytes = to_bytes(response.into_body(), 4096).await.expect("a small body");
        let body: serde_json::Value = serde_json::from_slice(&bytes).expect("json");
        assert_eq!(body["error"], "server_busy");
        assert_eq!(body["message"], "a backup is being created");
        assert_eq!(body.as_object().expect("an object").len(), 2);
    }

    #[test]
    fn an_internal_error_never_reaches_the_caller_verbatim() {
        let fault = Fault::from(anyhow::anyhow!("connection refused to /run/craftpanel/helper.sock"));
        assert_eq!(fault.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(fault.code(), "internal");
        assert!(!fault.message().contains("helper.sock"));
    }
}
