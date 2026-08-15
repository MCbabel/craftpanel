use std::collections::BTreeMap;
use std::sync::Arc;

use axum::http::{header::LOCATION, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use serde::{Deserialize, Serialize};

use crate::auth::access;
use crate::auth::error::{Failure, Result};
use crate::auth::{Caller, JsonBody, Params};
use crate::model::{
    Id, Operation, OperationAccepted, Permission, PowerAction, PowerState, PowerTarget,
    PropertiesFields, Server, UpdateChannel, UserRef,
};
use crate::servers::manager::{CreateContent, Created, Manager, NewServer, ServerWarning};
use crate::AppState;

pub fn router(manager: Arc<Manager>) -> Router<AppState> {
    Router::new()
        .route("/servers", get(list).post(create))
        .route("/servers/{server}", get(one).patch(amend).delete(remove))
        .route("/servers/{server}/power", post(power))
        .layer(Extension(manager))
        .layer(axum::middleware::from_fn(crate::auth::extract::same_origin))
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Scope {
    #[default]
    Visible,
    All,
}

#[derive(Debug, Default, Deserialize)]
struct ListQuery {
    #[serde(default)]
    scope: Scope,
}

#[derive(Debug, Serialize)]
struct ServerListResponse {
    servers: Vec<Server>,
    users: BTreeMap<Id, UserRef>,
}

#[derive(Debug, Deserialize)]
struct CreateServerRequest {
    name: String,
    #[serde(default)]
    owner_id: Option<Id>,
    memory_mib: u32,
    #[serde(default)]
    port: Option<u16>,
    eula_accepted: bool,
    content: CreateContent,
    #[serde(default)]
    properties: PropertiesFields,
}

#[derive(Debug, Serialize)]
struct CreateServerResponse {
    server: Server,
    operation: Operation,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    warnings: Vec<ServerWarning>,
}

#[derive(Debug, Deserialize)]
struct UpdateServerRequest {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    update_channel: Option<UpdateChannel>,
}

#[derive(Debug, Deserialize)]
struct DeleteQuery {
    #[serde(default = "yes")]
    keep_backups: bool,
}

fn yes() -> bool {
    true
}

#[derive(Debug, Deserialize)]
struct PowerRequest {
    action: PowerAction,
}

#[derive(Debug, Serialize)]
struct PowerResponse {
    power_state: PowerState,
    target: Option<PowerTarget>,
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
                .map_err(|_| unknown_server())?;
        raw.parse().map(Self).map_err(|_| unknown_server())
    }
}

fn unknown_server() -> Failure {
    Failure::not_found("server_not_found", "no such server")
}

async fn list(
    caller: Caller,
    Extension(manager): Extension<Arc<Manager>>,
    Params(query): Params<ListQuery>,
) -> Result<Json<ServerListResponse>> {
    let everything = query.scope == Scope::All;
    if everything && !caller.is_admin() {
        return Err(Failure::forbidden());
    }

    let (servers, users) = manager.list(&caller, everything).await?;
    Ok(Json(ServerListResponse { servers, users }))
}

async fn create(
    caller: Caller,
    Extension(manager): Extension<Arc<Manager>>,
    JsonBody(body): JsonBody<CreateServerRequest>,
) -> Result<Response> {
    if !body.eula_accepted {
        return Err(Failure::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "eula_not_accepted",
            "Mojang's EULA has to be accepted before a server can be made",
        ));
    }
    if (body.owner_id.is_some() || body.port.is_some()) && !caller.is_admin() {
        return Err(Failure::forbidden());
    }

    let owner_id = body.owner_id.unwrap_or_else(|| caller.id());
    let Created { server, operation, warnings } = manager
        .create(
            &caller,
            NewServer {
                name: body.name,
                owner_id,
                memory_mib: body.memory_mib,
                port: body.port,
                content: body.content,
                properties: body.properties,
            },
        )
        .await?;

    let location = format!("/api/v1/servers/{}", server.id);
    Ok((
        StatusCode::CREATED,
        [(LOCATION, location)],
        Json(CreateServerResponse { server, operation, warnings }),
    )
        .into_response())
}

async fn one(
    caller: Caller,
    Extension(manager): Extension<Arc<Manager>>,
    OfServer(server): OfServer,
) -> Result<Json<Server>> {
    let access = access::require(manager.pool(), &caller, server, Permission::BaseRead).await?;
    Ok(Json(manager.read(server, access.permissions).await?))
}

async fn amend(
    caller: Caller,
    Extension(manager): Extension<Arc<Manager>>,
    OfServer(server): OfServer,
    JsonBody(body): JsonBody<UpdateServerRequest>,
) -> Result<Json<Server>> {
    let access = access::require(manager.pool(), &caller, server, Permission::Advanced).await?;
    if body.name.is_none() && body.update_channel.is_none() {
        return Err(Failure::invalid_request("name or update_channel, and this is neither"));
    }

    let mut object =
        manager.amend(&caller, server, body.name.as_deref(), body.update_channel).await?;
    object.current_user_permissions = access.permissions;
    Ok(Json(object))
}

async fn remove(
    caller: Caller,
    Extension(manager): Extension<Arc<Manager>>,
    OfServer(server): OfServer,
    Params(query): Params<DeleteQuery>,
) -> Result<(StatusCode, Json<OperationAccepted>)> {
    access::of(manager.pool(), &caller, server).await?.require_ownership(&caller)?;
    let operation = manager.delete(&caller, server, query.keep_backups).await?;
    Ok((StatusCode::ACCEPTED, Json(OperationAccepted { operation })))
}

async fn power(
    caller: Caller,
    Extension(manager): Extension<Arc<Manager>>,
    OfServer(server): OfServer,
    JsonBody(body): JsonBody<PowerRequest>,
) -> Result<(StatusCode, Json<PowerResponse>)> {
    access::require(manager.pool(), &caller, server, Permission::PowerActions).await?;
    let (power_state, target) = manager.power(&caller, server, body.action).await?;
    Ok((StatusCode::ACCEPTED, Json(PowerResponse { power_state, target })))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::harness::{
        a_user, an_admin, as_user, body_json, empty, fetch, send, sign_in, state_with, test_pool,
        FakeHelper,
    };
    use crate::auth::Disks;
    use crate::config::Config;
    use crate::model::{Permissions, ServerRole, Timestamp};
    use crate::ops::testing::DataDir;
    use crate::ops::Operations;
    use crate::servers::manager::fake::Shelf;
    use crate::servers::Hub;
    use axum::body::Body;
    use axum::http::Request;
    use sqlx::SqlitePool;
    use tower::ServiceExt;

    struct Panel {
        app: Router,
        pool: SqlitePool,
        manager: Arc<Manager>,
        _helper: FakeHelper,
        _dir: DataDir,
    }

    impl Panel {
        async fn new() -> Self {
            let dir = DataDir::new();
            let pool = test_pool().await;
            let helper = FakeHelper::obliging().await;
            let config = Arc::new(Config {
                data_dir: dir.path().to_path_buf(),
                helper_socket: helper.socket(),
                ..Config::default()
            });
            let operations = Operations::new(pool.clone(), dir.path());
            let manager = Manager::new(
                pool.clone(),
                Arc::clone(&config),
                operations,
                Arc::new(Hub::new(dir.path().join("supervisors.sock"))),
                crate::helper::Helper::new(helper.socket()),
                Shelf::new(),
                Disks::none(),
            );
            let app = router(Arc::clone(&manager))
                .with_state(state_with(&pool, (*config).clone()));
            Self { app, pool, manager, _helper: helper, _dir: dir }
        }

        async fn call(&self, request: Request<Body>) -> axum::response::Response {
            self.app.clone().oneshot(request).await.expect("the router answers")
        }

        async fn as_who(&self, user: Id) -> String {
            sign_in(&self.pool, user).await
        }

        async fn a_ready_server(&self, owner: Id, name: &str) -> Id {
            let caller = self.caller(owner).await;
            let made = self
                .manager
                .create(
                    &caller,
                    crate::servers::manager::NewServer {
                        name: name.to_owned(),
                        owner_id: owner,
                        memory_mib: 1024,
                        port: None,
                        content: CreateContent::Loader {
                            loader: crate::model::LoaderId::Paper,
                            game_version: "1.21.8".to_owned(),
                            loader_version: None,
                        },
                        properties: PropertiesFields::default(),
                    },
                )
                .await
                .expect("a server");
            self.manager.run(made.operation.id).await;
            made.server.id
        }

        async fn caller(&self, user: Id) -> Caller {
            let secret = sign_in(&self.pool, user).await;
            Caller {
                user: crate::auth::users::load(&self.pool, user).await.expect("the user"),
                session: crate::auth::session::lookup(&self.pool, &secret, Timestamp::now())
                    .await
                    .expect("a session")
                    .expect("the session"),
                secure: false,
            }
        }

        async fn share(&self, server: Id, with: Id, role: ServerRole) {
            let now = Timestamp::now();
            sqlx::query(
                "INSERT INTO server_members (id, server_id, user_id, role, invited_at, joined_at)
                 VALUES (?, ?, ?, ?, ?, ?)",
            )
            .bind(Id::new())
            .bind(server)
            .bind(with)
            .bind(role)
            .bind(now)
            .bind(now)
            .execute(&self.pool)
            .await
            .expect("a membership");
        }
    }

    fn a_creation(name: &str) -> serde_json::Value {
        serde_json::json!({
            "name": name,
            "owner_id": null,
            "memory_mib": 1024,
            "port": null,
            "eula_accepted": true,
            "content": {
                "kind": "loader",
                "loader": "paper",
                "game_version": "1.21.8",
                "loader_version": null
            },
            "properties": { "known": {} }
        })
    }

    #[tokio::test]
    async fn the_list_carries_your_own_the_shared_and_the_names_behind_them() {
        let panel = Panel::new().await;
        let max = a_user(&panel.pool, "max").await;
        let anna = a_user(&panel.pool, "anna").await;
        let his = panel.a_ready_server(max, "his").await;
        let shared = panel.a_ready_server(anna, "shared").await;
        let _hers = panel.a_ready_server(anna, "hers").await;
        panel.share(shared, max, ServerRole::Viewer).await;

        let cookie = panel.as_who(max).await;
        let answer = panel.call(as_user(fetch("/servers"), &cookie)).await;
        assert_eq!(answer.status(), StatusCode::OK);
        let body = body_json(answer).await;

        let ids: Vec<&str> =
            body["servers"].as_array().unwrap().iter().map(|s| s["id"].as_str().unwrap()).collect();
        assert_eq!(ids.len(), 2, "his own and the one he was let into: {body}");
        assert!(ids.contains(&his.to_string().as_str()));
        assert!(ids.contains(&shared.to_string().as_str()));

        assert_eq!(body["users"][max.to_string()]["username"], "max");
        assert_eq!(body["users"][anna.to_string()]["username"], "anna");

        let mine: &serde_json::Value =
            body["servers"].as_array().unwrap().iter().find(|s| s["id"] == his.to_string()).unwrap();
        assert_eq!(mine["current_user_permissions"], "SERVER_ADMIN");
        let theirs: &serde_json::Value = body["servers"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["id"] == shared.to_string())
            .unwrap();
        assert_eq!(theirs["current_user_permissions"], "BASE_READ | POWER_ACTIONS");
    }

    #[tokio::test]
    async fn everything_on_the_machine_is_for_panel_admins_only() {
        let panel = Panel::new().await;
        let max = a_user(&panel.pool, "max").await;
        let anna = an_admin(&panel.pool, "anna").await;
        panel.a_ready_server(max, "his").await;

        let his = panel.as_who(max).await;
        let refused = panel.call(as_user(fetch("/servers?scope=all"), &his)).await;
        assert_eq!(refused.status(), StatusCode::FORBIDDEN);
        assert_eq!(body_json(refused).await["error"], "forbidden");

        let hers = panel.as_who(anna).await;
        let allowed = panel.call(as_user(fetch("/servers?scope=all"), &hers)).await;
        assert_eq!(allowed.status(), StatusCode::OK);
        assert_eq!(body_json(allowed).await["servers"].as_array().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn making_one_answers_201_with_the_server_the_run_and_where_to_find_it() {
        let panel = Panel::new().await;
        let max = a_user(&panel.pool, "max").await;
        let cookie = panel.as_who(max).await;

        let answer =
            panel.call(as_user(send("POST", "/servers", a_creation("Survival")), &cookie)).await;
        assert_eq!(answer.status(), StatusCode::CREATED);
        let location =
            answer.headers().get(LOCATION).expect("4.2 sends one").to_str().unwrap().to_owned();
        let body = body_json(answer).await;

        assert_eq!(body["server"]["name"], "Survival");
        assert_eq!(body["server"]["status"], "installing");
        assert_eq!(body["server"]["owner_id"], max.to_string());
        assert_eq!(body["server"]["net"]["port"], 25565);
        assert_eq!(body["server"]["game"], "Minecraft");
        assert_eq!(body["operation"]["kind"], "server_create");
        assert_eq!(body["operation"]["state"], "queued");
        assert_eq!(location, format!("/api/v1/servers/{}", body["server"]["id"].as_str().unwrap()));
        assert!(body.get("warnings").is_none(), "no warnings, no field");
    }

    #[tokio::test]
    async fn choosing_an_owner_or_a_port_is_the_administrator_s_alone() {
        let panel = Panel::new().await;
        let max = a_user(&panel.pool, "max").await;
        let anna = an_admin(&panel.pool, "anna").await;
        let his = panel.as_who(max).await;

        let mut asking_for_someone_else = a_creation("theirs");
        asking_for_someone_else["owner_id"] = serde_json::json!(anna.to_string());
        let refused = panel
            .call(as_user(send("POST", "/servers", asking_for_someone_else.clone()), &his))
            .await;
        assert_eq!(refused.status(), StatusCode::FORBIDDEN);

        let mut asking_for_a_port = a_creation("mine");
        asking_for_a_port["port"] = serde_json::json!(30000);
        let also_refused =
            panel.call(as_user(send("POST", "/servers", asking_for_a_port.clone()), &his)).await;
        assert_eq!(also_refused.status(), StatusCode::FORBIDDEN);

        let hers = panel.as_who(anna).await;
        let allowed = panel.call(as_user(send("POST", "/servers", asking_for_a_port), &hers)).await;
        assert_eq!(allowed.status(), StatusCode::CREATED);
        assert_eq!(body_json(allowed).await["server"]["net"]["port"], 30000);
    }

    #[tokio::test]
    async fn nothing_is_made_before_the_eula_is_accepted() {
        let panel = Panel::new().await;
        let max = a_user(&panel.pool, "max").await;
        let cookie = panel.as_who(max).await;
        let mut body = a_creation("Survival");
        body["eula_accepted"] = serde_json::json!(false);

        let refused = panel.call(as_user(send("POST", "/servers", body), &cookie)).await;
        assert_eq!(refused.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(body_json(refused).await["error"], "eula_not_accepted");

        let count: i64 =
            sqlx::query_scalar("SELECT count(*) FROM servers").fetch_one(&panel.pool).await.unwrap();
        assert_eq!(count, 0);
    }

    #[tokio::test]
    async fn a_server_you_may_not_see_does_not_exist() {
        let panel = Panel::new().await;
        let max = a_user(&panel.pool, "max").await;
        let anna = a_user(&panel.pool, "anna").await;
        let his = panel.a_ready_server(max, "his").await;

        let hers = panel.as_who(anna).await;
        let answer = panel.call(as_user(fetch(&format!("/servers/{his}")), &hers)).await;
        assert_eq!(answer.status(), StatusCode::NOT_FOUND, "1.7: 403 would hand out the id");
        assert_eq!(body_json(answer).await["error"], "server_not_found");
    }

    #[tokio::test]
    async fn renaming_takes_advanced_and_a_viewer_has_none() {
        let panel = Panel::new().await;
        let max = a_user(&panel.pool, "max").await;
        let anna = a_user(&panel.pool, "anna").await;
        let bea = a_user(&panel.pool, "bea").await;
        let server = panel.a_ready_server(max, "old").await;
        panel.share(server, anna, ServerRole::Viewer).await;
        panel.share(server, bea, ServerRole::Editor).await;

        let viewer = panel.as_who(anna).await;
        let refused = panel
            .call(as_user(
                send("PATCH", &format!("/servers/{server}"), serde_json::json!({ "name": "new" })),
                &viewer,
            ))
            .await;
        assert_eq!(refused.status(), StatusCode::FORBIDDEN);

        let editor = panel.as_who(bea).await;
        let allowed = panel
            .call(as_user(
                send(
                    "PATCH",
                    &format!("/servers/{server}"),
                    serde_json::json!({ "name": "new", "update_channel": "beta" }),
                ),
                &editor,
            ))
            .await;
        assert_eq!(allowed.status(), StatusCode::OK);
        let body = body_json(allowed).await;
        assert_eq!(body["name"], "new");
        assert_eq!(body["update_channel"], "beta");

        let (action, metadata): (String, Option<String>) = sqlx::query_as(
            "SELECT action, metadata FROM audit_log WHERE action = 'changed_server_name'",
        )
        .fetch_one(&panel.pool)
        .await
        .unwrap();
        assert_eq!(action, "changed_server_name");
        assert!(metadata.unwrap().contains("new"));
    }

    #[tokio::test]
    async fn switching_the_channel_forgets_when_updates_were_last_checked() {
        let panel = Panel::new().await;
        let max = a_user(&panel.pool, "max").await;
        let server = panel.a_ready_server(max, "his").await;
        let cookie = panel.as_who(max).await;

        let checked = Timestamp::now();
        sqlx::query("UPDATE servers SET updates_checked_at = ? WHERE id = ?")
            .bind(checked)
            .bind(server)
            .execute(&panel.pool)
            .await
            .unwrap();

        let renamed = panel
            .call(as_user(
                send("PATCH", &format!("/servers/{server}"), serde_json::json!({ "name": "same" })),
                &cookie,
            ))
            .await;
        assert_eq!(renamed.status(), StatusCode::OK);
        let kept: Option<Timestamp> =
            sqlx::query_scalar("SELECT updates_checked_at FROM servers WHERE id = ?")
                .bind(server)
                .fetch_one(&panel.pool)
                .await
                .unwrap();
        assert_eq!(kept, Some(checked), "a rename says nothing about updates");

        let switched = panel
            .call(as_user(
                send(
                    "PATCH",
                    &format!("/servers/{server}"),
                    serde_json::json!({ "update_channel": "alpha" }),
                ),
                &cookie,
            ))
            .await;
        assert_eq!(switched.status(), StatusCode::OK);
        assert_eq!(body_json(switched).await["update_channel"], "alpha");

        let cleared: Option<Timestamp> =
            sqlx::query_scalar("SELECT updates_checked_at FROM servers WHERE id = ?")
                .bind(server)
                .fetch_one(&panel.pool)
                .await
                .unwrap();
        assert!(cleared.is_none(), "the next content read has to check again");

        sqlx::query("UPDATE servers SET updates_checked_at = ? WHERE id = ?")
            .bind(checked)
            .bind(server)
            .execute(&panel.pool)
            .await
            .unwrap();
        let again = panel
            .call(as_user(
                send(
                    "PATCH",
                    &format!("/servers/{server}"),
                    serde_json::json!({ "name": "other", "update_channel": "alpha" }),
                ),
                &cookie,
            ))
            .await;
        assert_eq!(again.status(), StatusCode::OK);
        let untouched: Option<Timestamp> =
            sqlx::query_scalar("SELECT updates_checked_at FROM servers WHERE id = ?")
                .bind(server)
                .fetch_one(&panel.pool)
                .await
                .unwrap();
        assert_eq!(untouched, Some(checked), "the same channel is no switch");
    }

    #[tokio::test]
    async fn an_empty_patch_is_a_bad_request_and_a_bad_name_is_invalid_name() {
        let panel = Panel::new().await;
        let max = a_user(&panel.pool, "max").await;
        let server = panel.a_ready_server(max, "old").await;
        let cookie = panel.as_who(max).await;

        let nothing = panel
            .call(as_user(
                send("PATCH", &format!("/servers/{server}"), serde_json::json!({})),
                &cookie,
            ))
            .await;
        assert_eq!(nothing.status(), StatusCode::BAD_REQUEST);
        assert_eq!(body_json(nothing).await["error"], "invalid_request");

        let empty_name = panel
            .call(as_user(
                send("PATCH", &format!("/servers/{server}"), serde_json::json!({ "name": " " })),
                &cookie,
            ))
            .await;
        assert_eq!(empty_name.status(), StatusCode::BAD_REQUEST);
        assert_eq!(body_json(empty_name).await["error"], "invalid_name");
    }

    #[tokio::test]
    async fn only_the_owner_or_a_panel_admin_may_delete() {
        let panel = Panel::new().await;
        let max = a_user(&panel.pool, "max").await;
        let anna = a_user(&panel.pool, "anna").await;
        let server = panel.a_ready_server(max, "his").await;
        panel.share(server, anna, ServerRole::Editor).await;

        let editor = panel.as_who(anna).await;
        let refused =
            panel.call(as_user(empty("DELETE", &format!("/servers/{server}")), &editor)).await;
        assert_eq!(refused.status(), StatusCode::FORBIDDEN, "4.5: no server bit does this");

        let owner = panel.as_who(max).await;
        let accepted =
            panel.call(as_user(empty("DELETE", &format!("/servers/{server}")), &owner)).await;
        assert_eq!(accepted.status(), StatusCode::ACCEPTED);
        assert_eq!(body_json(accepted).await["operation"]["kind"], "server_delete");
    }

    #[tokio::test]
    async fn a_viewer_may_press_start_and_a_stranger_may_not_even_ask() {
        let panel = Panel::new().await;
        let max = a_user(&panel.pool, "max").await;
        let anna = a_user(&panel.pool, "anna").await;
        let bea = a_user(&panel.pool, "bea").await;
        let server = panel.a_ready_server(max, "his").await;
        panel.share(server, anna, ServerRole::Viewer).await;

        let viewer = panel.as_who(anna).await;
        let started = panel
            .call(as_user(
                send(
                    "POST",
                    &format!("/servers/{server}/power"),
                    serde_json::json!({ "action": "start" }),
                ),
                &viewer,
            ))
            .await;
        assert_eq!(started.status(), StatusCode::ACCEPTED, "2.1: a viewer holds POWER_ACTIONS");
        let body = body_json(started).await;
        assert_eq!(body["power_state"], "starting");
        assert_eq!(body["target"], "start");

        let stranger = panel.as_who(bea).await;
        let refused = panel
            .call(as_user(
                send(
                    "POST",
                    &format!("/servers/{server}/power"),
                    serde_json::json!({ "action": "stop" }),
                ),
                &stranger,
            ))
            .await;
        assert_eq!(refused.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn an_action_nobody_knows_is_a_bad_request_and_not_a_panic() {
        let panel = Panel::new().await;
        let max = a_user(&panel.pool, "max").await;
        let server = panel.a_ready_server(max, "his").await;
        let cookie = panel.as_who(max).await;

        let answer = panel
            .call(as_user(
                send(
                    "POST",
                    &format!("/servers/{server}/power"),
                    serde_json::json!({ "action": "explode" }),
                ),
                &cookie,
            ))
            .await;
        assert_eq!(answer.status(), StatusCode::BAD_REQUEST);
        assert_eq!(body_json(answer).await["error"], "invalid_request");
    }

    #[tokio::test]
    async fn without_a_session_every_one_of_them_is_401() {
        let panel = Panel::new().await;
        let max = a_user(&panel.pool, "max").await;
        let server = panel.a_ready_server(max, "his").await;

        for request in [
            fetch("/servers"),
            send("POST", "/servers", a_creation("Survival")),
            fetch(&format!("/servers/{server}")),
            send("PATCH", &format!("/servers/{server}"), serde_json::json!({ "name": "x" })),
            empty("DELETE", &format!("/servers/{server}")),
            send(
                "POST",
                &format!("/servers/{server}/power"),
                serde_json::json!({ "action": "start" }),
            ),
        ] {
            let answer = panel.call(request).await;
            assert_eq!(answer.status(), StatusCode::UNAUTHORIZED);
        }
    }

    #[tokio::test]
    async fn a_form_post_from_a_foreign_page_gets_nowhere() {
        let panel = Panel::new().await;
        let max = a_user(&panel.pool, "max").await;
        let cookie = panel.as_who(max).await;

        let mut request = as_user(send("POST", "/servers", a_creation("Survival")), &cookie);
        request.headers_mut().insert(axum::http::header::ORIGIN, "https://evil.example".parse().unwrap());
        request.headers_mut().insert(axum::http::header::HOST, "panel.example".parse().unwrap());

        let answer = panel.call(request).await;
        assert_eq!(answer.status(), StatusCode::FORBIDDEN);
        assert_eq!(body_json(answer).await["error"], "csrf_origin_mismatch");
    }

    #[tokio::test]
    async fn a_path_segment_that_is_no_ulid_is_answered_in_the_shape_of_1_7() {
        let panel = Panel::new().await;
        let max = a_user(&panel.pool, "max").await;
        let cookie = panel.as_who(max).await;

        for path in ["/servers/not-a-ulid", "/servers/..%2F..%2Fetc%2Fpasswd", "/servers/%2E%2E"] {
            for request in [
                fetch(path),
                send("PATCH", path, serde_json::json!({ "name": "x" })),
                empty("DELETE", path),
                send("POST", &format!("{path}/power"), serde_json::json!({ "action": "start" })),
            ] {
                let answer = panel.call(as_user(request, &cookie)).await;
                assert_eq!(answer.status(), StatusCode::NOT_FOUND, "{path}");
                let kind = answer
                    .headers()
                    .get(axum::http::header::CONTENT_TYPE)
                    .and_then(|value| value.to_str().ok())
                    .unwrap_or_default()
                    .to_owned();
                assert!(kind.starts_with("application/json"), "{path}: {kind}");
                let body = body_json(answer).await;
                assert_eq!(body["error"], "server_not_found", "{path}");
                assert_eq!(body.as_object().unwrap().len(), 2, "1.7 allows exactly two fields");
            }
        }
    }

    #[tokio::test]
    async fn renaming_is_refused_while_the_set_up_holds_the_server() {
        let panel = Panel::new().await;
        let max = a_user(&panel.pool, "max").await;
        let cookie = panel.as_who(max).await;
        let made =
            panel.call(as_user(send("POST", "/servers", a_creation("Survival")), &cookie)).await;
        let id = body_json(made).await["server"]["id"].as_str().unwrap().to_owned();

        let refused = panel
            .call(as_user(
                send("PATCH", &format!("/servers/{id}"), serde_json::json!({ "name": "new" })),
                &cookie,
            ))
            .await;
        assert_eq!(refused.status(), StatusCode::CONFLICT);
        assert_eq!(body_json(refused).await["error"], "server_busy");

        let server: Id = id.parse().unwrap();
        let operation: Id = sqlx::query_scalar("SELECT id FROM operations WHERE server_id = ?")
            .bind(server)
            .fetch_one(&panel.pool)
            .await
            .unwrap();
        panel.manager.run(operation).await;

        let allowed = panel
            .call(as_user(
                send("PATCH", &format!("/servers/{id}"), serde_json::json!({ "name": "new" })),
                &cookie,
            ))
            .await;
        assert_eq!(allowed.status(), StatusCode::OK);
        assert_eq!(body_json(allowed).await["name"], "new");
    }

    #[tokio::test]
    async fn the_answer_of_4_3_carries_the_readers_own_mask() {
        let panel = Panel::new().await;
        let max = a_user(&panel.pool, "max").await;
        let anna = a_user(&panel.pool, "anna").await;
        let server = panel.a_ready_server(max, "his").await;
        panel.share(server, anna, ServerRole::Editor).await;

        let cookie = panel.as_who(anna).await;
        let answer = panel.call(as_user(fetch(&format!("/servers/{server}")), &cookie)).await;
        assert_eq!(answer.status(), StatusCode::OK);
        let body = body_json(answer).await;
        assert_eq!(
            body["current_user_permissions"],
            Permissions::from_role(ServerRole::Editor).to_string()
        );
        assert_eq!(body["backup_quota"], 10, "the panel default of 12.10");
        assert_eq!(body["used_backup_quota"], 0);
        assert_eq!(body["upstream"], serde_json::Value::Null);
        assert_eq!(body["net"]["domain"], "");
    }
}
