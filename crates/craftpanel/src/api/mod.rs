#![allow(dead_code)]

pub mod access;
pub mod admin;
pub mod backups;
pub mod console;
pub mod content;
pub mod drive;
pub mod files;
pub mod mail;
pub mod playit;
pub mod recovery;
pub mod registration;
pub mod runtimes;
pub mod servers;
pub mod session;
pub mod settings;
pub mod ws;

use std::sync::Arc;

use axum::Router;

use crate::auth::{Disks, LiveServers};
use crate::drive::Drive;
use crate::playit::Playit;
use crate::AppState;

pub fn router(playit: Arc<Playit>, drive: Arc<Drive>) -> Router<AppState> {
    with_live(LiveServers::none(), Disks::none(), playit, drive)
}

pub fn with_live(
    live: LiveServers,
    disks: Disks,
    playit: Arc<Playit>,
    drive: Arc<Drive>,
) -> Router<AppState> {
    session::with_live(live.clone(), disks.clone())
        .merge(admin::with_live(live, disks, playit, drive))
}
