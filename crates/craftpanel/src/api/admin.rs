use std::path::{Path as FsPath, PathBuf};
use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

use crate::auth::error::{Failure, Result};
use crate::auth::users::UserRow;
use crate::auth::{extract, limits, password, reset, session, settings, usage, users};
use crate::auth::{Admin, Disks, JsonBody, LiveServers, Params};
use crate::config::Config;
use crate::drive::Drive;
use crate::helper::Helper;
use crate::model::{
    AccountOrigin, Id, PanelRole, PanelSettings, PanelUser, SystemUserState, Timestamp, UserLimits,
    UserUsage,
};
use crate::playit::Playit;
use crate::registration;
use crate::AppState;

const PAGE: u32 = 50;
const PAGE_CEILING: u32 = 200;
const MIB: u64 = 1024 * 1024;

fn page_size(asked: Option<u32>) -> u32 {
    asked.unwrap_or(PAGE).clamp(1, PAGE_CEILING)
}

pub fn router(playit: Arc<Playit>, drive: Arc<Drive>) -> Router<AppState> {
    with_live(LiveServers::none(), Disks::none(), playit, drive)
}

pub fn with_live(
    live: LiveServers,
    disks: Disks,
    playit: Arc<Playit>,
    drive: Arc<Drive>,
) -> Router<AppState> {
    Router::new()
        .route("/admin/host", get(host))
        .route("/admin/users", get(list_users).post(create_user))
        .route("/admin/users/{user_id}", get(one_user).patch(update_user).delete(delete_user))
        .route("/admin/users/{user_id}/limits", get(read_limits).put(write_limits))
        .route("/admin/users/{user_id}/system-user/retry", post(retry_system_user))
        .route("/admin/settings", get(read_settings).put(write_settings))
        .layer(Extension(live))
        .layer(Extension(disks))
        .layer(Extension(playit))
        .layer(Extension(drive))
        .layer(axum::middleware::from_fn(extract::same_origin))
}

#[derive(Serialize)]
struct HostCapacity {
    cpu_cores: u32,
    memory_total_bytes: u64,
    reserved_memory_mib: u32,
    assignable_memory_mib: u32,
    disk_total_bytes: u64,
    assignable_disk_mib: u32,
    allocated: Allocated,
    used: Used,
    user_count: u32,
    unlimited_users: u32,
    default_limits: UserLimits,
    measured_at: Timestamp,
}

#[derive(Serialize)]
struct Allocated {
    memory_mib: u32,
    cpu_cores: f64,
    disk_mib: u32,
}

#[derive(Serialize)]
struct Used {
    memory_bytes: u64,
    cpu_cores: f64,
    pids: u32,
}

#[derive(Serialize)]
struct AdminUserList {
    users: Vec<PanelUser>,
    total: u32,
}

#[derive(Serialize)]
struct AdminUserDetail {
    #[serde(flatten)]
    user: PanelUser,
    owned_servers: Vec<OwnedServerRef>,
    active_sessions: u32,
}

#[derive(Serialize)]
struct OwnedServerRef {
    id: Id,
    name: String,
    memory_mib: u32,
    running: bool,
}

#[derive(Serialize)]
struct UserLimitsResponse {
    limits: Option<UserLimits>,
    usage: UserUsage,
    host: HostRoom,
}

#[derive(Serialize)]
struct HostRoom {
    cpu_cores: u32,
    assignable_memory_mib: u32,
    assignable_disk_mib: u32,
}

#[derive(Deserialize)]
struct ListQuery {
    query: Option<String>,
    limit: Option<u32>,
    offset: Option<u32>,
}

#[derive(Deserialize)]
struct CreateUserRequest {
    username: String,
    password: String,
    panel_role: PanelRole,
    #[serde(default)]
    email: Option<String>,
    must_change_password: Option<bool>,
    limits: Option<UserLimits>,
}

#[derive(Deserialize)]
struct UpdateUserRequest {
    username: Option<String>,
    panel_role: Option<PanelRole>,
    password: Option<String>,
    #[serde(default, deserialize_with = "field_that_may_be_null")]
    email: Option<Option<String>>,
    must_change_password: Option<bool>,
}

fn field_that_may_be_null<'de, D, T>(reader: D) -> std::result::Result<Option<Option<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::Deserialize<'de>,
{
    Option::deserialize(reader).map(Some)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
enum DeleteUserServers {
    Delete,
    Transfer,
}

#[derive(Deserialize)]
struct DeleteQuery {
    servers: Option<DeleteUserServers>,
    transfer_to: Option<String>,
}

async fn host(State(state): State<AppState>, Admin(_): Admin) -> Result<Json<HostCapacity>> {
    let machine = usage::Host::measure();
    let disk_total = crate::files::filesystem_total_bytes(&state.config.data_dir);
    let promised = promised(&state.pool).await?;
    let everyone: Vec<Id> = sqlx::query_scalar("SELECT id FROM users").fetch_all(&state.pool).await?;
    let used = usage::shared().total(everyone);

    Ok(Json(HostCapacity {
        cpu_cores: machine.cpu_cores,
        memory_total_bytes: machine.memory_total_bytes,
        reserved_memory_mib: machine.reserved_memory_mib(),
        assignable_memory_mib: machine.assignable_memory_mib(),
        disk_total_bytes: disk_total,
        assignable_disk_mib: (disk_total / MIB) as u32,
        allocated: Allocated {
            memory_mib: promised.memory_mib,
            cpu_cores: promised.cpu_cores,
            disk_mib: promised.disk_mib,
        },
        used: Used {
            memory_bytes: used.memory_bytes,
            cpu_cores: used.used_cores,
            pids: used.pids,
        },
        user_count: promised.user_count,
        unlimited_users: promised.unlimited_users,
        default_limits: settings::load(&state.pool).await?.default_limits,
        measured_at: used.measured_at,
    }))
}

struct Promised {
    user_count: u32,
    unlimited_users: u32,
    memory_mib: u32,
    cpu_cores: f64,
    disk_mib: u32,
}

async fn promised(pool: &SqlitePool) -> sqlx::Result<Promised> {
    let (count, unlimited, memory, cores, disk): (i64, i64, i64, f64, i64) = sqlx::query_as(
        "SELECT count(*), \
                count(*) FILTER (WHERE role = 'admin'), \
                coalesce(sum(memory_mib) FILTER (WHERE role <> 'admin'), 0), \
                coalesce(sum(cpu_cores) FILTER (WHERE role <> 'admin'), 0.0), \
                coalesce(sum(disk_mib) FILTER (WHERE role <> 'admin'), 0) \
         FROM users",
    )
    .fetch_one(pool)
    .await?;

    Ok(Promised {
        user_count: count.max(0) as u32,
        unlimited_users: unlimited.max(0) as u32,
        memory_mib: memory.max(0) as u32,
        cpu_cores: cores,
        disk_mib: disk.max(0) as u32,
    })
}

async fn list_users(
    State(state): State<AppState>,
    Admin(_): Admin,
    Extension(live): Extension<LiveServers>,
    Extension(disks): Extension<Disks>,
    Params(query): Params<ListQuery>,
) -> Result<Json<AdminUserList>> {
    let (rows, total) = users::page(
        &state.pool,
        query.query.as_deref(),
        page_size(query.limit),
        query.offset.unwrap_or(0),
    )
    .await?;

    let mut listed = Vec::with_capacity(rows.len());
    for row in &rows {
        listed.push(users::panel_user(&state.pool, row, &live, &disks).await?);
    }
    Ok(Json(AdminUserList { users: listed, total }))
}

async fn create_user(
    State(state): State<AppState>,
    Admin(_): Admin,
    Extension(live): Extension<LiveServers>,
    Extension(disks): Extension<Disks>,
    JsonBody(body): JsonBody<CreateUserRequest>,
) -> Result<Response> {
    let username = body.username.trim().to_owned();
    users::claim_name(&state.pool, &username, None).await?;

    let email = match body.email.as_deref().map(str::trim).filter(|typed| !typed.is_empty()) {
        Some(typed) => {
            let address = registration::address::normalise(typed)?;
            users::claim_email(&state.pool, &address, None).await?;
            Some(address)
        }
        None => None,
    };

    let hash = password::hash(&body.password)?;

    if body.panel_role == PanelRole::Admin && body.limits.is_some() {
        return Err(Failure::invalid_request("an administrator has no limits"));
    }
    let wanted = match body.limits {
        Some(limits) => {
            limits::check(&limits)?;
            limits
        }
        None => settings::load(&state.pool).await?.default_limits,
    };

    let row = users::insert(
        &state.pool,
        users::NewUser {
            username: &username,
            email: email.clone(),
            origin: AccountOrigin::Admin,
            password_hash: hash,
            role: body.panel_role,
            must_change_password: body.must_change_password.unwrap_or(true),
            limits: wanted,
        },
    )
    .await
    .map_err(users::map_taken)?;

    let system = users::provision(&state.pool, &helper(&state), &row).await?;
    if let Some(reason) = &system.error_message {
        tracing::warn!(user = %row.id, "the helper could not set up the account: {reason}");
    }
    let row = UserRow {
        system_state: system.state,
        system_uid: system.uid,
        system_error_message: system.error_message,
        ..row
    };

    let answer = users::panel_user(&state.pool, &row, &live, &disks).await?;
    Ok((StatusCode::CREATED, Json(answer)).into_response())
}

async fn one_user(
    State(state): State<AppState>,
    Admin(_): Admin,
    Extension(live): Extension<LiveServers>,
    Extension(disks): Extension<Disks>,
    Path(user_id): Path<String>,
) -> Result<Json<AdminUserDetail>> {
    let row = users::load(&state.pool, parse_id(&user_id)?).await?;
    Ok(Json(detail(&state.pool, &row, &live, &disks).await?))
}

async fn update_user(
    State(state): State<AppState>,
    Admin(_): Admin,
    Extension(live): Extension<LiveServers>,
    Extension(disks): Extension<Disks>,
    Path(user_id): Path<String>,
    JsonBody(body): JsonBody<UpdateUserRequest>,
) -> Result<Json<AdminUserDetail>> {
    let row = users::load(&state.pool, parse_id(&user_id)?).await?;
    users::refuse_if_busy(&row)?;

    let name = match &body.username {
        Some(wanted) => {
            let wanted = wanted.trim();
            users::claim_name(&state.pool, wanted, Some(row.id)).await?;
            Some(wanted.to_owned())
        }
        None => None,
    };

    let email = match &body.email {
        Some(Some(typed)) if !typed.trim().is_empty() => {
            let address = registration::address::normalise(typed)?;
            users::claim_email(&state.pool, &address, Some(row.id)).await?;
            Some(Some(address))
        }
        Some(_) => Some(None),
        None => None,
    };

    let address_changed =
        matches!(&email, Some(wanted) if wanted.as_deref() != row.email.as_deref());

    if body.panel_role == Some(PanelRole::User) && users::is_last_admin(&state.pool, &row).await? {
        return Err(Failure::conflict(
            "last_admin",
            "a panel without an administrator cannot get one back",
        ));
    }

    let new_role = body.panel_role.filter(|wanted| *wanted != row.role);

    let hash = body.password.as_deref().map(password::hash).transpose()?;

    let now = Timestamp::now();
    let mut change = state.pool.begin().await?;
    if let Some(name) = name {
        sqlx::query("UPDATE users SET username = ?, updated_at = ? WHERE id = ?")
            .bind(name)
            .bind(now)
            .bind(row.id)
            .execute(&mut *change)
            .await
            .map_err(users::map_taken)?;
    }
    if let Some(hash) = hash {
        sqlx::query("UPDATE users SET password_hash = ?, updated_at = ? WHERE id = ?")
            .bind(hash)
            .bind(now)
            .bind(row.id)
            .execute(&mut *change)
            .await?;
    }
    if let Some(role) = body.panel_role {
        sqlx::query("UPDATE users SET role = ?, updated_at = ? WHERE id = ?")
            .bind(role)
            .bind(now)
            .bind(row.id)
            .execute(&mut *change)
            .await?;
    }
    if let Some(address) = &email {
        sqlx::query("UPDATE users SET email = ?, updated_at = ? WHERE id = ?")
            .bind(address.as_deref())
            .bind(now)
            .bind(row.id)
            .execute(&mut *change)
            .await
            .map_err(users::map_taken)?;
    }
    if let Some(must_change) = body.must_change_password {
        sqlx::query("UPDATE users SET must_change_password = ?, updated_at = ? WHERE id = ?")
            .bind(must_change)
            .bind(now)
            .bind(row.id)
            .execute(&mut *change)
            .await?;
    }
    change.commit().await?;

    if let Some(role) = new_role {
        settle_cgroup(&state, &row, role).await?;
    }

    if body.password.is_some() {
        session::close_all_of(&state.pool, row.id, None).await?;
        reset::forget_all(&state.pool, row.id).await?;
    }

    if address_changed && body.password.is_none() {
        let gone = reset::forget_all(&state.pool, row.id).await?;
        if gone > 0 {
            tracing::info!(user = %row.id, gone, "the address changed, so the open reset links fell");
        }
    }

    let row = users::load(&state.pool, row.id).await?;
    Ok(Json(detail(&state.pool, &row, &live, &disks).await?))
}

async fn settle_cgroup(state: &AppState, row: &UserRow, role: PanelRole) -> Result<()> {
    if row.system_state != SystemUserState::Ready {
        return Ok(());
    }

    let budget = limits::Budget::of(role, row.limits());
    let Err(err) = helper(state).apply_limits(&row.id.to_string(), budget.to_cgroup()).await
    else {
        return Ok(());
    };

    tracing::error!(user = %row.id, "the cgroup still holds the limits of the old role: {err:#}");
    sqlx::query("UPDATE users SET system_error_message = ?, updated_at = ? WHERE id = ?")
        .bind(format!("limits were not applied: {err:#}"))
        .bind(Timestamp::now())
        .bind(row.id)
        .execute(&state.pool)
        .await?;
    Ok(())
}

async fn delete_user(
    State(state): State<AppState>,
    Admin(admin): Admin,
    Extension(live): Extension<LiveServers>,
    Extension(playit): Extension<Arc<Playit>>,
    Extension(drive): Extension<Arc<Drive>>,
    Path(user_id): Path<String>,
    Params(query): Params<DeleteQuery>,
) -> Result<StatusCode> {
    let row = users::load(&state.pool, parse_id(&user_id)?).await?;
    if row.id == admin.id() {
        return Err(Failure::new(
            StatusCode::FORBIDDEN,
            "cannot_delete_self",
            "an administrator cannot delete himself",
        ));
    }
    users::refuse_if_busy(&row)?;
    if users::is_last_admin(&state.pool, &row).await? {
        return Err(Failure::conflict("last_admin", "this is the last administrator"));
    }

    let owned = users::owned_servers(&state.pool, row.id).await?;
    let decision = match (owned.is_empty(), query.servers) {
        (true, _) => None,
        (false, None) => {
            return Err(Failure::conflict(
                "user_has_servers",
                "say what should happen to the servers: ?servers=delete or ?servers=transfer",
            ))
        }
        (false, Some(decision)) => Some(decision),
    };

    let receiver = match decision {
        Some(DeleteUserServers::Transfer) => Some(transfer_target(&state, &query, &row).await?),
        _ => None,
    };

    if !users::claim_busy(&state.pool, row.id).await? {
        return Err(users::busy());
    }
    let outcome =
        dispose_of(&state, &live, &playit, &drive, &row, &owned, decision, receiver.as_ref()).await;
    if outcome.is_err() {
        users::set_busy(&state.pool, row.id, false).await?;
    }
    outcome?;

    Ok(StatusCode::NO_CONTENT)
}

async fn transfer_target(
    state: &AppState,
    query: &DeleteQuery,
    leaving: &UserRow,
) -> Result<UserRow> {
    let invalid = || Failure::bad_request("invalid_transfer_target", "no one to hand the servers to");

    let wanted = query.transfer_to.as_deref().ok_or_else(invalid)?;
    let wanted = wanted.parse::<Id>().map_err(|_| invalid())?;
    let target = users::find(&state.pool, wanted).await?.ok_or_else(invalid)?;

    if target.id == leaving.id || target.system_state != SystemUserState::Ready {
        return Err(invalid());
    }
    users::refuse_if_busy(&target)?;
    Ok(target)
}

async fn dispose_of(
    state: &AppState,
    live: &LiveServers,
    playit: &Arc<Playit>,
    drive: &Arc<Drive>,
    leaving: &UserRow,
    owned: &[users::OwnedServer],
    decision: Option<DeleteUserServers>,
    receiver: Option<&UserRow>,
) -> Result<()> {
    let ids: Vec<Id> = owned.iter().map(|server| server.id).collect();
    if !live.among(&ids).await.is_empty() {
        return Err(Failure::conflict(
            "servers_running",
            "stop the servers of this account first; we shoot nothing down",
        ));
    }

    if let (Some(DeleteUserServers::Transfer), Some(receiver)) = (decision, receiver) {
        hand_over(state, playit, leaving, receiver, owned).await?;
    }

    playit.dispose_of(leaving.id).await;

    drive.dispose_of(leaving.id).await;

    if matches!(decision, Some(DeleteUserServers::Delete)) {
        sqlx::query("DELETE FROM servers WHERE owner_id = ?")
            .bind(leaving.id)
            .execute(&state.pool)
            .await?;
    }

    sqlx::query("DELETE FROM users WHERE id = ?").bind(leaving.id).execute(&state.pool).await?;

    let helper = helper(state);
    let user_id = leaving.id.to_string();
    let backups: Vec<PathBuf> = match decision {
        Some(DeleteUserServers::Delete) => {
            ids.iter().map(|id| backup_dir(&state.config, *id)).collect()
        }
        _ => Vec::new(),
    };
    tokio::spawn(async move {
        for directory in backups {
            if let Err(err) = tokio::fs::remove_dir_all(&directory).await {
                if err.kind() != std::io::ErrorKind::NotFound {
                    tracing::warn!("could not remove {}: {err}", directory.display());
                }
            }
        }
        if let Err(err) = helper.delete_user(&user_id, true).await {
            tracing::warn!("the system account {user_id} is still there: {err:#}");
        }
    });

    Ok(())
}

async fn hand_over(
    state: &AppState,
    playit: &Arc<Playit>,
    leaving: &UserRow,
    receiver: &UserRow,
    owned: &[users::OwnedServer],
) -> Result<()> {
    for server in owned {
        playit.release_tunnel(server.id).await;
    }

    let mut moved: Vec<PathBuf> = Vec::new();
    for server in owned {
        let from = server_dir(&state.config, leaving.id, server.id);
        let to = server_dir(&state.config, receiver.id, server.id);
        if !from.exists() {
            continue;
        }
        if let Err(err) = move_directory(&from, &to) {
            for done in &moved {
                let back = server_dir(&state.config, leaving.id, folder_name(done));
                let _ = std::fs::rename(done, back);
            }
            return Err(Failure::internal(err));
        }
        moved.push(to);
    }

    sqlx::query("UPDATE servers SET owner_id = ?, updated_at = ? WHERE owner_id = ?")
        .bind(receiver.id)
        .bind(Timestamp::now())
        .bind(leaving.id)
        .execute(&state.pool)
        .await?;

    users::set_busy(&state.pool, receiver.id, true).await?;
    let helper = helper(state);
    let pool = state.pool.clone();
    let receiver_id = receiver.id;
    tokio::spawn(async move {
        for directory in moved {
            let steps = crate::helper::in_servers(folder_name(&directory));
            if let Err(err) = helper.chown_tree(&receiver_id.to_string(), steps).await {
                tracing::error!("{} still belongs to the old account: {err:#}", directory.display());
            }
        }
        if let Err(err) = users::set_busy(&pool, receiver_id, false).await {
            tracing::error!("could not release {receiver_id}: {err}");
        }
    });

    Ok(())
}

fn move_directory(from: &FsPath, to: &FsPath) -> std::io::Result<()> {
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::rename(from, to)
}

fn folder_name(path: &FsPath) -> Id {
    path.file_name()
        .and_then(|name| name.to_str())
        .and_then(|name| name.parse().ok())
        .expect("the paths we build end in a server id")
}

fn server_dir(config: &Config, owner: Id, server: Id) -> PathBuf {
    config.users_dir().join(owner.to_string()).join("servers").join(server.to_string())
}

fn backup_dir(config: &Config, server: Id) -> PathBuf {
    config.data_dir.join("backups").join(server.to_string())
}

async fn read_limits(
    State(state): State<AppState>,
    Admin(_): Admin,
    Extension(live): Extension<LiveServers>,
    Extension(disks): Extension<Disks>,
    Path(user_id): Path<String>,
) -> Result<Json<UserLimitsResponse>> {
    let row = users::load(&state.pool, parse_id(&user_id)?).await?;
    Ok(Json(limits_response(&state.pool, &state.config.data_dir, &row, &live, &disks).await?))
}

async fn write_limits(
    State(state): State<AppState>,
    Admin(_): Admin,
    Extension(live): Extension<LiveServers>,
    Extension(disks): Extension<Disks>,
    Path(user_id): Path<String>,
    JsonBody(wanted): JsonBody<UserLimits>,
) -> Result<Json<UserLimitsResponse>> {
    let row = users::load(&state.pool, parse_id(&user_id)?).await?;
    users::refuse_if_busy(&row)?;
    refuse_limits_for_an_admin(&row)?;
    limits::check(&wanted)?;

    helper(&state)
        .apply_limits(&row.id.to_string(), limits::Budget::of(row.role, wanted).to_cgroup())
        .await
        .map_err(Failure::internal)?;

    sqlx::query(
        "UPDATE users SET memory_mib = ?, cpu_mode = ?, cpu_cores = ?, pids_max = ?, \
         disk_mib = ?, updated_at = ? WHERE id = ?",
    )
    .bind(wanted.memory_mib)
    .bind(wanted.cpu_mode)
    .bind(wanted.cpu_cores)
    .bind(wanted.pids_max)
    .bind(wanted.disk_mib)
    .bind(Timestamp::now())
    .bind(row.id)
    .execute(&state.pool)
    .await?;

    let row = users::load(&state.pool, row.id).await?;
    Ok(Json(limits_response(&state.pool, &state.config.data_dir, &row, &live, &disks).await?))
}

async fn retry_system_user(
    State(state): State<AppState>,
    Admin(_): Admin,
    Extension(live): Extension<LiveServers>,
    Extension(disks): Extension<Disks>,
    Path(user_id): Path<String>,
) -> Result<Json<AdminUserDetail>> {
    let row = users::load(&state.pool, parse_id(&user_id)?).await?;

    if row.system_state != SystemUserState::Ready {
        let system = users::provision(&state.pool, &helper(&state), &row).await?;
        if system.state != SystemUserState::Ready {
            return Err(Failure::conflict(
                "system_user_not_ready",
                system.error_message.unwrap_or_else(|| "the helper refused again".to_owned()),
            ));
        }
    }

    let row = users::load(&state.pool, row.id).await?;
    Ok(Json(detail(&state.pool, &row, &live, &disks).await?))
}

async fn read_settings(
    State(state): State<AppState>,
    Admin(_): Admin,
) -> Result<Json<PanelSettings>> {
    Ok(Json(settings::load(&state.pool).await?))
}

async fn write_settings(
    State(state): State<AppState>,
    Admin(_): Admin,
    JsonBody(wanted): JsonBody<PanelSettings>,
) -> Result<Json<PanelSettings>> {
    settings::save(&state.pool, &wanted).await?;
    Ok(Json(settings::load(&state.pool).await?))
}

async fn detail(
    pool: &SqlitePool,
    row: &UserRow,
    live: &LiveServers,
    disks: &Disks,
) -> Result<AdminUserDetail> {
    let owned = users::owned_servers(pool, row.id).await?;
    let ids: Vec<Id> = owned.iter().map(|server| server.id).collect();
    let running = live.among(&ids).await;

    Ok(AdminUserDetail {
        user: users::panel_user(pool, row, live, disks).await?,
        owned_servers: owned
            .into_iter()
            .map(|server| OwnedServerRef {
                running: running.contains(&server.id),
                id: server.id,
                name: server.name,
                memory_mib: server.memory_mib,
            })
            .collect(),
        active_sessions: session::count_active(pool, row.id, Timestamp::now()).await?,
    })
}

async fn limits_response(
    pool: &SqlitePool,
    data_dir: &FsPath,
    row: &UserRow,
    live: &LiveServers,
    disks: &Disks,
) -> Result<UserLimitsResponse> {
    let machine = usage::Host::measure();
    Ok(UserLimitsResponse {
        limits: row.budget().limits(),
        usage: users::measure(pool, row, live, disks).await?,
        host: HostRoom {
            cpu_cores: machine.cpu_cores,
            assignable_memory_mib: machine.assignable_memory_mib(),
            assignable_disk_mib: (crate::files::filesystem_total_bytes(data_dir) / MIB) as u32,
        },
    })
}

fn helper(state: &AppState) -> Helper {
    Helper::new(&state.config.helper_socket)
}

fn refuse_limits_for_an_admin(row: &UserRow) -> Result<()> {
    if row.is_admin() {
        return Err(Failure::conflict(
            "role_unlimited",
            "an administrator has no limits; demote the account first",
        ));
    }
    Ok(())
}

fn parse_id(raw: &str) -> Result<Id> {
    raw.parse().map_err(|_| Failure::not_found("user_not_found", "no such user"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::harness::*;
    use crate::model::CpuMode;
    use tower::ServiceExt;

    fn app(pool: &SqlitePool, helper: &FakeHelper) -> Router {
        app_with(pool, helper, LiveServers::none())
    }

    fn app_with(pool: &SqlitePool, helper: &FakeHelper, live: LiveServers) -> Router {
        let config = Config { helper_socket: helper.socket(), ..Config::default() };
        let playit = Playit::against(pool.clone(), Arc::new(config.clone()), "http://127.0.0.1:1")
            .expect("the playit service");
        let drive = Drive::against(
            pool.clone(),
            &std::env::temp_dir().join(format!("craftpanel-admin-drive-{}", Id::new())),
            "http://127.0.0.1:1",
            "http://127.0.0.1:1",
        );
        with_live(live, Disks::none(), playit, drive).with_state(state_with(pool, config))
    }

    async fn an_admin_session(pool: &SqlitePool) -> String {
        let id = an_admin(pool, "boss").await;
        sign_in(pool, id).await
    }

    fn new_user(username: &str) -> serde_json::Value {
        serde_json::json!({
            "username": username,
            "password": "first-password-please-change",
            "panel_role": "user",
        })
    }

    #[tokio::test]
    async fn everything_under_admin_is_closed_to_ordinary_users() {
        let pool = test_pool().await;
        let fake = FakeHelper::obliging().await;
        let max = a_user(&pool, "max").await;
        let secret = sign_in(&pool, max).await;
        let target = max.to_string();

        let calls: Vec<(&str, String)> = vec![
            ("GET", "/admin/host".to_owned()),
            ("GET", "/admin/users".to_owned()),
            ("POST", "/admin/users".to_owned()),
            ("GET", format!("/admin/users/{target}")),
            ("PATCH", format!("/admin/users/{target}")),
            ("DELETE", format!("/admin/users/{target}")),
            ("GET", format!("/admin/users/{target}/limits")),
            ("PUT", format!("/admin/users/{target}/limits")),
            ("POST", format!("/admin/users/{target}/system-user/retry")),
            ("GET", "/admin/settings".to_owned()),
            ("PUT", "/admin/settings".to_owned()),
        ];

        for (method, uri) in calls {
            let request = as_user(send(method, &uri, serde_json::json!({})), &secret);
            let response = app(&pool, &fake).oneshot(request).await.unwrap();
            assert_eq!(response.status(), StatusCode::FORBIDDEN, "{method} {uri}");
            assert_eq!(body_json(response).await["error"], "forbidden", "{method} {uri}");
        }
    }

    #[tokio::test]
    async fn without_a_session_the_admin_area_says_unauthenticated_not_forbidden() {
        let pool = test_pool().await;
        let fake = FakeHelper::obliging().await;

        let response = app(&pool, &fake).oneshot(fetch("/admin/users")).await.unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(body_json(response).await["error"], "unauthenticated");
    }

    #[tokio::test]
    async fn creating_a_user_makes_a_system_account_along_with_it() {
        let pool = test_pool().await;
        let fake = FakeHelper::obliging().await;
        let secret = an_admin_session(&pool).await;

        let response = app(&pool, &fake)
            .oneshot(as_user(send("POST", "/admin/users", new_user("andre")), &secret))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);

        let body = body_json(response).await;
        assert_eq!(body["username"], "andre");
        assert_eq!(body["system_user"]["state"], "ready");
        assert_eq!(body["system_user"]["uid"], 6100);
        assert_eq!(body["must_change_password"], true, "12.3: true unless told otherwise");
        assert_eq!(body["limits"]["memory_mib"], 4096, "the panel default");
        assert_eq!(body["usage"]["servers"]["total"], 0);
        assert!(body["created_at"].as_str().unwrap().ends_with('Z'));

        let created = craftpanel_proto::HelperRequest::CreateUser {
            user_id: body["id"].as_str().unwrap().to_owned(),
        };
        assert!(
            fake.calls().iter().any(|call| format!("{call:?}") == format!("{created:?}")),
            "the helper was asked for exactly this id"
        );
    }

    #[tokio::test]
    async fn a_helper_that_refuses_still_leaves_an_account_that_can_sign_in() {
        let pool = test_pool().await;
        let fake = FakeHelper::refusing().await;
        let secret = an_admin_session(&pool).await;

        let response = app(&pool, &fake)
            .oneshot(as_user(send("POST", "/admin/users", new_user("andre")), &secret))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CREATED, "12.3: not a 500");

        let body = body_json(response).await;
        assert_eq!(body["system_user"]["state"], "error");
        assert!(body["system_user"]["error_message"].as_str().unwrap().contains("UID range"));

        let row = users::by_name(&pool, "andre").await.unwrap().expect("the account is there");
        assert_eq!(row.system_state, SystemUserState::Error);
    }

    #[tokio::test]
    async fn the_rules_for_names_and_passwords_hold_here_too() {
        let pool = test_pool().await;
        let fake = FakeHelper::obliging().await;
        let secret = an_admin_session(&pool).await;
        a_user(&pool, "anna").await;

        let cases = [
            (new_user("anna"), StatusCode::CONFLICT, "username_taken"),
            (new_user("Andre"), StatusCode::BAD_REQUEST, "invalid_request"),
            (new_user("an"), StatusCode::BAD_REQUEST, "invalid_request"),
            (
                serde_json::json!({
                    "username": "andre", "password": "short", "panel_role": "user"
                }),
                StatusCode::BAD_REQUEST,
                "weak_password",
            ),
            (
                serde_json::json!({
                    "username": "andre", "password": "longenoughyesyes", "panel_role": "chef"
                }),
                StatusCode::BAD_REQUEST,
                "invalid_request",
            ),
        ];

        for (body, status, code) in cases {
            let response = app(&pool, &fake)
                .oneshot(as_user(send("POST", "/admin/users", body.clone()), &secret))
                .await
                .unwrap();
            assert_eq!(response.status(), status, "{body}");
            assert_eq!(body_json(response).await["error"], code, "{body}");
        }

        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM users WHERE username = 'andre'")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 0, "nothing half-created stayed behind");
    }

    #[tokio::test]
    async fn the_list_pages_and_counts() {
        let pool = test_pool().await;
        let fake = FakeHelper::obliging().await;
        let secret = an_admin_session(&pool).await;
        for name in ["anna", "andre", "max"] {
            a_user(&pool, name).await;
        }

        let body = body_json(
            app(&pool, &fake).oneshot(as_user(fetch("/admin/users?limit=2"), &secret)).await.unwrap(),
        )
        .await;
        assert_eq!(body["users"].as_array().unwrap().len(), 2);
        assert_eq!(body["total"], 4, "three plus the administrator");

        let filtered = body_json(
            app(&pool, &fake)
                .oneshot(as_user(fetch("/admin/users?query=an"), &secret))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(filtered["total"], 2);
        assert_eq!(filtered["users"][0]["username"], "andre");
    }

    #[tokio::test]
    async fn one_user_carries_his_servers_and_his_sessions() {
        let pool = test_pool().await;
        let fake = FakeHelper::obliging().await;
        let secret = an_admin_session(&pool).await;
        let max = a_user(&pool, "max").await;
        let running = a_server(&pool, max, "one", 2048).await;
        a_server(&pool, max, "two", 1024).await;
        sign_in(&pool, max).await;
        sign_in(&pool, max).await;

        let body = body_json(
            app_with(&pool, &fake, LiveServers::fixed([running]))
                .oneshot(as_user(fetch(&format!("/admin/users/{max}")), &secret))
                .await
                .unwrap(),
        )
        .await;

        assert_eq!(body["active_sessions"], 2);
        let servers = body["owned_servers"].as_array().unwrap();
        assert_eq!(servers.len(), 2);
        assert_eq!(servers[0]["name"], "one");
        assert_eq!(servers[0]["running"], true);
        assert_eq!(servers[1]["running"], false);
        assert_eq!(body["usage"]["servers"]["running"], 1);
    }

    #[tokio::test]
    async fn an_unknown_or_unreadable_id_is_a_missing_user() {
        let pool = test_pool().await;
        let fake = FakeHelper::obliging().await;
        let secret = an_admin_session(&pool).await;

        for uri in [format!("/admin/users/{}", Id::new()), "/admin/users/not-a-ulid".to_owned()] {
            let response =
                app(&pool, &fake).oneshot(as_user(fetch(&uri), &secret)).await.unwrap();
            assert_eq!(response.status(), StatusCode::NOT_FOUND, "{uri}");
            assert_eq!(body_json(response).await["error"], "user_not_found", "{uri}");
        }
    }

    #[tokio::test]
    async fn renaming_leaves_the_system_account_alone() {
        let pool = test_pool().await;
        let fake = FakeHelper::obliging().await;
        let secret = an_admin_session(&pool).await;
        let max = a_user(&pool, "max").await;
        let before = users::load(&pool, max).await.unwrap().system_user().name;

        let body = body_json(
            app(&pool, &fake)
                .oneshot(as_user(
                    send("PATCH", &format!("/admin/users/{max}"), serde_json::json!({ "username": "moritz" })),
                    &secret,
                ))
                .await
                .unwrap(),
        )
        .await;

        assert_eq!(body["username"], "moritz");
        assert_eq!(body["system_user"]["name"], before, "the account is named after the id");
    }

    #[tokio::test]
    async fn setting_a_password_needs_no_old_one_and_ends_every_session() {
        let pool = test_pool().await;
        let fake = FakeHelper::obliging().await;
        let secret = an_admin_session(&pool).await;
        let max = a_user(&pool, "max").await;
        let his = sign_in(&pool, max).await;

        let response = app(&pool, &fake)
            .oneshot(as_user(
                send(
                    "PATCH",
                    &format!("/admin/users/{max}"),
                    serde_json::json!({ "password": "forgotten-so-new" }),
                ),
                &secret,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        assert_eq!(session::count_active(&pool, max, Timestamp::now()).await.unwrap(), 0);
        assert!(crate::auth::session::lookup(&pool, &his, Timestamp::now()).await.unwrap().is_none());
        let row = users::load(&pool, max).await.unwrap();
        assert!(password::verify("forgotten-so-new", &row.password_hash));
    }

    #[tokio::test]
    async fn the_last_administrator_cannot_be_demoted_by_anyone_including_himself() {
        let pool = test_pool().await;
        let fake = FakeHelper::obliging().await;
        let boss = an_admin(&pool, "boss").await;
        let secret = sign_in(&pool, boss).await;

        let refusal = app(&pool, &fake)
            .oneshot(as_user(
                send(
                    "PATCH",
                    &format!("/admin/users/{boss}"),
                    serde_json::json!({ "panel_role": "user" }),
                ),
                &secret,
            ))
            .await
            .unwrap();
        assert_eq!(refusal.status(), StatusCode::CONFLICT);
        assert_eq!(body_json(refusal).await["error"], "last_admin");

        let second = an_admin(&pool, "deputy").await;
        let allowed = app(&pool, &fake)
            .oneshot(as_user(
                send(
                    "PATCH",
                    &format!("/admin/users/{boss}"),
                    serde_json::json!({ "panel_role": "user" }),
                ),
                &secret,
            ))
            .await
            .unwrap();
        assert_eq!(allowed.status(), StatusCode::OK, "there is another one now");
        assert!(users::load(&pool, second).await.unwrap().is_admin());
        assert!(!users::load(&pool, boss).await.unwrap().is_admin());
    }

    #[tokio::test]
    async fn a_decision_the_contract_does_not_know_is_an_invalid_request() {
        let pool = test_pool().await;
        let fake = FakeHelper::obliging().await;
        let secret = an_admin_session(&pool).await;
        let max = a_user(&pool, "max").await;

        let response = app(&pool, &fake)
            .oneshot(as_user(empty("DELETE", &format!("/admin/users/{max}?servers=burn")), &secret))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(body_json(response).await["error"], "invalid_request");
        assert!(users::find(&pool, max).await.unwrap().is_some());
    }

    #[tokio::test]
    async fn a_user_with_servers_is_not_deleted_without_a_decision() {
        let pool = test_pool().await;
        let fake = FakeHelper::obliging().await;
        let secret = an_admin_session(&pool).await;
        let max = a_user(&pool, "max").await;
        a_server(&pool, max, "one", 2048).await;

        let response = app(&pool, &fake)
            .oneshot(as_user(empty("DELETE", &format!("/admin/users/{max}")), &secret))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CONFLICT);
        assert_eq!(body_json(response).await["error"], "user_has_servers");
        assert!(users::find(&pool, max).await.unwrap().is_some());
    }

    #[tokio::test]
    async fn a_running_server_is_never_shot_down_to_delete_its_owner() {
        let pool = test_pool().await;
        let fake = FakeHelper::obliging().await;
        let secret = an_admin_session(&pool).await;
        let max = a_user(&pool, "max").await;
        let server = a_server(&pool, max, "one", 2048).await;

        let response = app_with(&pool, &fake, LiveServers::fixed([server]))
            .oneshot(as_user(
                empty("DELETE", &format!("/admin/users/{max}?servers=delete")),
                &secret,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CONFLICT);
        assert_eq!(body_json(response).await["error"], "servers_running");

        assert!(users::find(&pool, max).await.unwrap().is_some());
        assert!(!users::load(&pool, max).await.unwrap().busy, "the flag was let go again");
    }

    #[tokio::test]
    async fn deleting_with_the_servers_takes_the_rows_with_it() {
        let pool = test_pool().await;
        let fake = FakeHelper::obliging().await;
        let secret = an_admin_session(&pool).await;
        let max = a_user(&pool, "max").await;
        let server = a_server(&pool, max, "one", 2048).await;

        let response = app(&pool, &fake)
            .oneshot(as_user(
                empty("DELETE", &format!("/admin/users/{max}?servers=delete")),
                &secret,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NO_CONTENT);

        assert!(users::find(&pool, max).await.unwrap().is_none());
        let left: i64 = sqlx::query_scalar("SELECT count(*) FROM servers WHERE id = ?")
            .bind(server)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(left, 0);
    }

    #[tokio::test]
    async fn transferring_moves_the_servers_and_keeps_them() {
        let pool = test_pool().await;
        let fake = FakeHelper::obliging().await;
        let secret = an_admin_session(&pool).await;
        let max = a_user(&pool, "max").await;
        let anna = a_user(&pool, "anna").await;
        let server = a_server(&pool, max, "one", 2048).await;

        let response = app(&pool, &fake)
            .oneshot(as_user(
                empty(
                    "DELETE",
                    &format!("/admin/users/{max}?servers=transfer&transfer_to={anna}"),
                ),
                &secret,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NO_CONTENT);

        assert!(users::find(&pool, max).await.unwrap().is_none());
        let owner: Id = sqlx::query_scalar("SELECT owner_id FROM servers WHERE id = ?")
            .bind(server)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(owner, anna, "the server outlived its owner");
    }

    #[tokio::test]
    async fn a_transfer_needs_a_target_that_is_ready_and_someone_else() {
        let pool = test_pool().await;
        let fake = FakeHelper::obliging().await;
        let secret = an_admin_session(&pool).await;
        let max = a_user(&pool, "max").await;
        a_server(&pool, max, "one", 2048).await;

        let unfinished = a_user(&pool, "anna").await;
        sqlx::query("UPDATE users SET system_state = 'error' WHERE id = ?")
            .bind(unfinished)
            .execute(&pool)
            .await
            .unwrap();

        let targets = [
            format!("/admin/users/{max}?servers=transfer"),
            format!("/admin/users/{max}?servers=transfer&transfer_to={max}"),
            format!("/admin/users/{max}?servers=transfer&transfer_to={}", Id::new()),
            format!("/admin/users/{max}?servers=transfer&transfer_to=nonsense"),
            format!("/admin/users/{max}?servers=transfer&transfer_to={unfinished}"),
        ];

        for uri in targets {
            let response =
                app(&pool, &fake).oneshot(as_user(empty("DELETE", &uri), &secret)).await.unwrap();
            assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{uri}");
            assert_eq!(body_json(response).await["error"], "invalid_transfer_target", "{uri}");
        }
        assert!(users::find(&pool, max).await.unwrap().is_some());
    }

    #[tokio::test]
    async fn an_admin_does_not_delete_himself_but_may_delete_a_second_one() {
        let pool = test_pool().await;
        let fake = FakeHelper::obliging().await;
        let boss = an_admin(&pool, "boss").await;
        let secret = sign_in(&pool, boss).await;

        let himself = app(&pool, &fake)
            .oneshot(as_user(empty("DELETE", &format!("/admin/users/{boss}")), &secret))
            .await
            .unwrap();
        assert_eq!(himself.status(), StatusCode::FORBIDDEN);
        assert_eq!(body_json(himself).await["error"], "cannot_delete_self");
        assert!(users::find(&pool, boss).await.unwrap().is_some());

        let other = an_admin(&pool, "deputy").await;
        let response = app(&pool, &fake)
            .oneshot(as_user(empty("DELETE", &format!("/admin/users/{other}")), &secret))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        assert!(users::find(&pool, other).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn a_busy_account_turns_every_other_change_away() {
        let pool = test_pool().await;
        let fake = FakeHelper::obliging().await;
        let secret = an_admin_session(&pool).await;
        let max = a_user(&pool, "max").await;
        users::set_busy(&pool, max, true).await.unwrap();

        let calls: Vec<(&str, String, serde_json::Value)> = vec![
            ("PATCH", format!("/admin/users/{max}"), serde_json::json!({ "username": "moritz" })),
            (
                "PUT",
                format!("/admin/users/{max}/limits"),
                serde_json::json!({
                    "memory_mib": 2048, "cpu_mode": "cap", "cpu_cores": 1.0,
                        "pids_max": 512, "disk_mib": 51200
                }),
            ),
            ("DELETE", format!("/admin/users/{max}"), serde_json::json!({})),
        ];

        for (method, uri, body) in calls {
            let response = app(&pool, &fake)
                .oneshot(as_user(send(method, &uri, body), &secret))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::CONFLICT, "{method} {uri}");
            assert_eq!(body_json(response).await["error"], "user_busy", "{method} {uri}");
        }
    }

    #[tokio::test]
    async fn a_patch_that_is_refused_writes_none_of_its_fields() {
        let pool = test_pool().await;
        let fake = FakeHelper::obliging().await;
        let boss = an_admin(&pool, "boss").await;
        let secret = sign_in(&pool, boss).await;
        let max = a_user(&pool, "max").await;
        a_user(&pool, "anna").await;

        let refusals = [
            (serde_json::json!({ "username": "moritz", "password": "short" }), "weak_password"),
            (
                serde_json::json!({ "username": "moritz", "panel_role": "admin" }),
                "username_taken",
            ),
        ];
        for (body, code) in refusals {
            let mut wanted = body.clone();
            if code == "username_taken" {
                wanted["username"] = serde_json::json!("anna");
            }
            let response = app(&pool, &fake)
                .oneshot(as_user(send("PATCH", &format!("/admin/users/{max}"), wanted), &secret))
                .await
                .unwrap();
            assert_eq!(body_json(response).await["error"], code);

            let row = users::load(&pool, max).await.unwrap();
            assert_eq!(row.username, "max", "{code} left a new name behind");
            assert!(!row.is_admin(), "{code} left a new role behind");
            assert!(password::verify(PASSWORD, &row.password_hash), "{code} touched the password");
        }

        let last = app(&pool, &fake)
            .oneshot(as_user(
                send(
                    "PATCH",
                    &format!("/admin/users/{boss}"),
                    serde_json::json!({ "username": "chef", "panel_role": "user" }),
                ),
                &secret,
            ))
            .await
            .unwrap();
        assert_eq!(body_json(last).await["error"], "last_admin");
        assert_eq!(users::load(&pool, boss).await.unwrap().username, "boss", "renamed anyway");
    }

    #[tokio::test]
    async fn servers_are_not_handed_to_an_account_that_is_already_busy() {
        let pool = test_pool().await;
        let fake = FakeHelper::obliging().await;
        let secret = an_admin_session(&pool).await;
        let max = a_user(&pool, "max").await;
        let anna = a_user(&pool, "anna").await;
        a_server(&pool, max, "one", 2048).await;
        users::set_busy(&pool, anna, true).await.unwrap();

        let response = app(&pool, &fake)
            .oneshot(as_user(
                empty("DELETE", &format!("/admin/users/{max}?servers=transfer&transfer_to={anna}")),
                &secret,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CONFLICT);
        assert_eq!(body_json(response).await["error"], "user_busy");

        assert!(users::find(&pool, max).await.unwrap().is_some());
        assert!(!users::load(&pool, max).await.unwrap().busy, "the flag was let go again");
    }

    #[tokio::test]
    async fn only_one_deletion_of_an_account_gets_through() {
        let pool = test_pool().await;
        let max = a_user(&pool, "max").await;

        assert!(users::claim_busy(&pool, max).await.unwrap(), "the first one takes it");
        assert!(!users::claim_busy(&pool, max).await.unwrap(), "the second one finds it taken");

        users::set_busy(&pool, max, false).await.unwrap();
        assert!(users::claim_busy(&pool, max).await.unwrap(), "and again once it is let go");
    }

    #[tokio::test]
    async fn an_address_is_set_folded_kept_and_taken_away() {
        let pool = test_pool().await;
        let fake = FakeHelper::obliging().await;
        let secret = an_admin_session(&pool).await;

        let mut asked = new_user("max");
        asked["email"] = serde_json::json!("  Max@Example.TEST  ");
        let created = app(&pool, &fake)
            .oneshot(as_user(send("POST", "/admin/users", asked), &secret))
            .await
            .unwrap();
        assert_eq!(created.status(), StatusCode::CREATED);
        let body = body_json(created).await;
        assert_eq!(body["email"], "max@example.test");
        let max: Id = body["id"].as_str().unwrap().parse().unwrap();

        let patch = |body: serde_json::Value| {
            send("PATCH", &format!("/admin/users/{max}"), body)
        };

        let renamed = app(&pool, &fake)
            .oneshot(as_user(patch(serde_json::json!({ "username": "maxi" })), &secret))
            .await
            .unwrap();
        assert_eq!(renamed.status(), StatusCode::OK);
        assert_eq!(body_json(renamed).await["email"], "max@example.test", "absent means unchanged");

        let moved = app(&pool, &fake)
            .oneshot(as_user(patch(serde_json::json!({ "email": "NEW@example.test" })), &secret))
            .await
            .unwrap();
        assert_eq!(body_json(moved).await["email"], "new@example.test");

        let cleared = app(&pool, &fake)
            .oneshot(as_user(patch(serde_json::json!({ "email": null })), &secret))
            .await
            .unwrap();
        assert_eq!(body_json(cleared).await["email"], serde_json::Value::Null);
        assert_eq!(users::load(&pool, max).await.unwrap().email, None);
    }

    #[tokio::test]
    async fn an_address_another_account_or_an_open_sign_up_holds_is_refused() {
        let pool = test_pool().await;
        let fake = FakeHelper::obliging().await;
        let secret = an_admin_session(&pool).await;

        let mut first = new_user("max");
        first["email"] = serde_json::json!("max@example.test");
        let created = app(&pool, &fake)
            .oneshot(as_user(send("POST", "/admin/users", first), &secret))
            .await
            .unwrap();
        assert_eq!(created.status(), StatusCode::CREATED);
        let max: Id = body_json(created).await["id"].as_str().unwrap().parse().unwrap();

        crate::registration::store::insert(
            &pool,
            crate::registration::store::NewApplication {
                username: "anna",
                email: "anna@example.test",
                password_hash: "x".to_owned(),
                signup_ip: None,
                token_hash: crate::auth::secret::digest("something"),
                token_expires_at: Timestamp::now(),
            },
            Timestamp::now(),
        )
        .await
        .unwrap();

        for (address, code) in [
            ("max@example.test", "email_taken"),
            ("anna@example.test", "email_taken"),
            ("no-at-sign", "invalid_email"),
        ] {
            let mut second = new_user("second");
            second["email"] = serde_json::json!(address);
            let refused = app(&pool, &fake)
                .oneshot(as_user(send("POST", "/admin/users", second), &secret))
                .await
                .unwrap();
            assert_eq!(body_json(refused).await["error"], code, "{address} was let through");
            assert!(users::by_name(&pool, "second").await.unwrap().is_none(), "{address}");
        }

        let anna = app(&pool, &fake)
            .oneshot(as_user(
                send(
                    "PATCH",
                    &format!("/admin/users/{max}"),
                    serde_json::json!({ "email": "anna@example.test" }),
                ),
                &secret,
            ))
            .await
            .unwrap();
        assert_eq!(anna.status(), StatusCode::CONFLICT);
        assert_eq!(body_json(anna).await["error"], "email_taken");

        let his_own = app(&pool, &fake)
            .oneshot(as_user(
                send(
                    "PATCH",
                    &format!("/admin/users/{max}"),
                    serde_json::json!({ "email": "max@example.test", "username": "maxi" }),
                ),
                &secret,
            ))
            .await
            .unwrap();
        assert_eq!(his_own.status(), StatusCode::OK);
        assert_eq!(users::load(&pool, max).await.unwrap().email.as_deref(), Some("max@example.test"));
    }

    #[tokio::test]
    async fn a_changed_address_throws_the_open_reset_links_away_but_saving_the_same_one_does_not() {
        let pool = test_pool().await;
        let fake = FakeHelper::obliging().await;
        let secret = an_admin_session(&pool).await;
        let max = a_user(&pool, "max").await;
        sqlx::query("UPDATE users SET email = ? WHERE id = ?")
            .bind("max@example.test")
            .bind(max)
            .execute(&pool)
            .await
            .unwrap();

        let links = |pool: SqlitePool| async move {
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM password_resets WHERE user_id = ?")
                .bind(max)
                .fetch_one(&pool)
                .await
                .unwrap()
        };
        let patch = |body: serde_json::Value| send("PATCH", &format!("/admin/users/{max}"), body);
        let mint = || reset::mint_for(&pool, max, None, None, Timestamp::now());

        mint().await.unwrap();
        let renamed = app(&pool, &fake)
            .oneshot(as_user(patch(serde_json::json!({ "username": "maxi" })), &secret))
            .await
            .unwrap();
        assert_eq!(renamed.status(), StatusCode::OK);
        assert_eq!(links(pool.clone()).await, 1, "a rename is not an address change");

        let again = app(&pool, &fake)
            .oneshot(as_user(patch(serde_json::json!({ "email": " MAX@example.test " })), &secret))
            .await
            .unwrap();
        assert_eq!(again.status(), StatusCode::OK);
        assert_eq!(links(pool.clone()).await, 1);

        let moved = app(&pool, &fake)
            .oneshot(as_user(patch(serde_json::json!({ "email": "other@example.test" })), &secret))
            .await
            .unwrap();
        assert_eq!(moved.status(), StatusCode::OK);
        assert_eq!(links(pool.clone()).await, 0, "the link went to the old mailbox");

        mint().await.unwrap();
        let cleared = app(&pool, &fake)
            .oneshot(as_user(patch(serde_json::json!({ "email": null })), &secret))
            .await
            .unwrap();
        assert_eq!(cleared.status(), StatusCode::OK);
        assert_eq!(links(pool.clone()).await, 0);

        let his = sign_in(&pool, max).await;
        mint().await.unwrap();
        let moved_again = app(&pool, &fake)
            .oneshot(as_user(patch(serde_json::json!({ "email": "again@example.test" })), &secret))
            .await
            .unwrap();
        assert_eq!(moved_again.status(), StatusCode::OK);
        assert_eq!(links(pool.clone()).await, 0);
        assert_eq!(
            session::count_active(&pool, max, Timestamp::now()).await.unwrap(),
            1,
            "{his} was closed by an address change"
        );
    }

    #[tokio::test]
    async fn the_page_of_12_2_stops_at_two_hundred_however_much_is_asked_for() {
        let pool = test_pool().await;
        let fake = FakeHelper::obliging().await;
        let secret = an_admin_session(&pool).await;

        for uri in ["/admin/users?limit=100000", "/admin/users?limit=0"] {
            let response =
                app(&pool, &fake).oneshot(as_user(fetch(uri), &secret)).await.unwrap();
            assert_eq!(response.status(), StatusCode::OK, "{uri}");
        }
        assert_eq!(page_size(Some(100_000)), PAGE_CEILING, "12.2: at most two hundred");
        assert_eq!(page_size(Some(0)), 1);
        assert_eq!(page_size(None), PAGE);
        assert_eq!(page_size(Some(7)), 7);
    }

    #[tokio::test]
    async fn limits_reach_the_cgroup_and_come_back_with_the_room_left() {
        let pool = test_pool().await;
        let fake = FakeHelper::obliging().await;
        let secret = an_admin_session(&pool).await;
        let max = a_user(&pool, "max").await;

        let wanted = serde_json::json!({
            "memory_mib": 6144, "cpu_mode": "cap", "cpu_cores": 3.0,
                        "pids_max": 1024, "disk_mib": 20480
        });
        let body = body_json(
            app(&pool, &fake)
                .oneshot(as_user(send("PUT", &format!("/admin/users/{max}/limits"), wanted), &secret))
                .await
                .unwrap(),
        )
        .await;

        assert_eq!(body["limits"]["memory_mib"], 6144);
        assert_eq!(body["limits"]["cpu_cores"], 3.0);
        assert_eq!(body["usage"]["memory"]["limit_mib"], 6144);
        assert!(body["host"]["assignable_memory_mib"].as_u64().unwrap() > 0);

        let applied = fake
            .calls()
            .into_iter()
            .find_map(|call| match call {
                craftpanel_proto::HelperRequest::ApplyLimits { limits, .. } => Some(limits),
                _ => None,
            })
            .expect("the cgroup was written");
        assert_eq!(applied.memory_high_bytes, Some(6144 * 1024 * 1024));
        assert_eq!(applied.memory_max_bytes, Some(7680 * 1024 * 1024));
        assert_eq!(applied.cpu_quota_percent, Some(300));
        assert_eq!(applied.pids_max, Some(1024));

        let row = users::load(&pool, max).await.unwrap();
        assert_eq!(row.memory_mib, 6144);
        assert_eq!(row.cpu_cores, 3.0);
    }

    #[tokio::test]
    async fn lowering_a_limit_below_what_is_handed_out_succeeds_and_marks_him_over() {
        let pool = test_pool().await;
        let fake = FakeHelper::obliging().await;
        let secret = an_admin_session(&pool).await;
        let max = a_user(&pool, "max").await;
        let server = a_server(&pool, max, "one", 4096).await;

        let body = body_json(
            app_with(&pool, &fake, LiveServers::fixed([server]))
                .oneshot(as_user(
                    send(
                        "PUT",
                        &format!("/admin/users/{max}/limits"),
                        serde_json::json!({
                            "memory_mib": 2048, "cpu_mode": "cap", "cpu_cores": 1.0,
                            "pids_max": 512, "disk_mib": 51200
                        }),
                    ),
                    &secret,
                ))
                .await
                .unwrap(),
        )
        .await;

        assert_eq!(body["usage"]["over_limit"], true);
        assert_eq!(body["usage"]["over_limit_dimensions"][0], "memory");
        assert_eq!(body["usage"]["servers"]["running"], 1, "nothing was shot down");

        let his = sign_in(&pool, max).await;
        let me = body_json(
            crate::api::session::router()
                .with_state(state(&pool))
                .oneshot(as_user(fetch("/me"), &his))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(me["capabilities"]["can_start_servers"], false);
        assert_eq!(me["capabilities"]["blocked_reason"], "over_limit");
    }

    #[tokio::test]
    async fn a_limit_outside_the_bounds_is_refused_before_the_kernel_hears_of_it() {
        let pool = test_pool().await;
        let fake = FakeHelper::obliging().await;
        let secret = an_admin_session(&pool).await;
        let max = a_user(&pool, "max").await;

        let response = app(&pool, &fake)
            .oneshot(as_user(
                send(
                    "PUT",
                    &format!("/admin/users/{max}/limits"),
                    serde_json::json!({
                        "memory_mib": 128, "cpu_mode": "cap", "cpu_cores": 1.0,
                        "pids_max": 512, "disk_mib": 51200
                    }),
                ),
                &secret,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(body_json(response).await["error"], "invalid_request");

        assert!(fake.calls().is_empty(), "nothing was written anywhere");
        assert_eq!(users::load(&pool, max).await.unwrap().memory_mib, 4096);
    }

    fn written(fake: &FakeHelper) -> Vec<craftpanel_proto::ResourceLimits> {
        fake.calls()
            .into_iter()
            .filter_map(|call| match call {
                craftpanel_proto::HelperRequest::ApplyLimits { limits, .. } => Some(limits),
                _ => None,
            })
            .collect()
    }

    fn last_written(fake: &FakeHelper) -> craftpanel_proto::ResourceLimits {
        written(fake).pop().expect("the cgroup was written")
    }

    #[tokio::test]
    async fn twelve_eight_refuses_an_administrator() {
        let pool = test_pool().await;
        let fake = FakeHelper::obliging().await;
        let boss = an_admin(&pool, "boss").await;
        let secret = sign_in(&pool, boss).await;

        let refused = app(&pool, &fake)
            .oneshot(as_user(
                send(
                    "PUT",
                    &format!("/admin/users/{boss}/limits"),
                    serde_json::json!({
                        "memory_mib": 6144, "cpu_mode": "cap", "cpu_cores": 3.0,
                        "pids_max": 1024, "disk_mib": 20480
                    }),
                ),
                &secret,
            ))
            .await
            .unwrap();

        assert_eq!(refused.status(), StatusCode::CONFLICT);
        assert_eq!(body_json(refused).await["error"], "role_unlimited");
        assert!(written(&fake).is_empty(), "the kernel was never asked");

        let row = users::load(&pool, boss).await.unwrap();
        assert_eq!(row.memory_mib, 4096, "the row a demotion would use is untouched");
        assert_eq!(row.disk_mib, 51200);

        let read = body_json(
            app(&pool, &fake)
                .oneshot(as_user(fetch(&format!("/admin/users/{boss}/limits")), &secret))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(read["limits"], serde_json::Value::Null, "there is no form behind this");
        assert_eq!(read["usage"]["memory"]["limit_mib"], serde_json::Value::Null);
    }

    #[tokio::test]
    async fn an_administrator_is_created_without_limits_or_not_at_all() {
        let pool = test_pool().await;
        let fake = FakeHelper::obliging().await;
        let secret = an_admin_session(&pool).await;

        let mut with_limits = new_user("deputy");
        with_limits["panel_role"] = serde_json::json!("admin");
        with_limits["limits"] = serde_json::json!({
            "memory_mib": 8192, "cpu_mode": "cap", "cpu_cores": 4.0,
            "pids_max": 512, "disk_mib": 51200
        });
        let refused = app(&pool, &fake)
            .oneshot(as_user(send("POST", "/admin/users", with_limits), &secret))
            .await
            .unwrap();
        assert_eq!(refused.status(), StatusCode::BAD_REQUEST);
        assert_eq!(body_json(refused).await["error"], "invalid_request");

        let mut plain = new_user("deputy");
        plain["panel_role"] = serde_json::json!("admin");
        let made = app(&pool, &fake)
            .oneshot(as_user(send("POST", "/admin/users", plain), &secret))
            .await
            .unwrap();
        assert_eq!(made.status(), StatusCode::CREATED);
        let body = body_json(made).await;
        assert_eq!(body["panel_role"], "admin");
        assert_eq!(body["limits"], serde_json::Value::Null);

        let id: Id = body["id"].as_str().unwrap().parse().unwrap();
        let row = users::load(&pool, id).await.unwrap();
        assert_eq!(row.memory_mib, 4096, "the panel default waits in the row");
        assert_eq!(row.disk_mib, 51200);
    }

    #[tokio::test]
    async fn promoting_and_demoting_moves_the_ceilings_both_ways() {
        let pool = test_pool().await;
        let fake = FakeHelper::obliging().await;
        let boss = an_admin(&pool, "boss").await;
        let secret = sign_in(&pool, boss).await;
        let max = a_user(&pool, "max").await;

        let role = |role: &str| {
            send(
                "PATCH",
                &format!("/admin/users/{max}"),
                serde_json::json!({ "panel_role": role }),
            )
        };

        let up = app(&pool, &fake).oneshot(as_user(role("admin"), &secret)).await.unwrap();
        assert_eq!(up.status(), StatusCode::OK);

        let promoted = last_written(&fake);
        assert_eq!(promoted.memory_high_bytes, None);
        assert_eq!(promoted.memory_max_bytes, None);
        assert_eq!(promoted.cpu_quota_percent, None);
        assert_eq!(promoted.pids_max, None, "all four files carry max");
        assert_eq!(users::load(&pool, max).await.unwrap().memory_mib, 4096, "the row is untouched");

        let down = app(&pool, &fake).oneshot(as_user(role("user"), &secret)).await.unwrap();
        assert_eq!(down.status(), StatusCode::OK);

        let demoted = last_written(&fake);
        assert_eq!(demoted.memory_high_bytes, Some(4096 * 1024 * 1024));
        assert_eq!(demoted.memory_max_bytes, Some(5120 * 1024 * 1024));
        assert_eq!(demoted.cpu_quota_percent, Some(200));
        assert_eq!(demoted.pids_max, Some(512));

        let so_far = written(&fake).len();
        for body in [serde_json::json!({ "username": "moritz" }), serde_json::json!({ "panel_role": "user" })] {
            app(&pool, &fake)
                .oneshot(as_user(send("PATCH", &format!("/admin/users/{max}"), body), &secret))
                .await
                .unwrap();
        }
        assert_eq!(written(&fake).len(), so_far, "only a role that actually changed writes");
    }

    #[tokio::test]
    async fn a_role_change_on_an_unfinished_account_leaves_the_helpers_reason_standing() {
        let pool = test_pool().await;
        let refusing = FakeHelper::refusing().await;
        let secret = an_admin_session(&pool).await;

        let andre = body_json(
            app(&pool, &refusing)
                .oneshot(as_user(send("POST", "/admin/users", new_user("andre")), &secret))
                .await
                .unwrap(),
        )
        .await;
        let andre: Id = andre["id"].as_str().unwrap().parse().unwrap();
        let complaint = users::load(&pool, andre).await.unwrap().system_error_message;
        assert!(complaint.as_deref().unwrap().contains("UID range"), "{complaint:?}");

        let promoted = body_json(
            app(&pool, &refusing)
                .oneshot(as_user(
                    send(
                        "PATCH",
                        &format!("/admin/users/{andre}"),
                        serde_json::json!({ "panel_role": "admin" }),
                    ),
                    &secret,
                ))
                .await
                .unwrap(),
        )
        .await;

        assert_eq!(promoted["panel_role"], "admin");
        assert_eq!(promoted["system_user"]["state"], "error");
        assert_eq!(
            users::load(&pool, andre).await.unwrap().system_error_message,
            complaint,
            "the helper's reason is what 12.9 puts in front of the administrator"
        );
        assert!(written(&refusing).is_empty(), "no cgroup was made for a user who has none");
    }

    #[tokio::test]
    async fn an_administrator_is_never_over_a_limit_and_an_ordinary_user_is() {
        let pool = test_pool().await;
        let fake = FakeHelper::obliging().await;
        let boss = an_admin(&pool, "boss").await;
        let secret = sign_in(&pool, boss).await;
        let max = a_user(&pool, "max").await;
        a_server(&pool, boss, "his", 8192).await;
        a_server(&pool, max, "hers", 8192).await;

        let body = body_json(
            app(&pool, &fake)
                .oneshot(as_user(fetch(&format!("/admin/users/{boss}")), &secret))
                .await
                .unwrap(),
        )
        .await;

        let nothing = serde_json::Value::Null;
        assert_eq!(body["usage"]["memory"]["allocated_mib"], 8192);
        assert_eq!(body["usage"]["over_limit"], false, "twice the row, and still not over");
        assert!(body["usage"]["over_limit_dimensions"].as_array().unwrap().is_empty());
        assert_eq!(body["limits"], nothing, "no limits, so no numbers to report");
        assert_eq!(body["usage"]["memory"]["limit_mib"], nothing);
        assert_eq!(body["usage"]["cpu"]["limit_cores"], nothing);
        assert_eq!(body["usage"]["pids"]["limit"], nothing);
        assert_eq!(body["usage"]["disk"]["limit_mib"], nothing);

        let his = body_json(
            crate::api::session::router()
                .with_state(state(&pool))
                .oneshot(as_user(fetch("/me"), &secret))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(his["capabilities"]["can_create_servers"], true);
        assert_eq!(his["capabilities"]["can_start_servers"], true);
        assert_eq!(his["capabilities"]["blocked_reason"], serde_json::Value::Null);

        let hers = body_json(
            app(&pool, &fake)
                .oneshot(as_user(fetch(&format!("/admin/users/{max}")), &secret))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(hers["usage"]["over_limit"], true, "the same servers, an ordinary account");
        assert_eq!(hers["usage"]["over_limit_dimensions"][0], "memory");
    }

    #[tokio::test]
    async fn the_machine_does_not_count_what_it_never_promised() {
        let pool = test_pool().await;
        let fake = FakeHelper::obliging().await;
        let secret = an_admin_session(&pool).await;
        an_admin(&pool, "deputy").await;
        a_user(&pool, "max").await;
        a_user(&pool, "anna").await;

        let body = body_json(
            app(&pool, &fake).oneshot(as_user(fetch("/admin/host"), &secret)).await.unwrap(),
        )
        .await;

        assert_eq!(body["user_count"], 4);
        assert_eq!(body["unlimited_users"], 2, "the two administrators");
        assert_eq!(body["allocated"]["memory_mib"], 8192, "only the two who were promised");
        assert_eq!(body["allocated"]["cpu_cores"], 4.0);
    }

    #[tokio::test]
    async fn retrying_the_system_account_finishes_what_the_helper_missed() {
        let pool = test_pool().await;
        let refusing = FakeHelper::refusing().await;
        let secret = an_admin_session(&pool).await;

        let created = body_json(
            app(&pool, &refusing)
                .oneshot(as_user(send("POST", "/admin/users", new_user("andre")), &secret))
                .await
                .unwrap(),
        )
        .await;
        let andre: Id = created["id"].as_str().unwrap().parse().unwrap();

        let again = app(&pool, &refusing)
            .oneshot(as_user(
                empty("POST", &format!("/admin/users/{andre}/system-user/retry")),
                &secret,
            ))
            .await
            .unwrap();
        assert_eq!(again.status(), StatusCode::CONFLICT);
        assert_eq!(body_json(again).await["error"], "system_user_not_ready");

        let obliging = FakeHelper::obliging().await;
        let body = body_json(
            app(&pool, &obliging)
                .oneshot(as_user(
                    empty("POST", &format!("/admin/users/{andre}/system-user/retry")),
                    &secret,
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(body["system_user"]["state"], "ready");
        assert_eq!(body["system_user"]["error_message"], serde_json::Value::Null);
    }

    #[tokio::test]
    async fn the_machine_reports_what_it_has_and_what_was_promised() {
        let pool = test_pool().await;
        let fake = FakeHelper::obliging().await;
        let secret = an_admin_session(&pool).await;
        a_user(&pool, "max").await;

        let body = body_json(
            app(&pool, &fake).oneshot(as_user(fetch("/admin/host"), &secret)).await.unwrap(),
        )
        .await;

        assert!(body["cpu_cores"].as_u64().unwrap() >= 1);
        assert!(body["memory_total_bytes"].as_u64().unwrap() > 0);
        assert_eq!(body["user_count"], 2);
        assert_eq!(body["allocated"]["memory_mib"], 4096, "one account at 4096, one admin");
        assert_eq!(body["allocated"]["cpu_cores"], 2.0);
        assert_eq!(body["unlimited_users"], 1);
        assert_eq!(body["default_limits"]["memory_mib"], 4096);
        assert!(
            body["assignable_memory_mib"].as_u64().unwrap()
                < body["memory_total_bytes"].as_u64().unwrap() / (1024 * 1024)
        );
    }

    #[tokio::test]
    async fn the_settings_are_read_and_written_whole() {
        let pool = test_pool().await;
        let fake = FakeHelper::obliging().await;
        let secret = an_admin_session(&pool).await;

        let before = body_json(
            app(&pool, &fake).oneshot(as_user(fetch("/admin/settings"), &secret)).await.unwrap(),
        )
        .await;
        assert_eq!(before["port_pool"]["from"], 25565);
        assert_eq!(before["external_services_enabled"], true);

        let mut wanted = before.clone();
        wanted["port_pool"] = serde_json::json!({ "from": 30000, "to": 30100 });
        wanted["external_services_enabled"] = serde_json::json!(false);
        wanted["public_address"] = serde_json::json!("minecraft.example");

        let response = app(&pool, &fake)
            .oneshot(as_user(send("PUT", "/admin/settings", wanted.clone()), &secret))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(body_json(response).await, wanted);

        let stored = settings::load(&pool).await.unwrap();
        assert_eq!(stored.port_pool.from, 30000);
        assert_eq!(stored.public_address.as_deref(), Some("minecraft.example"));
        assert!(!stored.external_services_enabled);
    }

    #[tokio::test]
    async fn settings_that_do_not_hold_together_are_refused_whole() {
        let pool = test_pool().await;
        let fake = FakeHelper::obliging().await;
        let secret = an_admin_session(&pool).await;

        let before = settings::load(&pool).await.unwrap();
        let mut wanted = serde_json::to_value(&before).unwrap();
        wanted["port_pool"] = serde_json::json!({ "from": 30000, "to": 20000 });

        let response = app(&pool, &fake)
            .oneshot(as_user(send("PUT", "/admin/settings", wanted), &secret))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(body_json(response).await["error"], "invalid_request");
        assert_eq!(settings::load(&pool).await.unwrap(), before);
    }

    #[tokio::test]
    async fn a_default_limit_of_the_panel_becomes_the_limit_of_the_next_account() {
        let pool = test_pool().await;
        let fake = FakeHelper::obliging().await;
        let secret = an_admin_session(&pool).await;

        let mut wanted = serde_json::to_value(settings::load(&pool).await.unwrap()).unwrap();
        wanted["default_limits"] = serde_json::json!({
            "memory_mib": 16384, "cpu_mode": "share", "cpu_cores": 8.0,
            "pids_max": 2048, "disk_mib": 102400
        });
        app(&pool, &fake)
            .oneshot(as_user(send("PUT", "/admin/settings", wanted), &secret))
            .await
            .unwrap();

        let created = body_json(
            app(&pool, &fake)
                .oneshot(as_user(send("POST", "/admin/users", new_user("andre")), &secret))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(created["limits"]["memory_mib"], 16384);
        assert_eq!(created["limits"]["cpu_mode"], "share");

        let andre: Id = created["id"].as_str().unwrap().parse().unwrap();
        assert_eq!(users::load(&pool, andre).await.unwrap().cpu_mode, CpuMode::Share);
    }

    #[tokio::test]
    async fn the_four_answers_of_section_12_carry_exactly_the_fields_of_14() {
        let pool = test_pool().await;
        let fake = FakeHelper::obliging().await;
        let boss = an_admin(&pool, "boss").await;
        let secret = sign_in(&pool, boss).await;

        let shapes: Vec<(String, Vec<&str>)> = vec![
            (
                "/admin/host".to_owned(),
                vec![
                    "allocated", "assignable_disk_mib", "assignable_memory_mib", "cpu_cores",
                    "default_limits", "disk_total_bytes", "measured_at", "memory_total_bytes",
                    "reserved_memory_mib", "unlimited_users", "used", "user_count",
                ],
            ),
            (
                "/admin/settings".to_owned(),
                vec![
                    "default_limits", "external_services_enabled", "max_backups_per_server",
                    "max_concurrent_operations", "max_upload_bytes", "port_pool",
                    "public_address", "registration_enabled", "registration_requires_approval",
                    "stop_grace_seconds",
                ],
            ),
            (
                format!("/admin/users/{boss}"),
                vec![
                    "active_sessions", "avatar_url", "created_at", "email", "id", "last_login_at",
                    "limits", "must_change_password", "origin", "owned_servers", "panel_role",
                    "system_user", "usage", "username",
                ],
            ),
            (format!("/admin/users/{boss}/limits"), vec!["host", "limits", "usage"]),
            ("/admin/users".to_owned(), vec!["total", "users"]),
        ];

        for (uri, expected) in shapes {
            let body =
                body_json(app(&pool, &fake).oneshot(as_user(fetch(&uri), &secret)).await.unwrap())
                    .await;
            let mut found: Vec<&str> =
                body.as_object().expect(&uri).keys().map(String::as_str).collect();
            found.sort_unstable();
            assert_eq!(found, expected, "{uri}");
        }
    }
}
