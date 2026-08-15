#![cfg(test)]

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::http::header::{CONTENT_TYPE, COOKIE};
use axum::http::{Request, Response};
use craftpanel_proto::{HelperErrorCode, HelperOk, HelperRequest, HelperResponse};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::SqlitePool;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixListener;

use crate::config::Config;
use crate::model::{CpuMode, Id, PanelRole, Timestamp, UserLimits};
use crate::AppState;

pub async fn test_pool() -> SqlitePool {
    let options = SqliteConnectOptions::new().in_memory(true).foreign_keys(true);
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .min_connections(1)
        .idle_timeout(None)
        .max_lifetime(None)
        .connect_with(options)
        .await
        .expect("opening the test database");

    sqlx::migrate!("./migrations").run(&pool).await.expect("running the migrations");
    pool
}

pub const PASSWORD: &str = "korrekthorsebatterystaple";

pub async fn a_user(pool: &SqlitePool, username: &str) -> Id {
    insert_user(pool, username, PanelRole::User, PASSWORD).await
}

pub async fn an_admin(pool: &SqlitePool, username: &str) -> Id {
    insert_user(pool, username, PanelRole::Admin, PASSWORD).await
}

pub async fn insert_user(
    pool: &SqlitePool,
    username: &str,
    role: PanelRole,
    password: &str,
) -> Id {
    let id = Id::new();
    let now = Timestamp::now();
    sqlx::query(
        "INSERT INTO users (id, username, password_hash, role, system_uid, system_state, \
         memory_mib, cpu_mode, cpu_cores, pids_max, disk_mib, created_at, updated_at) \
         VALUES (?, ?, ?, ?, 6100, 'ready', 4096, 'cap', 2.0, 512, 51200, ?, ?)",
    )
    .bind(id)
    .bind(username)
    .bind(super::password::hash(password).expect("the test password is long enough"))
    .bind(role)
    .bind(now)
    .bind(now)
    .execute(pool)
    .await
    .expect("inserting a user");
    id
}

pub async fn a_server(pool: &SqlitePool, owner: Id, name: &str, memory_mib: u32) -> Id {
    let id = Id::new();
    let now = Timestamp::now();
    sqlx::query(
        "INSERT INTO servers (id, name, owner_id, status, memory_mib, created_at, updated_at) \
         VALUES (?, ?, ?, 'available', ?, ?, ?)",
    )
    .bind(id)
    .bind(name)
    .bind(owner)
    .bind(memory_mib)
    .bind(now)
    .bind(now)
    .execute(pool)
    .await
    .expect("inserting a server");
    id
}

pub fn some_limits() -> UserLimits {
    UserLimits {
        memory_mib: 8192,
        cpu_mode: CpuMode::Cap,
        cpu_cores: 4.0,
        pids_max: 512,
        disk_mib: 51200,
    }
}

pub async fn sign_in(pool: &SqlitePool, user: Id) -> String {
    super::session::open(pool, user, None, Timestamp::now()).await.expect("opening a session").1
}

pub fn state(pool: &SqlitePool) -> AppState {
    state_with(pool, Config::default())
}

pub fn state_with(pool: &SqlitePool, config: Config) -> AppState {
    AppState { config: Arc::new(config), pool: pool.clone() }
}

pub fn fetch(uri: &str) -> Request<Body> {
    Request::builder().uri(uri).body(Body::empty()).expect("building a request")
}

pub fn send(method: &str, uri: &str, body: serde_json::Value) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header(CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .expect("building a request")
}

pub fn empty(method: &str, uri: &str) -> Request<Body> {
    Request::builder().method(method).uri(uri).body(Body::empty()).expect("building a request")
}

pub fn as_user(mut request: Request<Body>, secret: &str) -> Request<Body> {
    request
        .headers_mut()
        .insert(COOKIE, format!("craft_session={secret}").parse().expect("a cookie header"));
    request
}

pub async fn body_json(response: Response<Body>) -> serde_json::Value {
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.expect("a body");
    if bytes.is_empty() {
        return serde_json::Value::Null;
    }
    serde_json::from_slice(&bytes).expect("a JSON body")
}

pub fn set_cookie(response: &Response<Body>) -> Option<String> {
    response
        .headers()
        .get(axum::http::header::SET_COOKIE)
        .map(|value| value.to_str().expect("a printable cookie").to_owned())
}

pub struct FakeHelper {
    socket: PathBuf,
    calls: Arc<Mutex<Vec<HelperRequest>>>,
    users: Arc<Mutex<Option<PathBuf>>>,
}

impl FakeHelper {
    pub async fn obliging() -> Self {
        Self::start(true).await
    }

    pub async fn refusing() -> Self {
        Self::start(false).await
    }

    pub fn rooted_at(self, users: impl Into<PathBuf>) -> Self {
        *self.users.lock().unwrap() = Some(users.into());
        self
    }

    async fn start(agreeable: bool) -> Self {
        let socket = std::env::temp_dir().join(format!("craftpanel-helper-{}.sock", Id::new()));
        let listener = UnixListener::bind(&socket).expect("binding the fake helper socket");
        let calls = Arc::new(Mutex::new(Vec::new()));
        let users: Arc<Mutex<Option<PathBuf>>> = Arc::new(Mutex::new(None));

        let seen = Arc::clone(&calls);
        let root = Arc::clone(&users);
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let seen = Arc::clone(&seen);
                let root = Arc::clone(&root);
                tokio::spawn(async move {
                    let (reader, mut writer) = stream.into_split();
                    let mut lines = BufReader::new(reader).lines();
                    while let Ok(Some(line)) = lines.next_line().await {
                        let Ok(request) = serde_json::from_str::<HelperRequest>(&line) else {
                            continue;
                        };
                        let here = root.lock().unwrap().clone();
                        let answer = answer(&request, agreeable, here.as_deref());
                        seen.lock().unwrap().push(request);
                        let mut encoded = serde_json::to_vec(&answer).unwrap();
                        encoded.push(b'\n');
                        let _ = writer.write_all(&encoded).await;
                        let _ = writer.flush().await;
                    }
                });
            }
        });

        Self { socket, calls, users }
    }

    pub fn socket(&self) -> PathBuf {
        self.socket.clone()
    }

    pub fn calls(&self) -> Vec<HelperRequest> {
        self.calls.lock().unwrap().clone()
    }
}

impl Drop for FakeHelper {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.socket);
    }
}

fn answer(request: &HelperRequest, agreeable: bool, users: Option<&Path>) -> HelperResponse {
    if !agreeable {
        return HelperResponse::Error {
            code: HelperErrorCode::Internal,
            message: "useradd: UID range exhausted".to_owned(),
        };
    }
    HelperResponse::Ok(match request {
        HelperRequest::Ping => HelperOk::Pong { version: craftpanel_proto::HELPER_PROTOCOL_VERSION },
        HelperRequest::CreateUser { user_id } => HelperOk::UserCreated {
            uid: 6100,
            gid: 6100,
            home: PathBuf::from(format!("/var/lib/craftpanel/users/{user_id}")),
        },
        HelperRequest::DeleteUser { .. } => HelperOk::UserDeleted,
        HelperRequest::ApplyLimits { .. } => HelperOk::LimitsApplied,
        HelperRequest::ChownTree { user_id, steps } => {
            HelperOk::TreeChowned { entries: hand_back(users, user_id, steps) }
        }
        HelperRequest::Spawn(_) => HelperOk::Spawned { pid: 4242 },
    })
}

fn hand_back(users: Option<&Path>, user_id: &str, steps: &[String]) -> u64 {
    use std::os::unix::fs::PermissionsExt;

    let Some(users) = users else { return 0 };
    let mut root = users.join(user_id);
    for step in steps {
        root.push(step);
    }
    let root = root.as_path();

    let mut touched = 0;
    let mut stack = vec![root.to_path_buf()];
    while let Some(path) = stack.pop() {
        let Ok(meta) = std::fs::symlink_metadata(&path) else { continue };
        if meta.file_type().is_symlink() {
            continue;
        }
        let mode = if meta.is_dir() { 0o2770 } else { 0o0660 };
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode));
        touched += 1;
        if meta.is_dir() {
            if let Ok(entries) = std::fs::read_dir(&path) {
                stack.extend(entries.flatten().map(|entry| entry.path()));
            }
        }
    }
    touched
}
