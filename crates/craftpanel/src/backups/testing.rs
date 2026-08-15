#![cfg(test)]

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use craftpanel_proto::{OutputStream, PanelMessage, SupervisorMessage};
use sqlx::SqlitePool;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

use crate::auth::harness::FakeHelper;
use crate::auth::Disks;
use crate::drive::harness::FakeGoogle;
use crate::drive::Drive;
use crate::helper::Helper;
use crate::model::{Id, LoaderId, PanelRole, Timestamp};
use crate::ops::testing::DataDir;
use crate::ops::Operations;
use crate::servers::Hub;

use super::Backups;

const CONFIRMATION: &str = "[15:04:22] [Server thread/INFO]: Saved the game";

const WAIT_STEPS: usize = 2_000;

pub struct FakeServer {
    pub server: Id,
    pub owner: Id,
    pub operations: Arc<Operations>,
    pub backups: Arc<Backups>,
    pub drive: Arc<Drive>,
    google: FakeGoogle,
    hub: Arc<Hub>,
    pool: SqlitePool,
    commands: Arc<Mutex<Vec<String>>>,
    dir: DataDir,
    helper: FakeHelper,
}

impl FakeServer {
    pub async fn start() -> Self {
        Self::build(Attach::Answering).await
    }

    pub async fn silent() -> Self {
        Self::build(Attach::Silent).await
    }

    pub async fn stopped() -> Self {
        Self::build(Attach::None).await
    }

    pub async fn refusing_helper() -> Self {
        Self::build_with(Attach::None, FakeHelper::refusing().await).await
    }

    pub async fn holding_disk(disks: Disks) -> Self {
        Self::build_with_disks(Attach::None, FakeHelper::obliging().await, disks).await
    }

    async fn build(attach: Attach) -> Self {
        Self::build_with(attach, FakeHelper::obliging().await).await
    }

    async fn build_with(attach: Attach, helper: FakeHelper) -> Self {
        Self::build_with_disks(attach, helper, Disks::none()).await
    }

    async fn build_with_disks(attach: Attach, helper: FakeHelper, disks: Disks) -> Self {
        let dir = DataDir::new();
        let helper = helper.rooted_at(dir.path().join("users"));
        let pool = crate::ops::testing::busy_schema(&dir).await;
        let owner = crate::ops::testing::a_user(&pool, PanelRole::User).await;
        let server = crate::ops::testing::a_server(&pool, owner).await;

        let operations = Operations::new(pool.clone(), dir.path());
        let hub = Arc::new(Hub::new(dir.path().join("supervise.sock")));
        tokio::spawn({
            let hub = Arc::clone(&hub);
            async move {
                let _ = hub.listen().await;
            }
        });

        let commands = Arc::new(Mutex::new(Vec::new()));
        if attach != Attach::None {
            hub.set_token(server.to_string(), "a-token").await;
            supervisor(&hub, server, Arc::clone(&commands), attach == Attach::Answering).await;
        }

        let google = FakeGoogle::started().await;
        let drive = Drive::against(pool.clone(), dir.path(), google.base(), google.base());
        let backups = Backups::new(
            pool.clone(),
            dir.path(),
            Arc::clone(&operations),
            Arc::clone(&hub),
            Helper::new(helper.socket()),
            disks,
            Arc::clone(&drive),
        );

        Self {
            server,
            owner,
            operations,
            backups,
            drive,
            google,
            hub,
            pool,
            commands,
            dir,
            helper,
        }
    }

    pub fn google(&self) -> &FakeGoogle {
        &self.google
    }

    pub async fn connect_drive(&self) {
        crate::drive::harness::with_credentials(&self.drive).await;
        self.drive.of(self.owner).write_token("1//a-refresh-token").await;
    }

    pub async fn aim_at_drive(&self) {
        self.drive
            .set_target(self.server, crate::model::BackupLocation::Drive)
            .await
            .expect("the target");
    }

    pub fn hub(&self) -> &Hub {
        &self.hub
    }

    pub fn chowned(&self) -> Vec<String> {
        self.helper
            .calls()
            .into_iter()
            .filter_map(|call| match call {
                craftpanel_proto::HelperRequest::ChownTree { steps, .. } => Some(steps.join("/")),
                _ => None,
            })
            .collect()
    }

    pub fn hub_arc(&self) -> &Arc<Hub> {
        &self.hub
    }

    pub fn helper(&self) -> Helper {
        Helper::new(self.helper.socket())
    }

    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    pub fn data_dir(&self) -> &Path {
        self.dir.path()
    }

    pub fn server_dir(&self) -> PathBuf {
        self.operations.server_dir(self.owner, self.server)
    }

    pub async fn commands(&self) -> Vec<String> {
        self.commands.lock().expect("the command log").clone()
    }

    pub async fn settle(&self) {
        let mut seen = self.commands().await.len();
        for _ in 0..40 {
            tokio::time::sleep(Duration::from_millis(25)).await;
            let now = self.commands().await.len();
            if now == seen {
                return;
            }
            seen = now;
        }
    }

    pub fn file(&self, relative: &str, contents: &[u8]) -> PathBuf {
        let path = self.server_dir().join(relative);
        std::fs::create_dir_all(path.parent().expect("a parent")).expect("the parents");
        std::fs::write(&path, contents).expect("a file");
        path
    }

    pub async fn set_loader(&self, loader: LoaderId) {
        sqlx::query("UPDATE servers SET loader = ? WHERE id = ?")
            .bind(loader)
            .bind(self.server)
            .execute(&self.pool)
            .await
            .expect("a loader");
    }

    pub async fn fill_the_panel(&self) -> Id {
        sqlx::query("UPDATE panel_settings SET max_concurrent_operations = 1")
            .execute(&self.pool)
            .await
            .expect("a narrow panel");

        let elsewhere = crate::ops::testing::a_server(&self.pool, self.owner).await;
        let run = crate::ops::NewOperation::new(
            elsewhere,
            crate::model::OperationKind::InstallLoader,
            None,
        );
        let started = self.operations.create(run).await.expect("a run elsewhere");
        self.operations.begin(started.id).await.expect("no error").expect("it may start");
        started.id
    }

    pub async fn free_the_panel(&self, parked: Id) {
        self.operations.finish(parked).await.expect("the run elsewhere ends");
    }

    pub async fn set_quota(&self, max_backups: u32) {
        sqlx::query("UPDATE panel_settings SET max_backups_per_server = ? WHERE id = 1")
            .bind(max_backups)
            .execute(&self.pool)
            .await
            .expect("a quota");
    }

    pub async fn a_finished_backup(&self, name: &str) -> Id {
        if !self.server_dir().exists() {
            self.file("world/level.dat", b"a world");
        }
        let backup = self
            .backups
            .create(self.server, name, Some(self.owner), false)
            .await
            .expect("a backup");
        self.backups.run(backup.operation).await;
        backup.backup
    }

    pub async fn await_backup(&self, backup: Id) -> crate::model::BackupStatus {
        for _ in 0..WAIT_STEPS {
            let seen = crate::backups::store::one(&self.pool, backup)
                .await
                .expect("the backup")
                .status;
            if !matches!(
                seen,
                crate::model::BackupStatus::Pending | crate::model::BackupStatus::InProgress
            ) {
                return seen;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("the run on {backup} never ended");
    }

    pub async fn await_operation(&self, operation: Id) -> crate::model::OperationState {
        for _ in 0..WAIT_STEPS {
            let seen = self.operations.get(operation).await.expect("the run").state;
            if seen.is_terminal() {
                return seen;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("the run {operation} never ended");
    }

    pub async fn backup_rows(&self) -> i64 {
        sqlx::query_scalar("SELECT count(*) FROM backups WHERE server_id = ?")
            .bind(self.server)
            .fetch_one(&self.pool)
            .await
            .expect("a count")
    }

    pub async fn age_backup(&self, backup: Id, minutes: i64) {
        let when = Timestamp::at(
            Timestamp::now().as_datetime() - Duration::from_secs((minutes * 60) as u64),
        );
        sqlx::query("UPDATE backups SET created_at = ? WHERE id = ?")
            .bind(when)
            .bind(backup)
            .execute(&self.pool)
            .await
            .expect("an older backup");
        sqlx::query("UPDATE operations SET created_at = ? WHERE target_id = ?")
            .bind(when)
            .bind(backup)
            .execute(&self.pool)
            .await
            .expect("an older run");
    }
}

#[derive(PartialEq, Eq, Clone, Copy)]
enum Attach {
    Answering,
    Silent,
    None,
}

async fn supervisor(hub: &Hub, server: Id, log: Arc<Mutex<Vec<String>>>, answering: bool) {
    let socket = hub.socket().to_path_buf();
    let stream = connect(&socket).await;
    let (reader, mut writer) = stream.into_split();
    let mut lines = BufReader::new(reader).lines();

    let hello = SupervisorMessage::Hello {
        server_id: server.to_string(),
        token: "a-token".to_owned(),
        pid: 4242,
        protocol: craftpanel_proto::HELPER_PROTOCOL_VERSION,
    };
    let mut greeting = serde_json::to_vec(&hello).expect("json");
    greeting.push(b'\n');
    writer.write_all(&greeting).await.expect("a greeting");
    writer.flush().await.expect("a flush");

    let accepted = lines.next_line().await.expect("an answer").expect("a line");
    assert!(accepted.contains("accepted"), "the hub turned the supervisor away: {accepted}");

    tokio::spawn(async move {
        let mut seq = 0;
        while let Ok(Some(line)) = lines.next_line().await {
            let Ok(PanelMessage::Stdin { line }) = serde_json::from_str::<PanelMessage>(&line)
            else {
                continue;
            };
            let flush = line == "save-all flush";
            log.lock().expect("the command log").push(line);
            if flush && answering {
                seq += 1;
                let output = SupervisorMessage::Output {
                    seq,
                    line: CONFIRMATION.to_owned(),
                    stream: OutputStream::Stdout,
                };
                let mut encoded = serde_json::to_vec(&output).expect("json");
                encoded.push(b'\n');
                if writer.write_all(&encoded).await.is_err() {
                    return;
                }
                let _ = writer.flush().await;
            }
        }
    });
}

async fn connect(socket: &Path) -> UnixStream {
    for _ in 0..200 {
        if let Ok(stream) = UnixStream::connect(socket).await {
            return stream;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("the hub never started listening on {}", socket.display());
}
