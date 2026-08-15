use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::extract::{FromRequestParts, State};
use axum::http::header::RETRY_AFTER;
use axum::http::request::Parts;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use serde::Deserialize;

use crate::audit::{self, Event};
use crate::auth::error::{Failure, Result};
use crate::auth::{access, extract, Caller, JsonBody, Params};
use crate::console::logs::{self, LogFileContentResponse, LogFileListResponse};
use crate::console::{check_command, echo, mclogs, Console};
use crate::files::Workspace;
use crate::model::{Id, Permission};
use crate::ops::Operations;
use crate::servers::Hub;
use crate::AppState;

const PATIENCE: Duration = Duration::from_secs(5);

pub fn router(operations: Arc<Operations>, hub: Arc<Hub>) -> Router<AppState> {
    routes(operations, hub, Arc::new(Console::new()))
}

fn routes(operations: Arc<Operations>, hub: Arc<Hub>, service: Arc<Console>) -> Router<AppState> {
    Router::new()
        .route("/servers/{server}/console/command", post(command))
        .route("/servers/{server}/console/clear", post(clear))
        .route("/servers/{server}/console/crash-analysis", post(crash_analysis))
        .route("/servers/{server}/console/logs", get(list_logs).delete(delete_log))
        .route("/servers/{server}/console/logs/content", get(read_log))
        .layer(Extension(operations))
        .layer(Extension(hub))
        .layer(Extension(service))
        .layer(axum::middleware::from_fn(extract::same_origin))
}

struct OfServer(Id);

impl FromRequestParts<AppState> for OfServer {
    type Rejection = Failure;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self> {
        let axum::extract::Path(raw) =
            axum::extract::Path::<String>::from_request_parts(parts, state)
                .await
                .map_err(|_| unknown_server())?;
        raw.parse().map(Self).map_err(|_| unknown_server())
    }
}

#[derive(Debug, Deserialize)]
struct SendCommandRequest {
    command: String,
}

#[derive(Debug, Default, Deserialize)]
struct CrashAnalysisRequest {
    #[serde(default)]
    source: CrashAnalysisSource,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
enum CrashAnalysisSource {
    #[default]
    LatestLog,
    Buffer,
}

#[derive(Debug, Deserialize)]
struct LogListQuery {
    limit: Option<usize>,
    offset: Option<usize>,
}

#[derive(Debug, Deserialize)]
struct LogFileQuery {
    file: String,
}

async fn command(
    State(state): State<AppState>,
    Extension(operations): Extension<Arc<Operations>>,
    Extension(hub): Extension<Arc<Hub>>,
    Extension(service): Extension<Arc<Console>>,
    caller: Caller,
    OfServer(server): OfServer,
    JsonBody(body): JsonBody<SendCommandRequest>,
) -> Result<Response> {
    let seat = access::require(&state.pool, &caller, server, Permission::ExecCommands).await?;

    if let Some(seconds) = service.accept(caller.id(), server, Instant::now()) {
        let mut refused = Failure::new(
            StatusCode::TOO_MANY_REQUESTS,
            "rate_limited",
            format!("that is a lot of commands at once; try again in {seconds} seconds"),
        )
        .into_response();
        refused.headers_mut().insert(RETRY_AFTER, seconds.into());
        return Ok(refused);
    }

    check_command(&body.command)?;

    let link = match hub.link(&server.to_string()).await {
        Some(link) if link.state().await.is_live() => link,
        _ => return Err(stopped()),
    };

    {
        let turn = service.turn(server);
        let _held = turn.lock().await;

        match tokio::time::timeout(PATIENCE, link.send_command(body.command.clone())).await {
            Ok(Ok(())) => {}
            Ok(Err(err)) => {
                tracing::warn!(%server, "a command found no supervisor: {err:#}");
                return Err(stopped());
            }
            Err(_) => {
                return Err(Failure::conflict(
                    "server_not_running",
                    "the server is not reading its console",
                ))
            }
        }

        operations.console(server, &[echo(&body.command)]).await.map_err(fault)?;
    }

    audit::record(
        &state.pool,
        seat,
        &caller,
        Event::ConsoleCommandExecuted { command: body.command },
    )
    .await;
    Ok(StatusCode::NO_CONTENT.into_response())
}

async fn clear(
    State(state): State<AppState>,
    Extension(operations): Extension<Arc<Operations>>,
    caller: Caller,
    OfServer(server): OfServer,
) -> Result<StatusCode> {
    let seat = access::require(&state.pool, &caller, server, Permission::ExecCommands).await?;
    operations.channel(server).await.map_err(fault)?.clear_console();
    audit::record(&state.pool, seat, &caller, Event::ConsoleCleared).await;
    Ok(StatusCode::NO_CONTENT)
}

async fn crash_analysis(
    State(state): State<AppState>,
    Extension(operations): Extension<Arc<Operations>>,
    Extension(service): Extension<Arc<Console>>,
    caller: Caller,
    OfServer(server): OfServer,
    JsonBody(body): JsonBody<CrashAnalysisRequest>,
) -> Result<Json<mclogs::CrashAnalysis>> {
    let seat = access::require(&state.pool, &caller, server, Permission::BaseRead).await?;

    if !crate::auth::settings::load(&state.pool).await?.external_services_enabled {
        return Err(Failure::conflict(
            "external_services_disabled",
            "this panel does not call out to other services",
        ));
    }

    let (key, text) = match body.source {
        CrashAnalysisSource::LatestLog => {
            let workspace = workspace(&state, seat.owner_id, server, "log_file_missing")?;
            let (text, modified, size) =
                tokio::task::spawn_blocking(move || logs::latest_for_analysis(workspace.root()))
                    .await
                    .map_err(joined)??;
            (mclogs::Key::Log { server, modified, size }, text)
        }
        CrashAnalysisSource::Buffer => {
            let history = operations.channel(server).await.map_err(fault)?.attach().history;
            if history.lines.is_empty() {
                return Err(Failure::conflict(
                    "console_buffer_empty",
                    "there is nothing in the console to look at",
                ));
            }
            let mut text = history.lines.join("\n");
            text.push('\n');
            let key = mclogs::Key::Buffer {
                server,
                seq: history.first_seq + history.lines.len() as u64,
                lines: history.lines.len(),
            };
            (key, text)
        }
    };

    let analysed =
        service.analyst.analyse(key, logs::last_bytes(&text, logs::ANALYSIS_BYTES)).await?;
    Ok(Json(analysed))
}

async fn list_logs(
    State(state): State<AppState>,
    caller: Caller,
    OfServer(server): OfServer,
    Params(query): Params<LogListQuery>,
) -> Result<Json<LogFileListResponse>> {
    let seat = access::require(&state.pool, &caller, server, Permission::BaseRead).await?;
    let limit = query.limit.unwrap_or(logs::DEFAULT_LIMIT).clamp(1, logs::MAX_LIMIT);
    let offset = query.offset.unwrap_or(0);

    let workspace = match Workspace::open(&state.config, seat.owner_id, server) {
        Ok(workspace) => workspace,
        Err(err) if err.code() == "not_found" => {
            return Ok(Json(LogFileListResponse { total: 0, truncated: false, files: Vec::new() }))
        }
        Err(err) => return Err(err),
    };

    let listed = tokio::task::spawn_blocking(move || logs::list(workspace.root(), limit, offset))
        .await
        .map_err(joined)?;
    Ok(Json(listed))
}

async fn read_log(
    State(state): State<AppState>,
    caller: Caller,
    OfServer(server): OfServer,
    Params(query): Params<LogFileQuery>,
) -> Result<Json<LogFileContentResponse>> {
    let seat = access::require(&state.pool, &caller, server, Permission::BaseRead).await?;
    let at = logs::target(&query.file)?;
    let workspace = workspace(&state, seat.owner_id, server, "log_not_found")?;

    let read = tokio::task::spawn_blocking(move || logs::read(workspace.root(), &at))
        .await
        .map_err(joined)??;
    Ok(Json(read))
}

async fn delete_log(
    State(state): State<AppState>,
    Extension(operations): Extension<Arc<Operations>>,
    Extension(hub): Extension<Arc<Hub>>,
    caller: Caller,
    OfServer(server): OfServer,
    Params(query): Params<LogFileQuery>,
) -> Result<StatusCode> {
    let seat = access::require(&state.pool, &caller, server, Permission::FilesWrite).await?;
    operations.guard_write(server).await.map_err(fault)?;
    let at = logs::target(&query.file)?;

    if logs::on_the_wire(&at) == logs::LATEST && running(&hub, server).await {
        return Err(Failure::conflict(
            "log_file_in_use",
            "The current log cannot be deleted while the server is running.",
        ));
    }

    let workspace = workspace(&state, seat.owner_id, server, "log_not_found")?;
    let path = at.on_the_wire();
    tokio::task::spawn_blocking(move || logs::remove(workspace.root(), &at))
        .await
        .map_err(joined)??;

    audit::record(&state.pool, seat, &caller, Event::FileDeleted { path }).await;
    Ok(StatusCode::NO_CONTENT)
}

fn workspace(
    state: &AppState,
    owner: Id,
    server: Id,
    missing: &'static str,
) -> Result<Workspace> {
    Workspace::open(&state.config, owner, server).map_err(|err| match err.code() {
        "not_found" => Failure::not_found(missing, "this server has written nothing yet"),
        _ => err,
    })
}

async fn running(hub: &Hub, server: Id) -> bool {
    match hub.link(&server.to_string()).await {
        Some(link) => link.state().await.is_live(),
        None => false,
    }
}

fn stopped() -> Failure {
    Failure::conflict("server_not_running", "the server is not running")
}

fn unknown_server() -> Failure {
    Failure::not_found("server_not_found", "no such server")
}

fn fault(fault: crate::ops::Fault) -> Failure {
    Failure::new(fault.status(), fault.code(), fault.message().to_owned())
}

fn joined(err: tokio::task::JoinError) -> Failure {
    Failure::internal(anyhow::Error::new(err).context("a console task died"))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU16, AtomicUsize, Ordering};
    use std::sync::Mutex;

    use axum::body::Body;
    use axum::http::header::{CONTENT_TYPE, COOKIE};
    use axum::http::Request;
    use craftpanel_proto::{PanelMessage, SupervisorMessage, HELPER_PROTOCOL_VERSION};
    use sqlx::SqlitePool;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
    use tokio::net::UnixStream;
    use tower::ServiceExt;

    use super::*;
    use crate::auth::harness::{a_user, sign_in, state_with, test_pool};
    use crate::config::Config;
    use crate::files::testing::Sandbox;
    use crate::model::{ServerRole, Timestamp};

    const ANALYSED: &str = r#"{
        "success": true, "id": "abc", "name": null, "type": "Paper", "version": null,
        "title": "Minecraft Server",
        "analysis": {
            "problems": [{
                "message": "FAILED TO BIND TO PORT",
                "counter": 1,
                "entry": {"level": 2, "time": null, "prefix": "", "lines": []},
                "solutions": [{"message": "Free the port"}]
            }],
            "information": []
        },
        "entries": [{"level": 0, "time": null, "prefix": "", "lines": []}]
    }"#;

    #[derive(Default)]
    struct Upstream {
        calls: AtomicUsize,
        status: AtomicU16,
        bodies: Mutex<Vec<String>>,
    }

    struct FakeMclogs {
        base: String,
        state: Arc<Upstream>,
    }

    impl FakeMclogs {
        async fn start() -> Self {
            let state = Arc::new(Upstream::default());
            let app = Router::new()
                .route("/1/analyse", post(analyse))
                .with_state(Arc::clone(&state));
            let listener =
                tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("a free port");
            let base = format!("http://{}", listener.local_addr().expect("an address"));
            tokio::spawn(async move {
                let _ = axum::serve(listener, app).await;
            });
            Self { base, state }
        }

        fn calls(&self) -> usize {
            self.state.calls.load(Ordering::Relaxed)
        }

        fn answer_with(&self, status: u16) {
            self.state.status.store(status, Ordering::Relaxed);
        }

        fn last_body(&self) -> String {
            self.state.bodies.lock().unwrap().last().cloned().unwrap_or_default()
        }
    }

    async fn analyse(State(state): State<Arc<Upstream>>, body: String) -> Response {
        state.calls.fetch_add(1, Ordering::Relaxed);
        state.bodies.lock().unwrap().push(body);
        match state.status.load(Ordering::Relaxed) {
            0 => ([(CONTENT_TYPE, "application/json")], ANALYSED).into_response(),
            other => {
                (StatusCode::from_u16(other).expect("a status"), "no").into_response()
            }
        }
    }

    struct Panel {
        app: Router,
        pool: SqlitePool,
        sandbox: Sandbox,
        cookie: String,
        server: Id,
        hub: Arc<Hub>,
        operations: Arc<Operations>,
        upstream: FakeMclogs,
        socket: PathBuf,
    }

    async fn panel() -> Panel {
        let pool = test_pool().await;
        let sandbox = Sandbox::new();
        let now = Timestamp::now();

        sqlx::query(
            "INSERT INTO users (id, username, password_hash, role, system_uid, system_state, \
             memory_mib, cpu_mode, cpu_cores, pids_max, created_at, updated_at) \
             VALUES (?, 'max', 'argon2', 'user', 6100, 'ready', 4096, 'cap', 2.0, 512, ?, ?)",
        )
        .bind(sandbox.owner)
        .bind(now)
        .bind(now)
        .execute(&pool)
        .await
        .expect("the owner");
        sqlx::query(
            "INSERT INTO servers (id, name, owner_id, status, memory_mib, created_at, updated_at) \
             VALUES (?, 'Survival', ?, 'available', 2048, ?, ?)",
        )
        .bind(sandbox.server)
        .bind(sandbox.owner)
        .bind(now)
        .bind(now)
        .execute(&pool)
        .await
        .expect("the server");

        let socket = std::env::temp_dir().join(format!("craftpanel-console-{}.sock", Id::new()));
        let hub = Arc::new(Hub::new(socket.clone()));
        tokio::spawn({
            let hub = Arc::clone(&hub);
            async move {
                let _ = hub.listen().await;
            }
        });

        let config = sandbox.config();
        let operations = Operations::new(pool.clone(), config.data_dir.clone());
        let upstream = FakeMclogs::start().await;
        let service =
            Arc::new(Console::with_analyst(mclogs::Mclogs::with_base(upstream.base.clone())));

        let app = routes(Arc::clone(&operations), Arc::clone(&hub), service)
            .with_state(state_with(&pool, Config { ..config }));

        Panel {
            app,
            cookie: sign_in(&pool, sandbox.owner).await,
            pool,
            server: sandbox.server,
            sandbox,
            hub,
            operations,
            upstream,
            socket,
        }
    }

    impl Drop for Panel {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.socket);
        }
    }

    impl Panel {
        fn url(&self, tail: &str) -> String {
            format!("/servers/{}/console{tail}", self.server)
        }

        async fn send(&self, request: Request<Body>) -> Response {
            self.app.clone().oneshot(request).await.expect("an answer")
        }

        async fn get_as(&self, cookie: &str, tail: &str) -> Response {
            let request = Request::builder()
                .uri(self.url(tail))
                .header(COOKIE, format!("craft_session={cookie}"))
                .body(Body::empty())
                .expect("a request");
            self.send(request).await
        }

        async fn get(&self, tail: &str) -> Response {
            self.get_as(&self.cookie.clone(), tail).await
        }

        async fn post_as(&self, cookie: &str, tail: &str, body: serde_json::Value) -> Response {
            let request = Request::builder()
                .method("POST")
                .uri(self.url(tail))
                .header(CONTENT_TYPE, "application/json")
                .header(COOKIE, format!("craft_session={cookie}"))
                .body(Body::from(body.to_string()))
                .expect("a request");
            self.send(request).await
        }

        async fn post(&self, tail: &str, body: serde_json::Value) -> Response {
            self.post_as(&self.cookie.clone(), tail, body).await
        }

        async fn delete_as(&self, cookie: &str, tail: &str) -> Response {
            let request = Request::builder()
                .method("DELETE")
                .uri(self.url(tail))
                .header(COOKIE, format!("craft_session={cookie}"))
                .body(Body::empty())
                .expect("a request");
            self.send(request).await
        }

        async fn delete(&self, tail: &str) -> Response {
            self.delete_as(&self.cookie.clone(), tail).await
        }

        async fn a_member(&self, name: &str, role: ServerRole) -> String {
            let who = a_user(&self.pool, name).await;
            let now = Timestamp::now();
            sqlx::query(
                "INSERT INTO server_members (id, server_id, user_id, role, invited_at, joined_at) \
                 VALUES (?, ?, ?, ?, ?, ?)",
            )
            .bind(Id::new())
            .bind(self.server)
            .bind(who)
            .bind(role)
            .bind(now)
            .bind(now)
            .execute(&self.pool)
            .await
            .expect("a member");
            sign_in(&self.pool, who).await
        }

        async fn a_supervisor(&self) -> Supervisor {
            self.hub.set_token(self.server.to_string(), "a-token").await;

            let mut stream = None;
            for _ in 0..100 {
                match UnixStream::connect(&self.socket).await {
                    Ok(open) => {
                        stream = Some(open);
                        break;
                    }
                    Err(_) => tokio::time::sleep(Duration::from_millis(10)).await,
                }
            }
            let (reader, mut writer) = stream.expect("the hub is listening").into_split();

            let hello = serde_json::to_string(&SupervisorMessage::Hello {
                server_id: self.server.to_string(),
                token: "a-token".to_owned(),
                pid: 4242,
                protocol: HELPER_PROTOCOL_VERSION,
            })
            .expect("json");
            writer.write_all(format!("{hello}\n").as_bytes()).await.expect("greeting the hub");
            writer.flush().await.expect("greeting the hub");

            let mut lines = BufReader::new(reader);
            let mut greeting = String::new();
            lines.read_line(&mut greeting).await.expect("an answer");
            assert!(greeting.contains("accepted"), "the hub said {greeting}");

            for _ in 0..100 {
                if self.hub.link(&self.server.to_string()).await.is_some() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            Supervisor { lines, _writer: writer }
        }
    }

    struct Supervisor {
        lines: BufReader<OwnedReadHalf>,
        _writer: OwnedWriteHalf,
    }

    impl Supervisor {
        async fn next_message(&mut self) -> PanelMessage {
            let mut line = String::new();
            tokio::time::timeout(Duration::from_secs(5), self.lines.read_line(&mut line))
                .await
                .expect("the supervisor was told something")
                .expect("a line");
            serde_json::from_str(&line).expect("a panel message")
        }
    }

    async fn body_json(response: Response) -> serde_json::Value {
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.expect("a body");
        if bytes.is_empty() {
            return serde_json::Value::Null;
        }
        serde_json::from_slice(&bytes).expect("json")
    }

    #[tokio::test]
    async fn a_command_reaches_stdin_and_comes_back_as_a_console_line() {
        let panel = panel().await;
        let mut supervisor = panel.a_supervisor().await;

        let answer = panel.post("/command", serde_json::json!({ "command": "say hello" })).await;
        assert_eq!(answer.status(), StatusCode::NO_CONTENT);
        assert_eq!(body_json(answer).await, serde_json::Value::Null, "6.1 answers nothing");

        match supervisor.next_message().await {
            PanelMessage::Stdin { line } => assert_eq!(line, "say hello"),
            other => panic!("the supervisor got {other:?}"),
        }

        let history = panel.operations.channel(panel.server).await.unwrap().attach().history;
        let echoed = history.lines.last().expect("the echo is in the buffer");
        assert!(echoed.ends_with("[Panel/INFO]: > say hello"), "{echoed}");
        assert!(echoed.starts_with('['), "6.8: nothing stands in front of the clock");

        let recorded: (String, Option<String>) = sqlx::query_as(
            "SELECT action, metadata FROM audit_log WHERE server_id = ?",
        )
        .bind(panel.server)
        .fetch_one(&panel.pool)
        .await
        .expect("an audit entry");
        assert_eq!(recorded.0, "console_command_executed");
        assert!(recorded.1.expect("metadata").contains("say hello"));
    }

    #[tokio::test]
    async fn a_command_to_a_stopped_server_is_a_conflict_and_not_a_crash() {
        let panel = panel().await;
        let refused = panel.post("/command", serde_json::json!({ "command": "stop" })).await;
        assert_eq!(refused.status(), StatusCode::CONFLICT);
        assert_eq!(body_json(refused).await["error"], "server_not_running");
    }

    #[tokio::test]
    async fn a_command_is_one_line_and_the_refusals_say_which_rule_it_broke() {
        let panel = panel().await;
        let _supervisor = panel.a_supervisor().await;

        for (command, code) in [
            (serde_json::json!(""), "command_empty"),
            (serde_json::json!("   "), "command_empty"),
            (serde_json::json!("say hi\nstop"), "command_invalid"),
            (serde_json::json!("a".repeat(8193)), "command_too_long"),
        ] {
            let refused = panel.post("/command", serde_json::json!({ "command": command })).await;
            assert_eq!(refused.status(), StatusCode::UNPROCESSABLE_ENTITY, "{command}");
            assert_eq!(body_json(refused).await["error"], code);
        }

        let empty = panel.post("/command", serde_json::json!({})).await;
        assert_eq!(empty.status(), StatusCode::BAD_REQUEST, "the field is not optional");
    }

    #[tokio::test]
    async fn the_twenty_first_command_in_ten_seconds_is_braked() {
        let panel = panel().await;
        let _supervisor = panel.a_supervisor().await;

        for number in 0..20 {
            let sent = panel.post("/command", serde_json::json!({ "command": "list" })).await;
            assert_eq!(sent.status(), StatusCode::NO_CONTENT, "command {number}");
        }

        let braked = panel.post("/command", serde_json::json!({ "command": "list" })).await;
        assert_eq!(braked.status(), StatusCode::TOO_MANY_REQUESTS);
        let seconds = braked
            .headers()
            .get(RETRY_AFTER)
            .expect("1.7: the brake says how long")
            .to_str()
            .expect("a number")
            .parse::<u64>()
            .expect("a number");
        assert!((1..=10).contains(&seconds), "Retry-After was {seconds}");
        assert_eq!(body_json(braked).await["error"], "rate_limited");
    }

    #[tokio::test]
    async fn a_viewer_may_watch_the_console_and_not_type_in_it() {
        let panel = panel().await;
        panel.sandbox.write("logs/latest.log", b"[15:04:22] one\n");
        let viewer = panel.a_member("vera", ServerRole::Viewer).await;

        assert_eq!(panel.get_as(&viewer, "/logs").await.status(), StatusCode::OK);
        assert_eq!(
            panel.get_as(&viewer, "/logs/content?file=logs/latest.log").await.status(),
            StatusCode::OK
        );

        for (tail, body) in [
            ("/command", serde_json::json!({ "command": "list" })),
            ("/clear", serde_json::json!({})),
        ] {
            let refused = panel.post_as(&viewer, tail, body).await;
            assert_eq!(refused.status(), StatusCode::FORBIDDEN, "{tail}");
            assert_eq!(body_json(refused).await["error"], "forbidden");
        }

        let refused = panel.delete_as(&viewer, "/logs?file=logs/latest.log").await;
        assert_eq!(refused.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn a_stranger_is_told_the_server_does_not_exist() {
        let panel = panel().await;
        let outsider = a_user(&panel.pool, "olga").await;
        let cookie = sign_in(&panel.pool, outsider).await;

        let refused = panel.get_as(&cookie, "/logs").await;
        assert_eq!(refused.status(), StatusCode::NOT_FOUND);
        assert_eq!(body_json(refused).await["error"], "server_not_found");

        let nonsense = panel
            .app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/servers/not-a-ulid/console/logs")
                    .header(COOKIE, format!("craft_session={}", panel.cookie))
                    .body(Body::empty())
                    .expect("a request"),
            )
            .await
            .expect("an answer");
        assert_eq!(nonsense.status(), StatusCode::NOT_FOUND);
        assert_eq!(body_json(nonsense).await["error"], "server_not_found");
    }

    #[tokio::test]
    async fn clearing_empties_the_buffer_and_leaves_the_file_alone() {
        let panel = panel().await;
        let log = panel.sandbox.write("logs/latest.log", b"[15:04:22] the file stays\n");

        let channel = panel.operations.channel(panel.server).await.expect("a channel");
        channel.console_lines(&["[15:04:23] a live line".to_owned()]);
        let seq_before = channel.console_seq();
        let mut watching = channel.attach().events;

        let cleared = panel.post("/clear", serde_json::json!({})).await;
        assert_eq!(cleared.status(), StatusCode::NO_CONTENT);

        let said = match watching.try_recv().expect("everyone watching is told") {
            crate::ops::ServerEvent::Say(json) => json,
            other => panic!("the channel said {other:?}"),
        };
        assert!(said.contains("console_cleared"), "{said}");

        let after = channel.attach().history;
        assert!(after.lines.is_empty(), "the ring buffer is empty");
        assert_eq!(channel.console_seq(), seq_before, "6.2: seq keeps running");
        assert_eq!(std::fs::read(&log).unwrap(), b"[15:04:22] the file stays\n");

        let action: String =
            sqlx::query_scalar("SELECT action FROM audit_log WHERE server_id = ?")
                .bind(panel.server)
                .fetch_one(&panel.pool)
                .await
                .expect("an audit entry");
        assert_eq!(action, "console_cleared");
    }

    #[tokio::test]
    async fn the_log_list_names_files_the_way_the_provider_compares_them() {
        let panel = panel().await;
        panel.sandbox.write("logs/latest.log", b"now");
        panel.sandbox.write("logs/2026-08-11-1.log.gz", b"packed");
        panel.sandbox.write("crash-reports/crash-2026-08-11.txt", b"boom");

        let listed = body_json(panel.get("/logs").await).await;
        assert_eq!(listed["total"], 3);
        assert_eq!(listed["truncated"], false);
        assert_eq!(
            listed["files"][0]["file"], "logs/latest.log",
            "no leading slash: `console-manager.ts:52` compares against this literal"
        );
        assert_eq!(listed["files"][0]["name"], "latest.log");
        assert_eq!(listed["files"][0]["kind"], "log");
        assert_eq!(listed["files"][0]["compressed"], false);
        assert!(listed["files"][0]["modified_at"].as_str().expect("a stamp").ends_with('Z'));

        let packed = listed["files"]
            .as_array()
            .expect("a list")
            .iter()
            .find(|file| file["file"] == "logs/2026-08-11-1.log.gz")
            .expect("the rotated log");
        assert_eq!(packed["compressed"], true);
        assert_eq!(packed["size_bytes"], 6);

        let capped = body_json(panel.get("/logs?limit=1").await).await;
        assert_eq!(capped["files"].as_array().expect("a list").len(), 1);
        assert_eq!(capped["total"], 3);
        assert_eq!(capped["truncated"], true);
    }

    #[tokio::test]
    async fn a_server_without_a_directory_has_no_logs_rather_than_an_error() {
        let panel = panel().await;
        std::fs::remove_dir_all(panel.sandbox.server_dir()).expect("no directory yet");

        let listed = panel.get("/logs").await;
        assert_eq!(listed.status(), StatusCode::OK, "this is asked on every server page");
        assert_eq!(body_json(listed).await["total"], 0);
    }

    #[tokio::test]
    async fn reading_a_log_answers_its_text_and_its_two_lengths() {
        let panel = panel().await;
        panel.sandbox.write("logs/latest.log", b"[15:04:22] one\n[15:04:23] two\n");

        let read = body_json(panel.get("/logs/content?file=logs/latest.log").await).await;
        assert_eq!(read["content"], "[15:04:22] one\n[15:04:23] two\n");
        assert_eq!(read["size_bytes"], 30);
        assert_eq!(read["content_bytes"], 30);
        assert_eq!(read["truncated"], false);
        assert_eq!(read["file"], "logs/latest.log");
    }

    #[tokio::test]
    async fn a_path_out_of_the_two_directories_is_refused_by_the_endpoint() {
        let panel = panel().await;
        panel.sandbox.write("server.properties", b"secret=1");
        let secret = panel.sandbox.data_dir().join("panel.db");
        std::fs::write(&secret, b"password hashes").expect("the panel database");
        panel.sandbox.mkdir("logs");
        std::os::unix::fs::symlink(&secret, panel.sandbox.server_dir().join("logs/latest.log"))
            .expect("the link a plugin may lay");

        let elsewhere = panel.get("/logs/content?file=server.properties").await;
        assert_eq!(elsewhere.status(), StatusCode::FORBIDDEN);
        assert_eq!(body_json(elsewhere).await["error"], "forbidden_path");

        let climbing = panel.get("/logs/content?file=logs/../../panel.db").await;
        assert_eq!(climbing.status(), StatusCode::BAD_REQUEST);
        assert_eq!(body_json(climbing).await["error"], "invalid_path");

        let linked = panel.get("/logs/content?file=logs/latest.log").await;
        assert_eq!(linked.status(), StatusCode::FORBIDDEN, "this is the panel database");
        let answered = body_json(linked).await;
        assert_eq!(answered["error"], "forbidden_path");
        assert!(!answered.to_string().contains("password hashes"));
    }

    #[tokio::test]
    async fn the_running_log_cannot_be_deleted_and_the_message_reads_like_a_sentence() {
        let panel = panel().await;
        let log = panel.sandbox.write("logs/latest.log", b"[15:04:22] running\n");
        let _supervisor = panel.a_supervisor().await;

        let refused = panel.delete("/logs?file=logs/latest.log").await;
        assert_eq!(refused.status(), StatusCode::CONFLICT);
        let body = body_json(refused).await;
        assert_eq!(body["error"], "log_file_in_use");
        let sentence = "The current log cannot be deleted while the server is running.";
        assert_eq!(body["message"], sentence);
        assert!(log.exists());

        panel.sandbox.write("logs/2026-08-11-1.log.gz", b"packed");
        let older = panel.delete("/logs?file=logs/2026-08-11-1.log.gz").await;
        assert_eq!(older.status(), StatusCode::NO_CONTENT);
    }

    #[tokio::test]
    async fn a_log_cannot_be_deleted_out_from_under_a_running_backup() {
        let panel = panel().await;
        panel.sandbox.write("logs/2026-08-11-1.log", b"[15:04:22] older\n");

        let backup = Id::new();
        sqlx::query("INSERT INTO backups (id, server_id, name, created_at) VALUES (?, ?, 'B', ?)")
            .bind(backup)
            .bind(panel.server)
            .bind(Timestamp::now())
            .execute(&panel.pool)
            .await
            .expect("a backup row");
        let kind = crate::model::OperationKind::BackupCreate;
        let mut run = crate::ops::NewOperation::new(panel.server, kind, None);
        run.target_id = Some(backup);
        panel.operations.create(run).await.expect("a run");

        let refused = panel.delete("/logs?file=logs/2026-08-11-1.log").await;
        assert_eq!(refused.status(), StatusCode::CONFLICT);
        assert_eq!(body_json(refused).await["error"], "server_busy");
        assert!(panel.sandbox.server_dir().join("logs/2026-08-11-1.log").exists());

        assert_eq!(panel.get("/logs").await.status(), StatusCode::OK, "reading stays open");
        assert_eq!(
            panel.get("/logs/content?file=logs/2026-08-11-1.log").await.status(),
            StatusCode::OK
        );
    }

    #[tokio::test]
    async fn a_stopped_server_gives_its_log_up_and_the_deletion_is_written_down() {
        let panel = panel().await;
        let log = panel.sandbox.write("logs/latest.log", b"[15:04:22] stopped\n");

        let deleted = panel.delete("/logs?file=logs/latest.log").await;
        assert_eq!(deleted.status(), StatusCode::NO_CONTENT);
        assert!(!log.exists());

        let recorded: (String, Option<String>) = sqlx::query_as(
            "SELECT action, metadata FROM audit_log WHERE server_id = ?",
        )
        .bind(panel.server)
        .fetch_one(&panel.pool)
        .await
        .expect("an audit entry");
        assert_eq!(recorded.0, "file_deleted", "6.6: a name of our own would not render");
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&recorded.1.expect("metadata")).unwrap()
                ["path"],
            "/logs/latest.log"
        );

        let again = panel.delete("/logs?file=logs/latest.log").await;
        assert_eq!(again.status(), StatusCode::NOT_FOUND);
        assert_eq!(body_json(again).await["error"], "log_not_found");
    }

    #[tokio::test]
    async fn the_analysis_goes_out_trimmed_and_comes_back_trimmed() {
        let panel = panel().await;
        panel.sandbox.write("logs/latest.log", b"[15:04:22] FAILED TO BIND TO PORT\n");

        let answer = panel.post("/crash-analysis", serde_json::json!({})).await;
        assert_eq!(answer.status(), StatusCode::OK);
        let body = body_json(answer).await;
        assert_eq!(body["analysis"]["problems"][0]["solutions"][0]["message"], "Free the port");
        assert_eq!(body["type"], "Paper");
        assert!(body["name"].is_null(), "the real API answers null here");
        assert!(body.get("entries").is_none(), "6.3: 33 KB the layout never reads");
        assert!(body.get("success").is_none());

        assert_eq!(panel.upstream.calls(), 1);
        assert!(panel.upstream.last_body().starts_with("content="));
        assert!(panel.upstream.last_body().contains("FAILED+TO+BIND+TO+PORT"));

        let again = panel.post("/crash-analysis", serde_json::json!({})).await;
        assert_eq!(again.status(), StatusCode::OK);
        assert_eq!(panel.upstream.calls(), 1, "the second answer came out of the cache");

        panel.sandbox.write("logs/latest.log", b"[15:04:23] Could not reserve enough space\n");
        let asked = panel.post("/crash-analysis", serde_json::json!({})).await;
        assert_eq!(asked.status(), StatusCode::OK);
        assert_eq!(panel.upstream.calls(), 2);
    }

    #[tokio::test]
    async fn the_buffer_is_the_other_source_and_an_empty_one_is_a_conflict() {
        let panel = panel().await;

        let buffer = serde_json::json!({ "source": "buffer" });
        let refused = panel.post("/crash-analysis", buffer.clone()).await;
        assert_eq!(refused.status(), StatusCode::CONFLICT);
        assert_eq!(body_json(refused).await["error"], "console_buffer_empty");

        panel
            .operations
            .channel(panel.server)
            .await
            .expect("a channel")
            .console_lines(&["[15:04:22] [Server thread/ERROR]: boom".to_owned()]);

        let answered = panel.post("/crash-analysis", buffer).await;
        assert_eq!(answered.status(), StatusCode::OK);
        assert!(panel.upstream.last_body().contains("boom"));

        let missing = panel.post("/crash-analysis", serde_json::json!({})).await;
        assert_eq!(missing.status(), StatusCode::NOT_FOUND, "there is no latest.log");
        assert_eq!(body_json(missing).await["error"], "log_file_missing");
    }

    #[tokio::test]
    async fn a_throttled_mclogs_stops_the_panel_from_knocking_again() {
        let panel = panel().await;
        panel.sandbox.write("logs/latest.log", b"[15:04:22] one\n");
        panel.upstream.answer_with(429);

        let refused = panel.post("/crash-analysis", serde_json::json!({})).await;
        assert_eq!(refused.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(body_json(refused).await["error"], "upstream_rate_limited");
        assert_eq!(panel.upstream.calls(), 1);

        panel.sandbox.write("logs/latest.log", b"[15:04:23] another question\n");
        let again = panel.post("/crash-analysis", serde_json::json!({})).await;
        assert_eq!(again.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(panel.upstream.calls(), 1, "6.3: a minute of quiet, not a second try");
    }

    #[tokio::test]
    async fn an_upstream_that_breaks_is_a_bad_gateway() {
        let panel = panel().await;
        panel.sandbox.write("logs/latest.log", b"[15:04:22] one\n");
        panel.upstream.answer_with(500);

        let refused = panel.post("/crash-analysis", serde_json::json!({})).await;
        assert_eq!(refused.status(), StatusCode::BAD_GATEWAY);
        assert_eq!(body_json(refused).await["error"], "upstream_unavailable");
    }

    #[tokio::test]
    async fn nothing_goes_out_when_the_admin_switched_outgoing_calls_off() {
        let panel = panel().await;
        panel.sandbox.write("logs/latest.log", b"[15:04:22] one\n");
        sqlx::query("UPDATE panel_settings SET external_services_enabled = 0 WHERE id = 1")
            .execute(&panel.pool)
            .await
            .expect("12.10");

        let refused = panel.post("/crash-analysis", serde_json::json!({})).await;
        assert_eq!(refused.status(), StatusCode::CONFLICT);
        assert_eq!(body_json(refused).await["error"], "external_services_disabled");
        assert_eq!(panel.upstream.calls(), 0);
    }
}
