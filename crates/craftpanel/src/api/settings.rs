use std::sync::{Arc, OnceLock};

use axum::extract::{FromRequestParts, State};
use axum::http::request::Parts;
use axum::http::StatusCode;
use axum::routing::{delete, get, post, put};
use axum::{Extension, Json, Router};
use serde::Deserialize;

use crate::audit::{self, Event};
use crate::auth::access::Access;
use crate::auth::error::{Failure, Result};
use crate::auth::{access, extract, users, Caller, JsonBody, LiveServers, Params};
use crate::helper::Helper;
use crate::model::{Allocation, Id, LoaderId, Permission};
use crate::ops::Operations;
use crate::settings::allocations::{
    self, CreateAllocationRequest, RenameAllocationRequest, SetPrimaryResponse,
};
use crate::settings::catalog::{
    Catalog, GameVersionList, LoaderBuildList, LoaderList, LOADERS,
};
use crate::settings::install::{
    self, ContentPolicy, InstallAccepted, InstallRequest, Job, Plan, ResetRequest,
    ResetToSetupResponse, Runner,
};
use crate::settings::runtimes::{self, JavaRuntime, JavaRuntimeList};
use crate::settings::startup::{self, StartupOptions, StartupOptionsPatch};
use crate::settings::store::{self, ServerProperties, ServerPropertiesPatch};
use crate::settings::{load_server, ServerRow};
use crate::AppState;

pub fn router(operations: Arc<Operations>, live: LiveServers) -> Router<AppState> {
    Router::new()
        .route("/servers/{server}/properties", get(read_properties).patch(write_properties))
        .route("/servers/{server}/startup", get(read_startup).patch(write_startup))
        .route("/java-runtimes", get(java_runtimes))
        .route("/servers/{server}/allocations", get(list_allocations).post(add_allocation))
        .route(
            "/servers/{server}/allocations/{port}",
            delete(drop_allocation).patch(rename_allocation),
        )
        .route("/servers/{server}/allocations/{port}/primary", put(make_primary))
        .route("/loaders", get(loaders))
        .route("/loaders/{loader}/game-versions", get(game_versions))
        .route("/loaders/{loader}/game-versions/{game_version}/builds", get(builds))
        .route("/servers/{server}/install", post(install))
        .route("/servers/{server}/repair", post(repair))
        .route("/servers/{server}/reset", post(reset))
        .route("/servers/{server}/reset-to-setup", post(reset_to_setup))
        .layer(Extension(operations))
        .layer(Extension(live))
        .layer(axum::middleware::from_fn(extract::same_origin))
}

#[derive(Debug, Clone, Copy)]
struct Path<T>(pub T);

impl<T> FromRequestParts<AppState> for Path<T>
where
    T: serde::de::DeserializeOwned + Send,
{
    type Rejection = Failure;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self> {
        match axum::extract::Path::<T>::from_request_parts(parts, state).await {
            Ok(axum::extract::Path(value)) => Ok(Self(value)),
            Err(rejection) => Err(Failure::invalid_request(rejection.body_text())),
        }
    }
}

fn catalog() -> Result<&'static Catalog> {
    static SHARED: OnceLock<std::result::Result<Catalog, String>> = OnceLock::new();
    SHARED
        .get_or_init(|| Catalog::new().map_err(|err| err.to_string()))
        .as_ref()
        .map_err(|reason| {
            Failure::new(StatusCode::BAD_GATEWAY, "upstream_unavailable", reason.clone())
        })
}

async fn read_properties(
    State(state): State<AppState>,
    Extension(live): Extension<LiveServers>,
    caller: Caller,
    Path(server): Path<Id>,
) -> Result<Json<ServerProperties>> {
    let (_, row) = seen(&state, &caller, server, Permission::BaseRead).await?;
    let running = is_running(&live, server).await;
    let pending = store::has_pending(&state.pool, server).await?;

    let properties = store::read(&row.directory(&state.config.data_dir))?;
    Ok(Json(store::view(&properties, running && pending)))
}

async fn write_properties(
    State(state): State<AppState>,
    Extension(operations): Extension<Arc<Operations>>,
    Extension(live): Extension<LiveServers>,
    caller: Caller,
    Path(server): Path<Id>,
    JsonBody(patch): JsonBody<ServerPropertiesPatch>,
) -> Result<Json<ServerProperties>> {
    let (access, row) = seen(&state, &caller, server, Permission::Advanced).await?;
    operations.guard_write(server).await.map_err(crate::settings::from_fault)?;

    if !row.supports_properties() {
        return Err(Failure::conflict(
            "properties_unsupported",
            "a proxy reads no server.properties",
        ));
    }

    let wanted = store::plan(&patch)?;
    let directory = row.directory(&state.config.data_dir);
    let running = is_running(&live, server).await;

    if !running {
        store::replay(&state.pool, server, &directory).await?;
    }

    let mut properties = store::read(&directory)?;
    store::apply(&mut properties, &wanted);
    store::write(&directory, &properties)?;
    crate::settings::give_back(&helper(&state), row.owner_id, row.id).await?;

    if running {
        store::queue(&state.pool, server, &wanted).await?;
    }
    if !wanted.is_empty() {
        let changed = wanted
            .iter()
            .map(|(key, value)| (key.wire.clone(), value.clone()))
            .collect();
        audit::record(&state.pool, access, &caller, Event::ServerPropertiesModified {
            properties: changed,
        })
        .await;
    }

    let restart = running && store::has_pending(&state.pool, server).await?;
    Ok(Json(store::view(&properties, restart)))
}

async fn read_startup(
    State(state): State<AppState>,
    Extension(live): Extension<LiveServers>,
    caller: Caller,
    Path(server): Path<Id>,
) -> Result<Json<StartupOptions>> {
    let (_, row) = seen(&state, &caller, server, Permission::BaseRead).await?;
    let ceiling = memory_ceiling(&state, &caller, &row).await?;
    let running = is_running(&live, server).await;
    let restart = settle_restart_flag(&state, &row, running).await?;

    let here = runtimes::cached(&state.config.data_dir, &state.config.java_search);
    Ok(Json(startup::view(&row, &here, ceiling, restart)))
}

async fn write_startup(
    State(state): State<AppState>,
    Extension(operations): Extension<Arc<Operations>>,
    Extension(live): Extension<LiveServers>,
    caller: Caller,
    Path(server): Path<Id>,
    JsonBody(patch): JsonBody<StartupOptionsPatch>,
) -> Result<Json<StartupOptions>> {
    let (access, row) = seen(&state, &caller, server, Permission::Advanced).await?;
    if patch.startup_command.is_some() && !caller.is_admin() {
        return Err(Failure::new(
            StatusCode::FORBIDDEN,
            "forbidden",
            "only a panel administrator may change the startup command",
        ));
    }
    operations.guard_write(server).await.map_err(crate::settings::from_fault)?;

    let ceiling = memory_ceiling(&state, &caller, &row).await?;
    let memory = match patch.memory_mib {
        Some(wanted) => check_budget(&state, &caller, &row, wanted).await?,
        None => row.memory_mib,
    };

    let installed = runtimes::cached(&state.config.data_dir, &state.config.java_search);
    check_runtime(&installed, &patch)?;
    let change = startup::plan(&patch, &row, memory)?;

    let moved = change.java_major != row.java_major
        || change.jre_vendor != row.jre_vendor
        || change.memory_mib != row.memory_mib
        || change.extra_flags != row.extra_flags;
    let running = is_running(&live, server).await;
    let restart = running && (moved || row.restart_required);

    sqlx::query(
        "UPDATE servers SET java_major = ?, jre_vendor = ?, memory_mib = ?, extra_flags = ?, \
         restart_required = ?, updated_at = ? WHERE id = ?",
    )
    .bind(change.java_major)
    .bind(change.jre_vendor)
    .bind(change.memory_mib)
    .bind(serde_json::to_string(&change.extra_flags).unwrap_or_else(|_| "[]".to_owned()))
    .bind(restart)
    .bind(crate::model::Timestamp::now())
    .bind(server)
    .execute(&state.pool)
    .await?;

    let after = ServerRow {
        java_major: change.java_major,
        jre_vendor: change.jre_vendor,
        memory_mib: change.memory_mib,
        extra_flags: change.extra_flags,
        restart_required: restart,
        ..row.clone()
    };
    let shown = startup::view(&after, &installed, ceiling, restart).dropped(change.stripped_flags);
    for event in startup_events(&row, &shown) {
        audit::record(&state.pool, access, &caller, event).await;
    }
    operations.bus().say(server, &startup_message(&shown));
    Ok(Json(shown))
}

fn startup_events(before: &ServerRow, after: &StartupOptions) -> Vec<Event> {
    let mut events = Vec::new();
    if before.memory_mib != after.memory_mib {
        events.push(Event::ServerReallocated);
    }
    if before.extra_flags != after.extra_flags {
        events.push(Event::StartupCommandModified { command: after.startup_command.clone() });
    }
    if let Some(version) = after.java_version.filter(|major| before.java_major != Some(*major)) {
        events.push(Event::JavaVersionModified { version });
    }
    if let Some(vendor) = after.jre_vendor.filter(|name| before.jre_vendor != Some(*name)) {
        events.push(Event::JavaRuntimeModified { vendor });
    }
    events
}

fn startup_message(options: &StartupOptions) -> crate::ops::WsMessage {
    crate::ops::WsMessage::StartupChanged(crate::ops::StartupReport {
        java_version: options.java_version,
        jre_vendor: options.jre_vendor,
        memory_mib: options.memory_mib,
        startup_command: options.startup_command.clone(),
        original_invocation: options.original_invocation.clone(),
        restart_required: options.restart_required,
    })
}

fn check_runtime(installed: &[JavaRuntime], patch: &StartupOptionsPatch) -> Result<()> {
    let major = patch.java_version.flatten();
    let vendor = patch.jre_vendor.flatten();

    if let Some(major) = major {
        if !installed.iter().any(|runtime| runtime.major == major) {
            return Err(Failure::bad_request(
                "invalid_java_version",
                format!("no Java {major} on this machine"),
            ));
        }
    }
    if let Some(vendor) = vendor {
        if !installed.iter().any(|runtime| runtime.vendor == vendor) {
            return Err(Failure::bad_request(
                "invalid_jre_vendor",
                format!("no {vendor} runtime on this machine"),
            ));
        }
    }
    if let (Some(major), Some(vendor)) = (major, vendor) {
        let pair = installed
            .iter()
            .any(|runtime| runtime.major == major && runtime.vendor == vendor);
        if !pair {
            return Err(Failure::not_found(
                "runtime_not_installed",
                format!("no {vendor} build of Java {major} here, and this panel fetches none"),
            ));
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, Default, Deserialize)]
#[serde(default)]
struct RuntimeQuery {
    server_id: Option<Id>,
}

async fn java_runtimes(
    State(state): State<AppState>,
    caller: Caller,
    Params(query): Params<RuntimeQuery>,
) -> Result<Json<JavaRuntimeList>> {
    let default_major = match query.server_id {
        Some(server) => {
            let (_, row) = seen(&state, &caller, server, Permission::BaseRead).await?;
            row.game_version.as_deref().and_then(runtimes::default_major)
        }
        None => None,
    };

    Ok(Json(JavaRuntimeList {
        runtimes: runtimes::cached(&state.config.data_dir, &state.config.java_search),
        default_major_for_game_version: default_major,
    }))
}

async fn list_allocations(
    State(state): State<AppState>,
    caller: Caller,
    Path(server): Path<Id>,
) -> Result<Json<Vec<Allocation>>> {
    seen(&state, &caller, server, Permission::BaseRead).await?;
    Ok(Json(allocations::list(&state.pool, server).await?))
}

async fn add_allocation(
    State(state): State<AppState>,
    Extension(operations): Extension<Arc<Operations>>,
    caller: Caller,
    Path(server): Path<Id>,
    JsonBody(request): JsonBody<CreateAllocationRequest>,
) -> Result<(StatusCode, Json<Allocation>)> {
    let (access, _) = seen(&state, &caller, server, Permission::Advanced).await?;
    operations.guard_write(server).await.map_err(crate::settings::from_fault)?;

    let settings = crate::auth::settings::load(&state.pool).await?;
    let made =
        allocations::create(&state.pool, server, settings.port_pool, &request, caller.is_admin())
            .await?;

    audit::record(&state.pool, access, &caller, Event::PortAllocationAdded { port: made.port })
        .await;
    announce(&state, &operations, server).await;
    Ok((StatusCode::CREATED, Json(made)))
}

async fn rename_allocation(
    State(state): State<AppState>,
    Extension(operations): Extension<Arc<Operations>>,
    caller: Caller,
    Path((server, port)): Path<(Id, u16)>,
    JsonBody(request): JsonBody<RenameAllocationRequest>,
) -> Result<Json<Allocation>> {
    seen(&state, &caller, server, Permission::Advanced).await?;
    operations.guard_write(server).await.map_err(crate::settings::from_fault)?;
    let renamed = allocations::rename(&state.pool, server, port, &request.name).await?;

    announce(&state, &operations, server).await;
    Ok(Json(renamed))
}

async fn drop_allocation(
    State(state): State<AppState>,
    Extension(operations): Extension<Arc<Operations>>,
    caller: Caller,
    Path((server, port)): Path<(Id, u16)>,
) -> Result<StatusCode> {
    let (access, _) = seen(&state, &caller, server, Permission::Advanced).await?;
    operations.guard_write(server).await.map_err(crate::settings::from_fault)?;
    allocations::remove(&state.pool, server, port).await?;

    audit::record(&state.pool, access, &caller, Event::PortAllocationRemoved { port }).await;
    announce(&state, &operations, server).await;
    Ok(StatusCode::NO_CONTENT)
}

async fn make_primary(
    State(state): State<AppState>,
    Extension(operations): Extension<Arc<Operations>>,
    Extension(live): Extension<LiveServers>,
    caller: Caller,
    Path((server, port)): Path<(Id, u16)>,
) -> Result<Json<SetPrimaryResponse>> {
    let (access, row) = seen(&state, &caller, server, Permission::Advanced).await?;
    operations.guard_write(server).await.map_err(crate::settings::from_fault)?;
    allocations::set_primary(&state.pool, server, port).await?;

    if row.supports_properties() {
        let directory = row.directory(&state.config.data_dir);
        let mut properties = store::read(&directory)?;
        store::set_ports(&mut properties, port);
        store::write(&directory, &properties)?;
        crate::settings::give_back(&helper(&state), row.owner_id, row.id).await?;

        let written = store::port_overrides(port)
            .into_iter()
            .map(|(key, value)| (key.wire, value))
            .collect();
        audit::record(&state.pool, access, &caller, Event::ServerPropertiesModified {
            properties: written,
        })
        .await;

        if is_running(&live, server).await {
            store::queue(&state.pool, server, &store::port_overrides(port)).await?;
        }
    }

    let rest = allocations::list(&state.pool, server).await?;
    operations.bus().say(
        server,
        &crate::ops::WsMessage::NetworkChanged(crate::ops::NetworkReport {
            primary_port: port,
            allocations: rest.clone(),
        }),
    );

    Ok(Json(SetPrimaryResponse { primary_port: port, allocations: rest, restart_required: true }))
}

async fn announce(state: &AppState, operations: &Arc<Operations>, server: Id) {
    let Ok(rest) = allocations::list(&state.pool, server).await else { return };
    let primary = allocations::primary(&state.pool, server).await.ok().flatten();
    operations.bus().say(
        server,
        &crate::ops::WsMessage::NetworkChanged(crate::ops::NetworkReport {
            primary_port: primary.unwrap_or_default(),
            allocations: rest,
        }),
    );
}

async fn loaders(_caller: Caller) -> Json<LoaderList> {
    Json(LoaderList { loaders: LOADERS })
}

async fn game_versions(
    _caller: Caller,
    Path(loader): Path<String>,
) -> Result<Json<GameVersionList>> {
    Ok(Json(catalog()?.game_versions(read_loader(&loader)?).await?))
}

async fn builds(
    State(state): State<AppState>,
    _caller: Caller,
    Path((loader, game_version)): Path<(String, String)>,
) -> Result<Json<LoaderBuildList>> {
    let loader = read_loader(&loader)?;

    let installed: Vec<String> = sqlx::query_scalar(
        "SELECT DISTINCT loader_version FROM servers \
         WHERE loader = ? AND game_version = ? AND loader_version IS NOT NULL",
    )
    .bind(loader)
    .bind(&game_version)
    .fetch_all(&state.pool)
    .await?;

    Ok(Json(catalog()?.builds(loader, &game_version, &installed).await?))
}

fn read_loader(name: &str) -> Result<LoaderId> {
    name.parse()
        .map_err(|_| Failure::not_found("loader_not_found", format!("no loader called {name}")))
}

async fn install(
    State(state): State<AppState>,
    Extension(operations): Extension<Arc<Operations>>,
    Extension(live): Extension<LiveServers>,
    caller: Caller,
    Path(server): Path<Id>,
    JsonBody(request): JsonBody<InstallRequest>,
) -> Result<(StatusCode, Json<InstallAccepted>)> {
    let (access, row) = seen(&state, &caller, server, Permission::Setup).await?;
    let loader = install::read_loader(&request.loader)?;
    refuse_while_running(&live, server).await?;

    let warnings = install::check_change(row.loader, loader, request.content_policy)?;
    let build = resolve(loader, &request.game_version, request.loader_version.as_deref()).await?;

    let plan = Plan {
        loader,
        game_version: request.game_version,
        build,
        policy: request.content_policy,
    };
    let game_version = plan.game_version.clone();
    let build = plan.build.clone();
    let started = runner(&state, &operations)?
        .start(&row, caller.id(), Job::Install { plan, wipe_everything: false })
        .await?;

    audit::record(&state.pool, access, &caller, Event::LoaderVersionEdited {
        new_loader: loader,
        new_version: build,
    })
    .await;
    if row.game_version.as_deref() != Some(game_version.as_str()) {
        audit::record(&state.pool, access, &caller, Event::GameVersionEdited {
            new_version: game_version,
        })
        .await;
    }

    Ok((StatusCode::ACCEPTED, Json(InstallAccepted { operation: started, warnings })))
}

async fn repair(
    State(state): State<AppState>,
    Extension(operations): Extension<Arc<Operations>>,
    Extension(live): Extension<LiveServers>,
    caller: Caller,
    Path(server): Path<Id>,
) -> Result<(StatusCode, Json<crate::model::OperationAccepted>)> {
    let (access, row) = seen(&state, &caller, server, Permission::Setup).await?;
    refuse_while_running(&live, server).await?;

    let started = runner(&state, &operations)?.start(&row, caller.id(), Job::Repair).await?;
    audit::record(&state.pool, access, &caller, Event::ServerRepaired).await;
    Ok((
        StatusCode::ACCEPTED,
        Json(crate::model::OperationAccepted { operation: started }),
    ))
}

async fn reset(
    State(state): State<AppState>,
    Extension(operations): Extension<Arc<Operations>>,
    Extension(live): Extension<LiveServers>,
    caller: Caller,
    Path(server): Path<Id>,
    JsonBody(request): JsonBody<ResetRequest>,
) -> Result<(StatusCode, Json<crate::model::OperationAccepted>)> {
    let (access, row) = seen(&state, &caller, server, Permission::ResetServer).await?;
    let loader = install::read_loader(&request.loader)?;
    if !request.keep_backups {
        return Err(Failure::invalid_request(
            "keep_backups is fixed true; the page promises backups survive a reset",
        ));
    }
    refuse_while_running(&live, server).await?;

    let build = resolve(loader, &request.game_version, request.loader_version.as_deref()).await?;
    let plan = Plan {
        loader,
        game_version: request.game_version,
        build,
        policy: ContentPolicy::WipeMods,
    };
    let started = runner(&state, &operations)?
        .start(&row, caller.id(), Job::Install { plan, wipe_everything: true })
        .await?;

    audit::record(&state.pool, access, &caller, Event::ServerReset).await;
    Ok((
        StatusCode::ACCEPTED,
        Json(crate::model::OperationAccepted { operation: started }),
    ))
}

async fn reset_to_setup(
    State(state): State<AppState>,
    Extension(operations): Extension<Arc<Operations>>,
    Extension(live): Extension<LiveServers>,
    caller: Caller,
    Path(server): Path<Id>,
) -> Result<Json<ResetToSetupResponse>> {
    seen(&state, &caller, server, Permission::ResetServer).await?;
    if !caller.is_admin() {
        return Err(Failure::forbidden());
    }
    refuse_while_running(&live, server).await?;

    install::reset_to_setup(&state.pool, server).await?;
    operations.bus().channel(server).clear_console();

    Ok(Json(ResetToSetupResponse {
        server_id: server,
        flows: crate::settings::install::Flows { intro: true },
    }))
}

async fn resolve(
    loader: LoaderId,
    game_version: &str,
    build: Option<&str>,
) -> Result<Option<String>> {
    let catalog = catalog()?;
    let known = catalog.game_versions(loader).await?;
    if !known.game_versions.iter().any(|entry| entry.version == game_version) {
        return Err(Failure::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "unsupported_game_version",
            format!("{loader} has no {game_version}"),
        ));
    }

    if loader == LoaderId::Vanilla {
        return Ok(None);
    }
    let Some(build) = build else {
        if !catalog.has_stable_build(loader, game_version).await? {
            return Err(Failure::not_found(
                "build_not_found",
                format!("{loader} has no stable build for {game_version} yet; name one"),
            ));
        }
        return Ok(None);
    };

    if !catalog.knows_build(loader, game_version, build).await? {
        return Err(Failure::not_found(
            "build_not_found",
            format!("{loader} has no build {build} for {game_version}"),
        ));
    }
    Ok(Some(build.to_owned()))
}

async fn seen(
    state: &AppState,
    caller: &Caller,
    server: Id,
    permission: Permission,
) -> Result<(Access, ServerRow)> {
    let access = access::require(&state.pool, caller, server, permission).await?;
    Ok((access, load_server(&state.pool, server).await?))
}

async fn is_running(live: &LiveServers, server: Id) -> bool {
    live.among(&[server]).await.contains(&server)
}

async fn refuse_while_running(live: &LiveServers, server: Id) -> Result<()> {
    if is_running(live, server).await {
        return Err(Failure::conflict("server_running", "stop the server first"));
    }
    Ok(())
}

fn helper(state: &AppState) -> Helper {
    Helper::new(&state.config.helper_socket)
}

fn runner(state: &AppState, operations: &Arc<Operations>) -> Result<Arc<Runner>> {
    Ok(Runner::new(
        state.pool.clone(),
        Arc::clone(operations),
        catalog()?,
        helper(state),
        state.config.data_dir.clone(),
        state.config.cache_dir(),
    ))
}

async fn memory_ceiling(state: &AppState, caller: &Caller, row: &ServerRow) -> Result<u32> {
    let owner = users::load(&state.pool, row.owner_id).await?;
    let machine = crate::auth::usage::Host::measure().assignable_memory_mib();
    let Some(limit) = owner.budget().memory_mib() else { return Ok(machine) };
    if caller.is_admin() {
        return Ok(machine);
    }
    let others = spoken_for(state, row).await?;
    Ok(limit.saturating_sub(others))
}

async fn spoken_for(state: &AppState, row: &ServerRow) -> Result<u32> {
    let allocated = users::owned_servers(&state.pool, row.owner_id)
        .await?
        .iter()
        .fold(0u32, |sum, server| sum.saturating_add(server.memory_mib));
    Ok(allocated.saturating_sub(row.memory_mib))
}

async fn check_budget(
    state: &AppState,
    caller: &Caller,
    row: &ServerRow,
    wanted: u32,
) -> Result<u32> {
    if wanted < startup::MIN_MEMORY_MIB {
        return Err(Failure::bad_request(
            "memory_too_small",
            format!("a server needs at least {} MiB", startup::MIN_MEMORY_MIB),
        ));
    }
    let owner = users::load(&state.pool, row.owner_id).await?;
    let Some(limit) = owner.budget().memory_mib() else { return Ok(wanted) };
    if caller.is_admin() {
        return Ok(wanted);
    }

    let others = spoken_for(state, row).await?;

    if others > limit {
        return Err(Failure::conflict(
            "over_limit",
            "this account is already over its memory limit",
        ));
    }
    if others.saturating_add(wanted) > limit {
        return Err(Failure::conflict(
            "budget_exceeded",
            format!("{limit} MiB is the whole budget, and {others} MiB of it is spoken for"),
        ));
    }
    Ok(wanted)
}

async fn settle_restart_flag(state: &AppState, row: &ServerRow, running: bool) -> Result<bool> {
    if row.restart_required && !running {
        sqlx::query("UPDATE servers SET restart_required = 0 WHERE id = ?")
            .bind(row.id)
            .execute(&state.pool)
            .await?;
        return Ok(false);
    }
    Ok(row.restart_required && running)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::harness::{
        a_server, a_user, an_admin, body_json, empty, fetch, send, sign_in, state_with, test_pool,
        FakeHelper,
    };
    use crate::config::Config;
    use crate::model::Timestamp;
    use crate::settings::harness::{an_allocation, set_loader};
    use crate::settings::install::Flows;
    use sqlx::SqlitePool;
    use tower::ServiceExt;

    struct World {
        app: Router,
        pool: SqlitePool,
        dir: crate::settings::harness::Scratch,
        server: Id,
        owner: Id,
        _helper: FakeHelper,
    }

    async fn world() -> World {
        build(false).await
    }

    async fn a_running_world() -> World {
        build(true).await
    }

    async fn a_member(pool: &SqlitePool, server: Id, user: Id, role: &str) {
        sqlx::query(
            "INSERT INTO server_members (id, server_id, user_id, role, invited_at, joined_at) \
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(Id::new())
        .bind(server)
        .bind(user)
        .bind(role)
        .bind(Timestamp::now())
        .bind(Timestamp::now())
        .execute(pool)
        .await
        .expect("inserting a membership");
    }

    async fn build(running: bool) -> World {
        let pool = test_pool().await;
        let owner = a_user(&pool, "max").await;
        let server = a_server(&pool, owner, "Survival", 4096).await;
        set_loader(&pool, server, LoaderId::Paper, "1.21.8", Some("60")).await;

        let dir = crate::settings::harness::a_dir();
        let helper = FakeHelper::obliging().await;
        let config = Config {
            data_dir: dir.path().to_path_buf(),
            helper_socket: helper.socket(),
            ..Config::default()
        };
        let live =
            if running { LiveServers::fixed([server]) } else { LiveServers::none() };

        let operations = Operations::new(pool.clone(), dir.path());
        let app = router(operations, live).with_state(state_with(&pool, config));

        std::fs::create_dir_all(crate::settings::server_dir(dir.path(), owner, server)).unwrap();
        World { app, pool, dir, server, owner, _helper: helper }
    }

    impl World {
        fn server_dir(&self) -> std::path::PathBuf {
            crate::settings::server_dir(self.dir.path(), self.owner, self.server)
        }

        async fn as_owner(&self) -> String {
            sign_in(&self.pool, self.owner).await
        }

        async fn member(&self, name: &str, role: &str) -> String {
            let user = a_user(&self.pool, name).await;
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

        async fn call(
            &self,
            request: axum::http::Request<axum::body::Body>,
            secret: &str,
        ) -> axum::http::Response<axum::body::Body> {
            self.app
                .clone()
                .oneshot(crate::auth::harness::as_user(request, secret))
                .await
                .unwrap()
        }
    }

    #[tokio::test]
    async fn the_properties_page_reads_the_file_as_it_stands_and_writes_a_difference() {
        let world = world().await;
        let secret = world.as_owner().await;
        std::fs::write(
            world.server_dir().join("server.properties"),
            "#header\nmotd=Alt\nview-distance=10\nenable-command-block=false\n",
        )
        .unwrap();

        let read = world.call(fetch(&format!("/servers/{}/properties", world.server)), &secret).await;
        assert_eq!(read.status(), StatusCode::OK);
        let body = body_json(read).await;
        assert_eq!(body["known"]["motd"], "Alt");
        assert_eq!(body["known"]["view_distance"], "10");
        assert_eq!(body["custom"]["enable-command-block"], "false");
        assert_eq!(body["restart_required"], false);

        let written = world
            .call(
                send(
                    "PATCH",
                    &format!("/servers/{}/properties", world.server),
                    serde_json::json!({ "known": { "motd": "New", "view_distance": null } }),
                ),
                &secret,
            )
            .await;
        assert_eq!(written.status(), StatusCode::OK);

        let file = std::fs::read_to_string(world.server_dir().join("server.properties")).unwrap();
        assert!(file.starts_with("#header\n"), "the comment survives a write: {file}");
        assert!(file.contains("motd=New\n"));
        assert!(!file.contains("view-distance"));
        assert!(file.contains("enable-command-block=false\n"), "unnamed lines are left alone");
    }

    #[tokio::test]
    async fn writing_the_properties_hands_the_file_back_to_the_account_that_runs_the_game() {
        let world = world().await;
        let secret = world.as_owner().await;

        world
            .call(
                send(
                    "PATCH",
                    &format!("/servers/{}/properties", world.server),
                    serde_json::json!({ "known": { "motd": "New" } }),
                ),
                &secret,
            )
            .await;

        let chowned: Vec<Vec<String>> = world
            ._helper
            .calls()
            .into_iter()
            .filter_map(|call| match call {
                craftpanel_proto::HelperRequest::ChownTree { user_id, steps }
                    if user_id == world.owner.to_string() =>
                {
                    Some(steps)
                }
                _ => None,
            })
            .collect();

        assert_eq!(
            chowned,
            [crate::helper::in_servers(world.server)],
            "docs/PLAN.md:205 — without this the game gets Permission denied on its own config"
        );
    }

    #[tokio::test]
    async fn a_proxy_shows_no_properties_and_refuses_to_be_given_any() {
        let world = world().await;
        let secret = world.as_owner().await;
        set_loader(&world.pool, world.server, LoaderId::Velocity, "3.5.1", Some("1")).await;

        let read = world.call(fetch(&format!("/servers/{}/properties", world.server)), &secret).await;
        assert_eq!(read.status(), StatusCode::OK, "9.1: a missing file is still a 200");
        let body = body_json(read).await;
        assert_eq!(body["known"], serde_json::json!({}));
        assert_eq!(body["custom"], serde_json::json!({}));

        let refused = world
            .call(
                send(
                    "PATCH",
                    &format!("/servers/{}/properties", world.server),
                    serde_json::json!({ "known": { "motd": "x" } }),
                ),
                &secret,
            )
            .await;
        assert_eq!(refused.status(), StatusCode::CONFLICT);
        assert_eq!(body_json(refused).await["error"], "properties_unsupported");
    }

    #[tokio::test]
    async fn the_two_panel_owned_lines_can_only_be_moved_by_changing_the_port() {
        let world = world().await;
        let secret = world.as_owner().await;

        let refused = world
            .call(
                send(
                    "PATCH",
                    &format!("/servers/{}/properties", world.server),
                    serde_json::json!({ "custom": { "server-port": "25599" } }),
                ),
                &secret,
            )
            .await;
        assert_eq!(refused.status(), StatusCode::CONFLICT);
        assert_eq!(body_json(refused).await["error"], "property_is_panel_owned");

        an_allocation(&world.pool, world.server, 25565, "game", true).await;
        an_allocation(&world.pool, world.server, 25599, "second", false).await;

        let swapped = world
            .call(
                empty("PUT", &format!("/servers/{}/allocations/25599/primary", world.server)),
                &secret,
            )
            .await;
        assert_eq!(swapped.status(), StatusCode::OK);
        let body = body_json(swapped).await;
        assert_eq!(body["primary_port"], 25599);
        assert_eq!(body["restart_required"], true);
        assert_eq!(body["allocations"][0]["port"], 25565, "the old primary stays with the server");

        let file = std::fs::read_to_string(world.server_dir().join("server.properties")).unwrap();
        assert!(file.contains("server-port=25599\n"), "{file}");
        assert!(file.contains("query.port=25599\n"), "{file}");
    }

    #[tokio::test]
    async fn a_server_that_published_its_port_may_not_swap_it_out_from_under_the_tunnel() {
        let world = world().await;
        let secret = world.as_owner().await;

        an_allocation(&world.pool, world.server, 25565, "game", true).await;
        an_allocation(&world.pool, world.server, 25599, "second", false).await;
        crate::playit::store::claim_slot(&world.pool, world.owner, world.server, 25565, 4)
            .await
            .unwrap();

        let refused = world
            .call(
                empty("PUT", &format!("/servers/{}/allocations/25599/primary", world.server)),
                &secret,
            )
            .await;

        assert_eq!(refused.status(), StatusCode::CONFLICT);
        assert_eq!(body_json(refused).await["error"], "playit_tunnel_exists");
        assert_eq!(
            allocations::primary(&world.pool, world.server).await.unwrap(),
            Some(25565),
            "the port the tunnel points at stayed where it was"
        );
    }

    #[tokio::test]
    async fn a_change_made_while_the_server_runs_is_kept_for_after_the_stop() {
        let world = a_running_world().await;
        let secret = world.as_owner().await;
        std::fs::write(world.server_dir().join("server.properties"), "motd=Alt\n").unwrap();

        let answered = world
            .call(
                send(
                    "PATCH",
                    &format!("/servers/{}/properties", world.server),
                    serde_json::json!({ "known": { "motd": "New" } }),
                ),
                &secret,
            )
            .await;
        assert_eq!(answered.status(), StatusCode::OK);
        assert_eq!(body_json(answered).await["restart_required"], true);
        assert!(
            std::fs::read_to_string(world.server_dir().join("server.properties"))
                .unwrap()
                .contains("motd=New"),
            "9.2: the file is written at once, the page expects it"
        );

        std::fs::write(world.server_dir().join("server.properties"), "motd=Alt\n").unwrap();
        let replayed =
            store::replay(&world.pool, world.server, &world.server_dir()).await.unwrap();

        assert_eq!(replayed, 1);
        let file = std::fs::read_to_string(world.server_dir().join("server.properties")).unwrap();
        assert!(file.contains("motd=New\n"), "without the replay the edit is lost: {file}");
    }

    #[tokio::test]
    async fn the_startup_page_shows_the_command_and_keeps_xmx_out_of_reach() {
        let world = world().await;
        let secret = world.as_owner().await;
        let boss = an_admin(&world.pool, "boss").await;
        let admin_secret = sign_in(&world.pool, boss).await;

        let shown = world.call(fetch(&format!("/servers/{}/startup", world.server)), &secret).await;
        assert_eq!(shown.status(), StatusCode::OK);
        let body = body_json(shown).await;
        assert_eq!(body["memory_mib"], 4096);
        assert_eq!(body["managed_flags"][0], "-Xmx4096M");
        assert_eq!(body["stripped_flags"], serde_json::json!([]));

        let written = world
            .call(
                send(
                    "PATCH",
                    &format!("/servers/{}/startup", world.server),
                    serde_json::json!({
                        "startup_command": "java -Xmx32768M -XX:+UseG1GC -jar server.jar nogui"
                    }),
                ),
                &admin_secret,
            )
            .await;
        assert_eq!(written.status(), StatusCode::OK);
        let body = body_json(written).await;
        assert_eq!(body["extra_flags"], serde_json::json!(["-XX:+UseG1GC"]));
        assert_eq!(body["stripped_flags"], serde_json::json!(["-Xmx32768M"]));
        assert_eq!(body["memory_mib"], 4096, "the slider keeps the heap");
        assert!(!body["startup_command"].as_str().unwrap().contains("-Xmx"));

        let again = world.call(fetch(&format!("/servers/{}/startup", world.server)), &secret).await;
        assert_eq!(body_json(again).await["stripped_flags"], serde_json::json!([]));
    }

    #[tokio::test]
    async fn moving_the_slider_alone_does_not_make_the_panel_report_itself() {
        let world = world().await;
        let boss = an_admin(&world.pool, "boss").await;
        let secret = sign_in(&world.pool, boss).await;
        let path = format!("/servers/{}/startup", world.server);

        let opened = body_json(world.call(fetch(&path), &secret).await).await;
        let field = opened["startup_command"].as_str().unwrap().to_owned();

        let saved = world
            .call(
                send(
                    "PATCH",
                    &path,
                    serde_json::json!({ "startup_command": field, "memory_mib": 2048 }),
                ),
                &secret,
            )
            .await;

        assert_eq!(saved.status(), StatusCode::OK);
        let body = body_json(saved).await;
        assert_eq!(body["stripped_flags"], serde_json::json!([]), "nothing was taken from him");
        assert_eq!(body["memory_mib"], 2048);
        assert_eq!(body["managed_flags"], serde_json::json!(["-Xmx2048M"]));
    }

    #[tokio::test]
    async fn a_running_server_is_told_to_restart_and_a_stopped_one_is_not() {
        let running = a_running_world().await;
        let secret = running.as_owner().await;
        let path = format!("/servers/{}/startup", running.server);

        let saved = running
            .call(send("PATCH", &path, serde_json::json!({ "memory_mib": 1024 })), &secret)
            .await;
        assert_eq!(body_json(saved).await["restart_required"], true);

        let again = running
            .call(send("PATCH", &path, serde_json::json!({ "memory_mib": 1024 })), &secret)
            .await;
        assert_eq!(body_json(again).await["restart_required"], true);

        let stopped = world().await;
        let his = stopped.as_owner().await;
        let quiet = stopped
            .call(
                send(
                    "PATCH",
                    &format!("/servers/{}/startup", stopped.server),
                    serde_json::json!({ "memory_mib": 1024 }),
                ),
                &his,
            )
            .await;
        assert_eq!(
            body_json(quiet).await["restart_required"],
            false,
            "a stopped server starts with whatever the row says"
        );
    }

    #[tokio::test]
    async fn a_java_version_the_machine_does_not_have_is_refused_by_name() {
        let world = world().await;
        let secret = world.as_owner().await;

        let refused = world
            .call(
                send(
                    "PATCH",
                    &format!("/servers/{}/startup", world.server),
                    serde_json::json!({ "java_version": 3 }),
                ),
                &secret,
            )
            .await;
        assert_eq!(refused.status(), StatusCode::BAD_REQUEST);
        assert_eq!(body_json(refused).await["error"], "invalid_java_version");

        let boss = an_admin(&world.pool, "boss").await;
        let flags = world
            .call(
                send(
                    "PATCH",
                    &format!("/servers/{}/startup", world.server),
                    serde_json::json!({ "startup_command": "java -XX:+UseG1GC -jar server.jar nogui" }),
                ),
                &sign_in(&world.pool, boss).await,
            )
            .await;
        assert_eq!(flags.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn the_startup_command_is_the_panel_admins_and_the_owners_memory_is_his_own() {
        let world = world().await;
        let secret = world.as_owner().await;
        let path = format!("/servers/{}/startup", world.server);

        let refused = world
            .call(
                send(
                    "PATCH",
                    &path,
                    serde_json::json!({ "startup_command": "java -XX:+UseG1GC -jar server.jar nogui" }),
                ),
                &secret,
            )
            .await;
        assert_eq!(refused.status(), StatusCode::FORBIDDEN, "the owner is not enough");
        assert_eq!(body_json(refused).await["error"], "forbidden");

        let unchanged = body_json(world.call(fetch(&path), &secret).await).await;
        let same = unchanged["startup_command"].as_str().unwrap().to_owned();
        let echoed = world
            .call(send("PATCH", &path, serde_json::json!({ "startup_command": same })), &secret)
            .await;
        assert_eq!(echoed.status(), StatusCode::FORBIDDEN);

        let saved = world
            .call(send("PATCH", &path, serde_json::json!({ "memory_mib": 1024 })), &secret)
            .await;
        assert_eq!(saved.status(), StatusCode::OK);
        assert_eq!(body_json(saved).await["extra_flags"], serde_json::json!([]));

        let boss = an_admin(&world.pool, "boss").await;
        let allowed = world
            .call(
                send(
                    "PATCH",
                    &path,
                    serde_json::json!({ "startup_command": "java -XX:+UseG1GC -jar server.jar nogui" }),
                ),
                &sign_in(&world.pool, boss).await,
            )
            .await;
        assert_eq!(allowed.status(), StatusCode::OK, "and the panel admin may");
        assert_eq!(body_json(allowed).await["extra_flags"], serde_json::json!(["-XX:+UseG1GC"]));
    }

    #[tokio::test]
    async fn memory_stops_where_the_owners_budget_stops() {
        let world = world().await;
        let secret = world.as_owner().await;
        a_server(&world.pool, world.owner, "Second", 3072).await;

        let refused = world
            .call(
                send(
                    "PATCH",
                    &format!("/servers/{}/startup", world.server),
                    serde_json::json!({ "memory_mib": 2048 }),
                ),
                &secret,
            )
            .await;
        assert_eq!(refused.status(), StatusCode::CONFLICT);
        assert_eq!(body_json(refused).await["error"], "budget_exceeded");

        let small = world
            .call(
                send(
                    "PATCH",
                    &format!("/servers/{}/startup", world.server),
                    serde_json::json!({ "memory_mib": 256 }),
                ),
                &secret,
            )
            .await;
        assert_eq!(small.status(), StatusCode::BAD_REQUEST);
        assert_eq!(body_json(small).await["error"], "memory_too_small");

        let fits = world
            .call(
                send(
                    "PATCH",
                    &format!("/servers/{}/startup", world.server),
                    serde_json::json!({ "memory_mib": 1024 }),
                ),
                &secret,
            )
            .await;
        assert_eq!(fits.status(), StatusCode::OK);
        assert_eq!(body_json(fits).await["memory_mib"], 1024);
    }

    #[tokio::test]
    async fn the_allocation_endpoints_answer_the_shapes_the_page_calls_map_on() {
        let world = world().await;
        let secret = world.as_owner().await;
        an_allocation(&world.pool, world.server, 25565, "game", true).await;

        let listed =
            world.call(fetch(&format!("/servers/{}/allocations", world.server)), &secret).await;
        let body = body_json(listed).await;
        assert!(body.is_array(), "9.6: a bare list, or network.vue:275 throws");
        assert_eq!(body.as_array().unwrap().len(), 0, "and without the primary port");

        let made = world
            .call(
                send(
                    "POST",
                    &format!("/servers/{}/allocations", world.server),
                    serde_json::json!({ "name": "Map" }),
                ),
                &secret,
            )
            .await;
        assert_eq!(made.status(), StatusCode::CREATED);
        let port = body_json(made).await["port"].as_u64().unwrap() as u16;

        let renamed = world
            .call(
                send(
                    "PATCH",
                    &format!("/servers/{}/allocations/{port}", world.server),
                    serde_json::json!({ "name": "Dynmap" }),
                ),
                &secret,
            )
            .await;
        assert_eq!(body_json(renamed).await["name"], "Dynmap");

        let gone = world
            .call(empty("DELETE", &format!("/servers/{}/allocations/{port}", world.server)), &secret)
            .await;
        assert_eq!(gone.status(), StatusCode::NO_CONTENT);

        let missing = world
            .call(empty("DELETE", &format!("/servers/{}/allocations/{port}", world.server)), &secret)
            .await;
        assert_eq!(missing.status(), StatusCode::NOT_FOUND);
        assert_eq!(body_json(missing).await["error"], "allocation_not_found");
    }

    #[tokio::test]
    async fn the_primary_port_is_refused_and_an_empty_name_is_no_rename() {
        let world = world().await;
        let secret = world.as_owner().await;
        an_allocation(&world.pool, world.server, 25565, "game", true).await;
        an_allocation(&world.pool, world.server, 25566, "Map", false).await;

        let refused = world
            .call(empty("DELETE", &format!("/servers/{}/allocations/25565", world.server)), &secret)
            .await;
        assert_eq!(refused.status(), StatusCode::CONFLICT);
        assert_eq!(body_json(refused).await["error"], "primary_allocation");

        let nameless = world
            .call(
                send(
                    "PATCH",
                    &format!("/servers/{}/allocations/25566", world.server),
                    serde_json::json!({ "name": "" }),
                ),
                &secret,
            )
            .await;
        assert_eq!(nameless.status(), StatusCode::BAD_REQUEST);
        assert_eq!(body_json(nameless).await["error"], "invalid_name");

        let held: Vec<(u16, String)> =
            sqlx::query_as("SELECT port, name FROM allocations WHERE server_id = ? ORDER BY port")
                .bind(world.server)
                .fetch_all(&world.pool)
                .await
                .unwrap();
        assert_eq!(
            held,
            [(25565, "game".to_owned()), (25566, "Map".to_owned())],
            "neither call moved anything"
        );

        let written: i64 = sqlx::query_scalar("SELECT count(*) FROM audit_log WHERE server_id = ?")
            .bind(world.server)
            .fetch_one(&world.pool)
            .await
            .unwrap();
        assert_eq!(written, 0, "11.9 writes down deeds, not refusals");
    }

    #[tokio::test]
    async fn the_catalogue_is_ten_entries_and_needs_a_session() {
        let world = world().await;
        let secret = world.as_owner().await;

        let anonymous = world.app.clone().oneshot(fetch("/loaders")).await.unwrap();
        assert_eq!(anonymous.status(), StatusCode::UNAUTHORIZED);

        let listed = world.call(fetch("/loaders"), &secret).await;
        assert_eq!(listed.status(), StatusCode::OK);
        let body = body_json(listed).await;
        assert_eq!(body["loaders"].as_array().unwrap().len(), 10);
        assert_eq!(body["loaders"][0]["id"], "vanilla");
        assert_eq!(body["loaders"][7]["name"], "NeoForge");
        assert_eq!(body["loaders"][6]["supports_properties"], false, "velocity");
    }

    #[tokio::test]
    async fn a_loader_nobody_has_heard_of_is_a_404_and_not_a_502() {
        let world = world().await;
        let secret = world.as_owner().await;

        let refused = world.call(fetch("/loaders/spigot/game-versions"), &secret).await;
        assert_eq!(refused.status(), StatusCode::NOT_FOUND);
        assert_eq!(body_json(refused).await["error"], "loader_not_found");
    }

    #[tokio::test]
    async fn reset_to_setup_is_the_panel_admins_alone_and_leaves_the_files_alone() {
        let world = world().await;
        let secret = world.as_owner().await;
        std::fs::write(world.server_dir().join("server.properties"), "motd=x\n").unwrap();

        let refused = world
            .call(empty("POST", &format!("/servers/{}/reset-to-setup", world.server)), &secret)
            .await;
        assert_eq!(refused.status(), StatusCode::FORBIDDEN, "2.1: the owner is not enough");

        let boss = an_admin(&world.pool, "boss").await;
        let admin_secret = sign_in(&world.pool, boss).await;
        let done = world
            .call(empty("POST", &format!("/servers/{}/reset-to-setup", world.server)), &admin_secret)
            .await;
        assert_eq!(done.status(), StatusCode::OK);
        assert_eq!(body_json(done).await["flows"]["intro"], true);

        assert!(
            world.server_dir().join("server.properties").is_file(),
            "9.17: this is for finishing a setup, not for clearing up"
        );
        let (intro, loader): (bool, Option<String>) =
            sqlx::query_as("SELECT flows_intro, loader FROM servers WHERE id = ?")
                .bind(world.server)
                .fetch_one(&world.pool)
                .await
                .unwrap();
        assert!(intro);
        assert_eq!(loader, None);

        let _ = serde_json::to_value(Flows { intro: true }).unwrap();
    }

    #[tokio::test]
    async fn a_reset_that_would_drop_the_backups_is_refused() {
        let world = world().await;
        let secret = world.as_owner().await;

        let refused = world
            .call(
                send(
                    "POST",
                    &format!("/servers/{}/reset", world.server),
                    serde_json::json!({
                        "loader": "paper",
                        "game_version": "1.21.8",
                        "loader_version": null,
                        "keep_backups": false
                    }),
                ),
                &secret,
            )
            .await;
        assert_eq!(refused.status(), StatusCode::BAD_REQUEST);
        assert_eq!(body_json(refused).await["error"], "invalid_request");
    }

    #[tokio::test]
    async fn an_unknown_loader_in_a_body_is_422_and_a_family_swap_needs_a_wipe() {
        let world = world().await;
        let secret = world.as_owner().await;

        let unknown = world
            .call(
                send(
                    "POST",
                    &format!("/servers/{}/install", world.server),
                    serde_json::json!({
                        "loader": "spigot",
                        "game_version": "1.21.8",
                        "loader_version": null,
                        "content_policy": "keep"
                    }),
                ),
                &secret,
            )
            .await;
        assert_eq!(unknown.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(body_json(unknown).await["error"], "unknown_loader");

        let wipe = world
            .call(
                send(
                    "POST",
                    &format!("/servers/{}/install", world.server),
                    serde_json::json!({
                        "loader": "fabric",
                        "game_version": "1.21.8",
                        "loader_version": null,
                        "content_policy": "keep"
                    }),
                ),
                &secret,
            )
            .await;
        assert_eq!(wipe.status(), StatusCode::CONFLICT);
        assert_eq!(body_json(wipe).await["error"], "loader_change_needs_wipe");
    }

    #[tokio::test]
    async fn nothing_installs_while_the_server_is_running() {
        let world = a_running_world().await;
        let secret = world.as_owner().await;

        for (method, path, body) in [
            (
                "POST",
                format!("/servers/{}/install", world.server),
                serde_json::json!({
                    "loader": "paper", "game_version": "1.21.8",
                    "loader_version": null, "content_policy": "keep"
                }),
            ),
            (
                "POST",
                format!("/servers/{}/reset", world.server),
                serde_json::json!({
                    "loader": "paper", "game_version": "1.21.8",
                    "loader_version": null, "keep_backups": true
                }),
            ),
        ] {
            let response = world.call(send(method, &path, body), &secret).await;
            assert_eq!(response.status(), StatusCode::CONFLICT, "{path}");
            assert_eq!(body_json(response).await["error"], "server_running");
        }

        let repair = world
            .call(empty("POST", &format!("/servers/{}/repair", world.server)), &secret)
            .await;
        assert_eq!(repair.status(), StatusCode::CONFLICT);
        assert_eq!(body_json(repair).await["error"], "server_running");
    }

    #[tokio::test]
    async fn every_change_of_this_area_leaves_a_line_in_the_check_log() {
        let world = world().await;
        let secret = world.as_owner().await;
        an_allocation(&world.pool, world.server, 25565, "game", true).await;

        world
            .call(
                send(
                    "PATCH",
                    &format!("/servers/{}/properties", world.server),
                    serde_json::json!({ "known": { "motd": "New" } }),
                ),
                &secret,
            )
            .await;
        world
            .call(
                send(
                    "PATCH",
                    &format!("/servers/{}/startup", world.server),
                    serde_json::json!({ "memory_mib": 2048 }),
                ),
                &secret,
            )
            .await;
        let made = world
            .call(
                send(
                    "POST",
                    &format!("/servers/{}/allocations", world.server),
                    serde_json::json!({ "name": "Map" }),
                ),
                &secret,
            )
            .await;
        let port = body_json(made).await["port"].as_u64().unwrap();
        world
            .call(
                empty("DELETE", &format!("/servers/{}/allocations/{port}", world.server)),
                &secret,
            )
            .await;

        let written: Vec<String> = sqlx::query_scalar(
            "SELECT action FROM audit_log WHERE server_id = ? ORDER BY id",
        )
        .bind(world.server)
        .fetch_all(&world.pool)
        .await
        .unwrap();

        assert_eq!(
            written,
            [
                "server_properties_modified",
                "server_reallocated",
                "port_allocation_added",
                "port_allocation_removed",
            ],
            "11.9 keeps one name per deed, and -Xmx is `server_reallocated`"
        );

        world
            .call(
                send(
                    "PATCH",
                    &format!("/servers/{}/startup", world.server),
                    serde_json::json!({ "memory_mib": 2048 }),
                ),
                &secret,
            )
            .await;
        let after: i64 = sqlx::query_scalar("SELECT count(*) FROM audit_log WHERE server_id = ?")
            .bind(world.server)
            .fetch_one(&world.pool)
            .await
            .unwrap();
        assert_eq!(after, 4);
    }

    #[tokio::test]
    async fn a_stranger_is_told_the_server_does_not_exist_and_a_viewer_may_only_look() {
        let world = world().await;
        let anna = a_user(&world.pool, "anna").await;
        let stranger = sign_in(&world.pool, anna).await;

        for path in ["properties", "startup", "allocations"] {
            let response = world
                .call(fetch(&format!("/servers/{}/{path}", world.server)), &stranger)
                .await;
            assert_eq!(response.status(), StatusCode::NOT_FOUND, "{path}");
            assert_eq!(body_json(response).await["error"], "server_not_found");
        }

        sqlx::query(
            "INSERT INTO server_members (id, server_id, user_id, role, invited_at, joined_at) \
             VALUES (?, ?, ?, 'viewer', ?, ?)",
        )
        .bind(Id::new())
        .bind(world.server)
        .bind(anna)
        .bind(Timestamp::now())
        .bind(Timestamp::now())
        .execute(&world.pool)
        .await
        .unwrap();

        let allowed =
            world.call(fetch(&format!("/servers/{}/properties", world.server)), &stranger).await;
        assert_eq!(allowed.status(), StatusCode::OK, "2.1: a viewer holds BASE_READ");

        let refused = world
            .call(
                send(
                    "PATCH",
                    &format!("/servers/{}/properties", world.server),
                    serde_json::json!({ "known": { "motd": "x" } }),
                ),
                &stranger,
            )
            .await;
        assert_eq!(refused.status(), StatusCode::FORBIDDEN, "2.1: writing wants ADVANCED");
    }

    #[tokio::test]
    async fn the_matrix_of_2_1_holds_for_all_thirteen_endpoints() {
        let world = world().await;
        let install = serde_json::json!({
            "loader": "paper", "game_version": "1.21.8",
            "loader_version": null, "content_policy": "keep"
        });
        let reset = serde_json::json!({
            "loader": "paper", "game_version": "1.21.8",
            "loader_version": null, "keep_backups": true
        });

        let writes: Vec<(&str, String, Option<serde_json::Value>, bool)> = vec![
            (
                "PATCH",
                format!("/servers/{}/properties", world.server),
                Some(serde_json::json!({ "known": { "motd": "x" } })),
                true,
            ),
            (
                "PATCH",
                format!("/servers/{}/startup", world.server),
                Some(serde_json::json!({ "memory_mib": 1024 })),
                true,
            ),
            (
                "POST",
                format!("/servers/{}/allocations", world.server),
                Some(serde_json::json!({ "name": "Map" })),
                true,
            ),
            (
                "PATCH",
                format!("/servers/{}/allocations/25599", world.server),
                Some(serde_json::json!({ "name": "Map" })),
                true,
            ),
            ("DELETE", format!("/servers/{}/allocations/25599", world.server), None, true),
            ("PUT", format!("/servers/{}/allocations/25599/primary", world.server), None, true),
            ("POST", format!("/servers/{}/install", world.server), Some(install), true),
            ("POST", format!("/servers/{}/repair", world.server), None, true),
            ("POST", format!("/servers/{}/reset", world.server), Some(reset), false),
            ("POST", format!("/servers/{}/reset-to-setup", world.server), None, false),
        ];

        let viewer = world.member("vera", "viewer").await;
        let editor = world.member("erik", "editor").await;

        for (method, path, body, editor_may) in writes {
            let request = || match &body {
                Some(body) => send(method, &path, body.clone()),
                None => empty(method, &path),
            };

            let refused = world.call(request(), &viewer).await;
            assert_eq!(refused.status(), StatusCode::FORBIDDEN, "a viewer at {method} {path}");

            let answer = world.call(request(), &editor).await;
            assert_eq!(
                answer.status() == StatusCode::FORBIDDEN,
                !editor_may,
                "an editor at {method} {path} answered {}",
                answer.status()
            );
        }

        for path in ["properties", "startup", "allocations"] {
            let read =
                world.call(fetch(&format!("/servers/{}/{path}", world.server)), &viewer).await;
            assert_eq!(read.status(), StatusCode::OK, "a viewer reading {path}");
        }
    }

    #[tokio::test]
    async fn a_linked_properties_file_is_refused_rather_than_read_out_of_the_tree() {
        let world = world().await;
        let secret = world.as_owner().await;
        let panel_own = world.dir.path().join("config.toml");
        std::fs::write(&panel_own, "motd=THE PANEL SECRET\n").unwrap();
        std::os::unix::fs::symlink(&panel_own, world.server_dir().join("server.properties"))
            .unwrap();

        let read =
            world.call(fetch(&format!("/servers/{}/properties", world.server)), &secret).await;

        assert_eq!(read.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let body = body_json(read).await;
        assert_ne!(
            body["known"]["motd"], "THE PANEL SECRET",
            "the panel's own files are not this server's properties"
        );
    }

    #[tokio::test]
    async fn a_patch_writes_into_the_server_directory_and_nowhere_a_link_points() {
        let world = world().await;
        let secret = world.as_owner().await;
        let panel_own = world.dir.path().join("config.toml");
        std::fs::write(&panel_own, "bind = \"127.0.0.1:8080\"\n").unwrap();
        std::os::unix::fs::symlink(&panel_own, world.server_dir().join(".server.properties.new"))
            .unwrap();

        let written = world
            .call(
                send(
                    "PATCH",
                    &format!("/servers/{}/properties", world.server),
                    serde_json::json!({ "known": { "motd": "New" } }),
                ),
                &secret,
            )
            .await;

        assert_eq!(written.status(), StatusCode::OK);
        assert_eq!(
            std::fs::read_to_string(&panel_own).unwrap(),
            "bind = \"127.0.0.1:8080\"\n",
            "docs/PLAN.md:179 — the panel writes as itself and owns this file"
        );
        assert!(std::fs::read_to_string(world.server_dir().join("server.properties"))
            .unwrap()
            .contains("motd=New"));
    }

    #[tokio::test]
    async fn a_memory_figure_at_the_top_of_its_type_is_refused_rather_than_wrapped() {
        let world = world().await;
        let secret = world.as_owner().await;
        a_server(&world.pool, world.owner, "Second", 3072).await;

        let refused = world
            .call(
                send(
                    "PATCH",
                    &format!("/servers/{}/startup", world.server),
                    serde_json::json!({ "memory_mib": u32::MAX }),
                ),
                &secret,
            )
            .await;

        assert_eq!(refused.status(), StatusCode::CONFLICT);
        assert_eq!(body_json(refused).await["error"], "budget_exceeded");

        let held: u32 = sqlx::query_scalar("SELECT memory_mib FROM servers WHERE id = ?")
            .bind(world.server)
            .fetch_one(&world.pool)
            .await
            .unwrap();
        assert_eq!(held, 4096, "and nothing was written");
    }

    #[tokio::test]
    async fn an_admin_is_not_stopped_at_the_machine() {
        let world = world().await;
        let anna = an_admin(&world.pool, "anna").await;
        let secret = sign_in(&world.pool, anna).await;
        let machine = crate::auth::usage::Host::measure().assignable_memory_mib();

        let allowed = world
            .call(
                send(
                    "PATCH",
                    &format!("/servers/{}/startup", world.server),
                    serde_json::json!({ "memory_mib": machine + 1 }),
                ),
                &secret,
            )
            .await;
        assert_eq!(allowed.status(), StatusCode::OK);
        assert_eq!(body_json(allowed).await["memory_mib"], machine + 1);

        let held: u32 = sqlx::query_scalar("SELECT memory_mib FROM servers WHERE id = ?")
            .bind(world.server)
            .fetch_one(&world.pool)
            .await
            .unwrap();
        assert_eq!(held, machine + 1, "what was answered is what stands in the row");

        let over_the_owner = world
            .call(
                send(
                    "PATCH",
                    &format!("/servers/{}/startup", world.server),
                    serde_json::json!({ "memory_mib": 8192 }),
                ),
                &secret,
            )
            .await;
        assert_eq!(over_the_owner.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn a_member_of_an_admins_server_is_not_held_to_the_admins_row() {
        let world = world().await;
        let boss = an_admin(&world.pool, "boss").await;
        let his = a_server(&world.pool, boss, "Boss-Welt", 2048).await;
        set_loader(&world.pool, his, LoaderId::Paper, "1.21.8", Some("60")).await;
        a_member(&world.pool, his, world.owner, "editor").await;
        let secret = world.as_owner().await;
        let machine = crate::auth::usage::Host::measure().assignable_memory_mib();

        let shown = body_json(world.call(fetch(&format!("/servers/{his}/startup")), &secret).await)
            .await;
        assert_eq!(
            shown["memory_max_mib"],
            machine,
            "the owner has no budget, so the slider ends at the machine"
        );

        let saved = world
            .call(
                send(
                    "PATCH",
                    &format!("/servers/{his}/startup"),
                    serde_json::json!({ "memory_mib": 8192 }),
                ),
                &secret,
            )
            .await;
        assert_eq!(saved.status(), StatusCode::OK);
        assert_eq!(body_json(saved).await["memory_mib"], 8192);
    }

    #[tokio::test]
    async fn java_runtimes_says_nothing_about_a_server_the_caller_cannot_read() {
        let world = world().await;
        let anna = a_user(&world.pool, "anna").await;
        let stranger = sign_in(&world.pool, anna).await;

        let refused = world
            .call(fetch(&format!("/java-runtimes?server_id={}", world.server)), &stranger)
            .await;
        assert_eq!(refused.status(), StatusCode::NOT_FOUND);
        assert_eq!(body_json(refused).await["error"], "server_not_found");

        let listed = world.call(fetch("/java-runtimes"), &stranger).await;
        assert_eq!(listed.status(), StatusCode::OK, "without an id it is everybody's");
        assert_eq!(body_json(listed).await["default_major_for_game_version"], serde_json::Value::Null);
    }

    #[tokio::test]
    async fn a_path_that_is_not_a_ulid_is_refused_in_the_shape_of_1_7() {
        let world = world().await;
        let secret = world.as_owner().await;

        for path in [
            "/servers/not-a-ulid/properties",
            "/servers/not-a-ulid/startup",
            "/servers/not-a-ulid/allocations",
        ] {
            let refused = world.call(fetch(path), &secret).await;
            assert_eq!(refused.status(), StatusCode::BAD_REQUEST, "{path}");
            assert_eq!(
                refused.headers().get(axum::http::header::CONTENT_TYPE).unwrap(),
                "application/json",
                "{path}"
            );
            let body = body_json(refused).await;
            assert_eq!(body["error"], "invalid_request", "{path}");
            assert_eq!(body.as_object().unwrap().len(), 2, "1.7 allows exactly two fields");
        }

        let refused = world
            .call(empty("DELETE", &format!("/servers/{}/allocations/nope", world.server)), &secret)
            .await;
        assert_eq!(refused.status(), StatusCode::BAD_REQUEST);
        assert_eq!(body_json(refused).await["error"], "invalid_request");
    }

    #[tokio::test]
    async fn a_link_where_the_server_directory_belongs_reaches_no_other_tree() {
        let world = world().await;
        let secret = world.as_owner().await;

        let theirs = world.dir.path().join("users").join(Id::new().to_string()).join("servers");
        std::fs::create_dir_all(&theirs).unwrap();
        std::fs::write(theirs.join("server.properties"), "motd=BELONGS TO SOMEBODY ELSE\n")
            .unwrap();
        std::fs::remove_dir_all(world.server_dir()).unwrap();
        std::os::unix::fs::symlink(&theirs, world.server_dir()).unwrap();

        let answered = world
            .call(
                send(
                    "PATCH",
                    &format!("/servers/{}/properties", world.server),
                    serde_json::json!({ "known": { "motd": "MINE NOW" } }),
                ),
                &secret,
            )
            .await;

        assert_eq!(answered.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(
            std::fs::read_to_string(theirs.join("server.properties")).unwrap(),
            "motd=BELONGS TO SOMEBODY ELSE\n",
            "docs/PLAN.md:305 — the damage has to stop at the owner's edge"
        );

        let read = world
            .call(fetch(&format!("/servers/{}/properties", world.server)), &secret)
            .await;
        assert_eq!(read.status(), StatusCode::INTERNAL_SERVER_ERROR, "nor is it read out");
    }

    #[tokio::test]
    async fn a_version_out_of_the_url_cannot_choose_which_endpoint_the_source_is_asked_for() {
        let world = world().await;
        let secret = world.as_owner().await;

        let refused = world
            .call(
                fetch(&format!(
                    "/loaders/paper/game-versions/{}/builds",
                    "..%2F..%2Fvelocity%2Fversions%2F3.5.1"
                )),
                &secret,
            )
            .await;
        assert_eq!(refused.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(body_json(refused).await["error"], "unsupported_game_version");

        let unknown = world.call(fetch("/loaders/..%2F..%2Fadmin%2Fusers/game-versions"), &secret).await;
        assert_eq!(unknown.status(), StatusCode::NOT_FOUND);
        assert_eq!(body_json(unknown).await["error"], "loader_not_found");
    }

    #[tokio::test]
    async fn the_eula_is_written_into_the_directory_and_not_through_a_link() {
        let world = world().await;
        let panel_own = world.dir.path().join("config.toml");
        std::fs::write(&panel_own, "bind = \"127.0.0.1:8080\"\n").unwrap();
        std::os::unix::fs::symlink(&panel_own, world.server_dir().join("eula.txt")).unwrap();

        let row = crate::settings::load_server(&world.pool, world.server).await.unwrap();
        let runner = Runner::new(
            world.pool.clone(),
            Operations::new(world.pool.clone(), world.dir.path()),
            catalog().unwrap(),
            Helper::new(&std::path::PathBuf::from("/nonexistent")),
            world.dir.path().to_path_buf(),
            world.dir.path().join("cache"),
        );
        let plan = Plan {
            loader: LoaderId::Paper,
            game_version: "1.21.8".to_owned(),
            build: None,
            policy: ContentPolicy::Keep,
        };

        runner.write_config_for_test(&row, &world.server_dir(), &plan).await.unwrap();

        assert_eq!(std::fs::read_to_string(&panel_own).unwrap(), "bind = \"127.0.0.1:8080\"\n");
        assert!(std::fs::read_to_string(world.server_dir().join("eula.txt"))
            .unwrap()
            .contains("eula=true"));
    }
}
