#![cfg(test)]

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::Router;
use sqlx::SqlitePool;

use super::sink::Sink;
use super::Mail;
use crate::model::Id;

#[derive(Debug, Clone)]
pub struct Call {
    pub authorization: Option<String>,
    pub idempotency: Option<String>,
    pub body: serde_json::Value,
}

#[derive(Clone)]
struct Shared {
    calls: Arc<Mutex<Vec<Call>>>,
    answers: Arc<Mutex<VecDeque<(StatusCode, String)>>>,
}

pub struct FakeResend {
    base: String,
    shared: Shared,
}

impl FakeResend {
    pub async fn started() -> Self {
        let shared = Shared {
            calls: Arc::new(Mutex::new(Vec::new())),
            answers: Arc::new(Mutex::new(VecDeque::new())),
        };
        let app = Router::new().route("/emails", post(emails)).with_state(shared.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("a free port");
        let base = format!("http://{}", listener.local_addr().expect("an address"));

        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });

        Self { base, shared }
    }

    pub fn base(&self) -> &str {
        &self.base
    }

    pub fn answer_next(&self, status: u16, body: &str) {
        self.shared
            .answers
            .lock()
            .expect("the answer script")
            .push_back((StatusCode::from_u16(status).expect("a status"), body.to_owned()));
    }

    pub fn calls(&self) -> Vec<Call> {
        self.shared.calls.lock().expect("the call log").clone()
    }
}

async fn emails(State(shared): State<Shared>, headers: HeaderMap, body: Bytes) -> Response {
    let header = |name: &str| {
        headers.get(name).and_then(|value| value.to_str().ok()).map(str::to_owned)
    };
    shared.calls.lock().expect("the call log").push(Call {
        authorization: header("authorization"),
        idempotency: header("idempotency-key"),
        body: serde_json::from_slice(&body).unwrap_or(serde_json::Value::Null),
    });

    match shared.answers.lock().expect("the answer script").pop_front() {
        Some((status, body)) => (status, body).into_response(),
        None => (
            StatusCode::OK,
            r#"{"id":"49a3999c-0ce1-4ea6-ab68-afcd6dc2e794"}"#.to_owned(),
        )
            .into_response(),
    }
}

pub struct DataDir(PathBuf);

impl DataDir {
    pub fn new() -> Self {
        Self(std::env::temp_dir().join(format!("craftpanel-mail-{}", Id::new())))
    }

    pub fn path(&self) -> &Path {
        &self.0
    }

    pub fn key_file(&self) -> PathBuf {
        self.0.join("mail").join("api_key")
    }
}

impl Drop for DataDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

pub fn service(pool: &SqlitePool, dir: &DataDir, base: &str) -> Arc<Mail> {
    Mail::against(pool.clone(), dir.path().to_owned(), base, None)
}

pub fn service_with_sink(pool: &SqlitePool, dir: &DataDir, base: &str) -> Arc<Mail> {
    Mail::against(
        pool.clone(),
        dir.path().to_owned(),
        base,
        Some(Sink::at(dir.path().join("sink"))),
    )
}

pub async fn with_link_base(mail: &Arc<Mail>) {
    let form = super::store::Form {
        from_address: "panel@panel.example".to_owned(),
        from_name: "craftpanel".to_owned(),
        reply_to: None,
        link_base: Some("https://panel.example".to_owned()),
        daily_limit: 100,
    };
    mail.save(form, super::KeyChange::Keep, crate::model::Timestamp::now())
        .await
        .expect("saving the settings");
}

pub async fn with_key(mail: &Arc<Mail>, pool: &SqlitePool) {
    with_link_base(mail).await;
    mail.save(
        super::store::Form {
            from_address: "panel@panel.example".to_owned(),
            from_name: "craftpanel".to_owned(),
            reply_to: None,
            link_base: Some("https://panel.example".to_owned()),
            daily_limit: 100,
        },
        super::KeyChange::Replace("re_test_key".to_owned()),
        crate::model::Timestamp::now(),
    )
    .await
    .expect("saving the key");
    let (set,): (Option<String>,) =
        sqlx::query_as("SELECT key_set_at FROM mail_settings WHERE id = 1")
            .fetch_one(pool)
            .await
            .expect("the settings row");
    assert!(set.is_some(), "the key was not recorded");
}
