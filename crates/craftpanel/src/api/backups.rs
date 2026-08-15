use std::sync::Arc;

use axum::body::Body;
use axum::extract::{FromRequestParts, State};
use axum::http::header::{CONTENT_DISPOSITION, CONTENT_LENGTH, CONTENT_TYPE, RETRY_AFTER};
use axum::http::request::Parts;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, patch, post};
use axum::{Extension, Json, Router};
use serde::{Deserialize, Serialize};

use crate::audit::{self, Event};
use crate::auth::error::{Failure, Result};
use crate::auth::{access, extract, Caller, JsonBody};
use crate::backups::{
    BackupListResponse, Backups, BulkFailure, Download, RestoreAccepted, RetryAccepted,
    UpdateBackupScheduleRequest,
};
use crate::model::{
    Backup, BackupSchedule, BackupTarget, Id, Permission, UpdateBackupTargetRequest,
};
use crate::AppState;

const BULK_LIMIT: usize = 100;

#[derive(Deserialize)]
struct CreateBackupRequest {
    name: String,
}

#[derive(Deserialize)]
struct RenameBackupRequest {
    name: String,
}

#[derive(Deserialize)]
struct RestoreBackupRequest {
    name: String,
}

#[derive(Deserialize)]
struct BulkDeleteBackupsRequest {
    backup_ids: Vec<Id>,
}

#[derive(Serialize)]
struct BulkDeleteBackupsResponse {
    deleted: Vec<Id>,
    failed: Vec<BulkFailure>,
}

pub fn router(backups: Arc<Backups>) -> Router<AppState> {
    Router::new()
        .route("/servers/{server}/backups", get(list).post(create))
        .route("/servers/{server}/backups/schedule", get(read_schedule).put(write_schedule))
        .route("/servers/{server}/backups/bulk-delete", post(bulk_delete))
        .route("/servers/{server}/backups/target", get(read_target).put(write_target))
        .route("/servers/{server}/backups/{backup}", patch(rename).delete(remove))
        .route("/servers/{server}/backups/{backup}/restore", post(restore))
        .route("/servers/{server}/backups/{backup}/retry", post(retry))
        .route("/servers/{server}/backups/{backup}/download", get(download))
        .layer(Extension(backups))
        .layer(axum::middleware::from_fn(extract::same_origin))
}

pub fn compat_router(backups: Arc<Backups>) -> Router<AppState> {
    Router::new()
        .route("/modrinth/v0/backups/{backup}/download", get(compat_download))
        .layer(Extension(backups))
}

async fn list(
    State(state): State<AppState>,
    Extension(backups): Extension<Arc<Backups>>,
    caller: Caller,
    Route(server): Route<Id>,
) -> Result<Json<BackupListResponse>> {
    access::require(&state.pool, &caller, server, Permission::BaseRead).await?;
    Ok(Json(backups.list(server).await?))
}

async fn create(
    State(state): State<AppState>,
    Extension(backups): Extension<Arc<Backups>>,
    caller: Caller,
    Route(server): Route<Id>,
    JsonBody(body): JsonBody<CreateBackupRequest>,
) -> Result<Response> {
    let access = access::require(&state.pool, &caller, server, Permission::Backups).await?;

    if let Some(seconds) = backups.cooldown(server).await? {
        let mut refused = Failure::new(
            StatusCode::TOO_MANY_REQUESTS,
            "rate_limited",
            format!("another backup of this server may be made in {seconds} seconds"),
        )
        .into_response();
        refused.headers_mut().insert(RETRY_AFTER, seconds.into());
        return Ok(refused);
    }

    let made = backups.request(server, &body.name, caller.id()).await?;
    audit::record(&state.pool, access, &caller, Event::BackupCreated { backup: made.id }).await;
    Ok((StatusCode::ACCEPTED, Json(made)).into_response())
}

async fn rename(
    State(state): State<AppState>,
    Extension(backups): Extension<Arc<Backups>>,
    caller: Caller,
    Route((server, backup)): Route<(Id, Id)>,
    JsonBody(body): JsonBody<RenameBackupRequest>,
) -> Result<Json<Backup>> {
    let access = access::require(&state.pool, &caller, server, Permission::Backups).await?;
    let before = backups.one(server, backup).await?;
    let renamed = backups.rename(server, backup, &body.name).await?;
    audit::record(
        &state.pool,
        access,
        &caller,
        Event::BackupRenamed { backup, from: before.name, to: renamed.name.clone() },
    )
    .await;
    Ok(Json(renamed))
}

async fn remove(
    State(state): State<AppState>,
    Extension(backups): Extension<Arc<Backups>>,
    caller: Caller,
    Route((server, backup)): Route<(Id, Id)>,
) -> Result<StatusCode> {
    let access = access::require(&state.pool, &caller, server, Permission::Backups).await?;
    backups.delete(server, backup).await?;
    audit::record(&state.pool, access, &caller, Event::BackupDeleted { backup }).await;
    Ok(StatusCode::NO_CONTENT)
}

async fn bulk_delete(
    State(state): State<AppState>,
    Extension(backups): Extension<Arc<Backups>>,
    caller: Caller,
    Route(server): Route<Id>,
    JsonBody(body): JsonBody<BulkDeleteBackupsRequest>,
) -> Result<Json<BulkDeleteBackupsResponse>> {
    let access = access::require(&state.pool, &caller, server, Permission::Backups).await?;
    if body.backup_ids.is_empty() || body.backup_ids.len() > BULK_LIMIT {
        return Err(Failure::invalid_request(format!(
            "between one and {BULK_LIMIT} backups, not {}",
            body.backup_ids.len()
        )));
    }

    let (deleted, failed) = backups.delete_many(server, &body.backup_ids).await;
    for backup in deleted.iter().copied() {
        audit::record(&state.pool, access, &caller, Event::BackupDeleted { backup }).await;
    }
    Ok(Json(BulkDeleteBackupsResponse { deleted, failed }))
}

async fn restore(
    State(state): State<AppState>,
    Extension(backups): Extension<Arc<Backups>>,
    caller: Caller,
    Route((server, backup)): Route<(Id, Id)>,
    JsonBody(body): JsonBody<RestoreBackupRequest>,
) -> Result<(StatusCode, Json<RestoreAccepted>)> {
    let access = access::require(&state.pool, &caller, server, Permission::Backups).await?;
    let accepted = backups.restore(server, backup, &body.name, caller.id()).await?;
    audit::record(&state.pool, access, &caller, Event::BackupRestored { backup }).await;
    Ok((StatusCode::ACCEPTED, Json(accepted)))
}

async fn retry(
    State(state): State<AppState>,
    Extension(backups): Extension<Arc<Backups>>,
    caller: Caller,
    Route((server, backup)): Route<(Id, Id)>,
) -> Result<(StatusCode, Json<RetryAccepted>)> {
    access::require(&state.pool, &caller, server, Permission::Backups).await?;
    let accepted = backups.retry(server, backup, caller.id()).await?;
    Ok((StatusCode::ACCEPTED, Json(accepted)))
}

async fn download(
    State(state): State<AppState>,
    Extension(backups): Extension<Arc<Backups>>,
    caller: Caller,
    Route((server, backup)): Route<(Id, Id)>,
) -> Result<Response> {
    access::require(&state.pool, &caller, server, Permission::Backups).await?;
    stream(backups.download(server, backup).await?).await
}

async fn compat_download(
    State(state): State<AppState>,
    Extension(backups): Extension<Arc<Backups>>,
    caller: Caller,
    Route(backup): Route<Id>,
) -> Result<Response> {
    let server = backups.server_of(backup).await?;
    access::require(&state.pool, &caller, server, Permission::Backups).await?;
    stream(backups.download(server, backup).await?).await
}

async fn read_schedule(
    State(state): State<AppState>,
    Extension(backups): Extension<Arc<Backups>>,
    caller: Caller,
    Route(server): Route<Id>,
) -> Result<Json<BackupSchedule>> {
    access::require(&state.pool, &caller, server, Permission::BaseRead).await?;
    Ok(Json(backups.schedule(server).await?))
}

async fn write_schedule(
    State(state): State<AppState>,
    Extension(backups): Extension<Arc<Backups>>,
    caller: Caller,
    Route(server): Route<Id>,
    JsonBody(body): JsonBody<UpdateBackupScheduleRequest>,
) -> Result<Json<BackupSchedule>> {
    access::require(&state.pool, &caller, server, Permission::Backups).await?;
    Ok(Json(backups.write_schedule(server, body).await?))
}

async fn read_target(
    State(state): State<AppState>,
    Extension(backups): Extension<Arc<Backups>>,
    caller: Caller,
    Route(server): Route<Id>,
) -> Result<Json<BackupTarget>> {
    access::require(&state.pool, &caller, server, Permission::BaseRead).await?;
    Ok(Json(backups.drive().target_of(server).await?))
}

async fn write_target(
    State(state): State<AppState>,
    Extension(backups): Extension<Arc<Backups>>,
    caller: Caller,
    Route(server): Route<Id>,
    JsonBody(body): JsonBody<UpdateBackupTargetRequest>,
) -> Result<Json<BackupTarget>> {
    access::require(&state.pool, &caller, server, Permission::Backups).await?;
    Ok(Json(backups.drive().set_target(server, body.target).await?))
}

async fn stream(file: Download) -> Result<Response> {
    let handle = tokio::fs::File::open(&file.path)
        .await
        .map_err(|err| Failure::internal(anyhow::Error::from(err)))?;
    let body = Body::from_stream(tokio_util::io::ReaderStream::new(handle));

    Ok(Response::builder()
        .status(StatusCode::OK)
        .header(CONTENT_TYPE, "application/zstd")
        .header(CONTENT_LENGTH, file.size_bytes)
        .header(CONTENT_DISPOSITION, disposition(&file.name, &file.created_at.to_string()))
        .body(body)
        .map_err(|err| Failure::internal(anyhow::Error::from(err)))?)
}

fn disposition(name: &str, created_at: &str) -> String {
    let full = format!("{name}-{created_at}.tar.zst");
    format!(
        "attachment; filename=\"{}-{created_at}.tar.zst\"; filename*=UTF-8''{}",
        slug(name),
        percent_encode(&full)
    )
}

pub(crate) fn slug(name: &str) -> String {
    let reduced: String = name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') { c } else { '-' })
        .take(64)
        .collect();
    let trimmed = reduced.trim_matches(|c| c == '-' || c == '.').to_owned();
    if trimmed.is_empty() {
        "backup".to_owned()
    } else {
        trimmed
    }
}

fn percent_encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.as_bytes() {
        let plain = byte.is_ascii_alphanumeric()
            || matches!(byte, b'!' | b'#' | b'$' | b'&' | b'+' | b'-' | b'.' | b'^' | b'_' | b'`'
                | b'|' | b'~');
        if plain {
            out.push(*byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

struct Route<T>(pub T);

impl<T> FromRequestParts<AppState> for Route<T>
where
    T: serde::de::DeserializeOwned + Send,
{
    type Rejection = Failure;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self> {
        match axum::extract::Path::<T>::from_request_parts(parts, state).await {
            Ok(axum::extract::Path(path)) => Ok(Self(path)),
            Err(rejection) => Err(Failure::invalid_request(rejection.body_text())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::harness::{
        a_server, a_user, as_user, body_json, empty, fetch, send, sign_in, state_with, FakeHelper,
    };
    use crate::auth::Disks;
    use crate::config::Config;
    use crate::helper::Helper;
    use crate::model::{ServerRole, Timestamp};
    use crate::ops::testing::{busy_schema, DataDir};
    use crate::ops::Operations;
    use crate::servers::Hub;
    use axum::http::Request;
    use sqlx::SqlitePool;
    use tower::ServiceExt;

    struct Panel {
        app: Router,
        pool: SqlitePool,
        backups: Arc<Backups>,
        owner: Id,
        server: Id,
        cookie: String,
        dir: DataDir,
        _helper: FakeHelper,
    }

    async fn panel() -> Panel {
        let dir = DataDir::new();
        let pool = busy_schema(&dir).await;
        let owner = a_user(&pool, "max").await;
        let server = a_server(&pool, owner, "Survival", 2048).await;
        let cookie = sign_in(&pool, owner).await;

        let helper = FakeHelper::obliging().await;
        let operations = Operations::new(pool.clone(), dir.path());
        let hub = Arc::new(Hub::new(dir.path().join("supervise.sock")));
        let drive = crate::drive::Drive::against(
            pool.clone(),
            dir.path(),
            "http://127.0.0.1:1",
            "http://127.0.0.1:1",
        );
        let backups = Backups::new(
            pool.clone(),
            dir.path(),
            operations,
            hub,
            Helper::new(helper.socket()),
            Disks::none(),
            drive,
        );

        let config = Config { data_dir: dir.path().to_path_buf(), ..Config::default() };
        let app = Router::new()
            .nest("/api/v1", router(Arc::clone(&backups)))
            .merge(compat_router(Arc::clone(&backups)))
            .with_state(state_with(&pool, config));

        Panel { app, pool, backups, owner, server, cookie, dir, _helper: helper }
    }

    impl Panel {
        async fn call(&self, request: Request<Body>) -> Response<Body> {
            self.app.clone().oneshot(as_user(request, &self.cookie)).await.expect("an answer")
        }

        async fn as_other(&self, request: Request<Body>, cookie: &str) -> Response<Body> {
            self.app.clone().oneshot(as_user(request, cookie)).await.expect("an answer")
        }

        fn file(&self, relative: &str, contents: &[u8]) {
            let path = self
                .dir
                .path()
                .join("users")
                .join(self.owner.to_string())
                .join("servers")
                .join(self.server.to_string())
                .join(relative);
            std::fs::create_dir_all(path.parent().expect("a parent")).expect("the parents");
            std::fs::write(path, contents).expect("a file");
        }

        async fn a_backup(&self, name: &str) -> Id {
            self.file("world/level.dat", b"a world");
            let queued = self
                .backups
                .create(self.server, name, Some(self.owner), false)
                .await
                .expect("a backup");
            self.backups.run(queued.operation).await;
            queued.backup
        }

        async fn a_member(&self, username: &str, role: ServerRole) -> String {
            let user = a_user(&self.pool, username).await;
            sqlx::query(
                "INSERT INTO server_members (id, server_id, user_id, role, invited_at, joined_at) \
                 VALUES (?, ?, ?, ?, ?, ?)",
            )
            .bind(Id::new())
            .bind(self.server)
            .bind(user)
            .bind(role)
            .bind(Timestamp::now())
            .bind(Timestamp::now())
            .execute(&self.pool)
            .await
            .expect("a membership");
            sign_in(&self.pool, user).await
        }

        fn path(&self, tail: &str) -> String {
            format!("/api/v1/servers/{}/backups{tail}", self.server)
        }

        async fn settled(&self, backup: Id) -> crate::model::BackupStatus {
            for _ in 0..2_000 {
                let seen = self.backups.one(self.server, backup).await.expect("the backup").status;
                if !matches!(
                    seen,
                    crate::model::BackupStatus::Pending | crate::model::BackupStatus::InProgress
                ) {
                    return seen;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
            panic!("the run on {backup} never ended");
        }
    }

    #[tokio::test]
    async fn the_matrix_of_2_1_holds_for_every_verb() {
        let panel = panel().await;
        let backup = panel.a_backup("one").await;
        let viewer = panel.a_member("vera", ServerRole::Viewer).await;
        let body_for = |path: &str, backup: Id| {
            if path.ends_with("/schedule") {
                serde_json::json!({
                    "enabled": false, "interval_hours": 24, "hour_utc": 4, "keep_last": 5
                })
            } else if path.ends_with("/bulk-delete") {
                serde_json::json!({ "backup_ids": [backup.to_string()] })
            } else {
                serde_json::json!({ "name": "mine now" })
            }
        };

        let rows: Vec<(&str, String, StatusCode)> = vec![
            ("GET", panel.path(""), StatusCode::OK),
            ("GET", panel.path("/schedule"), StatusCode::OK),
            ("POST", panel.path(""), StatusCode::FORBIDDEN),
            ("PUT", panel.path("/schedule"), StatusCode::FORBIDDEN),
            ("POST", panel.path("/bulk-delete"), StatusCode::FORBIDDEN),
            ("PATCH", panel.path(&format!("/{backup}")), StatusCode::FORBIDDEN),
            ("DELETE", panel.path(&format!("/{backup}")), StatusCode::FORBIDDEN),
            ("POST", panel.path(&format!("/{backup}/restore")), StatusCode::FORBIDDEN),
            ("POST", panel.path(&format!("/{backup}/retry")), StatusCode::FORBIDDEN),
            ("GET", panel.path(&format!("/{backup}/download")), StatusCode::FORBIDDEN),
            (
                "GET",
                format!("/modrinth/v0/backups/{backup}/download"),
                StatusCode::FORBIDDEN,
            ),
        ];

        for (method, path, wanted) in &rows {
            let request = match *method {
                "GET" => fetch(path),
                "DELETE" => empty("DELETE", path),
                other => send(other, path, body_for(path, backup)),
            };
            let answer = panel.as_other(request, &viewer).await;
            assert_eq!(answer.status(), *wanted, "{method} {path}");
        }

        let editor = panel.a_member("edda", ServerRole::Editor).await;
        for (method, path, _) in &rows {
            let request = match *method {
                "GET" => fetch(path),
                "DELETE" => empty("DELETE", path),
                other => send(other, path, body_for(path, backup)),
            };
            let answer = panel.as_other(request, &editor).await;
            assert_ne!(answer.status(), StatusCode::FORBIDDEN, "{method} {path}");
        }
    }

    #[tokio::test]
    async fn how_far_along_a_backup_is_stays_between_it_and_its_own_server() {
        let panel = panel().await;
        let unfinished = panel
            .backups
            .create(panel.server, "under way", Some(panel.owner), false)
            .await
            .expect("a queued backup")
            .backup;

        let elsewhere = a_server(&panel.pool, panel.owner, "Creative", 1024).await;
        let through_the_wrong_server =
            format!("/api/v1/servers/{elsewhere}/backups/{unfinished}/download");
        let answer = panel.call(fetch(&through_the_wrong_server)).await;
        assert_eq!(answer.status(), StatusCode::NOT_FOUND);
        assert_eq!(body_json(answer).await["error"], "backup_not_found");

        let stranger = sign_in(&panel.pool, a_user(&panel.pool, "eve").await).await;
        for path in [
            panel.path(&format!("/{unfinished}/download")),
            format!("/modrinth/v0/backups/{unfinished}/download"),
        ] {
            let answer = panel.as_other(fetch(&path), &stranger).await;
            assert_eq!(answer.status(), StatusCode::NOT_FOUND, "{path}");
            assert_eq!(body_json(answer).await["error"], "server_not_found", "{path}");
        }
    }

    #[tokio::test]
    async fn a_stranger_is_told_the_server_does_not_exist() {
        let panel = panel().await;
        let stranger = sign_in(&panel.pool, a_user(&panel.pool, "eve").await).await;

        let answer = panel.as_other(fetch(&panel.path("")), &stranger).await;
        assert_eq!(answer.status(), StatusCode::NOT_FOUND);
        assert_eq!(body_json(answer).await["error"], "server_not_found");
    }

    #[tokio::test]
    async fn the_word_schedule_is_not_read_as_a_backup_id() {
        let panel = panel().await;
        let answer = panel.call(fetch(&panel.path("/schedule"))).await;
        assert_eq!(answer.status(), StatusCode::OK, "1.3: a static segment wins over a ULID");

        let body = body_json(answer).await;
        assert_eq!(body["enabled"], false, "the default is off (10.10)");
        assert_eq!(body["interval_hours"], 24);
        assert!(body["next_run_at"].is_null());
    }

    #[tokio::test]
    async fn the_schedule_is_written_back_with_the_next_time_worked_out() {
        let panel = panel().await;
        let wanted = serde_json::json!({
            "enabled": true, "interval_hours": 24, "hour_utc": 4, "keep_last": 3
        });
        let answer = panel.call(send("PUT", &panel.path("/schedule"), wanted)).await;
        assert_eq!(answer.status(), StatusCode::OK);
        let body = body_json(answer).await;
        assert_eq!(body["keep_last"], 3);
        assert!(body["next_run_at"].as_str().expect("a time").ends_with("04:00:00Z"));

        let over = serde_json::json!({
            "enabled": true, "interval_hours": 500, "hour_utc": 4, "keep_last": 3
        });
        let refused = panel.call(send("PUT", &panel.path("/schedule"), over)).await;
        assert_eq!(refused.status(), StatusCode::BAD_REQUEST);
        let body = body_json(refused).await;
        assert_eq!(body["error"], "invalid_schedule");
        assert!(body["message"].as_str().expect("a text").contains("interval_hours"));
    }

    #[tokio::test]
    async fn making_one_answers_202_with_the_whole_backup_and_the_next_is_asked_to_wait() {
        let panel = panel().await;
        panel.file("world/level.dat", b"a world");

        let answer =
            panel.call(send("POST", &panel.path(""), serde_json::json!({ "name": "one" }))).await;
        assert_eq!(answer.status(), StatusCode::ACCEPTED);
        let body = body_json(answer).await;
        assert!(body["id"].is_string(), "use-inline-backup.ts destructures id");
        assert_eq!(body["status"], "pending");
        assert_eq!(body["automated"], false);
        assert_eq!(body["locked"], false);
        assert_eq!(body["size_bytes"], 0);
        assert_eq!(body["history"][0]["operation_type"], "create");

        let backup: Id = body["id"].as_str().expect("an id").parse().expect("a ULID");
        assert_eq!(panel.settled(backup).await, crate::model::BackupStatus::Done);

        let again =
            panel.call(send("POST", &panel.path(""), serde_json::json!({ "name": "two" }))).await;
        assert_eq!(again.status(), StatusCode::TOO_MANY_REQUESTS);
        assert!(again.headers().contains_key(RETRY_AFTER), "10.2 asks for Retry-After");
        assert_eq!(body_json(again).await["error"], "rate_limited");
    }

    #[tokio::test]
    async fn an_empty_name_and_an_empty_list_are_both_bad_requests() {
        let panel = panel().await;
        panel.file("world/level.dat", b"a world");

        let empty_name =
            panel.call(send("POST", &panel.path(""), serde_json::json!({ "name": "  " }))).await;
        assert_eq!(empty_name.status(), StatusCode::BAD_REQUEST);
        assert_eq!(body_json(empty_name).await["error"], "invalid_name");

        let none = panel
            .call(send("POST", &panel.path("/bulk-delete"), serde_json::json!({ "backup_ids": [] })))
            .await;
        assert_eq!(none.status(), StatusCode::BAD_REQUEST);
        assert_eq!(body_json(none).await["error"], "invalid_request");

        let too_many: Vec<String> = (0..101).map(|_| Id::new().to_string()).collect();
        let flood = panel
            .call(send(
                "POST",
                &panel.path("/bulk-delete"),
                serde_json::json!({ "backup_ids": too_many }),
            ))
            .await;
        assert_eq!(flood.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn deleting_many_answers_what_went_and_what_stayed() {
        let panel = panel().await;
        let good = panel.a_backup("one").await;
        let gone = Id::new();

        let answer = panel
            .call(send(
                "POST",
                &panel.path("/bulk-delete"),
                serde_json::json!({ "backup_ids": [good.to_string(), gone.to_string()] }),
            ))
            .await;
        assert_eq!(answer.status(), StatusCode::OK);
        let body = body_json(answer).await;
        assert_eq!(body["deleted"], serde_json::json!([good.to_string()]));
        assert_eq!(body["failed"][0]["id"], gone.to_string());
        assert_eq!(body["failed"][0]["error"], "backup_not_found");
    }

    #[tokio::test]
    async fn an_unknown_backup_is_404_and_a_word_in_its_place_is_400() {
        let panel = panel().await;
        let unknown = panel.call(empty("DELETE", &panel.path(&format!("/{}", Id::new())))).await;
        assert_eq!(unknown.status(), StatusCode::NOT_FOUND);
        assert_eq!(body_json(unknown).await["error"], "backup_not_found");

        let nonsense = panel.call(empty("DELETE", &panel.path("/not-a-ulid"))).await;
        assert_eq!(nonsense.status(), StatusCode::BAD_REQUEST);
        assert_eq!(body_json(nonsense).await["error"], "invalid_request");
    }

    #[tokio::test]
    async fn the_download_carries_the_bytes_and_a_header_the_name_cannot_break() {
        let panel = panel().await;
        let backup = panel.a_backup("he said \"hi\"/../x").await;

        let answer = panel.call(fetch(&panel.path(&format!("/{backup}/download")))).await;
        assert_eq!(answer.status(), StatusCode::OK);
        assert_eq!(answer.headers()[CONTENT_TYPE], "application/zstd");
        let length: u64 = answer.headers()[CONTENT_LENGTH]
            .to_str()
            .expect("a number")
            .parse()
            .expect("a number");
        assert!(length > 0);

        let header = answer.headers()[CONTENT_DISPOSITION].to_str().expect("a header").to_owned();
        assert!(header.starts_with("attachment; filename=\"he-said--hi--..-x-"), "{header}");
        assert!(header.contains("filename*=UTF-8''"), "{header}");
        assert_eq!(
            header.matches('"').count(),
            2,
            "one quotation mark from the name would add a third: {header}"
        );

        let bytes = axum::body::to_bytes(answer.into_body(), usize::MAX).await.expect("the file");
        assert_eq!(bytes.len() as u64, length);
        assert_eq!(&bytes[..4], &[0x28, 0xB5, 0x2F, 0xFD], "a zstd frame, unpackable by hand");
    }

    #[tokio::test]
    async fn the_compat_path_finds_the_server_itself_and_ignores_the_auth_parameter() {
        let panel = panel().await;
        let backup = panel.a_backup("one").await;

        let answer = panel
            .call(fetch(&format!("/modrinth/v0/backups/{backup}/download?auth=whatever")))
            .await;
        assert_eq!(answer.status(), StatusCode::OK, "10.11: the cookie decides, not the query");
        assert_eq!(answer.headers()[CONTENT_TYPE], "application/zstd");

        let stranger = sign_in(&panel.pool, a_user(&panel.pool, "eve").await).await;
        let refused = panel
            .as_other(fetch(&format!("/modrinth/v0/backups/{backup}/download")), &stranger)
            .await;
        assert_eq!(refused.status(), StatusCode::NOT_FOUND);
        assert_eq!(body_json(refused).await["error"], "server_not_found");
    }

    #[tokio::test]
    async fn a_backup_of_another_server_is_not_reachable_through_this_one() {
        let panel = panel().await;
        let backup = panel.a_backup("one").await;
        let other = a_server(&panel.pool, panel.owner, "Creative", 1024).await;

        for (method, tail) in [
            ("GET", "/download"),
            ("POST", "/restore"),
            ("POST", "/retry"),
            ("PATCH", ""),
            ("DELETE", ""),
        ] {
            let path = format!("/api/v1/servers/{other}/backups/{backup}{tail}");
            let request = match method {
                "GET" => fetch(&path),
                "DELETE" => empty("DELETE", &path),
                _ => send(method, &path, serde_json::json!({ "name": "mine now" })),
            };
            let answer = panel.call(request).await;
            assert_eq!(
                answer.status(),
                StatusCode::NOT_FOUND,
                "a backup of another server is no backup of this one: {path}"
            );
            assert_eq!(body_json(answer).await["error"], "backup_not_found");
        }
    }

    #[tokio::test]
    async fn every_change_leaves_a_line_in_the_check_log() {
        let panel = panel().await;
        let backup = panel.a_backup("one").await;

        let renamed = panel
            .call(send(
                "PATCH",
                &panel.path(&format!("/{backup}")),
                serde_json::json!({ "name": "two" }),
            ))
            .await;
        assert_eq!(renamed.status(), StatusCode::OK);
        panel.call(empty("DELETE", &panel.path(&format!("/{backup}")))).await;

        let lines: Vec<(String, Option<String>)> =
            sqlx::query_as("SELECT action, metadata FROM audit_log ORDER BY id")
                .fetch_all(&panel.pool)
                .await
                .expect("the log");
        let actions: Vec<&str> = lines.iter().map(|(action, _)| action.as_str()).collect();
        assert_eq!(actions, vec!["backup_renamed", "backup_deleted"]);

        let rename: serde_json::Value =
            serde_json::from_str(lines[0].1.as_deref().expect("metadata")).expect("json");
        assert_eq!(rename["from"], "one");
        assert_eq!(rename["to"], "two");
        assert_eq!(rename["id"], backup.to_string());
    }

    #[tokio::test]
    async fn without_a_cookie_nothing_answers() {
        let panel = panel().await;
        let answer = panel.app.clone().oneshot(fetch(&panel.path(""))).await.expect("an answer");
        assert_eq!(answer.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn a_foreign_origin_is_turned_away_on_the_writing_routes() {
        let panel = panel().await;
        let mut request = send("POST", &panel.path(""), serde_json::json!({ "name": "one" }));
        request.headers_mut().insert("origin", "https://evil.example".parse().unwrap());
        request.headers_mut().insert("host", "panel.example".parse().unwrap());

        let answer = panel.call(request).await;
        assert_eq!(answer.status(), StatusCode::FORBIDDEN);
        assert_eq!(body_json(answer).await["error"], "csrf_origin_mismatch");
    }

    #[test]
    fn a_slug_keeps_what_a_file_name_may_hold_and_nothing_else() {
        assert_eq!(slug("Sunday backup"), "Sunday-backup");
        assert_eq!(slug("world.v1_2"), "world.v1_2");
        assert_eq!(slug("../../etc/passwd"), "etc-passwd");
        assert_eq!(slug("„"), "backup", "nothing left over is still a file name");
        assert_eq!(slug(&"a".repeat(200)).len(), 64);
    }

    #[test]
    fn the_encoded_name_carries_no_character_that_could_end_the_header() {
        let encoded = percent_encode("a \"quote\"\r\nInjected: yes");
        assert!(!encoded.contains('"'));
        assert!(!encoded.contains('\r'));
        assert!(!encoded.contains('\n'));
        assert_eq!(percent_encode("Renée"), "Ren%C3%A9e");
    }
}
