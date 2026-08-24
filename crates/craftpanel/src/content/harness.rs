#![cfg(test)]

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use axum::extract::{Path as RoutePath, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use sqlx::SqlitePool;

use super::modrinth::{a_version, MrFile, MrHashes, MrVersion};
use crate::model::{Id, Timestamp};

pub const FILE_BODY: &[u8] = b"the bytes of a mod jar";

pub use crate::ops::testing::schema;

pub struct DataDir(PathBuf);

impl DataDir {
    pub fn new() -> Self {
        let path = std::env::temp_dir().join(format!("craftpanel-content-{}", Id::new()));
        std::fs::create_dir_all(&path).expect("a data directory");
        Self(path)
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

pub async fn a_user(pool: &SqlitePool) -> Id {
    let id = Id::new();
    sqlx::query(
        "INSERT INTO users (id, username, password_hash, role, system_uid, system_state,
                            created_at, updated_at)
         VALUES (?, ?, 'argon2', 'user', 6100, 'ready', ?, ?)",
    )
    .bind(id)
    .bind(id.to_string())
    .bind(Timestamp::now())
    .bind(Timestamp::now())
    .execute(pool)
    .await
    .expect("a panel user");
    id
}

pub async fn a_server(pool: &SqlitePool, owner: Id, loader: &str, game_version: &str) -> Id {
    let id = Id::new();
    sqlx::query(
        "INSERT INTO servers (id, name, owner_id, status, loader, game_version, memory_mib,
                              created_at, updated_at)
         VALUES (?, 'Survival', ?, 'available', ?, ?, 4096, ?, ?)",
    )
    .bind(id)
    .bind(owner)
    .bind(loader)
    .bind(game_version)
    .bind(Timestamp::now())
    .bind(Timestamp::now())
    .execute(pool)
    .await
    .expect("a server");
    id
}

#[derive(Default)]
struct Fake {
    calls: AtomicUsize,
    conditional: AtomicUsize,
    broken: AtomicBool,
    forced: Mutex<Option<(u16, usize)>>,
    epoch: AtomicUsize,
    versions: Mutex<HashMap<String, Vec<MrVersion>>>,
    projects: Mutex<HashMap<String, serde_json::Value>>,
    files: Mutex<HashMap<String, Vec<u8>>>,
}

pub struct FakeModrinth {
    base: String,
    state: Arc<Fake>,
}

impl FakeModrinth {
    pub fn base(&self) -> &str {
        &self.base
    }

    pub fn calls(&self) -> usize {
        self.state.calls.load(Ordering::Relaxed)
    }

    pub fn conditional(&self) -> usize {
        self.state.conditional.load(Ordering::Relaxed)
    }

    pub fn break_down(&self) {
        self.state.broken.store(true, Ordering::Relaxed);
    }

    pub fn answer_with(&self, status: u16, times: usize) {
        *self.state.forced.lock().unwrap() = Some((status, times));
    }

    pub fn set_versions(&self, project: &str, versions: Vec<MrVersion>) {
        self.state.versions.lock().unwrap().insert(project.to_owned(), versions);
        self.state.epoch.fetch_add(1, Ordering::Relaxed);
    }

    pub fn set_project(&self, project: &str, value: serde_json::Value) {
        self.state.projects.lock().unwrap().insert(project.to_owned(), value);
    }

    pub fn add_file(&self, name: &str, body: Vec<u8>) -> String {
        self.state.files.lock().unwrap().insert(name.to_owned(), body);
        format!("{}/download/{name}", self.base)
    }
}

pub fn a_release(id: &str, project: &str, published: &str, url: &str, body: &[u8]) -> MrVersion {
    use sha2::Digest;
    let mut version = a_version(id, project, "release", published);
    version.files = vec![MrFile {
        hashes: MrHashes {
            sha1: None,
            sha512: Some(hex::encode(sha2::Sha512::digest(body))),
        },
        url: url.to_owned(),
        filename: url.rsplit('/').next().unwrap_or("file.jar").to_owned(),
        primary: true,
        size: body.len() as u64,
    }];
    version
}

pub fn client(pool: &SqlitePool, upstream: &FakeModrinth) -> super::modrinth::Modrinth {
    super::modrinth::Modrinth::with_base(pool.clone(), upstream.base())
        .expect("a client")
        .with_backoff(std::time::Duration::from_millis(1))
}

pub async fn fake_modrinth() -> FakeModrinth {
    let state = Arc::new(Fake::default());
    state.versions.lock().unwrap().insert(
        "P1".to_owned(),
        vec![
            a_version("v-new", "P1", "release", "2026-06-01T00:00:00Z"),
            a_version("v-old", "P1", "release", "2026-01-01T00:00:00Z"),
        ],
    );

    let app = Router::new()
        .route("/v2/project/{project}/version", get(versions))
        .route("/v2/project/{project}", get(project))
        .route("/v2/projects", get(projects))
        .route("/v2/version/{version}", get(version))
        .route("/v2/team/{team}/members", get(members))
        .route("/v2/search", get(search))
        .route("/v2/tag/game_version", get(game_versions))
        .route("/file", get(file))
        .route("/download/{name}", get(download))
        .with_state(Arc::clone(&state));

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("a free port");
    let base = format!("http://{}", listener.local_addr().expect("an address"));
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    FakeModrinth { base, state }
}

fn gate(state: &Fake, headers: &HeaderMap) -> Option<Response> {
    state.calls.fetch_add(1, Ordering::Relaxed);
    if headers.contains_key(axum::http::header::IF_NONE_MATCH) {
        state.conditional.fetch_add(1, Ordering::Relaxed);
    }
    if state.broken.load(Ordering::Relaxed) {
        return Some((StatusCode::BAD_GATEWAY, "gone").into_response());
    }

    let mut forced = state.forced.lock().unwrap();
    if let Some((status, left)) = forced.as_mut() {
        if *left > 0 {
            *left -= 1;
            let status = StatusCode::from_u16(*status).unwrap_or(StatusCode::BAD_GATEWAY);
            return Some((status, "no").into_response());
        }
        *forced = None;
    }
    None
}

async fn versions(
    State(state): State<Arc<Fake>>,
    RoutePath(project): RoutePath<String>,
    headers: HeaderMap,
) -> Response {
    if let Some(early) = gate(&state, &headers) {
        return early;
    }
    let etag = format!("\"v{}\"", state.epoch.load(Ordering::Relaxed));
    if headers
        .get(axum::http::header::IF_NONE_MATCH)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value == etag)
    {
        return StatusCode::NOT_MODIFIED.into_response();
    }

    let list = state.versions.lock().unwrap().get(&project).cloned().unwrap_or_default();
    (StatusCode::OK, [(axum::http::header::ETAG, etag)], axum::Json(list)).into_response()
}

async fn project(
    State(state): State<Arc<Fake>>,
    RoutePath(project): RoutePath<String>,
    headers: HeaderMap,
) -> Response {
    if let Some(early) = gate(&state, &headers) {
        return early;
    }
    axum::Json(a_project(&state, &project)).into_response()
}

async fn projects(
    State(state): State<Arc<Fake>>,
    Query(query): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Response {
    if let Some(early) = gate(&state, &headers) {
        return early;
    }
    let asked: Vec<String> = query
        .get("ids")
        .and_then(|ids| serde_json::from_str(ids).ok())
        .unwrap_or_default();
    let found: Vec<serde_json::Value> =
        asked.iter().filter(|id| *id != UNKNOWN).map(|id| a_project(&state, id)).collect();
    axum::Json(found).into_response()
}

pub const UNKNOWN: &str = "GONE";

fn a_project(state: &Fake, project: &str) -> serde_json::Value {
    let known = state.projects.lock().unwrap().get(project).cloned();
    known.unwrap_or_else(|| {
        serde_json::json!({
            "id": project,
            "slug": format!("{project}-slug"),
            "title": format!("Project {project}"),
            "description": "a project",
            "icon_url": null,
            "project_type": "mod",
            "downloads": 12,
            "followers": 3,
            "team": format!("team-{project}"),
            "categories": ["utility"],
            "client_side": "optional",
            "server_side": "required"
        })
    })
}

async fn version(
    State(state): State<Arc<Fake>>,
    RoutePath(wanted): RoutePath<String>,
    headers: HeaderMap,
) -> Response {
    if let Some(early) = gate(&state, &headers) {
        return early;
    }
    let found = state
        .versions
        .lock()
        .unwrap()
        .values()
        .flatten()
        .find(|version| version.id == wanted)
        .cloned();
    match found {
        Some(version) => axum::Json(version).into_response(),
        None => (StatusCode::NOT_FOUND, "no such version").into_response(),
    }
}

async fn members(
    State(state): State<Arc<Fake>>,
    RoutePath(team): RoutePath<String>,
    headers: HeaderMap,
) -> Response {
    if let Some(early) = gate(&state, &headers) {
        return early;
    }
    axum::Json(serde_json::json!([{
        "team_id": team,
        "role": "Owner",
        "is_owner": true,
        "user": { "id": "U1", "username": "somebody", "avatar_url": null }
    }]))
    .into_response()
}

async fn game_versions(State(state): State<Arc<Fake>>, headers: HeaderMap) -> Response {
    if let Some(early) = gate(&state, &headers) {
        return early;
    }
    let list: Vec<serde_json::Value> = ["1.21.1", "1.20.1", "1.19.2"]
        .iter()
        .map(|version| serde_json::json!({ "version": version, "version_type": "release" }))
        .collect();
    axum::Json(list).into_response()
}

async fn search(State(state): State<Arc<Fake>>, headers: HeaderMap) -> Response {
    if let Some(early) = gate(&state, &headers) {
        return early;
    }
    axum::Json(serde_json::json!({ "hits": [], "total_hits": 0 })).into_response()
}

async fn file(State(state): State<Arc<Fake>>, headers: HeaderMap) -> Response {
    if let Some(early) = gate(&state, &headers) {
        return early;
    }
    FILE_BODY.into_response()
}

async fn download(
    State(state): State<Arc<Fake>>,
    RoutePath(name): RoutePath<String>,
    headers: HeaderMap,
) -> Response {
    if let Some(early) = gate(&state, &headers) {
        return early;
    }
    match state.files.lock().unwrap().get(&name) {
        Some(body) => body.clone().into_response(),
        None => (StatusCode::NOT_FOUND, "no such file").into_response(),
    }
}
