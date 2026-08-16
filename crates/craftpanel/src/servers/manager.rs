#![allow(dead_code)]

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures::future::BoxFuture;
use craftpanel_proto::SpawnRequest;
use serde::{Deserialize, Serialize};
use sqlx::{Sqlite, SqlitePool, Transaction};

use crate::auth::disk::{self, Disks};
use crate::auth::error::{Failure, Result};
use crate::auth::extract::Caller;
use crate::auth::users::{self};
use crate::config::Config;
use crate::helper::Helper;
use crate::loaders::{Build, Channel, Checksum, Loader, LoaderError, Sources, Wanted};
use crate::model::{
    AuditAction, Id, LoaderId, Minecraft, Operation, OperationError, OperationErrorStep,
    OperationKind, OperationPhase, OperationState, Permission, Permissions, PowerAction,
    PowerState, PowerTarget,
    PropertiesFields, Server, ServerFlows, ServerNet, ServerRole, ServerStatus, ServerUpstream,
    SystemUserState, Timestamp, UpdateChannel, UserRef, KNOWN_PROPERTY_KEYS,
};
use crate::ops::{NewOperation, Operations, StateReport, StatsSample, Step};

use super::{Hub, RunState};

pub const JAR: &str = "server.jar";
const START_TIMEOUT: Duration = Duration::from_secs(120);
pub const REATTACH_GRACE: Duration = Duration::from_secs(6);
const WATCH_TICK: Duration = Duration::from_millis(400);
const DISPATCH_TICK: Duration = Duration::from_millis(300);
const SAMPLE_TICK: Duration = Duration::from_secs(1);
const IDLE_SAMPLE_EVERY: Duration = Duration::from_secs(30);
const STORAGE_EVERY: Duration = Duration::from_secs(30);
const USER_HZ: f64 = 100.0;
const MIB: u64 = 1024 * 1024;
const MIN_MEMORY_MIB: u32 = 512;
const NAME_LIMIT: usize = 64;

pub trait Builds: Send + Sync {
    fn resolve<'a>(
        &'a self,
        loader: Loader,
        game_version: &'a str,
        wanted: Wanted,
    ) -> BoxFuture<'a, std::result::Result<Build, LoaderError>>;

    fn fetch<'a>(
        &'a self,
        loader: Loader,
        build: &'a Build,
        dest: &'a Path,
    ) -> BoxFuture<'a, std::result::Result<u64, LoaderError>>;
}

impl Builds for Sources {
    fn resolve<'a>(
        &'a self,
        loader: Loader,
        game_version: &'a str,
        wanted: Wanted,
    ) -> BoxFuture<'a, std::result::Result<Build, LoaderError>> {
        Box::pin(async move { Sources::resolve(self, loader, game_version, &wanted).await })
    }

    fn fetch<'a>(
        &'a self,
        loader: Loader,
        build: &'a Build,
        dest: &'a Path,
    ) -> BoxFuture<'a, std::result::Result<u64, LoaderError>> {
        Box::pin(async move { Sources::download(self, loader, build, dest).await })
    }
}

#[derive(Debug, Clone)]
pub struct NewServer {
    pub name: String,
    pub owner_id: Id,
    pub memory_mib: u32,
    pub port: Option<u16>,
    pub content: CreateContent,
    pub properties: PropertiesFields,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CreateContent {
    Loader {
        loader: LoaderId,
        game_version: String,
        loader_version: Option<String>,
    },
    ModpackProject {
        project_id: String,
        version_id: String,
    },
    ModpackUpload {
        file_name: String,
        file_size: u64,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ServerWarning {
    MemoryOvercommitted,
    PropertiesWillBeIgnored,
}

#[derive(Debug, Clone)]
pub struct Created {
    pub server: Server,
    pub operation: Operation,
    pub warnings: Vec<ServerWarning>,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Reconciliation {
    pub attached: Vec<Id>,
    pub cleared: Vec<Id>,
    pub broken: Vec<Id>,
    pub resumed: Vec<Id>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Input {
    #[serde(default)]
    content: Option<CreateContent>,
    #[serde(default)]
    properties: PropertiesFields,
    #[serde(default)]
    build: Option<Chosen>,
    #[serde(default = "yes")]
    keep_backups: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Chosen {
    id: String,
    channel: Channel,
    url: String,
    filename: String,
    checksum: Option<Checksum>,
    size: Option<u64>,
    java_major: Option<u32>,
}

impl From<Build> for Chosen {
    fn from(build: Build) -> Self {
        Self {
            id: build.id,
            channel: build.channel,
            url: build.url,
            filename: build.filename,
            checksum: build.checksum,
            size: build.size,
            java_major: build.java_major,
        }
    }
}

impl Chosen {
    fn build(&self) -> Build {
        Build {
            id: self.id.clone(),
            channel: self.channel,
            url: self.url.clone(),
            filename: self.filename.clone(),
            checksum: self.checksum.clone(),
            size: self.size,
            java_major: self.java_major,
        }
    }
}

fn yes() -> bool {
    true
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Wish {
    target: Option<PowerTarget>,
    killed: bool,
    undelivered: bool,
}

pub struct Manager {
    pool: SqlitePool,
    config: Arc<Config>,
    operations: Arc<Operations>,
    hub: Arc<Hub>,
    helper: Helper,
    builds: Arc<dyn Builds>,
    disks: Disks,
    gates: Mutex<HashMap<Id, Arc<tokio::sync::Mutex<()>>>>,
    wishes: Mutex<HashMap<Id, Wish>>,
    meters: Mutex<HashMap<Id, Meter>>,
    watched: Mutex<BTreeSet<Id>>,
}

impl Manager {
    pub fn new(
        pool: SqlitePool,
        config: Arc<Config>,
        operations: Arc<Operations>,
        hub: Arc<Hub>,
        helper: Helper,
        builds: Arc<dyn Builds>,
        disks: Disks,
    ) -> Arc<Self> {
        Arc::new(Self {
            pool,
            config,
            operations,
            hub,
            helper,
            builds,
            disks,
            gates: Mutex::default(),
            wishes: Mutex::default(),
            meters: Mutex::default(),
            watched: Mutex::default(),
        })
    }

    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    pub fn operations(&self) -> &Arc<Operations> {
        &self.operations
    }

    pub fn dir(&self, owner: Id, server: Id) -> PathBuf {
        self.config.users_dir().join(owner.to_string()).join("servers").join(server.to_string())
    }

    fn backups_dir(&self, server: Id) -> PathBuf {
        self.config.data_dir.join("backups").join(server.to_string())
    }

    pub async fn read(&self, server: Id, mask: Permissions) -> Result<Server> {
        let row = sqlx::query_as::<_, Row>(&format!("{SELECT} WHERE s.id = ?"))
            .bind(server)
            .fetch_optional(&self.pool)
            .await?
            .ok_or_else(unknown_server)?;
        Ok(row.into_server(mask))
    }

    pub async fn list(
        &self,
        caller: &Caller,
        everything: bool,
    ) -> Result<(Vec<Server>, BTreeMap<Id, UserRef>)> {
        let rows = if everything {
            sqlx::query_as::<_, Row>(&format!(
                "{SELECT} WHERE {IN_VIEW} ORDER BY s.created_at, s.id"
            ))
            .fetch_all(&self.pool)
            .await?
        } else {
            sqlx::query_as::<_, Row>(&format!(
                "{SELECT} WHERE {IN_VIEW}
                    AND (s.owner_id = ?
                         OR EXISTS (SELECT 1 FROM server_members m
                                     WHERE m.server_id = s.id AND m.user_id = ?
                                       AND m.joined_at IS NOT NULL))
                  ORDER BY s.created_at, s.id"
            ))
            .bind(caller.id())
            .bind(caller.id())
            .fetch_all(&self.pool)
            .await?
        };

        let shared: HashMap<Id, ServerRole> = sqlx::query_as::<_, (Id, ServerRole)>(
            "SELECT server_id, role FROM server_members
              WHERE user_id = ? AND joined_at IS NOT NULL",
        )
        .bind(caller.id())
        .fetch_all(&self.pool)
        .await?
        .into_iter()
        .collect();

        let owner_mask = Permissions::of(Permission::ServerAdmin);
        let servers: Vec<Server> = rows
            .into_iter()
            .map(|row| {
                let mask = if caller.is_admin() || row.owner_id == caller.id() {
                    owner_mask
                } else {
                    shared.get(&row.id).copied().map_or(Permissions::NONE, Permissions::from_role)
                };
                row.into_server(mask)
            })
            .collect();

        let mut owners = BTreeMap::new();
        for server in &servers {
            if let std::collections::btree_map::Entry::Vacant(slot) = owners.entry(server.owner_id)
            {
                if let Some(user) = users::find(&self.pool, server.owner_id).await? {
                    slot.insert(user.reference());
                }
            }
        }
        Ok((servers, owners))
    }

    pub async fn amend(
        &self,
        caller: &Caller,
        server: Id,
        name: Option<&str>,
        channel: Option<UpdateChannel>,
    ) -> Result<Server> {
        self.operations.guard_write(server).await.map_err(fault)?;
        if let Some(name) = name {
            check_name(name).map_err(|_| {
                Failure::bad_request("invalid_name", "a name is 1 to 64 printable characters")
            })?;
            sqlx::query("UPDATE servers SET name = ?, updated_at = ? WHERE id = ?")
                .bind(name)
                .bind(Timestamp::now())
                .bind(server)
                .execute(&self.pool)
                .await?;
            self.note(
                server,
                caller.id(),
                AuditAction::ChangedServerName,
                Some(serde_json::json!({ "name": name })),
            )
            .await?;
        }
        if let Some(channel) = channel {
            let before: Option<UpdateChannel> =
                sqlx::query_scalar("SELECT update_channel FROM servers WHERE id = ?")
                    .bind(server)
                    .fetch_optional(&self.pool)
                    .await?;
            sqlx::query("UPDATE servers SET update_channel = ?, updated_at = ? WHERE id = ?")
                .bind(channel)
                .bind(Timestamp::now())
                .bind(server)
                .execute(&self.pool)
                .await?;
            if before != Some(channel) {
                sqlx::query("UPDATE servers SET updates_checked_at = NULL WHERE id = ?")
                    .bind(server)
                    .execute(&self.pool)
                    .await?;
            }
        }

        let object = self.read(server, Permissions::of(Permission::ServerAdmin)).await?;
        self.announce(&object);
        Ok(object)
    }

    fn announce(&self, server: &Server) {
        self.operations.bus().channel(server.id).send_server(Arc::new(server.clone()));
    }

    pub async fn create(&self, caller: &Caller, wish: NewServer) -> Result<Created> {
        check_name(&wish.name)?;
        if wish.memory_mib < MIN_MEMORY_MIB {
            return Err(Failure::invalid_request("memory_mib is at least 512"));
        }
        if let Some(port) = wish.port {
            if port < 1024 {
                return Err(Failure::bad_request("invalid_port", "a port is 1024 to 65535"));
            }
        }

        let owner = users::load(&self.pool, wish.owner_id).await?;
        if owner.system_state != SystemUserState::Ready {
            return Err(Failure::conflict(
                "system_user_not_ready",
                "the system account of this owner is not ready",
            ));
        }
        disk::guard(&self.pool, &self.disks, owner.id, 0).await?;

        let build = match &wish.content {
            CreateContent::Loader { loader, game_version, loader_version } => {
                Some(self.resolve(*loader, game_version, loader_version.as_deref()).await?)
            }
            _ => None,
        };

        let server = Id::new();
        let now = Timestamp::now();
        let mut warnings = Vec::new();
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;

        let budget = owner.budget();
        let allocated = allocated_mib(&mut tx, owner.id).await?;
        if budget.exceeded_by(allocated) {
            return Err(Failure::conflict(
                "over_limit",
                "the owner is already over the memory he was given",
            ));
        }
        if !budget.has_room_for(allocated, wish.memory_mib) {
            if !caller.is_admin() {
                return Err(Failure::conflict(
                    "budget_exceeded",
                    "this server does not fit in the owner's memory budget",
                ));
            }
            warnings.push(ServerWarning::MemoryOvercommitted);
        }

        let port = match wish.port {
            Some(port) => port,
            None => next_free_port(&mut tx).await?,
        };

        sqlx::query(
            "INSERT INTO servers (id, name, owner_id, status, memory_mib, flows_intro,
                                  created_at, updated_at)
             VALUES (?, ?, ?, 'installing', ?, 0, ?, ?)",
        )
        .bind(server)
        .bind(wish.name.trim())
        .bind(owner.id)
        .bind(wish.memory_mib)
        .bind(now)
        .bind(now)
        .execute(&mut *tx)
        .await?;

        sqlx::query(
            "INSERT INTO allocations (port, server_id, name, is_primary, created_at)
             VALUES (?, ?, 'primary', 1, ?)",
        )
        .bind(port)
        .bind(server)
        .bind(now)
        .execute(&mut *tx)
        .await
        .map_err(taken_port)?;

        note_in(&mut tx, server, caller.id(), AuditAction::ServerCreated, None).await?;
        tx.commit().await?;

        tokio::fs::create_dir_all(self.dir(owner.id, server))
            .await
            .map_err(|err| Failure::internal(anyhow::Error::from(err)))?;

        if matches!(&wish.content, CreateContent::Loader { loader, .. }
                    if !loader.supports_properties())
        {
            warnings.push(ServerWarning::PropertiesWillBeIgnored);
        }

        let input = Input {
            content: Some(wish.content.clone()),
            properties: wish.properties,
            build: build.map(Chosen::from),
            keep_backups: true,
        };
        let mut new = NewOperation::new(server, OperationKind::ServerCreate, Some(caller.id()));
        new.input = serde_json::to_value(&input).ok();
        new.expects_payload = matches!(wish.content, CreateContent::ModpackUpload { .. });
        let operation = self.operations.create(new).await.map_err(fault)?;

        let object = self.read(server, Permissions::of(Permission::ServerAdmin)).await?;
        Ok(Created { server: object, operation, warnings })
    }

    async fn resolve(
        &self,
        loader: LoaderId,
        game_version: &str,
        loader_version: Option<&str>,
    ) -> Result<Build> {
        let Some(source) = loader.source() else {
            return Err(Failure::new(
                axum::http::StatusCode::UNPROCESSABLE_ENTITY,
                "unknown_loader",
                format!("{} cannot be installed yet", loader.as_str()),
            ));
        };
        let wanted = match loader_version {
            Some(build) => Wanted::Build(build.to_owned()),
            None => Wanted::LatestStable,
        };
        self.builds.resolve(source, game_version, wanted).await.map_err(refusal)
    }

    pub async fn delete(
        &self,
        caller: &Caller,
        server: Id,
        keep_backups: bool,
    ) -> Result<Operation> {
        let gate = self.gate(server);
        let _held = gate.lock().await;

        self.operations.guard_write(server).await.map_err(fault)?;
        if self.current(server).await?.is_live() {
            return Err(Failure::conflict(
                "server_running",
                "stop or kill the server before deleting it",
            ));
        }

        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        sqlx::query("UPDATE servers SET status = 'deleting', updated_at = ? WHERE id = ?")
            .bind(Timestamp::now())
            .bind(server)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM allocations WHERE server_id = ?")
            .bind(server)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;

        if let Ok(object) = self.read(server, Permissions::NONE).await {
            self.announce(&object);
        }

        let mut new = NewOperation::new(server, OperationKind::ServerDelete, Some(caller.id()));
        new.input = Some(serde_json::json!({ "keep_backups": keep_backups }));
        self.operations.create(new).await.map_err(fault)
    }

    pub fn spawn_dispatcher(self: &Arc<Self>) -> tokio::task::JoinHandle<()> {
        let manager = Arc::clone(self);
        tokio::spawn(async move {
            loop {
                if let Err(failure) = manager.dispatch().await {
                    tracing::error!("dispatching: {failure}");
                }
                tokio::time::sleep(DISPATCH_TICK).await;
            }
        })
    }

    async fn dispatch(self: &Arc<Self>) -> Result<()> {
        for id in self.operations.runnable().await.map_err(fault)? {
            let Ok(operation) = self.operations.get(id).await else { continue };
            if !matches!(operation.kind, OperationKind::ServerCreate | OperationKind::ServerDelete)
            {
                continue;
            }
            let manager = Arc::clone(self);
            tokio::spawn(async move { manager.run(id).await });
        }
        Ok(())
    }

    pub async fn run(self: &Arc<Self>, id: Id) -> bool {
        let Ok(Some(operation)) = self.operations.begin(id).await else {
            return false;
        };
        let outcome = match operation.kind {
            OperationKind::ServerCreate => self.install(&operation).await,
            OperationKind::ServerDelete => self.remove(&operation).await,
            _ => return false,
        };

        if let Err(error) = outcome {
            tracing::warn!(operation = %id, code = %error.code, "a run failed: {}", error.message);
            if operation.kind == OperationKind::ServerCreate {
                self.mark_broken(operation.server_id).await;
            }
            if let Err(failure) = self.operations.fail(id, error).await {
                tracing::error!("a failed run could not be written down: {}", failure.message());
            }
        }
        true
    }

    async fn install(&self, operation: &Operation) -> std::result::Result<(), OperationError> {
        let id = operation.id;
        let server = operation.server_id;
        let input: Input = self.input(id).await?;

        self.step(id, OperationPhase::Analyzing, 0.02, None, None).await;
        let (owner, _memory, port) = self.setting(server).await?;
        let dir = self.dir(owner, server);
        let work = self.operations.work_dir(id).await.map_err(internal_error)?;

        let (loader, game_version, chosen) = match (&input.content, input.build.clone()) {
            (Some(CreateContent::Loader { loader, game_version, .. }), Some(build)) => {
                (*loader, game_version.clone(), build)
            }
            (Some(CreateContent::Loader { .. }), None) => {
                return Err(OperationError {
                    code: "invalid_version".to_owned(),
                    message: "the specified version may be incorrect".to_owned(),
                    step: OperationErrorStep::Modloader,
                })
            }
            _ => {
                return Err(OperationError {
                    code: "modpack_install_failed".to_owned(),
                    message: "failed to install modpack".to_owned(),
                    step: OperationErrorStep::Modpack,
                })
            }
        };
        let source = loader.source().ok_or_else(|| OperationError {
            code: "loader_install_failed".to_owned(),
            message: "internal error".to_owned(),
            step: OperationErrorStep::Modloader,
        })?;

        if self.called_off(id).await {
            return self.give_up(id, &work).await;
        }

        self.step(
            id,
            OperationPhase::InstallingLoader,
            0.05,
            Some(chosen.filename.clone()),
            None,
        )
        .await;
        tokio::fs::create_dir_all(&work).await.map_err(disk)?;
        let jar = work.join(JAR);
        let written =
            self.builds.fetch(source, &chosen.build(), &jar).await.map_err(loader_failure)?;

        self.step(id, OperationPhase::Verifying, 0.60, None, None).await;
        if self.called_off(id).await {
            return self.give_up(id, &work).await;
        }

        self.step(id, OperationPhase::WritingConfig, 0.95, None, Some(false)).await;
        tokio::fs::create_dir_all(&dir).await.map_err(disk)?;
        tokio::fs::write(dir.join("eula.txt"), eula_text()).await.map_err(disk)?;
        if loader.supports_properties() {
            tokio::fs::write(
                dir.join("server.properties"),
                properties_text(&input.properties, port),
            )
            .await
            .map_err(disk)?;
        }
        tokio::fs::rename(&jar, dir.join(JAR)).await.map_err(disk)?;

        self.helper
            .chown_tree(&owner.to_string(), crate::helper::in_servers(server))
            .await
            .map_err(|err| internal_error(Failure::internal(err)))?;

        sqlx::query(
            "UPDATE servers SET status = 'available', loader = ?, loader_version = ?,
                                game_version = ?, java_major = COALESCE(?, java_major),
                                updated_at = ? WHERE id = ?",
        )
        .bind(loader)
        .bind(&chosen.id)
        .bind(&game_version)
        .bind(chosen.java_major)
        .bind(Timestamp::now())
        .bind(server)
        .execute(&self.pool)
        .await
        .map_err(|err| internal_error(Failure::from(err)))?;

        let _ = tokio::fs::remove_dir_all(&work).await;

        tracing::info!(%server, loader = loader.as_str(), bytes = written, "a server was set up");
        self.operations.finish(id).await.map_err(internal_error)?;
        if let Ok(object) = self.read(server, Permissions::NONE).await {
            self.announce(&object);
        }
        Ok(())
    }

    async fn remove(&self, operation: &Operation) -> std::result::Result<(), OperationError> {
        let id = operation.id;
        let server = operation.server_id;
        let keep_backups = self.input::<Input>(id).await.map_or(true, |input| input.keep_backups);
        let (owner, _, _) = self.setting(server).await?;

        if let Some(doomed) = self.confined(&self.dir(owner, server)).map_err(internal_error)? {
            let steps = crate::helper::in_servers(server);
            if let Err(err) = self.helper.chown_tree(&owner.to_string(), steps).await {
                tracing::warn!(%server, "the tree was not handed back before deleting: {err:#}");
            }
            remove_tree(doomed).await?;
        }
        if !keep_backups {
            remove_tree(self.backups_dir(server)).await?;
        }

        self.operations.finish(id).await.map_err(internal_error)?;
        sqlx::query("DELETE FROM servers WHERE id = ?")
            .bind(server)
            .execute(&self.pool)
            .await
            .map_err(|err| internal_error(Failure::from(err)))?;
        self.hub.forget_token(&server.to_string()).await;
        self.operations.bus().forget(server);
        tracing::info!(%server, "a server was deleted");
        Ok(())
    }

    async fn input<T: serde::de::DeserializeOwned>(
        &self,
        operation: Id,
    ) -> std::result::Result<T, OperationError> {
        let raw: Option<String> = sqlx::query_scalar("SELECT input FROM operations WHERE id = ?")
            .bind(operation)
            .fetch_optional(&self.pool)
            .await
            .map_err(|err| internal_error(Failure::from(err)))?
            .flatten();
        let text = raw.unwrap_or_else(|| "null".to_owned());
        serde_json::from_str(&text).map_err(|err| {
            internal_error(Failure::internal(anyhow::anyhow!("unreadable run input: {err}")))
        })
    }

    async fn setting(&self, server: Id) -> std::result::Result<(Id, u32, u16), OperationError> {
        let row: Option<(Id, u32, Option<u16>)> = sqlx::query_as(
            "SELECT s.owner_id, s.memory_mib,
                    (SELECT port FROM allocations a WHERE a.server_id = s.id AND a.is_primary = 1)
               FROM servers s WHERE s.id = ?",
        )
        .bind(server)
        .fetch_optional(&self.pool)
        .await
        .map_err(|err| internal_error(Failure::from(err)))?;

        let (owner, memory, port) = row.ok_or_else(|| OperationError {
            code: "loader_install_failed".to_owned(),
            message: "internal error".to_owned(),
            step: OperationErrorStep::Internal,
        })?;
        Ok((owner, memory, port.unwrap_or_default()))
    }

    async fn step(
        &self,
        id: Id,
        phase: OperationPhase,
        progress: f64,
        file: Option<String>,
        cancellable: Option<bool>,
    ) {
        let step = Step {
            phase: Some(phase),
            progress: Some(progress),
            current_file: file,
            cancellable,
            ..Step::default()
        };
        if let Err(failure) = self.operations.advance(id, step).await {
            tracing::warn!("a step went missing: {}", failure.message());
        }
    }

    async fn called_off(&self, id: Id) -> bool {
        self.operations.cancel_requested(id).await.unwrap_or(false)
    }

    async fn give_up(&self, id: Id, work: &Path) -> std::result::Result<(), OperationError> {
        let _ = tokio::fs::remove_dir_all(work).await;
        let operation = self.operations.cancelled(id).await.map_err(internal_error)?;
        self.mark_broken(operation.server_id).await;
        Ok(())
    }

    async fn mark_broken(&self, server: Id) {
        let written = sqlx::query(
            "UPDATE servers SET status = 'broken', flows_intro = 1, updated_at = ?
              WHERE id = ? AND status <> 'deleting'",
        )
        .bind(Timestamp::now())
        .bind(server)
        .execute(&self.pool)
        .await;
        if let Err(err) = written {
            tracing::error!("a broken server could not be marked: {err}");
            return;
        }
        if let Ok(object) = self.read(server, Permissions::NONE).await {
            self.announce(&object);
        }
    }

    pub async fn power(
        self: &Arc<Self>,
        caller: &Caller,
        server: Id,
        action: PowerAction,
    ) -> Result<(PowerState, Option<PowerTarget>)> {
        let gate = self.gate(server);
        let _held = gate.lock().await;

        self.operations.guard_write(server).await.map_err(fault)?;

        let row = self.power_row(server).await?;
        let now = self.current(server).await?;
        let allowed = match action {
            PowerAction::Start => now.can_become(RunState::Starting),
            PowerAction::Stop | PowerAction::Restart => now.can_become(RunState::Stopping),
            PowerAction::Kill => now.is_live(),
        };
        if !allowed {
            return Err(Failure::conflict(
                "invalid_power_transition",
                format!("the server is {} and cannot {}", wire(now).as_str(), action.as_str()),
            ));
        }

        Ok(match action {
            PowerAction::Start => {
                self.start(Some(caller.id()), &row).await?;
                (PowerState::Starting, Some(PowerTarget::Start))
            }
            PowerAction::Stop => {
                self.ask_to_stop(Some(caller.id()), server, PowerTarget::Stop).await?;
                (PowerState::Stopping, Some(PowerTarget::Stop))
            }
            PowerAction::Restart => {
                self.ask_to_stop(Some(caller.id()), server, PowerTarget::Restart).await?;
                (PowerState::Stopping, Some(PowerTarget::Restart))
            }
            PowerAction::Kill => {
                self.kill(Some(caller.id()), server).await?;
                (PowerState::Stopping, None)
            }
        })
    }

    async fn power_row(&self, server: Id) -> Result<PowerRow> {
        sqlx::query_as::<_, PowerRow>(
            "SELECT id, owner_id, status, memory_mib, loader, java_major, extra_flags
               FROM servers WHERE id = ?",
        )
        .bind(server)
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(unknown_server)
    }

    async fn start(self: &Arc<Self>, actor: Option<Id>, row: &PowerRow) -> Result<()> {
        if row.status == ServerStatus::Broken {
            return Err(Failure::conflict("server_broken", "this server is not set up"));
        }
        if row.status != ServerStatus::Available {
            return Err(Failure::conflict("server_busy", "the server is not ready yet"));
        }

        let owner = users::load(&self.pool, row.owner_id).await?;
        if owner.system_state != SystemUserState::Ready {
            return Err(Failure::conflict(
                "system_user_not_ready",
                "the system account of this owner is not ready",
            ));
        }
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let allocated = allocated_mib(&mut tx, owner.id).await?;
        tx.commit().await?;
        if owner.budget().exceeded_by(allocated) {
            return Err(Failure::conflict(
                "over_limit",
                "the owner is over the memory he was given",
            ));
        }

        let token = hex::encode(rand::random::<[u8; 32]>());
        sqlx::query(
            "UPDATE servers SET supervisor_token = ?, power_state = 'starting',
                                running_since = NULL, exit_code = NULL, oom_killed = 0,
                                updated_at = ? WHERE id = ?",
        )
        .bind(&token)
        .bind(Timestamp::now())
        .bind(row.id)
        .execute(&self.pool)
        .await?;
        self.hub.set_token(row.id.to_string(), token.clone()).await;

        let request = SpawnRequest {
            user_id: row.owner_id.to_string(),
            server_id: row.id.to_string(),
            working_dir: crate::helper::in_servers(row.id),
            program: self.java(row.java_major)?,
            args: argv(row.loader, row.memory_mib, &row.flags()),
            env: Vec::new(),
            supervisor_socket: self.hub.socket().to_path_buf(),
            token,
        };
        self.set_wish(
            row.id,
            Wish { target: Some(PowerTarget::Start), killed: false, undelivered: false },
        );
        self.write_state(row.id, PowerState::Starting, false).await?;
        self.report(row.id, PowerState::Starting, Some(PowerTarget::Start), false).await;

        if let Err(err) = self.helper.spawn(request).await {
            self.set_wish(row.id, Wish::default());
            self.write_state(row.id, PowerState::Stopped, false).await?;
            self.report(row.id, PowerState::Stopped, None, false).await;
            return Err(Failure::internal(err));
        }

        if let Some(actor) = actor {
            self.note(row.id, actor, AuditAction::ServerStarted, None).await?;
        }
        self.watch(row.id);
        Ok(())
    }

    async fn ask_to_stop(
        self: &Arc<Self>,
        actor: Option<Id>,
        server: Id,
        target: PowerTarget,
    ) -> Result<()> {
        let undelivered = match self.hub.link(&server.to_string()).await {
            Some(link) => {
                let grace = self.stop_grace().await?;
                link.request_stop(Some("stop".to_owned()), grace)
                    .await
                    .map_err(Failure::internal)?;
                false
            }
            None if self.expects_a_supervisor(server).await? => true,
            None => {
                self.write_state(server, PowerState::Stopped, false).await?;
                self.report(server, PowerState::Stopped, None, false).await;
                return Err(Failure::conflict(
                    "invalid_power_transition",
                    "the server is stopped and cannot stop",
                ));
            }
        };

        self.set_wish(server, Wish { target: Some(target), killed: false, undelivered });
        self.write_state(server, PowerState::Stopping, false).await?;
        self.report(server, PowerState::Stopping, Some(target), false).await;
        if let Some(actor) = actor {
            let action = match target {
                PowerTarget::Restart => AuditAction::ServerRestarted,
                _ => AuditAction::ServerStopped,
            };
            self.note(server, actor, action, None).await?;
        }
        self.watch(server);
        Ok(())
    }

    async fn kill(self: &Arc<Self>, actor: Option<Id>, server: Id) -> Result<()> {
        let undelivered = match self.hub.link(&server.to_string()).await {
            Some(link) => {
                link.kill().await.map_err(Failure::internal)?;
                false
            }
            None => self.expects_a_supervisor(server).await?,
        };
        self.set_wish(server, Wish { target: None, killed: true, undelivered });
        self.write_state(server, PowerState::Stopping, false).await?;
        self.report(server, PowerState::Stopping, None, false).await;
        if let Some(actor) = actor {
            self.note(server, actor, AuditAction::ServerKilled, None).await?;
        }
        self.watch(server);
        Ok(())
    }

    pub async fn current(&self, server: Id) -> Result<RunState> {
        if let Some(link) = self.hub.link(&server.to_string()).await {
            return Ok(link.state().await);
        }
        let row: Option<(PowerState, bool)> =
            sqlx::query_as("SELECT power_state, oom_killed FROM servers WHERE id = ?")
                .bind(server)
                .fetch_optional(&self.pool)
                .await?;
        let (state, oom) = row.ok_or_else(unknown_server)?;
        Ok(match state {
            PowerState::Stopped => RunState::Stopped,
            PowerState::Starting => RunState::Starting,
            PowerState::Running => RunState::Running,
            PowerState::Stopping => RunState::Stopping,
            PowerState::Crashed if oom => RunState::OutOfMemory,
            PowerState::Crashed => RunState::Crashed,
        })
    }

    async fn stop_grace(&self) -> Result<u32> {
        let grace: Option<u32> =
            sqlx::query_scalar("SELECT stop_grace_seconds FROM panel_settings WHERE id = 1")
                .fetch_optional(&self.pool)
                .await?;
        Ok(grace.unwrap_or(60))
    }

    fn java(&self, major: Option<u32>) -> Result<PathBuf> {
        let Some(major) = major else {
            return Ok(system_java());
        };

        let managed = self
            .config
            .data_dir
            .join("runtimes")
            .join(format!("java-{major}"))
            .join("bin")
            .join("java");
        if managed.exists() {
            return Ok(managed);
        }

        if let Some(found) = installed_java(major) {
            return Ok(found);
        }

        if let Some(newer) = newest_java_at_least(major) {
            return Ok(newer);
        }

        Err(Failure::conflict(
            "java_runtime_missing",
            format!(
                "this version needs Java {major} and no such runtime is installed \
                 (try: apt install openjdk-{major}-jre-headless)"
            ),
        ))
    }

    fn gate(&self, server: Id) -> Arc<tokio::sync::Mutex<()>> {
        let mut gates = self.gates.lock().expect("the power gate");
        Arc::clone(gates.entry(server).or_default())
    }

    fn wish(&self, server: Id) -> Wish {
        self.wishes.lock().expect("the wish list").get(&server).copied().unwrap_or_default()
    }

    fn set_wish(&self, server: Id, wish: Wish) {
        self.wishes.lock().expect("the wish list").insert(server, wish);
    }

    async fn write_state(&self, server: Id, state: PowerState, oom: bool) -> Result<()> {
        sqlx::query(
            "UPDATE servers
                SET power_state = ?,
                    running_since = CASE WHEN ? = 'running' THEN coalesce(running_since, ?)
                                         ELSE NULL END,
                    oom_killed = ?, updated_at = ?
              WHERE id = ?",
        )
        .bind(state)
        .bind(state)
        .bind(Timestamp::now())
        .bind(oom)
        .bind(Timestamp::now())
        .bind(server)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn report(
        &self,
        server: Id,
        state: PowerState,
        target: Option<PowerTarget>,
        oom: bool,
    ) {
        let uptime = if state == PowerState::Running {
            let since: Option<Option<Timestamp>> =
                sqlx::query_scalar("SELECT running_since FROM servers WHERE id = ?")
                    .bind(server)
                    .fetch_optional(&self.pool)
                    .await
                    .ok()
                    .flatten();
            since
                .flatten()
                .map_or(0, |since| (Timestamp::now().unix_seconds() - since.unix_seconds()).max(0)
                    as u64)
        } else {
            0
        };
        let report = StateReport {
            power_state: state,
            target,
            uptime_seconds: uptime,
            exit_code: None,
            oom_killed: oom,
        };
        self.operations.bus().channel(server).set_state(report);
    }

    fn watch(self: &Arc<Self>, server: Id) {
        if !self.watched.lock().expect("the watch list").insert(server) {
            return;
        }
        let manager = Arc::clone(self);
        tokio::spawn(async move {
            manager.attend(server).await;
            manager.watched.lock().expect("the watch list").remove(&server);
        });
    }

    async fn attend(self: &Arc<Self>, server: Id) {
        let name = server.to_string();
        let waiting_since = Instant::now();
        let mut last = self.current(server).await.unwrap_or(RunState::Stopped);

        loop {
            tokio::time::sleep(WATCH_TICK).await;
            match self.hub.link(&name).await {
                Some(link) => {
                    if self.wish(server).undelivered {
                        self.hand_over(server, &link).await;
                    }
                    let now = link.state().await;
                    if now != last {
                        last = now;
                        let (state, oom) = crate::ops::power_state_of(now);
                        let _ = self.write_state(server, state, oom).await;
                        if !now.is_live() {
                            break;
                        }
                    }
                }
                None if last == RunState::Starting
                    && waiting_since.elapsed() < START_TIMEOUT => {}
                None => break,
            }
        }

        self.settle(server, last).await;
    }

    async fn expects_a_supervisor(&self, server: Id) -> Result<bool> {
        if self.wish(server).undelivered {
            return Ok(true);
        }
        let state: Option<PowerState> =
            sqlx::query_scalar("SELECT power_state FROM servers WHERE id = ?")
                .bind(server)
                .fetch_optional(&self.pool)
                .await?;
        Ok(state == Some(PowerState::Starting))
    }

    async fn hand_over(&self, server: Id, link: &super::Link) {
        let wish = self.wish(server);
        let given = if wish.killed {
            link.kill().await
        } else {
            match self.stop_grace().await {
                Ok(grace) => link.request_stop(Some("stop".to_owned()), grace).await,
                Err(failure) => Err(anyhow::anyhow!("{failure}")),
            }
        };
        match given {
            Ok(()) => self.set_wish(server, Wish { undelivered: false, ..wish }),
            Err(err) => {
                tracing::warn!(%server, "a waiting power wish could not be handed over: {err:#}");
            }
        }
    }

    async fn settle(self: &Arc<Self>, server: Id, last: RunState) {
        let gate = self.gate(server);
        let _held = gate.lock().await;
        let wish = self.wish(server);
        let stopping_on_purpose =
            matches!(wish.target, Some(PowerTarget::Stop | PowerTarget::Restart));
        let (state, oom) = match last {
            _ if wish.killed => (PowerState::Stopped, false),
            RunState::OutOfMemory => (PowerState::Crashed, true),
            _ if stopping_on_purpose => (PowerState::Stopped, false),
            RunState::Crashed => (PowerState::Crashed, false),
            RunState::Stopped | RunState::Installing => (PowerState::Stopped, false),
            _ => (PowerState::Crashed, false),
        };

        let _ = self.write_state(server, state, oom).await;
        self.hub.forget_token(&server.to_string()).await;
        self.report(server, state, None, oom).await;
        self.set_wish(server, Wish::default());

        if wish.target == Some(PowerTarget::Restart) {
            if let Err(failure) = self.start_again(server).await {
                tracing::warn!(%server, "the second half of a restart failed: {failure}");
            }
        }
    }

    async fn start_again(self: &Arc<Self>, server: Id) -> Result<()> {
        let row = self.power_row(server).await?;
        self.start(None, &row).await
    }

    pub async fn adopt_tokens(&self) -> Result<usize> {
        let rows: Vec<(Id, String)> = sqlx::query_as(
            "SELECT id, supervisor_token FROM servers WHERE supervisor_token IS NOT NULL",
        )
        .fetch_all(&self.pool)
        .await?;
        let count = rows.len();
        self.hub.load_tokens(rows.into_iter().map(|(id, token)| (id.to_string(), token))).await;
        Ok(count)
    }

    pub async fn reconcile(self: &Arc<Self>) -> Result<Reconciliation> {
        let attached: BTreeSet<Id> =
            self.hub.attached().await.into_iter().filter_map(|name| name.parse().ok()).collect();

        let mut found = Reconciliation::default();
        let claiming: Vec<(Id,)> = sqlx::query_as(
            "SELECT id FROM servers WHERE power_state IN ('starting', 'running', 'stopping')",
        )
        .fetch_all(&self.pool)
        .await?;

        for (server,) in claiming {
            if attached.contains(&server) {
                found.attached.push(server);
                self.watch(server);
                continue;
            }
            self.write_state(server, PowerState::Stopped, false).await?;
            sqlx::query("UPDATE servers SET supervisor_token = NULL WHERE id = ?")
                .bind(server)
                .execute(&self.pool)
                .await?;
            self.hub.forget_token(&server.to_string()).await;
            self.report(server, PowerState::Stopped, None, false).await;
            found.cleared.push(server);
        }

        let half_built: Vec<(Id,)> = sqlx::query_as(
            "SELECT id FROM servers
              WHERE status = 'installing'
                AND id NOT IN (SELECT server_id FROM operations
                                WHERE state IN ('queued', 'ongoing'))",
        )
        .fetch_all(&self.pool)
        .await?;
        for (server,) in half_built {
            self.mark_broken(server).await;
            found.broken.push(server);
        }

        found.resumed = self.resume_deletes().await?;

        tracing::info!(
            attached = found.attached.len(),
            cleared = found.cleared.len(),
            broken = found.broken.len(),
            resumed = found.resumed.len(),
            "servers reconciled"
        );
        Ok(found)
    }

    async fn resume_deletes(self: &Arc<Self>) -> Result<Vec<Id>> {
        let open: Vec<(Id, OperationState)> = sqlx::query_as(
            "SELECT id, state FROM operations
              WHERE kind = 'server_delete' AND state IN ('queued', 'ongoing') ORDER BY id",
        )
        .fetch_all(&self.pool)
        .await?;

        let mut resumed = Vec::new();
        for (id, state) in open {
            if state == OperationState::Queued {
                self.run(id).await;
            } else {
                let Ok(operation) = self.operations.get(id).await else { continue };
                if let Err(error) = self.remove(&operation).await {
                    let _ = self.operations.fail(id, error).await;
                }
            }
            resumed.push(id);
        }
        Ok(resumed)
    }

    pub fn spawn_recovery(self: &Arc<Self>) -> tokio::task::JoinHandle<()> {
        let manager = Arc::clone(self);
        tokio::spawn(async move {
            if let Err(failure) = manager.adopt_tokens().await {
                tracing::error!("the supervisor tokens could not be read back: {failure}");
            }
            tokio::time::sleep(REATTACH_GRACE).await;
            if let Err(failure) = manager.reconcile().await {
                tracing::error!("the servers could not be reconciled: {failure}");
            }
        })
    }

    pub async fn sample(&self, server: Id) -> Result<StatsSample> {
        let row: Option<(Id, u32)> =
            sqlx::query_as("SELECT owner_id, memory_mib FROM servers WHERE id = ?")
                .bind(server)
                .fetch_optional(&self.pool)
                .await?;
        let (owner, memory_mib) = row.ok_or_else(unknown_server)?;
        let quota = users::find(&self.pool, owner)
            .await?
            .and_then(|user| user.budget().cpu_cores())
            .unwrap_or_else(|| f64::from(crate::auth::usage::host().cpu_cores));
        let pid = self.hub.link(&server.to_string()).await.map(|link| link.pid);

        let ticks = pid.and_then(cpu_ticks);
        let ram_usage_bytes = pid.map_or(0, rss_bytes);

        let (cpu_percent, cached) = {
            let mut meters = self.meters.lock().expect("the meter");
            let meter = meters.entry(server).or_default();
            (meter.cpu(ticks, quota.max(0.01)), meter.storage)
        };

        let storage_usage_bytes = match cached {
            Some((measured, bytes)) if measured.elapsed() < STORAGE_EVERY => bytes,
            _ => {
                let dir = self.dir(owner, server);
                let bytes = tokio::task::spawn_blocking(move || crate::files::tree_size(&dir))
                    .await
                    .unwrap_or(0);
                self.meters.lock().expect("the meter").entry(server).or_default().storage =
                    Some((Instant::now(), bytes));
                bytes
            }
        };

        Ok(StatsSample {
            cpu_percent,
            ram_usage_bytes,
            ram_total_bytes: u64::from(memory_mib) * MIB,
            storage_usage_bytes,
            storage_total_bytes: crate::files::filesystem_total_bytes(&self.config.data_dir),
        })
    }

    pub fn spawn_metrics(self: &Arc<Self>) -> tokio::task::JoinHandle<()> {
        let manager = Arc::clone(self);
        tokio::spawn(async move {
            let mut slow = Instant::now() - IDLE_SAMPLE_EVERY;
            loop {
                tokio::time::sleep(SAMPLE_TICK).await;
                let idle_turn = slow.elapsed() >= IDLE_SAMPLE_EVERY;
                if idle_turn {
                    slow = Instant::now();
                }

                let running: BTreeSet<Id> = manager
                    .hub
                    .attached()
                    .await
                    .into_iter()
                    .filter_map(|name| name.parse().ok())
                    .collect();
                let servers: Vec<Id> = if idle_turn {
                    manager.operations.bus().servers()
                } else {
                    running.iter().copied().collect()
                };

                for server in servers {
                    let channel = manager.operations.bus().channel(server);
                    if channel.listeners() == 0 {
                        continue;
                    }
                    match manager.sample(server).await {
                        Ok(sample) => channel.stats(sample),
                        Err(failure) => tracing::debug!(%server, "no sample: {failure}"),
                    }
                }
            }
        })
    }

    async fn note(
        &self,
        server: Id,
        actor: Id,
        action: AuditAction,
        metadata: Option<serde_json::Value>,
    ) -> Result<()> {
        sqlx::query(
            "INSERT INTO audit_log (id, server_id, actor_user_id, action, metadata, created_at)
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(Id::new())
        .bind(server)
        .bind(actor)
        .bind(action)
        .bind(metadata.map(|value| value.to_string()))
        .bind(Timestamp::now())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub fn confined(&self, path: &Path) -> Result<Option<PathBuf>> {
        let root = match self.config.users_dir().canonicalize() {
            Ok(root) => root,
            Err(_) => return Ok(None),
        };
        let resolved = match path.canonicalize() {
            Ok(resolved) => resolved,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(err) => return Err(Failure::internal(anyhow::Error::from(err))),
        };
        if !resolved.starts_with(&root) {
            return Err(Failure::new(
                axum::http::StatusCode::FORBIDDEN,
                "forbidden_path",
                format!("{} is outside {}", resolved.display(), root.display()),
            ));
        }
        Ok(Some(resolved))
    }
}

const IN_VIEW: &str = "(s.status <> 'deleting'
              OR NOT EXISTS (SELECT 1 FROM operations o
                              WHERE o.server_id = s.id AND o.kind = 'server_delete'
                                AND o.state IN ('queued', 'ongoing')))";

const SELECT: &str = "SELECT s.id, s.name, s.owner_id, s.status, s.loader, s.loader_version,
            s.game_version, s.memory_mib, s.update_channel, s.flows_intro, s.created_at,
            (SELECT port FROM allocations a WHERE a.server_id = s.id AND a.is_primary = 1)
                AS port,
            m.project_id AS project_id, m.version_id AS version_id,
            (SELECT count(*) FROM backups b WHERE b.server_id = s.id) AS used_backup_quota,
            p.public_address AS public_address,
            p.max_backups_per_server AS backup_quota
       FROM servers s
       LEFT JOIN server_modpacks m ON m.server_id = s.id
       CROSS JOIN panel_settings p";

#[derive(sqlx::FromRow)]
struct Row {
    id: Id,
    name: String,
    owner_id: Id,
    status: ServerStatus,
    loader: Option<LoaderId>,
    loader_version: Option<String>,
    game_version: Option<String>,
    memory_mib: u32,
    update_channel: UpdateChannel,
    flows_intro: bool,
    created_at: Timestamp,
    port: Option<u16>,
    project_id: Option<String>,
    version_id: Option<String>,
    used_backup_quota: u32,
    public_address: Option<String>,
    backup_quota: u32,
}

impl Row {
    fn into_server(self, mask: Permissions) -> Server {
        Server {
            id: self.id,
            name: self.name,
            owner_id: self.owner_id,
            status: self.status,
            game: Minecraft,
            loader: self.loader,
            loader_version: self.loader_version,
            game_version: self.game_version,
            net: ServerNet {
                ip: self.public_address,
                port: self.port.unwrap_or_default(),
                domain: String::new(),
            },
            memory_mib: self.memory_mib,
            upstream: match (self.project_id, self.version_id) {
                (Some(project_id), Some(version_id)) => {
                    Some(ServerUpstream::Modpack { project_id, version_id })
                }
                _ => None,
            },
            flows: ServerFlows { intro: self.flows_intro },
            backup_quota: self.backup_quota,
            used_backup_quota: self.used_backup_quota,
            update_channel: self.update_channel,
            current_user_permissions: mask,
            created_at: self.created_at,
        }
    }
}

#[derive(sqlx::FromRow)]
struct PowerRow {
    id: Id,
    owner_id: Id,
    status: ServerStatus,
    memory_mib: u32,
    loader: Option<LoaderId>,
    java_major: Option<u32>,
    extra_flags: String,
}

impl PowerRow {
    fn flags(&self) -> Vec<String> {
        serde_json::from_str(&self.extra_flags).unwrap_or_default()
    }
}

fn check_name(name: &str) -> Result<()> {
    if name.trim().is_empty() || name.chars().count() > NAME_LIMIT {
        return Err(Failure::invalid_request("a name is 1 to 64 characters"));
    }
    if name.chars().any(char::is_control) {
        return Err(Failure::invalid_request("a name carries no control characters"));
    }
    Ok(())
}

async fn allocated_mib(tx: &mut Transaction<'_, Sqlite>, owner: Id) -> Result<u32> {
    let (sum,): (i64,) = sqlx::query_as(
        "SELECT coalesce(sum(memory_mib), 0) FROM servers
          WHERE owner_id = ? AND status <> 'deleting'",
    )
    .bind(owner)
    .fetch_one(&mut **tx)
    .await?;
    Ok(sum.max(0) as u32)
}

async fn next_free_port(tx: &mut Transaction<'_, Sqlite>) -> Result<u16> {
    let (from, to): (u16, u16) =
        sqlx::query_as("SELECT port_pool_from, port_pool_to FROM panel_settings WHERE id = 1")
            .fetch_one(&mut **tx)
            .await?;
    let taken: Vec<(u16,)> =
        sqlx::query_as("SELECT port FROM allocations WHERE port BETWEEN ? AND ? ORDER BY port")
            .bind(from)
            .bind(to)
            .fetch_all(&mut **tx)
            .await?;

    let mut used = taken.into_iter().map(|(port,)| port);
    let mut next = used.next();
    for candidate in from..=to {
        match next {
            Some(port) if port == candidate => next = used.next(),
            _ => return Ok(candidate),
        }
    }
    Err(Failure::conflict("port_pool_exhausted", "the port pool has nothing free left"))
}

fn taken_port(err: sqlx::Error) -> Failure {
    if let sqlx::Error::Database(db) = &err {
        if db.is_unique_violation() {
            return Failure::conflict("port_in_use", "that port belongs to another server");
        }
    }
    Failure::internal(anyhow::Error::from(err))
}

async fn note_in(
    tx: &mut Transaction<'_, Sqlite>,
    server: Id,
    actor: Id,
    action: AuditAction,
    metadata: Option<serde_json::Value>,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO audit_log (id, server_id, actor_user_id, action, metadata, created_at)
         VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(Id::new())
    .bind(server)
    .bind(actor)
    .bind(action)
    .bind(metadata.map(|value| value.to_string()))
    .bind(Timestamp::now())
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn remove_tree(path: PathBuf) -> std::result::Result<(), OperationError> {
    tokio::task::spawn_blocking(move || std::fs::remove_dir_all(path))
        .await
        .map_err(|err| internal_error(Failure::internal(anyhow::Error::from(err))))?
        .or_else(|err| {
            if err.kind() == std::io::ErrorKind::NotFound {
                Ok(())
            } else {
                Err(err)
            }
        })
        .map_err(removal)
}

fn unknown_server() -> Failure {
    Failure::not_found("server_not_found", "no such server")
}

fn fault(fault: crate::ops::Fault) -> Failure {
    Failure::new(fault.status(), fault.code(), fault.message())
}

fn wire(state: RunState) -> PowerState {
    crate::ops::power_state_of(state).0
}

fn argv(loader: Option<LoaderId>, memory_mib: u32, extra: &[String]) -> Vec<String> {
    let mut args = vec![format!("-Xmx{memory_mib}M")];
    for flag in extra {
        let owned = ["-Xmx", "-Xms", "-XX:MaxRAM", "-XX:MaxHeapSize"];
        if owned.iter().any(|prefix| flag.starts_with(prefix)) {
            continue;
        }
        args.push(flag.clone());
    }
    args.push("-jar".to_owned());
    args.push(JAR.to_owned());
    if loader != Some(LoaderId::Velocity) {
        args.push("nogui".to_owned());
    }
    args
}

fn eula_text() -> String {
    "# Mojang's EULA was accepted in the panel when this server was made.\neula=true\n".to_owned()
}

fn properties_text(fields: &PropertiesFields, port: u16) -> String {
    let mut lines: BTreeMap<String, String> = BTreeMap::new();
    for key in KNOWN_PROPERTY_KEYS {
        if let Some(value) = fields.known.get(key) {
            lines.insert(key.replace('_', "-"), clean(value));
        }
    }
    if let Some(custom) = &fields.custom {
        for (key, value) in custom {
            if !key.is_empty()
                && key.chars().all(|c| c.is_ascii_alphanumeric() || "._-".contains(c))
            {
                lines.insert(key.clone(), clean(value));
            }
        }
    }
    lines.insert("server-port".to_owned(), port.to_string());
    lines.insert("query.port".to_owned(), port.to_string());

    let mut text = String::from("# Written by the panel; server-port belongs to it.\n");
    for (key, value) in lines {
        text.push_str(&format!("{key}={value}\n"));
    }
    text
}

fn clean(value: &str) -> String {
    value.chars().filter(|letter| !letter.is_control()).collect()
}

fn refusal(err: LoaderError) -> Failure {
    use axum::http::StatusCode;
    match err {
        LoaderError::UnknownVersion { .. } | LoaderError::NoBuild { .. } => Failure::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "unsupported_game_version",
            err.to_string(),
        ),
        LoaderError::UnknownBuild { .. } => Failure::not_found("build_not_found", err.to_string()),
        LoaderError::Unreachable { .. }
        | LoaderError::Refused { .. }
        | LoaderError::Unreadable { .. }
        | LoaderError::Interrupted { .. } => {
            Failure::new(StatusCode::BAD_GATEWAY, "upstream_unavailable", err.to_string())
        }
        other => Failure::internal(anyhow::anyhow!("{other}")),
    }
}

fn loader_failure(err: LoaderError) -> OperationError {
    let (code, step, message) = match &err {
        LoaderError::UnknownVersion { .. } | LoaderError::NoBuild { .. } => (
            "unsupported_game_version",
            OperationErrorStep::Modloader,
            "this version is not yet supported".to_owned(),
        ),
        LoaderError::UnknownBuild { .. } => (
            "invalid_version",
            OperationErrorStep::Modloader,
            "the specified version may be incorrect".to_owned(),
        ),
        LoaderError::Damaged { .. } => {
            ("checksum_mismatch", OperationErrorStep::Download, err.to_string())
        }
        LoaderError::Unreachable { .. }
        | LoaderError::Refused { .. }
        | LoaderError::Unreadable { .. }
        | LoaderError::Interrupted { .. } => {
            ("upstream_unavailable", OperationErrorStep::Download, err.to_string())
        }
        LoaderError::Write { reason, .. } if reason.contains("No space left") => {
            ("no_space", OperationErrorStep::Filesystem, err.to_string())
        }
        _ => ("loader_install_failed", OperationErrorStep::Modloader, "internal error".to_owned()),
    };
    OperationError { code: code.to_owned(), message, step }
}

fn named_io(err: &std::io::Error) -> Option<&'static str> {
    match err.raw_os_error() {
        Some(libc::ENOSPC | libc::EDQUOT) => Some("no_space"),
        Some(libc::EACCES | libc::EPERM) => Some("permission_denied"),
        _ => None,
    }
}

fn disk(err: std::io::Error) -> OperationError {
    match named_io(&err) {
        Some(code) => OperationError {
            code: code.to_owned(),
            message: err.to_string(),
            step: OperationErrorStep::Filesystem,
        },
        None => {
            tracing::warn!("a set-up hit the filesystem: {err}");
            OperationError {
                code: "loader_install_failed".to_owned(),
                message: "internal error".to_owned(),
                step: OperationErrorStep::Modloader,
            }
        }
    }
}

fn removal(err: std::io::Error) -> OperationError {
    OperationError {
        code: named_io(&err).unwrap_or("delete_failed").to_owned(),
        message: err.to_string(),
        step: OperationErrorStep::Filesystem,
    }
}

fn internal_error(failure: impl std::fmt::Debug) -> OperationError {
    tracing::error!("{failure:?}");
    OperationError {
        code: "loader_install_failed".to_owned(),
        message: "internal error".to_owned(),
        step: OperationErrorStep::Internal,
    }
}

#[derive(Debug, Default, Clone, Copy)]
struct Meter {
    cpu: Option<(Instant, u64)>,
    storage: Option<(Instant, u64)>,
}

impl Meter {
    fn cpu(&mut self, ticks: Option<u64>, quota_cores: f64) -> f64 {
        let Some(ticks) = ticks else {
            self.cpu = None;
            return 0.0;
        };
        let now = Instant::now();
        let percent = match self.cpu {
            Some((then, before)) => {
                let seconds = now.duration_since(then).as_secs_f64();
                if seconds <= 0.0 {
                    0.0
                } else {
                    let spent = ticks.saturating_sub(before) as f64 / USER_HZ;
                    (spent / seconds / quota_cores * 100.0).max(0.0)
                }
            }
            None => 0.0,
        };
        self.cpu = Some((now, ticks));
        percent
    }
}

fn cpu_ticks(pid: u32) -> Option<u64> {
    let mut total = 0;
    let mut seen = false;
    for pid in tree(pid) {
        if let Some(ticks) = own_ticks(pid) {
            total += ticks;
            seen = true;
        }
    }
    seen.then_some(total)
}

fn own_ticks(pid: u32) -> Option<u64> {
    let text = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let fields: Vec<&str> = text.rsplit_once(')')?.1.split_whitespace().collect();
    let utime: u64 = fields.get(11)?.parse().ok()?;
    let stime: u64 = fields.get(12)?.parse().ok()?;
    Some(utime + stime)
}

fn rss_bytes(pid: u32) -> u64 {
    tree(pid)
        .into_iter()
        .filter_map(|pid| {
            let text = std::fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
            let line = text.lines().find_map(|line| line.strip_prefix("VmRSS:"))?;
            let kib: u64 = line.split_whitespace().next()?.parse().ok()?;
            Some(kib * 1024)
        })
        .sum()
}

fn tree(pid: u32) -> Vec<u32> {
    const CEILING: usize = 512;
    let mut found = vec![pid];
    let mut index = 0;
    while index < found.len() && found.len() < CEILING {
        let parent = found[index];
        index += 1;
        let Ok(tasks) = std::fs::read_dir(format!("/proc/{parent}/task")) else { continue };
        for task in tasks.flatten() {
            let Ok(children) = std::fs::read_to_string(task.path().join("children")) else {
                continue;
            };
            for child in children.split_whitespace().filter_map(|pid| pid.parse().ok()) {
                if !found.contains(&child) {
                    found.push(child);
                }
            }
        }
    }
    found
}

#[cfg(test)]
pub(crate) mod fake {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    pub struct Shelf {
        build: Build,
        refuse: Option<LoaderError>,
        fetched: AtomicUsize,
    }

    impl Shelf {
        pub fn new() -> Arc<Self> {
            Arc::new(Self { build: a_build(Some(9)), refuse: None, fetched: AtomicUsize::new(0) })
        }

        pub fn refusing(error: LoaderError) -> Arc<Self> {
            Arc::new(Self {
                build: a_build(None),
                refuse: Some(error),
                fetched: AtomicUsize::new(0),
            })
        }

        pub fn fetched(&self) -> usize {
            self.fetched.load(Ordering::Relaxed)
        }
    }

    fn a_build(size: Option<u64>) -> Build {
        Build {
            id: "45".to_owned(),
            channel: Channel::Stable,
            url: "https://example.invalid/paper-45.jar".to_owned(),
            filename: "paper-1.21.8-45.jar".to_owned(),
            checksum: None,
            size,
            java_major: None,
        }
    }

    impl Builds for Shelf {
        fn resolve<'a>(
            &'a self,
            _loader: Loader,
            _game_version: &'a str,
            _wanted: Wanted,
        ) -> BoxFuture<'a, std::result::Result<Build, LoaderError>> {
            Box::pin(async move { Ok(self.build.clone()) })
        }

        fn fetch<'a>(
            &'a self,
            _loader: Loader,
            _build: &'a Build,
            dest: &'a Path,
        ) -> BoxFuture<'a, std::result::Result<u64, LoaderError>> {
            Box::pin(async move {
                if let Some(refusal) = &self.refuse {
                    return Err(again(refusal));
                }
                self.fetched.fetch_add(1, Ordering::Relaxed);
                tokio::fs::write(dest, b"not a jar")
                    .await
                    .map_err(|err| LoaderError::write(dest, err))?;
                Ok(9)
            })
        }
    }

    fn again(error: &LoaderError) -> LoaderError {
        match error {
            LoaderError::Unreachable { service, reason } => {
                LoaderError::Unreachable { service, reason: reason.clone() }
            }
            other => LoaderError::Unreachable { service: "test", reason: other.to_string() },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fake::Shelf;
    use super::*;
    use crate::auth::harness::{a_user, an_admin, insert_user, sign_in, FakeHelper, PASSWORD};
    use crate::auth::session;
    use crate::model::{KnownProperties, OperationState, PanelRole};
    use crate::ops::testing::DataDir;
    use axum::http::StatusCode;

    struct Fixture {
        manager: Arc<Manager>,
        pool: SqlitePool,
        helper: FakeHelper,
        shelf: Arc<Shelf>,
        hub: Arc<Hub>,
        dir: DataDir,
    }

    impl Fixture {
        async fn new() -> Self {
            let dir = DataDir::new();
            let pool = crate::auth::harness::test_pool().await;
            Self::with(pool, dir, Shelf::new(), Disks::none()).await
        }

        async fn with_disks(disks: Disks) -> Self {
            let dir = DataDir::new();
            let pool = crate::auth::harness::test_pool().await;
            Self::with(pool, dir, Shelf::new(), disks).await
        }

        async fn failing(error: LoaderError) -> Self {
            let dir = DataDir::new();
            let pool = crate::auth::harness::test_pool().await;
            Self::with(pool, dir, Shelf::refusing(error), Disks::none()).await
        }

        async fn with(
            pool: SqlitePool,
            dir: DataDir,
            shelf: Arc<Shelf>,
            disks: Disks,
        ) -> Self {
            let helper = FakeHelper::obliging().await.rooted_at(dir.path().join("users"));
            let config = Arc::new(Config {
                data_dir: dir.path().to_path_buf(),
                helper_socket: helper.socket(),
                ..Config::default()
            });
            let operations = Operations::new(pool.clone(), dir.path());
            let hub = Arc::new(Hub::new(dir.path().join("supervisors.sock")));
            let manager = Manager::new(
                pool.clone(),
                config,
                operations,
                Arc::clone(&hub),
                Helper::new(helper.socket()),
                Arc::clone(&shelf) as Arc<dyn Builds>,
                disks,
            );
            Self { manager, pool, helper, shelf, hub, dir }
        }

        async fn caller(&self, user: Id) -> Caller {
            let secret = sign_in(&self.pool, user).await;
            let session = session::lookup(&self.pool, &secret, Timestamp::now())
                .await
                .expect("a session")
                .expect("the session we just opened");
            Caller {
                user: users::load(&self.pool, user).await.expect("the user"),
                session,
                secure: false,
            }
        }

        fn loader_wish(&self, name: &str, owner: Id, memory_mib: u32) -> NewServer {
            NewServer {
                name: name.to_owned(),
                owner_id: owner,
                memory_mib,
                port: None,
                content: CreateContent::Loader {
                    loader: LoaderId::Paper,
                    game_version: "1.21.8".to_owned(),
                    loader_version: None,
                },
                properties: PropertiesFields::default(),
            }
        }
    }

    #[tokio::test]
    async fn the_budget_of_the_owner_is_what_lets_a_server_be_made() {
        let fixture = Fixture::new().await;
        let max = a_user(&fixture.pool, "max").await;
        let caller = fixture.caller(max).await;

        let first =
            fixture.manager.create(&caller, fixture.loader_wish("one", max, 3072)).await.unwrap();
        assert_eq!(first.server.memory_mib, 3072);
        assert!(first.warnings.is_empty());

        let refused = fixture
            .manager
            .create(&caller, fixture.loader_wish("two", max, 2048))
            .await
            .unwrap_err();
        assert_eq!(refused.code(), "budget_exceeded");
        assert_eq!(refused.status(), StatusCode::CONFLICT);

        let third =
            fixture.manager.create(&caller, fixture.loader_wish("three", max, 1024)).await;
        assert!(third.is_ok(), "3072 + 1024 is exactly the budget");
    }

    #[tokio::test]
    async fn an_admin_makes_a_second_server_past_the_number_in_his_row() {
        let fixture = Fixture::new().await;
        let anna = an_admin(&fixture.pool, "anna").await;
        let caller = fixture.caller(anna).await;
        assert_eq!(
            users::load(&fixture.pool, anna).await.unwrap().memory_mib,
            4096,
            "the row says 4096, and none of it is in force"
        );

        let first = fixture
            .manager
            .create(&caller, fixture.loader_wish("big", anna, 8192))
            .await
            .expect("twice the row is not over a budget he has not got");
        assert!(first.warnings.is_empty(), "there is no budget to overcommit");

        let second = fixture
            .manager
            .create(&caller, fixture.loader_wish("bigger", anna, 8192))
            .await
            .expect("and 16384 handed out is still not over it");
        assert!(second.warnings.is_empty());
    }

    #[tokio::test]
    async fn a_server_of_an_admin_starts_though_his_row_is_smaller() {
        let fixture = Fixture::new().await;
        let anna = an_admin(&fixture.pool, "anna").await;
        let caller = fixture.caller(anna).await;
        let made = fixture
            .manager
            .create(&caller, fixture.loader_wish("big", anna, 8192))
            .await
            .unwrap();
        fixture.manager.run(made.operation.id).await;

        let powered = fixture.manager.power(&caller, made.server.id, PowerAction::Start).await;
        assert!(powered.is_ok(), "8192 against a row of 4096: {powered:?}");
    }

    #[tokio::test]
    async fn an_admin_who_creates_for_a_user_still_gets_the_warning() {
        let fixture = Fixture::new().await;
        let anna = an_admin(&fixture.pool, "anna").await;
        let max = a_user(&fixture.pool, "max").await;
        let caller = fixture.caller(anna).await;

        let made = fixture
            .manager
            .create(&caller, fixture.loader_wish("hers", max, 8192))
            .await
            .expect("4.2: an admin may go over somebody else's budget");
        assert_eq!(made.warnings, vec![ServerWarning::MemoryOvercommitted]);
    }

    #[tokio::test]
    async fn a_full_disk_stops_the_next_server_of_that_owner() {
        let fixture = Fixture::with_disks(Disks::fixed(2048 * MIB, 0)).await;
        let max = a_user(&fixture.pool, "max").await;
        let caller = fixture.caller(max).await;
        sqlx::query("UPDATE users SET disk_mib = 1024 WHERE id = ?")
            .bind(max)
            .execute(&fixture.pool)
            .await
            .unwrap();

        let refused =
            fixture.manager.create(&caller, fixture.loader_wish("one", max, 1024)).await.unwrap_err();
        assert_eq!(refused.code(), "disk_limit_reached");
        assert_eq!(refused.status(), StatusCode::CONFLICT);

        sqlx::query("UPDATE users SET disk_mib = 4096 WHERE id = ?")
            .bind(max)
            .execute(&fixture.pool)
            .await
            .unwrap();
        assert!(
            fixture.manager.create(&caller, fixture.loader_wish("one", max, 1024)).await.is_ok(),
            "2 GiB used against 4 GiB is room"
        );
    }

    #[tokio::test]
    async fn an_owner_who_is_already_over_his_limit_cannot_make_another_one() {
        let fixture = Fixture::new().await;
        let max = a_user(&fixture.pool, "max").await;
        let caller = fixture.caller(max).await;
        fixture.manager.create(&caller, fixture.loader_wish("one", max, 4096)).await.unwrap();

        sqlx::query("UPDATE users SET memory_mib = 1024 WHERE id = ?")
            .bind(max)
            .execute(&fixture.pool)
            .await
            .unwrap();

        let refused =
            fixture.manager.create(&caller, fixture.loader_wish("two", max, 512)).await.unwrap_err();
        assert_eq!(refused.code(), "over_limit");
    }

    #[tokio::test]
    async fn a_server_on_its_way_out_no_longer_counts_against_the_budget() {
        let fixture = Fixture::new().await;
        let max = a_user(&fixture.pool, "max").await;
        let caller = fixture.caller(max).await;
        let first =
            fixture.manager.create(&caller, fixture.loader_wish("one", max, 4096)).await.unwrap();
        fixture.manager.run(first.operation.id).await;

        fixture.manager.delete(&caller, first.server.id, true).await.expect("4.5");
        let second = fixture.manager.create(&caller, fixture.loader_wish("two", max, 4096)).await;
        assert!(second.is_ok(), "4.5: the budget comes free with the start of the run");
    }

    #[tokio::test]
    async fn ports_come_out_of_the_pool_one_after_the_other() {
        let fixture = Fixture::new().await;
        let max = a_user(&fixture.pool, "max").await;
        let caller = fixture.caller(max).await;

        let first =
            fixture.manager.create(&caller, fixture.loader_wish("one", max, 512)).await.unwrap();
        let second =
            fixture.manager.create(&caller, fixture.loader_wish("two", max, 512)).await.unwrap();
        assert_eq!(first.server.net.port, 25565);
        assert_eq!(second.server.net.port, 25566);
    }

    #[tokio::test]
    async fn an_exhausted_pool_is_a_conflict_and_not_a_second_server_on_one_port() {
        let fixture = Fixture::new().await;
        let max = a_user(&fixture.pool, "max").await;
        let caller = fixture.caller(max).await;
        sqlx::query("UPDATE panel_settings SET port_pool_from = 25565, port_pool_to = 25565")
            .execute(&fixture.pool)
            .await
            .unwrap();

        fixture.manager.create(&caller, fixture.loader_wish("one", max, 512)).await.unwrap();
        let refused =
            fixture.manager.create(&caller, fixture.loader_wish("two", max, 512)).await.unwrap_err();
        assert_eq!(refused.code(), "port_pool_exhausted");
    }

    #[tokio::test]
    async fn a_port_an_admin_picks_by_hand_can_already_belong_to_somebody() {
        let fixture = Fixture::new().await;
        let anna = an_admin(&fixture.pool, "anna").await;
        let caller = fixture.caller(anna).await;
        let first =
            fixture.manager.create(&caller, fixture.loader_wish("one", anna, 512)).await.unwrap();

        let mut wish = fixture.loader_wish("two", anna, 512);
        wish.port = Some(first.server.net.port);
        let refused = fixture.manager.create(&caller, wish).await.unwrap_err();
        assert_eq!(refused.code(), "port_in_use");
    }

    #[tokio::test]
    async fn two_creations_at_the_same_moment_never_share_a_port() {
        let dir = DataDir::new();
        let pool = crate::ops::testing::busy_schema(&dir).await;
        let fixture = Fixture::with(pool.clone(), dir, Shelf::new(), Disks::none()).await;
        let max = insert_user(&pool, "max", PanelRole::User, PASSWORD).await;
        sqlx::query("UPDATE users SET memory_mib = 65536 WHERE id = ?")
            .bind(max)
            .execute(&pool)
            .await
            .unwrap();
        let caller = fixture.caller(max).await;

        let mut made = Vec::new();
        for round in 0..8 {
            let manager = Arc::clone(&fixture.manager);
            let caller = caller.clone();
            let wish = fixture.loader_wish(&format!("server {round}"), max, 512);
            made.push(tokio::spawn(async move { manager.create(&caller, wish).await }));
        }

        let mut ports = BTreeSet::new();
        for handle in made {
            let created = handle.await.expect("the task").expect("8 servers fit the budget");
            assert!(ports.insert(created.server.net.port), "two servers on one port");
        }
        assert_eq!(ports.len(), 8);
    }

    #[tokio::test]
    async fn a_finished_setup_writes_the_jar_the_eula_and_the_panel_port() {
        let fixture = Fixture::new().await;
        let max = a_user(&fixture.pool, "max").await;
        let caller = fixture.caller(max).await;
        let mut wish = fixture.loader_wish("survival", max, 1024);
        let mut known = KnownProperties::default();
        known.motd = Some("hello\nworld".to_owned());
        wish.properties = PropertiesFields {
            known,
            custom: Some(BTreeMap::from([("server-port".to_owned(), "31337".to_owned())])),
        };
        let made = fixture.manager.create(&caller, wish).await.unwrap();

        assert!(fixture.manager.run(made.operation.id).await, "the run had its turn");
        let operation = fixture.manager.operations.get(made.operation.id).await.unwrap();
        assert_eq!(operation.state, OperationState::Done, "{:?}", operation.error);

        let dir = fixture.manager.dir(max, made.server.id);
        assert!(dir.join(JAR).is_file(), "the loader file is where the start command looks");
        assert_eq!(
            std::fs::read_to_string(dir.join("eula.txt")).unwrap().contains("eula=true"),
            true
        );
        let properties = std::fs::read_to_string(dir.join("server.properties")).unwrap();
        assert!(
            properties.contains(&format!("server-port={}", made.server.net.port)),
            "9.2: the port is the panel's: {properties}"
        );
        assert!(!properties.contains("31337"), "a custom key may not take the port");
        assert!(properties.contains("motd=helloworld"), "no line break may split a value");

        let after = fixture.manager.read(made.server.id, Permissions::NONE).await.unwrap();
        assert_eq!(after.status, ServerStatus::Available);
        assert_eq!(after.loader, Some(LoaderId::Paper));
        assert_eq!(after.loader_version.as_deref(), Some("45"));
        assert_eq!(after.game_version.as_deref(), Some("1.21.8"));
        assert!(!after.flows.intro);
    }

    #[tokio::test]
    async fn the_files_of_a_new_server_are_handed_back_to_the_account_that_runs_them() {
        let fixture = Fixture::new().await;
        let max = a_user(&fixture.pool, "max").await;
        let caller = fixture.caller(max).await;
        let made =
            fixture.manager.create(&caller, fixture.loader_wish("survival", max, 1024)).await.unwrap();

        fixture.manager.run(made.operation.id).await;

        let handed_back: Vec<_> = fixture
            .helper
            .calls()
            .into_iter()
            .filter_map(|call| match call {
                craftpanel_proto::HelperRequest::ChownTree { user_id, steps } => Some((user_id, steps)),
                _ => None,
            })
            .collect();
        assert_eq!(handed_back.len(), 1, "one call per run, not one per file");
        assert_eq!(handed_back[0].0, max.to_string());
        assert_eq!(handed_back[0].1, crate::helper::in_servers(made.server.id));
    }

    #[tokio::test]
    async fn a_download_that_fails_leaves_a_broken_server_and_the_intro_open() {
        let fixture = Fixture::failing(LoaderError::Unreachable {
            service: "PaperMC",
            reason: "connection refused".to_owned(),
        })
        .await;
        let max = a_user(&fixture.pool, "max").await;
        let caller = fixture.caller(max).await;
        let made =
            fixture.manager.create(&caller, fixture.loader_wish("survival", max, 1024)).await.unwrap();

        fixture.manager.run(made.operation.id).await;

        let operation = fixture.manager.operations.get(made.operation.id).await.unwrap();
        assert_eq!(operation.state, OperationState::Failed);
        let error = operation.error.expect("5.11 gives the run a code");
        assert_eq!(error.code, "upstream_unavailable");
        assert_eq!(error.step, OperationErrorStep::Download);

        let after = fixture.manager.read(made.server.id, Permissions::NONE).await.unwrap();
        assert_eq!(after.status, ServerStatus::Broken, "5.12");
        assert!(after.flows.intro, "5.12: the set-up is offered again");
    }

    #[tokio::test]
    async fn a_loader_of_the_second_wave_is_refused_before_a_row_is_written() {
        let fixture = Fixture::new().await;
        let max = a_user(&fixture.pool, "max").await;
        let caller = fixture.caller(max).await;
        let mut wish = fixture.loader_wish("forge", max, 1024);
        wish.content = CreateContent::Loader {
            loader: LoaderId::Forge,
            game_version: "1.20.1".to_owned(),
            loader_version: None,
        };

        let refused = fixture.manager.create(&caller, wish).await.unwrap_err();
        assert_eq!(refused.code(), "unknown_loader");
        let (servers, _) = fixture.manager.list(&caller, false).await.unwrap();
        assert!(servers.is_empty(), "nothing was written down");
    }

    #[tokio::test]
    async fn deleting_frees_the_port_at_once_and_the_row_at_the_end() {
        let fixture = Fixture::new().await;
        let max = a_user(&fixture.pool, "max").await;
        let caller = fixture.caller(max).await;
        let made =
            fixture.manager.create(&caller, fixture.loader_wish("survival", max, 1024)).await.unwrap();
        fixture.manager.run(made.operation.id).await;
        let dir = fixture.manager.dir(max, made.server.id);
        assert!(dir.exists());

        let run = fixture.manager.delete(&caller, made.server.id, true).await.unwrap();

        let (listed, _) = fixture.manager.list(&caller, false).await.unwrap();
        assert!(listed.is_empty(), "4.5: it leaves the list with the wish, not with the run");
        assert_eq!(
            fixture.manager.read(made.server.id, Permissions::NONE).await.unwrap().status,
            ServerStatus::Deleting
        );
        let ports: i64 = sqlx::query_scalar("SELECT count(*) FROM allocations")
            .fetch_one(&fixture.pool)
            .await
            .unwrap();
        assert_eq!(ports, 0, "the port is free with the beginning of the run");

        fixture.manager.run(run.id).await;
        assert!(!dir.exists(), "the directory goes with the run");
        assert_eq!(
            fixture.manager.read(made.server.id, Permissions::NONE).await.unwrap_err().code(),
            "server_not_found"
        );
    }

    #[tokio::test]
    async fn a_running_server_is_not_deleted_behind_the_owner_s_back() {
        let fixture = Fixture::new().await;
        let max = a_user(&fixture.pool, "max").await;
        let caller = fixture.caller(max).await;
        let made =
            fixture.manager.create(&caller, fixture.loader_wish("survival", max, 1024)).await.unwrap();
        fixture.manager.run(made.operation.id).await;
        sqlx::query("UPDATE servers SET power_state = 'running' WHERE id = ?")
            .bind(made.server.id)
            .execute(&fixture.pool)
            .await
            .unwrap();

        let refused = fixture.manager.delete(&caller, made.server.id, true).await.unwrap_err();
        assert_eq!(refused.code(), "server_running");
    }

    #[tokio::test]
    async fn a_symlink_out_of_the_users_tree_is_not_followed() {
        let fixture = Fixture::new().await;
        let max = a_user(&fixture.pool, "max").await;
        let elsewhere = fixture.dir.path().join("elsewhere");
        std::fs::create_dir_all(&elsewhere).unwrap();
        std::fs::write(elsewhere.join("precious.txt"), b"someone else's world").unwrap();

        let server = Id::new();
        let dir = fixture.manager.dir(max, server);
        std::fs::create_dir_all(dir.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(&elsewhere, &dir).unwrap();

        let refused = fixture.manager.confined(&dir).unwrap_err();
        assert_eq!(refused.code(), "forbidden_path");
        assert!(elsewhere.join("precious.txt").exists());

        std::fs::remove_file(&dir).unwrap();
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(fixture.manager.confined(&dir).unwrap(), Some(dir.canonicalize().unwrap()));
    }

    #[tokio::test]
    async fn a_delete_run_never_follows_a_server_directory_that_points_out_of_the_tree() {
        let fixture = Fixture::new().await;
        let max = a_user(&fixture.pool, "max").await;
        let caller = fixture.caller(max).await;
        let made = fixture
            .manager
            .create(&caller, fixture.loader_wish("survival", max, 1024))
            .await
            .unwrap();
        fixture.manager.run(made.operation.id).await;

        let elsewhere = fixture.dir.path().join("elsewhere");
        std::fs::create_dir_all(&elsewhere).unwrap();
        std::fs::write(elsewhere.join("precious.txt"), b"someone else's world").unwrap();
        let dir = fixture.manager.dir(max, made.server.id);
        std::fs::remove_dir_all(&dir).unwrap();
        std::os::unix::fs::symlink(&elsewhere, &dir).unwrap();

        let run = fixture.manager.delete(&caller, made.server.id, true).await.unwrap();
        fixture.manager.run(run.id).await;

        assert!(elsewhere.join("precious.txt").exists(), "the link was followed");
        let operation = fixture.manager.operations.get(run.id).await.unwrap();
        assert_eq!(operation.state, OperationState::Failed, "refusing beats guessing");
        assert!(
            fixture.manager.read(made.server.id, Permissions::NONE).await.is_ok(),
            "nothing was deleted, so the row stays"
        );
    }

    #[tokio::test]
    async fn a_link_inside_the_server_directory_is_unlinked_and_not_walked() {
        let fixture = Fixture::new().await;
        let max = a_user(&fixture.pool, "max").await;
        let caller = fixture.caller(max).await;
        let made = fixture
            .manager
            .create(&caller, fixture.loader_wish("survival", max, 1024))
            .await
            .unwrap();
        fixture.manager.run(made.operation.id).await;

        let elsewhere = fixture.dir.path().join("elsewhere");
        std::fs::create_dir_all(&elsewhere).unwrap();
        std::fs::write(elsewhere.join("precious.txt"), b"someone else's world").unwrap();
        let dir = fixture.manager.dir(max, made.server.id);
        std::os::unix::fs::symlink(&elsewhere, dir.join("world")).unwrap();

        let run = fixture.manager.delete(&caller, made.server.id, true).await.unwrap();
        fixture.manager.run(run.id).await;

        assert!(!dir.exists(), "the server's own directory goes");
        assert!(elsewhere.join("precious.txt").exists(), "what the link pointed at stays");
        assert!(elsewhere.exists());
    }

    #[tokio::test]
    async fn a_delete_gets_through_a_directory_the_panel_cannot_walk_into() {
        use std::os::unix::fs::PermissionsExt;

        let fixture = Fixture::new().await;
        let max = a_user(&fixture.pool, "max").await;
        let caller = fixture.caller(max).await;
        let made =
            fixture.manager.create(&caller, fixture.loader_wish("survival", max, 1024)).await.unwrap();
        fixture.manager.run(made.operation.id).await;

        let dir = fixture.manager.dir(max, made.server.id);
        let locked = dir.join("plugins/WorldEdit/.archive-unpack/0ac1a273");
        std::fs::create_dir_all(locked.join("lang")).unwrap();
        std::fs::write(locked.join("lang/strings.json"), b"{}").unwrap();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o500)).unwrap();
        let mode = std::fs::metadata(&locked).unwrap().permissions().mode();
        assert_eq!(mode & 0o070, 0, "the yardstick: the panel has no rights here");
        assert_eq!(mode & 0o200, 0, "and its owner cannot empty it either");

        let before = fixture.helper.calls().len();
        let run = fixture.manager.delete(&caller, made.server.id, true).await.unwrap();
        fixture.manager.run(run.id).await;

        assert!(!dir.exists(), "the directory goes, locked subtree and all");
        assert_eq!(
            fixture.manager.read(made.server.id, Permissions::NONE).await.unwrap_err().code(),
            "server_not_found",
            "the row goes last, so it is the receipt for the files"
        );

        let handed_back: Vec<_> = fixture
            .helper
            .calls()
            .split_off(before)
            .into_iter()
            .filter_map(|call| match call {
                craftpanel_proto::HelperRequest::ChownTree { user_id, steps } => Some((user_id, steps)),
                _ => None,
            })
            .collect();
        assert_eq!(
            handed_back,
            vec![(max.to_string(), crate::helper::in_servers(made.server.id))],
            "the tree is handed back before it goes, once"
        );
    }

    #[tokio::test]
    async fn a_delete_that_failed_puts_the_server_back_into_the_list() {
        let fixture = Fixture::new().await;
        let max = a_user(&fixture.pool, "max").await;
        let caller = fixture.caller(max).await;
        let made =
            fixture.manager.create(&caller, fixture.loader_wish("survival", max, 1024)).await.unwrap();
        fixture.manager.run(made.operation.id).await;

        let dir = fixture.manager.dir(max, made.server.id);
        std::fs::remove_dir_all(&dir).unwrap();
        std::fs::write(&dir, b"not a directory").unwrap();

        let run = fixture.manager.delete(&caller, made.server.id, true).await.unwrap();
        let (during, _) = fixture.manager.list(&caller, false).await.unwrap();
        assert!(during.is_empty(), "4.5: gone from the list while the run is under way");

        fixture.manager.run(run.id).await;
        let operation = fixture.manager.operations.get(run.id).await.unwrap();
        assert_eq!(operation.state, OperationState::Failed);
        let error = operation.error.expect("a failed run says why");
        assert_eq!(error.code, "delete_failed", "not the code of a loader that never ran");
        assert_eq!(error.step, OperationErrorStep::Filesystem);

        let (after, _) = fixture.manager.list(&caller, false).await.unwrap();
        assert_eq!(after.len(), 1, "a delete that failed brings the server back");
        assert_eq!(after[0].id, made.server.id);
        assert_eq!(after[0].status, ServerStatus::Deleting);

        let again = fixture.manager.delete(&caller, made.server.id, true).await.expect("4.5");
        let (second, _) = fixture.manager.list(&caller, false).await.unwrap();
        assert!(second.is_empty(), "the second run is a run like the first, and hides it too");

        fixture.manager.run(again.id).await;
        assert_eq!(
            fixture.manager.operations.get(again.id).await.unwrap().state,
            OperationState::Failed
        );
        let (twice, _) = fixture.manager.list(&caller, false).await.unwrap();
        assert_eq!(twice.len(), 1, "and a second failure brings it back as well");

        std::fs::remove_file(&dir).unwrap();
        std::fs::create_dir_all(&dir).unwrap();
        let last = fixture.manager.delete(&caller, made.server.id, true).await.expect("4.5");
        fixture.manager.run(last.id).await;
        let (gone, _) = fixture.manager.list(&caller, false).await.unwrap();
        assert!(gone.is_empty(), "the try that got through takes the row with it");
        assert_eq!(
            fixture.manager.read(made.server.id, Permissions::NONE).await.unwrap_err().code(),
            "server_not_found"
        );
    }

    #[test]
    fn an_io_error_is_called_what_it_is() {
        let denied = || std::io::Error::from_raw_os_error(libc::EACCES);
        let full = || std::io::Error::from_raw_os_error(libc::ENOSPC);
        let broken = || std::io::Error::from_raw_os_error(libc::EIO);

        assert_eq!(removal(denied()).code, "permission_denied");
        assert_eq!(removal(denied()).step, OperationErrorStep::Filesystem);
        assert_eq!(removal(broken()).code, "delete_failed");
        assert_eq!(removal(full()).code, "no_space");

        assert_eq!(disk(denied()).code, "permission_denied");
        assert_eq!(disk(full()).code, "no_space");
        assert_eq!(disk(std::io::Error::from_raw_os_error(libc::EDQUOT)).code, "no_space");
        let other = disk(broken());
        assert_eq!(other.code, "loader_install_failed");
        assert_eq!(other.step, OperationErrorStep::Modloader);
        assert_eq!(other.message, "internal error");
    }

    #[tokio::test]
    async fn starting_asks_the_helper_and_writes_a_token_nobody_else_knows() {
        let fixture = Fixture::new().await;
        let max = a_user(&fixture.pool, "max").await;
        let caller = fixture.caller(max).await;
        let made =
            fixture.manager.create(&caller, fixture.loader_wish("survival", max, 2048)).await.unwrap();
        fixture.manager.run(made.operation.id).await;

        let (state, target) =
            fixture.manager.power(&caller, made.server.id, PowerAction::Start).await.unwrap();
        assert_eq!(state, PowerState::Starting);
        assert_eq!(target, Some(PowerTarget::Start));

        let spawned = fixture
            .helper
            .calls()
            .into_iter()
            .find_map(|call| match call {
                craftpanel_proto::HelperRequest::Spawn(request) => Some(request),
                _ => None,
            })
            .expect("the helper was asked to start a supervisor");
        assert_eq!(spawned.user_id, max.to_string());
        assert_eq!(spawned.working_dir, crate::helper::in_servers(made.server.id));
        assert!(spawned.args.contains(&"-Xmx2048M".to_owned()), "{:?}", spawned.args);
        assert!(spawned.args.ends_with(&["-jar".to_owned(), JAR.to_owned(), "nogui".to_owned()]));

        let (token, power): (Option<String>, PowerState) =
            sqlx::query_as("SELECT supervisor_token, power_state FROM servers WHERE id = ?")
                .bind(made.server.id)
                .fetch_one(&fixture.pool)
                .await
                .unwrap();
        assert_eq!(token.as_deref(), Some(spawned.token.as_str()));
        assert_eq!(token.map(|token| token.len()), Some(64));
        assert_eq!(power, PowerState::Starting);
    }

    #[tokio::test]
    async fn the_transitions_of_4_6_are_the_ones_run_state_allows() {
        let fixture = Fixture::new().await;
        let max = a_user(&fixture.pool, "max").await;
        let caller = fixture.caller(max).await;
        let made =
            fixture.manager.create(&caller, fixture.loader_wish("survival", max, 2048)).await.unwrap();
        fixture.manager.run(made.operation.id).await;
        let server = made.server.id;

        for refused in [PowerAction::Stop, PowerAction::Restart, PowerAction::Kill] {
            let error = fixture.manager.power(&caller, server, refused).await.unwrap_err();
            assert_eq!(error.code(), "invalid_power_transition", "{refused:?}");
            assert_eq!(error.status(), StatusCode::CONFLICT);
        }
        fixture.manager.power(&caller, server, PowerAction::Start).await.unwrap();

        let twice = fixture.manager.power(&caller, server, PowerAction::Start).await.unwrap_err();
        assert_eq!(twice.code(), "invalid_power_transition");
    }

    #[tokio::test]
    async fn a_broken_server_does_not_start_and_a_busy_one_is_not_asked() {
        let fixture = Fixture::failing(LoaderError::Unreachable {
            service: "PaperMC",
            reason: "connection refused".to_owned(),
        })
        .await;
        let max = a_user(&fixture.pool, "max").await;
        let caller = fixture.caller(max).await;
        let made =
            fixture.manager.create(&caller, fixture.loader_wish("survival", max, 1024)).await.unwrap();

        let busy =
            fixture.manager.power(&caller, made.server.id, PowerAction::Start).await.unwrap_err();
        assert_eq!(busy.code(), "server_busy");

        fixture.manager.run(made.operation.id).await;
        let broken =
            fixture.manager.power(&caller, made.server.id, PowerAction::Start).await.unwrap_err();
        assert_eq!(broken.code(), "server_broken");
    }

    #[tokio::test]
    async fn a_start_is_refused_while_the_owner_is_over_his_limit() {
        let fixture = Fixture::new().await;
        let max = a_user(&fixture.pool, "max").await;
        let caller = fixture.caller(max).await;
        let made =
            fixture.manager.create(&caller, fixture.loader_wish("survival", max, 4096)).await.unwrap();
        fixture.manager.run(made.operation.id).await;
        sqlx::query("UPDATE users SET memory_mib = 1024 WHERE id = ?")
            .bind(max)
            .execute(&fixture.pool)
            .await
            .unwrap();

        let refused =
            fixture.manager.power(&caller, made.server.id, PowerAction::Start).await.unwrap_err();
        assert_eq!(refused.code(), "over_limit", "docs/PLAN.md:364-366");
    }

    #[tokio::test]
    async fn the_power_wishes_are_written_into_the_audit_log() {
        let fixture = Fixture::new().await;
        let max = a_user(&fixture.pool, "max").await;
        let caller = fixture.caller(max).await;
        let made =
            fixture.manager.create(&caller, fixture.loader_wish("survival", max, 1024)).await.unwrap();
        fixture.manager.run(made.operation.id).await;
        fixture.manager.power(&caller, made.server.id, PowerAction::Start).await.unwrap();

        let actions: Vec<(String,)> =
            sqlx::query_as("SELECT action FROM audit_log WHERE server_id = ? ORDER BY created_at")
                .bind(made.server.id)
                .fetch_all(&fixture.pool)
                .await
                .unwrap();
        let actions: Vec<String> = actions.into_iter().map(|(action,)| action).collect();
        assert!(actions.contains(&"server_created".to_owned()));
        assert!(actions.contains(&"server_started".to_owned()));
    }

    #[tokio::test]
    async fn a_restart_starts_the_server_again_once_it_has_stopped() {
        let fixture = Fixture::new().await;
        let max = a_user(&fixture.pool, "max").await;
        let caller = fixture.caller(max).await;
        let made = fixture
            .manager
            .create(&caller, fixture.loader_wish("survival", max, 1024))
            .await
            .unwrap();
        fixture.manager.run(made.operation.id).await;
        let server = made.server.id;
        fixture.manager.power(&caller, server, PowerAction::Start).await.unwrap();
        let first_starts = spawns(&fixture);

        fixture.manager.set_wish(
            server,
            Wish { target: Some(PowerTarget::Restart), killed: false, undelivered: false },
        );
        fixture.manager.settle(server, RunState::Stopped).await;

        assert_eq!(spawns(&fixture), first_starts + 1, "the server was started again");
        let state: PowerState = sqlx::query_scalar("SELECT power_state FROM servers WHERE id = ?")
            .bind(server)
            .fetch_one(&fixture.pool)
            .await
            .unwrap();
        assert_eq!(state, PowerState::Starting);
        assert_eq!(fixture.manager.wish(server).target, Some(PowerTarget::Start));
    }

    #[tokio::test]
    async fn a_killed_server_reports_stopped_and_never_crashed() {
        let fixture = Fixture::new().await;
        let max = a_user(&fixture.pool, "max").await;
        let caller = fixture.caller(max).await;
        let made = fixture
            .manager
            .create(&caller, fixture.loader_wish("survival", max, 1024))
            .await
            .unwrap();
        fixture.manager.run(made.operation.id).await;
        let server = made.server.id;

        fixture.manager.set_wish(server, Wish { target: None, killed: true, undelivered: false });
        fixture.manager.settle(server, RunState::Crashed).await;
        assert_eq!(power_of(&fixture, server).await, PowerState::Stopped);

        fixture.manager.set_wish(server, Wish::default());
        fixture.manager.settle(server, RunState::Crashed).await;
        assert_eq!(power_of(&fixture, server).await, PowerState::Crashed);

        fixture.manager.settle(server, RunState::OutOfMemory).await;
        let (state, oom): (PowerState, bool) =
            sqlx::query_as("SELECT power_state, oom_killed FROM servers WHERE id = ?")
                .bind(server)
                .fetch_one(&fixture.pool)
                .await
                .unwrap();
        assert_eq!(state, PowerState::Crashed);
        assert!(oom);
    }

    #[tokio::test]
    async fn a_stop_that_had_to_be_forced_is_not_reported_as_a_crash() {
        let fixture = Fixture::new().await;
        let max = a_user(&fixture.pool, "max").await;
        let caller = fixture.caller(max).await;
        let made = fixture
            .manager
            .create(&caller, fixture.loader_wish("survival", max, 1024))
            .await
            .unwrap();
        fixture.manager.run(made.operation.id).await;
        let server = made.server.id;

        fixture.manager.set_wish(
            server,
            Wish { target: Some(PowerTarget::Stop), killed: false, undelivered: false },
        );
        fixture.manager.settle(server, RunState::Crashed).await;
        assert_eq!(power_of(&fixture, server).await, PowerState::Stopped);

        let mut heard = fixture.manager.operations.bus().channel(server).attach().events;
        fixture.manager.set_wish(
            server,
            Wish { target: Some(PowerTarget::Restart), killed: false, undelivered: false },
        );
        fixture.manager.settle(server, RunState::Crashed).await;
        let mut said = Vec::new();
        while let Ok(event) = heard.try_recv() {
            if let crate::ops::ServerEvent::Say(line) = event {
                said.push(line.to_string());
            }
        }
        assert!(said.iter().any(|line| line.contains("\"stopped\"")), "{said:?}");
        assert!(!said.iter().any(|line| line.contains("\"crashed\"")), "{said:?}");

        fixture.manager.set_wish(
            server,
            Wish { target: Some(PowerTarget::Stop), killed: false, undelivered: false },
        );
        fixture.manager.settle(server, RunState::OutOfMemory).await;
        assert_eq!(power_of(&fixture, server).await, PowerState::Crashed);
    }

    #[tokio::test]
    async fn a_stop_pressed_before_the_supervisor_calls_in_is_handed_over_when_it_does() {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

        let fixture = Fixture::new().await;
        let max = a_user(&fixture.pool, "max").await;
        let caller = fixture.caller(max).await;
        let made = fixture
            .manager
            .create(&caller, fixture.loader_wish("survival", max, 1024))
            .await
            .unwrap();
        fixture.manager.run(made.operation.id).await;
        let server = made.server.id;
        let listening = tokio::spawn(Arc::clone(&fixture.hub).listen());

        fixture.manager.power(&caller, server, PowerAction::Start).await.unwrap();
        let (state, target) =
            fixture.manager.power(&caller, server, PowerAction::Stop).await.expect("4.6");
        assert_eq!(state, PowerState::Stopping);
        assert_eq!(target, Some(PowerTarget::Stop));
        assert!(fixture.manager.wish(server).undelivered, "nobody could take it yet");

        let token: String = sqlx::query_scalar("SELECT supervisor_token FROM servers WHERE id = ?")
            .bind(server)
            .fetch_one(&fixture.pool)
            .await
            .unwrap();
        let stream = loop {
            match tokio::net::UnixStream::connect(fixture.hub.socket()).await {
                Ok(stream) => break stream,
                Err(_) => tokio::time::sleep(Duration::from_millis(20)).await,
            }
        };
        let (reader, mut writer) = stream.into_split();
        let mut reader = BufReader::new(reader).lines();
        let hello = craftpanel_proto::SupervisorMessage::Hello {
            server_id: server.to_string(),
            token,
            pid: std::process::id(),
            protocol: craftpanel_proto::HELPER_PROTOCOL_VERSION,
        };
        let mut line = serde_json::to_vec(&hello).unwrap();
        line.push(b'\n');
        writer.write_all(&line).await.unwrap();
        writer.flush().await.unwrap();
        assert!(reader.next_line().await.unwrap().unwrap().contains("accepted"));

        let waiting = tokio::time::timeout(Duration::from_secs(10), reader.next_line())
            .await
            .expect("the watcher hands the wish over")
            .unwrap()
            .expect("a line");
        assert!(waiting.contains("\"stop\""), "{waiting}");
        assert!(!fixture.manager.wish(server).undelivered, "handed over once, not for ever");

        listening.abort();
    }

    #[tokio::test]
    async fn a_kill_pressed_before_the_supervisor_calls_in_is_kept_and_not_dropped() {
        let fixture = Fixture::new().await;
        let max = a_user(&fixture.pool, "max").await;
        let caller = fixture.caller(max).await;
        let made = fixture
            .manager
            .create(&caller, fixture.loader_wish("survival", max, 1024))
            .await
            .unwrap();
        fixture.manager.run(made.operation.id).await;
        let server = made.server.id;

        fixture.manager.power(&caller, server, PowerAction::Start).await.unwrap();
        let (state, target) =
            fixture.manager.power(&caller, server, PowerAction::Kill).await.unwrap();
        assert_eq!(state, PowerState::Stopping);
        assert_eq!(target, None, "14: a kill never stays behind as a wish");

        let wish = fixture.manager.wish(server);
        assert!(wish.killed);
        assert!(wish.undelivered, "the supervisor was not there to hear it");
    }

    #[tokio::test]
    async fn a_stop_on_a_row_that_only_claims_to_run_is_refused_and_the_row_corrected() {
        let fixture = Fixture::new().await;
        let max = a_user(&fixture.pool, "max").await;
        let caller = fixture.caller(max).await;
        let made = fixture
            .manager
            .create(&caller, fixture.loader_wish("survival", max, 1024))
            .await
            .unwrap();
        fixture.manager.run(made.operation.id).await;
        sqlx::query("UPDATE servers SET power_state = 'running' WHERE id = ?")
            .bind(made.server.id)
            .execute(&fixture.pool)
            .await
            .unwrap();

        let refused =
            fixture.manager.power(&caller, made.server.id, PowerAction::Stop).await.unwrap_err();
        assert_eq!(refused.code(), "invalid_power_transition");
        assert_eq!(power_of(&fixture, made.server.id).await, PowerState::Stopped);
    }

    #[tokio::test]
    async fn a_backup_being_made_keeps_the_start_button_shut() {
        let fixture = Fixture::new().await;
        let max = a_user(&fixture.pool, "max").await;
        let caller = fixture.caller(max).await;
        let made = fixture
            .manager
            .create(&caller, fixture.loader_wish("survival", max, 1024))
            .await
            .unwrap();
        fixture.manager.run(made.operation.id).await;

        fixture
            .manager
            .operations
            .create(NewOperation::new(made.server.id, OperationKind::BackupCreate, Some(max)))
            .await
            .unwrap();

        let refused =
            fixture.manager.power(&caller, made.server.id, PowerAction::Start).await.unwrap_err();
        assert_eq!(refused.code(), "server_busy");
        assert_eq!(refused.status(), StatusCode::CONFLICT);
        assert_eq!(
            spawns(&fixture),
            0,
            "5.8: nothing may be started while another run holds the server"
        );
    }

    #[tokio::test]
    async fn the_page_is_told_the_moment_a_server_turns_to_deleting() {
        let fixture = Fixture::new().await;
        let max = a_user(&fixture.pool, "max").await;
        let caller = fixture.caller(max).await;
        let made = fixture
            .manager
            .create(&caller, fixture.loader_wish("survival", max, 1024))
            .await
            .unwrap();
        fixture.manager.run(made.operation.id).await;

        let mut events =
            fixture.manager.operations.bus().channel(made.server.id).attach().events;
        fixture.manager.delete(&caller, made.server.id, true).await.unwrap();

        let mut told = None;
        while let Ok(event) = events.try_recv() {
            if let crate::ops::ServerEvent::Server(object) = event {
                told = Some(object);
            }
        }
        let told = told.expect("13.3: the stock object changed and nobody was told");
        assert_eq!(told.status, ServerStatus::Deleting);
    }

    fn spawns(fixture: &Fixture) -> usize {
        fixture
            .helper
            .calls()
            .iter()
            .filter(|call| matches!(call, craftpanel_proto::HelperRequest::Spawn(_)))
            .count()
    }

    async fn power_of(fixture: &Fixture, server: Id) -> PowerState {
        sqlx::query_scalar("SELECT power_state FROM servers WHERE id = ?")
            .bind(server)
            .fetch_one(&fixture.pool)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn a_row_that_claims_to_run_without_a_supervisor_is_put_straight() {
        let fixture = Fixture::new().await;
        let max = a_user(&fixture.pool, "max").await;
        let caller = fixture.caller(max).await;
        let made =
            fixture.manager.create(&caller, fixture.loader_wish("survival", max, 1024)).await.unwrap();
        fixture.manager.run(made.operation.id).await;
        sqlx::query(
            "UPDATE servers SET power_state = 'running', supervisor_token = 'stale' WHERE id = ?",
        )
        .bind(made.server.id)
        .execute(&fixture.pool)
        .await
        .unwrap();

        assert_eq!(fixture.manager.adopt_tokens().await.unwrap(), 1);
        let found = fixture.manager.reconcile().await.unwrap();

        assert_eq!(found.cleared, vec![made.server.id]);
        assert!(found.attached.is_empty());
        let (state, token): (PowerState, Option<String>) =
            sqlx::query_as("SELECT power_state, supervisor_token FROM servers WHERE id = ?")
                .bind(made.server.id)
                .fetch_one(&fixture.pool)
                .await
                .unwrap();
        assert_eq!(state, PowerState::Stopped);
        assert_eq!(token, None, "a token nobody used is a token nobody keeps");
    }

    #[tokio::test]
    async fn a_supervisor_that_found_us_again_keeps_its_server_running() {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

        let fixture = Fixture::new().await;
        let max = a_user(&fixture.pool, "max").await;
        let caller = fixture.caller(max).await;
        let made =
            fixture.manager.create(&caller, fixture.loader_wish("survival", max, 1024)).await.unwrap();
        fixture.manager.run(made.operation.id).await;
        sqlx::query(
            "UPDATE servers SET power_state = 'running', supervisor_token = 'the-token'
              WHERE id = ?",
        )
        .bind(made.server.id)
        .execute(&fixture.pool)
        .await
        .unwrap();

        let listening = tokio::spawn(Arc::clone(&fixture.hub).listen());
        fixture.manager.adopt_tokens().await.unwrap();

        let stream = loop {
            match tokio::net::UnixStream::connect(fixture.hub.socket()).await {
                Ok(stream) => break stream,
                Err(_) => tokio::time::sleep(Duration::from_millis(20)).await,
            }
        };
        let (reader, mut writer) = stream.into_split();
        let mut reader = BufReader::new(reader).lines();
        let hello = craftpanel_proto::SupervisorMessage::Hello {
            server_id: made.server.id.to_string(),
            token: "the-token".to_owned(),
            pid: std::process::id(),
            protocol: craftpanel_proto::HELPER_PROTOCOL_VERSION,
        };
        let mut line = serde_json::to_vec(&hello).unwrap();
        line.push(b'\n');
        writer.write_all(&line).await.unwrap();
        writer.flush().await.unwrap();
        let greeting = reader.next_line().await.unwrap().expect("the hub answers");
        assert!(greeting.contains("accepted"), "{greeting}");

        let found = fixture.manager.reconcile().await.unwrap();
        assert_eq!(found.attached, vec![made.server.id], "5.12: this one is still alive");
        assert!(found.cleared.is_empty());
        let state: PowerState = sqlx::query_scalar("SELECT power_state FROM servers WHERE id = ?")
            .bind(made.server.id)
            .fetch_one(&fixture.pool)
            .await
            .unwrap();
        assert_eq!(state, PowerState::Running, "what runs keeps running");

        listening.abort();
    }

    #[tokio::test]
    async fn a_set_up_that_the_panel_did_not_live_to_finish_says_so() {
        let fixture = Fixture::new().await;
        let max = a_user(&fixture.pool, "max").await;
        let caller = fixture.caller(max).await;
        let made =
            fixture.manager.create(&caller, fixture.loader_wish("survival", max, 1024)).await.unwrap();

        fixture
            .manager
            .operations
            .fail(
                made.operation.id,
                OperationError {
                    code: "panel_restarted".to_owned(),
                    message: "the panel was restarted".to_owned(),
                    step: OperationErrorStep::Internal,
                },
            )
            .await
            .unwrap();

        let found = fixture.manager.reconcile().await.unwrap();
        assert_eq!(found.broken, vec![made.server.id]);
        let after = fixture.manager.read(made.server.id, Permissions::NONE).await.unwrap();
        assert_eq!(after.status, ServerStatus::Broken);
        assert!(after.flows.intro);
    }

    #[tokio::test]
    async fn a_delete_that_was_interrupted_is_carried_on_after_the_restart() {
        let fixture = Fixture::new().await;
        let max = a_user(&fixture.pool, "max").await;
        let caller = fixture.caller(max).await;
        let made = fixture
            .manager
            .create(&caller, fixture.loader_wish("survival", max, 1024))
            .await
            .unwrap();
        fixture.manager.run(made.operation.id).await;
        let run = fixture.manager.delete(&caller, made.server.id, true).await.unwrap();
        sqlx::query("UPDATE operations SET state = 'ongoing' WHERE id = ?")
            .bind(run.id)
            .execute(&fixture.pool)
            .await
            .unwrap();

        let found = fixture.manager.reconcile().await.unwrap();

        assert_eq!(found.resumed, vec![run.id]);
        assert!(!fixture.manager.dir(max, made.server.id).exists());
        let left: i64 = sqlx::query_scalar("SELECT count(*) FROM servers")
            .fetch_one(&fixture.pool)
            .await
            .unwrap();
        assert_eq!(left, 0, "the row goes with the files");
    }

    #[tokio::test]
    async fn the_figures_of_a_stopped_server_are_measured_and_not_invented() {
        let fixture = Fixture::new().await;
        let max = a_user(&fixture.pool, "max").await;
        let caller = fixture.caller(max).await;
        let made =
            fixture.manager.create(&caller, fixture.loader_wish("survival", max, 1024)).await.unwrap();
        fixture.manager.run(made.operation.id).await;

        let sample = fixture.manager.sample(made.server.id).await.unwrap();
        assert_eq!(sample.ram_total_bytes, 1024 * MIB, "13.4: -Xmx and nothing else");
        assert_eq!(sample.cpu_percent, 0.0, "nothing is running");
        assert_eq!(sample.ram_usage_bytes, 0);
        assert!(sample.storage_usage_bytes > 0, "the jar and two files are on the disk");
        assert!(
            sample.storage_total_bytes > sample.storage_usage_bytes,
            "13.4: statvfs of the filesystem the data directory sits on"
        );
    }

    #[test]
    fn cpu_is_a_rate_and_a_rate_needs_two_visits() {
        let mut meter = Meter::default();
        assert_eq!(meter.cpu(Some(1_000), 1.0), 0.0, "one visit is not a rate");
        meter.cpu = Some((Instant::now() - Duration::from_secs(1), 1_000));
        let percent = meter.cpu(Some(1_100), 1.0);
        assert!((percent - 100.0).abs() < 5.0, "{percent}");

        meter.cpu = Some((Instant::now() - Duration::from_secs(1), 1_000));
        let half = meter.cpu(Some(1_100), 2.0);
        assert!((half - 50.0).abs() < 5.0, "two cores of budget halve the figure: {half}");
    }

    #[test]
    fn a_filesystem_that_does_not_exist_is_zero_and_not_a_guess() {
        assert_eq!(crate::files::filesystem_total_bytes(Path::new("/definitely/not/here")), 0);
        assert!(crate::files::filesystem_total_bytes(Path::new("/tmp")) > 0);
    }

    #[test]
    fn this_process_can_be_measured_at_all() {
        let me = std::process::id();
        assert!(cpu_ticks(me).is_some(), "/proc/<pid>/stat should be readable here");
        assert!(rss_bytes(me) > 0, "/proc/<pid>/status should carry a VmRSS");
        assert!(tree(me).contains(&me));
    }

    #[test]
    fn the_start_command_carries_the_heap_and_a_proxy_carries_no_nogui() {
        let ordinary = argv(Some(LoaderId::Paper), 4096, &[]);
        assert_eq!(ordinary, ["-Xmx4096M", "-jar", JAR, "nogui"]);

        let proxy = argv(Some(LoaderId::Velocity), 512, &[]);
        assert_eq!(proxy, ["-Xmx512M", "-jar", JAR]);

        let cheeky = argv(
            Some(LoaderId::Paper),
            1024,
            &["-Xmx64G".to_owned(), "-XX:+UseG1GC".to_owned(), "-Xms8G".to_owned()],
        );
        assert_eq!(cheeky, ["-Xmx1024M", "-XX:+UseG1GC", "-jar", JAR, "nogui"]);
    }

    #[test]
    fn the_loader_sources_fit_the_seam_the_manager_asks_for() {
        fn takes_a_source(_: Arc<dyn Builds>) {}
        if let Ok(sources) = Sources::new() {
            takes_a_source(Arc::new(sources));
        }
    }

    #[test]
    fn a_name_is_one_to_sixty_four_printable_characters() {
        assert!(check_name("Survival").is_ok());
        assert!(check_name(&"a".repeat(64)).is_ok());
        assert!(check_name("").is_err());
        assert!(check_name("   ").is_err());
        assert!(check_name(&"a".repeat(65)).is_err());
        assert!(check_name("two\nlines").is_err());
    }

    #[test]
    fn the_twenty_five_known_keys_lose_their_underscores() {
        let mut known = KnownProperties::default();
        known.max_players = Some("20".to_owned());
        known.resource_pack_sha1 = Some("abc".to_owned());
        let text = properties_text(&PropertiesFields { known, custom: None }, 25570);

        assert!(text.contains("max-players=20"), "{text}");
        assert!(text.contains("resource-pack-sha1=abc"), "{text}");
        assert!(text.contains("server-port=25570"));
        assert!(text.contains("query.port=25570"));
    }
}

fn system_java() -> PathBuf {
    let system = PathBuf::from("/usr/bin/java");
    if system.exists() {
        system
    } else {
        PathBuf::from("java")
    }
}

fn installed_javas() -> Vec<(u32, PathBuf)> {
    let Ok(entries) = std::fs::read_dir("/usr/lib/jvm") else {
        return Vec::new();
    };

    let mut found: Vec<(u32, PathBuf)> = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            let binary = entry.path().join("bin").join("java");
            if !binary.exists() {
                return None;
            }
            let major = name
                .split(|c: char| !c.is_ascii_digit())
                .filter(|part| !part.is_empty())
                .filter_map(|part| part.parse::<u32>().ok())
                .find(|major| (8..=99).contains(major))?;
            Some((major, binary))
        })
        .collect();

    found.sort_by_key(|(major, _)| *major);
    found.dedup_by_key(|(major, _)| *major);
    found
}

fn installed_java(major: u32) -> Option<PathBuf> {
    installed_javas().into_iter().find(|(found, _)| *found == major).map(|(_, path)| path)
}

fn newest_java_at_least(major: u32) -> Option<PathBuf> {
    installed_javas().into_iter().filter(|(found, _)| *found >= major).next_back().map(|(_, p)| p)
}
