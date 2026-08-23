use axum::extract::{FromRequest, FromRequestParts, Request};
use axum::http::header::{CONTENT_TYPE, HOST, ORIGIN, SET_COOKIE, USER_AGENT};
use axum::http::request::Parts;
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, Uri};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum_extra::extract::CookieJar;

use super::error::{Failure, Result};
use super::session::{self, Session};
use super::users::{self, UserRow};
use crate::model::{PanelRole, SessionRef, Timestamp};
use crate::AppState;

#[derive(Debug, Clone)]
pub struct Caller {
    pub user: UserRow,
    pub session: Session,
    pub secure: bool,
}

impl Caller {
    pub fn id(&self) -> crate::model::Id {
        self.user.id
    }

    pub fn is_admin(&self) -> bool {
        self.user.role == PanelRole::Admin
    }

    pub fn session_ref(&self) -> SessionRef {
        SessionRef { id: self.session.id, expires_at: self.session.expires_at }
    }
}

impl FromRequestParts<AppState> for Caller {
    type Rejection = Failure;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self> {
        let jar = CookieJar::from_headers(&parts.headers);
        let secret = jar.get(session::COOKIE).ok_or_else(Failure::unauthenticated)?;

        let session = session::lookup(&state.pool, secret.value(), Timestamp::now())
            .await?
            .ok_or_else(Failure::unauthenticated)?;

        let user = users::find(&state.pool, session.user_id)
            .await?
            .ok_or_else(Failure::unauthenticated)?;

        Ok(Self { user, session, secure: arrived_over_tls(parts) })
    }
}

#[derive(Debug, Clone)]
pub struct Admin(pub Caller);

impl FromRequestParts<AppState> for Admin {
    type Rejection = Failure;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self> {
        let caller = Caller::from_request_parts(parts, state).await?;
        if !caller.is_admin() {
            return Err(Failure::forbidden());
        }
        Ok(Self(caller))
    }
}

#[derive(Debug, Clone, Copy)]
pub struct JsonBody<T>(pub T);

impl<T> FromRequest<AppState> for JsonBody<T>
where
    T: serde::de::DeserializeOwned,
{
    type Rejection = Failure;

    async fn from_request(request: Request, state: &AppState) -> Result<Self> {
        if !is_json(request.headers()) {
            return Err(Failure::new(
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
                "unsupported_media_type",
                "this endpoint reads application/json",
            ));
        }

        match axum::Json::<T>::from_request(request, state).await {
            Ok(axum::Json(body)) => Ok(Self(body)),
            Err(rejection) => Err(Failure::invalid_request(rejection.body_text())),
        }
    }
}

impl<T> axum::extract::OptionalFromRequest<AppState> for JsonBody<T>
where
    T: serde::de::DeserializeOwned,
{
    type Rejection = Failure;

    async fn from_request(request: Request, state: &AppState) -> Result<Option<Self>> {
        if request.headers().get(CONTENT_TYPE).is_none() {
            return Ok(None);
        }
        <Self as FromRequest<AppState>>::from_request(request, state).await.map(Some)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Params<T>(pub T);

impl<T> FromRequestParts<AppState> for Params<T>
where
    T: serde::de::DeserializeOwned,
{
    type Rejection = Failure;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self> {
        match axum::extract::Query::<T>::from_request_parts(parts, state).await {
            Ok(axum::extract::Query(query)) => Ok(Self(query)),
            Err(rejection) => Err(Failure::invalid_request(rejection.body_text())),
        }
    }
}

fn is_json(headers: &HeaderMap) -> bool {
    headers
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .is_some_and(|kind| kind.trim().eq_ignore_ascii_case("application/json"))
}

pub fn user_agent(parts: &Parts) -> Option<&str> {
    parts.headers.get(USER_AGENT).and_then(|value| value.to_str().ok())
}

pub fn arrived_over_tls(parts: &Parts) -> bool {
    over_tls(&parts.headers, &parts.uri)
}

fn over_tls(headers: &HeaderMap, uri: &Uri) -> bool {
    if uri.scheme_str() == Some("https") {
        return true;
    }
    headers
        .get("x-forwarded-proto")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|proto| proto.split(',').next().is_some_and(|first| first.trim() == "https"))
}

pub async fn same_origin(request: Request, next: Next) -> Response {
    let changes_something =
        !matches!(*request.method(), Method::GET | Method::HEAD | Method::OPTIONS);

    if changes_something {
        if let Some(origin) = request.headers().get(ORIGIN).and_then(|v| v.to_str().ok()) {
            let host = request.headers().get(HOST).and_then(|v| v.to_str().ok());
            if !origin_matches(origin, host) {
                return Failure::new(
                    StatusCode::FORBIDDEN,
                    "csrf_origin_mismatch",
                    format!("{origin} is not this panel"),
                )
                .into_response();
            }
        }
    }

    next.run(request).await
}

fn origin_matches(origin: &str, host: Option<&str>) -> bool {
    let Some(authority) = origin.split("://").nth(1) else {
        return false;
    };
    host.is_some_and(|host| host.eq_ignore_ascii_case(authority))
}

pub async fn slide_sessions(
    axum::extract::State(state): axum::extract::State<AppState>,
    request: Request,
    next: Next,
) -> Response {
    let secret = CookieJar::from_headers(request.headers())
        .get(session::COOKIE)
        .map(|cookie| cookie.value().to_owned());
    let secure = over_tls(request.headers(), request.uri());

    let mut response = next.run(request).await;

    let Some(secret) = secret else {
        return response;
    };
    let now = Timestamp::now();
    let slid = match session::lookup(&state.pool, &secret, now).await {
        Ok(Some(current)) => session::refresh(&state.pool, &current, now).await,
        Ok(None) => Ok(None),
        Err(err) => Err(err),
    };

    match slid {
        Ok(Some(_)) => {
            let cookie = session::cookie(secret, secure).to_string();
            if let Ok(value) = HeaderValue::from_str(&cookie) {
                response.headers_mut().append(SET_COOKIE, value);
            }
        }
        Ok(None) => {}
        Err(err) => tracing::warn!("a session could not be slid along: {err}"),
    }

    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_form_is_not_json() {
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, "application/json".parse().unwrap());
        assert!(is_json(&headers));

        headers.insert(CONTENT_TYPE, "application/json; charset=utf-8".parse().unwrap());
        assert!(is_json(&headers));

        for wrong in
            ["application/x-www-form-urlencoded", "multipart/form-data; boundary=x", "text/plain"]
        {
            headers.insert(CONTENT_TYPE, wrong.parse().unwrap());
            assert!(!is_json(&headers), "{wrong}");
        }
    }

    #[tokio::test]
    async fn a_session_older_than_an_hour_is_slid_along_and_the_browser_told() {
        use crate::auth::harness::{a_user, as_user, fetch, set_cookie, state, test_pool};
        use crate::auth::session;
        use crate::model::Timestamp;
        use tower::ServiceExt;

        let pool = test_pool().await;
        let max = a_user(&pool, "max").await;
        let now = Timestamp::now();
        let (opened, secret) = session::open(&pool, max, None, now).await.unwrap();

        let app = axum::Router::new()
            .route("/me", axum::routing::get(|| async { "hello" }))
            .layer(axum::middleware::from_fn_with_state(state(&pool), slide_sessions))
            .with_state(state(&pool));

        let fresh = app.clone().oneshot(as_user(fetch("/me"), &secret)).await.unwrap();
        assert_eq!(set_cookie(&fresh), None, "nothing to write in the first hour");

        let two_hours_ago = Timestamp::at(now.as_datetime() - time::Duration::hours(2));
        sqlx::query("UPDATE sessions SET last_seen = ?, expires_at = ? WHERE id = ?")
            .bind(two_hours_ago)
            .bind(Timestamp::at(opened.expires_at.as_datetime() - time::Duration::hours(2)))
            .bind(opened.id)
            .execute(&pool)
            .await
            .unwrap();

        let before = Timestamp::now();
        let later = app.oneshot(as_user(fetch("/me"), &secret)).await.unwrap();
        let after = Timestamp::now();
        let cookie = set_cookie(&later).expect("the thirty days moved, so the cookie must too");
        assert!(cookie.contains(&secret), "the same secret, a later date: {cookie}");

        let (expires, seen): (Timestamp, Timestamp) =
            sqlx::query_as("SELECT expires_at, last_seen FROM sessions WHERE id = ?")
                .bind(opened.id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!(seen > two_hours_ago);
        assert!(
            (before..=after).contains(&seen),
            "the request itself is the moment: {before} <= {seen} <= {after}"
        );
        assert_eq!(
            expires,
            Timestamp::at(seen.as_datetime() + session::LIFETIME),
            "thirty days from that moment again"
        );
    }

    #[tokio::test]
    async fn a_cookie_that_names_no_session_is_left_alone() {
        use crate::auth::harness::{as_user, fetch, set_cookie, state, test_pool};
        use tower::ServiceExt;

        let pool = test_pool().await;
        let app = axum::Router::new()
            .route("/me", axum::routing::get(|| async { "hello" }))
            .layer(axum::middleware::from_fn_with_state(state(&pool), slide_sessions))
            .with_state(state(&pool));

        let response = app.oneshot(as_user(fetch("/me"), "nothing-of-ours")).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(set_cookie(&response), None);
    }

    #[test]
    fn an_origin_belongs_to_the_host_it_names() {
        assert!(origin_matches("http://panel.example:8080", Some("panel.example:8080")));
        assert!(origin_matches("https://panel.example", Some("panel.example")));
        assert!(origin_matches("https://PANEL.example", Some("panel.example")));

        assert!(!origin_matches("https://evil.example", Some("panel.example")));
        assert!(!origin_matches("https://panel.example", Some("panel.example:8080")));
        assert!(!origin_matches("null", Some("panel.example")));
        assert!(!origin_matches("https://panel.example", None));
    }
}
