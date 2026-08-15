use std::time::Duration;

use super::harness::{service, service_with_sink, DataDir, FakeResend};
use super::*;
use crate::auth::harness::{a_user, test_pool};

const DAY_LONG: Duration = Duration::from_secs(24 * 60 * 60);
const CLOSED: &str = "http://127.0.0.1:1";

fn at(text: &str) -> Timestamp {
    text.parse().expect("a timestamp")
}

fn verify_to(address: &str) -> Message {
    Message::VerifyEmail {
        to: Recipient::address(address),
        username: "anna".to_owned(),
        token: "tok".to_owned(),
        valid_for: DAY_LONG,
    }
}

async fn row_state(pool: &SqlitePool, id: Id) -> (String, i64, Option<String>, Option<String>) {
    sqlx::query_as("SELECT state, attempts, next_attempt_at, html FROM mail_outbox WHERE id = ?")
        .bind(id)
        .fetch_one(pool)
        .await
        .expect("the row")
}

#[tokio::test]
async fn queueing_a_mail_touches_no_network_at_all() {
    let pool = test_pool().await;
    let dir = DataDir::new();
    let mail = service(&pool, &dir, CLOSED);
    harness::with_key(&mail, &pool).await;

    let id = mail.send(verify_to("anna@example.com")).await.expect("the mail is queued");

    let (state, attempts, _, html) = row_state(&pool, id).await;
    assert_eq!(state, "queued");
    assert_eq!(attempts, 0);
    assert!(html.expect("a body").contains("Confirm email address"));
}

#[tokio::test]
async fn without_a_key_nothing_is_queued_and_the_caller_is_told_why() {
    let pool = test_pool().await;
    let dir = DataDir::new();
    let mail = service(&pool, &dir, CLOSED);

    assert!(!mail.configured().await);
    let refusal = mail.send(verify_to("anna@example.com")).await.expect_err("nothing is set up");
    assert_eq!(refusal.code(), "mail_not_configured");

    let (count,): (i64,) = sqlx::query_as("SELECT count(*) FROM mail_outbox")
        .fetch_one(&pool)
        .await
        .expect("a count");
    assert_eq!(count, 0, "a refused mail leaves no row behind");

    mail.notify(Message::PasswordChanged {
        to: Recipient::address("anna@example.com"),
        username: "anna".to_owned(),
        when: Timestamp::now(),
    })
    .await;
}

#[tokio::test]
async fn a_mail_with_a_link_refuses_while_the_panel_has_no_address() {
    let pool = test_pool().await;
    let dir = DataDir::new();
    let mail = service(&pool, &dir, CLOSED);
    harness::with_key(&mail, &pool).await;

    sqlx::query("UPDATE mail_settings SET link_base = NULL WHERE id = 1")
        .execute(&pool)
        .await
        .expect("clearing the panel address");

    let refusal = mail.send(verify_to("anna@example.com")).await.expect_err("no address, no link");
    assert_eq!(refusal.code(), "mail_no_link_base");

    mail.send(Message::PasswordChanged {
        to: Recipient::address("anna@example.com"),
        username: "anna".to_owned(),
        when: Timestamp::now(),
    })
    .await
    .expect("a mail without a link needs no address");
}

#[tokio::test]
async fn the_worker_sends_the_oldest_first_and_empties_the_body_afterwards() {
    let pool = test_pool().await;
    let dir = DataDir::new();
    let resend = FakeResend::started().await;
    let mail = service(&pool, &dir, resend.base());
    harness::with_key(&mail, &pool).await;

    let id = mail.send(verify_to("anna@example.com")).await.expect("queued");
    assert!(mail.deliver_next(Timestamp::now()).await.expect("a delivery"));
    assert!(!mail.deliver_next(Timestamp::now()).await.expect("nothing left"));

    let row: (String, Option<String>, Option<String>, Option<String>, String, String) =
        sqlx::query_as(
            "SELECT state, html, text, provider_id, kind, to_address FROM mail_outbox WHERE id = ?",
        )
        .bind(id)
        .fetch_one(&pool)
        .await
        .expect("the row");

    assert_eq!(row.0, "sent");
    assert_eq!(row.1, None, "the body carried a live link and is gone");
    assert_eq!(row.2, None);
    assert_eq!(row.3.as_deref(), Some("49a3999c-0ce1-4ea6-ab68-afcd6dc2e794"));
    assert_eq!(row.4, "verify_email", "what it was stays readable");
    assert_eq!(row.5, "anna@example.com");

    let calls = resend.calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].authorization.as_deref(), Some("Bearer re_test_key"));
    assert_eq!(calls[0].idempotency.as_deref(), Some(id.to_string().as_str()));
    assert_eq!(calls[0].body["to"], serde_json::json!(["anna@example.com"]));
    assert_eq!(calls[0].body["from"], "craftpanel <panel@panel.example>");
    assert_eq!(calls[0].body["tags"][0]["value"], "verify_email");
    assert!(calls[0].body["html"].as_str().expect("html").contains("panel.example/verify-email#"));
}

#[tokio::test]
async fn a_throttled_mail_waits_and_a_wrong_key_stops_at_once() {
    let pool = test_pool().await;
    let dir = DataDir::new();
    let resend = FakeResend::started().await;
    let mail = service(&pool, &dir, resend.base());
    harness::with_key(&mail, &pool).await;

    let now = at("2026-08-13T21:10:00Z");
    let throttled = mail.send(verify_to("anna@example.com")).await.expect("queued");
    resend.answer_next(429, r#"{"statusCode":429,"name":"rate_limit_exceeded","message":"Too many requests"}"#);
    assert!(mail.deliver_next(now).await.expect("a delivery attempt"));

    let (state, attempts, due, html) = row_state(&pool, throttled).await;
    assert_eq!(state, "queued");
    assert_eq!(attempts, 1);
    assert_eq!(due.as_deref(), Some("2026-08-13T21:10:30Z"));
    assert!(html.is_some(), "a mail that will be tried again keeps its body");

    assert!(!mail.deliver_next(at("2026-08-13T21:10:20Z")).await.expect("nothing due"));
    assert!(mail.deliver_next(at("2026-08-13T21:11:00Z")).await.expect("due again"));
    assert_eq!(row_state(&pool, throttled).await.0, "sent");

    let doomed = mail.send(verify_to("bob@example.com")).await.expect("queued");
    resend.answer_next(401, r#"{"statusCode":401,"name":"validation_error","message":"API key is invalid"}"#);
    assert!(mail.deliver_next(now).await.expect("a delivery attempt"));

    let (state, attempts, due, _) = row_state(&pool, doomed).await;
    assert_eq!(state, "failed", "a wrong key does not get five more tries");
    assert_eq!(attempts, 1);
    assert_eq!(due, None);

    let settings = mail.settings().await.expect("the settings");
    assert!(
        settings.last_error.expect("an error").contains("does not know this key"),
        "the admin page has to say what to do"
    );
}

#[tokio::test]
async fn a_used_up_daily_allowance_waits_for_the_next_day() {
    let pool = test_pool().await;
    let dir = DataDir::new();
    let resend = FakeResend::started().await;
    let mail = service(&pool, &dir, resend.base());
    harness::with_key(&mail, &pool).await;

    let id = mail.send(verify_to("anna@example.com")).await.expect("queued");
    resend.answer_next(
        429,
        r#"{"statusCode":429,"name":"daily_quota_exceeded","message":"You have reached your daily email sending quota"}"#,
    );
    assert!(mail.deliver_next(at("2026-08-13T21:10:00Z")).await.expect("an attempt"));

    let (state, _, due, _) = row_state(&pool, id).await;
    assert_eq!(state, "queued");
    assert_eq!(due.as_deref(), Some("2026-08-14T00:00:00Z"));
}

#[tokio::test]
async fn a_restart_mid_send_sends_again_under_the_same_idempotency_key() {
    let pool = test_pool().await;
    let dir = DataDir::new();
    let resend = FakeResend::started().await;
    let mail = service(&pool, &dir, resend.base());
    harness::with_key(&mail, &pool).await;

    let id = mail.send(verify_to("anna@example.com")).await.expect("queued");
    sqlx::query("UPDATE mail_outbox SET state = 'sending' WHERE id = ?")
        .bind(id)
        .execute(&pool)
        .await
        .expect("pretending the process died mid-send");

    assert!(!mail.deliver_next(Timestamp::now()).await.expect("nothing claimable"));
    assert_eq!(mail.requeue_stuck().await.expect("picking up"), 1);
    assert!(mail.deliver_next(Timestamp::now()).await.expect("a delivery"));

    let calls = resend.calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].idempotency.as_deref(), Some(id.to_string().as_str()));

    assert_eq!(mail.requeue_stuck().await.expect("picking up"), 0);
}

#[tokio::test]
async fn the_sixth_mail_of_one_kind_to_one_address_within_the_hour_is_refused() {
    let pool = test_pool().await;
    let dir = DataDir::new();
    let mail = service(&pool, &dir, CLOSED);
    harness::with_key(&mail, &pool).await;
    let now = at("2026-08-13T21:10:00Z");

    for _ in 0..5 {
        mail.enqueue(verify_to("anna@example.com"), now).await.expect("inside the brake");
    }
    let refusal =
        mail.enqueue(verify_to("anna@example.com"), now).await.expect_err("over the brake");
    assert_eq!(refusal.code(), "mail_rate_limited");

    mail.enqueue(verify_to("bob@example.com"), now).await.expect("a different address");
    mail.enqueue(
        Message::AccountApproved {
            to: Recipient::address("anna@example.com"),
            username: "anna".to_owned(),
        },
        now,
    )
    .await
    .expect("a different kind");

    mail.enqueue(verify_to("anna@example.com"), at("2026-08-13T22:11:00Z"))
        .await
        .expect("the window slid");
}

#[tokio::test]
async fn the_panels_own_daily_limit_is_the_number_the_admin_page_shows() {
    let pool = test_pool().await;
    let dir = DataDir::new();
    let mail = service(&pool, &dir, CLOSED);
    harness::with_key(&mail, &pool).await;
    sqlx::query("UPDATE mail_settings SET daily_limit = 3 WHERE id = 1")
        .execute(&pool)
        .await
        .expect("a small limit");

    let now = at("2026-08-13T21:10:00Z");
    for index in 0..3 {
        mail.enqueue(verify_to(&format!("anna{index}@example.com")), now)
            .await
            .expect("inside the limit");
    }

    let refusal =
        mail.enqueue(verify_to("late@example.com"), now).await.expect_err("over the limit");
    assert_eq!(refusal.code(), "mail_quota_reached");
    assert_eq!(mail.settings_at(now).await.expect("the settings").sent_today, 3);

    sqlx::query("UPDATE mail_settings SET daily_limit = 0 WHERE id = 1")
        .execute(&pool)
        .await
        .expect("no limit");
    mail.enqueue(verify_to("late@example.com"), now).await.expect("no brake at zero");
}

#[tokio::test]
async fn the_file_sink_writes_two_files_and_calls_nobody() {
    let pool = test_pool().await;
    let dir = DataDir::new();
    let resend = FakeResend::started().await;
    let mail = service_with_sink(&pool, &dir, resend.base());
    harness::with_link_base(&mail).await;

    assert!(mail.configured().await, "a sink is a way to deliver mail");
    assert_eq!(
        mail.settings().await.expect("the settings").state,
        ServiceState::FileSink,
        "a redirection nobody can see would be a trap"
    );

    let id = mail.send(verify_to("anna@example.com")).await.expect("queued");
    assert!(mail.deliver_next(at("2026-08-13T21:10:00Z")).await.expect("a delivery"));

    assert!(resend.calls().is_empty(), "not one call may leave the machine");
    let written: Vec<_> = std::fs::read_dir(dir.path().join("sink"))
        .expect("the sink directory")
        .map(|entry| entry.expect("an entry").file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(written.len(), 2, "{written:?}");

    let (state, provider): (String, Option<String>) =
        sqlx::query_as("SELECT state, provider_id FROM mail_outbox WHERE id = ?")
            .bind(id)
            .fetch_one(&pool)
            .await
            .expect("the row");
    assert_eq!(state, "sent");
    assert!(provider.expect("a receipt").starts_with("file:"));
}

#[tokio::test]
async fn the_test_mail_goes_out_inside_the_request_and_lands_in_the_list() {
    let pool = test_pool().await;
    let dir = DataDir::new();
    let resend = FakeResend::started().await;
    let mail = service(&pool, &dir, resend.base());
    harness::with_key(&mail, &pool).await;
    let now = at("2026-08-13T21:10:00Z");

    let sent = mail.send_test("owner@example.com", now).await.expect("a test mail");
    assert_eq!(sent.id, "49a3999c-0ce1-4ea6-ab68-afcd6dc2e794");
    assert_eq!(sent.to, "owner@example.com");
    assert_eq!(resend.calls().len(), 1, "not queued — sent");

    let settings = mail.settings_at(now).await.expect("the settings");
    assert_eq!(settings.last_test_at, Some(now));
    assert_eq!(settings.last_error, None);
    assert_eq!(settings.sent_today, 1, "a test mail spends the allowance like any other");

    let list = mail.outbox(50, None).await.expect("the outbox");
    assert_eq!(list.total, 1);
    assert_eq!(list.mails[0].kind, Kind::Test);
    assert_eq!(list.mails[0].state, State::Sent);
    assert!(!list.mails[0].has_content, "a delivered mail keeps no body");
}

#[tokio::test]
async fn a_refused_test_mail_says_what_to_do_and_leaves_a_failed_row() {
    let pool = test_pool().await;
    let dir = DataDir::new();
    let resend = FakeResend::started().await;
    let mail = service(&pool, &dir, resend.base());
    harness::with_key(&mail, &pool).await;

    resend.answer_next(
        403,
        r#"{"statusCode":403,"name":"validation_error","message":"The panel.example domain is not verified. Please, add and verify your domain on https://resend.com/domains"}"#,
    );
    let refusal =
        mail.send_test("owner@example.com", Timestamp::now()).await.expect_err("not verified");

    assert_eq!(refusal.code(), "mail_sender_rejected");
    assert!(refusal.message().contains("resend.com/domains"));

    let list = mail.outbox(50, None).await.expect("the outbox");
    assert_eq!(list.mails[0].state, State::Failed);
    assert!(list.mails[0].has_content, "a failed mail keeps its body, so it can be looked at");
    assert_eq!(mail.settings().await.expect("the settings").last_test_at, None);
}

#[tokio::test]
async fn a_failed_mail_can_be_sent_again_and_a_delivered_one_cannot() {
    let pool = test_pool().await;
    let dir = DataDir::new();
    let resend = FakeResend::started().await;
    let mail = service(&pool, &dir, resend.base());
    harness::with_key(&mail, &pool).await;

    let id = mail.send(verify_to("anna@example.com")).await.expect("queued");
    resend.answer_next(451, r#"{"statusCode":451,"name":"security_error","message":"flagged"}"#);
    assert!(mail.deliver_next(Timestamp::now()).await.expect("an attempt"));
    assert_eq!(row_state(&pool, id).await.0, "failed");

    assert!(mail.content(id).await.expect("the body").contains("Confirm email address"));
    mail.retry(id).await.expect("back into the queue");
    let (state, attempts, _, _) = row_state(&pool, id).await;
    assert_eq!(state, "queued");
    assert_eq!(attempts, 0, "the counter starts over, or one retry would be the last");

    assert!(mail.deliver_next(Timestamp::now()).await.expect("a delivery"));
    let gone = mail.content(id).await.expect_err("the body is gone");
    assert_eq!(gone.code(), "mail_content_gone");
    let again = mail.retry(id).await.expect_err("nothing left to send");
    assert_eq!(again.code(), "mail_content_gone");

    let missing = mail.retry(Id::new()).await.expect_err("no such row");
    assert_eq!(missing.code(), "mail_not_found");
}

#[tokio::test]
async fn a_queued_mail_may_not_be_pushed_around_by_the_retry_button() {
    let pool = test_pool().await;
    let dir = DataDir::new();
    let mail = service(&pool, &dir, CLOSED);
    harness::with_key(&mail, &pool).await;

    let id = mail.send(verify_to("anna@example.com")).await.expect("queued");
    let refusal = mail.retry(id).await.expect_err("it has not failed");
    assert_eq!(refusal.code(), "invalid_state");
}

#[tokio::test]
async fn the_key_is_a_file_and_stands_in_no_column_of_the_database() {
    use std::os::unix::fs::PermissionsExt;

    let pool = test_pool().await;
    let dir = DataDir::new();
    let mail = service(&pool, &dir, CLOSED);
    harness::with_key(&mail, &pool).await;

    let key_file = dir.key_file();
    assert_eq!(
        std::fs::metadata(&key_file).expect("the key file").permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(
        std::fs::metadata(key_file.parent().expect("the directory"))
            .expect("the directory")
            .permissions()
            .mode()
            & 0o777,
        0o700
    );

    let tables: Vec<(String,)> =
        sqlx::query_as("SELECT name FROM sqlite_master WHERE type = 'table'")
            .fetch_all(&pool)
            .await
            .expect("the tables");
    for (table, ) in tables {
        let columns: Vec<(i64, String, String, i64, Option<String>, i64)> =
            sqlx::query_as(&format!("PRAGMA table_info({table})"))
                .fetch_all(&pool)
                .await
                .expect("the columns");
        for column in columns {
            let (found,): (i64,) = sqlx::query_as(&format!(
                "SELECT count(*) FROM {table} WHERE CAST(\"{}\" AS TEXT) LIKE '%re_test_key%'",
                column.1
            ))
            .fetch_one(&pool)
            .await
            .expect("a search");
            assert_eq!(found, 0, "{table}.{} carries the key", column.1);
        }
    }

    let settings = mail.settings().await.expect("the settings");
    assert_eq!(settings.state, ServiceState::Configured);
    assert!(settings.key_set_at.is_some());
    let json = serde_json::to_string(&settings).expect("json");
    assert!(!json.contains("re_test_key"), "{json}");
}

#[tokio::test]
async fn deleting_the_key_closes_the_door_and_says_so() {
    let pool = test_pool().await;
    let dir = DataDir::new();
    let mail = service(&pool, &dir, CLOSED);
    harness::with_key(&mail, &pool).await;

    mail.forget_key().await.expect("removing the key");
    assert!(!dir.key_file().exists());
    assert!(!mail.configured().await);

    let settings = mail.settings().await.expect("the settings");
    assert_eq!(settings.state, ServiceState::NotConfigured);
    assert_eq!(settings.key_set_at, None);

    mail.forget_key().await.expect("no key, no complaint");
}

#[tokio::test]
async fn saving_the_sender_does_not_take_the_key_with_it() {
    let pool = test_pool().await;
    let dir = DataDir::new();
    let mail = service(&pool, &dir, CLOSED);
    harness::with_key(&mail, &pool).await;

    let form = Form {
        from_address: "hello@panel.example".to_owned(),
        from_name: "The panel".to_owned(),
        reply_to: Some("reply@panel.example".to_owned()),
        link_base: Some("https://panel.example/".to_owned()),
        daily_limit: 50,
    };
    let after = mail.save(form, KeyChange::Keep, Timestamp::now()).await.expect("saving");

    assert_eq!(after.state, ServiceState::Configured);
    assert_eq!(after.from_address, "hello@panel.example");
    assert_eq!(after.link_base.as_deref(), Some("https://panel.example"), "the slash is trimmed");
    assert_eq!(after.example_link.as_deref(), Some("https://panel.example/verify-email#…"));
    assert!(dir.key_file().exists());
}

#[tokio::test]
async fn a_header_cannot_be_broken_open_by_what_the_admin_types() {
    let pool = test_pool().await;
    let dir = DataDir::new();
    let mail = service(&pool, &dir, CLOSED);

    let form = Form {
        from_address: "panel@panel.example\r\nBcc: victim@example.com".to_owned(),
        from_name: "The \"panel\", <ha>\r\nX-Evil: 1".to_owned(),
        reply_to: None,
        link_base: None,
        daily_limit: 100,
    };
    let saved = mail.save(form, KeyChange::Keep, Timestamp::now()).await;

    assert_eq!(saved.expect_err("not an address").code(), "invalid_request");

    let form = Form {
        from_address: "panel@panel.example".to_owned(),
        from_name: "The \"panel\", <ha>\r\nX-Evil: 1".to_owned(),
        reply_to: None,
        link_base: None,
        daily_limit: 100,
    };
    let after = mail.save(form, KeyChange::Keep, Timestamp::now()).await.expect("saving");
    assert!(!after.from_name.contains('\r') && !after.from_name.contains('\n'));
    assert!(!after.from_name.chars().any(|ch| matches!(ch, '<' | '>' | '"' | ',')));
    assert!(!after.from_name.chars().any(char::is_control));
    assert!(after.from_name.starts_with("The panel"), "{}", after.from_name);
}

#[tokio::test]
async fn the_panel_address_needs_a_scheme_and_http_is_allowed() {
    let pool = test_pool().await;
    let dir = DataDir::new();
    let mail = service(&pool, &dir, CLOSED);

    let form = |base: &str| Form {
        from_address: "panel@panel.example".to_owned(),
        from_name: "craftpanel".to_owned(),
        reply_to: None,
        link_base: Some(base.to_owned()),
        daily_limit: 100,
    };

    let refused = mail
        .save(form("panel.example"), KeyChange::Keep, Timestamp::now())
        .await
        .expect_err("no scheme");
    assert_eq!(refused.code(), "invalid_request");

    let allowed = mail
        .save(form("http://192.168.1.10:8080"), KeyChange::Keep, Timestamp::now())
        .await
        .expect("http is a real answer");
    assert_eq!(allowed.link_base.as_deref(), Some("http://192.168.1.10:8080"));
}

#[tokio::test]
async fn a_wrong_key_text_is_refused_before_anything_is_written() {
    let pool = test_pool().await;
    let dir = DataDir::new();
    let mail = service(&pool, &dir, CLOSED);
    let before = mail.settings().await.expect("the settings");

    let form = Form {
        from_address: "panel@panel.example".to_owned(),
        from_name: "craftpanel".to_owned(),
        reply_to: None,
        link_base: None,
        daily_limit: 100,
    };
    let refused = mail
        .save(form, KeyChange::Replace("   ".to_owned()), Timestamp::now())
        .await
        .expect_err("not a key");

    assert_eq!(refused.code(), "invalid_request");
    assert!(!dir.key_file().exists());

    let after = mail.settings().await.expect("the settings");
    assert_eq!(after.from_address, before.from_address, "nothing was written");
    assert_eq!(after.daily_limit, before.daily_limit);
}

#[test]
fn the_six_lines_that_make_this_area_run_are_in_main() {
    const MAIN: &str = include_str!("../main.rs");

    for line in [
        "mod mail;",
        "Mail(mail::cli::MailCommand)",
        "mail::cli::run(command)",
        "mail::Mail::new(pool.clone()",
        "mail.start();",
        "mail::spawn_purge(pool.clone());",
        "api::mail::router(Arc::clone(&mail))",
    ] {
        assert!(MAIN.contains(line), "main.rs no longer carries `{line}`");
    }
}

#[tokio::test]
async fn deleting_an_account_takes_its_unsent_post_with_it() {
    let pool = test_pool().await;
    let dir = DataDir::new();
    let mail = service(&pool, &dir, CLOSED);
    harness::with_key(&mail, &pool).await;

    let anna = a_user(&pool, "anna").await;
    let id = mail
        .send(Message::ResetPassword {
            to: Recipient::account(anna, "anna@example.com"),
            username: "anna".to_owned(),
            token: "tok".to_owned(),
            valid_for: Duration::from_secs(30 * 60),
        })
        .await
        .expect("queued");

    sqlx::query("DELETE FROM users WHERE id = ?")
        .bind(anna)
        .execute(&pool)
        .await
        .expect("deleting the account");

    let left: Option<(String,)> = sqlx::query_as("SELECT state FROM mail_outbox WHERE id = ?")
        .bind(id)
        .fetch_optional(&pool)
        .await
        .expect("a look");
    assert!(left.is_none(), "a reset mail for a deleted account must not go out");
}

#[tokio::test]
async fn the_outbox_is_newest_first_and_can_be_narrowed_to_one_state() {
    let pool = test_pool().await;
    let dir = DataDir::new();
    let mail = service(&pool, &dir, CLOSED);
    harness::with_key(&mail, &pool).await;

    let older = mail
        .enqueue(verify_to("one@example.com"), at("2026-08-13T21:00:00Z"))
        .await
        .expect("queued");
    let newer = mail
        .enqueue(verify_to("two@example.com"), at("2026-08-13T21:05:00Z"))
        .await
        .expect("queued");
    store::mark_failed(&pool, older, "for the test").await.expect("marking");

    let all = mail.outbox(50, None).await.expect("the outbox");
    assert_eq!(all.total, 2);
    assert_eq!(all.mails[0].id, newer, "newest first");

    let failed = mail.outbox(50, Some(State::Failed)).await.expect("the failed ones");
    assert_eq!(failed.total, 1);
    assert_eq!(failed.mails[0].id, older);

    let capped = mail.outbox(1, None).await.expect("one line");
    assert_eq!(capped.mails.len(), 1);
    assert_eq!(capped.total, 2, "total counts the lot, not the page");
}

#[tokio::test]
async fn old_rows_are_swept_after_thirty_days() {
    let pool = test_pool().await;
    let dir = DataDir::new();
    let mail = service(&pool, &dir, CLOSED);
    harness::with_key(&mail, &pool).await;

    let now = Timestamp::now();
    let old = Timestamp::at(now.as_datetime() - time::Duration::days(31));
    mail.enqueue(verify_to("old@example.com"), old).await.expect("queued");
    mail.enqueue(verify_to("new@example.com"), now).await.expect("queued");

    let cutoff = Timestamp::at(now.as_datetime() - RETENTION);
    assert_eq!(store::purge(&pool, cutoff).await.expect("a sweep"), 1);

    let left = mail.outbox(50, None).await.expect("the outbox");
    assert_eq!(left.total, 1);
    assert_eq!(left.mails[0].to_address, "new@example.com");
}

#[test]
fn the_thin_address_check_lets_real_addresses_through() {
    assert!(plausible_address("anna@example.com"));
    assert!(plausible_address("anna+panel@sub.example.co.uk"));
    assert!(plausible_address("onboarding@resend.dev"));

    assert!(!plausible_address("anna"));
    assert!(!plausible_address("anna@"));
    assert!(!plausible_address("@example.com"));
    assert!(!plausible_address("anna@example"), "a domain without a dot reaches nobody");
    assert!(!plausible_address("anna@example.com anna@evil.com"));
    assert!(!plausible_address("two@at@example.com"));
    assert!(!plausible_address(&format!("{}@example.com", "a".repeat(250))));
}
