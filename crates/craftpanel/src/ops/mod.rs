#![allow(dead_code)]

mod access;
pub mod api;
mod console;
mod events;
mod fault;
mod follow;
mod store;
#[cfg(test)]
pub(crate) mod testing;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use sqlx::SqlitePool;

use crate::model::{
    BusyReasonCode, Id, Operation, OperationError, OperationState, Timestamp,
};

#[allow(unused_imports)]
pub use self::{
    access::{caller, permissions, require, session_alive, Caller, SESSION_COOKIE},
    api::router,
    console::{tidy, History},
    events::{
        power_state_of, Attachment, Bus, Channel, ContentChangeReason, NetworkReport, ServerEvent,
        StartupReport, StateReport, StatsSample, WsMessage,
    },
    fault::{Answer, Fault},
    follow::follow,
    store::{NewOperation, Payload, Snapshot, Step},
};

pub const WORK_DIR: &str = ".craftpanel-tmp";

pub const RETENTION: Duration = Duration::from_secs(7 * 24 * 60 * 60);
pub const STALL_LIMIT: Duration = Duration::from_secs(10 * 60);
pub const PAYLOAD_LIMIT: Duration = Duration::from_secs(15 * 60);
const HOUSEKEEPING: Duration = Duration::from_secs(30);

pub struct Operations {
    pool: SqlitePool,
    bus: Arc<Bus>,
    data_dir: PathBuf,
    receiving: Arc<Mutex<BTreeSet<Id>>>,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Recovery {
    pub failed: usize,
    pub resumed: Vec<Id>,
    pub work_dirs_removed: usize,
    pub purged: u64,
}

impl Operations {
    pub fn new(pool: SqlitePool, data_dir: impl Into<PathBuf>) -> Arc<Self> {
        Arc::new(Self {
            pool,
            bus: Arc::new(Bus::default()),
            data_dir: data_dir.into(),
            receiving: Arc::default(),
        })
    }

    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    pub fn bus(&self) -> &Arc<Bus> {
        &self.bus
    }

    pub async fn create(&self, new: NewOperation) -> Answer<Operation> {
        let operation = store::insert(&self.pool, &new).await?;
        self.publish(new.server_id, true).await;
        Ok(operation)
    }

    pub async fn begin(&self, id: Id) -> Answer<Option<Operation>> {
        let started = store::begin(&self.pool, id).await?;
        if let Some(operation) = &started {
            self.publish(operation.server_id, true).await;
        }
        Ok(started)
    }

    pub async fn runnable(&self) -> Answer<Vec<Id>> {
        store::runnable(&self.pool).await
    }

    pub async fn advance(&self, id: Id, step: Step) -> Answer<Operation> {
        let operation = store::advance(&self.pool, id, &step).await?;
        self.publish(operation.server_id, step.is_urgent()).await;
        Ok(operation)
    }

    pub async fn finish(&self, id: Id) -> Answer<Operation> {
        self.settle(id, OperationState::Done, None).await
    }

    pub async fn fail(&self, id: Id, error: OperationError) -> Answer<Operation> {
        self.settle(id, OperationState::Failed, Some(error)).await
    }

    pub async fn cancelled(&self, id: Id) -> Answer<Operation> {
        let error = OperationError {
            code: "cancelled_by_user".to_owned(),
            message: "the run was called off".to_owned(),
            step: crate::model::OperationErrorStep::Internal,
        };
        self.settle(id, OperationState::Cancelled, Some(error)).await
    }

    async fn settle(
        &self,
        id: Id,
        state: OperationState,
        error: Option<OperationError>,
    ) -> Answer<Operation> {
        let operation = store::settle(&self.pool, id, state, error).await?;
        self.publish(operation.server_id, true).await;
        Ok(operation)
    }

    pub async fn request_cancel(&self, id: Id) -> Answer<Operation> {
        let operation = store::request_cancel(&self.pool, id).await?;
        self.publish(operation.server_id, true).await;
        Ok(operation)
    }

    pub async fn cancel_requested(&self, id: Id) -> Answer<bool> {
        store::cancel_requested(&self.pool, id).await
    }

    pub async fn get(&self, id: Id) -> Answer<Operation> {
        store::fetch(&self.pool, id).await
    }

    pub async fn snapshot(&self, server: Id) -> Answer<Snapshot> {
        store::snapshot(&self.pool, server).await
    }

    pub async fn busy_reasons(&self, server: Id) -> Answer<Vec<BusyReasonCode>> {
        store::busy_reasons(&self.pool, server).await
    }

    pub async fn guard_write(&self, server: Id) -> Answer<()> {
        match store::busy_reasons(&self.pool, server).await?.first() {
            None => Ok(()),
            Some(reason) => Err(Fault::conflict("server_busy", explain(*reason))),
        }
    }

    pub async fn channel(&self, server: Id) -> Answer<Arc<Channel>> {
        let channel = self.bus.channel(server);
        if !channel.needs_priming() {
            return Ok(channel);
        }

        let row: Option<(i64, Id)> =
            sqlx::query_as("SELECT console_seq, owner_id FROM servers WHERE id = ?")
                .bind(server)
                .fetch_optional(&self.pool)
                .await?;
        let Some((seq, owner)) = row else {
            return Err(Fault::server_not_found());
        };

        let log = self.server_dir(owner, server).join("logs").join("latest.log");
        let lines = tokio::task::spawn_blocking(move || {
            console::tail_of_log(&log, console::MAX_LINES, console::MAX_BYTES as u64)
        })
        .await
        .unwrap_or_default();

        channel.prime(seq.max(0) as u64, lines);
        Ok(channel)
    }

    pub async fn console(&self, server: Id, lines: &[String]) -> Answer<()> {
        self.channel(server).await?.console_lines(lines);
        Ok(())
    }

    pub async fn publish(&self, server: Id, urgent: bool) {
        let channel = self.bus.channel(server);
        match channel.claim_snapshot(urgent) {
            events::Due::Now => match store::snapshot(&self.pool, server).await {
                Ok(snapshot) => channel.say(&WsMessage::Operations(snapshot)),
                Err(fault) => tracing::error!("no snapshot for {server}: {}", fault.message()),
            },
            events::Due::After(delay) => {
                let pool = self.pool.clone();
                tokio::spawn(async move {
                    tokio::time::sleep(delay).await;
                    channel.snapshot_sent();
                    if let Ok(snapshot) = store::snapshot(&pool, server).await {
                        channel.say(&WsMessage::Operations(snapshot));
                    }
                });
            }
            events::Due::Pending => {}
        }
    }

    pub fn receive(&self, operation: Id) -> Option<Receiving> {
        let mut receiving = self.receiving.lock().expect("the upload lock");
        receiving
            .insert(operation)
            .then(|| Receiving { held: Arc::clone(&self.receiving), operation })
    }

    pub fn server_dir(&self, owner: Id, server: Id) -> PathBuf {
        self.data_dir
            .join("users")
            .join(owner.to_string())
            .join("servers")
            .join(server.to_string())
    }

    pub async fn work_dir(&self, operation: Id) -> Answer<PathBuf> {
        let row: Option<(Id, Id)> = sqlx::query_as(
            "SELECT servers.id, servers.owner_id
               FROM operations JOIN servers ON servers.id = operations.server_id
              WHERE operations.id = ?",
        )
        .bind(operation)
        .fetch_optional(&self.pool)
        .await?;
        let (server, owner) = row.ok_or_else(store::operation_not_found)?;
        Ok(self.server_dir(owner, server).join(WORK_DIR).join(operation.to_string()))
    }

    pub async fn recover(&self) -> Answer<Recovery> {
        let failed = store::recover(&self.pool).await?;
        let resumed: Vec<Id> = sqlx::query_as::<_, (Id,)>(
            "SELECT id FROM operations
              WHERE kind = 'server_delete' AND state IN ('queued', 'ongoing') ORDER BY id",
        )
        .fetch_all(&self.pool)
        .await?
        .into_iter()
        .map(|(id,)| id)
        .collect();

        let work_dirs_removed = self.sweep_work_dirs().await?;
        let purged = self.purge().await?;

        Ok(Recovery { failed: failed.len(), resumed, work_dirs_removed, purged })
    }

    pub async fn sweep_work_dirs(&self) -> Answer<usize> {
        let keep: BTreeSet<Id> =
            store::open_ids(&self.pool).await?.into_iter().map(|(id, _, _)| id).collect();
        let servers = store::all_server_directories(&self.pool).await?;
        let dirs: Vec<PathBuf> =
            servers.into_iter().map(|(server, owner)| self.server_dir(owner, server)).collect();

        let removed = tokio::task::spawn_blocking(move || {
            let mut removed = 0;
            for dir in dirs {
                removed += sweep_one(&dir.join(WORK_DIR), &keep);
            }
            removed
        })
        .await
        .unwrap_or_default();
        Ok(removed)
    }

    pub async fn purge(&self) -> Answer<u64> {
        let cutoff = Timestamp::at(Timestamp::now().as_datetime() - RETENTION);
        store::purge_finished(&self.pool, cutoff).await
    }

    pub async fn housekeeping(&self) -> Answer<()> {
        let now = Timestamp::now().as_datetime();
        let ended = store::sweep_timeouts(
            &self.pool,
            Timestamp::at(now - STALL_LIMIT),
            Timestamp::at(now - PAYLOAD_LIMIT),
        )
        .await?;
        for operation in &ended {
            self.publish(operation.server_id, true).await;
        }
        self.purge().await?;
        self.persist_console_counts().await?;
        Ok(())
    }

    pub fn spawn_housekeeping(self: &Arc<Self>) -> tokio::task::JoinHandle<()> {
        let operations = Arc::clone(self);
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(HOUSEKEEPING);
            loop {
                tick.tick().await;
                if let Err(fault) = operations.housekeeping().await {
                    tracing::error!("housekeeping: {}", fault.message());
                }
            }
        })
    }

    async fn persist_console_counts(&self) -> Answer<()> {
        for server in self.bus.servers() {
            let Some(channel) = self.bus.existing(server) else { continue };
            sqlx::query("UPDATE servers SET console_seq = ? WHERE id = ? AND console_seq < ?")
                .bind(channel.console_seq() as i64)
                .bind(server)
                .bind(channel.console_seq() as i64)
                .execute(&self.pool)
                .await?;
        }
        Ok(())
    }
}

pub struct Receiving {
    held: Arc<Mutex<BTreeSet<Id>>>,
    operation: Id,
}

impl Drop for Receiving {
    fn drop(&mut self) {
        self.held.lock().expect("the upload lock").remove(&self.operation);
    }
}

fn sweep_one(work_dir: &Path, keep: &BTreeSet<Id>) -> usize {
    let Ok(entries) = std::fs::read_dir(work_dir) else {
        return 0;
    };
    let mut removed = 0;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let alive = name.to_str().and_then(|name| name.parse::<Id>().ok()).is_some_and(|id| keep.contains(&id));
        if alive {
            continue;
        }
        let path = entry.path();
        let gone = if path.is_dir() {
            std::fs::remove_dir_all(&path)
        } else {
            std::fs::remove_file(&path)
        };
        match gone {
            Ok(()) => removed += 1,
            Err(err) => tracing::warn!("leftover {} stays: {err}", path.display()),
        }
    }
    removed
}

fn explain(reason: BusyReasonCode) -> &'static str {
    match reason {
        BusyReasonCode::Installing => "an installation is running on this server",
        BusyReasonCode::SyncingContent => "content is being installed or updated",
        BusyReasonCode::BackupCreating => "a backup is being created",
        BusyReasonCode::BackupRestoring => "a backup is being restored",
        BusyReasonCode::Deleting => "this server is being deleted",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{
        OperationErrorStep, OperationKind, OperationPhase, PanelRole, ServerStatus,
    };
    use crate::ops::events::ServerEvent;
    use sqlx::SqlitePool;

    struct Panel {
        operations: Arc<Operations>,
        pool: SqlitePool,
        owner: Id,
        server: Id,
        _dir: testing::DataDir,
    }

    async fn panel() -> Panel {
        let (operations, dir, pool) = testing::operations().await;
        let owner = testing::a_user(&pool, PanelRole::User).await;
        let server = testing::a_server(&pool, owner).await;
        Panel { operations, pool, owner, server, _dir: dir }
    }

    impl Panel {
        async fn start(&self, kind: OperationKind) -> Operation {
            self.operations
                .create(NewOperation::new(self.server, kind, Some(self.owner)))
                .await
                .expect("an operation")
        }

        async fn on(&self, server: Id, kind: OperationKind) -> Operation {
            self.operations
                .create(NewOperation::new(server, kind, None))
                .await
                .expect("an operation")
        }

        async fn state_of(&self, id: Id) -> OperationState {
            self.operations.get(id).await.expect("the operation").state
        }

        async fn server_status(&self, server: Id) -> ServerStatus {
            sqlx::query_as::<_, (ServerStatus,)>("SELECT status FROM servers WHERE id = ?")
                .bind(server)
                .fetch_one(&self.pool)
                .await
                .expect("the server")
                .0
        }
    }

    fn broke() -> OperationError {
        OperationError {
            code: "checksum_mismatch".to_owned(),
            message: "the jar is not the jar we asked for".to_owned(),
            step: OperationErrorStep::Download,
        }
    }

    async fn age(pool: &SqlitePool, id: Id, column: &str, days: i64) {
        let when = Timestamp::at(
            Timestamp::now().as_datetime() - Duration::from_secs((days * 24 * 60 * 60) as u64),
        );
        sqlx::query(&format!("UPDATE operations SET {column} = ? WHERE id = ?"))
            .bind(when)
            .bind(id)
            .execute(pool)
            .await
            .expect("moving a timestamp");
    }

    #[tokio::test]
    async fn a_run_goes_queued_ongoing_done_and_says_when_each_happened() {
        let panel = panel().await;
        let created = panel.start(OperationKind::InstallLoader).await;
        assert_eq!(created.state, OperationState::Queued);
        assert_eq!(created.progress, 0.0);
        assert!(created.started_at.is_none());
        assert!(!created.cancellable, "install runs have no cancel button (5.8)");

        let started = panel.operations.begin(created.id).await.expect("no error").expect("it may");
        assert_eq!(started.state, OperationState::Ongoing);
        assert!(started.started_at.is_some());

        let stepped = panel
            .operations
            .advance(
                created.id,
                Step {
                    phase: Some(OperationPhase::InstallingLoader),
                    progress: Some(0.42),
                    current_file: Some("server.jar".to_owned()),
                    bytes_processed: Some(18_874_368),
                    ..Step::default()
                },
            )
            .await
            .expect("a step");
        assert_eq!(stepped.phase, Some(OperationPhase::InstallingLoader));
        assert_eq!(stepped.progress, 0.42);
        assert_eq!(stepped.bytes_processed, Some(18_874_368));

        let done = panel.operations.finish(created.id).await.expect("it finishes");
        assert_eq!(done.state, OperationState::Done);
        assert_eq!(done.progress, 1.0, "the banner reads the number, not the state");
        assert!(done.finished_at.is_some());
        assert!(done.dismissed_at.is_none());
    }

    #[tokio::test]
    async fn a_progress_outside_zero_to_one_never_reaches_the_wire() {
        let panel = panel().await;
        let run = panel.start(OperationKind::InstallContent).await;
        let stepped = panel
            .operations
            .advance(run.id, Step { progress: Some(42.0), ..Step::default() })
            .await
            .expect("a step");
        assert_eq!(stepped.progress, 1.0, "1.6: a fraction, never a percentage");
    }

    #[tokio::test]
    async fn one_run_at_a_time_per_server() {
        let panel = panel().await;
        let first = panel.start(OperationKind::InstallContent).await;
        let second = panel.start(OperationKind::UpdateContent).await;

        assert!(panel.operations.begin(first.id).await.expect("no error").is_some());
        assert!(
            panel.operations.begin(second.id).await.expect("no error").is_none(),
            "everything is serialised per server (5.13)"
        );
        assert_eq!(panel.state_of(second.id).await, OperationState::Queued);

        panel.operations.finish(first.id).await.expect("it finishes");
        assert!(panel.operations.begin(second.id).await.expect("no error").is_some());
    }

    #[tokio::test]
    async fn the_panel_wide_width_is_the_one_the_admin_set() {
        let panel = panel().await;
        let second = testing::a_server(&panel.pool, panel.owner).await;
        let third = testing::a_server(&panel.pool, panel.owner).await;

        let runs = [
            panel.start(OperationKind::InstallModpack).await,
            panel.on(second, OperationKind::InstallModpack).await,
            panel.on(third, OperationKind::InstallModpack).await,
        ];

        assert_eq!(panel.operations.runnable().await.expect("a list").len(), 2);
        assert!(panel.operations.begin(runs[0].id).await.expect("no error").is_some());
        assert!(panel.operations.begin(runs[1].id).await.expect("no error").is_some());
        assert!(
            panel.operations.begin(runs[2].id).await.expect("no error").is_none(),
            "ten modpack installs at once would saturate the line"
        );

        sqlx::query("UPDATE panel_settings SET max_concurrent_operations = 3 WHERE id = 1")
            .execute(&panel.pool)
            .await
            .expect("a wider queue");
        assert!(panel.operations.begin(runs[2].id).await.expect("no error").is_some());
    }

    #[tokio::test]
    async fn runs_offered_at_the_same_moment_wait_for_each_other() {
        let dir = testing::DataDir::new();
        let pool = testing::busy_schema(&dir).await;
        let operations = Operations::new(pool.clone(), dir.path());
        let owner = testing::a_user(&pool, PanelRole::User).await;

        let mut runs = Vec::new();
        for _ in 0..6 {
            let server = testing::a_server(&pool, owner).await;
            let run = NewOperation::new(server, OperationKind::InstallModpack, None);
            runs.push(operations.create(run).await.expect("an operation").id);
        }

        let offered = runs.into_iter().map(|id| {
            let operations = Arc::clone(&operations);
            async move { operations.begin(id).await }
        });
        let answers = futures::future::join_all(offered).await;

        let started = answers
            .into_iter()
            .filter(|answer| answer.as_ref().expect("no refusal from the database").is_some())
            .count();
        assert_eq!(started, 2, "the width of 12.10; the other four stay queued");
    }

    #[tokio::test]
    async fn the_locks_of_table_5_8_are_what_a_write_is_refused_for() {
        let panel = panel().await;
        let cases = [
            (OperationKind::ServerCreate, Some(BusyReasonCode::Installing)),
            (OperationKind::InstallJava, Some(BusyReasonCode::Installing)),
            (OperationKind::InstallContent, Some(BusyReasonCode::SyncingContent)),
            (OperationKind::BackupCreate, Some(BusyReasonCode::BackupCreating)),
            (OperationKind::BackupRestore, Some(BusyReasonCode::BackupRestoring)),
            (OperationKind::ServerDelete, Some(BusyReasonCode::Deleting)),
            (OperationKind::Unarchive, None),
        ];

        for (kind, expected) in cases {
            let server = testing::a_server(&panel.pool, panel.owner).await;
            let run = panel.on(server, kind).await;
            assert_eq!(
                panel.operations.busy_reasons(server).await.expect("reasons"),
                expected.into_iter().collect::<Vec<_>>(),
                "{kind}"
            );

            let guarded = panel.operations.guard_write(server).await;
            match expected {
                None => assert!(guarded.is_ok(), "{kind} must not lock anything"),
                Some(reason) => {
                    let fault = guarded.err().expect("a refusal");
                    assert_eq!(fault.code(), "server_busy");
                    assert_eq!(fault.message(), explain(reason));
                }
            }

            panel.operations.cancelled(run.id).await.expect("it ends");
            assert!(panel.operations.busy_reasons(server).await.expect("reasons").is_empty());
            assert!(panel.operations.guard_write(server).await.is_ok());
        }
    }

    #[tokio::test]
    async fn a_run_that_only_waits_already_holds_the_lock() {
        let panel = panel().await;
        panel.start(OperationKind::BackupCreate).await;
        assert_eq!(
            panel.operations.busy_reasons(panel.server).await.expect("reasons"),
            vec![BusyReasonCode::BackupCreating],
            "queued means about to run, and the answer must not change under the caller"
        );
    }

    #[tokio::test]
    async fn a_second_backup_run_reads_as_the_lock_it_is() {
        let panel = panel().await;
        panel.start(OperationKind::BackupCreate).await;

        let again = panel
            .operations
            .create(NewOperation::new(
                panel.server,
                OperationKind::BackupCreate,
                Some(panel.owner),
            ))
            .await;
        let fault = again.err().expect("the partial index of 10.2 refuses it");
        assert_eq!(fault.code(), "server_busy");

        assert!(panel
            .operations
            .create(NewOperation::new(panel.server, OperationKind::BackupRestore, None))
            .await
            .is_ok());
    }

    #[tokio::test]
    async fn every_change_raises_the_revision_and_a_restart_does_not_lower_it() {
        let panel = panel().await;
        let run = panel.start(OperationKind::InstallLoader).await;
        let after_create = panel.operations.snapshot(panel.server).await.expect("a snapshot");

        panel.operations.begin(run.id).await.expect("no error");
        let after_begin = panel.operations.snapshot(panel.server).await.expect("a snapshot");
        assert!(after_begin.revision > after_create.revision);

        panel
            .operations
            .advance(run.id, Step { progress: Some(0.5), ..Step::default() })
            .await
            .expect("a step");
        let after_progress = panel.operations.snapshot(panel.server).await.expect("a snapshot");
        assert!(
            after_progress.revision > after_begin.revision,
            "a throttled progress snapshot with an unchanged number would be thrown away"
        );

        let restarted = Operations::new(panel.pool.clone(), panel._dir.path());
        let after_restart = restarted.snapshot(panel.server).await.expect("a snapshot");
        assert_eq!(after_restart.revision, after_progress.revision);
    }

    #[tokio::test]
    async fn progress_alone_waits_a_second_and_a_phase_change_does_not() {
        let panel = panel().await;
        let channel = panel.operations.channel(panel.server).await.expect("a channel");
        let mut events = channel.attach().events;
        let run = panel.start(OperationKind::InstallModpack).await;

        for step in 1..=5 {
            panel
                .operations
                .advance(
                    run.id,
                    Step { progress: Some(step as f64 / 10.0), ..Step::default() },
                )
                .await
                .expect("a step");
        }
        assert_eq!(snapshots(&mut events), 1, "the five steps are held back, only the create went");

        tokio::time::sleep(Duration::from_millis(1200)).await;
        assert_eq!(snapshots(&mut events), 1, "and one goes out for the whole second");

        panel
            .operations
            .advance(
                run.id,
                Step { phase: Some(OperationPhase::Addons), ..Step::default() },
            )
            .await
            .expect("a step");
        assert_eq!(snapshots(&mut events), 1, "a phase change never waits");
    }

    fn snapshots(events: &mut tokio::sync::broadcast::Receiver<ServerEvent>) -> usize {
        let mut seen = 0;
        while let Ok(event) = events.try_recv() {
            if let ServerEvent::Say(json) = event {
                let value: serde_json::Value = serde_json::from_str(&json).expect("json");
                if value["type"] == "operations" {
                    seen += 1;
                }
            }
        }
        seen
    }

    #[tokio::test]
    async fn a_restart_fails_what_was_running_and_carries_on_deleting() {
        let panel = panel().await;
        let installing = panel.start(OperationKind::InstallLoader).await;
        panel.operations.begin(installing.id).await.expect("no error");
        let waiting = panel.start(OperationKind::InstallContent).await;

        let other = testing::a_server(&panel.pool, panel.owner).await;
        let deleting = panel.on(other, OperationKind::ServerDelete).await;
        panel.operations.begin(deleting.id).await.expect("no error");

        let recovery = panel.operations.recover().await.expect("a recovery");
        assert_eq!(recovery.failed, 2);
        assert_eq!(recovery.resumed, vec![deleting.id]);

        for id in [installing.id, waiting.id] {
            let operation = panel.operations.get(id).await.expect("the operation");
            assert_eq!(operation.state, OperationState::Failed);
            assert_eq!(operation.error.expect("an error").code, "panel_restarted");
            assert!(operation.finished_at.is_some());
            assert!(operation.dismissed_at.is_none(), "the user is meant to see it");
        }
        assert_eq!(panel.state_of(deleting.id).await, OperationState::Ongoing);
        assert_eq!(panel.server_status(panel.server).await, ServerStatus::Broken);
    }

    #[tokio::test]
    async fn a_restart_leaves_each_kind_the_way_5_12_describes() {
        let panel = panel().await;

        let creating = testing::a_server(&panel.pool, panel.owner).await;
        panel.on(creating, OperationKind::ServerCreate).await;

        let restoring = testing::a_server(&panel.pool, panel.owner).await;
        panel.on(restoring, OperationKind::BackupRestore).await;

        let backing_up = testing::a_server(&panel.pool, panel.owner).await;
        let backup = Id::new();
        sqlx::query("INSERT INTO backups (id, server_id, name, created_at) VALUES (?, ?, 'B', ?)")
            .bind(backup)
            .bind(backing_up)
            .bind(Timestamp::now())
            .execute(&panel.pool)
            .await
            .expect("a backup row");
        let mut run = NewOperation::new(backing_up, OperationKind::BackupCreate, None);
        run.target_id = Some(backup);
        panel.operations.create(run).await.expect("an operation");

        let extracting = testing::a_server(&panel.pool, panel.owner).await;
        let unarchive = panel.on(extracting, OperationKind::Unarchive).await;
        sqlx::query("UPDATE operations SET applied_at = ? WHERE id = ?")
            .bind(Timestamp::now())
            .bind(unarchive.id)
            .execute(&panel.pool)
            .await
            .expect("the moving started");

        let untouched = testing::a_server(&panel.pool, panel.owner).await;
        panel.on(untouched, OperationKind::InstallContent).await;

        panel.operations.recover().await.expect("a recovery");

        assert_eq!(panel.server_status(creating).await, ServerStatus::Broken);
        let (intro,): (bool,) =
            sqlx::query_as("SELECT flows_intro FROM servers WHERE id = ?")
                .bind(creating)
                .fetch_one(&panel.pool)
                .await
                .expect("the server");
        assert!(intro, "a create that never finished starts the flow over");

        assert_eq!(panel.server_status(restoring).await, ServerStatus::Broken);
        let restore = panel
            .operations
            .snapshot(restoring)
            .await
            .expect("a snapshot")
            .operations
            .remove(0);
        assert_eq!(
            restore.error.expect("an error").code,
            "restore_interrupted",
            "being honest beats calling a broken directory whole"
        );

        let (backups,): (i64,) = sqlx::query_as("SELECT count(*) FROM backups WHERE server_id = ?")
            .bind(backing_up)
            .fetch_one(&panel.pool)
            .await
            .expect("a count");
        assert_eq!(backups, 0, "the archive was never written, so the row promised one");

        let interrupted = panel.operations.get(unarchive.id).await.expect("the operation");
        assert_eq!(interrupted.error.expect("an error").code, "interrupted_while_applying");

        assert_eq!(panel.server_status(untouched).await, ServerStatus::Available);
        assert_eq!(panel.server_status(extracting).await, ServerStatus::Available);
    }

    #[tokio::test]
    async fn a_work_directory_without_a_run_behind_it_is_swept_away() {
        let panel = panel().await;
        let alive = panel.start(OperationKind::ServerDelete).await;
        let dead = panel.start(OperationKind::Unarchive).await;
        panel.operations.cancelled(dead.id).await.expect("it ends");

        let work = panel.operations.server_dir(panel.owner, panel.server).join(WORK_DIR);
        for name in [alive.id.to_string(), dead.id.to_string(), "01ARZ3NDEKTSV4RRFFQ69G5FAV".to_owned()] {
            std::fs::create_dir_all(work.join(&name)).expect("a work directory");
            std::fs::write(work.join(&name).join("half.jar"), b"partial").expect("a file");
        }
        std::fs::write(work.join("stray.tmp"), b"orphan").expect("a stray file");

        let removed = panel.operations.sweep_work_dirs().await.expect("a sweep");
        assert_eq!(removed, 3, "the cancelled run, the orphan and the stray");
        assert!(work.join(alive.id.to_string()).exists(), "the delete is carried on with");
        assert!(!work.join(dead.id.to_string()).exists());
        assert!(!work.join("stray.tmp").exists());
    }

    #[tokio::test]
    async fn finished_runs_are_kept_for_seven_days_and_open_ones_for_ever() {
        let panel = panel().await;
        let old = panel.start(OperationKind::InstallContent).await;
        panel.operations.finish(old.id).await.expect("it finishes");
        age(&panel.pool, old.id, "finished_at", 8).await;

        let recent = panel.start(OperationKind::InstallContent).await;
        panel.operations.finish(recent.id).await.expect("it finishes");
        age(&panel.pool, recent.id, "finished_at", 6).await;

        let ancient_but_open = panel.start(OperationKind::ServerDelete).await;
        age(&panel.pool, ancient_but_open.id, "created_at", 30).await;

        assert_eq!(panel.operations.purge().await.expect("a purge"), 1);
        assert!(panel.operations.get(old.id).await.is_err());
        assert!(panel.operations.get(recent.id).await.is_ok());
        assert!(panel.operations.get(ancient_but_open.id).await.is_ok());
    }

    #[tokio::test]
    async fn the_two_watchdogs_end_what_nobody_drives_any_more() {
        let panel = panel().await;
        let stalled = panel.start(OperationKind::InstallModpack).await;
        panel.operations.begin(stalled.id).await.expect("no error");
        sqlx::query("UPDATE operations SET progressed_at = ? WHERE id = ?")
            .bind(Timestamp::at(Timestamp::now().as_datetime() - Duration::from_secs(11 * 60)))
            .bind(stalled.id)
            .execute(&panel.pool)
            .await
            .expect("an old step");

        let other = testing::a_server(&panel.pool, panel.owner).await;
        let mut waiting = NewOperation::new(other, OperationKind::ServerCreate, None);
        waiting.expects_payload = true;
        let waiting = panel.operations.create(waiting).await.expect("an operation");
        age(&panel.pool, waiting.id, "created_at", 1).await;

        let third = testing::a_server(&panel.pool, panel.owner).await;
        let healthy = panel.on(third, OperationKind::InstallContent).await;
        panel.operations.begin(healthy.id).await.expect("no error");

        panel.operations.housekeeping().await.expect("a sweep");

        let timed_out = panel.operations.get(stalled.id).await.expect("the operation");
        assert_eq!(timed_out.state, OperationState::Failed);
        assert_eq!(timed_out.error.expect("an error").code, "timeout");

        let never_paid = panel.operations.get(waiting.id).await.expect("the operation");
        assert_eq!(never_paid.state, OperationState::Failed);
        assert_eq!(never_paid.error.expect("an error").code, "payload_timeout");

        assert_eq!(panel.state_of(healthy.id).await, OperationState::Ongoing);
    }

    #[tokio::test]
    async fn the_snapshot_is_the_whole_state_and_leaves_out_what_was_wiped() {
        let panel = panel().await;
        let wiped = panel.start(OperationKind::InstallContent).await;
        panel.operations.finish(wiped.id).await.expect("it finishes");
        store::dismiss(&panel.pool, wiped.id).await.expect("it is wiped");

        let failed = panel.start(OperationKind::InstallLoader).await;
        panel.operations.fail(failed.id, broke()).await.expect("it fails");
        let running = panel.start(OperationKind::BackupCreate).await;

        let snapshot = panel.operations.snapshot(panel.server).await.expect("a snapshot");
        let ids: Vec<Id> = snapshot.operations.iter().map(|operation| operation.id).collect();
        assert_eq!(ids, vec![running.id, failed.id], "newest first, and the wiped one is gone");
        assert_eq!(snapshot.busy_reasons, vec![BusyReasonCode::BackupCreating]);
        assert_eq!(snapshot.operations[1].error.as_ref().expect("an error").code, "checksum_mismatch");
    }

    #[tokio::test]
    async fn wiping_a_failed_delete_does_not_wipe_the_state_the_server_stands_in() {
        let panel = panel().await;
        let refused = OperationError {
            code: "permission_denied".to_owned(),
            message: "Permission denied (os error 13)".to_owned(),
            step: OperationErrorStep::Filesystem,
        };
        let run = panel.start(OperationKind::ServerDelete).await;
        panel.operations.fail(run.id, refused).await.expect("it fails");
        sqlx::query("UPDATE servers SET status = 'deleting' WHERE id = ?")
            .bind(panel.server)
            .execute(&panel.pool)
            .await
            .expect("a server on its way out");

        store::dismiss(&panel.pool, run.id).await.expect("it is wiped");

        let snapshot = panel.operations.snapshot(panel.server).await.expect("a snapshot");
        let ids: Vec<Id> = snapshot.operations.iter().map(|operation| operation.id).collect();
        assert_eq!(ids, vec![run.id], "a dismiss says 'I have read it', not 'it never happened'");
        assert!(snapshot.operations[0].dismissed_at.is_some(), "and it stays wiped");
        assert_eq!(
            snapshot.operations[0].error.as_ref().expect("an error").message,
            "Permission denied (os error 13)",
            "the reason is the whole point of the notice"
        );

        age(&panel.pool, run.id, "finished_at", 8).await;
        assert_eq!(panel.operations.purge().await.expect("a purge"), 0);
        assert!(panel.operations.get(run.id).await.is_ok());
    }

    #[tokio::test]
    async fn the_console_count_is_written_back_and_never_walks_backwards() {
        let panel = panel().await;
        let channel = panel.operations.channel(panel.server).await.expect("a channel");
        channel.console_lines(&["one".to_owned(), "two".to_owned()]);
        panel.operations.housekeeping().await.expect("housekeeping");

        let seq = || async {
            sqlx::query_as::<_, (i64,)>("SELECT console_seq FROM servers WHERE id = ?")
                .bind(panel.server)
                .fetch_one(&panel.pool)
                .await
                .expect("the server")
                .0
        };
        assert_eq!(seq().await, 2);

        sqlx::query("UPDATE servers SET console_seq = 900 WHERE id = ?")
            .bind(panel.server)
            .execute(&panel.pool)
            .await
            .expect("a higher count");
        panel.operations.housekeeping().await.expect("housekeeping");
        assert_eq!(seq().await, 900, "a stale writer must not pull the count back");
    }

    #[tokio::test]
    async fn the_buffer_of_a_server_that_was_running_before_the_restart_is_not_empty() {
        let panel = panel().await;
        let logs = panel.operations.server_dir(panel.owner, panel.server).join("logs");
        std::fs::create_dir_all(&logs).expect("a log directory");
        std::fs::write(
            logs.join("latest.log"),
            "[15:04:22] [Server thread/INFO]: Done\n[15:04:23] [Server thread/INFO]: Hello\n",
        )
        .expect("a log");
        sqlx::query("UPDATE servers SET console_seq = 41 WHERE id = ?")
            .bind(panel.server)
            .execute(&panel.pool)
            .await
            .expect("a count from before");

        let channel = panel.operations.channel(panel.server).await.expect("a channel");
        let history = channel.attach().history;
        assert_eq!(history.lines.len(), 2, "a stopped console must not open onto nothing");
        assert_eq!(history.first_seq, 41, "and the count carries on where it stood");
        assert_eq!(&*history.lines[1], "[15:04:23] [Server thread/INFO]: Hello");
    }
}
