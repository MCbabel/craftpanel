use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::header::{
    ACCEPT_RANGES, CACHE_CONTROL, CONTENT_DISPOSITION, CONTENT_LENGTH, CONTENT_RANGE,
    CONTENT_TYPE, ETAG, RANGE,
};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Extension, Json, Router};
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};
use tokio_util::io::ReaderStream;

use crate::auth::disk::{self, Disks};
use crate::auth::error::{Failure, Result};
use crate::auth::{access, Caller, JsonBody, Params};
use crate::files::archive::{self, ExtractDryRunResponse, ExtractRequest};
use crate::files::{
    self, ApiFileItem, FilesMetaResponse, Kind, ListDirectoryResponse, Part, RelPath, Workspace,
};
use crate::model::{Id, OperationAccepted, OperationKind, Permission};
use crate::ops::{NewOperation, Operations};
use crate::AppState;

pub fn router(operations: Arc<Operations>, disks: Disks) -> Router<AppState> {
    Router::new()
        .route("/servers/{server}/files/meta", get(meta))
        .route("/servers/{server}/files/list", get(list))
        .route("/servers/{server}/files/create", post(create))
        .route("/servers/{server}/files/move", post(move_item))
        .route("/servers/{server}/files", delete(remove))
        .route("/servers/{server}/files/content", get(read).put(write))
        .route("/servers/{server}/files/extract", post(extract))
        .layer(Extension(operations))
        .layer(Extension(disks))
        .layer(axum::middleware::from_fn(crate::auth::extract::same_origin))
}

struct OfServer(Id);

impl axum::extract::FromRequestParts<AppState> for OfServer {
    type Rejection = Failure;

    async fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        state: &AppState,
    ) -> Result<Self> {
        let axum::extract::Path(raw) =
            axum::extract::Path::<String>::from_request_parts(parts, state)
                .await
                .map_err(|_| Failure::not_found("server_not_found", "no such server"))?;
        raw.parse().map(Self).map_err(|_| Failure::not_found("server_not_found", "no such server"))
    }
}

#[derive(Debug, Deserialize)]
struct ListQuery {
    path: Option<String>,
    after: Option<String>,
    page_size: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct CreateItemRequest {
    path: String,
    #[serde(rename = "type")]
    kind: NewKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
enum NewKind {
    File,
    Directory,
}

#[derive(Debug, Serialize)]
struct CreateItemResponse {
    item: ApiFileItem,
}

#[derive(Debug, Deserialize)]
struct MoveItemRequest {
    source: String,
    destination: String,
    #[serde(default)]
    overwrite: bool,
}

#[derive(Debug, Serialize)]
struct MoveItemResponse {
    moved: bool,
}

#[derive(Debug, Deserialize)]
struct DeleteQuery {
    path: String,
    #[serde(default)]
    recursive: bool,
}

#[derive(Debug, Deserialize)]
struct ReadQuery {
    path: String,
    max_bytes: Option<u64>,
    #[serde(default)]
    download: Option<u8>,
}

#[derive(Debug, Deserialize)]
struct WriteQuery {
    path: String,
    #[serde(default)]
    on_conflict: Conflict,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Conflict {
    #[default]
    Fail,
    Overwrite,
}

impl Conflict {
    fn may_replace(self) -> bool {
        self == Self::Overwrite
    }
}

async fn meta(
    State(state): State<AppState>,
    caller: Caller,
    OfServer(server): OfServer,
) -> Result<Json<FilesMetaResponse>> {
    let seat = access::require(&state.pool, &caller, server, Permission::BaseRead).await?;
    let settings = crate::auth::settings::load(&state.pool).await?;

    Ok(Json(FilesMetaResponse {
        root_path: files::server_dir(&state.config, seat.owner_id, server)
            .to_string_lossy()
            .into_owned(),
        max_upload_bytes: settings.max_upload_bytes,
        max_text_bytes: files::MAX_TEXT_BYTES,
        max_page_size: files::MAX_PAGE_SIZE,
        default_page_size: files::DEFAULT_PAGE_SIZE,
        max_extract_uncompressed_bytes: files::MAX_EXTRACT_UNCOMPRESSED_BYTES,
        max_extract_entries: files::MAX_EXTRACT_ENTRIES,
    }))
}

async fn list(
    State(state): State<AppState>,
    caller: Caller,
    OfServer(server): OfServer,
    Params(query): Params<ListQuery>,
) -> Result<Json<ListDirectoryResponse>> {
    let (_, workspace) = workspace_for(&state, &caller, server, Permission::BaseRead).await?;
    let at = RelPath::parse(query.path.as_deref().unwrap_or("/"))?;
    let page_size = query.page_size.unwrap_or(files::DEFAULT_PAGE_SIZE).clamp(1, files::MAX_PAGE_SIZE);

    let listed = tokio::task::spawn_blocking(move || {
        files::page(workspace.root(), &at, query.after.as_deref(), page_size)
    })
    .await
    .map_err(joined)??;

    Ok(Json(listed))
}

async fn create(
    State(state): State<AppState>,
    caller: Caller,
    OfServer(server): OfServer,
    Extension(operations): Extension<Arc<Operations>>,
    JsonBody(body): JsonBody<CreateItemRequest>,
) -> Result<(StatusCode, Json<CreateItemResponse>)> {
    let (_, workspace) = writable(&state, &caller, server, &operations).await?;
    let at = RelPath::parse(&body.path)?;
    let name = at.name().ok_or_else(|| Failure::bad_request("invalid_name", "no name given"))?;
    files::path::check_name(name)?;
    let name = name.to_owned();

    let made = {
        let at = at.clone();
        let name = name.clone();
        tokio::task::spawn_blocking(move || {
            let root = workspace.root();
            let dir = root.parent_of(&at).map_err(|err| files::fault(&err, "parent_not_found"))?;
            refuse_lossy(&dir, &name)?;
            match body.kind {
                NewKind::File => dir.create_file(name.as_bytes()).map(drop),
                NewKind::Directory => dir.create_dir(name.as_bytes()),
            }
            .map_err(|err| files::fault(&err, "parent_not_found"))?;

            let meta = dir.meta(name.as_bytes()).map_err(|err| files::fault(&err, "not_found"))?;
            let mut item = ApiFileItem::new(&at, name, meta);
            if body.kind == NewKind::Directory {
                item.count = Some(0);
            }
            Ok::<_, Failure>((item, workspace))
        })
        .await
        .map_err(joined)?
    };
    let (item, workspace) = made?;

    workspace.hand_back(&at).await?;
    Ok((StatusCode::CREATED, Json(CreateItemResponse { item })))
}

async fn move_item(
    State(state): State<AppState>,
    caller: Caller,
    OfServer(server): OfServer,
    Extension(operations): Extension<Arc<Operations>>,
    JsonBody(body): JsonBody<MoveItemRequest>,
) -> Result<Json<MoveItemResponse>> {
    let (seat, workspace) = writable(&state, &caller, server, &operations).await?;
    let source = RelPath::parse(&body.source)?;
    let destination = RelPath::parse(&body.destination)?;

    if source == destination {
        return Ok(Json(MoveItemResponse { moved: true }));
    }
    if source.is_root() || destination.is_root() {
        return Err(invalid_move("the server directory itself cannot be moved"));
    }
    if destination.starts_with(&source) {
        return Err(invalid_move("a directory cannot be moved inside itself"));
    }
    files::path::check_name(destination.name().unwrap_or_default())?;

    let renamed = {
        let (source, destination) = (source.clone(), destination.clone());
        tokio::task::spawn_blocking(move || {
            let root = workspace.root();
            let from = root.parent_of(&source).map_err(|err| files::fault(&err, "not_found"))?;
            let to = root
                .parent_of(&destination)
                .map_err(|err| files::fault(&err, "parent_not_found"))?;
            let here = source.name().unwrap_or_default().as_bytes();
            let there = destination.name().unwrap_or_default().as_bytes();
            refuse_lossy(&from, source.name().unwrap_or_default())?;

            if let Err(err) = from.meta(here) {
                return Err(files::fault(&err, "not_found"));
            }
            match from.rename_to(here, &to, there, body.overwrite) {
                Ok(()) => Ok::<_, Failure>((true, workspace)),
                Err(err) if err.raw_os_error() == Some(libc::EXDEV) => Ok((false, workspace)),
                Err(err) => Err(files::fault(&err, "not_found")),
            }
        })
        .await
        .map_err(joined)?
    };

    let (in_one_step, workspace) = renamed?;
    let workspace = if in_one_step {
        workspace
    } else {
        hand_back_quietly(&workspace, &source).await;
        if body.overwrite && workspace.root().meta(&destination).is_ok() {
            hand_back_quietly(&workspace, &destination).await;
        }
        let (source, destination) = (source.clone(), destination.clone());
        tokio::task::spawn_blocking(move || {
            let root = workspace.root();
            let from = root.parent_of(&source).map_err(|err| files::fault(&err, "not_found"))?;
            let to = root
                .parent_of(&destination)
                .map_err(|err| files::fault(&err, "parent_not_found"))?;
            let here = source.name().unwrap_or_default().as_bytes();
            let there = destination.name().unwrap_or_default().as_bytes();

            if body.overwrite {
                let _ = to.remove_tree(there);
            }
            from.copy_tree(here, &to, there).map_err(|err| files::fault(&err, "not_found"))?;
            from.remove_tree(here).map_err(|err| files::fault(&err, "not_found"))?;
            Ok::<_, Failure>(workspace)
        })
        .await
        .map_err(joined)??
    };

    workspace.hand_back(&destination).await?;
    crate::audit::record(
        &state.pool,
        seat,
        &caller,
        crate::audit::Event::FileRenamed {
            from: source.on_the_wire(),
            to: destination.on_the_wire(),
        },
    )
    .await;
    Ok(Json(MoveItemResponse { moved: true }))
}

async fn remove(
    State(state): State<AppState>,
    caller: Caller,
    OfServer(server): OfServer,
    Extension(operations): Extension<Arc<Operations>>,
    Params(query): Params<DeleteQuery>,
) -> Result<StatusCode> {
    let (seat, workspace) = writable(&state, &caller, server, &operations).await?;
    let at = RelPath::parse(&query.path)?;
    if at.is_root() {
        return Err(Failure::bad_request("invalid_path", "the server directory itself stays"));
    }
    let gone_path = at.on_the_wire();

    if query.recursive
        && matches!(workspace.root().meta(&at), Ok(meta) if meta.kind == Kind::Directory)
    {
        hand_back_quietly(&workspace, &at).await;
    }

    let answer = tokio::task::spawn_blocking(move || {
        let root = workspace.root();
        let dir = match root.parent_of(&at) {
            Ok(dir) => dir,
            Err(err) if gone(&err) => return Ok(StatusCode::NO_CONTENT),
            Err(err) => return Err(files::fault(&err, "not_found")),
        };

        let name = at.name().unwrap_or_default().to_owned();
        let mut raw = name.clone().into_bytes();
        let kind = match dir.meta(&raw) {
            Ok(meta) => meta.kind,
            Err(err) if gone(&err) && files::looks_lossy(&name) => {
                match dir.only_lossy_match(&name) {
                    Ok(Some(real)) => {
                        let kind = dir.meta(&real).map_err(|err| files::fault(&err, "not_found"))?.kind;
                        raw = real;
                        kind
                    }
                    _ => return Ok(StatusCode::NO_CONTENT),
                }
            }
            Err(err) if gone(&err) => return Ok(StatusCode::NO_CONTENT),
            Err(err) => return Err(files::fault(&err, "not_found")),
        };

        let outcome = match (kind, query.recursive) {
            (Kind::Directory, false) => dir.rmdir(&raw),
            (Kind::Directory, true) => dir.remove_tree(&raw),
            _ => dir.unlink(&raw),
        };
        match outcome {
            Ok(()) => Ok(StatusCode::NO_CONTENT),
            Err(err) if gone(&err) => Ok(StatusCode::NO_CONTENT),
            Err(err) => Err(files::fault(&err, "not_found")),
        }
    })
    .await
    .map_err(joined)??;

    crate::audit::record(
        &state.pool,
        seat,
        &caller,
        crate::audit::Event::FileDeleted { path: gone_path },
    )
    .await;
    Ok(answer)
}

async fn read(
    State(state): State<AppState>,
    caller: Caller,
    OfServer(server): OfServer,
    Params(query): Params<ReadQuery>,
    headers: HeaderMap,
) -> Result<Response> {
    let (_, workspace) = workspace_for(&state, &caller, server, Permission::BaseRead).await?;
    let at = RelPath::parse(&query.path)?;
    if at.is_root() {
        return Err(Failure::bad_request("not_a_regular_file", "this is the server directory"));
    }

    let name = at.name().unwrap_or_default().to_owned();
    let opened = {
        let at = at.clone();
        let name = name.clone();
        tokio::task::spawn_blocking(move || {
            let root = workspace.root();
            let file = match root.open_read(&at) {
                Ok(file) => file,
                Err(err)
                    if err.raw_os_error() == Some(libc::ENOENT)
                        && files::looks_lossy(&name) =>
                {
                    let dir = root.parent_of(&at).map_err(|err| files::fault(&err, "not_found"))?;
                    return Err(match dir.only_lossy_match(&name) {
                        Ok(Some(_)) => files::non_utf8_name(),
                        _ => files::fault(&err, "not_found"),
                    });
                }
                Err(err) => return Err(files::fault(&err, "not_found")),
            };

            let meta = file.metadata().map_err(|err| files::fault(&err, "not_found"))?;
            if !meta.is_file() {
                return Err(Failure::bad_request(
                    "not_a_regular_file",
                    "only plain files can be read",
                ));
            }
            Ok::<_, Failure>((file, meta.len(), files::jail::seconds_of(meta.modified())))
        })
        .await
        .map_err(joined)?
    };
    let (file, size, modified) = opened?;

    if query.max_bytes.is_some_and(|ceiling| size > ceiling) {
        return Err(Failure::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            "file_too_large",
            "this file is larger than the caller allowed for",
        ));
    }

    let span = Span::wanted(&headers, size);
    let mut file = tokio::fs::File::from_std(file);
    if span.start > 0 {
        file.seek(std::io::SeekFrom::Start(span.start)).await.map_err(|err| {
            Failure::internal(anyhow::Error::new(err).context("seeking into a file"))
        })?;
    }

    let stream = ReaderStream::new(file.take(span.length));
    let mut response = Response::builder()
        .status(if span.partial { StatusCode::PARTIAL_CONTENT } else { StatusCode::OK })
        .header(CONTENT_TYPE, "application/octet-stream")
        .header("x-content-type-options", "nosniff")
        .header(CONTENT_DISPOSITION, disposition(&name))
        .header(CONTENT_LENGTH, span.length)
        .header(ACCEPT_RANGES, "bytes")
        .header(ETAG, format!("\"{modified}-{size}\""))
        .header(CACHE_CONTROL, "private, no-cache")
        .body(Body::from_stream(stream))
        .map_err(|err| Failure::internal(anyhow::Error::new(err)))?;

    if span.partial {
        let range = format!("bytes {}-{}/{size}", span.start, span.start + span.length - 1);
        if let Ok(value) = HeaderValue::from_str(&range) {
            response.headers_mut().insert(CONTENT_RANGE, value);
        }
    }
    Ok(response)
}

async fn write(
    State(state): State<AppState>,
    caller: Caller,
    OfServer(server): OfServer,
    Extension(operations): Extension<Arc<Operations>>,
    Extension(disks): Extension<Disks>,
    Params(query): Params<WriteQuery>,
    request: Request,
) -> Result<StatusCode> {
    refuse_a_form(request.headers())?;
    let (seat, workspace) = writable(&state, &caller, server, &operations).await?;
    let ceiling = crate::auth::settings::load(&state.pool).await?.max_upload_bytes;

    let at = RelPath::parse(&query.path)?;
    let name = at.name().ok_or_else(|| Failure::bad_request("invalid_name", "no name given"))?;
    files::path::check_name(name)?;
    let name = name.to_owned();

    let announced = announced(request.headers());
    if announced.is_some_and(|length| length > ceiling) {
        return Err(too_large());
    }
    disk::guard(&state.pool, &disks, seat.owner_id, announced.unwrap_or(0)).await?;

    let prepared = {
        let (at, name) = (at.clone(), name.clone());
        tokio::task::spawn_blocking(move || {
            let root = workspace.root();
            let dir = root.parent_of(&at).map_err(|err| files::fault(&err, "parent_not_found"))?;
            refuse_lossy(&dir, &name)?;
            if !query.on_conflict.may_replace() && dir.meta(name.as_bytes()).is_ok() {
                return Err(Failure::conflict("already_exists", "there is already a file there"));
            }
            let (part, file) =
                Part::create(dir, &name).map_err(|err| files::fault(&err, "parent_not_found"))?;
            Ok::<_, Failure>((part, file, workspace))
        })
        .await
        .map_err(joined)?
    };
    let (part, file, workspace) = prepared?;

    let mut sink = tokio::fs::File::from_std(file);
    let mut body = request.into_body().into_data_stream();
    let mut written = 0u64;
    while let Some(chunk) = body.next().await {
        let chunk = chunk.map_err(|err| {
            Failure::invalid_request(format!("the body stopped early: {err}"))
        })?;
        written += chunk.len() as u64;
        if written > ceiling {
            return Err(too_large());
        }
        sink.write_all(&chunk).await.map_err(as_disk_trouble)?;
    }
    sink.sync_all().await.map_err(as_disk_trouble)?;
    drop(sink);

    let replace = query.on_conflict.may_replace();
    let committed = {
        let name = name.clone();
        tokio::task::spawn_blocking(move || {
            part.commit(name.as_bytes(), replace).map_err(|err| files::fault(&err, "not_found"))
        })
        .await
        .map_err(joined)?
    };
    committed?;

    workspace.hand_back(&at).await?;
    let path = at.on_the_wire();
    let event = match replace {
        true => crate::audit::Event::FileEdited { path },
        false => crate::audit::Event::FileUploaded { path },
    };
    crate::audit::record(&state.pool, seat, &caller, event).await;
    Ok(StatusCode::NO_CONTENT)
}

async fn extract(
    State(state): State<AppState>,
    caller: Caller,
    OfServer(server): OfServer,
    Extension(operations): Extension<Arc<Operations>>,
    Extension(disks): Extension<Disks>,
    JsonBody(body): JsonBody<ExtractRequest>,
) -> Result<Response> {
    let (seat, workspace) = writable(&state, &caller, server, &operations).await?;
    let at = RelPath::parse(&body.path)?;
    let target = match body.target.as_deref() {
        None | Some("") => at.parent(),
        Some(given) => RelPath::parse(given)?,
    };

    let looked = {
        let (at, target) = (at.clone(), target.clone());
        tokio::task::spawn_blocking(move || {
            check_target(workspace.root(), &target)?;
            let found = archive::survey(workspace.root(), &at, &target)?;
            Ok::<_, Failure>((found, workspace))
        })
        .await
        .map_err(joined)?
    };
    let (found, workspace) = looked?;

    if body.dry {
        return Ok(Json(ExtractDryRunResponse {
            modpack_name: found.modpack_name,
            conflicting_files: found.conflicting_files,
        })
        .into_response());
    }
    disk::guard(&state.pool, &disks, seat.owner_id, found.uncompressed).await?;

    let mut run = NewOperation::new(server, OperationKind::Unarchive, Some(caller.id()));
    run.src = Some(at.on_the_wire());
    let operation = operations.create(run).await.map_err(|fault| {
        Failure::new(fault.status(), fault.code(), fault.message().to_owned())
    })?;

    archive::start(archive::Job {
        operations: Arc::clone(&operations),
        workspace,
        operation: operation.id,
        archive: at,
        target,
        replace: body.override_existing,
    });

    Ok((StatusCode::ACCEPTED, Json(OperationAccepted { operation })).into_response())
}

fn check_target(root: &crate::files::Root, target: &RelPath) -> Result<()> {
    match root.dir(target) {
        Ok(_) => Ok(()),
        Err(err) if err.raw_os_error() == Some(libc::ENOENT) => Ok(()),
        Err(err) if err.raw_os_error() == Some(libc::ENOTDIR) => {
            Err(Failure::bad_request("not_a_directory", "the target is not a directory"))
        }
        Err(err) => Err(files::fault(&err, "not_found")),
    }
}

async fn workspace_for(
    state: &AppState,
    caller: &Caller,
    server: Id,
    permission: Permission,
) -> Result<(access::Access, Workspace)> {
    let seat = access::require(&state.pool, caller, server, permission).await?;
    Ok((seat, Workspace::open(&state.config, seat.owner_id, server)?))
}

async fn writable(
    state: &AppState,
    caller: &Caller,
    server: Id,
    operations: &Operations,
) -> Result<(access::Access, Workspace)> {
    let seat = workspace_for(state, caller, server, Permission::FilesWrite).await?;
    operations
        .guard_write(server)
        .await
        .map_err(|fault| Failure::new(fault.status(), fault.code(), fault.message().to_owned()))?;
    Ok(seat)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Span {
    start: u64,
    length: u64,
    partial: bool,
}

impl Span {
    fn whole(size: u64) -> Self {
        Self { start: 0, length: size, partial: false }
    }

    fn wanted(headers: &HeaderMap, size: u64) -> Self {
        let Some(asked) = headers.get(RANGE).and_then(|value| value.to_str().ok()) else {
            return Self::whole(size);
        };
        let Some(one) = asked.strip_prefix("bytes=") else {
            return Self::whole(size);
        };
        if one.contains(',') {
            return Self::whole(size);
        }

        let (from, to) = match one.split_once('-') {
            Some(halves) => halves,
            None => return Self::whole(size),
        };
        let (start, end) = match (from.trim().parse::<u64>(), to.trim().parse::<u64>()) {
            (Ok(start), Ok(end)) => (start, end.min(size.saturating_sub(1))),
            (Ok(start), Err(_)) if !from.trim().is_empty() => (start, size.saturating_sub(1)),
            (Err(_), Ok(last)) => (size.saturating_sub(last), size.saturating_sub(1)),
            _ => return Self::whole(size),
        };
        if size == 0 || start > end || start >= size {
            return Self::whole(size);
        }

        Self { start, length: end - start + 1, partial: true }
    }
}

fn disposition(name: &str) -> String {
    let plain: String = name
        .chars()
        .map(|letter| if letter.is_ascii_alphanumeric() || "-._ ".contains(letter) { letter } else { '_' })
        .collect();
    let escaped: String = name
        .bytes()
        .map(|byte| {
            if byte.is_ascii_alphanumeric() || b"-._~".contains(&byte) {
                (byte as char).to_string()
            } else {
                format!("%{byte:02X}")
            }
        })
        .collect();
    format!("attachment; filename=\"{plain}\"; filename*=UTF-8''{escaped}")
}

fn announced(headers: &HeaderMap) -> Option<u64> {
    headers.get(CONTENT_LENGTH)?.to_str().ok()?.parse().ok()
}

fn refuse_a_form(headers: &HeaderMap) -> Result<()> {
    let kind = headers
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .map(str::trim)
        .unwrap_or_default()
        .to_ascii_lowercase();

    if kind == "multipart/form-data" || kind == "application/x-www-form-urlencoded" {
        return Err(Failure::new(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "unsupported_media_type",
            "this endpoint reads the raw bytes, not a form",
        ));
    }
    Ok(())
}

fn too_large() -> Failure {
    Failure::new(
        StatusCode::PAYLOAD_TOO_LARGE,
        "file_too_large",
        "this file is larger than the panel accepts",
    )
}

fn invalid_move(why: &'static str) -> Failure {
    Failure::bad_request("invalid_move", why)
}

fn refuse_lossy(dir: &crate::files::Dir, name: &str) -> Result<()> {
    if files::looks_lossy(name) && dir.only_lossy_match(name).ok().flatten().is_some() {
        return Err(files::non_utf8_name());
    }
    Ok(())
}

fn gone(err: &std::io::Error) -> bool {
    matches!(err.raw_os_error(), Some(libc::ENOENT) | Some(libc::ENOTDIR))
}

async fn hand_back_quietly(workspace: &files::Workspace, at: &RelPath) {
    if let Err(err) = workspace.hand_back(at).await {
        tracing::warn!("{} was not handed back before the panel took it on: {err}", at.on_the_wire());
    }
}

fn as_disk_trouble(err: std::io::Error) -> Failure {
    files::fault(&err, "not_found")
}

fn joined(err: tokio::task::JoinError) -> Failure {
    Failure::internal(anyhow::Error::new(err).context("a file task died"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::harness::{a_user, an_admin, sign_in, state_with, test_pool, FakeHelper};
    use crate::config::Config;
    use crate::files::testing::Sandbox;
    use crate::model::ServerRole;
    use axum::http::header::COOKIE;
    use craftpanel_proto::HelperRequest;
    use sqlx::SqlitePool;
    use std::os::unix::ffi::OsStringExt;
    use tower::ServiceExt;

    struct Panel {
        app: Router,
        pool: SqlitePool,
        sandbox: Sandbox,
        helper: FakeHelper,
        cookie: String,
        server: Id,
        operations: Arc<Operations>,
    }

    async fn panel() -> Panel {
        panel_with(Disks::none()).await
    }

    async fn panel_with(disks: Disks) -> Panel {
        let pool = test_pool().await;
        let sandbox = Sandbox::new();
        let helper = FakeHelper::obliging().await.rooted_at(sandbox.data_dir().join("users"));

        sqlx::query("INSERT INTO users (id, username, password_hash, role, system_uid, system_state, memory_mib, cpu_mode, cpu_cores, pids_max, created_at, updated_at) VALUES (?, 'max', 'argon2', 'user', 6100, 'ready', 4096, 'cap', 2.0, 512, ?, ?)")
            .bind(sandbox.owner)
            .bind(crate::model::Timestamp::now())
            .bind(crate::model::Timestamp::now())
            .execute(&pool)
            .await
            .expect("the owner");
        sqlx::query("INSERT INTO servers (id, name, owner_id, status, memory_mib, created_at, updated_at) VALUES (?, 'Survival', ?, 'available', 2048, ?, ?)")
            .bind(sandbox.server)
            .bind(sandbox.owner)
            .bind(crate::model::Timestamp::now())
            .bind(crate::model::Timestamp::now())
            .execute(&pool)
            .await
            .expect("the server");

        let cookie = sign_in(&pool, sandbox.owner).await;
        let config =
            Config { helper_socket: helper.socket(), ..sandbox.config() };
        let operations = Operations::new(pool.clone(), config.data_dir.clone());
        let app =
            router(Arc::clone(&operations), disks).with_state(state_with(&pool, config));

        Panel {
            app,
            pool,
            server: sandbox.server,
            sandbox,
            helper,
            cookie,
            operations,
        }
    }

    impl Panel {
        fn url(&self, tail: &str) -> String {
            format!("/servers/{}/files{tail}", self.server)
        }

        async fn send(&self, request: Request<Body>) -> Response {
            self.app.clone().oneshot(request).await.expect("an answer")
        }

        async fn as_owner(&self, request: axum::http::request::Builder) -> Response {
            let request = request
                .header(COOKIE, format!("craft_session={}", self.cookie))
                .body(Body::empty())
                .expect("a request");
            self.send(request).await
        }

        async fn get(&self, tail: &str) -> Response {
            self.as_owner(Request::builder().uri(self.url(tail))).await
        }

        async fn post(&self, tail: &str, body: serde_json::Value) -> Response {
            let request = Request::builder()
                .method("POST")
                .uri(self.url(tail))
                .header(CONTENT_TYPE, "application/json")
                .header(COOKIE, format!("craft_session={}", self.cookie))
                .body(Body::from(body.to_string()))
                .expect("a request");
            self.send(request).await
        }

        async fn put(&self, tail: &str, bytes: &'static [u8]) -> Response {
            let request = Request::builder()
                .method("PUT")
                .uri(self.url(tail))
                .header(CONTENT_TYPE, "application/octet-stream")
                .header(COOKIE, format!("craft_session={}", self.cookie))
                .body(Body::from(bytes))
                .expect("a request");
            self.send(request).await
        }

        async fn put_announcing(&self, tail: &str, bytes: &'static [u8]) -> Response {
            let request = Request::builder()
                .method("PUT")
                .uri(self.url(tail))
                .header(CONTENT_TYPE, "application/octet-stream")
                .header(CONTENT_LENGTH, bytes.len())
                .header(COOKIE, format!("craft_session={}", self.cookie))
                .body(Body::from(bytes))
                .expect("a request");
            self.send(request).await
        }

        async fn delete(&self, tail: &str) -> Response {
            let request = Request::builder()
                .method("DELETE")
                .uri(self.url(tail))
                .header(COOKIE, format!("craft_session={}", self.cookie))
                .body(Body::empty())
                .expect("a request");
            self.send(request).await
        }

        fn chowned(&self) -> Vec<String> {
            self.helper
                .calls()
                .into_iter()
                .filter_map(|call| match call {
                    HelperRequest::ChownTree { steps, .. } => Some(steps.join("/")),
                    _ => None,
                })
                .collect()
        }
    }

    #[tokio::test]
    async fn an_upload_over_the_disk_limit_is_refused_and_leaves_nothing_behind() {
        const MIB: u64 = 1024 * 1024;
        let panel = panel_with(Disks::fixed(1000 * MIB, 0)).await;
        sqlx::query("UPDATE users SET disk_mib = 1024 WHERE id = ?")
            .bind(panel.sandbox.owner)
            .execute(&panel.pool)
            .await
            .unwrap();

        let too_much: &'static [u8] = &[b'x'; 32 * 1024 * 1024];
        let refused = panel.put_announcing("/content?path=/big.dat", too_much).await;
        assert_eq!(refused.status(), StatusCode::CONFLICT);
        assert_eq!(body_json(refused).await["error"], "disk_limit_reached");

        let listed: Vec<String> = std::fs::read_dir(panel.sandbox.server_dir())
            .expect("the server directory")
            .filter_map(|entry| Some(entry.ok()?.file_name().to_string_lossy().into_owned()))
            .collect();
        assert!(listed.is_empty(), "not even a half-written part file: {listed:?}");

        let fits: &'static [u8] = &[b'x'; 1024];
        let allowed = panel.put_announcing("/content?path=/small.dat", fits).await;
        assert_eq!(allowed.status(), StatusCode::NO_CONTENT);
        assert!(panel.sandbox.server_dir().join("small.dat").exists());
    }

    async fn body_json(response: Response) -> serde_json::Value {
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.expect("a body");
        serde_json::from_slice(&bytes).expect("json")
    }

    async fn body_bytes(response: Response) -> Vec<u8> {
        axum::body::to_bytes(response.into_body(), usize::MAX).await.expect("a body").to_vec()
    }

    #[tokio::test]
    async fn meta_answers_the_real_path_and_the_ceilings() {
        let panel = panel().await;
        let answer = panel.get("/meta").await;
        assert_eq!(answer.status(), StatusCode::OK);

        let body = body_json(answer).await;
        assert_eq!(body["root_path"], panel.sandbox.server_dir().to_string_lossy().as_ref());
        assert_eq!(body["max_text_bytes"], 8 * 1024 * 1024);
        assert_eq!(body["default_page_size"], 1000);
        assert_eq!(body["max_upload_bytes"], 4 * 1024 * 1024 * 1024u64);
    }

    #[tokio::test]
    async fn listing_pages_and_answers_paths_with_a_leading_slash() {
        let panel = panel().await;
        panel.sandbox.write("plugins/a.jar", b"a");
        panel.sandbox.write("plugins/b.jar", b"b");

        let body = body_json(panel.get("/list?path=plugins&page_size=1").await).await;
        assert_eq!(body["items"][0]["path"], "/plugins/a.jar", "prefetchFile files it under this");
        assert_eq!(body["has_more"], true);
        assert_eq!(body["next_after"], "a.jar");

        let second = body_json(panel.get("/list?path=/plugins&after=a.jar").await).await;
        assert_eq!(second["items"][0]["name"], "b.jar");
        assert_eq!(second["has_more"], false);
    }

    #[tokio::test]
    async fn a_path_that_climbs_is_refused_before_anything_is_opened() {
        let panel = panel().await;
        let refused = panel.get("/list?path=../../..").await;
        assert_eq!(refused.status(), StatusCode::BAD_REQUEST);
        assert_eq!(body_json(refused).await["error"], "invalid_path");

        let content = panel.get("/content?path=/plugins/../../panel.db").await;
        assert_eq!(body_json(content).await["error"], "invalid_path");
    }

    #[tokio::test]
    async fn a_link_out_of_the_tree_is_forbidden_however_it_is_reached() {
        let panel = panel().await;
        let secret = panel.sandbox.data_dir().join("panel.db");
        std::fs::write(&secret, b"password hashes").expect("the database");
        std::os::unix::fs::symlink(&secret, panel.sandbox.server_dir().join("latest.log"))
            .expect("the link a plugin could lay");
        panel.sandbox.mkdir("logs");
        std::os::unix::fs::symlink("/etc", panel.sandbox.server_dir().join("logs/etc"))
            .expect("a second link");

        let read = panel.get("/content?path=/latest.log").await;
        assert_eq!(read.status(), StatusCode::FORBIDDEN, "this is the download of the panel db");
        assert_eq!(body_json(read).await["error"], "forbidden_path");

        let deeper = panel.get("/content?path=/logs/etc/passwd").await;
        assert_eq!(body_json(deeper).await["error"], "forbidden_path");

        let listed = panel.get("/list?path=/logs/etc").await;
        assert_eq!(body_json(listed).await["error"], "forbidden_path");
    }

    #[tokio::test]
    async fn no_endpoint_writes_through_a_directory_link_that_leaves_the_tree() {
        let panel = panel().await;
        let outside = panel.sandbox.data_dir().join("elsewhere");
        std::fs::create_dir_all(&outside).expect("somewhere outside");
        std::fs::write(outside.join("panel.db"), b"password hashes").expect("the database");
        std::os::unix::fs::symlink(&outside, panel.sandbox.server_dir().join("out"))
            .expect("the link a plugin could lay");

        let created = panel
            .post("/create", serde_json::json!({ "path": "/out/mine.txt", "type": "file" }))
            .await;
        assert_eq!(body_json(created).await["error"], "forbidden_path", "7.4 through a link");

        let written = panel.put("/content?path=/out/mine.txt", b"mine").await;
        assert_eq!(body_json(written).await["error"], "forbidden_path", "7.8 through a link");

        panel.sandbox.write("here.txt", b"here");
        let moved = panel
            .post("/move", serde_json::json!({ "source": "/here.txt", "destination": "/out/there.txt" }))
            .await;
        assert_eq!(body_json(moved).await["error"], "forbidden_path", "7.5 through a link");

        let deleted = panel.delete("?path=/out/panel.db").await;
        assert_eq!(body_json(deleted).await["error"], "forbidden_path", "7.6 through a link");

        assert_eq!(
            std::fs::read(outside.join("panel.db")).expect("still there"),
            b"password hashes",
            "nothing outside the server directory may be touched"
        );
        assert!(!outside.join("mine.txt").exists());
        assert!(!outside.join("there.txt").exists());
        assert!(panel.sandbox.server_dir().join("here.txt").exists(), "and the source stays put");
    }

    #[tokio::test]
    async fn a_twice_encoded_climb_is_a_filename_and_not_a_climb() {
        let panel = panel().await;
        let secret = panel.sandbox.data_dir().join("panel.db");
        std::fs::write(&secret, b"password hashes").expect("the database");

        let read = panel.get("/content?path=%252e%252e%252fpanel.db").await;
        assert_eq!(
            body_json(read).await["error"],
            "not_found",
            "the once-decoded name is `%2e%2e%2fpanel.db`, an ordinary filename that is not there"
        );
        assert_eq!(std::fs::read(&secret).expect("still there"), b"password hashes");

        let climbed = panel.get("/content?path=%2e%2e%2fpanel.db").await;
        assert_eq!(body_json(climbed).await["error"], "invalid_path", "the once-encoded one is N5");
    }

    #[tokio::test]
    async fn unpacking_cannot_be_aimed_out_of_the_tree() {
        let panel = panel().await;
        let outside = panel.sandbox.data_dir().join("elsewhere");
        std::fs::create_dir_all(&outside).expect("somewhere outside");
        std::os::unix::fs::symlink(&outside, panel.sandbox.server_dir().join("out")).unwrap();

        let mut buffer = std::io::Cursor::new(Vec::new());
        {
            let mut writer = zip::ZipWriter::new(&mut buffer);
            let options = zip::write::SimpleFileOptions::default();
            writer.start_file("owned.txt", options).expect("an entry");
            std::io::Write::write_all(&mut writer, b"owned").expect("the bytes");
            writer.finish().expect("the archive");
        }
        panel.sandbox.write("pack.zip", &buffer.into_inner());

        let aimed = panel
            .post(
                "/extract",
                serde_json::json!({ "path": "/pack.zip", "target": "/out", "override": true, "dry": false }),
            )
            .await;
        assert_eq!(aimed.status(), StatusCode::FORBIDDEN, "no run may start with a target outside");
        assert_eq!(body_json(aimed).await["error"], "forbidden_path");

        let pretended = panel
            .post(
                "/extract",
                serde_json::json!({ "path": "/pack.zip", "target": "/out", "override": true, "dry": true }),
            )
            .await;
        assert_eq!(body_json(pretended).await["error"], "forbidden_path");

        let climbed = panel
            .post(
                "/extract",
                serde_json::json!({ "path": "/pack.zip", "target": "/../..", "override": true, "dry": true }),
            )
            .await;
        assert_eq!(body_json(climbed).await["error"], "invalid_path");

        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        assert!(!outside.join("owned.txt").exists(), "nothing landed outside the server tree");
    }

    #[tokio::test]
    async fn the_server_directory_itself_cannot_be_a_link_somebody_laid() {
        let panel = panel().await;
        let secret = panel.sandbox.data_dir().join("panel.db");
        std::fs::write(&secret, b"password hashes").expect("the database");

        let real = panel.sandbox.server_dir();
        std::fs::remove_dir_all(&real).expect("the directory the owner may move");
        std::os::unix::fs::symlink(panel.sandbox.data_dir(), &real).expect("the link");

        let read = panel.get("/content?path=/panel.db").await;
        assert_ne!(read.status(), StatusCode::OK, "this is the panel database");
        assert_eq!(body_json(read).await["error"], "forbidden_path");

        let listed = panel.get("/list").await;
        assert_ne!(listed.status(), StatusCode::OK, "nor may its neighbours be listed");
        assert_eq!(body_json(listed).await["error"], "forbidden_path");

        let written = panel.put("/content?path=/panel.db", b"mine now").await;
        assert_eq!(body_json(written).await["error"], "forbidden_path");
        assert_eq!(std::fs::read(&secret).expect("still there"), b"password hashes");

        let neighbour = Id::new();
        let stranger =
            panel.sandbox.data_dir().join("users").join(neighbour.to_string()).join("servers");
        std::fs::create_dir_all(&stranger).expect("somebody else's tree");
        std::fs::write(stranger.join("their-world.txt"), b"not yours").expect("their file");
        std::fs::remove_file(&real).expect("the first link goes");
        std::os::unix::fs::symlink(format!("../../{neighbour}/servers"), &real)
            .expect("the second link");

        let theirs = panel.get("/list").await;
        assert_ne!(theirs.status(), StatusCode::OK, "a stranger's tree is not this server");
        assert_eq!(body_json(theirs).await["error"], "forbidden_path");
    }

    #[tokio::test]
    async fn deleting_a_link_leaves_the_file_it_points_at() {
        let panel = panel().await;
        let outside = panel.sandbox.data_dir().join("keep.db");
        std::fs::write(&outside, b"keep me").expect("a file outside");
        std::os::unix::fs::symlink(&outside, panel.sandbox.server_dir().join("bait")).unwrap();

        assert_eq!(panel.delete("?path=/bait").await.status(), StatusCode::NO_CONTENT);
        assert!(!panel.sandbox.server_dir().join("bait").is_symlink());
        assert_eq!(std::fs::read(&outside).expect("still there"), b"keep me");
    }

    #[tokio::test]
    async fn a_name_that_is_not_utf8_can_only_be_deleted() {
        let panel = panel().await;
        let broken = std::ffi::OsString::from_vec(vec![b'm', 0xff, b'd', b'.', b'j', b'a', b'r']);
        std::fs::write(panel.sandbox.server_dir().join(&broken), b"x").expect("the file");

        let listed = body_json(panel.get("/list").await).await;
        let shown = listed["items"][0]["name"].as_str().expect("a name").to_owned();
        assert!(shown.contains('\u{fffd}'), "the user has to be able to see it: {shown}");
        let escaped = urlencoding(&shown);

        let read = panel.get(&format!("/content?path=/{escaped}")).await;
        assert_eq!(body_json(read).await["error"], "non_utf8_name");

        let written = panel.put(&format!("/content?path=/{escaped}"), b"mine").await;
        assert_eq!(body_json(written).await["error"], "non_utf8_name");

        assert_eq!(
            panel.delete(&format!("?path=/{escaped}")).await.status(),
            StatusCode::NO_CONTENT
        );
        assert!(!panel.sandbox.server_dir().join(&broken).exists(), "the rubbish goes");
    }

    fn urlencoding(raw: &str) -> String {
        raw.bytes()
            .map(|byte| {
                if byte.is_ascii_alphanumeric() || b"-._~".contains(&byte) {
                    (byte as char).to_string()
                } else {
                    format!("%{byte:02X}")
                }
            })
            .collect()
    }

    #[tokio::test]
    async fn creating_answers_the_finished_item_and_hands_it_back_to_the_owner() {
        let panel = panel().await;
        panel.sandbox.mkdir("plugins");

        let made = panel
            .post("/create", serde_json::json!({ "path": "/plugins/new.yml", "type": "file" }))
            .await;
        assert_eq!(made.status(), StatusCode::CREATED);

        let body = body_json(made).await;
        assert_eq!(body["item"]["name"], "new.yml");
        assert_eq!(body["item"]["type"], "file");
        assert_eq!(body["item"]["path"], "/plugins/new.yml");
        assert_eq!(body["item"]["size"], 0);
        assert!(panel.sandbox.server_dir().join("plugins/new.yml").exists());

        assert!(
            panel.chowned().iter().any(|path| path.ends_with("plugins/new.yml")),
            "PLAN.md:205 — without chown_tree the game process cannot read what it just got: {:?}",
            panel.chowned()
        );

        let folder = panel
            .post("/create", serde_json::json!({ "path": "/plugins/fresh", "type": "directory" }))
            .await;
        let body = body_json(folder).await;
        assert_eq!(body["item"]["type"], "directory");
        assert_eq!(body["item"]["count"], 0);
        assert!(body["item"].get("size").is_none(), "and no size, because it is not a file");
    }

    #[tokio::test]
    async fn creating_twice_and_creating_without_a_parent() {
        let panel = panel().await;
        let body = serde_json::json!({ "path": "/plugins/new.yml", "type": "file" });

        let missing = panel.post("/create", body.clone()).await;
        assert_eq!(missing.status(), StatusCode::NOT_FOUND);
        assert_eq!(body_json(missing).await["error"], "parent_not_found");

        panel.sandbox.mkdir("plugins");
        assert_eq!(panel.post("/create", body.clone()).await.status(), StatusCode::CREATED);
        let again = panel.post("/create", body).await;
        assert_eq!(again.status(), StatusCode::CONFLICT);
        assert_eq!(body_json(again).await["error"], "already_exists");

        let named = panel
            .post("/create", serde_json::json!({ "path": "/plugins/..", "type": "file" }))
            .await;
        assert_eq!(body_json(named).await["error"], "invalid_path");
    }

    #[tokio::test]
    async fn moving_serves_renaming_and_refuses_moving_into_itself() {
        let panel = panel().await;
        panel.sandbox.write("plugins/a.jar", b"a");

        let renamed = panel
            .post(
                "/move",
                serde_json::json!({ "source": "/plugins/a.jar", "destination": "/plugins/b.jar" }),
            )
            .await;
        assert_eq!(renamed.status(), StatusCode::OK);
        assert_eq!(body_json(renamed).await["moved"], true);
        assert!(panel.sandbox.server_dir().join("plugins/b.jar").exists());
        assert!(
            panel.chowned().iter().any(|path| path.ends_with("plugins/b.jar")),
            "a move is a write and ends in chown_tree: {:?}",
            panel.chowned()
        );

        let inside = panel
            .post(
                "/move",
                serde_json::json!({ "source": "/plugins", "destination": "/plugins/deeper" }),
            )
            .await;
        assert_eq!(body_json(inside).await["error"], "invalid_move");

        let nowhere = panel
            .post("/move", serde_json::json!({ "source": "/nope", "destination": "/plugins/x" }))
            .await;
        assert_eq!(body_json(nowhere).await["error"], "not_found");

        let same = panel
            .post(
                "/move",
                serde_json::json!({ "source": "/plugins/b.jar", "destination": "/plugins/b.jar" }),
            )
            .await;
        assert_eq!(same.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn moving_onto_something_needs_overwrite() {
        let panel = panel().await;
        panel.sandbox.write("a.txt", b"a");
        panel.sandbox.write("b.txt", b"b");

        let refused = panel
            .post("/move", serde_json::json!({ "source": "/a.txt", "destination": "/b.txt" }))
            .await;
        assert_eq!(refused.status(), StatusCode::CONFLICT);
        assert_eq!(body_json(refused).await["error"], "already_exists");
        assert_eq!(std::fs::read(panel.sandbox.server_dir().join("b.txt")).unwrap(), b"b");

        let allowed = panel
            .post(
                "/move",
                serde_json::json!({ "source": "/a.txt", "destination": "/b.txt", "overwrite": true }),
            )
            .await;
        assert_eq!(allowed.status(), StatusCode::OK);
        assert_eq!(std::fs::read(panel.sandbox.server_dir().join("b.txt")).unwrap(), b"a");
    }

    #[tokio::test]
    async fn deleting_is_idempotent_and_a_full_directory_needs_recursive() {
        let panel = panel().await;
        panel.sandbox.write("world/region/r.0.0.mca", b"chunks");

        let refused = panel.delete("?path=/world").await;
        assert_eq!(refused.status(), StatusCode::CONFLICT);
        assert_eq!(body_json(refused).await["error"], "not_empty");

        assert_eq!(panel.delete("?path=/world&recursive=true").await.status(), StatusCode::NO_CONTENT);
        assert!(!panel.sandbox.server_dir().join("world").exists());

        assert_eq!(panel.delete("?path=/world&recursive=true").await.status(), StatusCode::NO_CONTENT);
        assert_eq!(panel.delete("?path=/never-was").await.status(), StatusCode::NO_CONTENT);
    }

    #[tokio::test]
    async fn a_recursive_delete_hands_the_tree_back_before_it_walks_into_it() {
        let panel = panel().await;
        panel.sandbox.write("plugins/WorldEdit/lang/strings.json", b"{}");
        panel.sandbox.write("plugins/config.yml", b"x");

        assert_eq!(
            panel.delete("?path=/plugins/WorldEdit&recursive=true").await.status(),
            StatusCode::NO_CONTENT
        );
        assert!(!panel.sandbox.server_dir().join("plugins/WorldEdit").exists());
        assert!(
            panel.chowned().iter().any(|path| path.ends_with("plugins/WorldEdit")),
            "without chown_tree the removal stops at the first directory the game shut: {:?}",
            panel.chowned()
        );

        let before = panel.chowned().len();
        assert_eq!(
            panel.delete("?path=/plugins/config.yml").await.status(),
            StatusCode::NO_CONTENT
        );
        assert_eq!(panel.chowned().len(), before, "one call per tree, none for a file");
    }

    #[tokio::test]
    async fn a_tree_nested_deeper_than_the_walk_goes_is_not_called_an_escape() {
        let panel = panel().await;
        let mut deep = panel.sandbox.server_dir().join("world");
        for _ in 0..300 {
            deep = deep.join("a");
        }
        std::fs::create_dir_all(&deep).expect("a tree only the game process could make");

        let refused = panel.delete("?path=/world&recursive=true").await;
        assert_eq!(refused.status(), StatusCode::BAD_REQUEST);
        assert_eq!(body_json(refused).await["error"], "path_too_long");
    }

    #[tokio::test]
    async fn reading_carries_the_headers_that_keep_an_upload_from_becoming_xss() {
        let panel = panel().await;
        panel.sandbox.write("evil.html", b"<script>alert(1)</script>");

        let answer = panel.get("/content?path=/evil.html").await;
        assert_eq!(answer.status(), StatusCode::OK);
        let headers = answer.headers().clone();
        assert_eq!(headers[CONTENT_TYPE], "application/octet-stream");
        assert_eq!(headers["x-content-type-options"], "nosniff");
        assert!(headers[CONTENT_DISPOSITION].to_str().unwrap().starts_with("attachment;"));
        assert_eq!(headers[ACCEPT_RANGES], "bytes");
        assert!(headers.contains_key(ETAG));
        assert_eq!(body_bytes(answer).await, b"<script>alert(1)</script>");
    }

    #[tokio::test]
    async fn a_file_over_max_bytes_is_refused_without_a_body() {
        let panel = panel().await;
        panel.sandbox.write("logs/latest.log", &vec![b'x'; 4096]);

        let refused = panel.get("/content?path=/logs/latest.log&max_bytes=1024").await;
        assert_eq!(refused.status(), StatusCode::PAYLOAD_TOO_LARGE);
        assert_eq!(body_json(refused).await["error"], "file_too_large");

        let allowed = panel.get("/content?path=/logs/latest.log&max_bytes=8192").await;
        assert_eq!(allowed.status(), StatusCode::OK);
        assert_eq!(body_bytes(allowed).await.len(), 4096);
    }

    #[tokio::test]
    async fn a_range_is_served_so_a_broken_download_can_go_on() {
        let panel = panel().await;
        panel.sandbox.write("world.zip", b"0123456789");

        let request = Request::builder()
            .uri(panel.url("/content?path=/world.zip"))
            .header(RANGE, "bytes=4-6")
            .header(COOKIE, format!("craft_session={}", panel.cookie))
            .body(Body::empty())
            .expect("a request");
        let answer = panel.send(request).await;

        assert_eq!(answer.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(answer.headers()[CONTENT_RANGE], "bytes 4-6/10");
        assert_eq!(body_bytes(answer).await, b"456");
    }

    #[tokio::test]
    async fn a_fifo_is_not_a_file_and_never_blocks_the_request() {
        let panel = panel().await;
        let fifo = panel.sandbox.server_dir().join("pipe");
        let name = std::ffi::CString::new(fifo.to_string_lossy().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o660) }, 0, "the test needs a fifo");

        let answer =
            tokio::time::timeout(std::time::Duration::from_secs(5), panel.get("/content?path=/pipe"))
                .await
                .expect("the request must not hang on the open");
        assert_eq!(answer.status(), StatusCode::BAD_REQUEST);
        assert_eq!(body_json(answer).await["error"], "not_a_regular_file");
    }

    #[tokio::test]
    async fn writing_is_atomic_conflicts_are_named_and_the_file_is_handed_back() {
        let panel = panel().await;
        panel.sandbox.mkdir("config");

        let written = panel.put("/content?path=/config/one.yml", b"a: 1\n").await;
        assert_eq!(written.status(), StatusCode::NO_CONTENT);
        assert_eq!(
            std::fs::read_to_string(panel.sandbox.server_dir().join("config/one.yml")).unwrap(),
            "a: 1\n"
        );
        assert!(
            panel.chowned().iter().any(|path| path.ends_with("config/one.yml")),
            "every write is followed by chown_tree: {:?}",
            panel.chowned()
        );

        let again = panel.put("/content?path=/config/one.yml", b"a: 2\n").await;
        assert_eq!(again.status(), StatusCode::CONFLICT);
        assert_eq!(body_json(again).await["error"], "already_exists");

        let editor = panel.put("/content?path=/config/one.yml&on_conflict=overwrite", b"a: 3\n").await;
        assert_eq!(editor.status(), StatusCode::NO_CONTENT);
        assert_eq!(
            std::fs::read_to_string(panel.sandbox.server_dir().join("config/one.yml")).unwrap(),
            "a: 3\n"
        );

        let leftovers = std::fs::read_dir(panel.sandbox.server_dir().join("config"))
            .expect("the directory")
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_name().to_string_lossy().contains(".part."))
            .count();
        assert_eq!(leftovers, 0, "the part file is renamed, never left lying");
    }

    #[tokio::test]
    async fn writing_past_the_upload_ceiling_is_refused() {
        let panel = panel().await;
        sqlx::query("UPDATE panel_settings SET max_upload_bytes = 8 WHERE id = 1")
            .execute(&panel.pool)
            .await
            .expect("a small ceiling");

        let refused = panel.put("/content?path=/big.bin", b"more than eight bytes").await;
        assert_eq!(refused.status(), StatusCode::PAYLOAD_TOO_LARGE);
        assert_eq!(body_json(refused).await["error"], "file_too_large");
        assert!(!panel.sandbox.server_dir().join("big.bin").exists());

        let left: Vec<_> = std::fs::read_dir(panel.sandbox.server_dir())
            .expect("the directory")
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        assert!(left.is_empty(), "a refused upload leaves nothing behind: {left:?}");
    }

    #[tokio::test]
    async fn a_form_at_the_byte_endpoint_is_the_wrong_media_type() {
        let panel = panel().await;
        let request = Request::builder()
            .method("PUT")
            .uri(panel.url("/content?path=/a.txt"))
            .header(CONTENT_TYPE, "multipart/form-data; boundary=x")
            .header(COOKIE, format!("craft_session={}", panel.cookie))
            .body(Body::from("--x--"))
            .expect("a request");

        let refused = panel.send(request).await;
        assert_eq!(refused.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
        assert_eq!(body_json(refused).await["error"], "unsupported_media_type");
    }

    #[tokio::test]
    async fn a_dry_run_answers_at_once_and_starts_no_run() {
        let panel = panel().await;
        let mut buffer = std::io::Cursor::new(Vec::new());
        {
            let mut writer = zip::ZipWriter::new(&mut buffer);
            let options = zip::write::SimpleFileOptions::default();
            writer.start_file("config/one.yml", options).expect("an entry");
            std::io::Write::write_all(&mut writer, b"from the pack").expect("the bytes");
            writer.finish().expect("the archive");
        }
        panel.sandbox.write("pack.zip", &buffer.into_inner());
        panel.sandbox.write("config/one.yml", b"mine");

        let answer = panel
            .post(
                "/extract",
                serde_json::json!({ "path": "/pack.zip", "override": true, "dry": true }),
            )
            .await;
        assert_eq!(answer.status(), StatusCode::OK);
        let body = body_json(answer).await;
        assert_eq!(body["conflicting_files"][0], "/config/one.yml");
        assert_eq!(body["modpack_name"], serde_json::Value::Null);

        let runs: i64 = sqlx::query_scalar("SELECT count(*) FROM operations")
            .fetch_one(&panel.pool)
            .await
            .expect("a count");
        assert_eq!(runs, 0, "a dry run makes no operation");
    }

    #[tokio::test]
    async fn unpacking_runs_as_an_operation_and_puts_the_files_in_place() {
        let panel = panel().await;
        let mut buffer = std::io::Cursor::new(Vec::new());
        {
            let mut writer = zip::ZipWriter::new(&mut buffer);
            let options = zip::write::SimpleFileOptions::default();
            writer.start_file("mods/sodium.jar", options).expect("an entry");
            std::io::Write::write_all(&mut writer, b"jar bytes").expect("the bytes");
            writer.finish().expect("the archive");
        }
        panel.sandbox.write("packs/pack.zip", &buffer.into_inner());

        let answer = panel
            .post(
                "/extract",
                serde_json::json!({ "path": "/packs/pack.zip", "override": true, "dry": false }),
            )
            .await;
        assert_eq!(answer.status(), StatusCode::ACCEPTED);

        let body = body_json(answer).await;
        assert_eq!(body["operation"]["kind"], "unarchive");
        assert_eq!(body["operation"]["src"], "/packs/pack.zip", "the banner reads src");
        let id: Id = body["operation"]["id"].as_str().expect("an id").parse().expect("a ulid");

        assert!(panel.operations.busy_reasons(panel.server).await.expect("no fault").is_empty());

        let landed = panel.sandbox.server_dir().join("packs/mods/sodium.jar");
        for _ in 0..100 {
            if landed.exists() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        assert_eq!(std::fs::read_to_string(&landed).expect("the entry"), "jar bytes");

        for _ in 0..100 {
            if panel.operations.get(id).await.expect("the run").state.is_terminal() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        let run = panel.operations.get(id).await.expect("the run");
        assert_eq!(run.state, crate::model::OperationState::Done, "{:?}", run.error);
        assert_eq!(run.progress, 1.0);

        assert!(
            panel.chowned().iter().any(|path| path.ends_with("/packs")),
            "the unpacked tree is handed back once, for the whole run: {:?}",
            panel.chowned()
        );
        assert!(
            !panel.sandbox.server_dir().join(crate::files::WORK_DIR).join(id.to_string()).exists(),
            "the work directory goes when the run is through"
        );
    }

    #[tokio::test]
    async fn unpacking_into_a_folder_that_is_not_there_yet_hands_the_whole_new_tree_back() {
        let panel = panel().await;
        let mut buffer = std::io::Cursor::new(Vec::new());
        {
            let mut writer = zip::ZipWriter::new(&mut buffer);
            let options = zip::write::SimpleFileOptions::default();
            writer.start_file("sodium.jar", options).expect("an entry");
            std::io::Write::write_all(&mut writer, b"jar bytes").expect("the bytes");
            writer.finish().expect("the archive");
        }
        panel.sandbox.write("pack.zip", &buffer.into_inner());

        let answer = panel
            .post(
                "/extract",
                serde_json::json!({
                    "path": "/pack.zip", "target": "/brand/new", "override": true, "dry": false
                }),
            )
            .await;
        assert_eq!(answer.status(), StatusCode::ACCEPTED);
        let id: Id = body_json(answer).await["operation"]["id"]
            .as_str()
            .expect("an id")
            .parse()
            .expect("a ulid");

        for _ in 0..100 {
            if panel.operations.get(id).await.expect("the run").state.is_terminal() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        let run = panel.operations.get(id).await.expect("the run");
        assert_eq!(run.state, crate::model::OperationState::Done, "{:?}", run.error);
        assert!(panel.sandbox.server_dir().join("brand/new/sodium.jar").exists());

        assert!(
            panel.chowned().iter().any(|path| path.ends_with("/brand")),
            "the handover starts at the topmost directory the run made: {:?}",
            panel.chowned()
        );
    }

    #[tokio::test]
    async fn a_jar_is_not_an_archive_here() {
        let panel = panel().await;
        panel.sandbox.write("mods/a.jar", b"PK\x03\x04 whatever");

        let refused = panel
            .post("/extract", serde_json::json!({ "path": "/mods/a.jar", "override": true, "dry": true }))
            .await;
        assert_eq!(refused.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
        assert_eq!(body_json(refused).await["error"], "unsupported_archive");
    }

    #[tokio::test]
    async fn every_write_leaves_the_line_11_9_asks_for() {
        let panel = panel().await;
        panel.sandbox.mkdir("config");

        panel.put("/content?path=/config/one.yml", b"a: 1\n").await;
        panel.put("/content?path=/config/one.yml&on_conflict=overwrite", b"a: 2\n").await;
        panel
            .post(
                "/move",
                serde_json::json!({ "source": "/config/one.yml", "destination": "/config/two.yml" }),
            )
            .await;
        panel.delete("?path=/config/two.yml").await;

        let written: Vec<(String, Option<String>)> =
            sqlx::query_as("SELECT action, metadata FROM audit_log ORDER BY id")
                .fetch_all(&panel.pool)
                .await
                .expect("the log");
        let actions: Vec<&str> = written.iter().map(|(action, _)| action.as_str()).collect();
        assert_eq!(
            actions,
            ["file_uploaded", "file_edited", "file_renamed", "file_deleted"],
            "on_conflict tells the editor from the uploader (7.8)"
        );

        let meta = |index: usize| -> serde_json::Value {
            serde_json::from_str(written[index].1.as_deref().expect("metadata")).expect("json")
        };
        assert_eq!(meta(0)["path"], "/config/one.yml");
        assert_eq!(meta(2)["from"], "/config/one.yml");
        assert_eq!(meta(2)["to"], "/config/two.yml");
        assert_eq!(meta(3)["path"], "/config/two.yml");
    }

    #[tokio::test]
    async fn a_viewer_may_look_and_may_not_write() {
        let panel = panel().await;
        panel.sandbox.write("server.properties", b"x");

        let anna = a_user(&panel.pool, "anna").await;
        sqlx::query("INSERT INTO server_members (id, server_id, user_id, role, invited_at, joined_at) VALUES (?, ?, ?, ?, ?, ?)")
            .bind(Id::new())
            .bind(panel.server)
            .bind(anna)
            .bind(ServerRole::Viewer)
            .bind(crate::model::Timestamp::now())
            .bind(crate::model::Timestamp::now())
            .execute(&panel.pool)
            .await
            .expect("a viewer");
        let cookie = sign_in(&panel.pool, anna).await;

        for tail in ["/meta", "/list", "/content?path=/server.properties"] {
            let read = Request::builder()
                .uri(panel.url(tail))
                .header(COOKIE, format!("craft_session={cookie}"))
                .body(Body::empty())
                .expect("a request");
            assert_eq!(panel.send(read).await.status(), StatusCode::OK, "{tail}");
        }

        let write = Request::builder()
            .method("PUT")
            .uri(panel.url("/content?path=/server.properties&on_conflict=overwrite"))
            .header(CONTENT_TYPE, "application/octet-stream")
            .header(COOKIE, format!("craft_session={cookie}"))
            .body(Body::from("mine now"))
            .expect("a request");
        let refused = panel.send(write).await;
        assert_eq!(refused.status(), StatusCode::FORBIDDEN);
        assert_eq!(body_json(refused).await["error"], "forbidden");
        assert_eq!(std::fs::read(panel.sandbox.server_dir().join("server.properties")).unwrap(), b"x");

        let posts = [
            ("/create", serde_json::json!({ "path": "/mine.txt", "type": "file" })),
            (
                "/move",
                serde_json::json!({ "source": "/server.properties", "destination": "/mine.txt" }),
            ),
            (
                "/extract",
                serde_json::json!({ "path": "/pack.zip", "override": true, "dry": true }),
            ),
        ];
        for (tail, body) in posts {
            let request = Request::builder()
                .method("POST")
                .uri(panel.url(tail))
                .header(CONTENT_TYPE, "application/json")
                .header(COOKIE, format!("craft_session={cookie}"))
                .body(Body::from(body.to_string()))
                .expect("a request");
            let refused = panel.send(request).await;
            assert_eq!(refused.status(), StatusCode::FORBIDDEN, "{tail}");
            assert_eq!(body_json(refused).await["error"], "forbidden", "{tail}");
        }

        let deleted = Request::builder()
            .method("DELETE")
            .uri(panel.url("?path=/server.properties"))
            .header(COOKIE, format!("craft_session={cookie}"))
            .body(Body::empty())
            .expect("a request");
        let refused = panel.send(deleted).await;
        assert_eq!(refused.status(), StatusCode::FORBIDDEN);
        assert_eq!(body_json(refused).await["error"], "forbidden");

        assert!(panel.sandbox.server_dir().join("server.properties").exists());
        assert!(!panel.sandbox.server_dir().join("mine.txt").exists());
    }

    #[tokio::test]
    async fn a_stranger_is_told_the_server_does_not_exist() {
        let panel = panel().await;
        let bea = a_user(&panel.pool, "bea").await;
        let cookie = sign_in(&panel.pool, bea).await;

        let request = Request::builder()
            .uri(panel.url("/list"))
            .header(COOKIE, format!("craft_session={cookie}"))
            .body(Body::empty())
            .expect("a request");
        let refused = panel.send(request).await;
        assert_eq!(refused.status(), StatusCode::NOT_FOUND);
        assert_eq!(body_json(refused).await["error"], "server_not_found", "403 would leak the id");
    }

    #[tokio::test]
    async fn without_a_session_nothing_answers() {
        let panel = panel().await;
        let request =
            Request::builder().uri(panel.url("/list")).body(Body::empty()).expect("a request");
        assert_eq!(panel.send(request).await.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn a_panel_admin_reaches_a_server_that_is_not_his() {
        let panel = panel().await;
        panel.sandbox.write("server.properties", b"x");
        let bea = an_admin(&panel.pool, "bea").await;
        let cookie = sign_in(&panel.pool, bea).await;

        let request = Request::builder()
            .uri(panel.url("/list"))
            .header(COOKIE, format!("craft_session={cookie}"))
            .body(Body::empty())
            .expect("a request");
        assert_eq!(panel.send(request).await.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn a_write_while_a_backup_runs_is_refused_but_an_unarchive_locks_nothing() {
        let panel = panel().await;
        let backup = Id::new();
        sqlx::query("INSERT INTO backups (id, server_id, name, created_at) VALUES (?, ?, 'B', ?)")
            .bind(backup)
            .bind(panel.server)
            .bind(crate::model::Timestamp::now())
            .execute(&panel.pool)
            .await
            .expect("a backup row");
        let mut run = NewOperation::new(panel.server, OperationKind::BackupCreate, None);
        run.target_id = Some(backup);
        panel.operations.create(run).await.expect("a run");

        let refused = panel.put("/content?path=/a.txt", b"x").await;
        assert_eq!(refused.status(), StatusCode::CONFLICT);
        assert_eq!(body_json(refused).await["error"], "server_busy");

        assert_eq!(panel.get("/list").await.status(), StatusCode::OK);
    }
}
