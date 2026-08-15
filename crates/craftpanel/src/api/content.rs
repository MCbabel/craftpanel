use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::{FromRequestParts, Path, RawQuery, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Extension, Json, Router};

use crate::auth::access;
use crate::auth::disk;
use crate::auth::error::{Failure, Result};
use crate::auth::{Caller, JsonBody, LiveServers, Params};
use crate::content::types::*;
use crate::content::{boundary_of, Content, PackSource, PartBody};
use crate::model::{Id, Operation, Permission};
use crate::ops::WORK_DIR;
use crate::AppState;

const JSON_LIMIT: usize = 16 * 1024;

const ALLOWED: [&[&str]; 14] = [
    &["v2", "search"],
    &["v3", "search"],
    &["v2", "project", "*"],
    &["v3", "project", "*"],
    &["v2", "projects"],
    &["v2", "project", "*", "version"],
    &["v2", "version", "*"],
    &["v2", "versions"],
    &["v2", "version_file", "*"],
    &["v2", "tag", "game_version"],
    &["v2", "tag", "loader"],
    &["v2", "tag", "category"],
    &["v2", "user", "*"],
    &["v2", "team", "*", "members"],
];

pub fn router(content: Arc<Content>, live: LiveServers) -> Router<AppState> {
    Router::new()
        .route("/servers/{server}/content", get(list))
        .route("/servers/{server}/content/modpack/contents", get(modpack_contents))
        .route("/servers/{server}/content/enable", post(enable))
        .route("/servers/{server}/content/disable", post(disable))
        .route("/servers/{server}/content/delete", post(delete))
        .route("/servers/{server}/content/update", post(update))
        .route("/servers/{server}/content/install", post(install))
        .route("/servers/{server}/content/upload", post(upload))
        .route("/servers/{server}/content/dependents", post(dependents))
        .route("/servers/{server}/content/modpack/install", post(modpack_install))
        .route("/servers/{server}/content/modpack/update", post(modpack_update))
        .route("/servers/{server}/content/modpack/unlink", post(modpack_unlink))
        .route("/servers/{server}/content/game-version/preview", get(preview))
        .route("/servers/{server}/content/game-version", post(game_version))
        .route("/modrinth/{*path}", get(modrinth))
        .layer(Extension(content))
        .layer(Extension(live))
        .layer(axum::middleware::from_fn(crate::auth::extract::same_origin))
}

struct OfServer(Id);

impl FromRequestParts<AppState> for OfServer {
    type Rejection = Failure;

    async fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        state: &AppState,
    ) -> Result<Self> {
        let Path(raw) = Path::<String>::from_request_parts(parts, state)
            .await
            .map_err(|_| Failure::not_found("server_not_found", "no such server"))?;
        raw.parse().map(Self).map_err(|_| Failure::not_found("server_not_found", "no such server"))
    }
}

async fn list(
    State(state): State<AppState>,
    Extension(content): Extension<Arc<Content>>,
    caller: Caller,
    OfServer(server): OfServer,
    Params(query): Params<ListQuery>,
) -> Result<Json<ContentListResponse>> {
    let access = access::require(&state.pool, &caller, server, Permission::BaseRead).await?;
    Ok(Json(content.list(access, query.refresh_updates).await?))
}

async fn modpack_contents(
    State(state): State<AppState>,
    Extension(content): Extension<Arc<Content>>,
    caller: Caller,
    OfServer(server): OfServer,
) -> Result<Json<ModpackContentsResponse>> {
    access::require(&state.pool, &caller, server, Permission::BaseRead).await?;
    Ok(Json(content.modpack_contents(server).await?))
}

async fn enable(
    State(state): State<AppState>,
    Extension(content): Extension<Arc<Content>>,
    caller: Caller,
    OfServer(server): OfServer,
    JsonBody(body): JsonBody<ContentIdsRequest>,
) -> Result<Json<ContentMutationResponse>> {
    switch(state, content, caller, server, body, true).await
}

async fn disable(
    State(state): State<AppState>,
    Extension(content): Extension<Arc<Content>>,
    caller: Caller,
    OfServer(server): OfServer,
    JsonBody(body): JsonBody<ContentIdsRequest>,
) -> Result<Json<ContentMutationResponse>> {
    switch(state, content, caller, server, body, false).await
}

async fn switch(
    state: AppState,
    content: Arc<Content>,
    caller: Caller,
    server: Id,
    body: ContentIdsRequest,
    enabled: bool,
) -> Result<Json<ContentMutationResponse>> {
    access::require(&state.pool, &caller, server, Permission::Setup).await?;
    let ids = at_least_one(body.ids)?;
    guard(&content, server).await?;
    all_or_some(content.set_enabled(server, &ids, enabled).await?)
}

async fn delete(
    State(state): State<AppState>,
    Extension(content): Extension<Arc<Content>>,
    caller: Caller,
    OfServer(server): OfServer,
    JsonBody(body): JsonBody<ContentIdsRequest>,
) -> Result<Json<ContentMutationResponse>> {
    access::require(&state.pool, &caller, server, Permission::Setup).await?;
    let ids = at_least_one(body.ids)?;
    guard(&content, server).await?;
    all_or_some(content.delete(server, &ids).await?)
}

async fn update(
    State(state): State<AppState>,
    Extension(content): Extension<Arc<Content>>,
    caller: Caller,
    OfServer(server): OfServer,
    JsonBody(body): JsonBody<ContentUpdateRequest>,
) -> Result<(StatusCode, Json<ContentUpdateResponse>)> {
    access::require(&state.pool, &caller, server, Permission::Setup).await?;
    if body.items.is_empty() && !body.all {
        return Err(Failure::invalid_request("name items or ask for all of them"));
    }
    outgoing(&content).await?;
    let answer = content.update(server, &body, Some(caller.id())).await?;
    Ok((StatusCode::ACCEPTED, Json(answer)))
}

async fn install(
    State(state): State<AppState>,
    Extension(content): Extension<Arc<Content>>,
    caller: Caller,
    OfServer(server): OfServer,
    JsonBody(body): JsonBody<ContentInstallRequest>,
) -> Result<(StatusCode, Json<ContentInstallResponse>)> {
    access::require(&state.pool, &caller, server, Permission::Setup).await?;
    outgoing(&content).await?;
    let answer = content.install(server, &body, Some(caller.id())).await?;
    Ok((StatusCode::ACCEPTED, Json(answer)))
}

async fn upload(
    State(state): State<AppState>,
    Extension(content): Extension<Arc<Content>>,
    caller: Caller,
    OfServer(server): OfServer,
    headers: HeaderMap,
    body: axum::body::Body,
) -> Result<Json<ContentUploadResponse>> {
    access::require(&state.pool, &caller, server, Permission::Setup).await?;
    guard(&content, server).await?;

    let facts = content.facts(server).await?;
    let announced = announced(&headers).unwrap_or(0);
    disk::guard(&state.pool, content.disks(), facts.owner_id, announced).await?;

    let staging = content.server_dir(&facts).join(WORK_DIR).join(format!("upload-{}", Id::new()));
    let parts = read_parts(&content, &headers, body, &staging).await?;

    let mut uploads = Vec::new();
    for part in parts {
        if let (Some(file_name), PartBody::File { path, size }) = (part.file_name, part.body) {
            uploads.push((file_name, path, size));
        }
    }
    if uploads.is_empty() {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(Failure::invalid_request("no file was sent"));
    }

    let answer = content.adopt_uploads(server, uploads).await;
    let _ = std::fs::remove_dir_all(&staging);
    every_file_refused(answer?)
}

async fn dependents(
    State(state): State<AppState>,
    Extension(content): Extension<Arc<Content>>,
    caller: Caller,
    OfServer(server): OfServer,
    JsonBody(body): JsonBody<ContentIdsRequest>,
) -> Result<Json<ContentDependentsResponse>> {
    access::require(&state.pool, &caller, server, Permission::BaseRead).await?;
    Ok(Json(content.dependents(server, &body.ids).await?))
}

async fn modpack_install(
    State(state): State<AppState>,
    Extension(content): Extension<Arc<Content>>,
    Extension(live): Extension<LiveServers>,
    caller: Caller,
    OfServer(server): OfServer,
    request: axum::extract::Request,
) -> Result<(StatusCode, Json<Accepted>)> {
    access::require(&state.pool, &caller, server, Permission::Setup).await?;
    if content.linked(server).await?.is_some() {
        return Err(Failure::conflict(
            "modpack_already_linked",
            "unlink the modpack before installing another",
        ));
    }

    let (parts, body) = request.into_parts();
    let headers = parts.headers;
    let (source, keep_extra) = match content_type(&headers) {
        Some(kind) if kind.starts_with("multipart/form-data") => {
            let facts = content.facts(server).await?;
            let announced = announced(&headers).unwrap_or(0);
            disk::guard(&state.pool, content.disks(), facts.owner_id, announced).await?;
            let staging =
                content.server_dir(&facts).join(WORK_DIR).join(format!("pack-{}", Id::new()));
            let parts = read_parts(&content, &headers, body, &staging).await?;

            let mut archive: Option<(PathBuf, String)> = None;
            let mut keep_extra = false;
            for part in parts {
                match (part.file_name, part.body) {
                    (Some(file_name), PartBody::File { path, .. }) => {
                        if !file_name.to_ascii_lowercase().ends_with(".mrpack") {
                            return Err(Failure::new(
                                StatusCode::UNSUPPORTED_MEDIA_TYPE,
                                "unsupported_file_type",
                                "a modpack is a .mrpack",
                            ));
                        }
                        archive = Some((path, file_name));
                    }
                    (None, PartBody::Text(text)) if part.name == "meta" => {
                        let meta: ModpackInstallRequest = serde_json::from_str(&text)
                            .map_err(|err| Failure::invalid_request(err.to_string()))?;
                        keep_extra = meta.keep_extra_content;
                    }
                    _ => {}
                }
            }

            let (archive, file_name) =
                archive.ok_or_else(|| Failure::invalid_request("no .mrpack was sent"))?;
            (PackSource::Upload { archive, file_name }, keep_extra)
        }
        Some(kind) if kind.split(';').next().is_some_and(|kind| kind.trim() == "application/json") => {
            let bytes = axum::body::to_bytes(body, JSON_LIMIT)
                .await
                .map_err(|err| Failure::invalid_request(err.to_string()))?;
            let request: ModpackInstallRequest = serde_json::from_slice(&bytes)
                .map_err(|err| Failure::invalid_request(err.to_string()))?;
            outgoing(&content).await?;
            match request.source {
                ModpackSource::Modrinth { project_id, version_id } => (
                    PackSource::Modrinth { project_id, version_id },
                    request.keep_extra_content,
                ),
                ModpackSource::Upload => {
                    return Err(Failure::invalid_request(
                        "an uploaded pack arrives as multipart/form-data",
                    ))
                }
            }
        }
        _ => {
            return Err(Failure::new(
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
                "unsupported_media_type",
                "8.10 reads application/json or multipart/form-data",
            ))
        }
    };

    let running = live.among(&[server]).await.contains(&server);
    let operation =
        content.install_modpack(server, source, keep_extra, Some(caller.id()), running).await?;
    Ok((StatusCode::ACCEPTED, Json(Accepted { operation })))
}

async fn modpack_update(
    State(state): State<AppState>,
    Extension(content): Extension<Arc<Content>>,
    Extension(live): Extension<LiveServers>,
    caller: Caller,
    OfServer(server): OfServer,
    JsonBody(body): JsonBody<ModpackUpdateRequest>,
) -> Result<(StatusCode, Json<Accepted>)> {
    access::require(&state.pool, &caller, server, Permission::Setup).await?;
    let Some(project_id) = content.linked(server).await?.flatten() else {
        return Err(Failure::conflict("modpack_not_linked", "no modpack on this server"));
    };
    outgoing(&content).await?;

    let running = live.among(&[server]).await.contains(&server);
    let source = PackSource::Modrinth { project_id, version_id: body.version_id };
    let operation =
        content.install_modpack(server, source, true, Some(caller.id()), running).await?;
    Ok((StatusCode::ACCEPTED, Json(Accepted { operation })))
}

async fn modpack_unlink(
    State(state): State<AppState>,
    Extension(content): Extension<Arc<Content>>,
    caller: Caller,
    OfServer(server): OfServer,
) -> Result<Json<ModpackUnlinkResponse>> {
    access::require(&state.pool, &caller, server, Permission::Setup).await?;
    guard(&content, server).await?;
    Ok(Json(content.unlink_modpack(server).await?))
}

async fn preview(
    State(state): State<AppState>,
    Extension(content): Extension<Arc<Content>>,
    caller: Caller,
    OfServer(server): OfServer,
    Params(query): Params<PreviewQuery>,
) -> Result<Json<GameVersionPreviewResponse>> {
    access::require(&state.pool, &caller, server, Permission::BaseRead).await?;
    outgoing(&content).await?;
    Ok(Json(content.preview(server, &query).await?))
}

async fn game_version(
    State(state): State<AppState>,
    Extension(content): Extension<Arc<Content>>,
    Extension(live): Extension<LiveServers>,
    caller: Caller,
    OfServer(server): OfServer,
    JsonBody(body): JsonBody<GameVersionChangeRequest>,
) -> Result<(StatusCode, Json<Accepted>)> {
    access::require(&state.pool, &caller, server, Permission::Setup).await?;
    outgoing(&content).await?;
    let running = live.among(&[server]).await.contains(&server);
    let operation =
        content.change_game_version(server, body, Some(caller.id()), running).await?;
    Ok((StatusCode::ACCEPTED, Json(Accepted { operation })))
}

async fn modrinth(
    Extension(content): Extension<Arc<Content>>,
    _caller: Caller,
    Path(path): Path<String>,
    RawQuery(query): RawQuery,
) -> Result<Response> {
    let path = format!("/{}", path.trim_start_matches('/'));
    if !on_the_list(&path) {
        return Err(Failure::forbidden());
    }
    outgoing(&content).await?;

    let answer = content
        .modrinth()
        .passthrough(&path, query.as_deref())
        .await
        .map_err(crate::content::upstream)?;

    let mut response = Response::builder()
        .status(StatusCode::from_u16(answer.status).unwrap_or(StatusCode::BAD_GATEWAY));
    if let Some(kind) = answer.content_type.as_deref().and_then(|kind| HeaderValue::from_str(kind).ok())
    {
        response = response.header(header::CONTENT_TYPE, kind);
    }
    Ok(response
        .header(header::CACHE_CONTROL, "public, max-age=300")
        .header(header::X_CONTENT_TYPE_OPTIONS, "nosniff")
        .body(axum::body::Body::from(answer.body))
        .map_err(|err| Failure::internal(anyhow::Error::from(err)))?
        .into_response())
}

fn on_the_list(path: &str) -> bool {
    let segments: Vec<&str> = path.trim_matches('/').split('/').collect();
    ALLOWED.iter().any(|pattern| {
        pattern.len() == segments.len()
            && pattern
                .iter()
                .zip(&segments)
                .all(|(wanted, given)| *wanted == "*" || wanted == given)
    })
}

#[derive(serde::Serialize)]
pub struct Accepted {
    pub operation: Operation,
}

fn all_or_some(answer: ContentMutationResponse) -> Result<Json<ContentMutationResponse>> {
    let nothing_found = !answer.results.is_empty()
        && answer.results.iter().all(|result| result.error.as_deref() == Some("content_not_found"));
    if nothing_found {
        return Err(Failure::not_found("content_not_found", "none of those items are here"));
    }
    Ok(Json(answer))
}

fn every_file_refused(answer: ContentUploadResponse) -> Result<Json<ContentUploadResponse>> {
    let all_wrong = !answer.results.is_empty()
        && answer
            .results
            .iter()
            .all(|result| result.error.as_deref() == Some("unsupported_file_type"));
    if all_wrong {
        return Err(Failure::new(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "unsupported_file_type",
            "only .jar and .zip go here; a .mrpack is 8.10",
        ));
    }
    Ok(Json(answer))
}

fn at_least_one(ids: Vec<Id>) -> Result<Vec<Id>> {
    if ids.is_empty() {
        return Err(Failure::invalid_request("the list of ids is empty"));
    }
    Ok(ids)
}

async fn guard(content: &Arc<Content>, server: Id) -> Result<()> {
    content.operations().guard_write(server).await.map_err(|fault| {
        Failure::new(fault.status(), fault.code(), fault.message().to_owned())
    })
}

async fn outgoing(content: &Arc<Content>) -> Result<()> {
    if content.modrinth().allowed().await {
        return Ok(());
    }
    Err(Failure::conflict(
        "external_services_disabled",
        "an administrator has switched outgoing calls off",
    ))
}

fn content_type(headers: &HeaderMap) -> Option<&str> {
    headers.get(header::CONTENT_TYPE)?.to_str().ok()
}

fn announced(headers: &HeaderMap) -> Option<u64> {
    headers.get(header::CONTENT_LENGTH)?.to_str().ok()?.parse().ok()
}

async fn read_parts(
    content: &Arc<Content>,
    headers: &HeaderMap,
    body: axum::body::Body,
    staging: &std::path::Path,
) -> Result<Vec<crate::content::Part>> {
    let boundary = content_type(headers)
        .ok_or(())
        .and_then(|kind| boundary_of(kind).map_err(|_| ()))
        .map_err(|()| {
            Failure::new(
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
                "unsupported_media_type",
                "this endpoint reads multipart/form-data",
            )
        })?;

    let limit = content.max_upload_bytes().await?;
    crate::content::collect_parts(&boundary, body, staging, limit).await.map_err(|fault| {
        let status = match fault.code() {
            "file_too_large" => StatusCode::PAYLOAD_TOO_LARGE,
            "unsupported_media_type" => StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "internal" => StatusCode::INTERNAL_SERVER_ERROR,
            _ => StatusCode::BAD_REQUEST,
        };
        Failure::new(status, fault.code(), fault.to_string())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::harness::{as_user, body_json, empty, fetch, send, sign_in, state_with};
    use crate::auth::Disks;
    use crate::content::harness::{self, DataDir};
    use crate::model::ServerRole;
    use axum::body::Body;
    use axum::http::Request;
    use sqlx::SqlitePool;
    use tower::ServiceExt;

    const MIB: u64 = 1024 * 1024;

    struct Panel {
        app: Router,
        pool: SqlitePool,
        owner: Id,
        server: Id,
        root: std::path::PathBuf,
        upstream: crate::content::harness::FakeModrinth,
        _helper: crate::auth::harness::FakeHelper,
        _dir: DataDir,
    }

    impl Panel {
        async fn new() -> Self {
            Self::with_disks(Disks::none()).await
        }

        async fn with_disks(disks: Disks) -> Self {
            let pool = harness::schema().await;
            let dir = DataDir::new();
            let owner = harness::a_user(&pool).await;
            let server = harness::a_server(&pool, owner, "fabric", "1.21.1").await;
            let root = dir
                .path()
                .join("users")
                .join(owner.to_string())
                .join("servers")
                .join(server.to_string());
            std::fs::create_dir_all(root.join("mods")).expect("a server directory");

            let upstream = harness::fake_modrinth().await;
            let helper = crate::auth::harness::FakeHelper::obliging().await;
            let operations = crate::ops::Operations::new(pool.clone(), dir.path());
            let content = Content::with_modrinth(
                pool.clone(),
                dir.path(),
                crate::helper::Helper::new(helper.socket()),
                operations,
                Arc::new(harness::client(&pool, &upstream)),
                disks,
            );

            let mut config = crate::config::Config::default();
            config.data_dir = dir.path().to_path_buf();
            let app = router(content, LiveServers::none()).with_state(state_with(&pool, config));

            Self { app, pool, owner, server, root, upstream, _helper: helper, _dir: dir }
        }

        async fn call(&self, request: Request<Body>) -> axum::http::Response<Body> {
            self.app.clone().oneshot(request).await.expect("an answer")
        }

        async fn a_member(&self, role: ServerRole) -> Id {
            let user = harness::a_user(&self.pool).await;
            sqlx::query(
                "INSERT INTO server_members (id, server_id, user_id, role, invited_at, joined_at)
                 VALUES (?, ?, ?, ?, ?, ?)",
            )
            .bind(Id::new())
            .bind(self.server)
            .bind(user)
            .bind(role)
            .bind(crate::model::Timestamp::now())
            .bind(crate::model::Timestamp::now())
            .execute(&self.pool)
            .await
            .expect("a membership");
            user
        }

        fn a_multipart(&self, tail: &str, file_name: &str, bytes: &[u8]) -> Request<Body> {
            let boundary = "----craftpanelUpload";
            let mut body = format!(
                "--{boundary}\r\n\
                 Content-Disposition: form-data; name=\"file\"; filename=\"{file_name}\"\r\n\r\n"
            )
            .into_bytes();
            body.extend_from_slice(bytes);
            body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());

            Request::builder()
                .method("POST")
                .uri(format!("/servers/{}/content{tail}", self.server))
                .header(header::CONTENT_TYPE, format!("multipart/form-data; boundary={boundary}"))
                .header(header::CONTENT_LENGTH, body.len())
                .body(Body::from(body))
                .expect("a request")
        }

        fn an_upload(&self, file_name: &str, bytes: &[u8]) -> Request<Body> {
            self.a_multipart("/upload", file_name, bytes)
        }

        async fn limit_of_a_gibibyte(&self) {
            sqlx::query("UPDATE users SET disk_mib = 1024 WHERE id = ?")
                .bind(self.owner)
                .execute(&self.pool)
                .await
                .expect("the disk limit");
        }
    }

    #[test]
    fn the_passthrough_lets_through_exactly_the_fourteen_patterns_of_8_15() {
        for allowed in [
            "/v2/search",
            "/v3/search",
            "/v2/project/AANobbMI",
            "/v3/project/AANobbMI",
            "/v2/projects",
            "/v2/project/AANobbMI/version",
            "/v2/version/aaaaaaaa",
            "/v2/versions",
            "/v2/version_file/abc123",
            "/v2/tag/game_version",
            "/v2/tag/loader",
            "/v2/tag/category",
            "/v2/user/somebody",
            "/v2/team/team-1/members",
        ] {
            assert!(on_the_list(allowed), "{allowed}");
        }

        for refused in [
            "/v2/project/AANobbMI/follow",
            "/v2/notifications",
            "/v2/tag/side_type",
            "/v2/team/team-1",
            "/v3/project/AANobbMI/version",
            "/",
            "/v2",
        ] {
            assert!(!on_the_list(refused), "{refused}");
        }
    }

    #[tokio::test]
    async fn the_passthrough_answers_for_a_pattern_on_the_list_and_refuses_the_rest() {
        let panel = Panel::new().await;
        let session = sign_in(&panel.pool, panel.owner).await;

        let allowed = panel.call(as_user(fetch("/modrinth/v2/search?query=sodium"), &session)).await;
        assert_eq!(allowed.status(), StatusCode::OK);
        assert!(allowed.headers().contains_key(header::CACHE_CONTROL));

        let refused = panel.call(as_user(fetch("/modrinth/v2/notifications"), &session)).await;
        assert_eq!(refused.status(), StatusCode::FORBIDDEN);
        assert_eq!(body_json(refused).await["error"], "forbidden");

        let stranger = panel.call(fetch("/modrinth/v2/search")).await;
        assert_eq!(stranger.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(panel.upstream.calls(), 1, "a refusal must not cost an upstream call");
    }

    #[tokio::test]
    async fn a_viewer_reads_the_list_and_may_not_change_it() {
        let panel = Panel::new().await;
        std::fs::write(panel.root.join("mods").join("foo.jar"), b"jar").expect("a mod");
        let viewer = panel.a_member(ServerRole::Viewer).await;
        let session = sign_in(&panel.pool, viewer).await;

        let listed =
            panel.call(as_user(fetch(&format!("/servers/{}/content", panel.server)), &session)).await;
        assert_eq!(listed.status(), StatusCode::OK);
        let body = body_json(listed).await;
        assert_eq!(body["items"].as_array().expect("items").len(), 1);
        assert_eq!(body["permissions"]["can_write"], false);

        let refused = panel
            .call(as_user(
                send(
                    "POST",
                    &format!("/servers/{}/content/disable", panel.server),
                    serde_json::json!({ "ids": [body["items"][0]["id"]] }),
                ),
                &session,
            ))
            .await;
        assert_eq!(refused.status(), StatusCode::FORBIDDEN, "2.1: 8.3–8.8 want SETUP");
    }

    #[tokio::test]
    async fn a_stranger_is_told_the_server_does_not_exist() {
        let panel = Panel::new().await;
        let stranger = harness::a_user(&panel.pool).await;
        let session = sign_in(&panel.pool, stranger).await;

        let answer =
            panel.call(as_user(fetch(&format!("/servers/{}/content", panel.server)), &session)).await;
        assert_eq!(answer.status(), StatusCode::NOT_FOUND);
        assert_eq!(body_json(answer).await["error"], "server_not_found");
    }

    #[tokio::test]
    async fn an_editor_may_disable_and_the_file_moves() {
        let panel = Panel::new().await;
        std::fs::write(panel.root.join("mods").join("foo.jar"), b"jar").expect("a mod");
        let editor = panel.a_member(ServerRole::Editor).await;
        let session = sign_in(&panel.pool, editor).await;

        let listed =
            panel.call(as_user(fetch(&format!("/servers/{}/content", panel.server)), &session)).await;
        let id = body_json(listed).await["items"][0]["id"].clone();

        let switched = panel
            .call(as_user(
                send(
                    "POST",
                    &format!("/servers/{}/content/disable", panel.server),
                    serde_json::json!({ "ids": [id] }),
                ),
                &session,
            ))
            .await;
        assert_eq!(switched.status(), StatusCode::OK);
        assert_eq!(body_json(switched).await["results"][0]["ok"], true);
        assert!(panel.root.join("mods").join("foo.jar.disabled").exists());
    }

    #[tokio::test]
    async fn naming_only_items_that_are_not_here_is_a_404() {
        let panel = Panel::new().await;
        let session = sign_in(&panel.pool, panel.owner).await;
        let answer = panel
            .call(as_user(
                send(
                    "POST",
                    &format!("/servers/{}/content/delete", panel.server),
                    serde_json::json!({ "ids": [Id::new().to_string()] }),
                ),
                &session,
            ))
            .await;
        assert_eq!(answer.status(), StatusCode::NOT_FOUND);
        assert_eq!(body_json(answer).await["error"], "content_not_found");
    }

    #[tokio::test]
    async fn one_bad_id_beside_a_good_one_still_answers_200_with_both_results() {
        let panel = Panel::new().await;
        std::fs::write(panel.root.join("mods").join("foo.jar"), b"jar").expect("a mod");
        let session = sign_in(&panel.pool, panel.owner).await;
        let listed =
            panel.call(as_user(fetch(&format!("/servers/{}/content", panel.server)), &session)).await;
        let id = body_json(listed).await["items"][0]["id"].clone();

        let answer = panel
            .call(as_user(
                send(
                    "POST",
                    &format!("/servers/{}/content/delete", panel.server),
                    serde_json::json!({ "ids": [id, Id::new().to_string()] }),
                ),
                &session,
            ))
            .await;
        assert_eq!(answer.status(), StatusCode::OK);
        let results = body_json(answer).await;
        assert_eq!(results["results"][0]["ok"], true);
        assert_eq!(results["results"][1]["ok"], false);
    }

    #[tokio::test]
    async fn an_empty_list_of_ids_is_a_bad_request() {
        let panel = Panel::new().await;
        let session = sign_in(&panel.pool, panel.owner).await;

        let answer = panel
            .call(as_user(
                send(
                    "POST",
                    &format!("/servers/{}/content/delete", panel.server),
                    serde_json::json!({ "ids": [] }),
                ),
                &session,
            ))
            .await;
        assert_eq!(answer.status(), StatusCode::BAD_REQUEST);
        assert_eq!(body_json(answer).await["error"], "invalid_request");
    }

    #[tokio::test]
    async fn a_form_posted_at_a_json_endpoint_is_the_wrong_media_type() {
        let panel = Panel::new().await;
        let session = sign_in(&panel.pool, panel.owner).await;

        let request = Request::builder()
            .method("POST")
            .uri(format!("/servers/{}/content/delete", panel.server))
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
            .body(Body::from("ids=1"))
            .expect("a request");
        let answer = panel.call(as_user(request, &session)).await;
        assert_eq!(answer.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
        assert_eq!(body_json(answer).await["error"], "unsupported_media_type");
    }

    #[tokio::test]
    async fn an_upload_arrives_as_multipart_and_lands_in_the_content_directory() {
        let panel = Panel::new().await;
        let session = sign_in(&panel.pool, panel.owner).await;
        let boundary = "----craftpanelUpload";
        let body = format!(
            "--{boundary}\r\n\
             Content-Disposition: form-data; name=\"file\"; filename=\"nice.jar\"\r\n\
             Content-Type: application/java-archive\r\n\r\n\
             the bytes of a jar\r\n\
             --{boundary}--\r\n"
        );

        let request = Request::builder()
            .method("POST")
            .uri(format!("/servers/{}/content/upload", panel.server))
            .header(header::CONTENT_TYPE, format!("multipart/form-data; boundary={boundary}"))
            .body(Body::from(body))
            .expect("a request");
        let answer = panel.call(as_user(request, &session)).await;

        assert_eq!(answer.status(), StatusCode::OK);
        let results = body_json(answer).await;
        assert_eq!(results["results"][0]["ok"], true);
        assert_eq!(results["results"][0]["file_name"], "nice.jar");
        assert_eq!(
            std::fs::read(panel.root.join("mods").join("nice.jar")).expect("the jar"),
            b"the bytes of a jar"
        );
    }

    #[tokio::test]
    async fn a_json_upload_at_the_upload_endpoint_is_refused() {
        let panel = Panel::new().await;
        let session = sign_in(&panel.pool, panel.owner).await;
        let answer = panel
            .call(as_user(
                send(
                    "POST",
                    &format!("/servers/{}/content/upload", panel.server),
                    serde_json::json!({ "file": "nice.jar" }),
                ),
                &session,
            ))
            .await;
        assert_eq!(answer.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
    }

    #[tokio::test]
    async fn the_modpack_endpoints_say_when_there_is_no_pack() {
        let panel = Panel::new().await;
        let session = sign_in(&panel.pool, panel.owner).await;

        let contents = panel
            .call(as_user(
                fetch(&format!("/servers/{}/content/modpack/contents", panel.server)),
                &session,
            ))
            .await;
        assert_eq!(contents.status(), StatusCode::CONFLICT);
        assert_eq!(body_json(contents).await["error"], "modpack_not_linked");

        let unlink = panel
            .call(as_user(
                empty("POST", &format!("/servers/{}/content/modpack/unlink", panel.server)),
                &session,
            ))
            .await;
        assert_eq!(unlink.status(), StatusCode::CONFLICT);
        assert_eq!(body_json(unlink).await["error"], "modpack_not_linked");
    }

    #[tokio::test]
    async fn a_preview_without_a_game_version_is_a_bad_request() {
        let panel = Panel::new().await;
        let session = sign_in(&panel.pool, panel.owner).await;
        let answer = panel
            .call(as_user(
                fetch(&format!("/servers/{}/content/game-version/preview", panel.server)),
                &session,
            ))
            .await;
        assert_eq!(answer.status(), StatusCode::BAD_REQUEST);
        assert_eq!(body_json(answer).await["error"], "invalid_request");
    }

    #[tokio::test]
    async fn an_install_answers_202_with_the_plan_it_is_about_to_carry_out() {
        let panel = Panel::new().await;
        let session = sign_in(&panel.pool, panel.owner).await;
        let body = b"a jar";
        let url = panel.upstream.add_file("thing.jar", body.to_vec());
        let mut version =
            crate::content::harness::a_release("v1", "MOD", "2026-06-01T00:00:00Z", &url, body);
        version.loaders = vec!["fabric".to_owned()];
        panel.upstream.set_versions("MOD", vec![version]);

        let answer = panel
            .call(as_user(
                send(
                    "POST",
                    &format!("/servers/{}/content/install", panel.server),
                    serde_json::json!({
                        "items": [{ "project_id": "MOD", "version_id": null }],
                        "resolve_dependencies": true
                    }),
                ),
                &session,
            ))
            .await;

        assert_eq!(answer.status(), StatusCode::ACCEPTED);
        let json = body_json(answer).await;
        assert_eq!(json["operation"]["kind"], "install_content");
        assert_eq!(json["planned"][0]["project_id"], "MOD");
        assert_eq!(json["planned"][0]["reason"], "requested");
        assert!(json["skipped"].as_array().expect("a list").is_empty());
    }

    #[tokio::test]
    async fn a_server_id_that_is_no_ulid_is_a_404_in_our_own_envelope() {
        let panel = Panel::new().await;
        let session = sign_in(&panel.pool, panel.owner).await;

        for path in ["/servers/not-an-id/content", "/servers/../content"] {
            let answer = panel.call(as_user(fetch(path), &session)).await;
            assert_eq!(answer.status(), StatusCode::NOT_FOUND, "{path}");
            let body = body_json(answer).await;
            assert_eq!(body["error"], "server_not_found", "{path}");
            assert!(body["message"].is_string(), "{path}");
        }
    }

    #[tokio::test]
    async fn an_upload_of_nothing_but_the_wrong_kind_is_a_415() {
        let panel = Panel::new().await;
        let session = sign_in(&panel.pool, panel.owner).await;
        let boundary = "----craftpanelUpload";
        let body = format!(
            "--{boundary}\r\n\
             Content-Disposition: form-data; name=\"file\"; filename=\"pack.mrpack\"\r\n\r\n\
             not for this endpoint\r\n\
             --{boundary}--\r\n"
        );

        let request = Request::builder()
            .method("POST")
            .uri(format!("/servers/{}/content/upload", panel.server))
            .header(header::CONTENT_TYPE, format!("multipart/form-data; boundary={boundary}"))
            .body(Body::from(body))
            .expect("a request");
        let answer = panel.call(as_user(request, &session)).await;
        assert_eq!(answer.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
        assert_eq!(body_json(answer).await["error"], "unsupported_file_type");
        assert!(!panel.root.join("mods").join("pack.mrpack").exists());
    }

    #[tokio::test]
    async fn an_upload_by_an_account_over_its_disk_limit_is_refused_and_lands_nowhere() {
        let panel = Panel::with_disks(Disks::fixed(2048 * MIB, 0)).await;
        panel.limit_of_a_gibibyte().await;
        let session = sign_in(&panel.pool, panel.owner).await;

        let refused =
            panel.call(as_user(panel.an_upload("nice.jar", b"the bytes of a jar"), &session)).await;
        assert_eq!(refused.status(), StatusCode::CONFLICT);
        assert_eq!(body_json(refused).await["error"], "disk_limit_reached");
        assert!(!panel.root.join("mods").join("nice.jar").exists());
        assert!(!panel.root.join(WORK_DIR).exists(), "not even a staging directory");

        let pack = panel
            .call(as_user(
                panel.a_multipart("/modpack/install", "pack.mrpack", b"PK\x03\x04"),
                &session,
            ))
            .await;
        assert_eq!(pack.status(), StatusCode::CONFLICT);
        assert_eq!(body_json(pack).await["error"], "disk_limit_reached");
        assert!(!panel.root.join(WORK_DIR).exists());
    }

    #[tokio::test]
    async fn an_upload_larger_than_the_room_left_is_refused_and_a_smaller_one_is_not() {
        let panel = Panel::with_disks(Disks::fixed(1024 * MIB - 8 * 1024, 0)).await;
        panel.limit_of_a_gibibyte().await;
        let session = sign_in(&panel.pool, panel.owner).await;

        let refused =
            panel.call(as_user(panel.an_upload("big.jar", &[b'x'; 16 * 1024]), &session)).await;
        assert_eq!(refused.status(), StatusCode::CONFLICT);
        assert_eq!(body_json(refused).await["error"], "disk_limit_reached");
        assert!(!panel.root.join("mods").join("big.jar").exists());

        let fits = panel.call(as_user(panel.an_upload("small.jar", &[b'x'; 1024]), &session)).await;
        assert_eq!(fits.status(), StatusCode::OK);
        assert_eq!(body_json(fits).await["results"][0]["ok"], true);
        assert!(panel.root.join("mods").join("small.jar").exists());
    }

    #[tokio::test]
    async fn an_install_that_does_not_fit_is_refused_with_the_size_the_plan_names() {
        let panel = Panel::with_disks(Disks::fixed(1024 * MIB - 8 * 1024, 0)).await;
        panel.limit_of_a_gibibyte().await;
        let session = sign_in(&panel.pool, panel.owner).await;

        let jar = vec![b'x'; 16 * 1024];
        let url = panel.upstream.add_file("thing.jar", jar.clone());
        let version = harness::a_release("v1", "MOD", "2026-06-01T00:00:00Z", &url, &jar);
        panel.upstream.set_versions("MOD", vec![version]);
        let asked = || {
            send(
                "POST",
                &format!("/servers/{}/content/install", panel.server),
                serde_json::json!({ "items": [{ "project_id": "MOD" }] }),
            )
        };

        let refused = panel.call(as_user(asked(), &session)).await;
        assert_eq!(refused.status(), StatusCode::CONFLICT);
        assert_eq!(body_json(refused).await["error"], "disk_limit_reached");

        sqlx::query("UPDATE users SET disk_mib = 4096 WHERE id = ?")
            .bind(panel.owner)
            .execute(&panel.pool)
            .await
            .expect("a wider limit");
        let allowed = panel.call(as_user(asked(), &session)).await;
        assert_eq!(allowed.status(), StatusCode::ACCEPTED, "1 GiB used against 4 GiB is room");
    }

    #[tokio::test]
    async fn an_update_and_a_game_version_change_are_refused_over_the_disk_limit_too() {
        let panel = Panel::with_disks(Disks::fixed(2048 * MIB, 0)).await;
        panel.limit_of_a_gibibyte().await;
        let session = sign_in(&panel.pool, panel.owner).await;

        for (uri, body) in [
            (
                format!("/servers/{}/content/update", panel.server),
                serde_json::json!({ "all": true }),
            ),
            (
                format!("/servers/{}/content/game-version", panel.server),
                serde_json::json!({ "game_version": "1.21.1", "incompatible_content": "disable" }),
            ),
        ] {
            let refused = panel.call(as_user(send("POST", &uri, body), &session)).await;
            assert_eq!(refused.status(), StatusCode::CONFLICT, "{uri}");
            assert_eq!(body_json(refused).await["error"], "disk_limit_reached", "{uri}");
        }
    }

    #[tokio::test]
    async fn the_passthrough_forbids_sniffing_what_it_hands_on() {
        let panel = Panel::new().await;
        let session = sign_in(&panel.pool, panel.owner).await;
        let answer = panel.call(as_user(fetch("/modrinth/v2/search"), &session)).await;
        assert_eq!(answer.status(), StatusCode::OK);
        assert_eq!(
            answer.headers().get("x-content-type-options").and_then(|v| v.to_str().ok()),
            Some("nosniff")
        );
    }

    #[tokio::test]
    async fn switching_the_outside_world_off_shows_up_on_every_endpoint_that_needs_it() {
        let panel = Panel::new().await;
        let session = sign_in(&panel.pool, panel.owner).await;
        sqlx::query("UPDATE panel_settings SET external_services_enabled = 0 WHERE id = 1")
            .execute(&panel.pool)
            .await
            .expect("the switch");

        for (method, uri, body) in [
            ("GET", "/modrinth/v2/search".to_owned(), Body::empty()),
            (
                "GET",
                format!("/servers/{}/content/game-version/preview?game_version=1.21.1", panel.server),
                Body::empty(),
            ),
            (
                "POST",
                format!("/servers/{}/content/game-version", panel.server),
                Body::from(
                    serde_json::json!({
                        "game_version": "1.21.1",
                        "incompatible_content": "disable"
                    })
                    .to_string(),
                ),
            ),
            (
                "POST",
                format!("/servers/{}/content/install", panel.server),
                Body::from(
                    serde_json::json!({ "items": [{ "project_id": "MOD" }] }).to_string(),
                ),
            ),
        ] {
            let request = Request::builder()
                .method(method)
                .uri(&uri)
                .header(header::CONTENT_TYPE, "application/json")
                .body(body)
                .expect("a request");
            let answer = panel.call(as_user(request, &session)).await;
            assert_eq!(answer.status(), StatusCode::CONFLICT, "{method} {uri}");
            assert_eq!(
                body_json(answer).await["error"],
                "external_services_disabled",
                "{method} {uri}"
            );
        }
        assert_eq!(panel.upstream.calls(), 0, "nothing may go out once the switch is off");
    }
}
