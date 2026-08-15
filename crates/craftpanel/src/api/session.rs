use std::net::{IpAddr, SocketAddr};
use std::time::Instant;

use axum::extract::{ConnectInfo, State};
use axum::http::request::Parts;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use axum_extra::extract::CookieJar;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

use crate::auth::error::{Failure, Result};
use crate::auth::session::Session;
use crate::auth::users::UserRow;
use crate::auth::{brake, extract, password, reset, session, users};
use crate::auth::{Caller, Disks, JsonBody, LiveServers, Params};
use crate::model::{Me, RegistrationState, Timestamp, UserRef};
use crate::registration;
use crate::AppState;

const SEARCH_LIMIT: u32 = 25;

pub fn router() -> Router<AppState> {
    with_live(LiveServers::none(), Disks::none())
}

pub fn with_live(live: LiveServers, disks: Disks) -> Router<AppState> {
    Router::new()
        .route("/auth/login", post(login))
        .route("/auth/logout", post(logout))
        .route("/me", get(me))
        .route("/me/password", post(change_password))
        .route("/users/search", get(search))
        .layer(Extension(live))
        .layer(Extension(disks))
        .layer(axum::middleware::from_fn(extract::same_origin))
}

#[derive(Deserialize)]
struct LoginRequest {
    username: String,
    password: String,
}

#[derive(Deserialize)]
struct ChangePasswordRequest {
    current_password: String,
    new_password: String,
}

#[derive(Deserialize)]
struct SearchQuery {
    query: String,
    limit: Option<u32>,
}

#[derive(Serialize)]
struct UserSearchResponse {
    users: Vec<UserRef>,
}

async fn login(
    State(state): State<AppState>,
    Extension(live): Extension<LiveServers>,
    Extension(disks): Extension<Disks>,
    jar: CookieJar,
    parts: Parts,
    JsonBody(body): JsonBody<LoginRequest>,
) -> Result<Response> {
    let username = body.username.trim().to_lowercase();
    let address = caller_address(&parts);
    brake::shared().check(&username, address, Instant::now())?;

    let found = users::by_name(&state.pool, &username).await?;
    let row = match found {
        Some(row) if password::verify(&body.password, &row.password_hash) => row,
        Some(_) => return Err(refuse(&username, address)),
        None => match registration::store::credentials(&state.pool, &username).await? {
            Some((hash, pending)) if password::verify(&body.password, &hash) => {
                return Err(refuse_half_an_account(pending))
            }
            Some(_) => return Err(refuse(&username, address)),
            None => {
                password::verify_against_nobody(&body.password);
                return Err(refuse(&username, address));
            }
        },
    };
    brake::shared().forget(&username, address);

    let now = Timestamp::now();
    session::purge_expired(&state.pool, now).await?;
    let (opened, secret) = session::open(&state.pool, row.id, extract::user_agent(&parts), now).await?;

    sqlx::query("UPDATE users SET last_login_at = ? WHERE id = ?")
        .bind(now)
        .bind(row.id)
        .execute(&state.pool)
        .await?;
    let row = UserRow { last_login_at: Some(now), ..row };

    let body = whoami(&state.pool, &row, &opened, &live, &disks).await?;
    let cookie = session::cookie(secret, extract::arrived_over_tls(&parts));
    Ok((jar.add(cookie), Json(body)).into_response())
}

fn refuse(username: &str, address: Option<IpAddr>) -> Failure {
    brake::shared().note_failure(username, address, Instant::now());
    Failure::new(StatusCode::UNAUTHORIZED, "invalid_credentials", "wrong name or password")
}

fn refuse_half_an_account(state: RegistrationState) -> Failure {
    match state {
        RegistrationState::EmailUnverified => Failure::new(
            StatusCode::FORBIDDEN,
            "email_unverified",
            "check your inbox: your address has not been confirmed yet",
        ),
        RegistrationState::AwaitingApproval => Failure::new(
            StatusCode::FORBIDDEN,
            "approval_pending",
            "an administrator still has to let this account in",
        ),
    }
}

async fn logout(State(state): State<AppState>, jar: CookieJar, parts: Parts) -> Result<Response> {
    if let Some(cookie) = jar.get(session::COOKIE) {
        if let Some(current) =
            session::lookup(&state.pool, cookie.value(), Timestamp::now()).await?
        {
            session::close(&state.pool, current.id).await?;
        }
    }

    let cleared = session::cleared_cookie(extract::arrived_over_tls(&parts));
    Ok((jar.remove(cleared), StatusCode::NO_CONTENT).into_response())
}

async fn me(
    State(state): State<AppState>,
    Extension(live): Extension<LiveServers>,
    Extension(disks): Extension<Disks>,
    caller: Caller,
) -> Result<Json<Me>> {
    Ok(Json(whoami(&state.pool, &caller.user, &caller.session, &live, &disks).await?))
}

async fn change_password(
    State(state): State<AppState>,
    jar: CookieJar,
    caller: Caller,
    JsonBody(body): JsonBody<ChangePasswordRequest>,
) -> Result<Response> {
    if !password::verify(&body.current_password, &caller.user.password_hash) {
        return Err(Failure::new(
            StatusCode::FORBIDDEN,
            "wrong_password",
            "the current password does not match",
        ));
    }

    let hash = password::hash(&body.new_password)?;
    sqlx::query(
        "UPDATE users SET password_hash = ?, must_change_password = 0, updated_at = ? WHERE id = ?",
    )
    .bind(hash)
    .bind(Timestamp::now())
    .bind(caller.id())
    .execute(&state.pool)
    .await?;

    session::close_all_of(&state.pool, caller.id(), None).await?;
    reset::forget_all(&state.pool, caller.id()).await?;
    let (_, secret) = session::open(&state.pool, caller.id(), None, Timestamp::now()).await?;

    let cookie = session::cookie(secret, caller.secure);
    Ok((jar.add(cookie), StatusCode::NO_CONTENT).into_response())
}

async fn search(
    State(state): State<AppState>,
    _caller: Caller,
    Params(query): Params<SearchQuery>,
) -> Result<Json<UserSearchResponse>> {
    if query.query.trim().is_empty() {
        return Err(Failure::invalid_request("query needs at least one character"));
    }
    let users = users::search(&state.pool, query.query.trim(), how_many(query.limit)).await?;
    Ok(Json(UserSearchResponse { users }))
}

fn how_many(asked: Option<u32>) -> u32 {
    asked.unwrap_or(SEARCH_LIMIT).clamp(1, SEARCH_LIMIT)
}

async fn whoami(
    pool: &SqlitePool,
    row: &UserRow,
    session: &Session,
    live: &LiveServers,
    disks: &Disks,
) -> Result<Me> {
    let user = users::panel_user(pool, row, live, disks).await?;
    let capabilities = users::capabilities(row, &user.usage);
    Ok(Me {
        user,
        capabilities,
        session: crate::model::SessionRef { id: session.id, expires_at: session.expires_at },
    })
}

fn caller_address(parts: &Parts) -> Option<IpAddr> {
    parts.extensions.get::<ConnectInfo<SocketAddr>>().map(|info| info.0.ip())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::harness::*;
    use crate::model::Id;
    use axum::body::Body;
    use axum::http::header::{CONTENT_TYPE, ORIGIN};
    use axum::http::Request;
    use tower::ServiceExt;

    fn app(pool: &SqlitePool) -> Router {
        router().with_state(state(pool))
    }

    fn app_with(pool: &SqlitePool, disks: Disks) -> Router {
        with_live(LiveServers::none(), disks).with_state(state(pool))
    }

    fn credentials(username: &str, secret: &str) -> serde_json::Value {
        serde_json::json!({ "username": username, "password": secret })
    }

    async fn a_session_for(pool: &SqlitePool, username: &str) -> String {
        let id = a_user(pool, username).await;
        sign_in(pool, id).await
    }

    #[tokio::test]
    async fn signing_in_answers_with_the_account_and_a_cookie() {
        let pool = test_pool().await;
        a_user(&pool, "max").await;

        let response = app(&pool)
            .oneshot(send("POST", "/auth/login", credentials("max", PASSWORD)))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let cookie = set_cookie(&response).expect("a session cookie");
        assert!(cookie.starts_with("craft_session="), "{cookie}");
        assert!(cookie.contains("HttpOnly"), "{cookie}");
        assert!(cookie.contains("SameSite=Lax"), "{cookie}");
        assert!(!cookie.contains("Secure"), "plain http: {cookie}");

        let body = body_json(response).await;
        assert_eq!(body["username"], "max");
        assert_eq!(body["panel_role"], "user");
        assert_eq!(body["avatar_url"], serde_json::Value::Null);
        assert!(body["session"]["id"].is_string());
        assert!(body["capabilities"]["can_create_servers"].as_bool().unwrap());
        assert!(body["last_login_at"].is_string(), "the sign-in that just happened");
        assert!(body.get("password_hash").is_none(), "{body}");
    }

    #[tokio::test]
    async fn the_name_of_the_wrong_password_never_reaches_the_answer() {
        let pool = test_pool().await;
        a_user(&pool, "max").await;

        let wrong = app(&pool)
            .oneshot(send("POST", "/auth/login", credentials("max", "wrong-password")))
            .await
            .unwrap();
        assert_eq!(wrong.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(body_json(wrong).await["error"], "invalid_credentials");

        let unknown = app(&pool)
            .oneshot(send("POST", "/auth/login", credentials("nobody", PASSWORD)))
            .await
            .unwrap();
        assert_eq!(unknown.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(body_json(unknown).await["error"], "invalid_credentials");
    }

    #[tokio::test]
    async fn an_unknown_name_costs_the_same_hashing_as_a_wrong_password() {
        let pool = test_pool().await;
        a_user(&pool, "max").await;
        password::verify_against_nobody("warm");

        let before = password::argon2_runs();
        app(&pool)
            .oneshot(send("POST", "/auth/login", credentials("max", "wrong-password")))
            .await
            .unwrap();
        let for_a_wrong_password = password::argon2_runs() - before;

        let before = password::argon2_runs();
        app(&pool)
            .oneshot(send("POST", "/auth/login", credentials("nobody-at-all", PASSWORD)))
            .await
            .unwrap();
        let for_an_unknown_name = password::argon2_runs() - before;

        assert_eq!(for_a_wrong_password, 1);
        assert_eq!(for_an_unknown_name, 1, "the unknown name skipped the work and told on itself");
    }

    #[tokio::test]
    async fn ten_wrong_tries_close_the_door_on_that_account() {
        let pool = test_pool().await;
        let name = format!("max{}", Id::new().to_string().to_lowercase());
        a_user(&pool, &name).await;

        for _ in 0..10 {
            let response = app(&pool)
                .oneshot(send("POST", "/auth/login", credentials(&name, "wrong-password")))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        }

        let response =
            app(&pool).oneshot(send("POST", "/auth/login", credentials(&name, PASSWORD))).await.unwrap();
        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(body_json(response).await["error"], "too_many_attempts");
    }

    #[tokio::test]
    async fn a_body_that_is_not_json_is_refused_by_media_type() {
        let pool = test_pool().await;
        let request = Request::builder()
            .method("POST")
            .uri("/auth/login")
            .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
            .body(Body::from("username=max&password=whatever"))
            .unwrap();

        let response = app(&pool).oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
        assert_eq!(body_json(response).await["error"], "unsupported_media_type");
    }

    #[tokio::test]
    async fn a_missing_field_is_an_invalid_request() {
        let pool = test_pool().await;
        let response = app(&pool)
            .oneshot(send("POST", "/auth/login", serde_json::json!({ "username": "max" })))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(body_json(response).await["error"], "invalid_request");
    }

    #[tokio::test]
    async fn a_request_from_somewhere_else_is_refused() {
        let pool = test_pool().await;
        a_user(&pool, "max").await;

        let mut request = send("POST", "/auth/login", credentials("max", PASSWORD));
        request.headers_mut().insert(ORIGIN, "https://evil.example".parse().unwrap());
        request.headers_mut().insert(axum::http::header::HOST, "panel.example".parse().unwrap());

        let response = app(&pool).oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        assert_eq!(body_json(response).await["error"], "csrf_origin_mismatch");
    }

    #[tokio::test]
    async fn me_without_a_cookie_is_unauthenticated() {
        let pool = test_pool().await;
        let response = app(&pool).oneshot(fetch("/me")).await.unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(body_json(response).await["error"], "unauthenticated");
    }

    #[tokio::test]
    async fn me_with_a_made_up_cookie_is_unauthenticated() {
        let pool = test_pool().await;
        a_session_for(&pool, "max").await;

        let response =
            app(&pool).oneshot(as_user(fetch("/me"), "not-a-session-of-ours")).await.unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn a_session_that_has_run_out_is_refused_although_the_cookie_is_right() {
        let pool = test_pool().await;
        let max = a_user(&pool, "max").await;
        let secret = sign_in(&pool, max).await;

        assert_eq!(
            app(&pool).oneshot(as_user(fetch("/me"), &secret)).await.unwrap().status(),
            StatusCode::OK,
            "the same cookie works while the session lasts"
        );

        sqlx::query("UPDATE sessions SET expires_at = ? WHERE user_id = ?")
            .bind(Timestamp::at(time::OffsetDateTime::now_utc() - time::Duration::seconds(1)))
            .bind(max)
            .execute(&pool)
            .await
            .unwrap();

        let response = app(&pool).oneshot(as_user(fetch("/me"), &secret)).await.unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(body_json(response).await["error"], "unauthenticated");
    }

    #[tokio::test]
    async fn me_carries_the_limits_the_usage_and_the_session() {
        let pool = test_pool().await;
        let max = a_user(&pool, "max").await;
        a_server(&pool, max, "one", 2048).await;
        let secret = sign_in(&pool, max).await;

        let body = body_json(app(&pool).oneshot(as_user(fetch("/me"), &secret)).await.unwrap()).await;
        assert_eq!(body["limits"]["memory_mib"], 4096);
        assert_eq!(body["limits"]["cpu_mode"], "cap");
        assert_eq!(body["usage"]["memory"]["allocated_mib"], 2048);
        assert_eq!(body["usage"]["memory"]["limit_mib"], 4096);
        assert_eq!(body["usage"]["servers"]["total"], 1);
        assert_eq!(body["usage"]["servers"]["running"], 0);
        assert_eq!(body["usage"]["over_limit"], false);
        assert_eq!(body["usage"]["over_limit_dimensions"].as_array().unwrap().len(), 0);
        assert_eq!(body["capabilities"]["blocked_reason"], serde_json::Value::Null);
        assert_eq!(body["system_user"]["state"], "ready");
        assert!(body["session"]["expires_at"].as_str().unwrap().ends_with('Z'));
    }

    #[tokio::test]
    async fn me_of_an_administrator_names_no_limits() {
        let pool = test_pool().await;
        let boss = an_admin(&pool, "boss").await;
        a_server(&pool, boss, "his", 8192).await;
        let secret = sign_in(&pool, boss).await;

        let body =
            body_json(app(&pool).oneshot(as_user(fetch("/me"), &secret)).await.unwrap()).await;

        let nothing = serde_json::Value::Null;
        assert_eq!(body["limits"], nothing);
        assert_eq!(body["usage"]["memory"]["limit_mib"], nothing);
        assert_eq!(body["usage"]["cpu"]["limit_cores"], nothing);
        assert_eq!(body["usage"]["pids"]["limit"], nothing);
        assert_eq!(body["usage"]["disk"]["limit_mib"], nothing);

        assert_eq!(body["usage"]["memory"]["allocated_mib"], 8192, "the figure is still measured");
        assert_eq!(body["usage"]["over_limit"], false);
        assert_eq!(body["capabilities"]["can_create_servers"], true);
        assert_eq!(body["capabilities"]["blocked_reason"], nothing);
    }

    #[tokio::test]
    async fn me_carries_the_disk_and_a_full_one_only_stops_what_is_new() {
        let pool = test_pool().await;
        let max = a_user(&pool, "max").await;
        let secret = sign_in(&pool, max).await;
        sqlx::query("UPDATE users SET disk_mib = 1024 WHERE id = ?")
            .bind(max)
            .execute(&pool)
            .await
            .unwrap();

        let room = Disks::fixed(400 * 1024 * 1024, 100 * 1024 * 1024);
        let body = body_json(
            app_with(&pool, room).oneshot(as_user(fetch("/me"), &secret)).await.unwrap(),
        )
        .await;
        assert_eq!(body["usage"]["disk"]["limit_mib"], 1024);
        assert_eq!(body["usage"]["disk"]["servers_bytes"], 419430400);
        assert_eq!(body["usage"]["disk"]["backups_bytes"], 104857600);
        assert_eq!(body["usage"]["disk"]["used_bytes"], 524288000, "the two added up");
        assert_eq!(body["usage"]["over_limit"], false);

        let full = Disks::fixed(2 * 1024 * 1024 * 1024, 0);
        let body = body_json(
            app_with(&pool, full).oneshot(as_user(fetch("/me"), &secret)).await.unwrap(),
        )
        .await;
        assert_eq!(body["usage"]["over_limit"], true);
        assert_eq!(body["usage"]["over_limit_dimensions"], serde_json::json!(["disk"]));
        assert_eq!(body["capabilities"]["can_create_servers"], false);
        assert_eq!(
            body["capabilities"]["can_start_servers"],
            true,
            "a full disk throttles; it does not stop what is there"
        );
        assert_eq!(body["capabilities"]["blocked_reason"], "over_limit");
    }

    #[tokio::test]
    async fn me_carries_exactly_the_fields_of_14() {
        let pool = test_pool().await;
        let max = a_user(&pool, "max").await;
        let secret = sign_in(&pool, max).await;
        let body = body_json(app(&pool).oneshot(as_user(fetch("/me"), &secret)).await.unwrap()).await;

        let shapes = [
            (
                "",
                vec![
                    "avatar_url", "capabilities", "created_at", "email", "id", "last_login_at",
                    "limits", "must_change_password", "origin", "panel_role", "session",
                    "system_user", "usage", "username",
                ],
            ),
            ("system_user", vec!["error_message", "name", "state", "uid"]),
            ("limits", vec!["cpu_cores", "cpu_mode", "disk_mib", "memory_mib", "pids_max"]),
            (
                "usage",
                vec![
                    "cpu", "disk", "measured_at", "memory", "over_limit", "over_limit_dimensions",
                    "pids", "servers",
                ],
            ),
            ("memory", vec!["allocated_mib", "limit_mib", "used_bytes"]),
            ("disk", vec!["backups_bytes", "complete", "limit_mib", "servers_bytes", "used_bytes"]),
            (
                "capabilities",
                vec![
                    "blocked_reason", "can_create_servers", "can_manage_panel_users",
                    "can_start_servers",
                ],
            ),
            ("session", vec!["expires_at", "id"]),
        ];

        for (path, expected) in shapes {
            let object = match path {
                "" => &body,
                "memory" => &body["usage"]["memory"],
                "disk" => &body["usage"]["disk"],
                other => &body[other],
            };
            let mut found: Vec<&str> =
                object.as_object().expect(path).keys().map(String::as_str).collect();
            found.sort_unstable();
            assert_eq!(found, expected, "Me.{path}");
        }
    }

    #[tokio::test]
    async fn signing_out_takes_the_row_and_the_cookie_with_it() {
        let pool = test_pool().await;
        let max = a_user(&pool, "max").await;
        let secret = sign_in(&pool, max).await;

        let response =
            app(&pool).oneshot(as_user(empty("POST", "/auth/logout"), &secret)).await.unwrap();
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        assert!(set_cookie(&response).unwrap().contains("Max-Age=0"));

        let left: i64 =
            sqlx::query_scalar("SELECT count(*) FROM sessions").fetch_one(&pool).await.unwrap();
        assert_eq!(left, 0);

        assert_eq!(
            app(&pool).oneshot(as_user(fetch("/me"), &secret)).await.unwrap().status(),
            StatusCode::UNAUTHORIZED
        );
    }

    #[tokio::test]
    async fn signing_out_twice_is_still_signing_out() {
        let pool = test_pool().await;
        let max = a_user(&pool, "max").await;
        let secret = sign_in(&pool, max).await;

        for _ in 0..2 {
            let again =
                app(&pool).oneshot(as_user(empty("POST", "/auth/logout"), &secret)).await.unwrap();
            assert_eq!(again.status(), StatusCode::NO_CONTENT, "the second one has nothing to do");
        }

        let without = app(&pool).oneshot(empty("POST", "/auth/logout")).await.unwrap();
        assert_eq!(without.status(), StatusCode::NO_CONTENT, "3.2: no cookie is still 204");
    }

    #[tokio::test]
    async fn a_new_password_drops_the_other_sessions_and_rotates_this_one() {
        let pool = test_pool().await;
        let max = a_user(&pool, "max").await;
        let elsewhere = sign_in(&pool, max).await;
        let here = sign_in(&pool, max).await;

        let response = app(&pool)
            .oneshot(as_user(
                send(
                    "POST",
                    "/me/password",
                    serde_json::json!({
                        "current_password": PASSWORD,
                        "new_password": "a-new-long-password",
                    }),
                ),
                &here,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NO_CONTENT);

        let fresh = set_cookie(&response).expect("a rotated cookie");
        assert!(!fresh.contains(&here), "the cookie that asked for the change is gone too");

        for old in [elsewhere, here] {
            assert_eq!(
                app(&pool).oneshot(as_user(fetch("/me"), &old)).await.unwrap().status(),
                StatusCode::UNAUTHORIZED
            );
        }

        let row = users::load(&pool, max).await.unwrap();
        assert!(password::verify("a-new-long-password", &row.password_hash));
        assert!(!password::verify(PASSWORD, &row.password_hash));
    }

    #[tokio::test]
    async fn the_old_password_has_to_be_right_and_the_new_one_long_enough() {
        let pool = test_pool().await;
        let max = a_user(&pool, "max").await;
        let secret = sign_in(&pool, max).await;

        let wrong = app(&pool)
            .oneshot(as_user(
                send(
                    "POST",
                    "/me/password",
                    serde_json::json!({
                        "current_password": "not-the-one",
                        "new_password": "a-new-long-password",
                    }),
                ),
                &secret,
            ))
            .await
            .unwrap();
        assert_eq!(wrong.status(), StatusCode::FORBIDDEN);
        assert_eq!(body_json(wrong).await["error"], "wrong_password");

        let short = app(&pool)
            .oneshot(as_user(
                send(
                    "POST",
                    "/me/password",
                    serde_json::json!({
                        "current_password": PASSWORD,
                        "new_password": "short",
                    }),
                ),
                &secret,
            ))
            .await
            .unwrap();
        assert_eq!(short.status(), StatusCode::BAD_REQUEST);
        assert_eq!(body_json(short).await["error"], "weak_password");

        let row = users::load(&pool, max).await.unwrap();
        assert!(password::verify(PASSWORD, &row.password_hash), "neither one changed anything");
    }

    #[tokio::test]
    async fn a_search_answers_with_names_and_nothing_else() {
        let pool = test_pool().await;
        a_user(&pool, "anna").await;
        a_user(&pool, "andre").await;
        let secret = a_session_for(&pool, "max").await;

        let body = body_json(
            app(&pool).oneshot(as_user(fetch("/users/search?query=an"), &secret)).await.unwrap(),
        )
        .await;

        let users = body["users"].as_array().unwrap();
        assert_eq!(users.len(), 2);
        assert_eq!(users[0]["username"], "andre");
        let mut fields: Vec<&str> = users[0].as_object().unwrap().keys().map(String::as_str).collect();
        fields.sort_unstable();
        assert_eq!(fields, ["avatar_url", "id", "username"], "3.5: nothing more");
    }

    #[tokio::test]
    async fn a_search_hands_out_no_more_than_the_twenty_five_of_3_5() {
        let pool = test_pool().await;
        for number in 0..30 {
            a_user(&pool, &format!("anna{number:02}")).await;
        }
        let secret = a_session_for(&pool, "max").await;

        let body = body_json(
            app(&pool)
                .oneshot(as_user(fetch("/users/search?query=anna&limit=1000"), &secret))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(body["users"].as_array().unwrap().len(), 25);

        assert_eq!(how_many(Some(1000)), SEARCH_LIMIT);
        assert_eq!(how_many(Some(0)), 1);
        assert_eq!(how_many(Some(4)), 4);
        assert_eq!(how_many(None), SEARCH_LIMIT);
    }

    #[tokio::test]
    async fn a_search_needs_a_session_and_a_word() {
        let pool = test_pool().await;
        let secret = a_session_for(&pool, "max").await;

        assert_eq!(
            app(&pool).oneshot(fetch("/users/search?query=an")).await.unwrap().status(),
            StatusCode::UNAUTHORIZED
        );

        let empty_query =
            app(&pool).oneshot(as_user(fetch("/users/search?query="), &secret)).await.unwrap();
        assert_eq!(empty_query.status(), StatusCode::BAD_REQUEST);
        assert_eq!(body_json(empty_query).await["error"], "invalid_request");
    }
}
