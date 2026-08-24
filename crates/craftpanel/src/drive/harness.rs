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
    taken: usize,
    total: u64,
    name: String,
    parent: Option<String>,
    backup_id: Option<String>,
    server_id: Option<String>,
    made: Option<String>,
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
    hold_chunk: Option<(usize, std::time::Duration)>,
    garble: bool,
    forget_after: Option<usize>,
    forget_once: bool,
    finish_after: Option<usize>,
    deaf: bool,
    overstate: u64,
    halve_downloads: usize,
    cut_downloads_at: std::collections::VecDeque<usize>,
    ignore_range: bool,
    from_the_front: bool,
    hold_download: Option<std::time::Duration>,
    disowned: bool,
    abusive: bool,
    hide_upload_md5: bool,
    hide_file_md5: bool,
    only_sha256: bool,
    hide_size: bool,
    each_chunk: Option<(usize, u16)>,
    turned_per_chunk: HashMap<String, usize>,
    turn_away: HashMap<String, TurnedAway>,
    token_life: Option<i64>,
    once_per_session: Option<(u16, String)>,
    refused_sessions: std::collections::HashSet<String>,
    room: Option<u64>,
    limitless: bool,
}

#[derive(Debug, Clone)]
struct TurnedAway {
    left: usize,
    status: u16,
    after: Option<String>,
    body: Option<String>,
}

#[derive(Clone)]
struct Shared {
    calls: Arc<Mutex<Vec<String>>>,
    sessions: Arc<Mutex<HashMap<String, Session>>>,
    files: Arc<Mutex<Vec<StoredFile>>>,
    script: Arc<Mutex<Script>>,
    chunks: Arc<Mutex<usize>>,
    polls: Arc<Mutex<usize>>,
    offered: Arc<Mutex<u64>>,
    tokens: Arc<Mutex<usize>>,
    asked_for: Arc<Mutex<Vec<Option<String>>>>,
    owned_up: Arc<Mutex<usize>>,
    handed: Arc<Mutex<u64>>,
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
            offered: Arc::new(Mutex::new(0)),
            tokens: Arc::new(Mutex::new(0)),
            asked_for: Arc::new(Mutex::new(Vec::new())),
            owned_up: Arc::new(Mutex::new(0)),
            handed: Arc::new(Mutex::new(0)),
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

    pub fn bytes_offered(&self) -> u64 {
        *self.shared.offered.lock().expect("the byte count")
    }

    pub fn times_called(&self, what: &str) -> usize {
        self.calls().iter().filter(|call| *call == what).count()
    }

    pub fn forget_the_session_after(&self, chunk: usize) {
        let mut script = self.shared.script.lock().expect("the script");
        script.forget_after = Some(chunk);
        script.forget_once = false;
    }

    pub fn forget_the_first_session_after(&self, chunk: usize) {
        let mut script = self.shared.script.lock().expect("the script");
        script.forget_after = Some(chunk);
        script.forget_once = true;
    }

    pub fn call_it_finished_after(&self, chunk: usize) {
        self.shared.script.lock().expect("the script").finish_after = Some(chunk);
    }

    pub fn turn_away_every_chunk(&self, times: usize, status: u16) {
        self.shared.script.lock().expect("the script").each_chunk = Some((times, status));
    }

    pub fn let_the_token_die_in(&self, seconds: i64) {
        self.shared.script.lock().expect("the script").token_life = Some(seconds);
    }

    pub fn turn_the_first_chunk_of_each_session_away(&self, status: u16, body: &str) {
        self.shared.script.lock().expect("the script").once_per_session =
            Some((status, body.to_owned()));
    }

    pub fn leave_room_for(&self, bytes: u64) {
        self.shared.script.lock().expect("the script").room = Some(bytes);
    }

    pub fn name_no_storage_limit(&self) {
        self.shared.script.lock().expect("the script").limitless = true;
    }

    pub fn name_no_size_either(&self) {
        self.shared.script.lock().expect("the script").hide_size = true;
    }

    pub fn cut_every_download_in_half(&self) {
        self.shared.script.lock().expect("the script").halve_downloads = usize::MAX;
    }

    pub fn cut_the_next_download_in_half(&self) {
        self.shared.script.lock().expect("the script").halve_downloads = 1;
    }

    pub fn cut_the_downloads_at(&self, percents: &[usize]) {
        self.shared.script.lock().expect("the script").cut_downloads_at =
            percents.iter().copied().collect();
    }

    pub fn hold_the_download(&self, how_long: std::time::Duration) {
        self.shared.script.lock().expect("the script").hold_download = Some(how_long);
    }

    pub fn ignore_the_range(&self) {
        self.shared.script.lock().expect("the script").ignore_range = true;
    }

    pub fn answer_from_the_front(&self) {
        self.shared.script.lock().expect("the script").from_the_front = true;
    }

    pub fn say_the_file_is_not_ours(&self) {
        self.shared.script.lock().expect("the script").disowned = true;
    }

    pub fn bytes_handed_out(&self) -> u64 {
        *self.shared.handed.lock().expect("the byte count")
    }

    pub fn swap_the_file(&self, id: &str, bytes: &[u8]) {
        for file in self.shared.files.lock().expect("the files").iter_mut() {
            if file.id == id {
                file.bytes = bytes.to_vec();
            }
        }
    }

    pub fn call_the_file_abusive(&self) {
        self.shared.script.lock().expect("the script").abusive = true;
    }

    pub fn acknowledgements(&self) -> usize {
        *self.shared.owned_up.lock().expect("the acknowledgements")
    }

    pub fn ranges_asked_for(&self) -> Vec<Option<String>> {
        self.shared.asked_for.lock().expect("the ranges").clone()
    }

    pub fn acknowledge_nothing_ever(&self) {
        self.shared.script.lock().expect("the script").deaf = true;
    }

    pub fn claim_more_than_arrived(&self, extra: u64) {
        self.shared.script.lock().expect("the script").overstate = extra;
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

    pub fn turn_away(&self, what: &str, times: usize, status: u16, after: Option<&str>) {
        self.shared.script.lock().expect("the script").turn_away.insert(
            what.to_owned(),
            TurnedAway { left: times, status, after: after.map(str::to_owned), body: None },
        );
    }

    pub fn turn_away_with_body(&self, what: &str, times: usize, status: u16, body: &str) {
        self.shared.script.lock().expect("the script").turn_away.insert(
            what.to_owned(),
            TurnedAway {
                left: times,
                status,
                after: None,
                body: Some(body.to_owned()),
            },
        );
    }

    pub fn hold_the_first_chunk(&self, how_long: std::time::Duration) {
        self.shared.script.lock().expect("the script").hold_first_chunk = Some(how_long);
    }

    pub fn hold_the_chunk(&self, number: usize, how_long: std::time::Duration) {
        self.shared.script.lock().expect("the script").hold_chunk = Some((number, how_long));
    }

    pub fn take_the_rest_quietly(&self, whole: &[u8]) {
        let mut sessions = self.shared.sessions.lock().expect("the sessions");
        let mut files = self.shared.files.lock().expect("the files");
        for session in sessions.values_mut() {
            if session.made.is_some() {
                continue;
            }
            let id = format!("file-{}", Id::new());
            files.push(StoredFile {
                id: id.clone(),
                name: session.name.clone(),
                bytes: whole.to_vec(),
                trashed: false,
                panel: Some(super::PANEL_TAG.to_owned()),
                server_id: session.server_id.clone(),
                backup_id: session.backup_id.clone(),
                folder: false,
            });
            session.bytes = whole.to_vec();
            session.made = Some(id);
        }
    }

    pub fn forget_every_session(&self) {
        self.shared.sessions.lock().expect("the sessions").clear();
    }

    pub fn sessions_open(&self) -> usize {
        self.shared.sessions.lock().expect("the sessions").len()
    }

    pub fn garble_what_arrives(&self) {
        self.shared.script.lock().expect("the script").garble = true;
    }

    pub fn finish_without_a_checksum(&self) {
        self.shared.script.lock().expect("the script").hide_upload_md5 = true;
    }

    pub fn name_only_a_sha256(&self) {
        self.shared.script.lock().expect("the script").only_sha256 = true;
    }

    pub fn name_no_checksum_at_all(&self) {
        let mut script = self.shared.script.lock().expect("the script");
        script.hide_upload_md5 = true;
        script.hide_file_md5 = true;
    }
}

fn note(shared: &Shared, what: &str) {
    shared.calls.lock().expect("the call log").push(what.to_owned());
}

fn turned_away(shared: &Shared, what: &str) -> Option<Response> {
    let told = {
        let mut script = shared.script.lock().expect("the script");
        let turn = script.turn_away.get_mut(what)?;
        if turn.left == 0 {
            return None;
        }
        turn.left -= 1;
        turn.clone()
    };

    let spoken = if told.status >= 500 {
        r#"{"error":{"code":500,"errors":[{"reason":"backendError","domain":"global",
            "message":"Backend Error"}],"message":"Backend Error"}}"#
    } else {
        r#"{"error":{"code":429,"errors":[{"reason":"rateLimitExceeded",
            "domain":"usageLimits","message":"Rate Limit Exceeded"}],
            "message":"Rate Limit Exceeded"}}"#
    };
    let mut refusal = json(told.status, told.body.as_deref().unwrap_or(spoken));
    if let Some(after) = told.after {
        refusal.headers_mut().insert(
            axum::http::header::RETRY_AFTER,
            after.parse().expect("a Retry-After header"),
        );
    }
    Some(refusal)
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
    let what = if refreshing { "token/refresh" } else { "token/device" };
    note(&shared, what);
    if refreshing {
        *shared.tokens.lock().expect("the token count") += 1;
    }
    if let Some(refusal) = turned_away(&shared, what) {
        return refusal;
    }

    let script = shared.script.lock().expect("the script");
    if refreshing {
        if script.revoked {
            return json(400, r#"{"error":"invalid_grant","error_description":"Token has been expired or revoked."}"#);
        }
        let life = script.token_life.unwrap_or(3599);
        let minted = shared.tokens.lock().expect("the token count");
        return json(
            200,
            &format!(
                r#"{{"access_token":"ya29.fresh-{}","expires_in":{life},"token_type":"Bearer"}}"#,
                *minted
            ),
        );
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
    if let Some(refusal) = turned_away(&shared, "about") {
        return refusal;
    }

    let (room, limitless) = {
        let script = shared.script.lock().expect("the script");
        (script.room, script.limitless)
    };
    if limitless {
        return json(
            200,
            r#"{"user":{"displayName":"Anna Example","emailAddress":"anna@example.com"},
                "storageQuota":{"usage":"2147483648"}}"#,
        );
    }
    let Some(room) = room else {
        return json(200, include_str!("testdata/about.json"));
    };
    let usage = 2_147_483_648u64;
    json(
        200,
        &format!(
            r#"{{"user":{{"displayName":"Anna Example","emailAddress":"anna@example.com"}},
                "storageQuota":{{"limit":"{}","usage":"{usage}"}}}}"#,
            usage + room
        ),
    )
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
    let what = if media { "files/download" } else { "files/get" };
    note(&shared, what);
    if let Some(refusal) = turned_away(&shared, what) {
        return refusal;
    }

    let (silent, abusive, only_sha256, sizeless, disowned) = {
        let script = shared.script.lock().expect("the script");
        (
            script.hide_file_md5,
            script.abusive,
            script.only_sha256,
            script.hide_size,
            script.disowned,
        )
    };
    let hold = shared.script.lock().expect("the script").hold_download;
    if media {
        if let Some(hold) = hold {
            tokio::time::sleep(hold).await;
        }
    }

    let files = shared.files.lock().expect("the files");
    let Some(file) = files.iter().find(|file| file.id == id) else {
        return json(404, r#"{"error":{"code":404,"message":"File not found."}}"#);
    };
    if media {
        let asked = headers
            .get(axum::http::header::RANGE)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        shared.asked_for.lock().expect("the ranges").push(asked.clone());

        let owning_up = query.get("acknowledgeAbuse").map(String::as_str) == Some("true");
        if owning_up {
            *shared.owned_up.lock().expect("the acknowledgements") += 1;
        }
        if abusive && !owning_up {
            return json(403, include_str!("testdata/cannot_download_abusive_file.json"));
        }

        let (halve, cut, ignore_range, from_the_front) = {
            let mut script = shared.script.lock().expect("the script");
            let halve = script.halve_downloads > 0;
            script.halve_downloads = script.halve_downloads.saturating_sub(1);
            let cut = script.cut_downloads_at.pop_front();
            (halve, cut, script.ignore_range, script.from_the_front)
        };
        let whole = file.bytes.len();
        let asked_at = asked.as_deref().and_then(asked_from).unwrap_or(0);
        if asked_at > 0 && asked_at >= whole {
            return json(416, r#"{"error":{"code":416,"message":"Range Not Satisfiable"}}"#);
        }
        let from = if from_the_front || ignore_range { 0 } else { asked_at };
        let mut bytes = file.bytes[from..].to_vec();
        if halve {
            bytes.truncate(bytes.len() / 2);
        }
        if let Some(cut) = cut {
            bytes.truncate((whole * cut / 100).saturating_sub(from));
        }
        *shared.handed.lock().expect("the byte count") += bytes.len() as u64;
        if asked.is_none() || ignore_range {
            return (StatusCode::OK, bytes).into_response();
        }
        let last = from + bytes.len().max(1) - 1;
        return (
            StatusCode::PARTIAL_CONTENT,
            [(axum::http::header::CONTENT_RANGE, format!("bytes {from}-{last}/{whole}"))],
            bytes,
        )
            .into_response();
    }
    json(200, &seen(file, silent, only_sha256, sizeless, disowned))
}

fn asked_from(range: &str) -> Option<usize> {
    let (_, span) = range.split_once('=')?;
    let (first, _) = span.split_once('-')?;
    first.trim().parse().ok()
}

fn seen(
    file: &StoredFile,
    silent: bool,
    only_sha256: bool,
    sizeless: bool,
    disowned: bool,
) -> String {
    let sums = if silent {
        String::new()
    } else if only_sha256 {
        format!(r#","sha256Checksum":"{}""#, sha256_of(&file.bytes))
    } else {
        format!(
            r#","md5Checksum":"{}","sha256Checksum":"{}""#,
            md5_of(&file.bytes),
            sha256_of(&file.bytes)
        )
    };
    let size = if sizeless {
        String::new()
    } else {
        format!(r#","size":"{}""#, file.bytes.len())
    };
    format!(
        r#"{{"id":"{}","name":"{}"{size},"trashed":{},"isAppAuthorized":{}{sums}}}"#,
        file.id,
        file.name,
        file.trashed,
        !disowned,
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
    if let Some(refusal) = turned_away(&shared, "files/delete") {
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
    if let Some(refusal) = turned_away(&shared, "upload/begin") {
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
            taken: 0,
            total: declared.unwrap_or(0),
            name: metadata["name"].as_str().unwrap_or("unnamed").to_owned(),
            parent: metadata["parents"][0].as_str().map(str::to_owned),
            backup_id: properties["backup_id"].as_str().map(str::to_owned),
            server_id: properties["server_id"].as_str().map(str::to_owned),
            made: None,
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

    let (fault, short, moved, hold, full, garble, silent, forget) = {
        let script = shared.script.lock().expect("the script");
        (
            script.chunk_faults.get(&number).copied(),
            script.short_after == Some(number),
            script.move_after == Some(number),
            script
                .hold_chunk
                .filter(|(at, _)| *at == number)
                .map(|(_, how_long)| how_long)
                .or(if number == 1 { script.hold_first_chunk } else { None }),
            script.drive_full,
            script.garble,
            script.hide_upload_md5,
            script.forget_after,
        )
    };
    let (finish_now, deaf, overstate) = {
        let script = shared.script.lock().expect("the script");
        (
            script.finish_after.is_some_and(|after| number >= after),
            script.deaf,
            script.overstate,
        )
    };
    let sizeless = shared.script.lock().expect("the script").hide_size;
    let only_sha256 = shared.script.lock().expect("the script").only_sha256;
    if let Some(refusal) = turned_away(&shared, "upload/chunk") {
        return refusal;
    }
    let once = {
        let mut script = shared.script.lock().expect("the script");
        match script.once_per_session.clone() {
            Some((status, body)) if !range.starts_with("bytes */") => script
                .refused_sessions
                .insert(id.clone())
                .then_some((status, body)),
            _ => None,
        }
    };
    if let Some((status, body)) = once {
        return json(status, &body);
    }
    let each = {
        let mut script = shared.script.lock().expect("the script");
        match script.each_chunk {
            Some((times, status)) if !range.starts_with("bytes */") => {
                let seen = script.turned_per_chunk.entry(range.clone()).or_insert(0);
                (*seen < times).then(|| {
                    *seen += 1;
                    status
                })
            }
            _ => None,
        }
    };
    if let Some(status) = each {
        return json(
            status,
            r#"{"error":{"code":429,"errors":[{"reason":"rateLimitExceeded",
                "domain":"usageLimits","message":"Rate Limit Exceeded"}],
                "message":"Rate Limit Exceeded"}}"#,
        );
    }
    let taken = {
        let mut sessions = shared.sessions.lock().expect("the sessions");
        match sessions.get_mut(&id) {
            Some(session) if !range.starts_with("bytes */") => {
                session.taken += 1;
                session.taken
            }
            Some(session) => session.taken,
            None => 0,
        }
    };
    if forget.is_some_and(|after| taken > after) {
        shared.sessions.lock().expect("the sessions").remove(&id);
        let mut script = shared.script.lock().expect("the script");
        if script.forget_once {
            script.forget_after = None;
        }
    }
    if !range.starts_with("bytes */") {
        *shared.offered.lock().expect("the byte count") += body.len() as u64;
    }

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
        let Some(made) = session.made.clone() else {
            return progress(session.bytes.len() as u64, None);
        };
        return json(
            200,
            &finished(&made, &session.name, &session.bytes, silent, only_sha256),
        );
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

    if deaf {
        return progress(0, None);
    }

    let mut arrived = body.to_vec();
    if short && arrived.len() > 1 {
        arrived.truncate(arrived.len() / 2);
    }
    if garble {
        if let Some(first) = arrived.first_mut() {
            *first ^= 0xff;
        }
    }
    session.bytes.extend_from_slice(&arrived);

    if finish_now && (session.bytes.len() as u64) < session.total {
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
        let answer = told(&file.id, &file.name, &file.bytes, silent, sizeless, only_sha256);
        session.made = Some(file.id.clone());
        drop(sessions);
        shared.files.lock().expect("the files").push(file);
        return json(200, &answer);
    }

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
        let answer = told(&file.id, &file.name, &file.bytes, silent, sizeless, only_sha256);
        session.made = Some(file.id.clone());
        let parent = session.parent.clone();
        drop(sessions);
        let _ = parent;
        shared.files.lock().expect("the files").push(file);
        return json(200, &answer);
    }

    let held = session.bytes.len() as u64 + overstate;
    let carried = session.clone();
    drop(sessions);

    let fresh = moved.then(|| {
        let renamed = format!("moved-{id}");
        shared.sessions.lock().expect("the sessions").insert(renamed.clone(), carried);
        session_url(&host(&headers), &renamed)
    });
    progress(held, fresh)
}

fn finished(id: &str, name: &str, bytes: &[u8], silent: bool, only_sha256: bool) -> String {
    told(id, name, bytes, silent, false, only_sha256)
}

fn told(
    id: &str,
    name: &str,
    bytes: &[u8],
    silent: bool,
    sizeless: bool,
    only_sha256: bool,
) -> String {
    let size = if sizeless {
        String::new()
    } else {
        format!(r#","size":"{}""#, bytes.len())
    };
    let sums = if silent {
        String::new()
    } else if only_sha256 {
        format!(r#","sha256Checksum":"{}""#, sha256_of(bytes))
    } else {
        format!(r#","md5Checksum":"{}""#, md5_of(bytes))
    };
    format!(r#"{{"id":"{id}","name":"{name}"{size}{sums}}}"#)
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

fn sha256_of(bytes: &[u8]) -> String {
    use sha2::Digest;
    let mut digest = sha2::Sha256::new();
    digest.update(bytes);
    hex::encode(digest.finalize())
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
