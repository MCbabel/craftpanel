#![allow(dead_code)]

pub mod allocations;
pub mod catalog;
pub mod disk;
pub mod install;
pub mod known;
pub mod properties;
pub mod runtimes;
pub mod startup;
pub mod store;

#[cfg(test)]
pub mod harness;

use std::path::{Path, PathBuf};

use sqlx::SqlitePool;

use crate::auth::error::{Failure, Result};
use crate::model::{Id, JreVendor, LoaderId, ServerStatus};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerRow {
    pub id: Id,
    pub owner_id: Id,
    pub name: String,
    pub status: ServerStatus,
    pub loader: Option<LoaderId>,
    pub loader_version: Option<String>,
    pub game_version: Option<String>,
    pub memory_mib: u32,
    pub java_major: Option<u32>,
    pub jre_vendor: Option<JreVendor>,
    pub extra_flags: Vec<String>,
    pub restart_required: bool,
}

impl ServerRow {
    pub fn supports_properties(&self) -> bool {
        self.loader.is_none_or(LoaderId::supports_properties)
    }

    pub fn directory(&self, data_dir: &Path) -> PathBuf {
        server_dir(data_dir, self.owner_id, self.id)
    }
}

pub fn server_dir(data_dir: &Path, owner: Id, server: Id) -> PathBuf {
    data_dir.join("users").join(owner.to_string()).join("servers").join(server.to_string())
}

#[derive(sqlx::FromRow)]
struct Row {
    id: Id,
    owner_id: Id,
    name: String,
    status: ServerStatus,
    loader: Option<LoaderId>,
    loader_version: Option<String>,
    game_version: Option<String>,
    memory_mib: u32,
    java_major: Option<u32>,
    jre_vendor: Option<JreVendor>,
    extra_flags: String,
    restart_required: bool,
}

pub async fn load_server(pool: &SqlitePool, server: Id) -> Result<ServerRow> {
    let row = sqlx::query_as::<_, Row>(
        "SELECT id, owner_id, name, status, loader, loader_version, game_version, memory_mib, \
         java_major, jre_vendor, extra_flags, restart_required \
         FROM servers WHERE id = ?",
    )
    .bind(server)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| Failure::not_found("server_not_found", "no such server"))?;

    Ok(ServerRow {
        id: row.id,
        owner_id: row.owner_id,
        name: row.name,
        status: row.status,
        loader: row.loader,
        loader_version: row.loader_version,
        game_version: row.game_version,
        memory_mib: row.memory_mib,
        java_major: row.java_major,
        jre_vendor: row.jre_vendor,
        extra_flags: flag_list(&row.extra_flags),
        restart_required: row.restart_required,
    })
}

fn flag_list(json: &str) -> Vec<String> {
    serde_json::from_str(json).unwrap_or_default()
}

pub fn from_fault(fault: crate::ops::Fault) -> Failure {
    Failure::new(fault.status(), fault.code(), fault.message())
}

pub async fn give_back(helper: &crate::helper::Helper, owner: Id, server: Id) -> Result<()> {
    helper
        .chown_tree(&owner.to_string(), crate::helper::in_servers(server))
        .await
        .map(|_| ())
        .map_err(|err| Failure::internal(err.context("handing the files back to their account")))
}
