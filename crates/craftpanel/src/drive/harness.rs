#![cfg(test)]

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use axum::body::Bytes;
use axum::extract::{Path as RoutePath, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use sqlx::SqlitePool;

use crate::model::Id;

use super::Drive;

#[derive(Debug, Default, Clone)]
struct Session {
    bytes: Vec<u8>,
    total: u64,
    name: String,
    parent: Option<String>,
    backup_id: Option<String>,
    server_id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct StoredFile {
    pub id: String,
    pub name: String,
    pub bytes: Vec<u8>,
    pub trashed: bool,
    pub panel: Option<String>,
    pub server_id: Option<String>,
    pub backup_id: Option<String>,
    pub folder: bool,
}

#[derive(Debug, Default)]
struct Script {
    chunk_faults: HashMap<usize, u16>,
    short_after: Option<usize>,
    move_after: Option<usize>,
    pending_polls: usize,
    device_outcome: Option<(u16, String)>,
    revoked: bool,
    drive_full: bool,
    hold_first_chunk: Option<std::time::Duration>,
}

#[derive(Clone)]
struct Shared {
    calls: Arc<Mutex<Vec<String>>>,
    sessions: Arc<Mutex<HashMap<String, Session>>>,
    files: Arc<Mutex<Vec<StoredFile>>>,
    script: Arc<Mutex<Script>>,
    chunks: Arc<Mutex<usize>>,
    polls: Arc<Mutex<usize>>,
}

pub struct FakeGoogle {
    base: String,
    shared: Shared,
}

impl FakeGoogle {
    pub async fn started() -> Self {
        let shared = Shared {
            calls: Arc::new(Mutex::new(Vec::new())),
            sessions: Arc::new(Mutex::new(HashMap::new())),
            files: Arc::new(Mutex::new(Vec::new())),
            script: Arc::new(Mutex::new(Script::default())),
            chunks: Arc::new(Mutex::new(0)),
            polls: Arc::new(Mutex::new(0)),
        };

        let app = Router::new()
            .route("/device/code", post(device_code))
            .route("/token", post(token))
            .route("/revoke", post(revoke))
            .route("/drive/v3/about", get(about))
            .route("/drive/v3/files", get(list).post(create))
            .route(
                "/drive/v3/files/{id}",
                get(one).delete(remove),
            )
            .route("/upload/drive/v3/files", post(open_session))
            .route(
                "/upload/session/{id}",
                axum::routing::put(chunk)
                    .layer(axum::extract::DefaultBodyLimit::disable()),
            )
            .with_state(shared.clone());

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

    pub fn calls(&self) -> Vec<String> {
        self.shared.calls.lock().expect("the call log").clone()
    }

    pub fn files(&self) -> Vec<StoredFile> {
        self.shared.files.lock().expect("the files").clone()
    }

    pub fn file_named(&self, name: &str) -> Option<StoredFile> {
        self.files().into_iter().find(|file| file.name == name)
    }

    pub fn file_of_backup(&self, backup: Id) -> Option<StoredFile> {
        let wanted = backup.to_string();
        self.files().into_iter().find(|file| file.backup_id.as_deref() == Some(&wanted))
    }

    pub fn put_stranger(&self, name: &str) -> String {
        let id = format!("stranger-{}", Id::new());
        self.shared.files.lock().expect("the files").push(StoredFile {
            id: id.clone(),
            name: name.to_owned(),
            bytes: b"a holiday photo".to_vec(),
            trashed: false,
            panel: None,
            server_id: None,
            backup_id: None,
            folder: false,
        });
        id
    }

    pub fn put_orphan(&self, backup: &str) -> String {
        let id = format!("orphan-{}", Id::new());
        self.shared.files.lock().expect("the files").push(StoredFile {
            id: id.clone(),
            name: format!("orphan--{backup}.tar.zst"),
            bytes: b"an archive nobody points at".to_vec(),
            trashed: false,
            panel: Some(super::PANEL_TAG.to_owned()),
            server_id: Some(Id::new().to_string()),
            backup_id: Some(backup.to_owned()),
            folder: false,
        });
        id
    }

    pub fn forget_file(&self, id: &str) {
        self.shared.files.lock().expect("the files").retain(|file| file.id != id);
    }

    pub fn trash_file(&self, id: &str) {
        for file in self.shared.files.lock().expect("the files").iter_mut() {
            if file.id == id {
                file.trashed = true;
            }
        }
    }

    pub fn chunks_seen(&self) -> usize {
        *self.shared.chunks.lock().expect("the chunk count")
    }

    pub fn fail_chunk(&self, number: usize, status: u16) {
        self.shared.script.lock().expect("the script").chunk_faults.insert(number, status);
    }

    pub fn acknowledge_short_after(&self, chunk: usize) {
        self.shared.script.lock().expect("the script").short_after = Some(chunk);
    }

    pub fn move_session_after(&self, chunk: usize) {
        self.shared.script.lock().expect("the script").move_after = Some(chunk);
    }

    pub fn keep_waiting(&self, polls: usize) {
        self.shared.script.lock().expect("the script").pending_polls = polls;
    }

    pub fn decline_the_code(&self) {
        self.end_the_flow_with(403, include_str!("testdata/access_denied.json"));
    }

    pub fn end_the_flow_with(&self, status: u16, body: &str) {
        self.shared.script.lock().expect("the script").device_outcome =
            Some((status, body.to_owned()));
    }

    pub fn withdraw_the_connection(&self) {
        self.shared.script.lock().expect("the script").revoked = true;
    }

    pub fn fill_the_drive(&self) {
        self.shared.script.lock().expect("the script").drive_full = true;
    }

    pub fn hold_the_first_chunk(&self, how_long: std::time::Duration) {
        self.shared.script.lock().expect("the script").hold_first_chunk = Some(how_long);
    }
}

fn note(shared: &Shared, what: &str) {
    shared.calls.lock().expect("the call log").push(what.to_owned());
}

async fn device_code(State(shared): State<Shared>, body: String) -> Response {
    note(&shared, "device/code");
    assert!(body.contains("scope="), "the scope has to be asked for: {body}");
    assert!(
        body.contains("drive.file"),
        "the scope has to be drive.file and nothing wider: {body}"
    );
    (
        StatusCode::OK,
        [(axum::http::header::CONTENT_TYPE, "application/json")],
        r#"{"device_code":"the-device-code","user_code":"GQVQ-JKEC","expires_in":1800,
            "interval":1,"verification_url":"https://www.google.com/device"}"#,
    )
        .into_response()
}

async fn token(State(shared): State<Shared>, body: String) -> Response {
    let refreshing = body.contains("grant_type=refresh_token");
    note(&shared, if refreshing { "token/refresh" } else { "token/device" });

    let script = shared.script.lock().expect("the script");
    if refreshing {
        if script.revoked {
            return json(400, r#"{"error":"invalid_grant","error_description":"Token has been expired or revoked."}"#);
        }
        return json(200, r#"{"access_token":"ya29.fresh","expires_in":3599,"token_type":"Bearer"}"#);
    }

    if let Some((status, body)) = script.device_outcome.clone() {
        return json(status, &body);
    }
    let pending = script.pending_polls;
    drop(script);

    let mut polls = shared.polls.lock().expect("the poll count");
    *polls += 1;
    if *polls <= pending {
        return json(428, r#"{"error":"authorization_pending"}"#);
    }
    json(
        200,
        r#"{"access_token":"ya29.first","refresh_token":"1//the-refresh-token",
            "expires_in":3599,"token_type":"Bearer"}"#,
    )
}

async fn revoke(State(shared): State<Shared>) -> Response {
    note(&shared, "revoke");
    (StatusCode::OK, "").into_response()
}

async fn about(State(shared): State<Shared>, headers: HeaderMap) -> Response {
    note(&shared, "about");
    if let Some(refusal) = needs_token(&headers) {
        return refusal;
    }
    json(200, include_str!("testdata/about.json"))
}

#[derive(serde::Deserialize)]
struct ListQuery {
    #[serde(default)]
    q: String,
}

async fn list(
    State(shared): State<Shared>,
    headers: HeaderMap,
    Query(query): Query<ListQuery>,
) -> Response {
    note(&shared, "files/list");
    if let Some(refusal) = needs_token(&headers) {
        return refusal;
    }

    let files = shared.files.lock().expect("the files");
    let folders_only = query.q.contains("application/vnd.google-apps.folder");
    let mine: Vec<&StoredFile> = files
        .iter()
        .filter(|file| file.panel.as_deref() == Some(super::PANEL_TAG))
        .filter(|file| file.folder == folders_only)
        .filter(|file| !folders_only || !file.trashed)
        .collect();

    let rendered: Vec<String> = mine
        .iter()
        .map(|file| {
            format!(
                r#"{{"id":"{}","name":"{}","size":"{}","trashed":{},"md5Checksum":"{}",
                    "appProperties":{{"panel":"{}","server_id":"{}","backup_id":"{}"}}}}"#,
                file.id,
                file.name,
                file.bytes.len(),
                file.trashed,
                md5_of(&file.bytes),
                super::PANEL_TAG,
                file.server_id.clone().unwrap_or_default(),
                file.backup_id.clone().unwrap_or_default(),
            )
        })
        .collect();
    json(200, &format!(r#"{{"files":[{}]}}"#, rendered.join(",")))
}

async fn create(State(shared): State<Shared>, headers: HeaderMap, body: Bytes) -> Response {
    note(&shared, "files/create");
    if let Some(refusal) = needs_token(&headers) {
        return refusal;
    }
    let parsed: serde_json::Value = serde_json::from_slice(&body).unwrap_or_default();
    let id = format!("folder-{}", Id::new());
    shared.files.lock().expect("the files").push(StoredFile {
        id: id.clone(),
        name: parsed["name"].as_str().unwrap_or("unnamed").to_owned(),
        bytes: Vec::new(),
        trashed: false,
        panel: Some(super::PANEL_TAG.to_owned()),
        server_id: None,
        backup_id: None,
        folder: true,
    });
    json(200, &format!(r#"{{"id":"{id}"}}"#))
}

async fn one(
    State(shared): State<Shared>,
    headers: HeaderMap,
    RoutePath(id): RoutePath<String>,
    Query(query): Query<HashMap<String, String>>,
) -> Response {
    if let Some(refusal) = needs_token(&headers) {
        return refusal;
    }
    let media = query.get("alt").map(String::as_str) == Some("media");
    note(&shared, if media { "files/download" } else { "files/get" });

    let files = shared.files.lock().expect("the files");
    let Some(file) = files.iter().find(|file| file.id == id) else {
        return json(404, r#"{"error":{"code":404,"message":"File not found."}}"#);
    };
    if media {
        return (StatusCode::OK, file.bytes.clone()).into_response();
    }
    json(
        200,
        &format!(
            r#"{{"id":"{}","name":"{}","size":"{}","trashed":{},"md5Checksum":"{}"}}"#,
            file.id,
            file.name,
            file.bytes.len(),
            file.trashed,
            md5_of(&file.bytes)
        ),
    )
}

async fn remove(
    State(shared): State<Shared>,
    headers: HeaderMap,
    RoutePath(id): RoutePath<String>,
) -> Response {
    note(&shared, "files/delete");
    if let Some(refusal) = needs_token(&headers) {
        return refusal;
    }
    shared.files.lock().expect("the files").retain(|file| file.id != id);
    StatusCode::NO_CONTENT.into_response()
}

async fn open_session(
    State(shared): State<Shared>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    note(&shared, "upload/begin");
    if let Some(refusal) = needs_token(&headers) {
        return refusal;
    }

    let declared = headers
        .get("x-upload-content-length")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok());
    assert!(
        declared.is_some(),
        "a resumable session without X-Upload-Content-Length cannot be resumed"
    );

    let metadata: serde_json::Value = serde_json::from_slice(&body).unwrap_or_default();
    let properties = &metadata["appProperties"];
    let id = format!("session-{}", Id::new());
    let session = session_url(&host(&headers), &id);
    shared.sessions.lock().expect("the sessions").insert(
        id.clone(),
        Session {
            bytes: Vec::new(),
            total: declared.unwrap_or(0),
            name: metadata["name"].as_str().unwrap_or("unnamed").to_owned(),
            parent: metadata["parents"][0].as_str().map(str::to_owned),
            backup_id: properties["backup_id"].as_str().map(str::to_owned),
            server_id: properties["server_id"].as_str().map(str::to_owned),
        },
    );

    Response::builder()
        .status(StatusCode::OK)
        .header(axum::http::header::LOCATION, session)
        .body(axum::body::Body::empty())
        .expect("a response")
}

async fn chunk(
    State(shared): State<Shared>,
    RoutePath(id): RoutePath<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    note(&shared, "upload/chunk");

    let range = headers
        .get(axum::http::header::CONTENT_RANGE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_owned();

    let number = {
        let mut chunks = shared.chunks.lock().expect("the chunk count");
        if !range.starts_with("bytes */") {
            *chunks += 1;
        }
        *chunks
    };

    let (fault, short, moved, hold, full) = {
        let script = shared.script.lock().expect("the script");
        (
            script.chunk_faults.get(&number).copied(),
            script.short_after == Some(number),
            script.move_after == Some(number),
            if number == 1 { script.hold_first_chunk } else { None },
            script.drive_full,
        )
    };

    if let Some(hold) = hold {
        tokio::time::sleep(hold).await;
    }
    if full {
        return json(403, include_str!("testdata/storage_quota_exceeded.json"));
    }
    if let Some(status) = fault {
        return json(status, r#"{"error":{"code":503,"message":"Backend Error"}}"#);
    }

    let mut sessions = shared.sessions.lock().expect("the sessions");
    let Some(session) = sessions.get_mut(&id) else {
        return json(404, r#"{"error":{"code":404,"message":"Upload session not found."}}"#);
    };

    if range.starts_with("bytes */") {
        return progress(session.bytes.len() as u64, None);
    }

    let offset = range
        .trim_start_matches("bytes ")
        .split('-')
        .next()
        .and_then(|start| start.parse::<u64>().ok())
        .unwrap_or(0);
    assert_eq!(
        offset as usize,
        session.bytes.len(),
        "a chunk arrived at the wrong offset — the resume is broken, and the file would \
         have a hole in it"
    );

    let mut arrived = body.to_vec();
    if short && arrived.len() > 1 {
        arrived.truncate(arrived.len() / 2);
    }
    session.bytes.extend_from_slice(&arrived);

    if session.bytes.len() as u64 >= session.total {
        let file = StoredFile {
            id: format!("file-{}", Id::new()),
            name: session.name.clone(),
            bytes: session.bytes.clone(),
            trashed: false,
            panel: Some(super::PANEL_TAG.to_owned()),
            server_id: session.server_id.clone(),
            backup_id: session.backup_id.clone(),
            folder: false,
        };
        let answer = format!(
            r#"{{"id":"{}","name":"{}","size":"{}","md5Checksum":"{}"}}"#,
            file.id,
            file.name,
            file.bytes.len(),
            md5_of(&file.bytes)
        );
        let parent = session.parent.clone();
        drop(sessions);
        let _ = parent;
        shared.files.lock().expect("the files").push(file);
        return json(200, &answer);
    }

    let held = session.bytes.len() as u64;
    let carried = session.clone();
    drop(sessions);

    let fresh = moved.then(|| {
        let renamed = format!("moved-{id}");
        shared.sessions.lock().expect("the sessions").insert(renamed.clone(), carried);
        session_url(&host(&headers), &renamed)
    });
    progress(held, fresh)
}

fn progress(held: u64, moved: Option<String>) -> Response {
    let mut builder = Response::builder().status(StatusCode::PERMANENT_REDIRECT);
    if held > 0 {
        builder = builder.header(axum::http::header::RANGE, format!("bytes=0-{}", held - 1));
    }
    if let Some(moved) = moved {
        builder = builder.header(axum::http::header::LOCATION, moved);
    }
    builder.body(axum::body::Body::empty()).expect("a response")
}

fn session_url(host: &str, id: &str) -> String {
    format!("http://{host}/upload/session/{id}")
}

fn host(headers: &HeaderMap) -> String {
    headers
        .get(axum::http::header::HOST)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("127.0.0.1")
        .to_owned()
}

fn needs_token(headers: &HeaderMap) -> Option<Response> {
    let carried = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    if carried.starts_with("Bearer ") && carried.len() > "Bearer ".len() {
        return None;
    }
    Some(json(401, r#"{"error":{"code":401,"message":"Invalid Credentials"}}"#))
}

fn json(status: u16, body: &str) -> Response {
    (
        StatusCode::from_u16(status).expect("a status"),
        [(axum::http::header::CONTENT_TYPE, "application/json")],
        body.to_owned(),
    )
        .into_response()
}

fn md5_of(bytes: &[u8]) -> String {
    use md5::Digest;
    let mut digest = md5::Md5::new();
    digest.update(bytes);
    hex::encode(digest.finalize())
}

pub struct DataDir(PathBuf);

impl DataDir {
    pub fn new() -> Self {
        Self(std::env::temp_dir().join(format!("craftpanel-drive-{}", Id::new())))
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for DataDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

pub fn service(pool: &SqlitePool, dir: &DataDir, google: &FakeGoogle) -> Arc<Drive> {
    Drive::against(pool.clone(), dir.path(), google.base(), google.base())
}

pub async fn with_credentials(drive: &Arc<Drive>) {
    drive
        .save(
            Some("1234.apps.googleusercontent.com".to_owned()),
            super::SecretChange::Replace("GOCSPX-test".to_owned()),
            crate::model::BackupTargetPolicy::UserChoice,
            "craftpanel-backups".to_owned(),
            crate::model::Timestamp::now(),
        )
        .await
        .expect("the operator's settings");
}
