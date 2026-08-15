#![cfg(test)]

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use sqlx::SqlitePool;

use crate::model::{Id, LoaderId, Timestamp};

static COUNTER: AtomicU32 = AtomicU32::new(0);

pub struct Scratch(PathBuf);

impl Scratch {
    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).ok();
    }
}

pub fn a_dir() -> Scratch {
    let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
    let path =
        std::env::temp_dir().join(format!("craftpanel-settings-{}-{unique}", std::process::id()));
    std::fs::create_dir_all(&path).expect("a scratch directory");
    Scratch(path)
}

pub async fn pool_with_server() -> (SqlitePool, Id, Id) {
    let pool = crate::auth::harness::test_pool().await;
    let owner = crate::auth::harness::a_user(&pool, "max").await;
    let server = crate::auth::harness::a_server(&pool, owner, "Survival", 4096).await;
    set_loader(&pool, server, LoaderId::Paper, "1.21.8", Some("60")).await;
    (pool, server, owner)
}

pub async fn set_loader(
    pool: &SqlitePool,
    server: Id,
    loader: LoaderId,
    game_version: &str,
    build: Option<&str>,
) {
    sqlx::query(
        "UPDATE servers SET loader = ?, game_version = ?, loader_version = ?, updated_at = ? \
         WHERE id = ?",
    )
    .bind(loader)
    .bind(game_version)
    .bind(build)
    .bind(Timestamp::now())
    .bind(server)
    .execute(pool)
    .await
    .expect("setting the loader");
}

pub async fn an_allocation(pool: &SqlitePool, server: Id, port: u16, name: &str, primary: bool) {
    sqlx::query(
        "INSERT INTO allocations (port, server_id, name, is_primary, created_at) \
         VALUES (?, ?, ?, ?, ?)",
    )
    .bind(port)
    .bind(server)
    .bind(name)
    .bind(primary)
    .bind(Timestamp::now())
    .execute(pool)
    .await
    .expect("an allocation");
}
