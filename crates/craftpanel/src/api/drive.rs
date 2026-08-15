use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{delete, get, post};
use axum::{Extension, Json, Router};
use serde::Deserialize;

use crate::auth::error::{Failure, Result};
use crate::auth::{extract, Admin, Caller, JsonBody, Params};
use crate::drive::{Drive, DriveAdminOverview, DriveLink, DriveStatus, Files, SecretChange};
use crate::model::{BackupTargetPolicy, Id, Timestamp};
use crate::AppState;

pub fn router(drive: Arc<Drive>) -> Router<AppState> {
    Router::new()
        .route("/drive", get(status).delete(disconnect))
        .route("/drive/link", get(link).post(begin_link).delete(cancel_link))
        .route("/drive/check", post(check))
        .route("/admin/drive", get(overview).put(save))
        .route("/admin/drive/credentials", delete(forget_credentials))
        .route("/admin/drive/{user}", delete(disconnect_user))
        .layer(Extension(drive))
        .layer(axum::middleware::from_fn(extract::same_origin))
}

async fn status(
    caller: Caller,
    Extension(drive): Extension<Arc<Drive>>,
) -> Result<Json<DriveStatus>> {
    Ok(Json(drive.of(caller.id()).status().await?))
}

async fn begin_link(
    caller: Caller,
    Extension(drive): Extension<Arc<Drive>>,
) -> Result<(StatusCode, Json<DriveLink>)> {
    let started = drive.of(caller.id()).begin_link().await?;
    Ok((StatusCode::CREATED, Json(started)))
}

async fn link(caller: Caller, Extension(drive): Extension<Arc<Drive>>) -> Result<Json<DriveLink>> {
    Ok(Json(drive.of(caller.id()).link().await?))
}

async fn cancel_link(
    caller: Caller,
    Extension(drive): Extension<Arc<Drive>>,
) -> Result<StatusCode> {
    drive.of(caller.id()).cancel_link().await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
struct DisconnectQuery {
    files: Option<String>,
}

async fn disconnect(
    caller: Caller,
    Extension(drive): Extension<Arc<Drive>>,
    Params(query): Params<DisconnectQuery>,
) -> Result<StatusCode> {
    drive.of(caller.id()).disconnect(disposal(query.files.as_deref())?).await?;
    Ok(StatusCode::NO_CONTENT)
}

fn disposal(files: Option<&str>) -> Result<Files> {
    match files {
        None => Ok(Files::Refuse),
        Some("delete") => Ok(Files::Delete),
        Some("keep") => Ok(Files::Keep),
        Some(_) => Err(Failure::bad_request(
            "invalid_request",
            "files is either delete or keep",
        )),
    }
}

async fn check(
    caller: Caller,
    Extension(drive): Extension<Arc<Drive>>,
) -> Result<(StatusCode, Json<DriveStatus>)> {
    let seen = drive.of(caller.id()).check().await?;
    Ok((StatusCode::ACCEPTED, Json(seen)))
}

async fn overview(
    _: Admin,
    Extension(drive): Extension<Arc<Drive>>,
) -> Result<Json<DriveAdminOverview>> {
    Ok(Json(drive.admin_overview().await?))
}

#[derive(Deserialize)]
struct UpdateDriveSettingsRequest {
    client_id: Option<String>,
    #[serde(default)]
    client_secret: Option<String>,
    target_policy: BackupTargetPolicy,
    folder_name: String,
}

async fn save(
    _: Admin,
    Extension(drive): Extension<Arc<Drive>>,
    JsonBody(body): JsonBody<UpdateDriveSettingsRequest>,
) -> Result<Json<DriveAdminOverview>> {
    let secret = match body.client_secret {
        None => SecretChange::Keep,
        Some(text) if text.trim().is_empty() => SecretChange::Remove,
        Some(text) => SecretChange::Replace(text),
    };
    let saved = drive
        .save(
            body.client_id,
            secret,
            body.target_policy,
            body.folder_name,
            Timestamp::now(),
        )
        .await?;
    Ok(Json(saved))
}

async fn forget_credentials(
    _: Admin,
    Extension(drive): Extension<Arc<Drive>>,
) -> Result<StatusCode> {
    drive.forget_credentials(Timestamp::now()).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn disconnect_user(
    admin: Admin,
    State(state): State<AppState>,
    Extension(drive): Extension<Arc<Drive>>,
    Path(user): Path<String>,
) -> Result<StatusCode> {
    let user: Id = user.parse().map_err(|_| unknown_user())?;
    let username: Option<String> = sqlx::query_scalar("SELECT username FROM users WHERE id = ?")
        .bind(user)
        .fetch_optional(&state.pool)
        .await?;
    let username = username.ok_or_else(unknown_user)?;

    drive.of(user).disconnect(Files::Keep).await?;
    tracing::warn!(
        by = %admin.0.user.username,
        user = %username,
        "a panel administrator disconnected somebody else's Google Drive"
    );
    Ok(StatusCode::NO_CONTENT)
}

fn unknown_user() -> Failure {
    Failure::not_found("user_not_found", "no such account")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::harness::{
        a_server, a_user, an_admin, as_user, body_json, empty, send as posted, sign_in, state_with,
        test_pool,
    };
    use crate::config::Config;
    use crate::drive::SecretChange;
    use crate::model::{BackupTargetPolicy, Timestamp};
    use axum::body::Body;
    use axum::http::Request;
    use sqlx::SqlitePool;
    use tower::ServiceExt;

    struct Panel {
        app: Router,
        pool: SqlitePool,
        drive: Arc<Drive>,
        dir: std::path::PathBuf,
    }

    impl Drop for Panel {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    async fn panel() -> Panel {
        let pool = test_pool().await;
        let dir = std::env::temp_dir().join(format!("craftpanel-drive-api-{}", Id::new()));
        let _ = std::fs::remove_dir_all(&dir);

        let mut config = Config::default();
        config.data_dir = dir.clone();

        let drive = Drive::against(pool.clone(), &dir, "http://127.0.0.1:1", "http://127.0.0.1:1");
        let backups = crate::backups::Backups::new(
            pool.clone(),
            &dir,
            crate::ops::Operations::new(pool.clone(), &dir),
            Arc::new(crate::servers::Hub::new(dir.join("supervise.sock"))),
            crate::helper::Helper::new(dir.join("helper.sock")),
            crate::auth::Disks::none(),
            Arc::clone(&drive),
        );
        let app = Router::new()
            .nest(
                "/api/v1",
                router(Arc::clone(&drive)).merge(crate::api::backups::router(backups)),
            )
            .with_state(state_with(&pool, config));

        Panel { app, pool, drive, dir }
    }

    async fn send(panel: &Panel, request: Request<Body>) -> axum::response::Response {
        panel.app.clone().oneshot(request).await.expect("a response")
    }

    async fn set_up(panel: &Panel) {
        panel
            .drive
            .save(
                Some("1234.apps.googleusercontent.com".to_owned()),
                SecretChange::Replace("GOCSPX-test".to_owned()),
                BackupTargetPolicy::UserChoice,
                "craftpanel-backups".to_owned(),
                Timestamp::now(),
            )
            .await
            .expect("the operator's settings");
    }

    #[tokio::test]
    async fn the_own_page_needs_a_session_and_answers_without_a_google_project() {
        let panel = panel().await;
        let anna = a_user(&panel.pool, "anna").await;
        let cookie = sign_in(&panel.pool, anna).await;

        let anonymous = send(&panel, empty("GET", "/api/v1/drive")).await;
        assert_eq!(anonymous.status(), StatusCode::UNAUTHORIZED);

        let hers = send(&panel, as_user(empty("GET", "/api/v1/drive"), &cookie)).await;
        assert_eq!(hers.status(), StatusCode::OK);
        let body = body_json(hers).await;
        assert_eq!(body["panel_configured"], false, "22.2: nothing set up is an answer");
        assert_eq!(body["configured"], false);
        assert_eq!(body["folder_name"], "craftpanel-backups");
        assert!(body["link"].is_null());
    }

    #[tokio::test]
    async fn an_account_in_the_middle_of_an_attempt_is_not_an_error_on_the_wire() {
        let panel = panel().await;
        set_up(&panel).await;
        let anna = a_user(&panel.pool, "anna").await;
        let hers = sign_in(&panel.pool, anna).await;
        let root = an_admin(&panel.pool, "root").await;
        let his = sign_in(&panel.pool, root).await;

        let now = Timestamp::now();
        crate::drive::store::begin_link(
            &panel.pool,
            anna,
            &crate::drive::store::Link {
                user_code: "GQVQ-JKEC".to_owned(),
                state: crate::model::DriveLinkState::Waiting,
                started_at: now,
                expires_at: now,
            },
            now,
        )
        .await
        .expect("an attempt");

        let own = send(&panel, as_user(empty("GET", "/api/v1/drive"), &hers)).await;
        let mine = body_json(own).await;
        assert!(mine["state"].is_null(), "an account that is connecting is not broken: {mine}");
        assert!(mine["last_error"].is_null(), "and it has nothing to complain about: {mine}");
        assert_eq!(mine["configured"], false);

        let overview =
            body_json(send(&panel, as_user(empty("GET", "/api/v1/admin/drive"), &his)).await).await;
        let line = &overview["accounts"][0];
        assert_eq!(line["username"], "anna");
        assert!(line["state"].is_null(), "the operator read `error` here for every user: {line}");
    }

    #[tokio::test]
    async fn only_an_administrator_reaches_the_operators_endpoints() {
        let panel = panel().await;
        let anna = a_user(&panel.pool, "anna").await;
        let hers = sign_in(&panel.pool, anna).await;
        let root = an_admin(&panel.pool, "root").await;
        let his = sign_in(&panel.pool, root).await;

        for path in ["/api/v1/admin/drive"] {
            let refused = send(&panel, as_user(empty("GET", path), &hers)).await;
            assert_eq!(refused.status(), StatusCode::FORBIDDEN, "{path}");
        }
        let refused =
            send(&panel, as_user(empty("DELETE", "/api/v1/admin/drive/credentials"), &hers)).await;
        assert_eq!(refused.status(), StatusCode::FORBIDDEN);
        let refused =
            send(&panel, as_user(empty("DELETE", &format!("/api/v1/admin/drive/{anna}")), &hers))
                .await;
        assert_eq!(refused.status(), StatusCode::FORBIDDEN);

        let allowed = send(&panel, as_user(empty("GET", "/api/v1/admin/drive"), &his)).await;
        assert_eq!(allowed.status(), StatusCode::OK);
        let body = body_json(allowed).await;
        assert_eq!(body["configured"], false);
        assert_eq!(body["target_policy"], "user_choice");
        assert!(body["accounts"].as_array().expect("a list").is_empty());
        assert!(body.get("client_secret").is_none());
    }

    #[tokio::test]
    async fn saving_the_credentials_never_hands_the_secret_back() {
        let panel = panel().await;
        let root = an_admin(&panel.pool, "root").await;
        let cookie = sign_in(&panel.pool, root).await;

        let saved = send(
            &panel,
            as_user(
                posted("PUT", "/api/v1/admin/drive", serde_json::json!({
                        "client_id": "1234.apps.googleusercontent.com",
                        "client_secret": "GOCSPX-super-secret",
                        "target_policy": "drive_only",
                        "folder_name": "backups",
                    }),
                ),
                &cookie,
            ),
        )
        .await;

        assert_eq!(saved.status(), StatusCode::OK);
        let body = body_json(saved).await;
        assert_eq!(body["configured"], true);
        assert_eq!(body["client_id"], "1234.apps.googleusercontent.com");
        assert_eq!(body["target_policy"], "drive_only");
        assert!(
            !serde_json::to_string(&body).expect("json").contains("GOCSPX"),
            "the client secret came back out of the panel: {body}"
        );

        let file = panel.dir.join("drive").join("client_secret");
        assert!(file.exists());
        let mode = std::os::unix::fs::PermissionsExt::mode(
            &std::fs::metadata(&file).expect("the file").permissions(),
        );
        assert_eq!(mode & 0o777, 0o600);
    }

    #[tokio::test]
    async fn disconnecting_takes_only_the_two_words_it_knows() {
        let panel = panel().await;
        let anna = a_user(&panel.pool, "anna").await;
        let cookie = sign_in(&panel.pool, anna).await;

        let nonsense =
            send(&panel, as_user(empty("DELETE", "/api/v1/drive?files=maybe"), &cookie)).await;
        assert_eq!(nonsense.status(), StatusCode::BAD_REQUEST);
        assert_eq!(body_json(nonsense).await["error"], "invalid_request");

        let bare = send(&panel, as_user(empty("DELETE", "/api/v1/drive"), &cookie)).await;
        assert_eq!(bare.status(), StatusCode::CONFLICT);
        assert_eq!(body_json(bare).await["error"], "drive_not_connected");
    }

    #[tokio::test]
    async fn connecting_without_a_google_project_is_a_conflict_and_not_a_failure() {
        let panel = panel().await;
        let anna = a_user(&panel.pool, "anna").await;
        let cookie = sign_in(&panel.pool, anna).await;

        let refused = send(&panel, as_user(empty("POST", "/api/v1/drive/link"), &cookie)).await;
        assert_eq!(refused.status(), StatusCode::CONFLICT);
        assert_eq!(body_json(refused).await["error"], "drive_not_configured");

        let none = send(&panel, as_user(empty("GET", "/api/v1/drive/link"), &cookie)).await;
        assert_eq!(none.status(), StatusCode::NOT_FOUND);
        assert_eq!(body_json(none).await["error"], "drive_link_not_found");

        let nothing = send(&panel, as_user(empty("DELETE", "/api/v1/drive/link"), &cookie)).await;
        assert_eq!(nothing.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn the_target_of_a_server_is_read_and_written_at_its_own_path() {
        let panel = panel().await;
        let anna = a_user(&panel.pool, "anna").await;
        let cookie = sign_in(&panel.pool, anna).await;
        let server = a_server(&panel.pool, anna, "Survival", 2048).await;
        let path = format!("/api/v1/servers/{server}/backups/target");

        let read = send(&panel, as_user(empty("GET", &path), &cookie)).await;
        assert_eq!(read.status(), StatusCode::OK, "the static segment was read as a backup id");
        let body = body_json(read).await;
        assert_eq!(body["target"], "local");
        assert_eq!(body["effective_target"], "local");
        assert_eq!(body["reason"], "not_configured", "22.9 says why, so no switch is grey for nothing");

        let refused = send(
            &panel,
            as_user(posted("PUT", &path, serde_json::json!({ "target": "drive" })), &cookie),
        )
        .await;
        assert_eq!(refused.status(), StatusCode::CONFLICT);
        assert_eq!(body_json(refused).await["error"], "drive_not_configured");

        set_up(&panel).await;
        let refused = send(
            &panel,
            as_user(posted("PUT", &path, serde_json::json!({ "target": "drive" })), &cookie),
        )
        .await;
        assert_eq!(refused.status(), StatusCode::CONFLICT);
        assert_eq!(body_json(refused).await["error"], "drive_not_connected");

        let nonsense = send(
            &panel,
            as_user(posted("PUT", &path, serde_json::json!({ "target": "dropbox" })), &cookie),
        )
        .await;
        assert_eq!(nonsense.status(), StatusCode::BAD_REQUEST);
    }

    #[test]
    fn the_lines_that_make_this_area_run_are_in_main() {
        const MAIN: &str = include_str!("../main.rs");

        for line in [
            "mod drive;",
            "drive::Drive::new(pool.clone()",
            "drive.start();",
            "api::drive::router(Arc::clone(&drive))",
        ] {
            assert!(MAIN.contains(line), "main.rs no longer carries `{line}`");
        }
    }

    #[tokio::test]
    async fn every_path_of_section_22_is_answered_by_something() {
        let panel = panel().await;
        let root = an_admin(&panel.pool, "root").await;
        let cookie = sign_in(&panel.pool, root).await;
        let server = a_server(&panel.pool, root, "Survival", 2048).await;
        let nobody = Id::new();

        let calls: Vec<(&str, String, serde_json::Value)> = vec![
            ("GET", "/api/v1/drive".to_owned(), serde_json::Value::Null),
            ("DELETE", "/api/v1/drive".to_owned(), serde_json::Value::Null),
            ("GET", "/api/v1/drive/link".to_owned(), serde_json::Value::Null),
            ("POST", "/api/v1/drive/link".to_owned(), serde_json::Value::Null),
            ("DELETE", "/api/v1/drive/link".to_owned(), serde_json::Value::Null),
            ("POST", "/api/v1/drive/check".to_owned(), serde_json::Value::Null),
            ("GET", "/api/v1/admin/drive".to_owned(), serde_json::Value::Null),
            (
                "PUT",
                "/api/v1/admin/drive".to_owned(),
                serde_json::json!({
                    "target_policy": "user_choice",
                    "folder_name": "craftpanel-backups",
                }),
            ),
            ("DELETE", "/api/v1/admin/drive/credentials".to_owned(), serde_json::Value::Null),
            ("DELETE", format!("/api/v1/admin/drive/{nobody}"), serde_json::Value::Null),
            ("GET", format!("/api/v1/servers/{server}/backups/target"), serde_json::Value::Null),
            (
                "PUT",
                format!("/api/v1/servers/{server}/backups/target"),
                serde_json::json!({ "target": "local" }),
            ),
        ];

        for (method, path, body) in calls {
            let request = if body.is_null() {
                as_user(empty(method, &path), &cookie)
            } else {
                as_user(posted(method, &path, body), &cookie)
            };
            let response = send(&panel, request).await;
            let status = response.status();
            assert_ne!(
                status,
                StatusCode::METHOD_NOT_ALLOWED,
                "{method} {path} is mounted under another method"
            );

            if status == StatusCode::NOT_FOUND {
                let body = body_json(response).await;
                assert!(
                    body.get("error").and_then(serde_json::Value::as_str).is_some(),
                    "{method} {path} is not mounted: {body}"
                );
            }
        }
    }
}
