use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use sqlx::SqlitePool;

use crate::model::{
    Id, Minecraft, PanelRole, Permissions, Server, ServerFlows, ServerNet, ServerStatus,
    Timestamp, UpdateChannel,
};

use super::Operations;

pub async fn schema() -> SqlitePool {
    let options = sqlx::sqlite::SqliteConnectOptions::new().in_memory(true).foreign_keys(true);
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .idle_timeout(None)
        .max_lifetime(None)
        .connect_with(options)
        .await
        .expect("an in-memory database");
    sqlx::migrate!("./migrations").run(&pool).await.expect("the migrations apply");
    pool
}

pub async fn busy_schema(dir: &DataDir) -> SqlitePool {
    let options = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(dir.path().join("panel.db"))
        .create_if_missing(true)
        .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
        .foreign_keys(true)
        .busy_timeout(std::time::Duration::from_secs(10));
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(8)
        .connect_with(options)
        .await
        .expect("a database on disk");
    sqlx::migrate!("./migrations").run(&pool).await.expect("the migrations apply");
    pool
}

pub async fn a_user(pool: &SqlitePool, role: PanelRole) -> Id {
    let id = Id::new();
    sqlx::query(
        "INSERT INTO users (id, username, password_hash, role, created_at, updated_at)
         VALUES (?, ?, 'argon2', ?, ?, ?)",
    )
    .bind(id)
    .bind(id.to_string())
    .bind(role)
    .bind(Timestamp::now())
    .bind(Timestamp::now())
    .execute(pool)
    .await
    .expect("a panel user");
    id
}

pub async fn a_server(pool: &SqlitePool, owner: Id) -> Id {
    let id = Id::new();
    sqlx::query(
        "INSERT INTO servers (id, name, owner_id, status, loader, memory_mib,
                              created_at, updated_at)
         VALUES (?, 'Survival', ?, 'available', 'paper', 4096, ?, ?)",
    )
    .bind(id)
    .bind(owner)
    .bind(Timestamp::now())
    .bind(Timestamp::now())
    .execute(pool)
    .await
    .expect("a server");
    id
}

pub async fn a_session(pool: &SqlitePool, user: Id) -> String {
    let token = format!("token-{}", Id::new());
    let now = Timestamp::now();
    sqlx::query(
        "INSERT INTO sessions (id, user_id, token_hash, created_at, expires_at, last_seen)
         VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(Id::new())
    .bind(user)
    .bind(super::access::token_hash(&token))
    .bind(now)
    .bind(Timestamp::at(now.as_datetime() + std::time::Duration::from_secs(3600)))
    .bind(now)
    .execute(pool)
    .await
    .expect("a session");
    token
}

pub fn a_server_object() -> Server {
    Server {
        id: Id::new(),
        name: "Survival".to_owned(),
        owner_id: Id::new(),
        status: ServerStatus::Available,
        game: Minecraft,
        loader: None,
        loader_version: None,
        game_version: None,
        net: ServerNet { ip: None, port: 25565, domain: String::new() },
        memory_mib: 4096,
        upstream: None,
        flows: ServerFlows { intro: false },
        backup_quota: 10,
        used_backup_quota: 0,
        update_channel: UpdateChannel::Release,
        current_user_permissions: Permissions::NONE,
        created_at: Timestamp::now(),
    }
}

static COUNTER: AtomicU32 = AtomicU32::new(0);

pub struct DataDir(PathBuf);

impl DataDir {
    pub fn new() -> Self {
        let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("craftpanel-ops-{}-{unique}", std::process::id()));
        std::fs::create_dir_all(&path).expect("a data directory");
        Self(path)
    }

    pub fn path(&self) -> &std::path::Path {
        &self.0
    }

    pub fn holding_java(self, major: u32, version: &str) -> Self {
        let home = self.0.join("runtimes").join(format!("java-{major}"));
        std::fs::create_dir_all(home.join("bin")).expect("a runtime directory");
        std::fs::write(home.join("bin").join("java"), "#!/bin/sh\n").expect("a launcher");
        std::fs::write(
            home.join("release"),
            format!("IMPLEMENTOR=\"Eclipse Adoptium\"\nJAVA_VERSION=\"{version}\"\n"),
        )
        .expect("a release file");
        crate::settings::runtimes::forget(&self.0);
        self
    }
}

impl Drop for DataDir {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).ok();
    }
}

pub async fn operations() -> (Arc<Operations>, DataDir, SqlitePool) {
    let pool = schema().await;
    let dir = DataDir::new();
    let operations = Operations::new(pool.clone(), dir.path());
    (operations, dir, pool)
}
