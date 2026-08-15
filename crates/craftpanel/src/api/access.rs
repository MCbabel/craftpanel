use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

use crate::audit::{self, Event};
use crate::auth::access::{self, Access};
use crate::auth::error::{Failure, Result};
use crate::auth::{extract, users, Caller, JsonBody, Params};
use crate::model::{
    Id, Invitation, PanelRole, Permission, Permissions, ServerMember, ServerRef, ServerRole,
    Timestamp, UserRef,
};
use crate::AppState;

const RESEND_COOLDOWN: time::Duration = time::Duration::seconds(120);

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/servers/{server}/members", get(list_members).post(add_member))
        .route(
            "/servers/{server}/members/{user}",
            axum::routing::patch(update_member).delete(remove_member),
        )
        .route("/servers/{server}/members/{user}/reinvite", post(reinvite))
        .route("/servers/{server}/audit-log", get(audit_log))
        .route("/invitations", get(invitations))
        .route("/invitations/{invitation}/accept", post(accept))
        .route("/invitations/{invitation}/decline", post(decline))
        .layer(axum::middleware::from_fn(extract::same_origin))
}

#[derive(Serialize)]
struct ServerMemberList {
    members: Vec<ServerMember>,
}

#[derive(Deserialize)]
struct AddMemberRequest {
    user_id: String,
    role: String,
}

#[derive(Deserialize)]
struct UpdateMemberRequest {
    role: String,
}

#[derive(Serialize)]
struct ReinviteResponse {
    sent: bool,
    cooldown_seconds: Option<u32>,
    member: ServerMember,
}

#[derive(Serialize)]
struct InvitationList {
    invitations: Vec<Invitation>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct MemberRow {
    id: Id,
    user_id: Id,
    username: String,
    role: ServerRole,
    invited_at: Timestamp,
    joined_at: Option<Timestamp>,
    last_invite_sent: Option<Timestamp>,
}

const MEMBERS: &str = "SELECT m.id, m.user_id, u.username, m.role, m.invited_at, m.joined_at, \
                       m.last_invite_sent FROM server_members m \
                       JOIN users u ON u.id = m.user_id AND u.role <> 'admin' \
                       JOIN servers s ON s.id = m.server_id AND s.owner_id <> m.user_id";

impl MemberRow {
    fn public(&self) -> ServerMember {
        ServerMember {
            id: self.id,
            user: UserRef {
                id: self.user_id,
                username: self.username.clone(),
                avatar_url: None,
            },
            role: self.role,
            permissions: Permissions::from_role(self.role),
            joined_at: self.joined_at,
            invited_at: self.invited_at,
            last_invite_sent: self.last_invite_sent,
            invite_resend_available_at: self.last_invite_sent.map(resend_at),
            pending: self.joined_at.is_none(),
            is_owner: false,
        }
    }
}

async fn list_members(
    State(state): State<AppState>,
    caller: Caller,
    Path(server): Path<String>,
) -> Result<Json<ServerMemberList>> {
    let server = parse_server(&server)?;
    let access = access::require(&state.pool, &caller, server, Permission::BaseRead).await?;

    let mut members = vec![owner_entry(&state.pool, access).await?];
    for row in member_rows(&state.pool, server).await? {
        members.push(row.public());
    }
    Ok(Json(ServerMemberList { members }))
}

async fn add_member(
    State(state): State<AppState>,
    caller: Caller,
    Path(server): Path<String>,
    JsonBody(body): JsonBody<AddMemberRequest>,
) -> Result<Response> {
    let server = parse_server(&server)?;
    let access = access::require(&state.pool, &caller, server, Permission::ManageUsers).await?;
    let role = assignable_role(&body.role)?;
    let target: Id = body.user_id.parse().map_err(|_| unknown_user())?;

    if target == caller.id() {
        return Err(Failure::bad_request("cannot_invite_self", "you are already here"));
    }
    let user = users::find(&state.pool, target).await?.ok_or_else(unknown_user)?;
    if target == access.owner_id {
        return Err(already_member());
    }
    if user.role == PanelRole::Admin {
        return Err(Failure::conflict(
            "already_member",
            "a panel administrator already has every right on this server",
        ));
    }

    let now = Timestamp::now();
    let id = Id::new();
    sqlx::query(
        "INSERT INTO server_members \
         (id, server_id, user_id, role, invited_by, invited_at, last_invite_sent) \
         VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(id)
    .bind(server)
    .bind(target)
    .bind(role)
    .bind(caller.id())
    .bind(now)
    .bind(now)
    .execute(&state.pool)
    .await
    .map_err(map_already_member)?;

    audit::record(
        &state.pool,
        access,
        &caller,
        Event::UserInvited { user: target, permissions: Permissions::from_role(role) },
    )
    .await;

    let member = MemberRow {
        id,
        user_id: target,
        username: user.username,
        role,
        invited_at: now,
        joined_at: None,
        last_invite_sent: Some(now),
    };
    Ok((StatusCode::CREATED, Json(member.public())).into_response())
}

async fn update_member(
    State(state): State<AppState>,
    caller: Caller,
    Path((server, user)): Path<(String, String)>,
    JsonBody(body): JsonBody<UpdateMemberRequest>,
) -> Result<Json<ServerMember>> {
    let server = parse_server(&server)?;
    let access = access::require(&state.pool, &caller, server, Permission::ManageUsers).await?;
    let role = assignable_role(&body.role)?;
    let target = parse_member(&user)?;

    let row = member(&state.pool, server, target).await?;
    sqlx::query("UPDATE server_members SET role = ? WHERE id = ?")
        .bind(role)
        .bind(row.id)
        .execute(&state.pool)
        .await?;

    audit::record(
        &state.pool,
        access,
        &caller,
        Event::UserPermissionModified { user: target, permissions: Permissions::from_role(role) },
    )
    .await;

    Ok(Json(MemberRow { role, ..row }.public()))
}

async fn remove_member(
    State(state): State<AppState>,
    caller: Caller,
    Path((server, user)): Path<(String, String)>,
) -> Result<StatusCode> {
    let server = parse_server(&server)?;
    let access = access::of(&state.pool, &caller, server).await?.require(Permission::BaseRead)?;
    let target = parse_member(&user)?;
    if target != caller.id() {
        access.require(Permission::ManageUsers)?;
    }

    if target == access.owner_id {
        return Err(Failure::bad_request(
            "cannot_remove_owner",
            "the owner keeps his own server; hand it over instead",
        ));
    }

    let row = member(&state.pool, server, target).await?;
    let gone = sqlx::query("DELETE FROM server_members WHERE id = ?")
        .bind(row.id)
        .execute(&state.pool)
        .await?;
    if gone.rows_affected() == 0 {
        return Err(Failure::not_found("member_not_found", "no such member on this server"));
    }

    let event = match row.joined_at {
        Some(_) => Event::UserRemoved { user: target },
        None => Event::UserInviteRevoked { user: target },
    };
    audit::record(&state.pool, access, &caller, event).await;

    Ok(StatusCode::NO_CONTENT)
}

async fn reinvite(
    State(state): State<AppState>,
    caller: Caller,
    Path((server, user)): Path<(String, String)>,
) -> Result<Json<ReinviteResponse>> {
    let server = parse_server(&server)?;
    access::require(&state.pool, &caller, server, Permission::ManageUsers).await?;
    let target = parse_member(&user)?;

    let row = member(&state.pool, server, target).await?;
    if row.joined_at.is_some() {
        return Err(already_member());
    }

    let now = Timestamp::now();
    if let Some(available_at) = row.last_invite_sent.map(resend_at).filter(|at| *at > now) {
        let left = available_at.unix_seconds() - now.unix_seconds();
        return Ok(Json(ReinviteResponse {
            sent: false,
            cooldown_seconds: Some(left.max(0) as u32),
            member: row.public(),
        }));
    }

    sqlx::query("UPDATE server_members SET last_invite_sent = ? WHERE id = ?")
        .bind(now)
        .bind(row.id)
        .execute(&state.pool)
        .await?;

    Ok(Json(ReinviteResponse {
        sent: true,
        cooldown_seconds: Some(RESEND_COOLDOWN.whole_seconds() as u32),
        member: MemberRow { last_invite_sent: Some(now), ..row }.public(),
    }))
}

async fn invitations(
    State(state): State<AppState>,
    caller: Caller,
) -> Result<Json<InvitationList>> {
    if caller.is_admin() {
        return Ok(Json(InvitationList { invitations: Vec::new() }));
    }

    let rows: Vec<(Id, Id, String, ServerRole, Timestamp, Option<Timestamp>, Id, String)> =
        sqlx::query_as(
            "SELECT m.id, s.id, s.name, m.role, m.invited_at, m.last_invite_sent, \
             coalesce(m.invited_by, s.owner_id), coalesce(inviter.username, owner.username) \
             FROM server_members m \
             JOIN servers s ON s.id = m.server_id \
             JOIN users owner ON owner.id = s.owner_id \
             LEFT JOIN users inviter ON inviter.id = m.invited_by \
             WHERE m.user_id = ? AND m.joined_at IS NULL AND s.owner_id <> m.user_id \
             ORDER BY m.id DESC",
        )
        .bind(caller.id())
        .fetch_all(&state.pool)
        .await?;

    let invitations = rows
        .into_iter()
        .map(|(id, server_id, name, role, invited_at, last_invite_sent, by, by_name)| Invitation {
            id,
            server: ServerRef { id: server_id, name },
            role,
            invited_by: UserRef { id: by, username: by_name, avatar_url: None },
            invited_at,
            last_invite_sent,
        })
        .collect();
    Ok(Json(InvitationList { invitations }))
}

async fn accept(
    State(state): State<AppState>,
    caller: Caller,
    Path(invitation): Path<String>,
) -> Result<Json<ServerMember>> {
    let row = invitation_of(&state.pool, &caller, &invitation).await?;
    let now = Timestamp::now();
    sqlx::query("UPDATE server_members SET joined_at = ? WHERE id = ?")
        .bind(now)
        .bind(row.id)
        .execute(&state.pool)
        .await?;

    Ok(Json(MemberRow { joined_at: Some(now), ..row }.public()))
}

async fn decline(
    State(state): State<AppState>,
    caller: Caller,
    Path(invitation): Path<String>,
) -> Result<StatusCode> {
    let row = invitation_of(&state.pool, &caller, &invitation).await?;
    sqlx::query("DELETE FROM server_members WHERE id = ?")
        .bind(row.id)
        .execute(&state.pool)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn audit_log(
    State(state): State<AppState>,
    caller: Caller,
    Path(server): Path<String>,
    Params(pairs): Params<Vec<(String, String)>>,
) -> Result<Json<audit::Page>> {
    let server = parse_server(&server)?;
    access::require(&state.pool, &caller, server, Permission::BaseRead).await?;

    let query = audit::Query::read(&pairs)?;
    Ok(Json(audit::page(&state.pool, server, &query).await?))
}

async fn owner_entry(pool: &SqlitePool, access: Access) -> Result<ServerMember> {
    let (username, created_at): (String, Timestamp) = sqlx::query_as(
        "SELECT u.username, s.created_at FROM servers s JOIN users u ON u.id = s.owner_id \
         WHERE s.id = ?",
    )
    .bind(access.server_id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(unknown_server)?;

    Ok(ServerMember {
        id: access.server_id,
        user: UserRef { id: access.owner_id, username, avatar_url: None },
        role: ServerRole::Owner,
        permissions: Permissions::from_role(ServerRole::Owner),
        joined_at: Some(created_at),
        invited_at: created_at,
        last_invite_sent: None,
        invite_resend_available_at: None,
        pending: false,
        is_owner: true,
    })
}

async fn member_rows(pool: &SqlitePool, server: Id) -> Result<Vec<MemberRow>> {
    Ok(
        sqlx::query_as::<_, MemberRow>(&format!("{MEMBERS} WHERE m.server_id = ? ORDER BY m.id"))
            .bind(server)
            .fetch_all(pool)
            .await?,
    )
}

async fn member(pool: &SqlitePool, server: Id, user: Id) -> Result<MemberRow> {
    let sql = format!("{MEMBERS} WHERE m.server_id = ? AND m.user_id = ?");
    sqlx::query_as::<_, MemberRow>(&sql)
        .bind(server)
        .bind(user)
        .fetch_optional(pool)
        .await?
        .ok_or_else(|| Failure::not_found("member_not_found", "no such member on this server"))
}

async fn invitation_of(pool: &SqlitePool, caller: &Caller, id: &str) -> Result<MemberRow> {
    let id: Id = id.parse().map_err(|_| unknown_invitation())?;
    let sql = format!("{MEMBERS} WHERE m.id = ? AND m.user_id = ?");
    let row = sqlx::query_as::<_, MemberRow>(&sql)
        .bind(id)
        .bind(caller.id())
        .fetch_optional(pool)
        .await?
        .ok_or_else(unknown_invitation)?;

    match row.joined_at {
        Some(_) => Err(already_member()),
        None => Ok(row),
    }
}

fn assignable_role(raw: &str) -> Result<ServerRole> {
    match raw.parse() {
        Ok(ServerRole::Owner) => Err(Failure::bad_request(
            "role_not_assignable",
            "owner belongs to the server, not to a membership",
        )),
        Ok(role) => Ok(role),
        Err(_) => Err(Failure::bad_request(
            "invalid_role",
            format!("{raw:?} is neither editor nor viewer"),
        )),
    }
}

fn resend_at(sent: Timestamp) -> Timestamp {
    Timestamp::at(sent.as_datetime() + RESEND_COOLDOWN)
}

fn parse_server(raw: &str) -> Result<Id> {
    raw.parse().map_err(|_| unknown_server())
}

fn parse_member(raw: &str) -> Result<Id> {
    raw.parse().map_err(|_| Failure::not_found("member_not_found", "no such member"))
}

fn unknown_server() -> Failure {
    Failure::not_found("server_not_found", "no such server")
}

fn unknown_user() -> Failure {
    Failure::not_found("user_not_found", "no such user")
}

fn unknown_invitation() -> Failure {
    Failure::not_found("invitation_not_found", "no such invitation")
}

fn already_member() -> Failure {
    Failure::conflict("already_member", "that user is already on this server")
}

fn map_already_member(err: sqlx::Error) -> Failure {
    match &err {
        sqlx::Error::Database(database) if database.is_unique_violation() => already_member(),
        _ => Failure::internal(err),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::harness::*;
    use crate::model::AuditAction;
    use axum::body::Body;
    use axum::http::Request;
    use serde_json::{json, Value};
    use tower::ServiceExt;

    struct Panel {
        pool: SqlitePool,
        server: Id,
        owner: Id,
        owner_key: String,
        editor: Id,
        editor_key: String,
        viewer: Id,
        viewer_key: String,
        stranger_key: String,
    }

    impl Panel {
        async fn open() -> Self {
            let pool = test_pool().await;
            let owner = a_user(&pool, "max").await;
            let editor = a_user(&pool, "anna").await;
            let viewer = a_user(&pool, "bea").await;
            let stranger = a_user(&pool, "carl").await;
            let server = a_server(&pool, owner, "one", 2048).await;

            let panel = Self {
                owner_key: sign_in(&pool, owner).await,
                editor_key: sign_in(&pool, editor).await,
                viewer_key: sign_in(&pool, viewer).await,
                stranger_key: sign_in(&pool, stranger).await,
                pool,
                server,
                owner,
                editor,
                viewer,
            };

            panel.join(editor, ServerRole::Editor, true).await;
            panel.join(viewer, ServerRole::Viewer, false).await;
            panel
        }

        async fn join(&self, user: Id, role: ServerRole, accepted: bool) -> Id {
            let id = Id::new();
            let now = Timestamp::now();
            sqlx::query(
                "INSERT INTO server_members \
                 (id, server_id, user_id, role, invited_by, invited_at, joined_at, \
                 last_invite_sent) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(id)
            .bind(self.server)
            .bind(user)
            .bind(role)
            .bind(self.owner)
            .bind(now)
            .bind(accepted.then_some(now))
            .bind(now)
            .execute(&self.pool)
            .await
            .expect("inserting a membership");
            id
        }

        fn app(&self) -> Router {
            router().with_state(state(&self.pool))
        }

        async fn call(&self, request: Request<Body>) -> (StatusCode, Value) {
            let response = self.app().oneshot(request).await.expect("a response");
            (response.status(), body_json(response).await)
        }

        async fn get(&self, key: &str, uri: &str) -> (StatusCode, Value) {
            self.call(as_user(fetch(uri), key)).await
        }

        async fn post(&self, key: &str, uri: &str, body: Value) -> (StatusCode, Value) {
            self.call(as_user(send("POST", uri, body), key)).await
        }

        async fn patch(&self, key: &str, uri: &str, body: Value) -> (StatusCode, Value) {
            self.call(as_user(send("PATCH", uri, body), key)).await
        }

        async fn poke(&self, key: &str, method: &str, uri: &str) -> (StatusCode, Value) {
            self.call(as_user(empty(method, uri), key)).await
        }

        fn members_uri(&self) -> String {
            format!("/servers/{}/members", self.server)
        }

        fn member_uri(&self, user: Id) -> String {
            format!("/servers/{}/members/{user}", self.server)
        }

        async fn log(&self) -> Vec<(AuditAction, Value)> {
            let (status, body) = self.get(&self.owner_key, &format!("/servers/{}/audit-log", self.server)).await;
            assert_eq!(status, StatusCode::OK);
            body["data"]
                .as_array()
                .expect("a data array")
                .iter()
                .map(|entry| {
                    (
                        entry["action"]["action"].as_str().unwrap().parse().unwrap(),
                        entry["action"]["metadata"].clone(),
                    )
                })
                .collect()
        }
    }

    #[tokio::test]
    async fn the_owner_heads_the_list_and_wears_the_server_id() {
        let panel = Panel::open().await;
        let (status, body) = panel.get(&panel.owner_key, &panel.members_uri()).await;
        assert_eq!(status, StatusCode::OK);

        let members = body["members"].as_array().unwrap();
        assert_eq!(members.len(), 3, "the owner, the editor and the open invitation");

        let owner = &members[0];
        assert_eq!(owner["is_owner"], true);
        assert_eq!(owner["id"], panel.server.to_string(), "11.1: the row key is the server id");
        assert_eq!(owner["role"], "owner");
        assert_eq!(owner["permissions"], "SERVER_ADMIN");
        assert_eq!(owner["pending"], false);
        assert!(owner["joined_at"].is_string());
        assert!(owner["user"]["avatar_url"].is_null(), "we have no avatars");

        let invited = members.iter().find(|m| m["user"]["id"] == panel.viewer.to_string()).unwrap();
        assert_eq!(invited["pending"], true, "an open invitation reads as pending");
        assert!(invited["joined_at"].is_null());
        assert!(
            invited["invite_resend_available_at"].is_string(),
            "the resend button needs the wait, not a null"
        );
        assert_eq!(invited["permissions"], "BASE_READ | POWER_ACTIONS");
    }

    #[tokio::test]
    async fn a_panel_admin_reads_the_list_without_standing_in_it() {
        let panel = Panel::open().await;
        let boss = an_admin(&panel.pool, "boss").await;
        let key = sign_in(&panel.pool, boss).await;

        let (status, body) = panel.get(&key, &panel.members_uri()).await;
        assert_eq!(status, StatusCode::OK);
        let members = body["members"].as_array().unwrap();
        assert_eq!(members.len(), 3, "1.10: he holds SERVER_ADMIN without a row");
        assert!(members.iter().all(|m| m["user"]["id"] != boss.to_string()));
    }

    #[tokio::test]
    async fn a_stranger_is_told_the_server_does_not_exist() {
        let panel = Panel::open().await;
        let (status, body) = panel.get(&panel.stranger_key, &panel.members_uri()).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "1.7: a 403 here would leak the id");
        assert_eq!(body["error"], "server_not_found");

        let (status, _) = panel.get(&panel.viewer_key, &panel.members_uri()).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "an invitation is not access yet");
    }

    #[tokio::test]
    async fn inviting_somebody_leaves_an_open_invitation_and_a_line_in_the_log() {
        let panel = Panel::open().await;
        let dora = a_user(&panel.pool, "dora").await;

        let (status, body) = panel
            .post(
                &panel.owner_key,
                &panel.members_uri(),
                json!({ "user_id": dora.to_string(), "role": "editor" }),
            )
            .await;
        assert_eq!(status, StatusCode::CREATED);
        assert_eq!(body["pending"], true);
        assert!(body["joined_at"].is_null());
        assert_eq!(body["role"], "editor");
        assert_eq!(body["user"]["username"], "dora");

        let log = panel.log().await;
        assert_eq!(log.len(), 1);
        assert_eq!(log[0].0, AuditAction::UserInvited);
        assert_eq!(log[0].1["user_id"], dora.to_string());
        assert!(log[0].1["permissions"].as_str().unwrap().contains("FILES_WRITE"));
    }

    #[tokio::test]
    async fn the_four_ways_an_invitation_is_refused() {
        let panel = Panel::open().await;
        let dora = a_user(&panel.pool, "dora").await;
        let uri = panel.members_uri();

        let refusals = [
            (json!({ "user_id": panel.editor.to_string(), "role": "editor" }), StatusCode::CONFLICT, "already_member"),
            (json!({ "user_id": panel.viewer.to_string(), "role": "editor" }), StatusCode::CONFLICT, "already_member"),
            (json!({ "user_id": panel.owner.to_string(), "role": "editor" }), StatusCode::BAD_REQUEST, "cannot_invite_self"),
            (json!({ "user_id": Id::new().to_string(), "role": "editor" }), StatusCode::NOT_FOUND, "user_not_found"),
            (json!({ "user_id": dora.to_string(), "role": "owner" }), StatusCode::BAD_REQUEST, "role_not_assignable"),
            (json!({ "user_id": dora.to_string(), "role": "admin" }), StatusCode::BAD_REQUEST, "invalid_role"),
        ];

        for (body, status, code) in refusals {
            let (got, answer) = panel.post(&panel.owner_key, &uri, body.clone()).await;
            assert_eq!(got, status, "{body}");
            assert_eq!(answer["error"], code, "{body}");
        }

        assert!(panel.log().await.is_empty(), "nothing happened, so nothing is written down");
    }

    #[tokio::test]
    async fn a_panel_admin_is_not_invited_because_he_is_already_everywhere() {
        let panel = Panel::open().await;
        let boss = an_admin(&panel.pool, "boss").await;

        let (status, body) = panel
            .post(
                &panel.owner_key,
                &panel.members_uri(),
                json!({ "user_id": boss.to_string(), "role": "viewer" }),
            )
            .await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(body["error"], "already_member");

        let (_, list) = panel.get(&panel.owner_key, &panel.members_uri()).await;
        assert_eq!(list["members"].as_array().unwrap().len(), 3, "no row, no line in the list");
    }

    #[tokio::test]
    async fn inviting_the_owner_of_somebody_elses_server_is_a_conflict_not_a_second_owner() {
        let panel = Panel::open().await;
        let boss = an_admin(&panel.pool, "boss").await;
        let key = sign_in(&panel.pool, boss).await;

        let (status, body) = panel
            .post(&key, &panel.members_uri(), json!({ "user_id": panel.owner.to_string(), "role": "editor" }))
            .await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(body["error"], "already_member");
    }

    #[tokio::test]
    async fn a_role_change_takes_effect_at_once_and_is_written_down() {
        let panel = Panel::open().await;
        let (status, body) = panel
            .patch(&panel.owner_key, &panel.member_uri(panel.editor), json!({ "role": "viewer" }))
            .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["role"], "viewer");
        assert_eq!(body["permissions"], "BASE_READ | POWER_ACTIONS");

        let access = access::of(&panel.pool, &signed_in(&panel.pool, panel.editor).await, panel.server)
            .await
            .unwrap();
        assert_eq!(access.role(), ServerRole::Viewer, "the next request already sees it");
        assert!(!access.allows(Permission::FilesWrite));

        let log = panel.log().await;
        assert_eq!(log[0].0, AuditAction::UserPermissionModified);
        assert_eq!(log[0].1["user_id"], panel.editor.to_string());
    }

    #[tokio::test]
    async fn a_role_change_reaches_an_invitation_that_is_still_open() {
        let panel = Panel::open().await;
        let (status, body) = panel
            .patch(&panel.owner_key, &panel.member_uri(panel.viewer), json!({ "role": "editor" }))
            .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["role"], "editor");
        assert_eq!(body["pending"], true);
        assert!(body["joined_at"].is_null());

        let (_, waiting) = panel.get(&panel.viewer_key, "/invitations").await;
        assert_eq!(waiting["invitations"][0]["role"], "editor");
    }

    #[tokio::test]
    async fn a_role_change_for_somebody_who_is_not_a_member() {
        let panel = Panel::open().await;
        let dora = a_user(&panel.pool, "dora").await;

        for (target, code) in [(dora, "member_not_found"), (Id::new(), "member_not_found")] {
            let (status, body) = panel
                .patch(&panel.owner_key, &panel.member_uri(target), json!({ "role": "viewer" }))
                .await;
            assert_eq!(status, StatusCode::NOT_FOUND);
            assert_eq!(body["error"], code);
        }
    }

    #[tokio::test]
    async fn the_two_roles_a_change_is_refused_for() {
        let panel = Panel::open().await;

        for (role, code) in [("owner", "role_not_assignable"), ("admin", "invalid_role")] {
            let (status, body) = panel
                .patch(&panel.owner_key, &panel.member_uri(panel.editor), json!({ "role": role }))
                .await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{role}");
            assert_eq!(body["error"], code, "{role}");
        }

        let access = access::of(&panel.pool, &signed_in(&panel.pool, panel.editor).await, panel.server)
            .await
            .unwrap();
        assert_eq!(access.role(), ServerRole::Editor, "he is what he was");
        assert!(panel.log().await.is_empty());
    }

    #[tokio::test]
    async fn the_owner_can_neither_be_removed_nor_demoted() {
        let panel = Panel::open().await;
        let boss = an_admin(&panel.pool, "boss").await;
        let admin_key = sign_in(&panel.pool, boss).await;
        let uri = panel.member_uri(panel.owner);

        for key in [&panel.owner_key, &admin_key] {
            let (status, body) = panel.poke(key, "DELETE", &uri).await;
            assert_eq!(status, StatusCode::BAD_REQUEST);
            assert_eq!(body["error"], "cannot_remove_owner");
        }

        let (status, body) = panel.patch(&panel.owner_key, &uri, json!({ "role": "viewer" })).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "he is no membership row to write to");
        assert_eq!(body["error"], "member_not_found");

        let (_, list) = panel.get(&panel.owner_key, &panel.members_uri()).await;
        assert_eq!(list["members"][0]["permissions"], "SERVER_ADMIN", "still every bit");
        assert_eq!(list["members"][0]["is_owner"], true);
    }

    #[tokio::test]
    async fn a_server_handed_to_one_of_its_own_members_has_no_second_entry_for_him() {
        let panel = Panel::open().await;
        sqlx::query("UPDATE servers SET owner_id = ? WHERE id = ?")
            .bind(panel.viewer)
            .bind(panel.server)
            .execute(&panel.pool)
            .await
            .unwrap();

        let (status, body) = panel.get(&panel.viewer_key, &panel.members_uri()).await;
        assert_eq!(status, StatusCode::OK);
        let members = body["members"].as_array().unwrap();
        assert_eq!(members.len(), 2, "the new owner and the editor, not the owner twice");
        assert_eq!(members[0]["user"]["id"], panel.viewer.to_string());
        assert_eq!(members[0]["permissions"], "SERVER_ADMIN");
        assert_eq!(members[0]["is_owner"], true);
        assert!(members[1..].iter().all(|m| m["user"]["id"] != panel.viewer.to_string()));

        let (_, body) = panel.get(&panel.viewer_key, "/invitations").await;
        assert!(
            body["invitations"].as_array().unwrap().is_empty(),
            "one cannot be invited to a server one owns"
        );

        let uri = panel.member_uri(panel.viewer);
        let (status, body) = panel.patch(&panel.viewer_key, &uri, json!({ "role": "viewer" })).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "the leftover row is no membership");
        assert_eq!(body["error"], "member_not_found");

        let (status, body) = panel.poke(&panel.viewer_key, "DELETE", &uri).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["error"], "cannot_remove_owner");
    }

    #[tokio::test]
    async fn a_member_who_is_made_a_panel_admin_leaves_the_member_list() {
        let panel = Panel::open().await;
        async fn promote(pool: &SqlitePool, user: Id, role: &str) {
            sqlx::query("UPDATE users SET role = ? WHERE id = ?")
                .bind(role)
                .bind(user)
                .execute(pool)
                .await
                .unwrap();
        }
        promote(&panel.pool, panel.editor, "admin").await;
        promote(&panel.pool, panel.viewer, "admin").await;

        let (_, body) = panel.get(&panel.owner_key, &panel.members_uri()).await;
        let members = body["members"].as_array().unwrap();
        assert_eq!(members.len(), 1, "1.10: only the owner is left to show");

        let uri = panel.member_uri(panel.editor);
        let (status, body) = panel.patch(&panel.owner_key, &uri, json!({ "role": "viewer" })).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "the leftover row is no membership");
        assert_eq!(body["error"], "member_not_found");

        let (status, _) = panel.poke(&panel.owner_key, "DELETE", &uri).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "and nothing the role box may take away");

        let (_, waiting) = panel.get(&panel.viewer_key, "/invitations").await;
        assert!(
            waiting["invitations"].as_array().unwrap().is_empty(),
            "an invitation offers him less than he already has"
        );
        let (status, _) = panel
            .post(
                &panel.owner_key,
                &panel.members_uri(),
                json!({ "user_id": panel.viewer.to_string(), "role": "viewer" }),
            )
            .await;
        assert_eq!(status, StatusCode::CONFLICT, "11.2 refuses him a second row too");

        promote(&panel.pool, panel.editor, "user").await;
        let (_, body) = panel.get(&panel.owner_key, &panel.members_uri()).await;
        assert_eq!(body["members"].as_array().unwrap().len(), 2, "demoted, he is a member again");
        assert_eq!(body["members"][1]["role"], "editor", "with the role he always had");
    }

    #[tokio::test]
    async fn one_removal_is_one_line_in_the_log() {
        let panel = Panel::open().await;
        let uri = panel.member_uri(panel.editor);
        let (first, second) = tokio::join!(
            panel.poke(&panel.owner_key, "DELETE", &uri),
            panel.poke(&panel.owner_key, "DELETE", &uri)
        );

        let mut answers = [first.0, second.0];
        answers.sort();
        assert_eq!(answers, [StatusCode::NO_CONTENT, StatusCode::NOT_FOUND]);
        assert_eq!(
            panel.log().await.into_iter().map(|(action, _)| action).collect::<Vec<_>>(),
            vec![AuditAction::UserRemoved]
        );
    }

    #[tokio::test]
    async fn removing_a_member_and_withdrawing_an_invitation_are_one_call_and_two_entries() {
        let panel = Panel::open().await;

        let (status, _) = panel.poke(&panel.owner_key, "DELETE", &panel.member_uri(panel.editor)).await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let (status, _) = panel.poke(&panel.owner_key, "DELETE", &panel.member_uri(panel.viewer)).await;
        assert_eq!(status, StatusCode::NO_CONTENT);

        let (_, list) = panel.get(&panel.owner_key, &panel.members_uri()).await;
        assert_eq!(list["members"].as_array().unwrap().len(), 1, "the owner is left");

        let mut written: Vec<AuditAction> = panel.log().await.into_iter().map(|(action, _)| action).collect();
        written.sort();
        assert_eq!(written, vec![AuditAction::UserInviteRevoked, AuditAction::UserRemoved]);
    }

    #[tokio::test]
    async fn anybody_may_hand_back_his_own_access() {
        let panel = Panel::open().await;
        let (status, _) = panel.poke(&panel.editor_key, "DELETE", &panel.member_uri(panel.editor)).await;
        assert_eq!(status, StatusCode::NO_CONTENT, "11.4: or the caller removes himself");

        let (status, body) = panel.get(&panel.editor_key, &panel.members_uri()).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "and is a stranger from then on");
        assert_eq!(body["error"], "server_not_found");
    }

    #[tokio::test]
    async fn an_editor_may_not_manage_members() {
        let panel = Panel::open().await;
        let dora = a_user(&panel.pool, "dora").await;

        let calls: Vec<(&str, String, Value)> = vec![
            ("POST", panel.members_uri(), json!({ "user_id": dora.to_string(), "role": "viewer" })),
            ("PATCH", panel.member_uri(panel.viewer), json!({ "role": "editor" })),
            ("DELETE", panel.member_uri(panel.viewer), Value::Null),
            ("POST", format!("{}/reinvite", panel.member_uri(panel.viewer)), Value::Null),
        ];

        for (method, uri, body) in calls {
            let (status, answer) = match body {
                Value::Null => panel.poke(&panel.editor_key, method, &uri).await,
                body => panel.call(as_user(send(method, &uri, body), &panel.editor_key)).await,
            };
            assert_eq!(status, StatusCode::FORBIDDEN, "{method} {uri}");
            assert_eq!(answer["error"], "forbidden", "{method} {uri}");
        }

        let (_, list) = panel.get(&panel.owner_key, &panel.members_uri()).await;
        assert_eq!(list["members"].as_array().unwrap().len(), 3, "nothing moved");
        assert!(panel.log().await.is_empty());
    }

    #[tokio::test]
    async fn a_viewer_may_not_manage_members_but_may_hand_his_own_access_back() {
        let panel = Panel::open().await;
        let eva = a_user(&panel.pool, "eva").await;
        panel.join(eva, ServerRole::Viewer, true).await;
        let key = sign_in(&panel.pool, eva).await;
        let dora = a_user(&panel.pool, "dora").await;

        let (status, _) = panel.get(&key, &panel.members_uri()).await;
        assert_eq!(status, StatusCode::OK, "2.1: the list reads with BASE_READ");

        let refused: Vec<(&str, String, Value)> = vec![
            ("POST", panel.members_uri(), json!({ "user_id": dora.to_string(), "role": "viewer" })),
            ("PATCH", panel.member_uri(panel.editor), json!({ "role": "viewer" })),
            ("DELETE", panel.member_uri(panel.editor), Value::Null),
            ("POST", format!("{}/reinvite", panel.member_uri(panel.viewer)), Value::Null),
        ];

        for (method, uri, body) in refused {
            let (status, answer) = match body {
                Value::Null => panel.poke(&key, method, &uri).await,
                body => panel.call(as_user(send(method, &uri, body), &key)).await,
            };
            assert_eq!(status, StatusCode::FORBIDDEN, "{method} {uri}");
            assert_eq!(answer["error"], "forbidden", "{method} {uri}");
        }

        let (status, _) = panel.poke(&key, "DELETE", &panel.member_uri(eva)).await;
        assert_eq!(status, StatusCode::NO_CONTENT, "2.1: the second half of 11.4 is his own row");
    }

    #[tokio::test]
    async fn every_endpoint_tells_a_stranger_the_same_thing() {
        let panel = Panel::open().await;
        let ida = a_user(&panel.pool, "ida").await;
        let key = sign_in(&panel.pool, ida).await;
        a_server(&panel.pool, ida, "hers", 2048).await;

        let nonsense = format!("/servers/{}/members/not-a-ulid", panel.server);
        let calls: Vec<(&str, String, Value)> = vec![
            ("GET", panel.members_uri(), Value::Null),
            (
                "POST",
                panel.members_uri(),
                json!({ "user_id": panel.editor.to_string(), "role": "viewer" }),
            ),
            ("PATCH", panel.member_uri(panel.editor), json!({ "role": "viewer" })),
            ("DELETE", panel.member_uri(panel.editor), Value::Null),
            ("POST", format!("{}/reinvite", panel.member_uri(panel.viewer)), Value::Null),
            ("GET", format!("/servers/{}/audit-log", panel.server), Value::Null),
            ("DELETE", nonsense.clone(), Value::Null),
            ("PATCH", nonsense, json!({ "role": "viewer" })),
        ];

        for (method, uri, body) in calls {
            let (status, answer) = match body {
                Value::Null => panel.poke(&key, method, &uri).await,
                body => panel.call(as_user(send(method, &uri, body), &key)).await,
            };
            assert_eq!(status, StatusCode::NOT_FOUND, "{method} {uri}");
            assert_eq!(answer["error"], "server_not_found", "{method} {uri}");
        }

        let (_, list) = panel.get(&panel.owner_key, &panel.members_uri()).await;
        assert_eq!(list["members"].as_array().unwrap().len(), 3, "and nothing moved");
    }

    #[tokio::test]
    async fn resending_inside_the_wait_is_a_two_hundred_that_did_not_send() {
        let panel = Panel::open().await;
        let uri = format!("{}/reinvite", panel.member_uri(panel.viewer));

        let (status, body) = panel.poke(&panel.owner_key, "POST", &uri).await;
        assert_eq!(status, StatusCode::OK, "11.5: a wait is not an error");
        assert_eq!(body["sent"], false);
        let left = body["cooldown_seconds"].as_u64().expect("the remaining seconds");
        assert!(left > 0 && left <= 120, "{left}");
        assert_eq!(body["member"]["pending"], true);

        let long_ago = Timestamp::at(Timestamp::now().as_datetime() - time::Duration::minutes(5));
        sqlx::query("UPDATE server_members SET last_invite_sent = ? WHERE user_id = ?")
            .bind(long_ago)
            .bind(panel.viewer)
            .execute(&panel.pool)
            .await
            .unwrap();

        let (status, body) = panel.poke(&panel.owner_key, "POST", &uri).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["sent"], true);
        assert_eq!(body["cooldown_seconds"], 120);
        assert!(
            body["member"]["invite_resend_available_at"].as_str().unwrap()
                > body["member"]["invited_at"].as_str().unwrap(),
            "the wait starts again"
        );
    }

    #[tokio::test]
    async fn resending_to_somebody_who_has_long_since_joined() {
        let panel = Panel::open().await;
        let (status, body) = panel
            .poke(&panel.owner_key, "POST", &format!("{}/reinvite", panel.member_uri(panel.editor)))
            .await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(body["error"], "already_member");

        let (status, body) = panel
            .poke(&panel.owner_key, "POST", &format!("{}/reinvite", panel.member_uri(Id::new())))
            .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(body["error"], "member_not_found");
    }

    #[tokio::test]
    async fn an_invitation_is_listed_accepted_and_then_gone() {
        let panel = Panel::open().await;
        let (status, body) = panel.get(&panel.viewer_key, "/invitations").await;
        assert_eq!(status, StatusCode::OK);

        let invitations = body["invitations"].as_array().unwrap();
        assert_eq!(invitations.len(), 1);
        assert_eq!(invitations[0]["server"]["name"], "one");
        assert_eq!(invitations[0]["invited_by"]["username"], "max");
        assert_eq!(invitations[0]["role"], "viewer");
        let id = invitations[0]["id"].as_str().unwrap().to_owned();

        let (status, member) = panel.poke(&panel.viewer_key, "POST", &format!("/invitations/{id}/accept")).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(member["pending"], false);
        assert!(member["joined_at"].is_string());
        assert_eq!(member["id"], id, "11.6: the invitation is the membership row");

        let (status, _) = panel.get(&panel.viewer_key, &panel.members_uri()).await;
        assert_eq!(status, StatusCode::OK, "and now he may look");

        let (_, body) = panel.get(&panel.viewer_key, "/invitations").await;
        assert!(body["invitations"].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn an_invitation_outlives_the_account_that_sent_it() {
        let panel = Panel::open().await;
        let boss = an_admin(&panel.pool, "boss").await;
        let boss_key = sign_in(&panel.pool, boss).await;
        let dora = a_user(&panel.pool, "dora").await;
        let dora_key = sign_in(&panel.pool, dora).await;

        let (status, _) = panel
            .post(&boss_key, &panel.members_uri(), json!({ "user_id": dora.to_string(), "role": "viewer" }))
            .await;
        assert_eq!(status, StatusCode::CREATED);
        sqlx::query("DELETE FROM users WHERE id = ?").bind(boss).execute(&panel.pool).await.unwrap();

        let (status, body) = panel.get(&dora_key, "/invitations").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            body["invitations"][0]["invited_by"]["username"], "max",
            "invited_by is mandatory, so the owner stands in for the account that is gone"
        );
    }

    #[tokio::test]
    async fn declining_leaves_room_for_a_second_invitation() {
        let panel = Panel::open().await;
        let (_, body) = panel.get(&panel.viewer_key, "/invitations").await;
        let id = body["invitations"][0]["id"].as_str().unwrap().to_owned();

        let (status, _) = panel.poke(&panel.viewer_key, "POST", &format!("/invitations/{id}/decline")).await;
        assert_eq!(status, StatusCode::NO_CONTENT);

        let (status, _) = panel
            .post(
                &panel.owner_key,
                &panel.members_uri(),
                json!({ "user_id": panel.viewer.to_string(), "role": "viewer" }),
            )
            .await;
        assert_eq!(status, StatusCode::CREATED, "11.8: a new invitation is possible afterwards");
    }

    #[tokio::test]
    async fn somebody_elses_invitation_is_not_found_rather_than_forbidden() {
        let panel = Panel::open().await;
        let (_, body) = panel.get(&panel.viewer_key, "/invitations").await;
        let id = body["invitations"][0]["id"].as_str().unwrap().to_owned();

        for (key, what) in [(&panel.stranger_key, "a stranger"), (&panel.owner_key, "the inviter")] {
            let (status, answer) = panel.poke(key, "POST", &format!("/invitations/{id}/accept")).await;
            assert_eq!(status, StatusCode::NOT_FOUND, "{what}");
            assert_eq!(answer["error"], "invitation_not_found", "{what}");
        }

        let (status, answer) = panel
            .poke(&panel.editor_key, "POST", &format!("/invitations/{}/accept", Id::new()))
            .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(answer["error"], "invitation_not_found");
    }

    #[tokio::test]
    async fn an_invitation_that_was_taken_up_cannot_be_taken_up_twice() {
        let panel = Panel::open().await;
        let joined: Id = sqlx::query_scalar("SELECT id FROM server_members WHERE user_id = ?")
            .bind(panel.editor)
            .fetch_one(&panel.pool)
            .await
            .unwrap();

        let (status, body) = panel
            .poke(&panel.editor_key, "POST", &format!("/invitations/{joined}/accept"))
            .await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(body["error"], "already_member");
    }

    #[tokio::test]
    async fn the_log_pages_by_offset_and_says_when_it_is_over() {
        let panel = Panel::open().await;
        for _ in 0..5 {
            crate::audit::record_by(&panel.pool, panel.server, panel.owner, crate::audit::Event::ServerStarted).await;
        }

        let uri = format!("/servers/{}/audit-log", panel.server);
        let (status, body) = panel.get(&panel.owner_key, &format!("{uri}?limit=2")).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["data"].as_array().unwrap().len(), 2);
        assert_eq!(body["next_offset"], 2);

        let (_, body) = panel.get(&panel.owner_key, &format!("{uri}?limit=2&offset=4")).await;
        assert_eq!(body["data"].as_array().unwrap().len(), 1);
        assert!(body["next_offset"].is_null(), "null on the last page, or the table pages for ever");
    }

    #[tokio::test]
    async fn the_log_filters_by_actor_by_action_and_by_time() {
        let panel = Panel::open().await;
        let dora = a_user(&panel.pool, "dora").await;
        let uri = format!("/servers/{}/audit-log", panel.server);

        panel.post(&panel.owner_key, &panel.members_uri(), json!({ "user_id": dora.to_string(), "role": "viewer" })).await;
        panel.patch(&panel.owner_key, &panel.member_uri(panel.editor), json!({ "role": "viewer" })).await;
        crate::audit::record_by(&panel.pool, panel.server, panel.editor, crate::audit::Event::ServerStopped).await;

        let (_, all) = panel.get(&panel.owner_key, &uri).await;
        assert_eq!(all["data"].as_array().unwrap().len(), 3);
        assert_eq!(all["data"][0]["action"]["action"], "server_stopped", "newest first");
        assert_eq!(all["data"][0]["world_id"], Value::Null);
        assert_eq!(all["data"][0]["actor"]["type"], "user");

        let (_, mine) = panel.get(&panel.owner_key, &format!("{uri}?actor={}", panel.editor)).await;
        assert_eq!(mine["data"].as_array().unwrap().len(), 1);

        let (_, both) = panel
            .get(&panel.owner_key, &format!("{uri}?actor={}&actor={}", panel.editor, panel.owner))
            .await;
        assert_eq!(both["data"].as_array().unwrap().len(), 3, "repeated actors are ORed");

        let (_, picked) = panel
            .get(&panel.owner_key, &format!("{uri}?action=user_invited&action=server_stopped"))
            .await;
        assert_eq!(picked["data"].as_array().unwrap().len(), 2);

        let (_, oldest) = panel.get(&panel.owner_key, &format!("{uri}?order=asc")).await;
        assert_eq!(oldest["data"][0]["action"]["action"], "user_invited");

        let tomorrow = Timestamp::at(Timestamp::now().as_datetime() + time::Duration::days(1));
        let (_, none) = panel.get(&panel.owner_key, &format!("{uri}?min_datetime={tomorrow}")).await;
        assert!(none["data"].as_array().unwrap().is_empty());

        let (_, all) = panel.get(&panel.owner_key, &format!("{uri}?max_datetime={tomorrow}")).await;
        assert_eq!(all["data"].as_array().unwrap().len(), 3);
    }

    #[tokio::test]
    async fn the_log_keeps_the_order_the_lines_were_written_in() {
        let panel = Panel::open().await;
        for step in 0..25u32 {
            crate::audit::record_by(
                &panel.pool,
                panel.server,
                panel.owner,
                crate::audit::Event::ConsoleCommandExecuted { command: step.to_string() },
            )
            .await;
        }

        let steps = |body: &Value| -> Vec<u32> {
            body["data"]
                .as_array()
                .expect("a data array")
                .iter()
                .map(|entry| entry["action"]["metadata"]["command"].as_str().unwrap().parse().unwrap())
                .collect()
        };
        let newest_first: Vec<u32> = (0..25).rev().collect();
        let uri = format!("/servers/{}/audit-log", panel.server);

        let (_, body) = panel.get(&panel.owner_key, &uri).await;
        assert_eq!(steps(&body), newest_first, "desc is newest first, not shuffled");

        let (_, body) = panel.get(&panel.owner_key, &format!("{uri}?order=asc")).await;
        assert_eq!(steps(&body), (0..25).collect::<Vec<_>>(), "and asc is the way round again");

        let mut paged = Vec::new();
        for offset in [0, 10, 20] {
            let (_, page) =
                panel.get(&panel.owner_key, &format!("{uri}?limit=10&offset={offset}")).await;
            paged.extend(steps(&page));
        }
        assert_eq!(paged, newest_first, "and no page shows a line twice or skips one");
    }

    #[tokio::test]
    async fn a_filter_is_bound_and_never_pasted() {
        let panel = Panel::open().await;
        crate::audit::record_by(&panel.pool, panel.server, panel.owner, crate::audit::Event::ServerStarted)
            .await;
        let uri = format!("/servers/{}/audit-log", panel.server);

        for hostile in [
            "actor=%27%29%3B+DROP+TABLE+audit_log%3B--",
            "action=server_started%27%29+OR+1%3D1--",
            "min_datetime=%27%29+OR+1%3D1--",
        ] {
            let (status, body) = panel.get(&panel.owner_key, &format!("{uri}?{hostile}")).await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{hostile}");
            assert_eq!(body["error"], "invalid_request", "{hostile}");
        }

        let (status, body) = panel.get(&panel.owner_key, "/servers/..%2F..%2Fetc/members").await;
        assert_eq!(status, StatusCode::NOT_FOUND, "a path is a ULID or it is nothing");
        assert_eq!(body["error"], "server_not_found");

        let repeated: String =
            (0..500).map(|_| format!("&actor={}", panel.owner)).collect::<Vec<_>>().concat();
        let (status, body) = panel.get(&panel.owner_key, &format!("{uri}?{}", &repeated[1..])).await;
        assert_eq!(status, StatusCode::OK, "a name repeated is one bound value, not five hundred");
        assert_eq!(body["data"].as_array().unwrap().len(), 1);

        let left: i64 = sqlx::query_scalar("SELECT count(*) FROM audit_log")
            .fetch_one(&panel.pool)
            .await
            .unwrap();
        assert_eq!(left, 1, "the table is where it was, with the row it had");
    }

    #[tokio::test]
    async fn more_actors_than_the_filter_takes_is_a_bad_request() {
        let panel = Panel::open().await;
        crate::audit::record_by(&panel.pool, panel.server, panel.owner, crate::audit::Event::ServerStarted)
            .await;
        let uri = format!("/servers/{}/audit-log", panel.server);

        let names = |count: usize| -> String {
            (0..count).map(|_| format!("&actor={}", Id::new())).collect::<Vec<_>>().concat()
        };

        let (status, body) =
            panel.get(&panel.owner_key, &format!("{uri}?actor={}{}", panel.owner, names(499))).await;
        assert_eq!(status, StatusCode::OK, "five hundred names still answer");
        assert_eq!(body["data"].as_array().unwrap().len(), 1);

        let (status, body) = panel.get(&panel.owner_key, &format!("{uri}?{}", &names(600)[1..])).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["error"], "invalid_request");

        let flood: Vec<(String, String)> =
            (0..40_000).map(|_| ("actor".to_owned(), Id::new().to_string())).collect();
        assert_eq!(crate::audit::Query::read(&flood).unwrap_err().code(), "invalid_request");
    }

    #[tokio::test]
    async fn an_action_that_is_not_in_the_catalogue_is_a_bad_request() {
        let panel = Panel::open().await;
        let uri = format!("/servers/{}/audit-log", panel.server);

        for wrong in ["action=sftp_login", "action=", "order=sideways", "limit=lots", "actor=nobody", "min_datetime=yesterday"] {
            let (status, body) = panel.get(&panel.owner_key, &format!("{uri}?{wrong}")).await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{wrong}");
            assert_eq!(body["error"], "invalid_request", "{wrong}");
        }
    }

    #[tokio::test]
    async fn the_log_names_everybody_it_mentions() {
        let panel = Panel::open().await;
        let dora = a_user(&panel.pool, "dora").await;
        panel
            .post(&panel.owner_key, &panel.members_uri(), json!({ "user_id": dora.to_string(), "role": "viewer" }))
            .await;

        let (_, body) = panel.get(&panel.owner_key, &format!("/servers/{}/audit-log", panel.server)).await;
        let users = body["users"].as_object().unwrap();
        assert_eq!(users[&panel.owner.to_string()]["username"], "max", "the actor");
        assert_eq!(users[&dora.to_string()]["username"], "dora", "and the one in the metadata");
        assert!(users[&dora.to_string()]["avatar_url"].is_null());
        assert!(body["addons"].is_object() && body["versions"].is_object());
    }

    #[tokio::test]
    async fn a_viewer_reads_the_log_and_a_stranger_does_not() {
        let panel = Panel::open().await;
        panel.join(a_user(&panel.pool, "eva").await, ServerRole::Viewer, true).await;
        let eva = sign_in(&panel.pool, users::by_name(&panel.pool, "eva").await.unwrap().unwrap().id).await;
        let uri = format!("/servers/{}/audit-log", panel.server);

        let (status, _) = panel.get(&eva, &uri).await;
        assert_eq!(status, StatusCode::OK, "11.9 reads with BASE_READ");

        let (status, body) = panel.get(&panel.stranger_key, &uri).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(body["error"], "server_not_found");
    }

    #[tokio::test]
    async fn one_server_does_not_show_another_ones_log() {
        let panel = Panel::open().await;
        let other = a_server(&panel.pool, panel.owner, "two", 2048).await;
        crate::audit::record_by(&panel.pool, other, panel.owner, crate::audit::Event::ServerKilled).await;
        crate::audit::record_by(&panel.pool, panel.server, panel.owner, crate::audit::Event::ServerStarted).await;

        let log = panel.log().await;
        assert_eq!(log.len(), 1);
        assert_eq!(log[0].0, AuditAction::ServerStarted);
    }

    #[tokio::test]
    async fn no_path_segment_is_anything_but_a_ulid() {
        let panel = Panel::open().await;
        let hostile = [
            "..",
            "../..",
            "..%2F..%2Fetc%2Fpasswd",
            "%2e%2e%2f",
            "..%00",
            "%00",
            "01K2FA0B1C2D3E4F5G6H7J8K9M%2F..",
            "ZZZZZZZZZZZZZZZZZZZZZZZZZZ",
            "01K2FA0B1C2D3E4F5G6H7J8K9",
            "801K2FA0B1C2D3E4F5G6H7J8K9M",
        ];

        for bad in hostile {
            let calls = [
                format!("/servers/{bad}/members"),
                format!("/servers/{bad}/audit-log"),
                format!("/servers/{}/members/{bad}", panel.server),
                format!("/servers/{bad}/members/{}", panel.editor),
                format!("/servers/{}/members/{bad}/reinvite", panel.server),
                format!("/invitations/{bad}/accept"),
                format!("/invitations/{bad}/decline"),
            ];
            for uri in calls {
                let (status, body) = panel.get(&panel.owner_key, &uri).await;
                assert!(
                    status.is_client_error(),
                    "{uri} answered {status}: {body}"
                );
            }
            for uri in [
                format!("/servers/{}/members/{bad}/reinvite", panel.server),
                format!("/invitations/{bad}/accept"),
            ] {
                let (status, body) = panel.poke(&panel.owner_key, "POST", &uri).await;
                assert_eq!(status, StatusCode::NOT_FOUND, "{uri}");
                assert!(
                    body.is_null()
                        || ["server_not_found", "member_not_found", "invitation_not_found"]
                            .contains(&body["error"].as_str().unwrap_or_default()),
                    "{uri}: {body}"
                );
            }
        }

        let (status, body) = panel
            .get(&panel.owner_key, &format!("/servers/{}/members", panel.server.to_string().to_lowercase()))
            .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["members"][0]["id"], panel.server.to_string(), "uppercase, once");
    }

    #[tokio::test]
    async fn without_a_session_nothing_here_answers() {
        let panel = Panel::open().await;
        let calls = [
            ("GET", panel.members_uri()),
            ("GET", "/invitations".to_owned()),
            ("GET", format!("/servers/{}/audit-log", panel.server)),
        ];

        for (method, uri) in calls {
            let (status, body) = panel.call(empty(method, &uri)).await;
            assert_eq!(status, StatusCode::UNAUTHORIZED, "{method} {uri}");
            assert_eq!(body["error"], "unauthenticated");
        }
    }


    async fn signed_in(pool: &SqlitePool, user: Id) -> Caller {
        let row = users::load(pool, user).await.unwrap();
        let (session, _) = crate::auth::session::open(pool, user, None, Timestamp::now()).await.unwrap();
        Caller { user: row, session, secure: false }
    }
}
