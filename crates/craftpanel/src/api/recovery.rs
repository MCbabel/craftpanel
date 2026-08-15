use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;

use axum::extract::{ConnectInfo, Path, State};
use axum::http::request::Parts;
use axum::http::StatusCode;
use axum::routing::post;
use axum::{Extension, Json, Router};
use serde::{Deserialize, Serialize};

use crate::auth::error::{Failure, Result};
use crate::auth::reset::Recovery;
use crate::auth::{extract, users, Admin, JsonBody};
use crate::model::{Id, Timestamp};
use crate::AppState;

pub fn router(service: Arc<Recovery>) -> Router<AppState> {
    Router::new()
        .route("/auth/password-reset", post(request))
        .route("/auth/password-reset/verify", post(verify))
        .route("/auth/password-reset/confirm", post(confirm))
        .route("/admin/users/{user_id}/password-reset", post(on_behalf_of))
        .layer(Extension(service))
        .layer(axum::middleware::from_fn(extract::same_origin))
}

#[derive(Deserialize)]
struct AddressRequest {
    email: String,
}

#[derive(Deserialize)]
struct TokenRequest {
    token: String,
}

#[derive(Deserialize)]
struct ConfirmRequest {
    token: String,
    new_password: String,
}

#[derive(Serialize)]
struct WhoseResponse {
    username: String,
}

async fn request(
    Extension(service): Extension<Arc<Recovery>>,
    parts: Parts,
    JsonBody(body): JsonBody<AddressRequest>,
) -> Result<StatusCode> {
    let from = caller_address(&parts);
    service.note_request(&body.email, from)?;

    let user_agent = extract::user_agent(&parts).map(str::to_owned);
    tokio::spawn(async move {
        service.begin(&body.email, from, user_agent, Timestamp::now()).await;
    });

    Ok(StatusCode::ACCEPTED)
}

async fn verify(
    Extension(service): Extension<Arc<Recovery>>,
    parts: Parts,
    JsonBody(body): JsonBody<TokenRequest>,
) -> Result<Json<WhoseResponse>> {
    let username =
        service.whose(&body.token, caller_address(&parts), Timestamp::now()).await?;
    Ok(Json(WhoseResponse { username }))
}

async fn confirm(
    Extension(service): Extension<Arc<Recovery>>,
    parts: Parts,
    JsonBody(body): JsonBody<ConfirmRequest>,
) -> Result<StatusCode> {
    service
        .confirm(&body.token, &body.new_password, caller_address(&parts), Timestamp::now())
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn on_behalf_of(
    State(state): State<AppState>,
    Admin(_): Admin,
    Extension(service): Extension<Arc<Recovery>>,
    Path(user_id): Path<String>,
) -> Result<StatusCode> {
    let id: Id = user_id
        .parse()
        .map_err(|_| Failure::not_found("user_not_found", "no such user"))?;
    let row = users::load(&state.pool, id).await?;

    service.on_behalf_of(&row, Timestamp::now()).await?;
    Ok(StatusCode::ACCEPTED)
}

fn caller_address(parts: &Parts) -> Option<IpAddr> {
    parts.extensions.get::<ConnectInfo<SocketAddr>>().map(|info| info.0.ip())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::harness::{
        a_user, an_admin, as_user, body_json, empty, insert_user, send, sign_in, state_with,
        test_pool, PASSWORD,
    };
    use crate::auth::reset::Gate;
    use crate::auth::{password, session};
    use crate::config::Config;
    use crate::mail::harness::{with_key, DataDir, FakeResend};
    use crate::mail::Mail;
    use crate::model::PanelRole;
    use axum::body::Body;
    use axum::http::Request;
    use sqlx::SqlitePool;
    use std::time::Duration;
    use tower::ServiceExt;

    const CHOSEN: &str = "a-new-password";

    struct Panel {
        app: Router,
        pool: SqlitePool,
        service: Arc<Recovery>,
        mail: Arc<Mail>,
        gate: Arc<Gate>,
        _dir: DataDir,
        _resend: FakeResend,
    }

    async fn panel() -> Panel {
        build(true).await
    }

    async fn panel_without_mail() -> Panel {
        build(false).await
    }

    async fn panel_without_a_panel_address() -> Panel {
        let panel = build(true).await;
        panel
            .mail
            .save(
                crate::mail::store::Form {
                    from_address: "panel@panel.example".to_owned(),
                    from_name: "craftpanel".to_owned(),
                    reply_to: None,
                    link_base: None,
                    daily_limit: 100,
                },
                crate::mail::KeyChange::Keep,
                Timestamp::now(),
            )
            .await
            .expect("taking the panel address away");
        panel
    }

    async fn build(with_mail: bool) -> Panel {
        let pool = test_pool().await;
        let dir = DataDir::new();
        let resend = FakeResend::started().await;
        let mail = Mail::against(pool.clone(), dir.path().to_owned(), resend.base(), None);
        if with_mail {
            with_key(&mail, &pool).await;
        }

        let gate = Arc::new(Gate::default());
        let service = Recovery::gated(pool.clone(), Arc::clone(&mail), Arc::clone(&gate));

        let mut config = Config::default();
        config.data_dir = dir.path().to_owned();
        let app = Router::new()
            .nest("/api/v1", router(Arc::clone(&service)))
            .with_state(state_with(&pool, config));

        Panel { app, pool, service, mail, gate, _dir: dir, _resend: resend }
    }

    async fn call(panel: &Panel, request: Request<Body>) -> axum::response::Response {
        panel.app.clone().oneshot(request).await.expect("a response")
    }

    async fn a_user_with_an_address(pool: &SqlitePool, username: &str) -> (Id, String) {
        let id = insert_user(pool, username, PanelRole::User, PASSWORD).await;
        let email = format!("{username}@example.test");
        sqlx::query("UPDATE users SET email = ? WHERE id = ?")
            .bind(&email)
            .bind(id)
            .execute(pool)
            .await
            .expect("writing the address");
        (id, email)
    }

    async fn let_the_work_through(panel: &Panel) {
        panel.gate.open.notify_one();
        tokio::time::timeout(Duration::from_secs(5), panel.gate.finished.notified())
            .await
            .expect("the reset task finished");
    }

    async fn rows(pool: &SqlitePool) -> i64 {
        sqlx::query_scalar("SELECT count(*) FROM password_resets").fetch_one(pool).await.unwrap()
    }

    async fn reset_mails(pool: &SqlitePool) -> i64 {
        sqlx::query_scalar("SELECT count(*) FROM mail_outbox WHERE kind = 'reset_password'")
            .fetch_one(pool)
            .await
            .unwrap()
    }

    async fn a_link_for(panel: &Panel, user: Id) -> String {
        crate::auth::reset::mint_for(&panel.pool, user, None, None, Timestamp::now())
            .await
            .expect("minting a link")
    }

    #[tokio::test]
    async fn the_answer_is_out_before_anything_is_looked_up() {
        let panel = panel().await;
        let (user, email) = a_user_with_an_address(&panel.pool, "max").await;

        let response = tokio::time::timeout(
            Duration::from_secs(5),
            call(&panel, send("POST", "/api/v1/auth/password-reset", serde_json::json!({ "email": email }))),
        )
        .await
        .expect("the handler answered without waiting for the work");

        assert_eq!(response.status(), StatusCode::ACCEPTED);
        assert_eq!(rows(&panel.pool).await, 0, "the work ran inside the request");

        let_the_work_through(&panel).await;
        assert_eq!(rows(&panel.pool).await, 1);
        assert_eq!(reset_mails(&panel.pool).await, 1);

        let owner: Id = sqlx::query_scalar("SELECT user_id FROM password_resets")
            .fetch_one(&panel.pool)
            .await
            .unwrap();
        assert_eq!(owner, user);
    }

    #[tokio::test]
    async fn a_known_and_an_unknown_address_answer_identically() {
        let panel = panel().await;
        a_user_with_an_address(&panel.pool, "max").await;

        let mut seen = Vec::new();
        for asked in ["max@example.test", "nobody@example.test", "not-an-address"] {
            let response = call(
                &panel,
                send("POST", "/api/v1/auth/password-reset", serde_json::json!({ "email": asked })),
            )
            .await;
            let status = response.status();
            let headers: Vec<String> = response
                .headers()
                .iter()
                .map(|(name, value)| format!("{name}: {value:?}"))
                .collect();
            let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
            seen.push((status, headers, bytes));
            let_the_work_through(&panel).await;
        }

        assert_eq!(seen[0].0, StatusCode::ACCEPTED);
        assert!(seen[0].2.is_empty(), "21.1 answers without a body");
        assert!(
            !seen[0].1.iter().any(|header| header.starts_with("set-cookie")),
            "{:?}", seen[0].1
        );
        assert_eq!(seen[0], seen[1], "an unknown address answers differently");
        assert_eq!(seen[0], seen[2], "something that is no address answers differently");

        assert_eq!(rows(&panel.pool).await, 1, "and only the real one got a row");
    }

    #[tokio::test]
    async fn the_token_itself_is_nowhere_in_the_database() {
        let panel = panel().await;
        let (user, _) = a_user_with_an_address(&panel.pool, "max").await;
        let token = a_link_for(&panel, user).await;

        let stored: String =
            sqlx::query_scalar("SELECT token_hash FROM password_resets").fetch_one(&panel.pool).await.unwrap();
        assert_ne!(stored, token);
        assert_eq!(stored, crate::auth::secret::digest(&token));
        assert_eq!(stored.len(), 64, "sha-256 in hex");
        assert_eq!(token.len(), 43, "256 bits in unpadded base64url");
    }

    #[tokio::test]
    async fn without_mail_no_row_is_written_and_the_answer_is_the_same() {
        let panel = panel_without_mail().await;
        let (_, email) = a_user_with_an_address(&panel.pool, "max").await;

        let response = call(
            &panel,
            send("POST", "/api/v1/auth/password-reset", serde_json::json!({ "email": email })),
        )
        .await;
        assert_eq!(response.status(), StatusCode::ACCEPTED);

        let_the_work_through(&panel).await;
        assert_eq!(rows(&panel.pool).await, 0);
        assert_eq!(reset_mails(&panel.pool).await, 0);
    }

    #[tokio::test]
    async fn an_open_application_gets_no_reset_link() {
        let panel = panel().await;
        crate::registration::store::insert(
            &panel.pool,
            crate::registration::store::NewApplication {
                username: "max",
                email: "max@example.test",
                password_hash: "x".to_owned(),
                signup_ip: None,
                token_hash: crate::auth::secret::digest("something"),
                token_expires_at: Timestamp::now(),
            },
            Timestamp::now(),
        )
        .await
        .unwrap();

        let response = call(
            &panel,
            send(
                "POST",
                "/api/v1/auth/password-reset",
                serde_json::json!({ "email": "max@example.test" }),
            ),
        )
        .await;
        assert_eq!(response.status(), StatusCode::ACCEPTED);

        let_the_work_through(&panel).await;
        assert_eq!(rows(&panel.pool).await, 0);
    }

    #[tokio::test]
    async fn two_requests_in_one_minute_make_one_mail() {
        let panel = panel().await;
        let (_, email) = a_user_with_an_address(&panel.pool, "max").await;

        for _ in 0..2 {
            let response = call(
                &panel,
                send("POST", "/api/v1/auth/password-reset", serde_json::json!({ "email": email })),
            )
            .await;
            assert_eq!(response.status(), StatusCode::ACCEPTED, "the answer never changes");
            let_the_work_through(&panel).await;
        }

        assert_eq!(rows(&panel.pool).await, 1);
        assert_eq!(reset_mails(&panel.pool).await, 1);
    }

    #[tokio::test]
    async fn a_living_link_says_whose_it_is_without_spending_itself() {
        let panel = panel().await;
        let (user, _) = a_user_with_an_address(&panel.pool, "max").await;
        let token = a_link_for(&panel, user).await;

        for _ in 0..2 {
            let response = call(
                &panel,
                send(
                    "POST",
                    "/api/v1/auth/password-reset/verify",
                    serde_json::json!({ "token": token }),
                ),
            )
            .await;
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(body_json(response).await["username"], "max");
        }

        let spent: Option<String> =
            sqlx::query_scalar("SELECT used_at FROM password_resets").fetch_one(&panel.pool).await.unwrap();
        assert_eq!(spent, None, "asking whose it is does not spend it");
    }

    #[tokio::test]
    async fn a_link_that_ran_out_is_no_link() {
        let panel = panel().await;
        let (user, _) = a_user_with_an_address(&panel.pool, "max").await;
        let token = a_link_for(&panel, user).await;

        sqlx::query("UPDATE password_resets SET expires_at = ?")
            .bind(Timestamp::at(Timestamp::now().as_datetime() - time::Duration::seconds(1)))
            .execute(&panel.pool)
            .await
            .unwrap();

        let refused = call(
            &panel,
            send("POST", "/api/v1/auth/password-reset/verify", serde_json::json!({ "token": token })),
        )
        .await;
        assert_eq!(refused.status(), StatusCode::BAD_REQUEST);
        assert_eq!(body_json(refused).await["error"], "invalid_reset_token");
    }

    #[tokio::test]
    async fn unknown_expired_and_spent_all_read_the_same() {
        let panel = panel().await;
        let (user, _) = a_user_with_an_address(&panel.pool, "max").await;

        let spent = a_link_for(&panel, user).await;
        sqlx::query("UPDATE password_resets SET used_at = ?")
            .bind(Timestamp::now())
            .execute(&panel.pool)
            .await
            .unwrap();

        let expired = a_link_for(&panel, user).await;
        sqlx::query("UPDATE password_resets SET expires_at = ? WHERE token_hash = ?")
            .bind(Timestamp::at(Timestamp::now().as_datetime() - time::Duration::seconds(1)))
            .bind(crate::auth::secret::digest(&expired))
            .execute(&panel.pool)
            .await
            .unwrap();

        let mut seen = Vec::new();
        for token in [spent.as_str(), expired.as_str(), "nonsense"] {
            let response = call(
                &panel,
                send(
                    "POST",
                    "/api/v1/auth/password-reset/verify",
                    serde_json::json!({ "token": token }),
                ),
            )
            .await;
            seen.push((response.status(), body_json(response).await));
        }

        assert_eq!(seen[0].0, StatusCode::BAD_REQUEST);
        assert_eq!(seen[0], seen[1]);
        assert_eq!(seen[0], seen[2]);
    }

    #[tokio::test]
    async fn setting_a_password_closes_every_session_and_opens_none() {
        let panel = panel().await;
        let (user, _) = a_user_with_an_address(&panel.pool, "max").await;
        sqlx::query("UPDATE users SET must_change_password = 1 WHERE id = ?")
            .bind(user)
            .execute(&panel.pool)
            .await
            .unwrap();
        sign_in(&panel.pool, user).await;
        sign_in(&panel.pool, user).await;
        let token = a_link_for(&panel, user).await;
        let second_link = a_link_for(&panel, user).await;

        let response = call(
            &panel,
            send(
                "POST",
                "/api/v1/auth/password-reset/confirm",
                serde_json::json!({ "token": token, "new_password": CHOSEN }),
            ),
        )
        .await;

        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        assert!(
            !response.headers().contains_key(axum::http::header::SET_COOKIE),
            "after setting a password one is not signed in (OWASP)"
        );

        let row = crate::auth::users::load(&panel.pool, user).await.unwrap();
        assert!(password::verify(CHOSEN, &row.password_hash), "the new password is in force");
        assert!(!password::verify(PASSWORD, &row.password_hash), "and the old one is not");
        assert!(!row.must_change_password, "he chose it himself, so the flag falls");

        assert_eq!(session::count_active(&panel.pool, user, Timestamp::now()).await.unwrap(), 0);

        let left = call(
            &panel,
            send(
                "POST",
                "/api/v1/auth/password-reset/verify",
                serde_json::json!({ "token": second_link }),
            ),
        )
        .await;
        assert_eq!(left.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn a_link_works_exactly_once() {
        let panel = panel().await;
        let (user, _) = a_user_with_an_address(&panel.pool, "max").await;
        let token = a_link_for(&panel, user).await;

        let body = serde_json::json!({ "token": token, "new_password": CHOSEN });
        let first = call(&panel, send("POST", "/api/v1/auth/password-reset/confirm", body.clone())).await;
        assert_eq!(first.status(), StatusCode::NO_CONTENT);

        let again = call(&panel, send("POST", "/api/v1/auth/password-reset/confirm", body)).await;
        assert_eq!(again.status(), StatusCode::BAD_REQUEST);
        assert_eq!(body_json(again).await["error"], "invalid_reset_token");

        let row = crate::auth::users::load(&panel.pool, user).await.unwrap();
        assert!(password::verify(CHOSEN, &row.password_hash));
    }

    #[tokio::test]
    async fn a_fresh_request_makes_the_older_link_worthless() {
        let panel = panel().await;
        let (user, email) = a_user_with_an_address(&panel.pool, "max").await;
        let old = a_link_for(&panel, user).await;

        panel.gate.open.notify_one();
        panel
            .service
            .begin(
                &email,
                None,
                None,
                Timestamp::at(Timestamp::now().as_datetime() + time::Duration::hours(1)),
            )
            .await;
        assert_eq!(rows(&panel.pool).await, 1, "one open link per account");

        let refused = call(
            &panel,
            send("POST", "/api/v1/auth/password-reset/verify", serde_json::json!({ "token": old })),
        )
        .await;
        assert_eq!(refused.status(), StatusCode::BAD_REQUEST, "two live links are two windows");
    }

    #[tokio::test]
    async fn a_password_that_is_too_short_costs_neither_the_link_nor_the_brake() {
        let panel = panel().await;
        let (user, _) = a_user_with_an_address(&panel.pool, "max").await;
        let token = a_link_for(&panel, user).await;

        for _ in 0..12 {
            let refused = call(
                &panel,
                send(
                    "POST",
                    "/api/v1/auth/password-reset/confirm",
                    serde_json::json!({ "token": token, "new_password": "short" }),
                ),
            )
            .await;
            assert_eq!(refused.status(), StatusCode::BAD_REQUEST);
            assert_eq!(body_json(refused).await["error"], "weak_password");
        }

        let good = call(
            &panel,
            send(
                "POST",
                "/api/v1/auth/password-reset/confirm",
                serde_json::json!({ "token": token, "new_password": CHOSEN }),
            ),
        )
        .await;
        assert_eq!(good.status(), StatusCode::NO_CONTENT, "twelve short tries locked the door");
    }

    #[tokio::test]
    async fn the_admin_nudge_is_plain_about_what_is_missing() {
        let panel = panel().await;
        let admin = an_admin(&panel.pool, "chef").await;
        let cookie = sign_in(&panel.pool, admin).await;
        let (with_address, _) = a_user_with_an_address(&panel.pool, "max").await;
        let without = a_user(&panel.pool, "anna").await;

        let sent = call(
            &panel,
            as_user(
                empty("POST", &format!("/api/v1/admin/users/{with_address}/password-reset")),
                &cookie,
            ),
        )
        .await;
        assert_eq!(sent.status(), StatusCode::ACCEPTED);
        assert_eq!(reset_mails(&panel.pool).await, 1);

        let no_address = call(
            &panel,
            as_user(empty("POST", &format!("/api/v1/admin/users/{without}/password-reset")), &cookie),
        )
        .await;
        assert_eq!(no_address.status(), StatusCode::CONFLICT);
        assert_eq!(body_json(no_address).await["error"], "no_email_address");

        let nobody = call(
            &panel,
            as_user(
                empty("POST", &format!("/api/v1/admin/users/{}/password-reset", Id::new())),
                &cookie,
            ),
        )
        .await;
        assert_eq!(nobody.status(), StatusCode::NOT_FOUND);
        assert_eq!(body_json(nobody).await["error"], "user_not_found");
    }

    #[tokio::test]
    async fn without_mail_the_admin_hears_why_instead_of_a_silent_202() {
        let panel = panel_without_mail().await;
        let admin = an_admin(&panel.pool, "chef").await;
        let cookie = sign_in(&panel.pool, admin).await;
        let (user, _) = a_user_with_an_address(&panel.pool, "max").await;

        let refused = call(
            &panel,
            as_user(empty("POST", &format!("/api/v1/admin/users/{user}/password-reset")), &cookie),
        )
        .await;
        assert_eq!(refused.status(), StatusCode::CONFLICT);
        let body = body_json(refused).await;
        assert_eq!(body["error"], "mail_not_configured");
        assert!(body["message"].as_str().unwrap().contains("reset-link"), "{body}");
        assert_eq!(rows(&panel.pool).await, 0);
    }

    #[tokio::test]
    async fn without_a_panel_address_the_link_the_account_already_has_survives() {
        let panel = panel_without_a_panel_address().await;
        let admin = an_admin(&panel.pool, "chef").await;
        let cookie = sign_in(&panel.pool, admin).await;
        let (user, _) = a_user_with_an_address(&panel.pool, "max").await;
        let mailed = a_link_for(&panel, user).await;

        let refused = call(
            &panel,
            as_user(empty("POST", &format!("/api/v1/admin/users/{user}/password-reset")), &cookie),
        )
        .await;

        assert_eq!(refused.status(), StatusCode::CONFLICT);
        let body = body_json(refused).await;
        assert_eq!(body["error"], "mail_no_link_base");
        assert_eq!(reset_mails(&panel.pool).await, 0);

        let still_good = call(
            &panel,
            send(
                "POST",
                "/api/v1/auth/password-reset/verify",
                serde_json::json!({ "token": mailed }),
            ),
        )
        .await;
        assert_eq!(still_good.status(), StatusCode::OK, "the account's own link paid for the refusal");

        assert!(body["message"].as_str().unwrap().contains("reset-link"), "{body}");
    }

    #[tokio::test]
    async fn the_admin_nudge_has_no_cool_down_but_five_an_hour_is_the_ceiling() {
        let panel = panel().await;
        let admin = an_admin(&panel.pool, "chef").await;
        let cookie = sign_in(&panel.pool, admin).await;
        let (user, _) = a_user_with_an_address(&panel.pool, "max").await;
        let press = || {
            as_user(empty("POST", &format!("/api/v1/admin/users/{user}/password-reset")), &cookie)
        };

        for round in 1..=5 {
            let sent = call(&panel, press()).await;
            assert_eq!(sent.status(), StatusCode::ACCEPTED, "press {round} waited on a cool-down");
        }
        assert_eq!(reset_mails(&panel.pool).await, 5);

        let living: String = sqlx::query_scalar("SELECT token_hash FROM password_resets")
            .fetch_one(&panel.pool)
            .await
            .unwrap();

        let refused = call(&panel, press()).await;
        assert_eq!(refused.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(body_json(refused).await["error"], "too_many_attempts");

        assert_eq!(
            sqlx::query_scalar::<_, String>("SELECT token_hash FROM password_resets")
                .fetch_one(&panel.pool)
                .await
                .unwrap(),
            living,
        );
        assert_eq!(reset_mails(&panel.pool).await, 5);

        let (other, _) = a_user_with_an_address(&panel.pool, "anna").await;
        let elsewhere = call(
            &panel,
            as_user(empty("POST", &format!("/api/v1/admin/users/{other}/password-reset")), &cookie),
        )
        .await;
        assert_eq!(elsewhere.status(), StatusCode::ACCEPTED);
    }

    #[tokio::test]
    async fn the_admin_nudge_is_admin_only() {
        let panel = panel().await;
        let (user, _) = a_user_with_an_address(&panel.pool, "max").await;
        let theirs = sign_in(&panel.pool, user).await;

        let refused = call(
            &panel,
            as_user(empty("POST", &format!("/api/v1/admin/users/{user}/password-reset")), &theirs),
        )
        .await;
        assert_eq!(refused.status(), StatusCode::FORBIDDEN);

        let nobody =
            call(&panel, empty("POST", &format!("/api/v1/admin/users/{user}/password-reset"))).await;
        assert_eq!(nobody.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn changing_a_password_any_other_way_throws_the_open_links_away() {
        let panel = panel().await;
        let (user, _) = a_user_with_an_address(&panel.pool, "max").await;
        a_link_for(&panel, user).await;
        assert_eq!(rows(&panel.pool).await, 1);

        let cookie = sign_in(&panel.pool, user).await;
        let app = Router::new()
            .nest("/api/v1", crate::api::session::router())
            .with_state(state_with(&panel.pool, Config::default()));
        let response = app
            .oneshot(as_user(
                send(
                    "POST",
                    "/api/v1/me/password",
                    serde_json::json!({ "current_password": PASSWORD, "new_password": CHOSEN }),
                ),
                &cookie,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NO_CONTENT);

        assert_eq!(
            rows(&panel.pool).await,
            0,
            "an old mailed link would open an account its owner has just taken back"
        );
    }
}
