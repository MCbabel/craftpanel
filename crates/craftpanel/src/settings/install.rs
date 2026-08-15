use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

use super::catalog::Catalog;
use super::{server_dir, store, ServerRow};
use crate::auth::error::{Failure, Result};
use crate::helper::Helper;
use crate::model::{
    Id, LoaderId, Operation, OperationError, OperationErrorStep, OperationKind, OperationPhase,
    ServerStatus, Timestamp,
};
use crate::ops::{NewOperation, Operations, Step};

const WAIT_FOR_ROOM: Duration = Duration::from_secs(15 * 60);
const POLL: Duration = Duration::from_secs(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContentPolicy {
    Keep,
    WipeMods,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ServerWarning {
    MemoryOvercommitted,
    PropertiesWillBeIgnored,
}

#[derive(Debug, Clone, Deserialize)]
pub struct InstallRequest {
    pub loader: String,
    pub game_version: String,
    pub loader_version: Option<String>,
    pub content_policy: ContentPolicy,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ResetRequest {
    pub loader: String,
    pub game_version: String,
    pub loader_version: Option<String>,
    pub keep_backups: bool,
}

pub fn read_loader(name: &str) -> Result<LoaderId> {
    name.parse().map_err(|_| {
        Failure::new(
            axum::http::StatusCode::UNPROCESSABLE_ENTITY,
            "unknown_loader",
            format!("{name} is not one of the ten loaders"),
        )
    })
}

#[derive(Debug, Clone, Serialize)]
pub struct InstallAccepted {
    pub operation: Operation,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<ServerWarning>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ResetToSetupResponse {
    pub server_id: Id,
    pub flows: Flows,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct Flows {
    pub intro: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    pub loader: LoaderId,
    pub game_version: String,
    pub build: Option<String>,
    pub policy: ContentPolicy,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Job {
    Install { plan: Plan, wipe_everything: bool },
    Repair,
}

impl Job {
    pub fn kind(&self) -> OperationKind {
        match self {
            Self::Install { wipe_everything: true, .. } => OperationKind::ResetServer,
            Self::Install { .. } => OperationKind::InstallLoader,
            Self::Repair => OperationKind::RepairContent,
        }
    }
}

pub fn check_change(
    current: Option<LoaderId>,
    wanted: LoaderId,
    policy: ContentPolicy,
) -> Result<Vec<ServerWarning>> {
    let Some(current) = current else {
        return Ok(Vec::new());
    };
    if current.family() != wanted.family() && policy != ContentPolicy::WipeMods {
        return Err(Failure::conflict(
            "loader_change_needs_wipe",
            format!(
                "{current} and {wanted} do not read each other's content; \
                 send content_policy \"wipe_mods\""
            ),
        ));
    }

    let was_proxy = current == LoaderId::Velocity;
    let will_be_proxy = wanted == LoaderId::Velocity;
    Ok(if was_proxy == will_be_proxy {
        Vec::new()
    } else {
        vec![ServerWarning::PropertiesWillBeIgnored]
    })
}

pub struct Runner {
    pool: SqlitePool,
    operations: Arc<Operations>,
    catalog: &'static Catalog,
    helper: Helper,
    data_dir: PathBuf,
    cache_dir: PathBuf,
}

impl Runner {
    pub fn new(
        pool: SqlitePool,
        operations: Arc<Operations>,
        catalog: &'static Catalog,
        helper: Helper,
        data_dir: PathBuf,
        cache_dir: PathBuf,
    ) -> Arc<Self> {
        Arc::new(Self { pool, operations, catalog, helper, data_dir, cache_dir })
    }

    pub async fn start(
        self: &Arc<Self>,
        server: &ServerRow,
        by: Id,
        job: Job,
    ) -> Result<Operation> {
        self.operations.guard_write(server.id).await.map_err(super::from_fault)?;

        let mut new = NewOperation::new(server.id, job.kind(), Some(by));
        new.message = Some(message_of(&job));
        new.input = serde_json::to_value(Input::of(&job)).ok();
        let operation =
            self.operations.create(new).await.map_err(super::from_fault)?;

        let runner = Arc::clone(self);
        let server = server.clone();
        let id = operation.id;
        tokio::spawn(async move {
            if let Err(err) = runner.drive(id, server, job).await {
                tracing::error!("install run {id} could not be finished: {err:#}");
            }
        });

        Ok(operation)
    }

    async fn drive(&self, id: Id, server: ServerRow, job: Job) -> anyhow::Result<()> {
        if !self.wait_for_room(id).await? {
            self.operations
                .fail(
                    id,
                    error(
                        "timeout",
                        "the machine stayed busy for fifteen minutes",
                        OperationErrorStep::Internal,
                    ),
                )
                .await
                .ok();
            return Ok(());
        }

        match self.work(id, &server, &job).await {
            Ok(()) => {
                self.operations.finish(id).await.ok();
            }
            Err(failure) => {
                mark_broken(&self.pool, server.id).await.ok();
                self.operations.fail(id, failure).await.ok();
            }
        }
        Ok(())
    }

    async fn wait_for_room(&self, id: Id) -> anyhow::Result<bool> {
        let deadline = std::time::Instant::now() + WAIT_FOR_ROOM;
        loop {
            if self.operations.begin(id).await.is_ok_and(|started| started.is_some()) {
                return Ok(true);
            }
            if std::time::Instant::now() >= deadline {
                return Ok(false);
            }
            tokio::time::sleep(POLL).await;
        }
    }

    async fn work(
        &self,
        id: Id,
        server: &ServerRow,
        job: &Job,
    ) -> std::result::Result<(), OperationError> {
        let dir = server_dir(&self.data_dir, server.owner_id, server.id);

        super::disk::ensure_reachable(&dir).map_err(filesystem)?;

        let plan = match job {
            Job::Install { plan, .. } => plan.clone(),
            Job::Repair => Plan {
                loader: server.loader.ok_or_else(|| {
                    error(
                        "loader_install_failed",
                        "this server has no loader to repair",
                        OperationErrorStep::Modloader,
                    )
                })?,
                game_version: server.game_version.clone().unwrap_or_default(),
                build: server.loader_version.clone(),
                policy: ContentPolicy::Keep,
            },
        };

        self.step(id, OperationPhase::Analyzing, 0.03, None).await;
        let build = self
            .catalog
            .resolve(plan.loader, &plan.game_version, plan.build.as_deref())
            .await
            .map_err(resolve_failed)?;

        if let Job::Install { wipe_everything: true, .. } = job {
            let owner = server.owner_id.to_string();
            let steps = crate::helper::in_servers(server.id);
            if let Err(err) = self.helper.chown_tree(&owner, steps).await {
                tracing::warn!(server = %server.id, "the reset did not open the tree: {err:#}");
            }
            clear(&dir).map_err(filesystem)?;
        } else if plan.policy == ContentPolicy::WipeMods {
            put_aside(&dir).map_err(filesystem)?;
        }

        self.step(id, OperationPhase::InstallingLoader, 0.10, Some(build.filename.clone())).await;
        let cached = self
            .cache_dir
            .join("loaders")
            .join(plan.loader.as_str())
            .join(&plan.game_version)
            .join(&build.id)
            .join(&build.filename);

        if !cached.is_file() {
            let source = plan.loader.source().ok_or_else(|| {
                error(
                    "loader_install_failed",
                    "this loader has no source in this build",
                    OperationErrorStep::Modloader,
                )
            })?;
            self.catalog
                .sources()
                .download(source, &build, &cached)
                .await
                .map_err(download_failed)?;
        }

        self.step(id, OperationPhase::Verifying, 0.70, None).await;
        let jar = super::startup::launch_of(Some(plan.loader)).jar;
        super::disk::copy(&dir, jar, &cached).map_err(filesystem)?;

        self.step(id, OperationPhase::WritingConfig, 0.90, None).await;
        self.write_config(server, &dir, &plan).await.map_err(filesystem)?;

        self.helper
            .chown_tree(&server.owner_id.to_string(), crate::helper::in_servers(server.id))
            .await
            .map_err(|err| {
                error("loader_install_failed", err.to_string(), OperationErrorStep::Filesystem)
            })?;

        record(&self.pool, server.id, &plan, &build.id).await.map_err(|err| {
            error("loader_install_failed", err.to_string(), OperationErrorStep::Internal)
        })?;
        Ok(())
    }

    async fn write_config(
        &self,
        server: &ServerRow,
        dir: &Path,
        plan: &Plan,
    ) -> std::io::Result<()> {
        super::disk::write(
            dir,
            "eula.txt",
            b"# Accepted through the panel, see https://aka.ms/MinecraftEULA\neula=true\n",
        )?;

        if !plan.loader.supports_properties() {
            return Ok(());
        }
        let port = super::allocations::primary(&self.pool, server.id).await.ok().flatten();
        let mut properties = store::read(dir).unwrap_or_default();
        if let Some(port) = port {
            store::set_ports(&mut properties, port);
        }
        store::write(dir, &properties)
            .map_err(|err| std::io::Error::other(err.to_string()))
    }

    #[cfg(test)]
    pub async fn write_config_for_test(
        &self,
        server: &ServerRow,
        dir: &Path,
        plan: &Plan,
    ) -> std::io::Result<()> {
        self.write_config(server, dir, plan).await
    }

    async fn step(&self, id: Id, phase: OperationPhase, progress: f64, file: Option<String>) {
        let step = Step {
            phase: Some(phase),
            progress: Some(progress),
            current_file: file,
            ..Step::default()
        };
        if let Err(fault) = self.operations.advance(id, step).await {
            tracing::warn!("a step of {id} was not written: {}", fault.message());
        }
    }
}

#[derive(Serialize, Deserialize)]
struct Input {
    loader: Option<LoaderId>,
    game_version: Option<String>,
    loader_version: Option<String>,
    content_policy: Option<ContentPolicy>,
    wipe_everything: bool,
}

impl Input {
    fn of(job: &Job) -> Self {
        match job {
            Job::Install { plan, wipe_everything } => Self {
                loader: Some(plan.loader),
                game_version: Some(plan.game_version.clone()),
                loader_version: plan.build.clone(),
                content_policy: Some(plan.policy),
                wipe_everything: *wipe_everything,
            },
            Job::Repair => Self {
                loader: None,
                game_version: None,
                loader_version: None,
                content_policy: None,
                wipe_everything: false,
            },
        }
    }
}

fn message_of(job: &Job) -> String {
    match job {
        Job::Install { plan, wipe_everything: true } => {
            format!("Resetting to {} {}", plan.loader, plan.game_version)
        }
        Job::Install { plan, .. } => format!("Installing {} {}", plan.loader, plan.game_version),
        Job::Repair => "Repairing the installation".to_owned(),
    }
}

fn clear(dir: &Path) -> std::io::Result<()> {
    match std::fs::remove_dir_all(dir) {
        Ok(()) => {}
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => return Err(err),
    }
    std::fs::create_dir_all(dir)
}

fn put_aside(dir: &Path) -> std::io::Result<()> {
    let stamp = Timestamp::now().unix_seconds();
    for name in ["mods", "plugins"] {
        let from = dir.join(name);
        if from.is_dir() {
            std::fs::rename(&from, dir.join(format!("{name}.before-{stamp}")))?;
        }
    }
    Ok(())
}

async fn record(pool: &SqlitePool, server: Id, plan: &Plan, build: &str) -> sqlx::Result<()> {
    let build = (plan.loader != LoaderId::Vanilla).then_some(build);
    sqlx::query(
        "UPDATE servers SET loader = ?, game_version = ?, loader_version = ?, \
         status = 'available', flows_intro = 0, updated_at = ? WHERE id = ?",
    )
    .bind(plan.loader)
    .bind(&plan.game_version)
    .bind(build)
    .bind(Timestamp::now())
    .bind(server)
    .execute(pool)
    .await
    .map(|_| ())
}

async fn mark_broken(pool: &SqlitePool, server: Id) -> sqlx::Result<()> {
    sqlx::query("UPDATE servers SET status = ?, updated_at = ? WHERE id = ?")
        .bind(ServerStatus::Broken)
        .bind(Timestamp::now())
        .bind(server)
        .execute(pool)
        .await
        .map(|_| ())
}

pub async fn reset_to_setup(pool: &SqlitePool, server: Id) -> sqlx::Result<()> {
    sqlx::query(
        "UPDATE servers SET flows_intro = 1, loader = NULL, loader_version = NULL, \
         game_version = NULL, updated_at = ? WHERE id = ?",
    )
    .bind(Timestamp::now())
    .bind(server)
    .execute(pool)
    .await
    .map(|_| ())
}

fn error(code: &str, message: impl Into<String>, step: OperationErrorStep) -> OperationError {
    OperationError { code: code.to_owned(), message: message.into(), step }
}

fn resolve_failed(failure: Failure) -> OperationError {
    match failure.code() {
        "unsupported_game_version" => error(
            "unsupported_game_version",
            "this version is not yet supported",
            OperationErrorStep::Modloader,
        ),
        "build_not_found" => error(
            "invalid_version",
            "the specified version may be incorrect",
            OperationErrorStep::Modloader,
        ),
        _ => error("upstream_unavailable", failure.to_string(), OperationErrorStep::Download),
    }
}

fn download_failed(err: crate::loaders::LoaderError) -> OperationError {
    match err {
        crate::loaders::LoaderError::Damaged { .. } => {
            error("checksum_mismatch", err.to_string(), OperationErrorStep::Download)
        }
        crate::loaders::LoaderError::Write { .. } => {
            error("no_space", err.to_string(), OperationErrorStep::Filesystem)
        }
        other => error("upstream_unavailable", other.to_string(), OperationErrorStep::Download),
    }
}

fn filesystem(err: std::io::Error) -> OperationError {
    let out_of_room = matches!(err.raw_os_error(), Some(28) | Some(122));
    let code = if out_of_room { "no_space" } else { "loader_install_failed" };
    error(code, err.to_string(), OperationErrorStep::Filesystem)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::harness::a_dir;

    #[test]
    fn a_swap_inside_a_family_keeps_the_content_and_one_across_it_does_not() {
        assert_eq!(
            check_change(Some(LoaderId::Paper), LoaderId::Purpur, ContentPolicy::Keep).unwrap(),
            []
        );
        assert_eq!(
            check_change(Some(LoaderId::Fabric), LoaderId::Quilt, ContentPolicy::Keep).unwrap(),
            []
        );

        let refusal =
            check_change(Some(LoaderId::Paper), LoaderId::Fabric, ContentPolicy::Keep).unwrap_err();
        assert_eq!(refusal.code(), "loader_change_needs_wipe");
        assert_eq!(refusal.status(), axum::http::StatusCode::CONFLICT);

        assert!(check_change(Some(LoaderId::Paper), LoaderId::Fabric, ContentPolicy::WipeMods)
            .is_ok());
    }

    #[test]
    fn a_move_to_or_from_a_proxy_warns_that_the_properties_stop_being_read() {
        assert_eq!(
            check_change(Some(LoaderId::Paper), LoaderId::Velocity, ContentPolicy::WipeMods)
                .unwrap(),
            [ServerWarning::PropertiesWillBeIgnored]
        );
        assert_eq!(
            check_change(Some(LoaderId::Velocity), LoaderId::Paper, ContentPolicy::WipeMods)
                .unwrap(),
            [ServerWarning::PropertiesWillBeIgnored]
        );
        assert_eq!(
            check_change(Some(LoaderId::Velocity), LoaderId::Velocity, ContentPolicy::Keep)
                .unwrap(),
            []
        );
        assert_eq!(
            check_change(None, LoaderId::Velocity, ContentPolicy::Keep).unwrap(),
            [],
            "a server without a loader is not changing away from one"
        );
    }

    #[test]
    fn wiping_mods_moves_them_aside_and_leaves_the_world_alone() {
        let dir = a_dir();
        std::fs::create_dir_all(dir.path().join("mods")).unwrap();
        std::fs::create_dir_all(dir.path().join("plugins")).unwrap();
        std::fs::create_dir_all(dir.path().join("world")).unwrap();
        std::fs::write(dir.path().join("mods").join("a.jar"), "x").unwrap();
        std::fs::write(dir.path().join("world").join("level.dat"), "x").unwrap();

        put_aside(dir.path()).unwrap();

        assert!(!dir.path().join("mods").exists());
        assert!(!dir.path().join("plugins").exists());
        assert!(dir.path().join("world").join("level.dat").is_file(), "9.14: the world stays");

        let saved: Vec<String> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|name| name.starts_with("mods.before-"))
            .collect();
        assert_eq!(saved.len(), 1, "the old mods are put by, not thrown away");
    }

    #[test]
    fn a_run_will_not_start_where_the_server_directory_is_a_link() {
        let root = a_dir();
        let theirs = root.path().join("users").join("01THEM").join("servers").join("01SERVER");
        std::fs::create_dir_all(theirs.join("mods")).unwrap();

        let mine = root.path().join("users").join("01ME").join("servers");
        std::fs::create_dir_all(&mine).unwrap();
        std::os::unix::fs::symlink(&theirs, mine.join("01SERVER")).unwrap();

        let refusal = super::super::disk::ensure_reachable(&mine.join("01SERVER")).unwrap_err();
        assert!(refusal.to_string().contains("symbolic link"), "{refusal}");
        assert!(theirs.join("mods").is_dir(), "and nothing of theirs moved");
    }

    #[test]
    fn a_reset_takes_the_whole_directory_and_leaves_it_there_and_empty() {
        let dir = a_dir();
        std::fs::create_dir_all(dir.path().join("world")).unwrap();
        std::fs::write(dir.path().join("server.properties"), "motd=x\n").unwrap();

        clear(dir.path()).unwrap();

        assert!(dir.path().is_dir());
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[test]
    fn every_kind_of_run_is_the_one_section_five_names_for_it() {
        let plan = Plan {
            loader: LoaderId::Paper,
            game_version: "1.21.8".to_owned(),
            build: None,
            policy: ContentPolicy::Keep,
        };
        assert_eq!(
            Job::Install { plan: plan.clone(), wipe_everything: false }.kind(),
            OperationKind::InstallLoader
        );
        assert_eq!(
            Job::Install { plan, wipe_everything: true }.kind(),
            OperationKind::ResetServer
        );
        assert_eq!(Job::Repair.kind(), OperationKind::RepairContent);

        for kind in [
            OperationKind::InstallLoader,
            OperationKind::ResetServer,
            OperationKind::RepairContent,
        ] {
            assert!(!kind.allows_running_server(), "{kind} needs the server stopped (5.8)");
            assert!(kind.busy_reason().is_some(), "{kind} locks the server (5.8)");
        }
    }
}
