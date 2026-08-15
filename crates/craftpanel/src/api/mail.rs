use std::sync::Arc;

use axum::extract::{FromRequestParts, Path, State};
use axum::http::header::CONTENT_TYPE;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Extension, Json, Router};
use serde::Deserialize;

use crate::auth::error::{Failure, Result};
use crate::auth::{extract, Admin, JsonBody, Params};
use crate::mail::store::{Form, State as Delivery};
use crate::mail::{Kind, KeyChange, Mail, MailOutboxList, MailSettings, TestSent};
use crate::model::Id;
use crate::AppState;

const PAGE: u32 = 50;
const PAGE_CEILING: u32 = 200;

pub fn router(mail: Arc<Mail>) -> Router<AppState> {
    Router::new()
        .route("/admin/mail", get(settings).put(save))
        .route("/admin/mail/key", delete(drop_key))
        .route("/admin/mail/test", post(test))
        .route("/admin/mail/outbox", get(outbox))
        .route("/admin/mail/outbox/{id}/content", get(content))
        .route("/admin/mail/outbox/{id}/retry", post(retry))
        .route("/admin/mail/preview/{kind}", get(preview))
        .layer(Extension(mail))
        .layer(axum::middleware::from_fn(extract::same_origin))
}

struct OfMail(Id);

impl FromRequestParts<AppState> for OfMail {
    type Rejection = Failure;

    async fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        state: &AppState,
    ) -> Result<Self> {
        let Path(raw) = Path::<String>::from_request_parts(parts, state)
            .await
            .map_err(|_| unknown_mail())?;
        raw.parse().map(Self).map_err(|_| unknown_mail())
    }
}

fn unknown_mail() -> Failure {
    Failure::not_found("mail_not_found", "no such mail")
}

async fn settings(_: Admin, Extension(mail): Extension<Arc<Mail>>) -> Result<Json<MailSettings>> {
    mail.settings().await.map(Json)
}

#[derive(Deserialize)]
struct UpdateMailSettings {
    from_address: String,
    from_name: String,
    #[serde(default)]
    reply_to: Option<String>,
    #[serde(default)]
    link_base: Option<String>,
    daily_limit: u32,
    #[serde(default)]
    api_key: Option<String>,
}

async fn save(
    _: Admin,
    Extension(mail): Extension<Arc<Mail>>,
    JsonBody(body): JsonBody<UpdateMailSettings>,
) -> Result<Json<MailSettings>> {
    let key = match body.api_key {
        None => KeyChange::Keep,
        Some(text) if text.trim().is_empty() => KeyChange::Remove,
        Some(text) => KeyChange::Replace(text),
    };
    let form = Form {
        from_address: body.from_address,
        from_name: body.from_name,
        reply_to: body.reply_to,
        link_base: body.link_base,
        daily_limit: body.daily_limit,
    };

    mail.save(form, key, crate::model::Timestamp::now()).await.map(Json)
}

async fn drop_key(_: Admin, Extension(mail): Extension<Arc<Mail>>) -> Result<StatusCode> {
    mail.forget_key().await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
struct SendTestMail {
    #[serde(default)]
    to: Option<String>,
}

async fn test(
    admin: Admin,
    State(state): State<AppState>,
    Extension(mail): Extension<Arc<Mail>>,
    JsonBody(body): JsonBody<SendTestMail>,
) -> Result<Json<TestSent>> {
    let typed =
        body.to.map(|text| crate::mail::clean_header(&text)).filter(|text| !text.is_empty());

    let to = match typed {
        Some(address) => address,
        None => own_address(&state, admin.0.id()).await?.ok_or_else(|| {
            Failure::invalid_request(
                "Type an address for the test mail — your own account has none.",
            )
        })?,
    };

    if !crate::mail::plausible_address(&to) {
        return Err(Failure::invalid_request(
            "That is not an address. The form is name@domain.tld.",
        ));
    }

    Ok(Json(mail.send_test(&to, crate::model::Timestamp::now()).await?))
}

async fn own_address(state: &AppState, user: Id) -> Result<Option<String>> {
    let row: Option<(Option<String>,)> = sqlx::query_as("SELECT email FROM users WHERE id = ?")
        .bind(user)
        .fetch_optional(&state.pool)
        .await?;
    Ok(row.and_then(|row| row.0).filter(|address| !address.is_empty()))
}

#[derive(Deserialize)]
struct OutboxQuery {
    limit: Option<u32>,
    state: Option<String>,
}

async fn outbox(
    _: Admin,
    Extension(mail): Extension<Arc<Mail>>,
    Params(query): Params<OutboxQuery>,
) -> Result<Json<MailOutboxList>> {
    let state = match query.state.as_deref() {
        None | Some("") => None,
        Some(text) => Some(text.parse::<Delivery>().map_err(|_| {
            Failure::invalid_request("state is one of queued, sending, sent, failed")
        })?),
    };

    let limit = query.limit.unwrap_or(PAGE).clamp(1, PAGE_CEILING);
    mail.outbox(limit, state).await.map(Json)
}

async fn content(
    _: Admin,
    Extension(mail): Extension<Arc<Mail>>,
    OfMail(id): OfMail,
) -> Result<Response> {
    let html = mail.content(id).await?;
    Ok(html_page(html))
}

async fn retry(
    _: Admin,
    Extension(mail): Extension<Arc<Mail>>,
    OfMail(id): OfMail,
) -> Result<StatusCode> {
    mail.retry(id).await?;
    Ok(StatusCode::ACCEPTED)
}

async fn preview(_: Admin, Path(kind): Path<String>) -> Result<Response> {
    let kind: Kind = kind
        .parse()
        .map_err(|_| Failure::not_found("not_found", "there is no mail by that name"))?;
    Ok(html_page(crate::mail::render::sample(kind).html))
}

fn html_page(html: String) -> Response {
    ([(CONTENT_TYPE, "text/html; charset=utf-8")], html).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::harness::{
        a_user, an_admin, as_user, body_json, empty, send, sign_in, state_with, test_pool,
    };
    use crate::config::Config;
    use crate::mail::harness::{DataDir, FakeResend};
    use crate::model::Timestamp;
    use axum::body::Body;
    use axum::http::Request;
    use serde_json::json;
    use sqlx::SqlitePool;
    use tower::ServiceExt;

    struct Panel {
        app: Router,
        pool: SqlitePool,
        mail: Arc<Mail>,
        dir: DataDir,
        resend: FakeResend,
    }

    async fn panel() -> Panel {
        let pool = test_pool().await;
        let dir = DataDir::new();
        let resend = FakeResend::started().await;
        let mail = crate::mail::harness::service(&pool, &dir, resend.base());

        let mut config = Config::default();
        config.data_dir = dir.path().to_owned();
        let app = Router::new()
            .nest("/api/v1", router(Arc::clone(&mail)))
            .with_state(state_with(&pool, config));

        Panel { app, pool, mail, dir, resend }
    }

    async fn call(panel: &Panel, request: Request<Body>) -> Response {
        panel.app.clone().oneshot(request).await.expect("a response")
    }

    async fn text(response: Response) -> String {
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.expect("a body");
        String::from_utf8(bytes.to_vec()).expect("printable")
    }

    fn saving(key: serde_json::Value) -> serde_json::Value {
        serde_json::json!({
            "from_address": "panel@panel.example",
            "from_name": "craftpanel",
            "reply_to": null,
            "link_base": "https://panel.example",
            "daily_limit": 100,
            "api_key": key,
        })
    }

    #[tokio::test]
    async fn every_one_of_the_eight_is_shut_to_a_stranger_and_to_an_ordinary_user() {
        let panel = panel().await;
        let anna = a_user(&panel.pool, "anna").await;
        let hers = sign_in(&panel.pool, anna).await;

        let calls: Vec<(&str, String)> = vec![
            ("GET", "/api/v1/admin/mail".to_owned()),
            ("PUT", "/api/v1/admin/mail".to_owned()),
            ("DELETE", "/api/v1/admin/mail/key".to_owned()),
            ("POST", "/api/v1/admin/mail/test".to_owned()),
            ("GET", "/api/v1/admin/mail/outbox".to_owned()),
            ("GET", format!("/api/v1/admin/mail/outbox/{}/content", Id::new())),
            ("POST", format!("/api/v1/admin/mail/outbox/{}/retry", Id::new())),
            ("GET", "/api/v1/admin/mail/preview/verify_email".to_owned()),
        ];

        for (method, path) in calls {
            let anonymous = call(&panel, send(method, &path, serde_json::json!({}))).await;
            assert_eq!(anonymous.status(), StatusCode::UNAUTHORIZED, "{method} {path}");

            let ordinary =
                call(&panel, as_user(send(method, &path, serde_json::json!({})), &hers)).await;
            assert_eq!(ordinary.status(), StatusCode::FORBIDDEN, "{method} {path}");
        }
    }

    #[tokio::test]
    async fn the_first_look_says_not_configured_without_a_word_of_alarm() {
        let panel = panel().await;
        let admin = an_admin(&panel.pool, "max").await;
        let cookie = sign_in(&panel.pool, admin).await;

        let response = call(&panel, as_user(empty("GET", "/api/v1/admin/mail"), &cookie)).await;
        assert_eq!(response.status(), StatusCode::OK);

        let body = body_json(response).await;
        assert_eq!(body["provider"], "resend");
        assert_eq!(body["state"], "not_configured");
        assert_eq!(body["from_address"], "onboarding@resend.dev");
        assert_eq!(body["from_name"], "CraftPanel");
        assert_eq!(body["daily_limit"], 100);
        assert_eq!(body["key_set_at"], serde_json::Value::Null);
        assert_eq!(body["link_base"], serde_json::Value::Null);
        assert_eq!(body["example_link"], serde_json::Value::Null);
        assert_eq!(body["sent_today"], 0);
        assert!(body.get("api_key").is_none(), "no field could even carry a key");
    }

    #[tokio::test]
    async fn saving_the_key_and_then_the_sender_keeps_the_key() {
        let panel = panel().await;
        let admin = an_admin(&panel.pool, "max").await;
        let cookie = sign_in(&panel.pool, admin).await;

        let stored = call(
            &panel,
            as_user(send("PUT", "/api/v1/admin/mail", saving("re_from_the_form".into())), &cookie),
        )
        .await;
        assert_eq!(stored.status(), StatusCode::OK);
        let body = body_json(stored).await;
        assert_eq!(body["state"], "configured");
        assert!(body["key_set_at"].is_string());
        assert_eq!(body["example_link"], "https://panel.example/verify-email#…");
        assert_eq!(
            std::fs::read_to_string(panel.dir.key_file()).expect("the key file").trim(),
            "re_from_the_form"
        );

        let again = call(
            &panel,
            as_user(
                send(
                    "PUT",
                    "/api/v1/admin/mail",
                    serde_json::json!({
                        "from_address": "hello@panel.example",
                        "from_name": "The panel",
                        "reply_to": "",
                        "link_base": "https://panel.example",
                        "daily_limit": 20,
                    }),
                ),
                &cookie,
            ),
        )
        .await;
        let body = body_json(again).await;
        assert_eq!(body["state"], "configured");
        assert_eq!(body["from_address"], "hello@panel.example");
        assert_eq!(body["daily_limit"], 20);
        assert!(panel.dir.key_file().exists(), "the key survived a save of the sender");

        let cleared = call(
            &panel,
            as_user(send("PUT", "/api/v1/admin/mail", saving("".into())), &cookie),
        )
        .await;
        assert_eq!(body_json(cleared).await["state"], "not_configured");
        assert!(!panel.dir.key_file().exists());
    }

    #[tokio::test]
    async fn a_negative_daily_limit_and_a_broken_address_are_both_invalid_requests() {
        let panel = panel().await;
        let admin = an_admin(&panel.pool, "max").await;
        let cookie = sign_in(&panel.pool, admin).await;

        let negative = call(
            &panel,
            as_user(
                send(
                    "PUT",
                    "/api/v1/admin/mail",
                    serde_json::json!({
                        "from_address": "panel@panel.example",
                        "from_name": "craftpanel",
                        "daily_limit": -1,
                    }),
                ),
                &cookie,
            ),
        )
        .await;
        assert_eq!(negative.status(), StatusCode::BAD_REQUEST);
        assert_eq!(body_json(negative).await["error"], "invalid_request");

        let broken = call(
            &panel,
            as_user(
                send(
                    "PUT",
                    "/api/v1/admin/mail",
                    serde_json::json!({
                        "from_address": "not an address",
                        "from_name": "craftpanel",
                        "daily_limit": 100,
                    }),
                ),
                &cookie,
            ),
        )
        .await;
        assert_eq!(broken.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn the_test_button_answers_with_resends_own_id_and_needs_an_address() {
        let panel = panel().await;
        let admin = an_admin(&panel.pool, "max").await;
        let cookie = sign_in(&panel.pool, admin).await;

        let closed = call(
            &panel,
            as_user(
                send("POST", "/api/v1/admin/mail/test", serde_json::json!({"to": "o@example.com"})),
                &cookie,
            ),
        )
        .await;
        assert_eq!(closed.status(), StatusCode::CONFLICT);
        assert_eq!(body_json(closed).await["error"], "mail_not_configured");

        crate::mail::harness::with_key(&panel.mail, &panel.pool).await;

        let nowhere =
            call(&panel, as_user(send("POST", "/api/v1/admin/mail/test", json!({})), &cookie))
                .await;
        assert_eq!(nowhere.status(), StatusCode::BAD_REQUEST);
        assert_eq!(body_json(nowhere).await["error"], "invalid_request");

        let sent = call(
            &panel,
            as_user(
                send(
                    "POST",
                    "/api/v1/admin/mail/test",
                    serde_json::json!({"to": "owner@example.com"}),
                ),
                &cookie,
            ),
        )
        .await;
        assert_eq!(sent.status(), StatusCode::OK);
        let body = body_json(sent).await;
        assert_eq!(body["id"], "49a3999c-0ce1-4ea6-ab68-afcd6dc2e794");
        assert_eq!(body["to"], "owner@example.com");
        assert_eq!(panel.resend.calls().len(), 1);
    }

    #[tokio::test]
    async fn the_test_button_falls_back_to_the_address_on_the_account() {
        let panel = panel().await;
        let admin = an_admin(&panel.pool, "max").await;
        sqlx::query("UPDATE users SET email = 'max@example.com' WHERE id = ?")
            .bind(admin)
            .execute(&panel.pool)
            .await
            .expect("an address on the account");
        let cookie = sign_in(&panel.pool, admin).await;
        crate::mail::harness::with_key(&panel.mail, &panel.pool).await;

        let sent =
            call(&panel, as_user(send("POST", "/api/v1/admin/mail/test", json!({})), &cookie))
                .await;
        assert_eq!(sent.status(), StatusCode::OK);
        assert_eq!(body_json(sent).await["to"], "max@example.com");
    }

    #[tokio::test]
    async fn a_refused_test_mail_answers_502_with_the_sentence_the_admin_needs() {
        let panel = panel().await;
        let admin = an_admin(&panel.pool, "max").await;
        let cookie = sign_in(&panel.pool, admin).await;
        crate::mail::harness::with_key(&panel.mail, &panel.pool).await;

        panel.resend.answer_next(
            401,
            r#"{"statusCode":401,"name":"restricted_api_key","message":"This API key is restricted to only send emails"}"#,
        );
        let refused = call(
            &panel,
            as_user(
                send("POST", "/api/v1/admin/mail/test", serde_json::json!({"to": "o@example.com"})),
                &cookie,
            ),
        )
        .await;

        assert_eq!(refused.status(), StatusCode::BAD_GATEWAY);
        let body = body_json(refused).await;
        assert_eq!(body["error"], "mail_key_rejected");
        assert!(
            body["message"].as_str().expect("a sentence").contains("resend.com/api-keys"),
            "{body}"
        );
    }

    #[tokio::test]
    async fn the_outbox_reads_back_and_a_bad_filter_is_refused() {
        let panel = panel().await;
        let admin = an_admin(&panel.pool, "max").await;
        let cookie = sign_in(&panel.pool, admin).await;
        crate::mail::harness::with_key(&panel.mail, &panel.pool).await;

        let id = panel
            .mail
            .send(crate::mail::Message::AccountApproved {
                to: crate::mail::Recipient::address("anna@example.com"),
                username: "anna".to_owned(),
            })
            .await
            .expect("queued");

        let list =
            call(&panel, as_user(empty("GET", "/api/v1/admin/mail/outbox?limit=10"), &cookie)).await;
        let body = body_json(list).await;
        assert_eq!(body["total"], 1);
        assert_eq!(body["mails"][0]["id"], id.to_string());
        assert_eq!(body["mails"][0]["kind"], "account_approved");
        assert_eq!(body["mails"][0]["state"], "queued");
        assert_eq!(body["mails"][0]["has_content"], true);
        assert!(body["mails"][0].get("html").is_none(), "the list carries no bodies");

        let filtered =
            call(&panel, as_user(empty("GET", "/api/v1/admin/mail/outbox?state=failed"), &cookie))
                .await;
        assert_eq!(body_json(filtered).await["total"], 0);

        let nonsense =
            call(&panel, as_user(empty("GET", "/api/v1/admin/mail/outbox?state=maybe"), &cookie))
                .await;
        assert_eq!(nonsense.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn the_body_of_a_mail_can_be_read_until_it_is_delivered() {
        let panel = panel().await;
        let admin = an_admin(&panel.pool, "max").await;
        let cookie = sign_in(&panel.pool, admin).await;
        crate::mail::harness::with_key(&panel.mail, &panel.pool).await;

        let id = panel
            .mail
            .send(crate::mail::Message::AccountApproved {
                to: crate::mail::Recipient::address("anna@example.com"),
                username: "anna".to_owned(),
            })
            .await
            .expect("queued");

        let path = format!("/api/v1/admin/mail/outbox/{id}/content");
        let response = call(&panel, as_user(empty("GET", &path), &cookie)).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get(CONTENT_TYPE).expect("a type"),
            "text/html; charset=utf-8"
        );
        assert!(text(response).await.contains("Your account is ready"));

        panel.mail.deliver_next(Timestamp::now()).await.expect("a delivery");
        let gone = call(&panel, as_user(empty("GET", &path), &cookie)).await;
        assert_eq!(gone.status(), StatusCode::NOT_FOUND);
        assert_eq!(body_json(gone).await["error"], "mail_content_gone");

        let nonsense = call(
            &panel,
            as_user(empty("GET", "/api/v1/admin/mail/outbox/not-a-ulid/content"), &cookie),
        )
        .await;
        assert_eq!(nonsense.status(), StatusCode::NOT_FOUND);
        assert_eq!(body_json(nonsense).await["error"], "mail_not_found");
    }

    #[tokio::test]
    async fn retrying_answers_202_and_only_for_a_mail_that_failed() {
        let panel = panel().await;
        let admin = an_admin(&panel.pool, "max").await;
        let cookie = sign_in(&panel.pool, admin).await;
        crate::mail::harness::with_key(&panel.mail, &panel.pool).await;

        let id = panel
            .mail
            .send(crate::mail::Message::AccountApproved {
                to: crate::mail::Recipient::address("anna@example.com"),
                username: "anna".to_owned(),
            })
            .await
            .expect("queued");
        let path = format!("/api/v1/admin/mail/outbox/{id}/retry");

        let early = call(&panel, as_user(empty("POST", &path), &cookie)).await;
        assert_eq!(early.status(), StatusCode::CONFLICT);
        assert_eq!(body_json(early).await["error"], "invalid_state");

        panel.resend.answer_next(451, r#"{"statusCode":451,"name":"security_error","message":"no"}"#);
        panel.mail.deliver_next(Timestamp::now()).await.expect("an attempt");

        let accepted = call(&panel, as_user(empty("POST", &path), &cookie)).await;
        assert_eq!(accepted.status(), StatusCode::ACCEPTED);
    }

    #[tokio::test]
    async fn every_one_of_the_eight_mails_can_be_looked_at_in_the_browser() {
        let panel = panel().await;
        let admin = an_admin(&panel.pool, "max").await;
        let cookie = sign_in(&panel.pool, admin).await;

        for kind in Kind::ALL {
            let path = format!("/api/v1/admin/mail/preview/{kind}");
            let response = call(&panel, as_user(empty("GET", &path), &cookie)).await;
            assert_eq!(response.status(), StatusCode::OK, "{kind}");
            assert_eq!(
                response.headers().get(CONTENT_TYPE).expect("a type"),
                "text/html; charset=utf-8"
            );

            let html = text(response).await;
            assert!(html.contains(&format!("<title>{}</title>", kind.subject())), "{kind}");
            assert!(!html.contains("{{"), "{kind}");
        }

        let unknown =
            call(&panel, as_user(empty("GET", "/api/v1/admin/mail/preview/nope"), &cookie)).await;
        assert_eq!(unknown.status(), StatusCode::NOT_FOUND);
    }
}
