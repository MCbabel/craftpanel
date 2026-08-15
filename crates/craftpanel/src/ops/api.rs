use std::sync::Arc;

use axum::body::Body;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;

use crate::model::{
    BusyReasonCode, Id, Operation, OperationAccepted, OperationKind, OperationState, Permission,
    Permissions,
};

use super::access;
use super::fault::{Answer, Fault, Params, Route};
use super::store::{self, Payload, Snapshot};
use super::Operations;

pub const PAYLOAD_FILE: &str = "payload.mrpack";

#[derive(Serialize)]
struct AllOperationsResponse {
    operations: Vec<Operation>,
    busy_reasons_by_server: std::collections::BTreeMap<Id, Vec<BusyReasonCode>>,
}

pub fn router(operations: Arc<Operations>) -> Router<crate::AppState> {
    routes(operations)
}

fn routes<S: Clone + Send + Sync + 'static>(operations: Arc<Operations>) -> Router<S> {
    Router::new()
        .route("/operations", get(all_operations))
        .route("/servers/{server}/operations", get(server_operations))
        .route("/servers/{server}/operations/{operation}", get(one_operation))
        .route("/servers/{server}/operations/{operation}/cancel", post(cancel))
        .route("/servers/{server}/operations/{operation}/dismiss", post(dismiss))
        .route("/servers/{server}/operations/{operation}/retry", post(retry))
        .route("/servers/{server}/operations/{operation}/payload", put(payload))
        .layer(axum::middleware::from_fn(crate::auth::extract::same_origin))
        .with_state(operations)
}

async fn all_operations(
    State(operations): State<Arc<Operations>>,
    headers: HeaderMap,
    Params(query): Params<Vec<(String, String)>>,
) -> Answer<Json<AllOperationsResponse>> {
    let caller = access::caller(operations.pool(), &headers).await?;
    let query = PanelQuery::read(&query)?;

    let mut servers = access::visible_servers(operations.pool(), &caller).await?;
    if !query.servers.is_empty() {
        servers.retain(|server| query.servers.contains(server));
    }

    Ok(Json(AllOperationsResponse {
        operations: store::list_for_servers(
            operations.pool(),
            &servers,
            query.active_only,
            query.limit,
            query.before,
        )
        .await?,
        busy_reasons_by_server: store::busy_reasons_by_server(operations.pool(), &servers).await?,
    }))
}

async fn server_operations(
    State(operations): State<Arc<Operations>>,
    headers: HeaderMap,
    Route(server): Route<Id>,
    Params(query): Params<ServerQuery>,
) -> Answer<Json<Snapshot>> {
    let caller = access::caller(operations.pool(), &headers).await?;
    access::permissions(operations.pool(), server, &caller).await?;

    Ok(Json(Snapshot {
        revision: store::revision(operations.pool(), server).await?,
        operations: store::list_for_server(
            operations.pool(),
            server,
            query.state.unwrap_or_default().is_active(),
            query.include_dismissed,
            query.limit.unwrap_or(50).clamp(1, 200),
            query.before,
        )
        .await?,
        busy_reasons: store::busy_reasons(operations.pool(), server).await?,
    }))
}

async fn one_operation(
    State(operations): State<Arc<Operations>>,
    headers: HeaderMap,
    Route((server, operation)): Route<(Id, Id)>,
) -> Answer<Json<Operation>> {
    let caller = access::caller(operations.pool(), &headers).await?;
    access::permissions(operations.pool(), server, &caller).await?;
    Ok(Json(store::fetch_of_server(operations.pool(), server, operation).await?))
}

async fn cancel(
    State(operations): State<Arc<Operations>>,
    headers: HeaderMap,
    Route((server, id)): Route<(Id, Id)>,
) -> Answer<Json<Operation>> {
    let caller = access::caller(operations.pool(), &headers).await?;
    let mask = access::permissions(operations.pool(), server, &caller).await?;
    let operation = store::fetch_of_server(operations.pool(), server, id).await?;
    may_cancel(mask, operation.kind)?;

    let refuse = || {
        Fault::conflict("operation_not_cancellable", "this run cannot be cancelled any more")
    };
    if !operation.kind.is_cancellable() || operation.state.is_terminal() || !operation.cancellable {
        return Err(refuse());
    }
    if operation.kind == OperationKind::BackupRestore && operation.state != OperationState::Queued {
        return Err(refuse());
    }

    let answer = if operation.state == OperationState::Queued {
        operations.cancelled(id).await?
    } else {
        operations.request_cancel(id).await?
    };
    Ok(Json(answer))
}

fn may_cancel(mask: Permissions, kind: OperationKind) -> Answer<()> {
    match kind {
        OperationKind::Unarchive => access::require(mask, Permission::FilesWrite),
        OperationKind::BackupCreate | OperationKind::BackupRestore => {
            access::require(mask, Permission::Backups)
        }
        OperationKind::ServerDelete => access::require(mask, Permission::ServerAdmin),
        _ => access::require(mask, Permission::Setup),
    }
}

async fn dismiss(
    State(operations): State<Arc<Operations>>,
    headers: HeaderMap,
    Route((server, id)): Route<(Id, Id)>,
) -> Answer<StatusCode> {
    let caller = access::caller(operations.pool(), &headers).await?;
    access::permissions(operations.pool(), server, &caller).await?;
    let operation = store::fetch_of_server(operations.pool(), server, id).await?;

    if operation.state.is_open() {
        return Err(Fault::conflict("operation_still_running", "this run has not finished"));
    }
    store::dismiss(operations.pool(), id).await?;
    operations.publish(server, true).await;
    Ok(StatusCode::NO_CONTENT)
}

async fn retry(
    State(operations): State<Arc<Operations>>,
    headers: HeaderMap,
    Route((server, id)): Route<(Id, Id)>,
) -> Answer<(StatusCode, Json<OperationAccepted>)> {
    let caller = access::caller(operations.pool(), &headers).await?;
    let mask = access::permissions(operations.pool(), server, &caller).await?;
    let operation = store::fetch_of_server(operations.pool(), server, id).await?;

    match operation.kind {
        OperationKind::BackupCreate | OperationKind::BackupRestore => {
            access::require(mask, Permission::Backups)?
        }
        _ => access::require(mask, Permission::Setup)?,
    }
    if !operation.kind.is_retryable() || operation.state != OperationState::Failed {
        return Err(Fault::conflict(
            "operation_not_retryable",
            "this kind is not repeated this way, or the run did not fail",
        ));
    }
    operations.guard_write(server).await?;

    let mut inputs = store::inputs_of(operations.pool(), id).await?;
    inputs.started_by = Some(caller.user_id);
    let payload_waits = inputs.expects_payload;
    let fresh = operations.create(inputs).await?;

    if payload_waits && carry_payload(&operations, id, fresh.id).await? {
        store::set_payload(operations.pool(), fresh.id, Payload::Delivered).await?;
    }
    store::dismiss(operations.pool(), id).await?;
    operations.publish(server, true).await;

    let operation = store::fetch(operations.pool(), fresh.id).await?;
    Ok((StatusCode::ACCEPTED, Json(OperationAccepted { operation })))
}

async fn carry_payload(operations: &Operations, from: Id, to: Id) -> Answer<bool> {
    let old = operations.work_dir(from).await?.join(PAYLOAD_FILE);
    let new = operations.work_dir(to).await?.join(PAYLOAD_FILE);
    if !old.exists() {
        return Ok(false);
    }
    if let Some(parent) = new.parent() {
        tokio::fs::create_dir_all(parent).await.map_err(anyhow::Error::from)?;
    }
    Ok(tokio::fs::rename(&old, &new).await.is_ok())
}

async fn payload(
    State(operations): State<Arc<Operations>>,
    headers: HeaderMap,
    Route((server, id)): Route<(Id, Id)>,
    body: Body,
) -> Answer<(StatusCode, Json<OperationAccepted>)> {
    let caller = access::caller(operations.pool(), &headers).await?;
    let mask = access::permissions(operations.pool(), server, &caller).await?;
    access::require(mask, Permission::Setup)?;
    store::fetch_of_server(operations.pool(), server, id).await?;

    let content_type = headers.get(axum::http::header::CONTENT_TYPE).and_then(|v| v.to_str().ok());
    if !content_type.is_none_or(|value| value.starts_with("application/octet-stream")) {
        return Err(Fault::new(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "unsupported_media_type",
            "the body of a payload is application/octet-stream",
        ));
    }

    match store::payload_state(operations.pool(), id).await? {
        Payload::None => {
            return Err(Fault::conflict("payload_not_expected", "this run waits for no upload"))
        }
        Payload::Delivered => {
            return Err(Fault::conflict("payload_already_delivered", "the upload is already here"))
        }
        Payload::Expected => {}
    }
    let _receiving = operations.receive(id).ok_or_else(|| {
        Fault::conflict("payload_already_delivered", "an upload for this run is already on its way")
    })?;

    let ceiling = upload_ceiling(&operations).await?;
    let path = operations.work_dir(id).await?.join(PAYLOAD_FILE);
    let written = write_body(body, &path, ceiling).await;
    if let Err(fault) = written {
        tokio::fs::remove_file(&path).await.ok();
        return Err(fault);
    }

    let check = path.clone();
    let readable = tokio::task::spawn_blocking(move || is_modpack(&check))
        .await
        .map_err(anyhow::Error::from)?;
    if !readable {
        tokio::fs::remove_file(&path).await.ok();
        return Err(Fault::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_modpack",
            "modrinth.index.json is missing or the archive is unreadable",
        ));
    }

    let operation = store::set_payload(operations.pool(), id, Payload::Delivered).await?;
    operations.publish(server, true).await;
    Ok((StatusCode::ACCEPTED, Json(OperationAccepted { operation })))
}

async fn upload_ceiling(operations: &Operations) -> Answer<u64> {
    let (bytes,): (i64,) =
        sqlx::query_as("SELECT max_upload_bytes FROM panel_settings WHERE id = 1")
            .fetch_one(operations.pool())
            .await?;
    Ok(bytes.max(0) as u64)
}

async fn write_body(body: Body, path: &std::path::Path, ceiling: u64) -> Answer<u64> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await.map_err(anyhow::Error::from)?;
    }
    let mut file = tokio::fs::File::create(path).await.map_err(anyhow::Error::from)?;
    let mut stream = body.into_data_stream();
    let mut written = 0u64;

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|err| Fault::invalid_request(err.to_string()))?;
        written += chunk.len() as u64;
        if written > ceiling {
            return Err(Fault::new(
                StatusCode::PAYLOAD_TOO_LARGE,
                "file_too_large",
                "the upload is over the panel's limit",
            ));
        }
        file.write_all(&chunk).await.map_err(anyhow::Error::from)?;
    }
    file.flush().await.map_err(anyhow::Error::from)?;
    Ok(written)
}

fn is_modpack(path: &std::path::Path) -> bool {
    let Ok(file) = std::fs::File::open(path) else {
        return false;
    };
    let Ok(mut archive) = zip::ZipArchive::new(file) else {
        return false;
    };
    let found = archive.by_name("modrinth.index.json").is_ok();
    found
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
enum StateFilter {
    #[default]
    Active,
    All,
}

impl StateFilter {
    fn is_active(self) -> bool {
        self == Self::Active
    }
}

#[derive(Debug, Deserialize)]
struct ServerQuery {
    state: Option<StateFilter>,
    #[serde(default)]
    include_dismissed: bool,
    limit: Option<u32>,
    before: Option<Id>,
}

#[derive(Debug, Default)]
struct PanelQuery {
    active_only: bool,
    servers: Vec<Id>,
    limit: u32,
    before: Option<Id>,
}

impl PanelQuery {
    fn read(pairs: &[(String, String)]) -> Answer<Self> {
        let mut query = Self { active_only: true, limit: 100, ..Self::default() };
        for (key, value) in pairs {
            match key.as_str() {
                "state" => query.active_only = match value.as_str() {
                    "active" => true,
                    "all" => false,
                    other => return Err(Fault::invalid_request(format!("unknown state {other:?}"))),
                },
                "server_id" => query.servers.push(parse(value, "server_id")?),
                "before" => query.before = Some(parse(value, "before")?),
                "limit" => query.limit = parse::<u32>(value, "limit")?.clamp(1, 200),
                _ => {}
            }
        }
        Ok(query)
    }
}

fn parse<T: std::str::FromStr>(value: &str, field: &'static str) -> Answer<T> {
    value.parse().map_err(|_| Fault::invalid_request(format!("{field} is not readable")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{OperationError, OperationErrorStep, PanelRole, ServerRole, Timestamp};
    use crate::ops::store::NewOperation;
    use crate::ops::testing::{self, DataDir};
    use axum::body::to_bytes;
    use axum::http::Request;
    use sqlx::SqlitePool;
    use tower::ServiceExt;

    struct Panel {
        operations: Arc<Operations>,
        pool: SqlitePool,
        router: Router,
        owner: Id,
        cookie: String,
        server: Id,
        _dir: DataDir,
    }

    async fn panel() -> Panel {
        let (operations, dir, pool) = testing::operations().await;
        let owner = testing::a_user(&pool, PanelRole::User).await;
        let cookie = testing::a_session(&pool, owner).await;
        let server = testing::a_server(&pool, owner).await;
        let router = routes::<()>(Arc::clone(&operations));
        Panel { operations, pool, router, owner, cookie, server, _dir: dir }
    }

    impl Panel {
        async fn send(&self, request: Request<Body>) -> (StatusCode, serde_json::Value) {
            let response =
                self.router.clone().oneshot(request).await.expect("the router answers");
            let status = response.status();
            let bytes = to_bytes(response.into_body(), 1 << 20).await.expect("a body");
            let value = if bytes.is_empty() {
                serde_json::Value::Null
            } else {
                serde_json::from_slice(&bytes).expect("json")
            };
            (status, value)
        }

        fn request(&self, method: &str, path: &str) -> Request<Body> {
            Request::builder()
                .method(method)
                .uri(path)
                .header("cookie", format!("craft_session={}", self.cookie))
                .body(Body::empty())
                .expect("a request")
        }

        async fn get(&self, path: &str) -> (StatusCode, serde_json::Value) {
            self.send(self.request("GET", path)).await
        }

        async fn post(&self, path: &str) -> (StatusCode, serde_json::Value) {
            self.send(self.request("POST", path)).await
        }

        async fn an_operation(&self, kind: OperationKind) -> Operation {
            self.operations
                .create(NewOperation::new(self.server, kind, Some(self.owner)))
                .await
                .expect("an operation")
        }
    }

    fn failed() -> OperationError {
        OperationError {
            code: "loader_install_failed".to_owned(),
            message: "internal error".to_owned(),
            step: OperationErrorStep::Modloader,
        }
    }

    #[tokio::test]
    async fn the_two_lists_answer_the_shapes_of_5_1_and_5_2() {
        let panel = panel().await;
        let install = panel.an_operation(OperationKind::InstallLoader).await;

        let (status, body) = panel.get("/operations").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["operations"][0]["id"], install.id.to_string());
        assert_eq!(body["operations"][0]["kind"], "install_loader");
        assert_eq!(body["operations"][0]["state"], "queued");
        assert_eq!(
            body["busy_reasons_by_server"][panel.server.to_string()],
            serde_json::json!(["installing"])
        );

        let (status, body) =
            panel.get(&format!("/servers/{}/operations", panel.server)).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["busy_reasons"], serde_json::json!(["installing"]));
        assert!(body["revision"].as_i64().expect("a revision") > 0);
        assert_eq!(body["operations"].as_array().expect("a list").len(), 1);
    }

    #[tokio::test]
    async fn active_is_the_default_and_all_is_the_way_to_see_what_is_over() {
        let panel = panel().await;
        let install = panel.an_operation(OperationKind::InstallLoader).await;
        panel.operations.finish(install.id).await.expect("it finishes");

        let path = format!("/servers/{}/operations", panel.server);
        let (_, active) = panel.get(&path).await;
        assert_eq!(active["operations"].as_array().expect("a list").len(), 0);
        assert_eq!(active["busy_reasons"], serde_json::json!([]));

        let (_, all) = panel.get(&format!("{path}?state=all")).await;
        assert_eq!(all["operations"][0]["state"], "done");
        assert_eq!(all["operations"][0]["progress"], 1.0);

        let (_, panel_wide) = panel.get("/operations?state=all").await;
        assert_eq!(panel_wide["operations"].as_array().expect("a list").len(), 1);
        assert_eq!(panel_wide["busy_reasons_by_server"], serde_json::json!({}));
    }

    #[tokio::test]
    async fn the_list_is_paged_by_a_limit_and_a_cursor() {
        let panel = panel().await;
        let mut made = Vec::new();
        for _ in 0..5 {
            made.push(panel.an_operation(OperationKind::InstallContent).await.id.to_string());
        }
        let ids = |body: &serde_json::Value| -> Vec<String> {
            body["operations"]
                .as_array()
                .expect("a list")
                .iter()
                .map(|operation| operation["id"].as_str().expect("an id").to_owned())
                .collect()
        };

        let path = format!("/servers/{}/operations?state=all", panel.server);
        let (_, first) = panel.get(&format!("{path}&limit=2")).await;
        assert_eq!(ids(&first), vec![made[4].clone(), made[3].clone()]);

        let cursor = ids(&first).pop().expect("the last of the page");
        let (_, second) = panel.get(&format!("{path}&limit=2&before={cursor}")).await;
        assert_eq!(
            ids(&second),
            vec![made[2].clone(), made[1].clone()],
            "the cursor is exclusive, or every page repeats its first line"
        );
    }

    #[tokio::test]
    async fn the_panel_wide_list_leaves_out_what_was_wiped() {
        let panel = panel().await;
        let operation = panel.an_operation(OperationKind::InstallContent).await;
        panel.operations.finish(operation.id).await.expect("it finishes");

        let (_, before) = panel.get("/operations?state=all").await;
        assert_eq!(before["operations"].as_array().expect("a list").len(), 1);

        let wipe = format!("/servers/{}/operations/{}/dismiss", panel.server, operation.id);
        assert_eq!(panel.post(&wipe).await.0, StatusCode::NO_CONTENT);

        let (_, after) = panel.get("/operations?state=all").await;
        assert_eq!(after["operations"].as_array().expect("a list").len(), 0);
    }

    #[tokio::test]
    async fn a_query_or_a_path_that_will_not_read_answers_in_the_envelope() {
        let panel = panel().await;

        let (status, body) =
            panel.get(&format!("/servers/{}/operations?state=sideways", panel.server)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["error"], "invalid_request");

        let (status, body) = panel.get("/servers/not-a-ulid/operations").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["error"], "invalid_request");
        assert!(body["message"].is_string());
    }

    #[tokio::test]
    async fn a_stranger_gets_the_same_answer_as_for_a_server_that_is_not_there() {
        let panel = panel().await;
        let stranger = testing::a_user(&panel.pool, PanelRole::User).await;
        let cookie = testing::a_session(&panel.pool, stranger).await;
        let operation = panel.an_operation(OperationKind::Unarchive).await;

        let request = Request::builder()
            .method("GET")
            .uri(format!("/servers/{}/operations/{}", panel.server, operation.id))
            .header("cookie", format!("craft_session={cookie}"))
            .body(Body::empty())
            .expect("a request");
        let (status, body) = panel.send(request).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(body["error"], "server_not_found");

        let (status, body) = panel
            .send(
                Request::builder()
                    .method("GET")
                    .uri(format!("/servers/{}/operations", panel.server))
                    .body(Body::empty())
                    .expect("a request"),
            )
            .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(body["error"], "unauthenticated");
    }

    #[tokio::test]
    async fn an_operation_of_another_server_is_not_found_through_this_one() {
        let panel = panel().await;
        let second = testing::a_server(&panel.pool, panel.owner).await;
        let operation = panel
            .operations
            .create(NewOperation::new(second, OperationKind::InstallJava, None))
            .await
            .expect("an operation");

        let (status, body) = panel
            .get(&format!("/servers/{}/operations/{}", panel.server, operation.id))
            .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(body["error"], "operation_not_found");
    }

    #[tokio::test]
    async fn only_the_three_cancellable_kinds_may_be_cancelled() {
        let panel = panel().await;
        for kind in [OperationKind::InstallLoader, OperationKind::ServerDelete] {
            let operation = panel.an_operation(kind).await;
            let (status, body) = panel
                .post(&format!(
                    "/servers/{}/operations/{}/cancel",
                    panel.server, operation.id
                ))
                .await;
            assert_eq!(status, StatusCode::CONFLICT, "{kind} should not be cancellable");
            assert_eq!(body["error"], "operation_not_cancellable");
        }

        let unarchive = panel.an_operation(OperationKind::Unarchive).await;
        let (status, body) = panel
            .post(&format!("/servers/{}/operations/{}/cancel", panel.server, unarchive.id))
            .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["state"], "cancelled");
        assert!(body["finished_at"].is_string());
        assert_eq!(body["error"]["code"], "cancelled_by_user", "the one code 5.11 keeps for it");
        assert_eq!(body["error"]["step"], "internal");
    }

    #[tokio::test]
    async fn a_restore_can_be_called_off_while_it_waits_and_not_once_it_runs() {
        let panel = panel().await;
        let restore = panel.an_operation(OperationKind::BackupRestore).await;
        panel.operations.begin(restore.id).await.expect("it starts");

        let path = format!("/servers/{}/operations/{}/cancel", panel.server, restore.id);
        let (status, body) = panel.post(&path).await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(body["error"], "operation_not_cancellable");
    }

    #[tokio::test]
    async fn a_run_under_way_is_asked_to_stop_and_says_so_when_it_has() {
        let panel = panel().await;
        let unarchive = panel.an_operation(OperationKind::Unarchive).await;
        panel.operations.begin(unarchive.id).await.expect("it starts");

        let (status, body) = panel
            .post(&format!("/servers/{}/operations/{}/cancel", panel.server, unarchive.id))
            .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["state"], "ongoing", "the answer is the run, not a promise");
        assert!(panel.operations.cancel_requested(unarchive.id).await.expect("a flag"));

        let ended = panel.operations.cancelled(unarchive.id).await.expect("the worker gives up");
        assert_eq!(ended.state, OperationState::Cancelled);
    }

    #[tokio::test]
    async fn a_viewer_may_watch_and_wipe_but_not_cancel() {
        let panel = panel().await;
        let viewer = testing::a_user(&panel.pool, PanelRole::User).await;
        let cookie = testing::a_session(&panel.pool, viewer).await;
        sqlx::query(
            "INSERT INTO server_members (id, server_id, user_id, role, invited_at, joined_at)
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(Id::new())
        .bind(panel.server)
        .bind(viewer)
        .bind(ServerRole::Viewer)
        .bind(Timestamp::now())
        .bind(Timestamp::now())
        .execute(&panel.pool)
        .await
        .expect("a membership");

        let unarchive = panel.an_operation(OperationKind::Unarchive).await;
        let as_viewer = |method: &str, path: String| {
            Request::builder()
                .method(method)
                .uri(path)
                .header("cookie", format!("craft_session={cookie}"))
                .body(Body::empty())
                .expect("a request")
        };

        let (status, _) = panel
            .send(as_viewer("GET", format!("/servers/{}/operations", panel.server)))
            .await;
        assert_eq!(status, StatusCode::OK, "a viewer may watch");

        let (status, body) = panel
            .send(as_viewer(
                "POST",
                format!("/servers/{}/operations/{}/cancel", panel.server, unarchive.id),
            ))
            .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(body["error"], "forbidden");

        panel.operations.cancelled(unarchive.id).await.expect("it ends");
        let (status, _) = panel
            .send(as_viewer(
                "POST",
                format!("/servers/{}/operations/{}/dismiss", panel.server, unarchive.id),
            ))
            .await;
        assert_eq!(status, StatusCode::NO_CONTENT, "wiping asks for less than cancelling");
    }

    #[tokio::test]
    async fn a_click_from_a_foreign_window_gets_no_further() {
        let panel = panel().await;
        let unarchive = panel.an_operation(OperationKind::Unarchive).await;
        let from = |origin: &str| {
            Request::builder()
                .method("POST")
                .uri(format!("/servers/{}/operations/{}/cancel", panel.server, unarchive.id))
                .header("cookie", format!("craft_session={}", panel.cookie))
                .header("host", "panel.example")
                .header("origin", origin)
                .body(Body::empty())
                .expect("a request")
        };

        let (status, body) = panel.send(from("https://evil.example")).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(body["error"], "csrf_origin_mismatch");

        let (status, _) = panel.send(from("https://panel.example")).await;
        assert_eq!(status, StatusCode::OK, "our own page is let through");
    }

    #[tokio::test]
    async fn wiping_is_for_runs_that_are_over_and_may_be_repeated() {
        let panel = panel().await;
        let operation = panel.an_operation(OperationKind::InstallContent).await;
        let path = format!("/servers/{}/operations/{}/dismiss", panel.server, operation.id);

        let (status, body) = panel.post(&path).await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(body["error"], "operation_still_running");

        panel.operations.finish(operation.id).await.expect("it finishes");
        assert_eq!(panel.post(&path).await.0, StatusCode::NO_CONTENT);
        assert_eq!(panel.post(&path).await.0, StatusCode::NO_CONTENT, "wiping twice is fine");

        let (_, all) =
            panel.get(&format!("/servers/{}/operations?state=all", panel.server)).await;
        assert_eq!(all["operations"].as_array().expect("a list").len(), 0);

        let (_, kept) = panel
            .get(&format!(
                "/servers/{}/operations?state=all&include_dismissed=true",
                panel.server
            ))
            .await;
        assert_eq!(kept["operations"].as_array().expect("a list").len(), 1);
    }

    #[tokio::test]
    async fn a_retry_is_a_new_run_and_the_old_one_is_wiped_away() {
        let panel = panel().await;
        let first = panel.an_operation(OperationKind::InstallLoader).await;
        panel.operations.begin(first.id).await.expect("it starts");
        panel.operations.fail(first.id, failed()).await.expect("it fails");

        let (status, body) = panel
            .post(&format!("/servers/{}/operations/{}/retry", panel.server, first.id))
            .await;
        assert_eq!(status, StatusCode::ACCEPTED);
        let second = body["operation"]["id"].as_str().expect("an id").to_owned();
        assert_ne!(second, first.id.to_string());
        assert_eq!(body["operation"]["kind"], "install_loader");
        assert_eq!(body["operation"]["state"], "queued");

        let (_, all) =
            panel.get(&format!("/servers/{}/operations?state=all", panel.server)).await;
        let ids: Vec<&str> =
            all["operations"].as_array().expect("a list").iter().map(|o| o["id"].as_str().expect("an id")).collect();
        assert_eq!(ids, vec![second.as_str()]);
    }

    #[tokio::test]
    async fn what_5_6_excludes_is_refused_and_so_is_a_run_that_did_not_fail() {
        let panel = panel().await;
        for kind in
            [OperationKind::Unarchive, OperationKind::ServerDelete, OperationKind::BackupCreate]
        {
            let operation = panel.an_operation(kind).await;
            panel.operations.begin(operation.id).await.ok();
            panel.operations.fail(operation.id, failed()).await.expect("it fails");

            let (status, body) = panel
                .post(&format!("/servers/{}/operations/{}/retry", panel.server, operation.id))
                .await;
            assert_eq!(status, StatusCode::CONFLICT, "{kind} is not retried this way");
            assert_eq!(body["error"], "operation_not_retryable");
        }

        let running = panel.an_operation(OperationKind::InstallModpack).await;
        let (status, body) = panel
            .post(&format!("/servers/{}/operations/{}/retry", panel.server, running.id))
            .await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(body["error"], "operation_not_retryable");
    }

    #[tokio::test]
    async fn a_retry_waits_for_the_lock_of_a_run_that_is_still_going() {
        let panel = panel().await;
        let broken = panel.an_operation(OperationKind::InstallLoader).await;
        panel.operations.fail(broken.id, failed()).await.expect("it fails");
        panel.an_operation(OperationKind::BackupCreate).await;

        let (status, body) = panel
            .post(&format!("/servers/{}/operations/{}/retry", panel.server, broken.id))
            .await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(body["error"], "server_busy");
        assert_eq!(body["message"], "a backup is being created");
    }

    fn a_mrpack() -> Vec<u8> {
        let mut buffer = std::io::Cursor::new(Vec::new());
        let mut zip = zip::ZipWriter::new(&mut buffer);
        zip.start_file("modrinth.index.json", zip::write::SimpleFileOptions::default())
            .expect("an entry");
        std::io::Write::write_all(&mut zip, br#"{"formatVersion":1}"#).expect("the index");
        zip.finish().expect("a zip");
        buffer.into_inner()
    }

    async fn upload(panel: &Panel, id: Id, bytes: Vec<u8>) -> (StatusCode, serde_json::Value) {
        let request = Request::builder()
            .method("PUT")
            .uri(format!("/servers/{}/operations/{}/payload", panel.server, id))
            .header("cookie", format!("craft_session={}", panel.cookie))
            .header("content-type", "application/octet-stream")
            .body(Body::from(bytes))
            .expect("a request");
        panel.send(request).await
    }

    #[tokio::test]
    async fn a_waiting_run_takes_its_payload_once_and_then_no_more() {
        let panel = panel().await;
        let mut waiting = NewOperation::new(
            panel.server,
            OperationKind::ServerCreate,
            Some(panel.owner),
        );
        waiting.expects_payload = true;
        let operation = panel.operations.create(waiting).await.expect("an operation");

        assert!(panel.operations.begin(operation.id).await.expect("no error").is_none());

        let (status, body) = upload(&panel, operation.id, a_mrpack()).await;
        assert_eq!(status, StatusCode::ACCEPTED);
        assert_eq!(body["operation"]["state"], "queued");
        assert!(panel.operations.begin(operation.id).await.expect("no error").is_some());

        let path = panel.operations.work_dir(operation.id).await.expect("a path").join(PAYLOAD_FILE);
        assert!(path.exists(), "the upload lands in the work directory of its run");

        let (status, body) = upload(&panel, operation.id, a_mrpack()).await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(body["error"], "payload_already_delivered");
    }

    #[tokio::test]
    async fn a_payload_nobody_waits_for_and_one_that_is_no_modpack() {
        let panel = panel().await;
        let plain = panel.an_operation(OperationKind::InstallLoader).await;
        let (status, body) = upload(&panel, plain.id, a_mrpack()).await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(body["error"], "payload_not_expected");

        let mut waiting =
            NewOperation::new(panel.server, OperationKind::ServerCreate, Some(panel.owner));
        waiting.expects_payload = true;
        let operation = panel.operations.create(waiting).await.expect("an operation");

        let (status, body) = upload(&panel, operation.id, b"not a zip at all".to_vec()).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(body["error"], "invalid_modpack");
        let path = panel.operations.work_dir(operation.id).await.expect("a path").join(PAYLOAD_FILE);
        assert!(!path.exists(), "a body we refuse leaves nothing behind");

        assert_eq!(
            store::payload_state(panel.operations.pool(), operation.id).await.expect("a state"),
            Payload::Expected
        );
    }

    #[tokio::test]
    async fn a_second_upload_is_turned_away_while_the_first_is_still_arriving() {
        let panel = panel().await;
        let mut waiting =
            NewOperation::new(panel.server, OperationKind::ServerCreate, Some(panel.owner));
        waiting.expects_payload = true;
        let operation = panel.operations.create(waiting).await.expect("an operation");

        let first = panel.operations.receive(operation.id).expect("the first upload");
        let (status, body) = upload(&panel, operation.id, a_mrpack()).await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(body["error"], "payload_already_delivered");

        drop(first);
        assert_eq!(
            upload(&panel, operation.id, a_mrpack()).await.0,
            StatusCode::ACCEPTED,
            "and the run is free again once the first one is over"
        );
    }

    #[tokio::test]
    async fn a_form_post_at_the_payload_endpoint_is_the_wrong_media_type() {
        let panel = panel().await;
        let mut waiting =
            NewOperation::new(panel.server, OperationKind::ServerCreate, Some(panel.owner));
        waiting.expects_payload = true;
        let operation = panel.operations.create(waiting).await.expect("an operation");

        let request = Request::builder()
            .method("PUT")
            .uri(format!("/servers/{}/operations/{}/payload", panel.server, operation.id))
            .header("cookie", format!("craft_session={}", panel.cookie))
            .header("content-type", "multipart/form-data; boundary=x")
            .body(Body::from(a_mrpack()))
            .expect("a request");
        let (status, body) = panel.send(request).await;
        assert_eq!(status, StatusCode::UNSUPPORTED_MEDIA_TYPE);
        assert_eq!(body["error"], "unsupported_media_type");
    }

    #[tokio::test]
    async fn an_upload_over_the_panel_limit_is_cut_off() {
        let panel = panel().await;
        sqlx::query("UPDATE panel_settings SET max_upload_bytes = 64 WHERE id = 1")
            .execute(&panel.pool)
            .await
            .expect("a smaller ceiling");

        let mut waiting =
            NewOperation::new(panel.server, OperationKind::ServerCreate, Some(panel.owner));
        waiting.expects_payload = true;
        let operation = panel.operations.create(waiting).await.expect("an operation");

        let (status, body) = upload(&panel, operation.id, vec![0u8; 4096]).await;
        assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
        assert_eq!(body["error"], "file_too_large");
    }

    #[tokio::test]
    async fn the_panel_wide_list_can_be_narrowed_to_the_servers_that_were_asked_for() {
        let panel = panel().await;
        let second = testing::a_server(&panel.pool, panel.owner).await;
        panel.an_operation(OperationKind::InstallLoader).await;
        panel
            .operations
            .create(NewOperation::new(second, OperationKind::BackupCreate, None))
            .await
            .expect("an operation");

        let (_, both) = panel.get("/operations").await;
        assert_eq!(both["operations"].as_array().expect("a list").len(), 2);
        assert_eq!(both["busy_reasons_by_server"].as_object().expect("a map").len(), 2);

        let (_, one) = panel.get(&format!("/operations?server_id={second}")).await;
        assert_eq!(one["operations"].as_array().expect("a list").len(), 1);
        assert_eq!(
            one["busy_reasons_by_server"][second.to_string()],
            serde_json::json!(["backup_creating"])
        );
    }
}
