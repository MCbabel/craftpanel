#![allow(dead_code)]

pub(crate) mod archive;
mod quiesce;
mod schedule;
pub(crate) mod store;
#[cfg(test)]
pub mod testing;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use sqlx::SqlitePool;

use crate::auth::disk::{self, Disks};
use crate::auth::error::{Failure, Result};
use crate::drive::Drive;
use crate::helper::Helper;
use crate::model::{
    Backup, BackupLocation, BackupOperationState, BackupOperationType, BackupSchedule,
    BackupStatus, DriveFileState, Id, LoaderId, OperationError, OperationErrorStep, OperationKind,
    OperationState, Timestamp,
};
use crate::ops::{Fault, NewOperation, Operations, Step, WsMessage};
use crate::servers::Hub;

pub use schedule::UpdateBackupScheduleRequest;
pub use store::BackupListResponse;

pub const MAX_NAME: usize = 128;
pub const SAFETY_NAME: usize = 92;
pub const COOLDOWN: Duration = Duration::from_secs(60);
const HEADROOM: f64 = 1.1;

const QUEUE_POLL: Duration = Duration::from_millis(200);
const PROGRESS_POLL: Duration = Duration::from_millis(300);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Queued {
    pub backup: Id,
    pub operation: Id,
}

#[derive(Debug, Clone, Copy, serde::Serialize)]
pub struct RestoreAccepted {
    pub restore_operation_id: Id,
    pub safety_backup: SafetyRef,
}

#[derive(Debug, Clone, Copy, serde::Serialize)]
pub struct SafetyRef {
    pub id: Id,
    pub create_operation_id: Id,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct BulkFailure {
    pub id: Id,
    pub error: &'static str,
    pub message: String,
}

#[derive(Debug, Clone, Copy, serde::Serialize)]
pub struct RetryAccepted {
    pub operation_id: Id,
    pub operation_type: BackupOperationType,
}

#[derive(Debug)]
pub struct Download {
    pub path: PathBuf,
    pub name: String,
    pub created_at: Timestamp,
    pub size_bytes: u64,
}

pub struct Backups {
    pool: SqlitePool,
    data_dir: PathBuf,
    operations: Arc<Operations>,
    hub: Arc<Hub>,
    helper: Helper,
    disks: Disks,
    drive: Arc<Drive>,
    warned: std::sync::Mutex<std::collections::BTreeSet<Id>>,
}

impl Backups {
    pub fn new(
        pool: SqlitePool,
        data_dir: impl Into<PathBuf>,
        operations: Arc<Operations>,
        hub: Arc<Hub>,
        helper: Helper,
        disks: Disks,
        drive: Arc<Drive>,
    ) -> Arc<Self> {
        Arc::new(Self {
            pool,
            data_dir: data_dir.into(),
            operations,
            hub,
            helper,
            disks,
            drive,
            warned: std::sync::Mutex::default(),
        })
    }

    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    pub fn drive(&self) -> &Arc<Drive> {
        &self.drive
    }

    pub fn root(&self) -> PathBuf {
        self.data_dir.join("backups")
    }

    pub fn dir_of(&self, server: Id) -> PathBuf {
        self.root().join(server.to_string())
    }

    pub fn archive_of(&self, server: Id, backup: Id) -> PathBuf {
        self.dir_of(server).join(format!("{backup}.tar.zst"))
    }

    fn make_dir(&self, server: Id) -> std::io::Result<PathBuf> {
        let dir = self.dir_of(server);
        std::fs::create_dir_all(&dir)?;
        shut(&self.root())?;
        shut(&dir)?;
        Ok(dir)
    }

    pub async fn server_dir(&self, server: Id) -> Result<PathBuf> {
        Ok(self.operations.server_dir(self.owner_of(server).await?, server))
    }

    async fn owner_of(&self, server: Id) -> Result<Id> {
        sqlx::query_scalar::<_, Id>("SELECT owner_id FROM servers WHERE id = ?")
            .bind(server)
            .fetch_optional(&self.pool)
            .await?
            .ok_or_else(unknown_server)
    }

    async fn name_of(&self, server: Id) -> String {
        sqlx::query_scalar::<_, String>("SELECT name FROM servers WHERE id = ?")
            .bind(server)
            .fetch_optional(&self.pool)
            .await
            .ok()
            .flatten()
            .unwrap_or_else(|| server.to_string())
    }

    async fn loader_of(&self, server: Id) -> Result<Option<LoaderId>> {
        Ok(sqlx::query_scalar::<_, Option<LoaderId>>("SELECT loader FROM servers WHERE id = ?")
            .bind(server)
            .fetch_optional(&self.pool)
            .await?
            .flatten())
    }

    pub async fn list(&self, server: Id) -> Result<BackupListResponse> {
        store::list(&self.pool, server).await
    }

    pub async fn one(&self, server: Id, backup: Id) -> Result<Backup> {
        self.mine(server, backup).await?;
        store::one(&self.pool, backup).await
    }

    async fn mine(&self, server: Id, backup: Id) -> Result<store::Row> {
        let row = store::find(&self.pool, backup).await?;
        if row.server_id != server {
            return Err(Failure::not_found("backup_not_found", "no such backup"));
        }
        Ok(row)
    }

    pub async fn server_of(&self, backup: Id) -> Result<Id> {
        Ok(store::find(&self.pool, backup).await?.server_id)
    }

    pub async fn download(&self, server: Id, backup: Id) -> Result<Download> {
        let row = self.mine(server, backup).await?;
        if store::one(&self.pool, backup).await?.status != BackupStatus::Done {
            return Err(Failure::conflict(
                "backup_not_downloadable",
                "this backup is not finished",
            ));
        }
        if row.location == BackupLocation::Drive {
            let link = row
                .drive_file_id
                .as_deref()
                .map(crate::drive::files::web_link)
                .unwrap_or_else(|| "your Google Drive".to_owned());
            return Err(Failure::conflict(
                "backup_lives_in_drive",
                format!("this backup lies in your own Google Drive: {link}"),
            ));
        }
        let path = self.archive_of(row.server_id, row.id);
        let size_bytes = tokio::fs::metadata(&path)
            .await
            .map_err(|err| Failure::internal(anyhow::Error::from(err)))?
            .len();
        Ok(Download { path, name: row.name, created_at: row.created_at, size_bytes })
    }

    pub async fn quota(&self) -> Result<u32> {
        store::quota(&self.pool).await
    }

    pub async fn used_quota(&self, server: Id) -> Result<u32> {
        store::count(&self.pool, server).await
    }

    pub async fn schedule(&self, server: Id) -> Result<BackupSchedule> {
        store::schedule(&self.pool, server).await
    }

    pub async fn write_schedule(
        &self,
        server: Id,
        wanted: UpdateBackupScheduleRequest,
    ) -> Result<BackupSchedule> {
        let quota = store::quota(&self.pool).await?;
        wanted.check(quota)?;
        let next = wanted
            .enabled
            .then(|| schedule::next_after(Timestamp::now(), wanted.interval_hours, wanted.hour_utc));
        store::save_schedule(&self.pool, server, &wanted.into_schedule(next)).await
    }

    pub async fn cooldown(&self, server: Id) -> Result<Option<u64>> {
        let Some(last) = store::newest_manual_request(&self.pool, server).await? else {
            return Ok(None);
        };
        let waited = Timestamp::now().as_datetime() - last.as_datetime();
        let waited = Duration::try_from(waited).unwrap_or_default();
        Ok(COOLDOWN.checked_sub(waited).filter(|left| !left.is_zero()).map(|left| left.as_secs() + 1))
    }

    pub async fn request(self: &Arc<Self>, server: Id, name: &str, by: Id) -> Result<Backup> {
        let name = check_name(name)?;
        self.operations.guard_write(server).await.map_err(relay)?;
        self.drive.guard_backup(server).await?;
        self.check_quota(server, 1).await?;
        self.check_space(server).await?;

        let queued = self.create(server, &name, Some(by), false).await?;
        let made = store::one(&self.pool, queued.backup).await?;
        self.spawn(queued.operation);
        self.announce(server).await;
        Ok(made)
    }

    pub async fn create(
        &self,
        server: Id,
        name: &str,
        by: Option<Id>,
        automated: bool,
    ) -> Result<Queued> {
        self.create_for(server, name, by, automated, None).await
    }

    async fn create_for(
        &self,
        server: Id,
        name: &str,
        by: Option<Id>,
        automated: bool,
        parent: Option<Id>,
    ) -> Result<Queued> {
        let location = self.drive.effective_target(server).await.unwrap_or(BackupLocation::Local);
        let row = store::insert(&self.pool, server, name, automated, location).await?;
        let mut new = NewOperation::new(server, OperationKind::BackupCreate, by);
        new.target_id = Some(row.id);
        new.parent_operation_id = parent;

        match self.operations.create(new).await {
            Ok(operation) => Ok(Queued { backup: row.id, operation: operation.id }),
            Err(fault) => {
                store::remove(&self.pool, row.id).await.ok();
                Err(relay(fault))
            }
        }
    }

    async fn check_quota(&self, server: Id, wanted: u32) -> Result<()> {
        let quota = store::quota(&self.pool).await?;
        if store::count(&self.pool, server).await? + wanted > quota {
            return Err(Failure::conflict(
                "backup_limit_reached",
                format!("this server may keep {quota} backups"),
            ));
        }
        Ok(())
    }

    async fn check_space(&self, server: Id) -> Result<()> {
        let owner = self.owner_of(server).await?;
        let directory = self.server_dir(server).await?;
        let backups = self.root();
        let answer = tokio::task::spawn_blocking(move || {
            std::fs::create_dir_all(&backups)?;
            shut(&backups)?;
            let plan = archive::survey(&directory)?;
            let free = archive::free_bytes(&backups)?;
            anyhow::Ok((plan.bytes, free))
        })
        .await
        .map_err(|err| Failure::internal(anyhow::anyhow!("{err}")))?
        .map_err(Failure::internal)?;

        let (needed, free) = answer;
        if (free as f64) < needed as f64 * HEADROOM {
            return Err(Failure::new(
                axum::http::StatusCode::INSUFFICIENT_STORAGE,
                "no_space",
                format!(
                    "{needed} bytes to pack and only {free} free — a backup into Google Drive \
                     is built here first as well"
                ),
            ));
        }
        disk::guard(&self.pool, &self.disks, owner, needed).await
    }

    pub async fn rename(&self, server: Id, backup: Id, name: &str) -> Result<Backup> {
        self.mine(server, backup).await?;
        let name = check_name(name)?;
        self.guard_backup(backup).await?;
        store::rename(&self.pool, backup, &name).await?;
        self.announce(server).await;
        store::one(&self.pool, backup).await
    }

    pub async fn delete(&self, server: Id, backup: Id) -> Result<()> {
        self.mine(server, backup).await?;
        self.guard_backup(backup).await?;
        self.forget(server, backup).await?;
        self.announce(server).await;
        Ok(())
    }

    pub async fn delete_many(&self, server: Id, ids: &[Id]) -> (Vec<Id>, Vec<BulkFailure>) {
        let mut deleted = Vec::new();
        let mut failed = Vec::new();
        for backup in ids.iter().copied() {
            match self.delete_one(server, backup).await {
                Ok(()) => deleted.push(backup),
                Err(failure) => failed.push(failure),
            }
        }
        if !deleted.is_empty() {
            self.announce(server).await;
        }
        (deleted, failed)
    }

    async fn delete_one(
        &self,
        server: Id,
        backup: Id,
    ) -> std::result::Result<(), BulkFailure> {
        let refuse = |error, message: String| Err(BulkFailure { id: backup, error, message });
        match store::find(&self.pool, backup).await {
            Ok(row) if row.server_id == server => {}
            _ => return refuse("backup_not_found", "no such backup".to_owned()),
        }
        match store::is_busy(&self.pool, backup).await {
            Ok(false) => {}
            Ok(true) => {
                return refuse("server_busy", "a run on this backup is not finished".to_owned())
            }
            Err(err) => return refuse("internal", err.to_string()),
        }
        match self.forget(server, backup).await {
            Ok(()) => Ok(()),
            Err(err) => refuse("internal", err.to_string()),
        }
    }

    async fn forget(&self, server: Id, backup: Id) -> Result<()> {
        let known = store::find(&self.pool, backup).await.ok();
        store::remove(&self.pool, backup).await?;

        if let Some(file_id) = known
            .as_ref()
            .filter(|row| row.location == BackupLocation::Drive)
            .and_then(|row| row.drive_file_id.clone())
        {
            self.drive.remove_archive(server, &file_id).await;
        }

        let path = self.archive_of(server, backup);
        crate::drive::drop_the_part(&with_suffix(&path, ".part")).await;
        if let Err(err) = tokio::fs::remove_file(&path).await {
            if err.kind() != std::io::ErrorKind::NotFound {
                tracing::warn!("{} stays behind: {err}", path.display());
            }
        }
        Ok(())
    }

    async fn guard_backup(&self, backup: Id) -> Result<()> {
        if store::is_busy(&self.pool, backup).await? {
            return Err(Failure::conflict("server_busy", "a run on this backup is not finished"));
        }
        Ok(())
    }

    pub async fn restore(
        self: &Arc<Self>,
        server: Id,
        backup: Id,
        safety_name: &str,
        by: Id,
    ) -> Result<RestoreAccepted> {
        self.mine(server, backup).await?;
        let name = check_name(safety_name)?;

        if quiesce::running_link(&self.hub, server).await.is_some() {
            return Err(Failure::conflict("server_running", "stop the server before restoring"));
        }
        if store::one(&self.pool, backup).await?.status != BackupStatus::Done {
            return Err(Failure::conflict(
                "backup_not_restorable",
                "this backup is not finished",
            ));
        }
        let source = self.mine(server, backup).await?;
        match source.drive_state {
            Some(DriveFileState::Missing) => {
                return Err(Failure::conflict(
                    "backup_not_restorable",
                    "this backup is no longer in your Google Drive",
                ))
            }
            Some(DriveFileState::Trashed) => {
                return Err(Failure::conflict(
                    "backup_not_restorable",
                    "this backup is in the bin of your Google Drive; put it back there first",
                ))
            }
            _ => {}
        }
        if source.drive_content_changed_at.is_some() {
            return Err(Failure::conflict(
                "backup_not_restorable",
                "the file of this backup in your Google Drive is no longer the archive the panel \
                 put there; whatever lies under it now is not this backup and will not be \
                 unpacked over your world",
            ));
        }
        self.operations.guard_write(server).await.map_err(relay)?;
        self.check_quota(server, 1).await?;
        self.check_space(server).await?;

        let mut new = NewOperation::new(server, OperationKind::BackupRestore, Some(by));
        new.target_id = Some(backup);
        let restore = self.operations.create(new).await.map_err(relay)?;

        let safety = match self.create_for(server, &name, Some(by), false, Some(restore.id)).await {
            Ok(safety) => safety,
            Err(failure) => {
                let _ = self.operations.cancelled(restore.id).await;
                return Err(failure);
            }
        };
        self.dismiss(server, safety.operation).await;

        self.spawn(restore.id);
        self.announce(server).await;
        Ok(RestoreAccepted {
            restore_operation_id: restore.id,
            safety_backup: SafetyRef { id: safety.backup, create_operation_id: safety.operation },
        })
    }

    pub async fn retry(
        self: &Arc<Self>,
        server: Id,
        backup: Id,
        by: Id,
        acknowledge_abuse: bool,
    ) -> Result<RetryAccepted> {
        let row = self.mine(server, backup).await?;
        let last = store::newest_run(&self.pool, backup)
            .await?
            .ok_or_else(|| Failure::conflict("nothing_to_retry", "this backup has no run"))?;
        if !matches!(
            last.state,
            BackupOperationState::Failed | BackupOperationState::TimedOut
        ) {
            return Err(Failure::conflict("nothing_to_retry", "the last run did not fail"));
        }
        self.operations.guard_write(server).await.map_err(relay)?;

        match last.operation_type {
            BackupOperationType::Create => {
                let path = self.archive_of(server, backup);
                if self.drive.resumable(backup, &path, Timestamp::now()).await.is_none() {
                    self.check_space(server).await?;
                    self.drive.forget_session(backup).await;
                    tokio::fs::remove_file(&path).await.ok();
                }

                let mut new = NewOperation::new(server, OperationKind::BackupCreate, Some(by));
                new.target_id = Some(backup);
                let operation = self.operations.create(new).await.map_err(relay)?;
                self.spawn(operation.id);
                self.announce(server).await;
                Ok(RetryAccepted {
                    operation_id: operation.id,
                    operation_type: BackupOperationType::Create,
                })
            }
            BackupOperationType::Restore => {
                if quiesce::running_link(&self.hub, server).await.is_some() {
                    return Err(Failure::conflict(
                        "server_running",
                        "stop the server before restoring",
                    ));
                }

                let existing = store::safety_copy_for(&self.pool, backup).await?;
                let usable = match existing {
                    Some(copy) => store::one(&self.pool, copy).await?.status == BackupStatus::Done,
                    None => false,
                };
                if !usable {
                    self.check_quota(server, 1).await?;
                    self.check_space(server).await?;
                }

                let mut new = NewOperation::new(server, OperationKind::BackupRestore, Some(by));
                new.target_id = Some(backup);
                let restore = self.operations.create(new).await.map_err(relay)?;

                if !usable {
                    let name = safety_name_for(&row.name);
                    match self.create_for(server, &name, Some(by), false, Some(restore.id)).await {
                        Ok(safety) => self.dismiss(server, safety.operation).await,
                        Err(failure) => {
                            let _ = self.operations.cancelled(restore.id).await;
                            return Err(failure);
                        }
                    }
                }
                if acknowledge_abuse {
                    self.warned.lock().expect("the abuse lock").insert(restore.id);
                }

                self.spawn(restore.id);
                self.announce(server).await;
                Ok(RetryAccepted {
                    operation_id: restore.id,
                    operation_type: BackupOperationType::Restore,
                })
            }
        }
    }

    pub fn spawn(self: &Arc<Self>, operation: Id) {
        let backups = Arc::clone(self);
        tokio::spawn(async move { backups.run(operation).await });
    }

    pub async fn run(self: &Arc<Self>, operation: Id) {
        let Ok(run) = self.operations.get(operation).await else { return };
        match run.kind {
            OperationKind::BackupCreate => self.run_create(operation).await,
            OperationKind::BackupRestore => self.run_restore(operation).await,
            other => tracing::error!("{other} is not a backup run"),
        }
    }

    async fn run_create(self: &Arc<Self>, id: Id) {
        let Ok(run) = self.operations.get(id).await else { return };
        let (server, backup) = (run.server_id, run.target_id);
        let Some(backup) = backup else {
            self.blame(id, "internal", "a create run without a backup").await;
            return;
        };

        match self.wait_for_a_turn(id).await {
            Turn::Taken => {}
            Turn::CalledOff => {
                self.forget(server, backup).await.ok();
                self.announce(server).await;
                return;
            }
            Turn::Gone => return,
        }

        let archive = self.archive_of(server, backup);
        let carried = self.drive.resumable(backup, &archive, Timestamp::now()).await;
        if let Some(size) = carried {
            tracing::info!(
                %server, %backup, size,
                "the archive of an interrupted upload is still here and is being carried on \
                 instead of packed again"
            );
        }

        let outcome = match carried {
            Some(size) => self.deliver(id, server, backup, size).await.map(|_| ()),
            None => match self.pack(id, server, backup).await {
                Ok(size) => self.deliver(id, server, backup, size).await.map(|_| ()),
                Err(other) => Err(other),
            },
        };

        match outcome {
            Ok(()) => {
                let _ = self.operations.finish(id).await;
            }
            Err(Ended::CalledOff) => {
                self.drive.forget_session(backup).await;
                tokio::fs::remove_file(&archive).await.ok();
                self.forget(server, backup).await.ok();
                let _ = self.operations.cancelled(id).await;
            }
            Err(Ended::Failed(error)) => {
                self.drive.forget_session(backup).await;
                tokio::fs::remove_file(&archive).await.ok();
                let _ = self.operations.fail(id, error).await;
            }
        }
        self.announce(server).await;
    }

    async fn deliver(
        self: &Arc<Self>,
        id: Id,
        server: Id,
        backup: Id,
        size: u64,
    ) -> std::result::Result<(), Ended> {
        let row = store::find(&self.pool, backup).await.map_err(|_| Ended::gone())?;
        if row.location == BackupLocation::Local {
            store::set_size(&self.pool, backup, size).await.ok();
            return Ok(());
        }

        let archive = self.archive_of(server, backup);
        let name = drive_name(&self.name_of(server).await, &row);
        let progress = Arc::new(archive::Progress::default());
        let watcher = self.watch_between(id, Arc::clone(&progress), size, 0.5, 0.99);

        let uploaded = self
            .drive
            .upload_archive(server, backup, &archive, size, &name, &progress)
            .await;
        watcher.abort();

        let stored = match uploaded {
            Ok(stored) => stored,
            Err(crate::drive::http::DriveError::Cancelled) => return Err(Ended::CalledOff),
            Err(err) => return Err(Ended::drive(&err)),
        };

        store::finish_upload(
            &self.pool,
            backup,
            &stored.file_id,
            size,
            stored.md5.as_deref(),
            Timestamp::now(),
        )
        .await
        .map_err(|_| Ended::gone())?;
        if let Err(err) = tokio::fs::remove_file(&archive).await {
            if err.kind() != std::io::ErrorKind::NotFound {
                tracing::warn!("{} stays behind after the upload: {err}", archive.display());
            }
        }
        Ok(())
    }

    async fn pack(
        self: &Arc<Self>,
        id: Id,
        server: Id,
        backup: Id,
    ) -> std::result::Result<u64, Ended> {
        let owner = self.owner_of(server).await.map_err(|_| Ended::gone())?;
        let directory = self.operations.server_dir(owner, server);
        let loader = self.loader_of(server).await.unwrap_or_default();
        self.make_dir(server).map_err(|err| Ended::io(&err))?;
        let target = self.archive_of(server, backup);

        let held = quiesce::Held::take(&self.hub, server, loader).await;
        if held.as_ref().is_some_and(|held| !held.confirmed) {
            let warning = quiesce::Held::warning(server);
            let _ = self.operations.console(server, &[warning]).await;
        }

        let steps = crate::helper::in_servers(server);
        if let Err(err) = self.helper.chown_tree(&owner.to_string(), steps).await {
            tracing::warn!(%server, "the tree could not be made readable before packing: {err:#}");
        }

        let survey = {
            let directory = directory.clone();
            tokio::task::spawn_blocking(move || archive::survey(&directory)).await
        };
        let plan = match survey {
            Ok(Ok(plan)) => plan,
            Ok(Err(err)) => return Err(Ended::from_anyhow(&err)),
            Err(join) => return Err(Ended::thread(&join)),
        };

        let progress = Arc::new(archive::Progress::default());
        let watcher = self.watch(id, Arc::clone(&progress), plan.bytes);

        let packing = {
            let progress = Arc::clone(&progress);
            let directory = directory.clone();
            let target = target.clone();
            tokio::task::spawn_blocking(move || {
                archive::pack(&directory, &plan, &target, &progress)
            })
            .await
        };
        watcher.abort();
        drop(held);

        match packing {
            Ok(Ok(size)) => {
                let _ = self
                    .operations
                    .advance(id, Step { progress: Some(1.0), ..Step::default() })
                    .await;
                Ok(size)
            }
            Ok(Err(err)) if err.downcast_ref::<archive::Cancelled>().is_some() => {
                Err(Ended::CalledOff)
            }
            Ok(Err(err)) => Err(Ended::from_anyhow(&err)),
            Err(join) => Err(Ended::thread(&join)),
        }
    }

    async fn run_restore(self: &Arc<Self>, id: Id) {
        let Ok(run) = self.operations.get(id).await else { return };
        let (server, source) = (run.server_id, run.target_id);
        let Some(source) = source else {
            self.blame(id, "internal", "a restore run without a backup").await;
            return;
        };

        if let Some(safety) = self.safety_run_of(id).await {
            self.run_create(safety).await;
            let done = self
                .operations
                .get(safety)
                .await
                .map(|run| run.state == OperationState::Done)
                .unwrap_or(false);
            if !done {
                self.blame(id, "safety_backup_failed", "the safety copy could not be made").await;
                self.announce(server).await;
                return;
            }
        }

        match self.wait_for_a_turn(id).await {
            Turn::Taken => {}
            Turn::CalledOff | Turn::Gone => {
                self.announce(server).await;
                return;
            }
        }

        match self.unroll(id, server, source).await {
            Ok(()) => {
                let _ = self.operations.finish(id).await;
            }
            Err(Ended::CalledOff) => {
                let _ = self.operations.cancelled(id).await;
            }
            Err(Ended::Failed(error)) => {
                let _ = self.operations.fail(id, error).await;
            }
        }
        self.warned.lock().expect("the abuse lock").remove(&id);
        self.announce(server).await;
    }

    async fn unroll(
        self: &Arc<Self>,
        id: Id,
        server: Id,
        backup: Id,
    ) -> std::result::Result<(), Ended> {
        let owner = self.owner_of(server).await.map_err(|_| Ended::gone())?;
        let directory = self.operations.server_dir(owner, server);
        let archive = self.archive_of(server, backup);
        let fresh = with_suffix(&directory, &format!(".restoring-{id}"));
        let previous = with_suffix(&directory, &format!(".old-{id}"));

        self.clear_leftovers(owner, server, &directory).await;

        let from_drive = self.bring_down(id, server, backup, &archive).await?;

        let compressed = tokio::fs::metadata(&archive).await.map(|meta| meta.len()).unwrap_or(0);
        let progress = Arc::new(archive::Progress::default());
        let floor = if from_drive { 0.4 } else { 0.0 };
        let watcher = self.watch_between(id, Arc::clone(&progress), compressed, floor, 0.99);
        let unpacking = {
            let progress = Arc::clone(&progress);
            let archive = archive.clone();
            let fresh = fresh.clone();
            tokio::task::spawn_blocking(move || archive::unpack(&archive, &fresh, &progress)).await
        };
        watcher.abort();

        match unpacking {
            Ok(Ok(())) => {}
            Ok(Err(err)) if err.downcast_ref::<archive::Cancelled>().is_some() => {
                self.clear_or_say(owner, server, &fresh).await;
                return Err(Ended::CalledOff);
            }
            Ok(Err(err)) => {
                self.clear_or_say(owner, server, &fresh).await;
                return Err(Ended::unpacking(&err));
            }
            Err(join) => {
                self.clear_or_say(owner, server, &fresh).await;
                return Err(Ended::thread(&join));
            }
        }

        let steps = crate::helper::in_servers(format!("{server}.restoring-{id}"));
        if let Err(err) = self.helper.chown_tree(&owner.to_string(), steps).await {
            self.clear_or_say(owner, server, &fresh).await;
            return Err(Ended::Failed(OperationError {
                code: "internal".to_owned(),
                message: format!("the restored files could not be handed back: {err:#}"),
                step: OperationErrorStep::Filesystem,
            }));
        }

        let moved_away = directory.exists();
        if moved_away {
            tokio::fs::rename(&directory, &previous).await.map_err(|err| Ended::io(&err))?;
        }
        if let Err(err) = tokio::fs::rename(&fresh, &directory).await {
            if moved_away {
                tokio::fs::rename(&previous, &directory).await.ok();
            }
            return Err(Ended::io(&err));
        }
        self.clear_or_say(owner, server, &previous).await;
        if from_drive {
            tokio::fs::remove_file(&archive).await.ok();
        }
        Ok(())
    }

    async fn clear_leftovers(&self, owner: Id, server: Id, directory: &Path) {
        let Some(parent) = directory.parent() else { return };
        let Ok(mut entries) = tokio::fs::read_dir(parent).await else { return };
        let (restoring, old) = (format!("{server}.restoring"), format!("{server}.old"));
        let stands = directory.exists();

        while let Ok(Some(entry)) = entries.next_entry().await {
            let name = entry.file_name();
            let Some(name) = name.to_str() else { continue };
            if name.starts_with(&restoring) || (stands && name.starts_with(&old)) {
                self.clear_or_say(owner, server, &entry.path()).await;
            }
        }
    }

    async fn clear_or_say(&self, owner: Id, server: Id, path: &Path) {
        let Err(err) = self.clear(owner, path).await else { return };
        let line = format!(
            "[craftpanel] {} could not be removed and goes on taking up disk: {err}",
            path.display()
        );
        tracing::warn!(%server, "{line}");
        let _ = self.operations.console(server, &[line]).await;
    }

    async fn clear(&self, owner: Id, path: &Path) -> std::io::Result<()> {
        match tokio::fs::remove_dir_all(path).await {
            Err(first) if first.kind() != std::io::ErrorKind::NotFound => {
                let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
                    return Err(first);
                };
                let steps = crate::helper::in_servers(name);
                if self.helper.chown_tree(&owner.to_string(), steps).await.is_err() {
                    return Err(first);
                }
                match tokio::fs::remove_dir_all(path).await {
                    Err(err) if err.kind() != std::io::ErrorKind::NotFound => Err(err),
                    _ => Ok(()),
                }
            }
            _ => Ok(()),
        }
    }

    async fn bring_down(
        self: &Arc<Self>,
        id: Id,
        server: Id,
        backup: Id,
        archive: &Path,
    ) -> std::result::Result<bool, Ended> {
        let row = store::find(&self.pool, backup).await.map_err(|_| Ended::gone())?;
        if row.location == BackupLocation::Local {
            return Ok(false);
        }
        let Some(file_id) = row.drive_file_id.clone() else {
            return Err(Ended::Failed(OperationError {
                code: "drive_file_missing".to_owned(),
                message: "this backup never finished its upload".to_owned(),
                step: OperationErrorStep::Filesystem,
            }));
        };

        let known = self.drive.size_of(server, &file_id).await.map_err(|err| Ended::drive(&err))?;
        if known.trashed {
            return Err(Ended::Failed(OperationError {
                code: "drive_file_missing".to_owned(),
                message: "this backup is in the bin of your Google Drive".to_owned(),
                step: OperationErrorStep::Filesystem,
            }));
        }
        self.room_for(server, known.bytes().unwrap_or(0)).await?;

        self.make_dir(server).map_err(|err| Ended::io(&err))?;
        let part = with_suffix(archive, ".part");
        let progress = Arc::new(archive::Progress::default());
        let watcher =
            self.watch_between(id, Arc::clone(&progress), known.bytes().unwrap_or(0), 0.0, 0.39);
        let warned = self.warned.lock().expect("the abuse lock").remove(&id);
        let recorded = crate::drive::Recorded {
            bytes: (row.size_bytes > 0).then(|| row.size_bytes as u64),
            md5: row.drive_md5.as_deref(),
        };
        let brought = self
            .drive
            .fetch_archive(server, &file_id, &part, &progress, recorded, warned)
            .await;
        watcher.abort();

        match brought {
            Ok(_) => {}
            Err(crate::drive::http::DriveError::Cancelled) => {
                crate::drive::drop_the_part(&part).await;
                return Err(Ended::CalledOff);
            }
            Err(err) => {
                if !worth_carrying_on(&err) {
                    crate::drive::drop_the_part(&part).await;
                }
                return Err(Ended::drive(&err));
            }
        }

        tokio::fs::rename(&part, archive).await.map_err(|err| Ended::io(&err))?;
        Ok(true)
    }

    async fn room_for(&self, server: Id, archive_bytes: u64) -> std::result::Result<(), Ended> {
        let owner = self.owner_of(server).await.map_err(|_| Ended::gone())?;
        let needed = archive_bytes.saturating_mul(5);
        let backups = self.root();
        let free = tokio::task::spawn_blocking(move || {
            std::fs::create_dir_all(&backups)?;
            archive::free_bytes(&backups)
        })
        .await
        .map_err(|join| Ended::thread(&join))?
        .map_err(|err| Ended::from_anyhow(&err))?;

        if (free as f64) < needed as f64 * HEADROOM {
            return Err(Ended::Failed(OperationError {
                code: "no_space".to_owned(),
                message: format!("{needed} bytes to bring the backup back and only {free} free"),
                step: OperationErrorStep::Filesystem,
            }));
        }
        if let Err(failure) = disk::guard(&self.pool, &self.disks, owner, needed).await {
            return Err(Ended::Failed(OperationError {
                code: failure.code().to_owned(),
                message: "this account has no room left for the archive".to_owned(),
                step: OperationErrorStep::Filesystem,
            }));
        }
        Ok(())
    }

    async fn safety_run_of(&self, restore: Id) -> Option<Id> {
        sqlx::query_scalar::<_, Id>(
            "SELECT id FROM operations \
              WHERE parent_operation_id = ? AND kind = 'backup_create' \
              ORDER BY created_at DESC LIMIT 1",
        )
        .bind(restore)
        .fetch_optional(&self.pool)
        .await
        .ok()
        .flatten()
    }

    fn watch(
        self: &Arc<Self>,
        id: Id,
        progress: Arc<archive::Progress>,
        total: u64,
    ) -> tokio::task::JoinHandle<()> {
        self.watch_between(id, progress, total, 0.0, 0.99)
    }

    fn watch_between(
        self: &Arc<Self>,
        id: Id,
        progress: Arc<archive::Progress>,
        total: u64,
        floor: f64,
        ceiling: f64,
    ) -> tokio::task::JoinHandle<()> {
        let backups = Arc::clone(self);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(PROGRESS_POLL).await;
                if backups.operations.cancel_requested(id).await.unwrap_or(false) {
                    progress.cancel();
                }
                let step = Step {
                    bytes_processed: Some(progress.bytes()),
                    files_processed: Some(progress.files()),
                    message: Some(progress.holdup().unwrap_or_default()),
                    progress: (total > 0).then(|| {
                        let share = (progress.done() as f64 / total as f64).clamp(0.0, 1.0);
                        (floor + share * (ceiling - floor)).clamp(floor, ceiling)
                    }),
                    ..Step::default()
                };
                if backups.operations.advance(id, step).await.is_err() {
                    return;
                }
            }
        })
    }

    async fn wait_for_a_turn(&self, id: Id) -> Turn {
        loop {
            match self.operations.begin(id).await {
                Ok(Some(_)) => return Turn::Taken,
                Ok(None) => {}
                Err(_) => return Turn::Gone,
            }
            match self.operations.get(id).await {
                Ok(run) if run.state == OperationState::Queued => {}
                Ok(run) if run.state == OperationState::Cancelled => return Turn::CalledOff,
                _ => return Turn::Gone,
            }
            tokio::time::sleep(QUEUE_POLL).await;
        }
    }

    async fn blame(&self, id: Id, code: &str, message: &str) {
        let _ = self
            .operations
            .fail(
                id,
                OperationError {
                    code: code.to_owned(),
                    message: message.to_owned(),
                    step: OperationErrorStep::Filesystem,
                },
            )
            .await;
    }

    pub async fn announce(&self, server: Id) {
        match self.operations.channel(server).await {
            Ok(channel) => channel.say(&WsMessage::BackupListChanged),
            Err(fault) => tracing::warn!("no channel for {server}: {}", fault.message()),
        }
    }

    async fn dismiss(&self, server: Id, operation: Id) {
        let wiped = sqlx::query(
            "UPDATE operations SET dismissed_at = ? WHERE id = ? AND dismissed_at IS NULL",
        )
        .bind(Timestamp::now())
        .bind(operation)
        .execute(&self.pool)
        .await;
        if let Err(err) = wiped {
            tracing::warn!("a run could not be wiped: {err}");
            return;
        }
        let _ = sqlx::query(
            "UPDATE servers SET operations_revision = operations_revision + 1 \
              WHERE id = (SELECT server_id FROM operations WHERE id = ?)",
        )
        .bind(operation)
        .execute(&self.pool)
        .await;
        self.announce(server).await;
    }

    pub async fn recover(self: &Arc<Self>) -> Result<Vec<Id>> {
        for (server, backup) in store::interrupted_creates(&self.pool).await? {
            let part = self.archive_of(server, backup);
            if self.drive.resumable(backup, &part, Timestamp::now()).await.is_some() {
                tracing::info!(
                    "{} is half in Google's hands already and stays where it is",
                    part.display()
                );
                continue;
            }
            match tokio::fs::remove_file(&part).await {
                Ok(()) => tracing::info!("{} was never finished and is gone", part.display()),
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
                Err(err) => tracing::warn!("{} stays behind: {err}", part.display()),
            }
        }
        Ok(quiesce::sweep_after_restart(&self.pool, Arc::clone(&self.hub)).await?)
    }
}

enum Ended {
    CalledOff,
    Failed(OperationError),
}

impl Ended {
    fn io(err: &std::io::Error) -> Self {
        Self::Failed(OperationError {
            code: if out_of_room(err) { "no_space" } else { "internal" }.to_owned(),
            message: err.to_string(),
            step: OperationErrorStep::Filesystem,
        })
    }

    fn from_anyhow(err: &anyhow::Error) -> Self {
        match err.downcast_ref::<std::io::Error>() {
            Some(io) => Self::io(io),
            None => Self::Failed(OperationError {
                code: "internal".to_owned(),
                message: format!("{err:#}"),
                step: OperationErrorStep::Filesystem,
            }),
        }
    }

    fn drive(err: &crate::drive::http::DriveError) -> Self {
        let code = err.operation_code();
        Self::Failed(OperationError {
            code: code.to_owned(),
            message: err.to_string(),
            step: match code {
                "drive_unavailable"
                | "drive_checksum_mismatch"
                | "drive_unconfirmed"
                | "drive_file_replaced"
                | "drive_abuse_blocked" => {
                    OperationErrorStep::Download
                }
                _ => OperationErrorStep::Filesystem,
            },
        })
    }

    fn unpacking(err: &anyhow::Error) -> Self {
        if err.downcast_ref::<archive::Escapes>().is_some() {
            return Self::Failed(OperationError {
                code: "invalid_path".to_owned(),
                message: format!("{err:#}"),
                step: OperationErrorStep::Filesystem,
            });
        }
        if let Some(io) = err.downcast_ref::<std::io::Error>().filter(|io| out_of_room(io)) {
            return Self::io(io);
        }
        Self::Failed(OperationError {
            code: "archive_corrupted".to_owned(),
            message: format!("{err:#}"),
            step: OperationErrorStep::Filesystem,
        })
    }

    fn thread(join: &tokio::task::JoinError) -> Self {
        Self::Failed(OperationError {
            code: "internal".to_owned(),
            message: format!("the working thread ended badly: {join}"),
            step: OperationErrorStep::Internal,
        })
    }

    fn gone() -> Self {
        Self::Failed(OperationError {
            code: "internal".to_owned(),
            message: "the server is gone".to_owned(),
            step: OperationErrorStep::Internal,
        })
    }
}

enum Turn {
    Taken,
    CalledOff,
    Gone,
}

fn out_of_room(err: &std::io::Error) -> bool {
    matches!(err.raw_os_error(), Some(libc::ENOSPC) | Some(libc::EDQUOT))
}

fn drive_name(server_name: &str, row: &store::Row) -> String {
    format!(
        "{}--{}--{}.tar.zst",
        crate::api::backups::slug(server_name),
        crate::api::backups::slug(&row.name),
        row.created_at
    )
}

pub fn check_name(name: &str) -> Result<String> {
    let name = name.trim();
    let length = name.chars().count();
    if length == 0 || length > MAX_NAME {
        return Err(Failure::bad_request(
            "invalid_name",
            format!("a name is 1 to {MAX_NAME} characters, this one is {length}"),
        ));
    }
    if name.chars().any(char::is_control) {
        return Err(Failure::bad_request("invalid_name", "a name holds no control characters"));
    }
    Ok(name.to_owned())
}

pub fn safety_name_for(original: &str) -> String {
    let wanted = format!("Before restoring \"{original}\"");
    wanted.chars().take(SAFETY_NAME).collect()
}

fn shut(dir: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
}

fn worth_carrying_on(err: &crate::drive::http::DriveError) -> bool {
    use crate::drive::http::DriveError;

    !matches!(
        err,
        DriveError::Gone
            | DriveError::Abusive(_)
            | DriveError::Unreadable(_)
            | DriveError::Replaced(_)
    )
}

fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}

fn unknown_server() -> Failure {
    Failure::not_found("server_not_found", "no such server")
}

fn relay(fault: Fault) -> Failure {
    Failure::new(fault.status(), fault.code(), fault.message().to_owned())
}

#[cfg(test)]
mod tests;
