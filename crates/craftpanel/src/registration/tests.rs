use std::net::IpAddr;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, Response, StatusCode};
use axum::response::IntoResponse;
use sqlx::SqlitePool;
use tower::ServiceExt;

use super::{store, Registrations};
use crate::auth::harness::{
    a_user, an_admin, as_user, body_json, empty, insert_user, send, sign_in, state_with, test_pool,
    FakeHelper, PASSWORD,
};
use crate::auth::{password, secret, settings, users};
use crate::config::Config;
use crate::helper::Helper;
use crate::mail::harness::{with_key, DataDir, FakeResend};
use crate::mail::Mail;
use crate::model::{
    AccountOrigin, Id, PanelRole, RegistrationState, SystemUserState, Timestamp,
};

const CHOSEN: &str = "a-good-password";
const STRANGERS: &str = "a-strangers-password";

struct Panel {
    app: axum::Router,
    pool: SqlitePool,
    service: Arc<Registrations>,
    helper: FakeHelper,
    _dir: DataDir,
    _resend: FakeResend,
}

async fn open_panel() -> Panel {
    let panel = panel().await;
    switch_on(&panel.pool, true, true).await;
    panel
}

async fn panel() -> Panel {
    let pool = test_pool().await;
    let dir = DataDir::new();
    let resend = FakeResend::started().await;
    let mail = crate::mail::harness::service(&pool, &dir, resend.base());
    with_key(&mail, &pool).await;

    let helper = FakeHelper::obliging().await;
    let service =
        Registrations::with_own_rates(pool.clone(), Arc::clone(&mail), Helper::new(helper.socket()));

    let mut config = Config::default();
    config.data_dir = dir.path().to_owned();
    let app = axum::Router::new()
        .nest("/api/v1", crate::api::registration::router(Arc::clone(&service)))
        .with_state(state_with(&pool, config));

    Panel { app, pool, service, helper, _dir: dir, _resend: resend }
}

async fn panel_without_mail() -> Panel {
    let pool = test_pool().await;
    let dir = DataDir::new();
    let resend = FakeResend::started().await;
    let mail = Mail::against(pool.clone(), dir.path().to_owned(), resend.base(), None);

    let helper = FakeHelper::obliging().await;
    let service =
        Registrations::with_own_rates(pool.clone(), Arc::clone(&mail), Helper::new(helper.socket()));

    let mut config = Config::default();
    config.data_dir = dir.path().to_owned();
    let app = axum::Router::new()
        .nest("/api/v1", crate::api::registration::router(Arc::clone(&service)))
        .with_state(state_with(&pool, config));

    Panel { app, pool, service, helper, _dir: dir, _resend: resend }
}

async fn switch_on(pool: &SqlitePool, enabled: bool, requires_approval: bool) {
    sqlx::query(
        "UPDATE panel_settings SET registration_enabled = ?, \
         registration_requires_approval = ? WHERE id = 1",
    )
    .bind(enabled)
    .bind(requires_approval)
    .execute(pool)
    .await
    .expect("switching the sign-up on");
}

async fn call(panel: &Panel, request: Request<Body>) -> Response<Body> {
    panel.app.clone().oneshot(request).await.expect("a response")
}

fn form(username: &str, email: &str) -> serde_json::Value {
    serde_json::json!({ "username": username, "email": email, "password": CHOSEN })
}

fn from(last: u8) -> Option<IpAddr> {
    Some(format!("198.51.100.{last}").parse().expect("an address"))
}

fn a_machine_of_its_own() -> Option<IpAddr> {
    Some(IpAddr::from(std::net::Ipv6Addr::from(rand::random::<u128>())))
}

fn an_address() -> String {
    format!("{}@example.test", Id::new().to_string().to_lowercase())
}

async fn a_token_for(pool: &SqlitePool, id: Id, now: Timestamp) -> String {
    let token = secret::fresh();
    let named = store::replace_token(
        pool,
        id,
        &secret::digest(&token),
        Timestamp::at(now.as_datetime() + time::Duration::hours(24)),
        now,
    )
    .await
    .expect("writing a token");
    assert!(named.is_some(), "no application to mint a token for");
    token
}

async fn confirmation_mails(pool: &SqlitePool, address: &str) -> Vec<String> {
    let bodies: Vec<Option<String>> = sqlx::query_scalar(
        "SELECT text FROM mail_outbox WHERE to_address = ? AND kind = 'verify_email'",
    )
    .bind(address)
    .fetch_all(pool)
    .await
    .expect("the outbox");
    bodies.into_iter().map(|text| text.expect("a mail with a body")).collect()
}

fn token_in(mail: &str) -> String {
    mail.split("/verify-email#")
        .nth(1)
        .expect("a confirmation link")
        .split_whitespace()
        .next()
        .expect("a token")
        .to_owned()
}

fn mail_naming<'a>(mails: &'a [String], username: &str) -> &'a String {
    let greeting = format!("Hi {username},");
    let mut found = mails.iter().filter(|text| text.contains(&greeting));
    let mine = found.next().unwrap_or_else(|| panic!("no mail names {username}: {mails:?}"));
    assert!(found.next().is_none(), "two mails name {username}: {mails:?}");
    mine
}

async fn confirm(pool: &SqlitePool, id: Id, now: Timestamp) {
    let token = a_token_for(pool, id, now).await;
    assert!(store::mark_verified(pool, id, &token, now).await.expect("confirming"));
}

async fn one_application(pool: &SqlitePool) -> store::Row {
    let (rows, total) = store::page(pool, 10, 0).await.expect("the queue");
    assert_eq!(total, 1, "exactly one application");
    rows.into_iter().next().expect("a row")
}

async fn creations(helper: &FakeHelper) -> usize {
    helper
        .calls()
        .iter()
        .filter(|call| matches!(call, craftpanel_proto::HelperRequest::CreateUser { .. }))
        .count()
}

#[tokio::test]
async fn a_closed_panel_says_so_and_writes_nothing() {
    let panel = panel().await;
    let refused = call(&panel, send("POST", "/api/v1/auth/register", form("max", "max@example.test")))
        .await;
    assert_eq!(refused.status(), StatusCode::CONFLICT);
    assert_eq!(body_json(refused).await["error"], "registration_disabled");

    let left: i64 =
        sqlx::query_scalar("SELECT count(*) FROM registrations").fetch_one(&panel.pool).await.unwrap();
    assert_eq!(left, 0);
}

#[tokio::test]
async fn without_mail_the_sign_up_stays_shut_however_the_switch_stands() {
    let panel = panel_without_mail().await;
    switch_on(&panel.pool, true, true).await;

    let options = body_json(call(&panel, empty("GET", "/api/v1/auth/options")).await).await;
    assert_eq!(options["registration_enabled"], false, "the switch is on, the mail is not");
    assert_eq!(options["password_reset_enabled"], false);

    let refused = call(&panel, send("POST", "/api/v1/auth/register", form("max", "max@example.test")))
        .await;
    assert_eq!(refused.status(), StatusCode::CONFLICT);
    assert_eq!(body_json(refused).await["error"], "registration_disabled");
}

#[tokio::test]
async fn a_panel_with_no_address_of_its_own_takes_no_sign_ups_and_tells_nobody_apart() {
    let panel = panel().await;
    switch_on(&panel.pool, true, true).await;
    sqlx::query("UPDATE mail_settings SET link_base = NULL WHERE id = 1")
        .execute(&panel.pool)
        .await
        .expect("clearing the panel address");

    let options = body_json(call(&panel, empty("GET", "/api/v1/auth/options")).await).await;
    assert_eq!(options["registration_enabled"], false, "a key, but no link to send");
    assert_eq!(options["password_reset_enabled"], false, "21.1 stands on a link too");

    let anna = insert_user(&panel.pool, "anna", PanelRole::User, PASSWORD).await;
    sqlx::query("UPDATE users SET email = 'anna@example.test' WHERE id = ?")
        .bind(anna)
        .execute(&panel.pool)
        .await
        .unwrap();

    let mut seen = Vec::new();
    for (name, address) in [("new", "new@example.test"), ("known", "anna@example.test")] {
        let response = call(&panel, send("POST", "/api/v1/auth/register", form(name, address))).await;
        seen.push((response.status(), body_json(response).await));
    }
    assert_eq!(seen[0].0, StatusCode::CONFLICT);
    assert_eq!(seen[0].1["error"], "registration_disabled");
    assert_eq!(seen[0], seen[1], "a known address answers differently");
}

#[tokio::test]
async fn a_sign_up_whose_mail_is_refused_answers_like_all_the_others_and_leaves_nothing() {
    let panel = open_panel().await;
    let anna = insert_user(&panel.pool, "anna", PanelRole::User, PASSWORD).await;
    sqlx::query("UPDATE users SET email = 'anna@example.test' WHERE id = ?")
        .bind(anna)
        .execute(&panel.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE mail_settings SET daily_limit = 1 WHERE id = 1")
        .execute(&panel.pool)
        .await
        .expect("the daily ceiling");
    crate::mail::store::insert(
        &panel.pool,
        &crate::mail::store::NewMail {
            id: Id::new(),
            kind: crate::mail::Kind::Test,
            user: None,
            to_address: "chef@example.test",
            subject: "spent",
            html: "",
            text: "",
            state: crate::mail::store::State::Queued,
            created_at: Timestamp::now(),
        },
    )
    .await
    .expect("spending the allowance");

    let mut seen = Vec::new();
    for (name, address) in [("new", "new@example.test"), ("known", "anna@example.test")] {
        let response = call(&panel, send("POST", "/api/v1/auth/register", form(name, address))).await;
        seen.push((response.status(), body_json(response).await));
    }
    assert_eq!(seen[0].0, StatusCode::ACCEPTED, "{:?}", seen[0].1);
    assert_eq!(seen[0], seen[1], "the mail that was refused says which address is free");

    let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM registrations")
        .fetch_one(&panel.pool)
        .await
        .unwrap();
    assert_eq!(rows, 0, "an application whose confirmation was refused stays behind");

    users::claim_name(&panel.pool, "new", None).await.expect("the name is free");
}

#[tokio::test]
async fn the_options_endpoint_carries_the_three_answers_and_no_more() {
    let panel = open_panel().await;
    let body = body_json(call(&panel, empty("GET", "/api/v1/auth/options")).await).await;

    assert_eq!(body["registration_enabled"], true);
    assert_eq!(body["registration_requires_approval"], true);
    assert_eq!(body["password_reset_enabled"], true);
    assert_eq!(
        body.as_object().unwrap().len(),
        3,
        "no user count, no panel name, no address: it is reachable without a session"
    );
}

#[tokio::test]
async fn a_known_and_an_unknown_address_cost_the_same_argon2() {
    let panel = open_panel().await;
    insert_user(&panel.pool, "anna", PanelRole::User, PASSWORD).await;
    sqlx::query("UPDATE users SET email = 'anna@example.test' WHERE username = 'anna'")
        .execute(&panel.pool)
        .await
        .unwrap();

    let before = password::argon2_runs();
    panel.service.apply("max", "max@example.test", CHOSEN, from(1), Timestamp::now()).await.unwrap();
    let for_a_new_address = password::argon2_runs() - before;

    let before = password::argon2_runs();
    panel
        .service
        .apply("berta", "anna@example.test", CHOSEN, from(2), Timestamp::now())
        .await
        .unwrap();
    let for_a_known_address = password::argon2_runs() - before;

    assert_eq!(for_a_new_address, 1);
    assert_eq!(for_a_known_address, 1, "the known address skipped the work and told on itself");
}

#[tokio::test]
async fn new_known_and_blocked_addresses_answer_identically() {
    let panel = open_panel().await;
    let anna = insert_user(&panel.pool, "anna", PanelRole::User, PASSWORD).await;
    sqlx::query("UPDATE users SET email = 'anna@example.test' WHERE id = ?")
        .bind(anna)
        .execute(&panel.pool)
        .await
        .unwrap();
    store::block(&panel.pool, "blocked@example.test", None, Some("spam"), Timestamp::now())
        .await
        .unwrap();

    let mut seen = Vec::new();
    for (name, address) in [
        ("new", "new@example.test"),
        ("known", "anna@example.test"),
        ("blocked", "blocked@example.test"),
    ] {
        let response = call(&panel, send("POST", "/api/v1/auth/register", form(name, address))).await;
        let status = response.status();
        let headers: Vec<String> = response
            .headers()
            .iter()
            .map(|(name, value)| format!("{name}: {value:?}"))
            .collect();
        seen.push((status, headers, body_json(response).await));
    }

    assert_eq!(seen[0].0, StatusCode::ACCEPTED);
    assert_eq!(seen[0].2["status"], "check_your_email");
    assert_eq!(seen[0], seen[1], "a known address answers differently");
    assert_eq!(seen[0], seen[2], "a blocked address answers differently");

    let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM registrations")
        .fetch_one(&panel.pool)
        .await
        .unwrap();
    assert_eq!(rows, 1, "only the new address became an application");
}

#[tokio::test]
async fn a_taken_name_is_refused_out_loud() {
    let panel = open_panel().await;
    a_user(&panel.pool, "max").await;

    let refused = call(&panel, send("POST", "/api/v1/auth/register", form("max", "max@example.test")))
        .await;
    assert_eq!(refused.status(), StatusCode::CONFLICT);
    assert_eq!(body_json(refused).await["error"], "username_taken");
}

#[tokio::test]
async fn a_name_held_by_an_open_application_is_not_free() {
    let panel = open_panel().await;
    panel.service.apply("max", "max@example.test", CHOSEN, from(1), Timestamp::now()).await.unwrap();

    let refusal = users::claim_name(&panel.pool, "max", None).await.unwrap_err();
    assert_eq!(refusal.code(), "username_taken");
    assert!(refusal.to_string().contains("open sign-up"), "{refusal}");
}

#[tokio::test]
async fn a_second_application_with_the_same_name_hears_the_same_thing() {
    let panel = open_panel().await;
    call(&panel, send("POST", "/api/v1/auth/register", form("max", "max@example.test"))).await;

    let again =
        call(&panel, send("POST", "/api/v1/auth/register", form("max", "second@example.test"))).await;
    assert_eq!(again.status(), StatusCode::CONFLICT);
    assert_eq!(body_json(again).await["error"], "username_taken");
}

#[tokio::test]
async fn what_cannot_be_an_address_or_a_password_is_refused_before_anything_is_written() {
    let panel = open_panel().await;

    let bad_address =
        call(&panel, send("POST", "/api/v1/auth/register", form("max", "not-an-address"))).await;
    assert_eq!(bad_address.status(), StatusCode::BAD_REQUEST);
    assert_eq!(body_json(bad_address).await["error"], "invalid_email");

    let weak = call(
        &panel,
        send(
            "POST",
            "/api/v1/auth/register",
            serde_json::json!({ "username": "max", "email": "max@example.test", "password": "short" }),
        ),
    )
    .await;
    assert_eq!(weak.status(), StatusCode::BAD_REQUEST);
    assert_eq!(body_json(weak).await["error"], "weak_password");

    let shouty =
        call(&panel, send("POST", "/api/v1/auth/register", form("Max", "max@example.test"))).await;
    assert_eq!(shouty.status(), StatusCode::BAD_REQUEST);
    assert_eq!(body_json(shouty).await["error"], "invalid_request");

    let left: i64 =
        sqlx::query_scalar("SELECT count(*) FROM registrations").fetch_one(&panel.pool).await.unwrap();
    assert_eq!(left, 0);
}

#[tokio::test]
async fn a_strangers_application_is_no_claim_on_an_address() {
    let panel = open_panel().await;
    switch_on(&panel.pool, true, false).await;
    let now = Timestamp::now();
    let address = an_address();

    panel.service.apply("stranger", &address, STRANGERS, from(1), now).await.unwrap();
    panel.service.apply("victim", &address.to_uppercase(), CHOSEN, from(2), now).await.unwrap();

    let row = one_application(&panel.pool).await;
    assert_eq!(row.username, "victim", "the later application did not take the row over");
    assert_eq!(row.email, address, "and one address still has one application");

    let mails = confirmation_mails(&panel.pool, &address).await;
    assert_eq!(mails.len(), 2, "both applications sent their link: {mails:?}");
    let his = token_in(mail_naming(&mails, "stranger"));
    let hers = token_in(mail_naming(&mails, "victim"));

    let dead =
        call(&panel, send("POST", "/api/v1/auth/verify-email", serde_json::json!({ "token": his })))
            .await;
    assert_eq!(dead.status(), StatusCode::NOT_FOUND, "the stranger's link still works");
    assert_eq!(body_json(dead).await["error"], "invalid_token");

    let good =
        call(&panel, send("POST", "/api/v1/auth/verify-email", serde_json::json!({ "token": hers })))
            .await;
    assert_eq!(good.status(), StatusCode::OK);
    assert_eq!(body_json(good).await["state"], "active");

    let account = users::by_email(&panel.pool, &address).await.unwrap().expect("an account");
    assert_eq!(account.username, "victim", "the owner's click made the stranger's account");
    assert_eq!(account.origin, AccountOrigin::Registration);

    let app = axum::Router::new()
        .nest("/api/v1", crate::api::session::router())
        .with_state(state_with(&panel.pool, Config::default()));
    let owner = app
        .clone()
        .oneshot(send(
            "POST",
            "/api/v1/auth/login",
            serde_json::json!({ "username": "victim", "password": CHOSEN }),
        ))
        .await
        .unwrap();
    assert_eq!(owner.status(), StatusCode::OK, "the owner cannot get into his own account");

    let stranger = app
        .oneshot(send(
            "POST",
            "/api/v1/auth/login",
            serde_json::json!({ "username": "victim", "password": STRANGERS }),
        ))
        .await
        .unwrap();
    assert_eq!(stranger.status(), StatusCode::UNAUTHORIZED);

    let his_name: i64 = sqlx::query_scalar("SELECT count(*) FROM users WHERE username = 'stranger'")
        .fetch_one(&panel.pool)
        .await
        .unwrap();
    assert_eq!(his_name, 0);
    let left: i64 = sqlx::query_scalar("SELECT count(*) FROM registrations")
        .fetch_one(&panel.pool)
        .await
        .unwrap();
    assert_eq!(left, 0, "the admitted application is gone and no second one was left");
}

#[tokio::test]
async fn a_confirmed_application_is_not_taken_over_by_a_second_form() {
    let panel = open_panel().await;
    let now = Timestamp::now();
    let address = an_address();
    panel.service.apply("waiting", &address, CHOSEN, from(1), now).await.unwrap();

    let mails = confirmation_mails(&panel.pool, &address).await;
    let confirmed = call(
        &panel,
        send(
            "POST",
            "/api/v1/auth/verify-email",
            serde_json::json!({ "token": token_in(mail_naming(&mails, "waiting")) }),
        ),
    )
    .await;
    assert_eq!(confirmed.status(), StatusCode::OK);
    assert_eq!(body_json(confirmed).await["state"], "awaiting_approval");
    let waiting = one_application(&panel.pool).await;

    panel.service.apply("stranger", &address, STRANGERS, from(2), now).await.unwrap();

    let after = one_application(&panel.pool).await;
    assert_eq!(after, waiting, "a second form changed a confirmed application");
    assert_eq!(after.state, RegistrationState::AwaitingApproval);
    assert_eq!(
        confirmation_mails(&panel.pool, &address).await.len(),
        1,
        "and no second link went out: there is nothing left to confirm"
    );

    let admitted = panel
        .service
        .approve(after.id, &crate::auth::LiveServers::none(), &crate::auth::Disks::none())
        .await
        .unwrap();
    assert_eq!(admitted.username, "waiting", "the approval went to the stranger's name");
}

#[tokio::test]
async fn the_same_form_twice_is_one_application_with_one_living_link() {
    let panel = open_panel().await;
    let now = Timestamp::now();
    let address = an_address();

    panel.service.apply("again", &address, CHOSEN, from(1), now).await.unwrap();
    panel.service.apply("again", &address, CHOSEN, from(1), now).await.unwrap();

    let row = one_application(&panel.pool).await;
    assert_eq!(row.username, "again");
    assert_eq!(row.tokens_sent, 1, "the second application counts its own links");

    let mails = confirmation_mails(&panel.pool, &address).await;
    assert_eq!(mails.len(), 2, "each form sent one: {mails:?}");
    let mut answers: Vec<StatusCode> = Vec::new();
    for mail in &mails {
        let response = call(
            &panel,
            send("POST", "/api/v1/auth/verify-email", serde_json::json!({ "token": token_in(mail) })),
        )
        .await;
        answers.push(response.status());
    }
    answers.sort();
    assert_eq!(
        answers,
        [StatusCode::OK, StatusCode::NOT_FOUND],
        "one living link and one dead one: {answers:?}"
    );
}

#[tokio::test]
async fn a_token_whose_row_changed_hands_answers_for_nobody() {
    let panel = open_panel().await;
    let now = Timestamp::now();
    let address = an_address();
    panel.service.apply("stranger", &address, STRANGERS, from(1), now).await.unwrap();

    let row = one_application(&panel.pool).await;
    let his = a_token_for(&panel.pool, row.id, now).await;
    let before = store::password_hash_for_token(&panel.pool, row.id, &his)
        .await
        .unwrap()
        .expect("his own token finds his own hash");

    let hers = secret::fresh();
    let taken = store::take_over(
        &panel.pool,
        row.id,
        store::NewApplication {
            username: "victim",
            email: &address,
            password_hash: "hers".to_owned(),
            signup_ip: None,
            token_hash: secret::digest(&hers),
            token_expires_at: Timestamp::at(now.as_datetime() + time::Duration::hours(24)),
        },
        now,
    )
    .await
    .unwrap();
    assert!(taken);

    assert_eq!(
        store::password_hash_for_token(&panel.pool, row.id, &his).await.unwrap(),
        None,
        "the old token still hands out a password"
    );
    assert_eq!(
        store::password_hash_for_token(&panel.pool, row.id, &hers).await.unwrap().as_deref(),
        Some("hers"),
        "and the new one hands out hers"
    );
    assert_ne!(before, "hers");

    assert!(
        !store::mark_verified(&panel.pool, row.id, &his, now).await.unwrap(),
        "the old link confirmed the application that had replaced it"
    );

    let greeting = store::replace_token(
        &panel.pool,
        row.id,
        &secret::digest(&secret::fresh()),
        Timestamp::at(now.as_datetime() + time::Duration::hours(24)),
        now,
    )
    .await
    .unwrap();
    assert_eq!(greeting.as_deref(), Some("victim"), "a mail would have greeted the applicant before");
}

#[tokio::test]
async fn a_counter_somebody_else_spent_does_not_leave_the_new_application_mute() {
    let panel = open_panel().await;
    let now = Timestamp::now();
    let address = an_address();
    panel.service.apply("stranger", &address, STRANGERS, from(1), now).await.unwrap();

    let row = one_application(&panel.pool).await;
    sqlx::query("UPDATE registrations SET tokens_sent = 5 WHERE id = ?")
        .bind(row.id)
        .execute(&panel.pool)
        .await
        .unwrap();

    panel.service.apply("victim", &address, CHOSEN, from(2), now).await.unwrap();

    let after = one_application(&panel.pool).await;
    assert_eq!(after.username, "victim");
    assert_eq!(after.tokens_sent, 1);

    let mails = confirmation_mails(&panel.pool, &address).await;
    let good = call(
        &panel,
        send(
            "POST",
            "/api/v1/auth/verify-email",
            serde_json::json!({ "token": token_in(mail_naming(&mails, "victim")) }),
        ),
    )
    .await;
    assert_eq!(good.status(), StatusCode::OK, "his link never came or never worked");
}

#[tokio::test]
async fn the_token_itself_is_nowhere_in_the_database() {
    let panel = open_panel().await;
    let now = Timestamp::now();
    panel.service.apply("max", "max@example.test", CHOSEN, from(1), now).await.unwrap();
    let row = one_application(&panel.pool).await;

    let token = a_token_for(&panel.pool, row.id, now).await;
    let stored: String = sqlx::query_scalar("SELECT token_hash FROM registrations WHERE id = ?")
        .bind(row.id)
        .fetch_one(&panel.pool)
        .await
        .unwrap();

    assert_ne!(stored, token);
    assert_eq!(stored, secret::digest(&token));
    assert_eq!(stored.len(), 64, "sha-256 in hex");
    assert_eq!(token.len(), 43, "256 bits in unpadded base64url");
}

#[tokio::test]
async fn a_token_nobody_minted_is_a_404_that_says_what_to_do() {
    let panel = open_panel().await;
    let refused = call(
        &panel,
        send("POST", "/api/v1/auth/verify-email", serde_json::json!({ "token": "nonsense" })),
    )
    .await;

    assert_eq!(refused.status(), StatusCode::NOT_FOUND);
    let body = body_json(refused).await;
    assert_eq!(body["error"], "invalid_token");
    assert!(
        body["message"].as_str().unwrap().contains("sign in"),
        "the second click after an admission lands here: {body}"
    );
}

#[tokio::test]
async fn a_token_that_ran_out_is_gone_and_says_so() {
    let panel = open_panel().await;
    let now = Timestamp::now();
    panel.service.apply("max", "max@example.test", CHOSEN, from(1), now).await.unwrap();
    let row = one_application(&panel.pool).await;
    let token = a_token_for(&panel.pool, row.id, now).await;

    let a_second_late = Timestamp::at(now.as_datetime() + time::Duration::hours(24));
    let refusal = panel
        .service
        .verify(&token, from(1), &crate::auth::LiveServers::none(), &crate::auth::Disks::none(), a_second_late)
        .await
        .unwrap_err();

    assert_eq!(refusal.status(), StatusCode::GONE);
    assert_eq!(refusal.code(), "token_expired");
}

#[tokio::test]
async fn a_second_click_answers_again_while_the_application_waits() {
    let panel = open_panel().await;
    let now = Timestamp::now();
    panel.service.apply("max", "max@example.test", CHOSEN, from(1), now).await.unwrap();
    let row = one_application(&panel.pool).await;
    let token = a_token_for(&panel.pool, row.id, now).await;

    let body = serde_json::json!({ "token": token });
    let first = call(&panel, send("POST", "/api/v1/auth/verify-email", body.clone())).await;
    assert_eq!(first.status(), StatusCode::OK);
    assert_eq!(body_json(first).await["state"], "awaiting_approval");

    let second = call(&panel, send("POST", "/api/v1/auth/verify-email", body)).await;
    assert_eq!(second.status(), StatusCode::OK);
    assert_eq!(body_json(second).await["state"], "awaiting_approval");

    let confirmed: i64 =
        sqlx::query_scalar("SELECT count(*) FROM registrations WHERE state = 'awaiting_approval'")
            .fetch_one(&panel.pool)
            .await
            .unwrap();
    assert_eq!(confirmed, 1, "and it was only confirmed once");
}

#[tokio::test]
async fn asking_again_makes_the_older_link_worthless() {
    let panel = open_panel().await;
    let now = Timestamp::now();
    panel.service.apply("max", "max@example.test", CHOSEN, from(1), now).await.unwrap();
    let row = one_application(&panel.pool).await;
    let old = a_token_for(&panel.pool, row.id, now).await;
    let new = a_token_for(&panel.pool, row.id, now).await;

    let refused = call(
        &panel,
        send("POST", "/api/v1/auth/verify-email", serde_json::json!({ "token": old })),
    )
    .await;
    assert_eq!(refused.status(), StatusCode::NOT_FOUND, "two live links are two windows");

    let good = call(
        &panel,
        send("POST", "/api/v1/auth/verify-email", serde_json::json!({ "token": new })),
    )
    .await;
    assert_eq!(good.status(), StatusCode::OK);
}

#[tokio::test]
async fn confirming_an_address_hands_out_no_cookie() {
    let panel = open_panel().await;
    switch_on(&panel.pool, true, false).await;
    let now = Timestamp::now();
    panel.service.apply("max", "max@example.test", CHOSEN, from(1), now).await.unwrap();
    let row = one_application(&panel.pool).await;
    let token = a_token_for(&panel.pool, row.id, now).await;

    let response = call(
        &panel,
        send("POST", "/api/v1/auth/verify-email", serde_json::json!({ "token": token })),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        !response.headers().contains_key(axum::http::header::SET_COOKIE),
        "a link out of a mailbox does not sign anybody in"
    );
    assert_eq!(body_json(response).await["state"], "active");

    let sessions: i64 =
        sqlx::query_scalar("SELECT count(*) FROM sessions").fetch_one(&panel.pool).await.unwrap();
    assert_eq!(sessions, 0);
}

#[tokio::test]
async fn no_system_user_exists_before_an_administrator_says_yes() {
    let panel = open_panel().await;
    let now = Timestamp::now();

    panel.service.apply("max", "max@example.test", CHOSEN, from(1), now).await.unwrap();
    assert_eq!(creations(&panel.helper).await, 0, "the form alone made a system account");

    let row = one_application(&panel.pool).await;
    let token = a_token_for(&panel.pool, row.id, now).await;
    call(&panel, send("POST", "/api/v1/auth/verify-email", serde_json::json!({ "token": token })))
        .await;
    assert_eq!(creations(&panel.helper).await, 0, "confirming alone made a system account");

    let accounts: i64 =
        sqlx::query_scalar("SELECT count(*) FROM users").fetch_one(&panel.pool).await.unwrap();
    assert_eq!(accounts, 0, "an application is not an account");

    let admin = an_admin(&panel.pool, "chef").await;
    let cookie = sign_in(&panel.pool, admin).await;
    let approved = call(
        &panel,
        as_user(
            empty("POST", &format!("/api/v1/admin/registrations/{}/approve", row.id)),
            &cookie,
        ),
    )
    .await;

    assert_eq!(approved.status(), StatusCode::CREATED);
    assert_eq!(creations(&panel.helper).await, 1, "and exactly one, now");
    assert_eq!(body_json(approved).await["system_user"]["state"], "ready");
}

#[tokio::test]
async fn a_restart_does_not_notice_an_open_application() {
    let panel = open_panel().await;
    panel
        .service
        .apply("max", "max@example.test", CHOSEN, from(1), Timestamp::now())
        .await
        .unwrap();

    let fresh = FakeHelper::obliging().await;
    let ready = users::reconcile(&panel.pool, &Helper::new(fresh.socket())).await.unwrap();

    assert_eq!(ready, 0);
    assert_eq!(creations(&fresh).await, 0);
    let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM registrations")
        .fetch_one(&panel.pool)
        .await
        .unwrap();
    assert_eq!(rows, 1, "and the application is still standing");
}

#[tokio::test]
async fn an_open_application_cannot_be_invited_onto_a_server() {
    let panel = open_panel().await;
    panel
        .service
        .apply("max", "max@example.test", CHOSEN, from(1), Timestamp::now())
        .await
        .unwrap();

    let found = users::search(&panel.pool, "ma", 25).await.unwrap();
    assert!(found.is_empty(), "{found:?}");
}

#[tokio::test]
async fn an_admitted_account_is_a_plain_user_with_the_panel_defaults() {
    let panel = open_panel().await;
    let now = Timestamp::now();
    panel.service.apply("max", "max@example.test", CHOSEN, from(1), now).await.unwrap();
    let row = one_application(&panel.pool).await;
    confirm(&panel.pool, row.id, now).await;

    let answer = panel
        .service
        .approve(row.id, &crate::auth::LiveServers::none(), &crate::auth::Disks::none())
        .await
        .unwrap();

    assert_eq!(answer.panel_role, PanelRole::User);
    assert_eq!(answer.email.as_deref(), Some("max@example.test"));
    assert_eq!(answer.origin, AccountOrigin::Registration);
    assert!(!answer.must_change_password, "he chose his own password");

    let defaults = settings::load(&panel.pool).await.unwrap().default_limits;
    assert_eq!(answer.limits, Some(defaults), "the defaults at the moment of admission");
    assert_eq!(answer.limits.unwrap().disk_mib, defaults.disk_mib);

    let stored = users::by_name(&panel.pool, "max").await.unwrap().expect("the account");
    assert!(password::verify(CHOSEN, &stored.password_hash), "his own password came along");

    let left: i64 =
        sqlx::query_scalar("SELECT count(*) FROM registrations").fetch_one(&panel.pool).await.unwrap();
    assert_eq!(left, 0, "the application is gone");
}

#[tokio::test]
async fn a_body_that_asks_for_admin_gets_a_plain_user() {
    let panel = open_panel().await;
    switch_on(&panel.pool, true, false).await;
    let now = Timestamp::now();

    let response = call(
        &panel,
        send(
            "POST",
            "/api/v1/auth/register",
            serde_json::json!({
                "username": "max",
                "email": "max@example.test",
                "password": CHOSEN,
                "panel_role": "admin",
                "role": "admin",
                "limits": { "memory_mib": 999_999 },
            }),
        ),
    )
    .await;
    assert_eq!(response.status(), StatusCode::ACCEPTED);

    let row = one_application(&panel.pool).await;
    let token = a_token_for(&panel.pool, row.id, now).await;
    call(&panel, send("POST", "/api/v1/auth/verify-email", serde_json::json!({ "token": token })))
        .await;

    let stored = users::by_name(&panel.pool, "max").await.unwrap().expect("the account");
    assert_eq!(stored.role, PanelRole::User);
    assert!(!stored.is_admin());
    assert_eq!(stored.memory_mib, settings::load(&panel.pool).await.unwrap().default_limits.memory_mib);
}

#[tokio::test]
async fn an_unconfirmed_application_cannot_be_approved() {
    let panel = open_panel().await;
    panel
        .service
        .apply("max", "max@example.test", CHOSEN, from(1), Timestamp::now())
        .await
        .unwrap();
    let row = one_application(&panel.pool).await;

    let refusal = panel
        .service
        .approve(row.id, &crate::auth::LiveServers::none(), &crate::auth::Disks::none())
        .await
        .unwrap_err();

    assert_eq!(refusal.status(), StatusCode::CONFLICT);
    assert_eq!(refusal.code(), "invalid_state");
    assert_eq!(creations(&panel.helper).await, 0);
}

#[tokio::test]
async fn a_rejection_takes_the_row_and_blocks_the_address_for_thirty_days() {
    let panel = open_panel().await;
    let now = Timestamp::now();
    panel.service.apply("max", "max@example.test", CHOSEN, from(1), now).await.unwrap();
    let row = one_application(&panel.pool).await;
    confirm(&panel.pool, row.id, now).await;

    panel.service.reject(row.id, Some("not this time"), now).await.unwrap();

    let left: i64 =
        sqlx::query_scalar("SELECT count(*) FROM registrations").fetch_one(&panel.pool).await.unwrap();
    assert_eq!(left, 0);
    assert!(store::is_blocked(&panel.pool, "max@example.test", now).await.unwrap());

    let in_a_month = Timestamp::at(now.as_datetime() + time::Duration::days(31));
    assert!(
        !store::is_blocked(&panel.pool, "max@example.test", in_a_month).await.unwrap(),
        "thirty days, not for ever"
    );

    let kept: Option<String> =
        sqlx::query_scalar("SELECT reason FROM registration_blocks WHERE email = ?")
            .bind("max@example.test")
            .fetch_one(&panel.pool)
            .await
            .unwrap();
    assert_eq!(kept.as_deref(), Some("not this time"));
}

#[tokio::test]
async fn a_rejection_needs_no_body_at_all() {
    let panel = open_panel().await;
    let now = Timestamp::now();
    panel.service.apply("max", "max@example.test", CHOSEN, from(1), now).await.unwrap();
    let row = one_application(&panel.pool).await;

    let admin = an_admin(&panel.pool, "chef").await;
    let cookie = sign_in(&panel.pool, admin).await;
    let response = call(
        &panel,
        as_user(empty("POST", &format!("/api/v1/admin/registrations/{}/reject", row.id)), &cookie),
    )
    .await;

    assert_eq!(response.status(), StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn an_unknown_application_is_a_404_whether_the_id_is_a_ulid_or_not() {
    let panel = open_panel().await;
    let admin = an_admin(&panel.pool, "chef").await;
    let cookie = sign_in(&panel.pool, admin).await;

    for id in [Id::new().to_string(), "not-a-ulid".to_owned()] {
        let response = call(
            &panel,
            as_user(empty("POST", &format!("/api/v1/admin/registrations/{id}/approve")), &cookie),
        )
        .await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{id}");
        assert_eq!(body_json(response).await["error"], "registration_not_found");
    }
}

#[tokio::test]
async fn the_queue_is_admin_only_and_carries_no_secret() {
    let panel = open_panel().await;
    panel
        .service
        .apply("max", "max@example.test", CHOSEN, from(7), Timestamp::now())
        .await
        .unwrap();

    let stranger = a_user(&panel.pool, "anna").await;
    let theirs = sign_in(&panel.pool, stranger).await;
    let refused =
        call(&panel, as_user(empty("GET", "/api/v1/admin/registrations"), &theirs)).await;
    assert_eq!(refused.status(), StatusCode::FORBIDDEN);

    let nobody = call(&panel, empty("GET", "/api/v1/admin/registrations")).await;
    assert_eq!(nobody.status(), StatusCode::UNAUTHORIZED);

    let admin = an_admin(&panel.pool, "chef").await;
    let cookie = sign_in(&panel.pool, admin).await;
    let body =
        body_json(call(&panel, as_user(empty("GET", "/api/v1/admin/registrations"), &cookie)).await)
            .await;

    assert_eq!(body["total"], 1);
    let listed = &body["registrations"][0];
    assert_eq!(listed["username"], "max");
    assert_eq!(listed["email"], "max@example.test");
    assert_eq!(listed["state"], "email_unverified");
    assert_eq!(listed["signup_ip"], "198.51.100.7", "the only trace for triage (20.5)");
    assert!(listed.get("password_hash").is_none(), "{listed}");
    assert!(listed.get("token_hash").is_none(), "{listed}");
    assert!(listed.get("token").is_none(), "{listed}");
}

#[tokio::test]
async fn the_signup_address_disappears_with_the_admission() {
    let panel = open_panel().await;
    switch_on(&panel.pool, true, false).await;
    let now = Timestamp::now();
    panel.service.apply("max", "max@example.test", CHOSEN, from(7), now).await.unwrap();
    let row = one_application(&panel.pool).await;
    assert_eq!(row.signup_ip.as_deref(), Some("198.51.100.7"));

    let token = a_token_for(&panel.pool, row.id, now).await;
    panel
        .service
        .verify(&token, from(7), &crate::auth::LiveServers::none(), &crate::auth::Disks::none(), now)
        .await
        .unwrap();

    let left: i64 =
        sqlx::query_scalar("SELECT count(*) FROM registrations").fetch_one(&panel.pool).await.unwrap();
    assert_eq!(left, 0, "the row went, and the address with it");
}

#[tokio::test]
async fn the_fourth_sign_up_from_one_machine_in_an_hour_waits() {
    let panel = open_panel().await;
    let machine = a_machine_of_its_own();

    for round in 0..3 {
        let name = format!("max{round}{}", Id::new().to_string().to_lowercase());
        panel
            .service
            .apply(&name[..20], &an_address(), CHOSEN, machine, Timestamp::now())
            .await
            .expect("three are free");
    }

    let refusal = panel
        .service
        .apply("fourth", &an_address(), CHOSEN, machine, Timestamp::now())
        .await
        .unwrap_err();
    assert_eq!(refusal.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(refusal.code(), "rate_limited");

    let response = refusal.into_response();
    let wait = response
        .headers()
        .get(axum::http::header::RETRY_AFTER)
        .expect("1.7 asks for Retry-After")
        .to_str()
        .unwrap()
        .parse::<u64>()
        .unwrap();
    assert!((1..=3600).contains(&wait), "Retry-After was {wait}");
}

#[tokio::test]
async fn the_ceiling_holds_over_a_socket_and_the_machine_is_written_down() {
    let panel = open_panel().await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("a free port");
    let base = format!("http://{}", listener.local_addr().expect("an address"));
    let served = panel.app.clone();
    let serving = tokio::spawn(async move {
        let _ = axum::serve(
            listener,
            served.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .await;
    });

    let client = reqwest::Client::new();
    let mut seen = Vec::new();
    for round in 0..4 {
        let name = format!("who{round}");
        let answer = client
            .post(format!("{base}/api/v1/auth/register"))
            .json(&form(&name, &format!("{name}@example.test")))
            .send()
            .await
            .expect("an answer");
        seen.push(answer.status().as_u16());
    }
    serving.abort();

    assert_eq!(&seen[..3], [202, 202, 202], "three an hour are free: {seen:?}");
    assert_eq!(seen[3], 429, "the fourth from one machine waits: {seen:?}");

    let machines: Vec<Option<String>> = sqlx::query_scalar("SELECT signup_ip FROM registrations")
        .fetch_all(&panel.pool)
        .await
        .unwrap();
    assert_eq!(machines.len(), 3);
    assert!(
        machines.iter().all(|ip| ip.as_deref() == Some("127.0.0.1")),
        "the caller's address never reached the handler: {machines:?}"
    );
}

#[test]
fn the_running_panel_hands_the_caller_address_to_the_router() {
    const MAIN: &str = include_str!("../main.rs");
    assert!(
        MAIN.contains("app.into_make_service_with_connect_info::<SocketAddr>()"),
        "main.rs serves the router without connect info: every brake keyed on the caller's \
         address counts nothing"
    );
}

#[tokio::test]
async fn a_second_mail_to_one_address_inside_five_minutes_is_not_sent() {
    let panel = open_panel().await;
    let now = Timestamp::now();
    let address = an_address();
    panel.service.apply("max", &address, CHOSEN, from(1), now).await.unwrap();

    let row = one_application(&panel.pool).await;
    let sent_first = row.tokens_sent;

    let response = call(
        &panel,
        send("POST", "/api/v1/auth/verify-email/resend", serde_json::json!({ "email": address })),
    )
    .await;
    assert_eq!(response.status(), StatusCode::ACCEPTED, "a refusal here would be an answer");
    assert_eq!(body_json(response).await["status"], "check_your_email");

    let after = one_application(&panel.pool).await;
    assert_eq!(after.tokens_sent, sent_first, "no second token, no second mail");
}

#[tokio::test]
async fn resending_to_an_unknown_address_answers_exactly_like_a_known_one() {
    let panel = open_panel().await;
    let address = an_address();
    panel.service.apply("max", &address, CHOSEN, from(1), Timestamp::now()).await.unwrap();

    let mut seen = Vec::new();
    for asked in [address.as_str(), "nobody@example.test", "not-an-address-at-all"] {
        let response = call(
            &panel,
            send("POST", "/api/v1/auth/verify-email/resend", serde_json::json!({ "email": asked })),
        )
        .await;
        seen.push((response.status(), body_json(response).await));
    }

    assert_eq!(seen[0].0, StatusCode::ACCEPTED);
    assert_eq!(seen[0], seen[1], "an unknown address answers differently");
    assert_eq!(seen[0], seen[2], "even nonsense gets the same answer");
}

#[tokio::test]
async fn an_application_gets_five_links_and_no_more() {
    let panel = open_panel().await;
    let now = Timestamp::now();
    let address = an_address();
    panel.service.apply("max", &address, CHOSEN, from(1), now).await.unwrap();

    let row = one_application(&panel.pool).await;
    sqlx::query("UPDATE registrations SET tokens_sent = 5 WHERE id = ?")
        .bind(row.id)
        .execute(&panel.pool)
        .await
        .unwrap();
    let hash_before: String =
        sqlx::query_scalar("SELECT token_hash FROM registrations WHERE id = ?")
            .bind(row.id)
            .fetch_one(&panel.pool)
            .await
            .unwrap();

    let response = call(
        &panel,
        send("POST", "/api/v1/auth/verify-email/resend", serde_json::json!({ "email": address })),
    )
    .await;
    assert_eq!(response.status(), StatusCode::ACCEPTED, "a refusal here would be an answer");

    let after = one_application(&panel.pool).await;
    assert_eq!(after.tokens_sent, 5, "whoever cannot find five links needs a different address");
    let hash_after: String =
        sqlx::query_scalar("SELECT token_hash FROM registrations WHERE id = ?")
            .bind(row.id)
            .fetch_one(&panel.pool)
            .await
            .unwrap();
    assert_eq!(hash_after, hash_before, "and the link he has keeps working");
}

#[tokio::test]
async fn an_unconfirmed_application_can_be_told_apart_only_with_the_right_password() {
    let panel = open_panel().await;
    panel
        .service
        .apply("max", "max@example.test", CHOSEN, from(1), Timestamp::now())
        .await
        .unwrap();

    let app = axum::Router::new()
        .nest("/api/v1", crate::api::session::router())
        .with_state(state_with(&panel.pool, Config::default()));

    let right = app
        .clone()
        .oneshot(send(
            "POST",
            "/api/v1/auth/login",
            serde_json::json!({ "username": "max", "password": CHOSEN }),
        ))
        .await
        .unwrap();
    assert_eq!(right.status(), StatusCode::FORBIDDEN);
    assert_eq!(body_json(right).await["error"], "email_unverified");

    let wrong = app
        .oneshot(send(
            "POST",
            "/api/v1/auth/login",
            serde_json::json!({ "username": "max", "password": "wrong-wrong-wrong" }),
        ))
        .await
        .unwrap();
    assert_eq!(wrong.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(body_json(wrong).await["error"], "invalid_credentials");
}

#[tokio::test]
async fn an_application_waiting_for_approval_says_that_instead() {
    let panel = open_panel().await;
    let now = Timestamp::now();
    panel.service.apply("max", "max@example.test", CHOSEN, from(1), now).await.unwrap();
    let row = one_application(&panel.pool).await;
    confirm(&panel.pool, row.id, now).await;

    let app = axum::Router::new()
        .nest("/api/v1", crate::api::session::router())
        .with_state(state_with(&panel.pool, Config::default()));

    let response = app
        .oneshot(send(
            "POST",
            "/api/v1/auth/login",
            serde_json::json!({ "username": "max", "password": CHOSEN }),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert!(
        !response.headers().contains_key(axum::http::header::SET_COOKIE),
        "no session for half an account"
    );
    assert_eq!(body_json(response).await["error"], "approval_pending");
}

#[tokio::test]
async fn a_name_that_is_neither_an_account_nor_an_application_costs_the_same_argon2() {
    let panel = open_panel().await;
    panel
        .service
        .apply("max", "max@example.test", CHOSEN, from(1), Timestamp::now())
        .await
        .unwrap();
    password::verify_against_nobody("warm the decoy");

    let app = axum::Router::new()
        .nest("/api/v1", crate::api::session::router())
        .with_state(state_with(&panel.pool, Config::default()));

    let before = password::argon2_runs();
    app.clone()
        .oneshot(send(
            "POST",
            "/api/v1/auth/login",
            serde_json::json!({ "username": "max", "password": "wrong-wrong-wrong" }),
        ))
        .await
        .unwrap();
    let for_an_application = password::argon2_runs() - before;

    let before = password::argon2_runs();
    app.oneshot(send(
        "POST",
        "/api/v1/auth/login",
        serde_json::json!({ "username": "nobody", "password": "wrong-wrong-wrong" }),
    ))
    .await
    .unwrap();
    let for_a_stranger = password::argon2_runs() - before;

    assert_eq!(for_an_application, 1);
    assert_eq!(for_a_stranger, 1, "an application told on itself by being slower");
}

#[tokio::test]
async fn the_sweep_takes_what_is_stale_and_leaves_the_rest() {
    let panel = open_panel().await;
    let now = Timestamp::now();

    let ages = [
        ("six", "six@example.test", 6, RegistrationState::EmailUnverified),
        ("eight", "eight@example.test", 8, RegistrationState::EmailUnverified),
        ("twenty", "twenty@example.test", 20, RegistrationState::AwaitingApproval),
        ("thirty", "thirty@example.test", 31, RegistrationState::AwaitingApproval),
    ];
    for (name, email, days, state) in ages {
        let when = Timestamp::at(now.as_datetime() - time::Duration::days(days));
        let id = store::insert(
            &panel.pool,
            store::NewApplication {
                username: name,
                email,
                password_hash: "x".to_owned(),
                signup_ip: None,
                token_hash: secret::digest(&secret::fresh()),
                token_expires_at: when,
            },
            when,
        )
        .await
        .unwrap();
        if state == RegistrationState::AwaitingApproval {
            confirm(&panel.pool, id, when).await;
        }
    }

    store::block(
        &panel.pool,
        "expired@example.test",
        Some(Timestamp::at(now.as_datetime() - time::Duration::days(1))),
        None,
        now,
    )
    .await
    .unwrap();
    store::block(&panel.pool, "for-ever@example.test", None, Some("by hand"), now).await.unwrap();

    let swept = panel.service.sweep(now).await.unwrap();
    assert_eq!(swept.unverified, 1);
    assert_eq!(swept.waiting, 1);
    assert_eq!(swept.blocks, 1);

    let left: Vec<String> = sqlx::query_scalar("SELECT username FROM registrations ORDER BY username")
        .fetch_all(&panel.pool)
        .await
        .unwrap();
    assert_eq!(left, vec!["six".to_owned(), "twenty".to_owned()]);

    let blocks: Vec<String> = sqlx::query_scalar("SELECT email FROM registration_blocks")
        .fetch_all(&panel.pool)
        .await
        .unwrap();
    assert_eq!(blocks, vec!["for-ever@example.test".to_owned()], "the operator's own block stays");
}

#[tokio::test]
async fn an_address_on_an_open_application_is_not_free_for_the_admin_ways() {
    let panel = open_panel().await;
    panel
        .service
        .apply("max", "max@example.test", CHOSEN, from(1), Timestamp::now())
        .await
        .unwrap();

    let refusal = users::claim_email(&panel.pool, "max@example.test", None).await.unwrap_err();
    assert_eq!(refusal.code(), "email_taken");
    assert_eq!(refusal.status(), StatusCode::CONFLICT);

    assert!(users::claim_email(&panel.pool, "free@example.test", None).await.is_ok());
}

#[tokio::test]
async fn accounts_made_by_hand_keep_no_address_and_say_where_they_came_from() {
    let pool = test_pool().await;
    let id = a_user(&pool, "max").await;
    let row = users::load(&pool, id).await.unwrap();

    assert_eq!(row.email, None);
    assert_eq!(row.origin, AccountOrigin::Admin, "the default of migration 0010");
    assert_eq!(row.system_state, SystemUserState::Ready);
}

#[test]
fn the_lines_that_make_these_two_areas_run_are_in_main() {
    const MAIN: &str = include_str!("../main.rs");

    for line in [
        "mod registration;",
        "registration::Registrations::new(pool.clone()",
        "auth::reset::Recovery::new(pool.clone()",
        "registration::spawn_sweep(Arc::clone(&sign_ups));",
        "api::registration::with_live(Arc::clone(&sign_ups)",
        "api::recovery::router(Arc::clone(&recovery))",
    ] {
        assert!(MAIN.contains(line), "main.rs no longer carries `{line}`");
    }
}

#[tokio::test]
async fn every_path_of_sections_20_and_21_is_answered_by_something() {
    let panel = open_panel().await;
    let recovery = crate::auth::reset::Recovery::new(
        panel.pool.clone(),
        crate::mail::Mail::against(
            panel.pool.clone(),
            std::env::temp_dir().join(format!("craftpanel-wiring-{}", Id::new())),
            "http://127.0.0.1:1",
            None,
        ),
    );
    let app = axum::Router::new()
        .nest(
            "/api/v1",
            crate::api::registration::router(Arc::clone(&panel.service))
                .merge(crate::api::recovery::router(recovery)),
        )
        .with_state(state_with(&panel.pool, Config::default()));

    let admin = an_admin(&panel.pool, "chef").await;
    let cookie = sign_in(&panel.pool, admin).await;
    let some = Id::new();


    let calls: Vec<(&str, String, serde_json::Value)> = vec![
        ("GET", "/api/v1/auth/options".to_owned(), serde_json::Value::Null),
        ("POST", "/api/v1/auth/register".to_owned(), form("who", "who@example.test")),
        ("POST", "/api/v1/auth/verify-email".to_owned(), serde_json::json!({ "token": "x" })),
        (
            "POST",
            "/api/v1/auth/verify-email/resend".to_owned(),
            serde_json::json!({ "email": "who@example.test" }),
        ),
        ("GET", "/api/v1/admin/registrations".to_owned(), serde_json::Value::Null),
        ("POST", format!("/api/v1/admin/registrations/{some}/approve"), serde_json::Value::Null),
        ("POST", format!("/api/v1/admin/registrations/{some}/reject"), serde_json::Value::Null),
        (
            "POST",
            "/api/v1/auth/password-reset".to_owned(),
            serde_json::json!({ "email": "who@example.test" }),
        ),
        (
            "POST",
            "/api/v1/auth/password-reset/verify".to_owned(),
            serde_json::json!({ "token": "x" }),
        ),
        (
            "POST",
            "/api/v1/auth/password-reset/confirm".to_owned(),
            serde_json::json!({ "token": "x", "new_password": "a-good-password" }),
        ),
        ("POST", format!("/api/v1/admin/users/{some}/password-reset"), serde_json::Value::Null),
    ];

    for (method, path, body) in calls {
        let request = if body.is_null() {
            as_user(empty(method, &path), &cookie)
        } else {
            as_user(send(method, &path, body), &cookie)
        };
        let response = app.clone().oneshot(request).await.expect("a response");
        let status = response.status();
        assert_ne!(
            status,
            StatusCode::METHOD_NOT_ALLOWED,
            "{method} {path} is mounted under another method"
        );

        if status == StatusCode::NOT_FOUND {
            let body = body_json(response).await;
            assert!(
                body.get("error").and_then(serde_json::Value::as_str).is_some(),
                "{method} {path} is not mounted: {body}"
            );
        }
    }
}
