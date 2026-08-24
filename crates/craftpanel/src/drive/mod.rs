#![allow(dead_code)]

pub mod day;
pub mod files;
pub mod http;
pub mod keys;
pub mod oauth;
pub mod retry;
pub mod store;
pub mod upload;

#[cfg(test)]
mod attacks;
#[cfg(test)]
pub mod harness;
#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::http::StatusCode;
use serde::Serialize;
use sqlx::SqlitePool;
use tokio::sync::Semaphore;
use tokio::task::JoinHandle;

use crate::auth::error::{Failure, Result};
use crate::backups::archive::Progress;
use crate::config::Config;
use crate::model::{
    BackupLocation, BackupTarget, BackupTargetPolicy, BackupTargetReason, DriveAccountState,
    DriveFileState, DriveLinkState, Id, Timestamp,
};

use self::http::{DriveError, Http};
use self::keys::Keys;
use self::oauth::{Access, Bearer, Credentials, Secret};

pub const PANEL_TAG: &str = "craftpanel";

pub const ARCHIVE_TYPE: &str = "application/zstd";

const LINK_LOOPS: usize = 4;

const SWEEP_EVERY: Duration = Duration::from_secs(60 * 60);

pub const SESSION_LIFE: time::Duration = time::Duration::days(6);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Files {
    Refuse,
    Delete,
    Keep,
}

#[derive(Debug, Clone)]
pub enum SecretChange {
    Keep,
    Remove,
    Replace(String),
}

#[derive(Debug, Clone)]
pub struct Stored {
    pub file_id: String,
    pub md5: Option<String>,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct Recorded<'a> {
    pub bytes: Option<u64>,
    pub md5: Option<&'a str>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DriveLink {
    pub user_code: String,
    pub verification_url: String,
    pub state: DriveLinkState,
    pub started_at: Timestamp,
    pub expires_at: Timestamp,
    pub interval: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct DriveStatus {
    pub panel_configured: bool,
    pub configured: bool,
    pub state: Option<DriveAccountState>,
    pub google_name: Option<String>,
    pub google_email: Option<String>,
    pub folder_name: String,
    pub storage_limit_bytes: Option<u64>,
    pub storage_usage_bytes: Option<u64>,
    pub uploaded_today_bytes: u64,
    pub daily_upload_limit_bytes: u64,
    pub link: Option<DriveLink>,
    pub last_error: Option<String>,
    pub checked_at: Option<Timestamp>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DriveOverview {
    pub user_id: Id,
    pub username: String,
    pub state: Option<DriveAccountState>,
    pub google_email: Option<String>,
    pub storage_limit_bytes: Option<u64>,
    pub storage_usage_bytes: Option<u64>,
    pub uploaded_today_bytes: u64,
    pub daily_upload_limit_bytes: u64,
    pub backups: u32,
    pub backup_bytes: u64,
    pub last_error: Option<String>,
    pub checked_at: Option<Timestamp>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DriveAdminOverview {
    pub configured: bool,
    pub client_id: Option<String>,
    pub target_policy: BackupTargetPolicy,
    pub folder_name: String,
    pub accounts: Vec<DriveOverview>,
}

struct Shared {
    pool: SqlitePool,
    keys: Keys,
    http: Http,
    link_slots: Arc<Semaphore>,
    sending: Arc<Mutex<std::collections::BTreeSet<Id>>>,
}

impl Shared {
    fn claim(&self, backup: Id) -> Option<Sending> {
        let mut sending = self.sending.lock().expect("the drive upload lock");
        sending
            .insert(backup)
            .then(|| Sending { held: Arc::clone(&self.sending), backup })
    }
}

pub struct Sending {
    held: Arc<Mutex<std::collections::BTreeSet<Id>>>,
    backup: Id,
}

impl Drop for Sending {
    fn drop(&mut self) {
        self.held.lock().expect("the drive upload lock").remove(&self.backup);
    }
}

enum Carried {
    Fresh,
    At { session: String, from: u64, carried: upload::Digests },
    Finished { file: upload::Uploaded, whole: upload::Digests },
}

struct Marks<'a> {
    account: &'a Account,
    backup: Id,
}

impl upload::Ledger for Marks<'_> {
    fn offered(&self, upto: u64, proof: String) -> futures::future::BoxFuture<'_, ()> {
        Box::pin(self.account.note_offer(self.backup, upto, proof))
    }
}

pub struct Drive {
    shared: Arc<Shared>,
    users: Mutex<HashMap<Id, Arc<Account>>>,
}

impl Drive {
    pub fn new(pool: SqlitePool, config: Arc<Config>) -> anyhow::Result<Arc<Self>> {
        let http = Http::against(http::OAUTH, http::API).map_err(|err| anyhow::anyhow!("{err}"))?;
        Ok(Self::with(pool, &config.data_dir, http))
    }

    #[cfg(test)]
    pub(crate) fn against(
        pool: SqlitePool,
        data_dir: &Path,
        oauth: &str,
        api: &str,
    ) -> Arc<Self> {
        let http = Http::against(oauth, api).expect("a client against a local address");
        Self::with(pool, data_dir, http)
    }

    fn with(pool: SqlitePool, data_dir: &Path, http: Http) -> Arc<Self> {
        Arc::new(Self {
            shared: Arc::new(Shared {
                pool,
                keys: Keys::in_dir(data_dir.join("drive")),
                http,
                link_slots: Arc::new(Semaphore::new(LINK_LOOPS)),
                sending: Arc::default(),
            }),
            users: Mutex::default(),
        })
    }

    pub fn of(&self, user: Id) -> Arc<Account> {
        let mut users = self.users.lock().expect("the drive users lock");
        Arc::clone(
            users
                .entry(user)
                .or_insert_with(|| Account::new(Arc::clone(&self.shared), user)),
        )
    }

    pub fn start(self: &Arc<Self>) {
        let this = Arc::clone(self);
        tokio::spawn(async move {
            if let Err(err) = this.pick_up().await {
                tracing::warn!("the Google Drive accounts could not be picked up: {err}");
            }
        });
    }

    async fn pick_up(self: &Arc<Self>) -> anyhow::Result<()> {
        let mut woken = 0;
        for user in store::connected(&self.shared.pool).await? {
            let account = self.of(user);
            let _ = store::clear_link(&self.shared.pool, user, Timestamp::now()).await;
            if account.has_token().await {
                account.engage();
                woken += 1;
            }
        }
        if woken > 0 {
            tracing::info!(accounts = woken, "Google Drive accounts picked up");
        }

        let held = self.sweep_sessions(Timestamp::now()).await;
        if held > 0 {
            tracing::info!(
                uploads = held,
                "half-finished uploads are waiting to be carried on where they stopped"
            );
        }
        Ok(())
    }

    pub async fn sweep_sessions(&self, now: Timestamp) -> usize {
        let expired = Timestamp::at(now.as_datetime() - SESSION_LIFE);
        for old in store::uploads_opened_before(&self.shared.pool, expired).await.unwrap_or_default()
        {
            tracing::info!(
                user = %old.user_id, backup = %old.backup_id,
                "an upload session ran out of Google's week and is being let go"
            );
            self.let_go_of(old.user_id, old.backup_id).await;
        }

        let live = store::uploads(&self.shared.pool).await.unwrap_or_default();
        for user in store::connected(&self.shared.pool).await.unwrap_or_default() {
            for backup in self.shared.keys.sessions_of(user).await {
                if live.iter().any(|row| row.backup_id == backup && row.user_id == user) {
                    continue;
                }
                tracing::info!(
                    %user, %backup,
                    "an upload address on the disk belongs to no backup any more and is being wiped"
                );
                let _ = self.shared.keys.forget_session(user, backup).await;
            }
        }
        live.len()
    }

    async fn let_go_of(&self, user: Id, backup: Id) {
        let _ = self.shared.keys.forget_session(user, backup).await;
        let _ = store::forget_upload(&self.shared.pool, backup).await;
    }

    pub async fn resumable(&self, backup: Id, archive: &Path, now: Timestamp) -> Option<u64> {
        let row = store::upload_of(&self.shared.pool, backup).await.ok().flatten()?;
        if now.as_datetime() - row.opened_at.as_datetime() >= SESSION_LIFE {
            return None;
        }
        self.shared.keys.read_session(row.user_id, backup).await?;
        let on_disk = store::print_of(archive).await?;
        (on_disk == row.print()).then_some(on_disk.bytes)
    }

    pub async fn forget_session(&self, backup: Id) {
        let Ok(Some(row)) = store::upload_of(&self.shared.pool, backup).await else { return };
        self.let_go_of(row.user_id, backup).await;
    }

    pub async fn settings(&self) -> Result<store::Settings> {
        store::settings(&self.shared.pool).await
    }

    pub async fn panel_configured(&self) -> bool {
        self.credentials().await.is_some()
    }

    async fn credentials(&self) -> Option<Credentials> {
        let settings = store::settings(&self.shared.pool).await.ok()?;
        Some(Credentials {
            client_id: settings.client_id?,
            client_secret: self.shared.keys.read_client_secret().await?,
        })
    }

    pub async fn save(
        &self,
        client_id: Option<String>,
        secret: SecretChange,
        policy: BackupTargetPolicy,
        folder_name: String,
        now: Timestamp,
    ) -> Result<DriveAdminOverview> {
        let folder_name = folder_name.trim().to_owned();
        if folder_name.is_empty() || folder_name.chars().count() > 96 {
            return Err(Failure::bad_request(
                "invalid_request",
                "the folder name has to be between 1 and 96 characters",
            ));
        }
        let client_id = client_id.map(|id| id.trim().to_owned()).filter(|id| !id.is_empty());

        let secret = match secret {
            SecretChange::Keep => None,
            SecretChange::Remove => Some(None),
            SecretChange::Replace(text) => Some(Some(Secret::parse(&text).ok_or_else(|| {
                Failure::bad_request("invalid_request", "the client secret is empty")
            })?)),
        };

        store::save_settings(
            &self.shared.pool,
            client_id.as_deref(),
            policy,
            &folder_name,
            now,
        )
        .await?;

        match secret {
            Some(Some(secret)) => self
                .shared
                .keys
                .write_client_secret(&secret)
                .await
                .map_err(|err| Failure::internal(anyhow::Error::from(err)))?,
            Some(None) => self
                .shared
                .keys
                .forget_client_secret()
                .await
                .map_err(|err| Failure::internal(anyhow::Error::from(err)))?,
            None => {}
        }
        self.admin_overview().await
    }

    pub async fn forget_credentials(&self, now: Timestamp) -> Result<()> {
        let settings = store::settings(&self.shared.pool).await?;
        store::save_settings(
            &self.shared.pool,
            None,
            settings.target_policy,
            &settings.folder_name,
            now,
        )
        .await?;
        self.shared
            .keys
            .forget_client_secret()
            .await
            .map_err(|err| Failure::internal(anyhow::Error::from(err)))
    }

    pub async fn admin_overview(&self) -> Result<DriveAdminOverview> {
        let settings = store::settings(&self.shared.pool).await?;
        let rows = store::overview(&self.shared.pool).await?;
        let usage = store::usage(&self.shared.pool).await?;
        let today =
            store::sent_today_by_everybody(&self.shared.pool, &day::day_of(Timestamp::now()))
                .await?;

        let accounts = rows
            .into_iter()
            .map(|row| {
                let mine = usage.iter().find(|entry| entry.user_id == row.user_id);
                DriveOverview {
                    user_id: row.user_id,
                    username: row.username,
                    state: row.state,
                    google_email: row.google_email,
                    storage_limit_bytes: row.storage_limit_bytes.map(|bytes| bytes.max(0) as u64),
                    storage_usage_bytes: row.storage_usage_bytes.map(|bytes| bytes.max(0) as u64),
                    uploaded_today_bytes: today
                        .iter()
                        .find(|entry| entry.user_id == row.user_id)
                        .map(|entry| entry.bytes.max(0) as u64)
                        .unwrap_or(0),
                    daily_upload_limit_bytes: day::CEILING,
                    backups: mine.map(|entry| entry.backups.max(0) as u32).unwrap_or(0),
                    backup_bytes: mine.map(|entry| entry.backup_bytes.max(0) as u64).unwrap_or(0),
                    last_error: row.last_error,
                    checked_at: row.checked_at,
                }
            })
            .collect();

        Ok(DriveAdminOverview {
            configured: self.panel_configured().await,
            client_id: settings.client_id,
            target_policy: settings.target_policy,
            folder_name: settings.folder_name,
            accounts,
        })
    }

    pub async fn target_of(&self, server: Id) -> Result<BackupTarget> {
        let settings = store::settings(&self.shared.pool).await?;
        let asked = store::target(&self.shared.pool, server).await?;
        let owner = self.owner_of(server).await?;

        let panel = self.panel_configured().await;
        let connected = self.usable(owner).await;
        let (effective, reason) = match settings.target_policy {
            BackupTargetPolicy::LocalOnly => (BackupLocation::Local, BackupTargetReason::Policy),

            BackupTargetPolicy::DriveOnly => (
                BackupLocation::Drive,
                if !panel {
                    BackupTargetReason::NotConfigured
                } else if !connected {
                    BackupTargetReason::NotConnected
                } else {
                    BackupTargetReason::Policy
                },
            ),

            BackupTargetPolicy::UserChoice if !panel => {
                (BackupLocation::Local, BackupTargetReason::NotConfigured)
            }
            BackupTargetPolicy::UserChoice if !connected => {
                (BackupLocation::Local, BackupTargetReason::NotConnected)
            }
            BackupTargetPolicy::UserChoice => (asked, BackupTargetReason::Ok),
        };

        Ok(BackupTarget {
            target: asked,
            effective_target: effective,
            policy: settings.target_policy,
            reason,
        })
    }

    pub async fn effective_target(&self, server: Id) -> Result<BackupLocation> {
        Ok(self.target_of(server).await?.effective_target)
    }

    pub async fn set_target(&self, server: Id, wanted: BackupLocation) -> Result<BackupTarget> {
        let settings = store::settings(&self.shared.pool).await?;
        let owner = self.owner_of(server).await?;

        let allowed = match settings.target_policy {
            BackupTargetPolicy::UserChoice => true,
            BackupTargetPolicy::DriveOnly => wanted == BackupLocation::Drive,
            BackupTargetPolicy::LocalOnly => wanted == BackupLocation::Local,
        };
        if !allowed {
            return Err(Failure::conflict(
                "target_not_allowed",
                match settings.target_policy {
                    BackupTargetPolicy::DriveOnly => {
                        "this panel only backs up into the user's own Google Drive"
                    }
                    _ => "this panel only backs up to its own disk",
                },
            ));
        }

        if wanted == BackupLocation::Drive {
            if !self.panel_configured().await {
                return Err(not_configured());
            }
            if !self.usable(owner).await {
                return Err(not_connected());
            }
        }

        store::set_target(&self.shared.pool, server, wanted, Timestamp::now()).await?;
        self.target_of(server).await
    }

    pub async fn guard_backup(&self, server: Id) -> Result<()> {
        if self.effective_target(server).await? != BackupLocation::Drive {
            return Ok(());
        }
        if !self.panel_configured().await {
            return Err(not_configured());
        }
        let owner = self.owner_of(server).await?;
        if !self.usable(owner).await {
            return Err(not_connected());
        }
        self.day_has_room(owner).await
    }

    async fn day_has_room(&self, owner: Id) -> Result<()> {
        let today = day::day_of(Timestamp::now());
        let tally = day::Tally::of(store::sent_today(&self.shared.pool, owner, &today).await?);
        if tally.full() {
            return Err(upstream(tally.reached()));
        }
        Ok(())
    }

    async fn usable(&self, owner: Id) -> bool {
        if !self.of(owner).has_token().await {
            return false;
        }
        store::account(&self.shared.pool, owner)
            .await
            .ok()
            .flatten()
            .and_then(|row| row.state)
            .is_none_or(|state| state != DriveAccountState::Revoked)
    }

    async fn owner_of(&self, server: Id) -> Result<Id> {
        store::owner_of(&self.shared.pool, server).await?.ok_or_else(|| {
            Failure::not_found("server_not_found", "this server does not exist any more")
        })
    }

    pub async fn upload_archive(
        &self,
        server: Id,
        backup: Id,
        path: &Path,
        size: u64,
        name: &str,
        progress: &Progress,
    ) -> std::result::Result<Stored, DriveError> {
        let owner = self
            .owner_of(server)
            .await
            .map_err(|_| DriveError::Unreachable("this server has no owner any more".to_owned()))?;
        self.of(owner).upload(server, backup, path, size, name, progress).await
    }

    pub async fn fetch_archive(
        &self,
        server: Id,
        file_id: &str,
        into: &Path,
        progress: &Progress,
        recorded: Recorded<'_>,
        acknowledge_abuse: bool,
    ) -> std::result::Result<u64, DriveError> {
        let owner = self
            .owner_of(server)
            .await
            .map_err(|_| DriveError::Unreachable("this server has no owner any more".to_owned()))?;
        self.of(owner).fetch(file_id, into, progress, recorded, acknowledge_abuse).await
    }

    pub async fn remove_archive(&self, server: Id, file_id: &str) {
        let Ok(owner) = self.owner_of(server).await else { return };
        if let Err(err) = self.of(owner).remove(file_id).await {
            tracing::warn!(
                %server, file = %file_id,
                "an archive stays behind in the user's Google Drive: {err}"
            );
        }
    }

    pub async fn size_of(
        &self,
        server: Id,
        file_id: &str,
    ) -> std::result::Result<files::File, DriveError> {
        let owner = self
            .owner_of(server)
            .await
            .map_err(|_| DriveError::Unreachable("this server has no owner any more".to_owned()))?;
        self.of(owner).describe(file_id).await
    }

    pub async fn dispose_of(&self, user: Id) {
        self.of(user).dispose().await;
        self.users.lock().expect("the drive users lock").remove(&user);
    }
}

pub struct Account {
    shared: Arc<Shared>,
    user: Id,
    access: tokio::sync::RwLock<Option<Access>>,
    renewing: tokio::sync::Mutex<()>,
    linking: tokio::sync::Mutex<Option<JoinHandle<()>>>,
    sweeping: tokio::sync::Mutex<Option<JoinHandle<()>>>,
}

impl Account {
    fn new(shared: Arc<Shared>, user: Id) -> Arc<Self> {
        Arc::new(Self {
            shared,
            user,
            access: tokio::sync::RwLock::default(),
            renewing: tokio::sync::Mutex::default(),
            linking: tokio::sync::Mutex::default(),
            sweeping: tokio::sync::Mutex::default(),
        })
    }

    pub async fn has_token(&self) -> bool {
        tokio::fs::metadata(self.shared.keys.refresh_token_path(self.user)).await.is_ok()
    }

    pub async fn status(&self) -> Result<DriveStatus> {
        let settings = store::settings(&self.shared.pool).await?;
        let row = store::account(&self.shared.pool, self.user).await?;
        let today = day::day_of(Timestamp::now());
        let sent_today = store::sent_today(&self.shared.pool, self.user, &today).await?;
        let link = row.as_ref().and_then(|row| row.link()).map(|link| DriveLink {
            user_code: link.user_code,
            verification_url: DEVICE_PAGE.to_owned(),
            state: link.state,
            started_at: link.started_at,
            expires_at: link.expires_at,
            interval: 5,
        });

        Ok(DriveStatus {
            panel_configured: self.panel_configured().await,
            configured: self.has_token().await,
            state: row.as_ref().and_then(|row| row.state),
            google_name: row.as_ref().and_then(|row| row.google_name.clone()),
            google_email: row.as_ref().and_then(|row| row.google_email.clone()),
            folder_name: settings.folder_name,
            storage_limit_bytes: row
                .as_ref()
                .and_then(|row| row.storage_limit_bytes)
                .map(|bytes| bytes.max(0) as u64),
            storage_usage_bytes: row
                .as_ref()
                .and_then(|row| row.storage_usage_bytes)
                .map(|bytes| bytes.max(0) as u64),
            uploaded_today_bytes: sent_today,
            daily_upload_limit_bytes: day::CEILING,
            link,
            last_error: row.as_ref().and_then(|row| row.last_error.clone()),
            checked_at: row.as_ref().and_then(|row| row.checked_at),
        })
    }

    pub async fn link(&self) -> Result<DriveLink> {
        self.status()
            .await?
            .link
            .ok_or_else(|| Failure::not_found("drive_link_not_found", "no attempt is under way"))
    }

    pub async fn begin_link(self: &Arc<Self>) -> Result<DriveLink> {
        self.require_external().await?;
        let credentials = self.credentials().await.ok_or_else(not_configured)?;
        if self.has_token().await {
            return Err(Failure::conflict(
                "drive_already_linked",
                "this account has already connected a Google Drive",
            ));
        }

        self.stop_link().await;

        let device = oauth::begin(&self.shared.http, &credentials).await.map_err(upstream_link)?;
        let now = Timestamp::now();
        let row = store::Link {
            user_code: device.user_code.clone(),
            state: DriveLinkState::Waiting,
            started_at: now,
            expires_at: device.expires_at,
        };
        store::begin_link(&self.shared.pool, self.user, &row, now).await?;

        let view = DriveLink {
            user_code: device.user_code.clone(),
            verification_url: device.verification_url.clone(),
            state: DriveLinkState::Waiting,
            started_at: now,
            expires_at: device.expires_at,
            interval: device.interval.as_secs(),
        };
        self.watch_link(device, credentials).await;
        Ok(view)
    }

    pub async fn cancel_link(&self) -> Result<()> {
        if !store::clear_link(&self.shared.pool, self.user, Timestamp::now()).await? {
            return Err(Failure::not_found("drive_link_not_found", "no attempt is under way"));
        }
        self.stop_link().await;
        Ok(())
    }

    async fn watch_link(self: &Arc<Self>, device: oauth::Device, credentials: Credentials) {
        let mut linking = self.linking.lock().await;
        if let Some(task) = linking.take() {
            task.abort();
        }
        *linking = Some(tokio::spawn(Arc::clone(self).poll_link(device, credentials)));
    }

    async fn stop_link(&self) {
        if let Some(task) = self.linking.lock().await.take() {
            task.abort();
        }
    }

    async fn poll_link(self: Arc<Self>, device: oauth::Device, credentials: Credentials) {
        let Ok(_turn) = Arc::clone(&self.shared.link_slots).acquire_owned().await else { return };
        let mut wait = device.interval;

        loop {
            tokio::time::sleep(wait).await;

            if Timestamp::now() >= device.expires_at {
                self.settle_link(&device.user_code, oauth::ending(&DriveError::Expired)).await;
                return;
            }
            let ours = store::account(&self.shared.pool, self.user).await.is_ok_and(|row| {
                row.and_then(|row| row.link()).is_some_and(|link| link.user_code == device.user_code)
            });
            if !ours {
                return;
            }

            match oauth::poll(&self.shared.http, &credentials, &device.device_code).await {
                Ok(tokens) => {
                    self.adopt(tokens, &device.user_code).await;
                    return;
                }
                Err(DriveError::Pending { slow_down }) => {
                    if slow_down {
                        wait = (wait * 2).min(Duration::from_secs(60));
                    }
                }
                Err(DriveError::RateLimited) => wait = Duration::from_secs(60),
                Err(other) => {
                    self.settle_link(&device.user_code, oauth::ending(&other)).await;
                    return;
                }
            }
        }
    }

    async fn settle_link(&self, code: &str, ending: oauth::Ending) {
        let now = Timestamp::now();
        let _ = store::advance_link(
            &self.shared.pool,
            self.user,
            code,
            ending.state,
            &ending.sentence,
            now,
        )
        .await;
        tracing::info!(user = %self.user, state = %ending.state, "{}", ending.sentence);
    }

    async fn adopt(self: &Arc<Self>, tokens: oauth::Tokens, code: &str) {
        let now = Timestamp::now();
        if let Err(err) = self.shared.keys.write_refresh_token(self.user, &tokens.refresh).await {
            self.settle_link(
                code,
                oauth::Ending {
                    state: DriveLinkState::Expired,
                    sentence: format!(
                        "Google confirmed the code, but the panel could not keep the connection: \
                         {err}. Nothing is connected; try again."
                    ),
                },
            )
            .await;
            return;
        }
        *self.access.write().await = Some(tokens.access);
        let _ = store::connect(&self.shared.pool, self.user, None, now).await;
        tracing::info!(user = %self.user, "a Google Drive was connected");

        if let Err(err) = self.check().await {
            tracing::warn!(user = %self.user, "the fresh Google connection did not answer: {err}");
        }
        self.engage();
    }

    pub async fn check(self: &Arc<Self>) -> Result<DriveStatus> {
        self.require_external().await?;
        if !self.has_token().await {
            return Err(not_connected());
        }
        match self.reconcile().await {
            Ok(()) => self.status().await,
            Err(err) if err.is_revoked() => self.status().await,
            Err(err) => Err(upstream(err)),
        }
    }

    async fn reconcile(self: &Arc<Self>) -> std::result::Result<(), DriveError> {
        let over = self.shared.http.briefly("the look into the Drive");
        let access = self.access_while(&over).await?;
        let who = files::about(&self.shared.http, &access, &over).await?;
        let now = Timestamp::now();
        let _ = store::record_check(&self.shared.pool, self.user, &who, now).await;
        self.take_stock(&access, now).await
    }

    async fn take_stock(
        &self,
        access: &Access,
        now: Timestamp,
    ) -> std::result::Result<(), DriveError> {
        let over = self.shared.http.briefly("the look into the Drive");
        let theirs = files::ours(&self.shared.http, access, &over).await?;
        let ours = store::backups_of(&self.shared.pool, self.user).await.map_err(|_| {
            DriveError::Unreachable("the backups of this account could not be read".to_owned())
        })?;

        for row in &ours {
            let Some(file_id) = row.drive_file_id.as_deref() else { continue };
            let seen = theirs.iter().find(|file| file.id == file_id);
            let state = match seen {
                Some(file) if file.trashed => DriveFileState::Trashed,
                Some(_) => DriveFileState::Present,
                None => DriveFileState::Missing,
            };
            if row.drive_state != Some(state) {
                if state == DriveFileState::Missing {
                    tracing::info!(
                        user = %self.user, backup = %row.id,
                        "an archive is no longer in the user's Google Drive"
                    );
                }
                let _ = store::set_file_state(&self.shared.pool, row.id, state, now).await;
            }

            let named = seen.and_then(|file| file.md5_checksum.as_deref());
            let differs = match (row.drive_md5.as_deref(), named) {
                (Some(ours), Some(named)) => !named.trim().eq_ignore_ascii_case(ours.trim()),
                _ => continue,
            };
            match (differs, row.drive_content_changed_at) {
                (true, None) => {
                    tracing::warn!(
                        user = %self.user, backup = %row.id, file = %file_id,
                        "the file of this backup in the user's Google Drive no longer holds the \
                         archive the panel put there"
                    );
                    let _ = store::set_content_changed(&self.shared.pool, row.id, Some(now)).await;
                }
                (false, Some(_)) => {
                    tracing::info!(
                        user = %self.user, backup = %row.id, file = %file_id,
                        "the file of this backup holds the archive the panel put there again"
                    );
                    let _ = store::set_content_changed(&self.shared.pool, row.id, None).await;
                }
                _ => {}
            }
        }

        let folder = store::account(&self.shared.pool, self.user)
            .await
            .ok()
            .flatten()
            .and_then(|row| row.folder_id);
        for file in &theirs {
            if Some(file.id.as_str()) == folder.as_deref() {
                continue;
            }
            let Some(backup) = file.backup_id() else { continue };
            if ours.iter().any(|row| row.drive_file_id.as_deref() == Some(file.id.as_str())) {
                continue;
            }
            tracing::info!(
                user = %self.user, file = %file.id, %backup,
                "an archive in the user's Google Drive belongs to no backup and is being removed"
            );
            let _ = files::delete(&self.shared.http, access, &file.id, &over).await;
        }
        Ok(())
    }

    pub async fn disconnect(self: &Arc<Self>, mode: Files) -> Result<()> {
        let mine = store::backups_of(&self.shared.pool, self.user).await?;

        if mode == Files::Refuse && !mine.is_empty() {
            return Err(Failure::conflict(
                "drive_has_backups",
                "backups of yours lie in your Google Drive; say whether to delete them there \
                 or leave them where they are",
            ));
        }
        if !self.has_token().await {
            return Err(not_connected());
        }

        let now = Timestamp::now();
        if mode == Files::Delete {
            self.require_external().await?;
            let over = self.shared.http.briefly("the clearing out of the Drive");
            let access = self.access_while(&over).await.map_err(upstream)?;
            for row in &mine {
                if let Some(file) = row.drive_file_id.as_deref() {
                    files::delete(&self.shared.http, &access, file, &over)
                        .await
                        .map_err(upstream)?;
                }
            }
            store::forget_drive_backups(&self.shared.pool, self.user).await?;
        } else {
            store::mark_unreachable(&self.shared.pool, self.user, now).await?;
        }

        if self.external_on().await {
            if let Some(token) = self.shared.keys.read_refresh_token(self.user).await {
                if let Err(err) = oauth::revoke(&self.shared.http, &token).await {
                    tracing::warn!(
                        user = %self.user,
                        "Google would not take the token back: {err}"
                    );
                }
            }
        }

        self.stop_link().await;
        self.stop_sweep().await;
        *self.access.write().await = None;
        for backup in store::forget_uploads_of(&self.shared.pool, self.user).await? {
            let _ = self.shared.keys.forget_session(self.user, backup).await;
        }
        for backup in self.shared.keys.sessions_of(self.user).await {
            let _ = self.shared.keys.forget_session(self.user, backup).await;
        }
        self.shared
            .keys
            .forget_refresh_token(self.user)
            .await
            .map_err(|err| Failure::internal(anyhow::Error::from(err)))?;
        store::forget_account(&self.shared.pool, self.user).await?;
        Ok(())
    }

    async fn dispose(self: &Arc<Self>) {
        let _ = self.disconnect(Files::Keep).await;
        let _ = self.shared.keys.forget_user(self.user).await;
    }

    async fn upload(
        &self,
        server: Id,
        backup: Id,
        path: &Path,
        size: u64,
        name: &str,
        progress: &Progress,
    ) -> std::result::Result<Stored, DriveError> {
        let _turn = self.shared.claim(backup).ok_or(DriveError::Busy)?;
        let budget = self.shared.http.over_the_run();
        let over = self
            .shared
            .http
            .patiently("sending an archive up")
            .watched_by(progress)
            .within(&budget);
        let access = self.access_while(&over).await?;
        let print = store::print_of(path).await.ok_or_else(|| {
            DriveError::Unreachable("the archive to send is not on this machine".to_owned())
        })?;

        let today = day::day_of(Timestamp::now());
        let sent_before =
            store::sent_today(&self.shared.pool, self.user, &today).await.unwrap_or(0);
        let tally = day::Tally::of(sent_before);
        if tally.full() {
            return Err(self.held_back(tally.reached()).await);
        }

        let (mut session, mut from, mut carried) =
            match self.carry_on(&access, backup, path, size, print, &over).await? {
                Carried::Finished { file, whole } => {
                    self.let_go(backup).await;
                    let sent = upload::Sent::of(file, &whole, size);
                    return self.confirm(&access, sent, size).await;
                }
                Carried::At { session, from, carried } => (session, from, carried),
                Carried::Fresh => {
                    let fresh = self
                        .open_session(&access, server, backup, name, size, print, &over)
                        .await?;
                    (fresh.0, fresh.1, upload::Digests::default())
                }
            };

        let mut begun_again = false;
        let marks = Marks { account: self, backup };
        let carry =
            upload::Carry { bearer: self, tally: &tally, ledger: &marks, progress, over };
        let sent = loop {
            let started_at = session.clone();
            let outcome = upload::send(
                &self.shared.http,
                &carry,
                &mut session,
                path,
                size,
                from,
                carried.clone(),
            )
            .await;

            match outcome {
                Ok(sent) => break sent,
                Err(DriveError::SessionOver) if !begun_again => {
                    begun_again = true;
                    self.let_go(backup).await;
                    tracing::info!(
                        user = %self.user, %backup,
                        "Google let this upload session run out, so the archive goes up again \
                         from the front in the same run"
                    );
                    progress.back_to(0);
                    let fresh = self
                        .open_session(&access, server, backup, name, size, print, &over)
                        .await?;
                    session = fresh.0;
                    from = fresh.1;
                    carried = upload::Digests::default();
                }
                Err(err) => {
                    self.note_day(&today, &tally).await;
                    if matches!(err, DriveError::SessionOver | DriveError::Cancelled) {
                        self.let_go(backup).await;
                    } else if session != started_at {
                        self.moved_to(backup, &session).await;
                    }
                    if matches!(err, DriveError::DayFull(_) | DriveError::Throttled(_)) {
                        return Err(self.held_back(err).await);
                    }
                    return Err(err);
                }
            }
        };
        self.note_day(&today, &tally).await;
        self.let_go(backup).await;
        let access = self.access_while(&over).await?;
        if store::print_of(path).await != Some(print) {
            return Err(self
                .throw_away(
                    &access,
                    &sent.file.id,
                    "the archive on the disk was written over while it was going up, so what \
                     lies in the Drive is the front of one archive and the back of another"
                        .to_owned(),
                )
                .await);
        }
        self.confirm(&access, sent, size).await
    }

    async fn note_day(&self, today: &str, tally: &day::Tally) {
        let _ = store::note_sent(
            &self.shared.pool,
            self.user,
            today,
            tally.added(),
            Timestamp::now(),
        )
        .await;
    }

    async fn held_back(&self, err: DriveError) -> DriveError {
        let _ =
            store::note_holdup(&self.shared.pool, self.user, &err.to_string(), Timestamp::now())
                .await;
        tracing::warn!(user = %self.user, "an upload was held back rather than pressed on: {err}");
        err
    }

    async fn room_for(
        &self,
        access: &Access,
        size: u64,
        over: &retry::Waiting<'_>,
    ) -> std::result::Result<(), DriveError> {
        let who = files::about(&self.shared.http, access, over).await?;
        let _ = store::record_check(&self.shared.pool, self.user, &who, Timestamp::now()).await;

        let Some(limit) = who.limit_bytes else {
            return Ok(());
        };
        let free = limit.saturating_sub(who.usage_bytes.unwrap_or(0));
        if size <= free {
            return Ok(());
        }
        Err(DriveError::QuotaFull(format!(
            "the archive is {size} bytes and this Drive has {free} of its {limit} bytes free, so \
             Google would turn it away partway through rather than at the door"
        )))
    }

    async fn open_session(
        &self,
        access: &Access,
        server: Id,
        backup: Id,
        name: &str,
        size: u64,
        print: store::Print,
        over: &retry::Waiting<'_>,
    ) -> std::result::Result<(String, u64, Timestamp), DriveError> {
        self.room_for(access, size, over).await?;
        let folder = self.folder(access, over).await?;
        let session = upload::begin(
            &self.shared.http,
            access,
            &upload::NewFile {
                name: name.to_owned(),
                parent: Some(folder),
                server_id: server.to_string(),
                backup_id: backup.to_string(),
            },
            size,
            over,
        )
        .await?;
        let opened = Timestamp::now();
        self.remember(backup, &session, print, opened).await;
        Ok((session, 0, opened))
    }

    async fn carry_on(
        &self,
        access: &Access,
        backup: Id,
        path: &Path,
        size: u64,
        print: store::Print,
        over: &retry::Waiting<'_>,
    ) -> std::result::Result<Carried, DriveError> {
        let Ok(Some(row)) = store::upload_of(&self.shared.pool, backup).await else {
            return Ok(Carried::Fresh);
        };

        if row.user_id != self.user || row.total_bytes.max(0) as u64 != size || row.print() != print
        {
            tracing::warn!(
                user = %self.user, %backup,
                "an upload session was opened for a different archive than the one on the disk; \
                 it is thrown away rather than spliced onto this one"
            );
            self.let_go(backup).await;
            return Ok(Carried::Fresh);
        }
        if Timestamp::now().as_datetime() - row.opened_at.as_datetime() >= SESSION_LIFE {
            self.let_go(backup).await;
            return Ok(Carried::Fresh);
        }
        let Some(address) = self.shared.keys.read_session(self.user, backup).await else {
            self.let_go(backup).await;
            return Ok(Carried::Fresh);
        };

        match upload::standing(&self.shared.http, access, address.expose(), size, over).await {
            Ok(upload::Chunk::Done(file)) => {
                let Some(whole) = self.proven(path, size, &row).await else {
                    self.let_go(backup).await;
                    return Ok(Carried::Fresh);
                };
                tracing::info!(
                    user = %self.user, %backup,
                    "Google had the whole archive already; only the answer was lost"
                );
                Ok(Carried::Finished { file, whole })
            }
            Ok(upload::Chunk::More { received, moved }) => {
                let carried = match received {
                    0 => upload::Digests::default(),
                    _ => match self.proven(path, received, &row).await {
                        Some(carried) => carried,
                        None => {
                            self.let_go(backup).await;
                            return Ok(Carried::Fresh);
                        }
                    },
                };
                let session = match moved {
                    Some(fresh) => {
                        self.moved_to(backup, &fresh).await;
                        fresh
                    }
                    None => address.expose().to_owned(),
                };
                tracing::info!(
                    user = %self.user, %backup, received, size,
                    "an upload carries on where the panel stopped"
                );
                Ok(Carried::At { session, from: received, carried })
            }
            Err(DriveError::SessionOver) => {
                tracing::info!(
                    user = %self.user, %backup,
                    "Google no longer knows this upload session; it begins again from the front"
                );
                self.let_go(backup).await;
                Ok(Carried::Fresh)
            }
            Err(other) => Err(other),
        }
    }

    async fn proven(
        &self,
        path: &Path,
        keep: u64,
        row: &store::Upload,
    ) -> Option<upload::Digests> {
        let backup = row.backup_id;
        let Some((offered, proof)) = row.offer() else {
            tracing::warn!(
                user = %self.user, %backup,
                "nothing was written down about what went into this upload session, so what \
                 Google holds cannot be shown to be this archive; it begins again from the front"
            );
            return None;
        };
        if keep > offered {
            tracing::warn!(
                user = %self.user, %backup, keep, offered,
                "Google holds more of this upload than the panel ever offered it; the part \
                 beyond cannot be checked, so the archive begins again from the front"
            );
            return None;
        }

        let prefix = match upload::prefix_of(path, keep, offered).await {
            Ok(prefix) => prefix,
            Err(err) => {
                tracing::warn!(
                    user = %self.user, %backup,
                    "the archive could not be read back to prove what Google already holds: {err}"
                );
                return None;
            }
        };
        if !prefix.proof.eq_ignore_ascii_case(proof) {
            tracing::warn!(
                user = %self.user, %backup, offered,
                "the archive on the disk is no longer the one this upload session was fed; the \
                 two are not spliced into one file, and it begins again from the front"
            );
            return None;
        }
        Some(prefix.carried)
    }

    async fn remember(&self, backup: Id, address: &str, print: store::Print, opened: Timestamp) {
        if !self.moved_to(backup, address).await {
            return;
        }
        let _ = store::open_upload(
            &self.shared.pool,
            backup,
            self.user,
            print,
            opened,
            Timestamp::now(),
        )
        .await;
    }

    async fn moved_to(&self, backup: Id, address: &str) -> bool {
        let Some(secret) = Secret::parse(address) else {
            tracing::warn!(user = %self.user, %backup, "Google gave an empty session address");
            return false;
        };
        if let Err(err) = self.shared.keys.write_session(self.user, backup, &secret).await {
            tracing::warn!(
                user = %self.user, %backup,
                "this upload will not survive a restart of the panel: {err}"
            );
            return false;
        }
        true
    }

    async fn note_offer(&self, backup: Id, upto: u64, proof: String) {
        if let Err(err) =
            store::note_offer(&self.shared.pool, backup, upto, &proof, Timestamp::now()).await
        {
            tracing::warn!(
                user = %self.user, %backup,
                "how far this upload has come could not be written down, so a restart would \
                 begin again from the front: {err}"
            );
        }
    }

    async fn let_go(&self, backup: Id) {
        let _ = self.shared.keys.forget_session(self.user, backup).await;
        let _ = store::forget_upload(&self.shared.pool, backup).await;
    }

    pub async fn drop_expired_sessions(&self, now: Timestamp) {
        let expired = Timestamp::at(now.as_datetime() - SESSION_LIFE);
        let old = store::uploads_opened_before(&self.shared.pool, expired).await;
        for row in old.unwrap_or_default().into_iter().filter(|row| row.user_id == self.user) {
            tracing::info!(
                user = %self.user, backup = %row.backup_id,
                "an upload session ran out of Google's week and is let go"
            );
            self.let_go(row.backup_id).await;
        }
    }

    async fn confirm(
        &self,
        access: &Access,
        sent: upload::Sent,
        size: u64,
    ) -> std::result::Result<Stored, DriveError> {
        let over = self.shared.http.patiently("the word from Google on the archive");
        let mut held = sent.file.bytes();
        let id = sent.file.id;
        let mut named = strongest(sent.file.sha256_checksum, sent.file.md5_checksum);

        if named.is_none() {
            let seen = files::get(&self.shared.http, access, &id, &over).await.map_err(|err| {
                DriveError::Unconfirmed(format!(
                    "the archive went up, and Google would not say afterwards what it holds \
                     under {id}: {err}"
                ))
            })?;
            held = held.or_else(|| seen.bytes());
            named = strongest(seen.sha256_checksum, seen.md5_checksum);
        }

        if let Some(held) = held.filter(|held| *held != size) {
            return Err(self
                .throw_away(
                    access,
                    &id,
                    format!("Google kept {held} bytes of an archive that is {size} bytes long"),
                )
                .await);
        }
        if sent.covered != size {
            return Err(self
                .throw_away(
                    access,
                    &id,
                    format!(
                        "Google called the archive finished after {} of its {size} bytes",
                        sent.covered
                    ),
                )
                .await);
        }

        let Some((algorithm, theirs)) = named else {
            tracing::warn!(
                file = %id,
                "Google named no checksum for this archive, so nothing confirms what lies \
                 in the Drive"
            );
            return Ok(Stored { file_id: id, md5: None });
        };
        let ours = match algorithm {
            "sha256" => &sent.sha256,
            _ => &sent.md5,
        };
        if !theirs.trim().eq_ignore_ascii_case(ours) {
            return Err(self
                .throw_away(
                    access,
                    &id,
                    format!(
                        "the archive in the Drive hashes to {theirs} ({algorithm}), the one that \
                         left this machine to {ours}"
                    ),
                )
                .await);
        }
        Ok(Stored { file_id: id, md5: Some(sent.md5) })
    }

    async fn throw_away(&self, access: &Access, id: &str, why: String) -> DriveError {
        let over = self.shared.http.briefly("the removal of an archive nobody may trust");
        if let Err(err) = files::delete(&self.shared.http, access, id, &over).await {
            tracing::warn!(file = %id, "an archive nobody may trust stays in the Drive: {err}");
        }
        DriveError::Damaged(why)
    }

    async fn fetch(
        &self,
        file_id: &str,
        into: &Path,
        progress: &Progress,
        recorded: Recorded<'_>,
        acknowledge_abuse: bool,
    ) -> std::result::Result<u64, DriveError> {
        let over = self.shared.http.patiently("fetching the archive back").watched_by(progress);
        let access = self.access_while(&over).await?;
        let known = files::get(&self.shared.http, &access, file_id, &over).await?;
        if known.trashed {
            return Err(DriveError::Gone);
        }
        let whole = match (known.bytes(), recorded.bytes) {
            (Some(theirs), Some(ours)) if theirs != ours => {
                return Err(DriveError::Unreadable(format!(
                    "Google holds {theirs} bytes under {file_id}, and the archive that went up \
                     from here was {ours} bytes long"
                )));
            }
            (theirs, ours) => theirs.or(ours),
        };
        if known.is_app_authorized == Some(false) {
            tracing::warn!(
                user = %self.user, file = %file_id,
                "Google says this archive was never opened by this panel, so it may refuse to \
                 hand it back"
            );
        }

        let mark = mark_beside(into);
        let stamp = stamp_of(&known);
        let from = match stamp.as_deref() {
            Some(stamp) => resume_at(into, &mark, stamp, whole).await,
            None => {
                drop_half(into, &mark).await;
                0
            }
        };
        if let Some(stamp) = stamp.as_deref() {
            if let Err(err) = tokio::fs::write(&mark, stamp).await {
                tracing::warn!(
                    file = %file_id,
                    "a download that breaks off will have to start over: {err}"
                );
            }
        }
        if from > 0 {
            tracing::info!(
                user = %self.user, file = %file_id, from,
                "a download carries on where the last attempt stopped"
            );
        }

        let fetched = match files::download(
            &self.shared.http,
            &access,
            files::Fetch { id: file_id, into, from, acknowledge_abuse },
            progress,
            &over,
        )
        .await
        {
            Ok(fetched) => fetched,
            Err(DriveError::Cancelled) => {
                drop_half(into, &mark).await;
                return Err(DriveError::Cancelled);
            }
            Err(err) => return Err(err),
        };

        let here = tokio::fs::metadata(into).await.map(|seen| seen.len()).unwrap_or(0);
        if let Some(whole) = whole {
            if here < whole {
                return Err(DriveError::Unreachable(format!(
                    "Google broke off after {here} of {whole} bytes"
                )));
            }
            if here > whole {
                drop_half(into, &mark).await;
                return Err(DriveError::Unreadable(format!(
                    "Google sent {here} bytes of an archive it calls {whole} bytes long"
                )));
            }
        }

        let vouched = fetched.holds(&known);
        match vouched {
            Some(false) => {
                drop_half(into, &mark).await;
                return Err(DriveError::Unreadable(
                    "the archive that came down from Google is not the one it says it is"
                        .to_owned(),
                ));
            }
            None => tracing::warn!(
                file = %file_id,
                "Google named no checksum for this archive, so nothing confirms what came down"
            ),
            Some(true) => {}
        }
        match recorded.md5 {
            Some(ours) if !ours.trim().eq_ignore_ascii_case(&fetched.md5) => {
                drop_half(into, &mark).await;
                if vouched.is_some() {
                    let _ = store::note_content_changed(
                        &self.shared.pool,
                        file_id,
                        Timestamp::now(),
                    )
                    .await;
                }
                return Err(DriveError::Replaced(not_ours(
                    file_id,
                    ours.trim(),
                    &fetched.md5,
                    vouched.is_some(),
                )));
            }
            Some(_) => {}
            None => tracing::warn!(
                file = %file_id,
                "no checksum was written down when this archive went up, so nothing here can \
                 say whether the file in the Drive is still the one the panel put there"
            ),
        }
        tokio::fs::remove_file(&mark).await.ok();
        Ok(whole.unwrap_or(here))
    }

    async fn describe(&self, file_id: &str) -> std::result::Result<files::File, DriveError> {
        let over = self.shared.http.briefly("the look at the archive");
        let access = self.access_while(&over).await?;
        files::get(&self.shared.http, &access, file_id, &over).await
    }

    async fn remove(&self, file_id: &str) -> std::result::Result<(), DriveError> {
        let over = self.shared.http.briefly("the removal of the archive");
        let access = self.access_while(&over).await?;
        files::delete(&self.shared.http, &access, file_id, &over).await
    }

    async fn folder(
        &self,
        access: &Access,
        over: &retry::Waiting<'_>,
    ) -> std::result::Result<String, DriveError> {
        let row = store::account(&self.shared.pool, self.user).await.ok().flatten();
        if let Some(folder) = row.and_then(|row| row.folder_id) {
            return Ok(folder);
        }
        let name = store::settings(&self.shared.pool)
            .await
            .map(|settings| settings.folder_name)
            .unwrap_or_else(|_| "craftpanel-backups".to_owned());
        let folder = files::ensure_folder(&self.shared.http, access, &name, over).await?;
        let _ = store::set_folder(&self.shared.pool, self.user, &folder, Timestamp::now()).await;
        Ok(folder)
    }

    async fn access(&self) -> std::result::Result<Access, DriveError> {
        self.access_while(&self.shared.http.briefly("a fresh access token")).await
    }

    async fn access_while(
        &self,
        over: &retry::Waiting<'_>,
    ) -> std::result::Result<Access, DriveError> {
        self.token_unless(None, over).await
    }

    async fn held(&self, stale: Option<&Access>) -> Option<Access> {
        let access = self.access.read().await.clone()?;
        let good = access.usable(Timestamp::now())
            && stale.is_none_or(|stale| access.newer_than(stale));
        good.then_some(access)
    }

    async fn token_unless(
        &self,
        stale: Option<&Access>,
        over: &retry::Waiting<'_>,
    ) -> std::result::Result<Access, DriveError> {
        if let Some(access) = self.held(stale).await {
            return Ok(access);
        }
        let _turn = self.renewing.lock().await;
        if let Some(access) = self.held(stale).await {
            return Ok(access);
        }

        let credentials = self.credentials().await.ok_or_else(|| {
            DriveError::Unreachable("this panel has no Google project set up".to_owned())
        })?;
        let token = self
            .shared
            .keys
            .read_refresh_token(self.user)
            .await
            .ok_or_else(|| DriveError::Revoked("no Google Drive is connected".to_owned()))?;

        let over = over.doing("a fresh access token");
        match oauth::refresh(&self.shared.http, &credentials, &token, &over).await {
            Ok(access) => {
                *self.access.write().await = Some(access.clone());
                Ok(access)
            }
            Err(err) if err.is_revoked() => {
                self.mark_revoked(&err).await;
                Err(err)
            }
            Err(err) => Err(err),
        }
    }

    async fn mark_revoked(&self, err: &DriveError) {
        let now = Timestamp::now();
        let young = match self.shared.keys.token_written_at(self.user).await {
            Some(written) => oauth::looks_like_a_testing_project(
                Timestamp::at(time::OffsetDateTime::from(written)),
                now,
            ),
            None => false,
        };
        let sentence = if young {
            oauth::TESTING_HINT.to_owned()
        } else {
            format!("Google no longer accepts this connection: {err}. Connect again.")
        };

        *self.access.write().await = None;
        let _ = store::record_error(
            &self.shared.pool,
            self.user,
            DriveAccountState::Revoked,
            &sentence,
            now,
        )
        .await;
        tracing::warn!(
            user = %self.user,
            "a Google Drive connection was withdrawn and its owner has not been told"
        );
    }

    fn bearing(&self) -> retry::Waiting<'static> {
        self.shared.http.patiently("a fresh access token")
    }

    pub fn engage(self: &Arc<Self>) {
        let this = Arc::clone(self);
        tokio::spawn(async move {
            let mut sweeping = this.sweeping.lock().await;
            if sweeping.as_ref().is_some_and(|task| !task.is_finished()) {
                return;
            }
            *sweeping = Some(tokio::spawn(Arc::clone(&this).sweep_forever()));
        });
    }

    async fn stop_sweep(&self) {
        if let Some(task) = self.sweeping.lock().await.take() {
            task.abort();
        }
    }

    async fn sweep_forever(self: Arc<Self>) {
        tokio::time::sleep(offset_of(self.user, SWEEP_EVERY)).await;

        loop {
            if !self.has_token().await {
                return;
            }
            self.drop_expired_sessions(Timestamp::now()).await;
            let _ = store::forget_days_before(
                &self.shared.pool,
                self.user,
                &day::day_of(Timestamp::now()),
            )
            .await;
            if self.external_on().await {
                match self.reconcile().await {
                    Ok(()) => {}
                    Err(err) if err.is_revoked() => {}
                    Err(err) => {
                        let now = Timestamp::now();
                        let _ = store::record_error(
                            &self.shared.pool,
                            self.user,
                            DriveAccountState::Error,
                            &format!("Google did not answer: {err}"),
                            now,
                        )
                        .await;
                    }
                }
            }
            tokio::time::sleep(SWEEP_EVERY).await;
        }
    }

    async fn credentials(&self) -> Option<Credentials> {
        let settings = store::settings(&self.shared.pool).await.ok()?;
        Some(Credentials {
            client_id: settings.client_id?,
            client_secret: self.shared.keys.read_client_secret().await?,
        })
    }

    async fn panel_configured(&self) -> bool {
        self.credentials().await.is_some()
    }

    async fn external_on(&self) -> bool {
        crate::auth::settings::load(&self.shared.pool)
            .await
            .map(|settings| settings.external_services_enabled)
            .unwrap_or(false)
    }

    async fn require_external(&self) -> Result<()> {
        if self.external_on().await {
            return Ok(());
        }
        Err(Failure::conflict(
            "external_services_disabled",
            "this panel does not call out to other services",
        ))
    }

    #[cfg(test)]
    pub async fn write_token(&self, token: &str) {
        let token = Secret::parse(token).expect("a token");
        self.shared
            .keys
            .write_refresh_token(self.user, &token)
            .await
            .expect("writing the token");
        store::connect(&self.shared.pool, self.user, None, Timestamp::now())
            .await
            .expect("a row");
    }
}

impl Bearer for Account {
    fn token(&self) -> futures::future::BoxFuture<'_, std::result::Result<Access, DriveError>> {
        Box::pin(async move { self.token_unless(None, &self.bearing()).await })
    }

    fn renew<'a>(
        &'a self,
        stale: &'a Access,
    ) -> futures::future::BoxFuture<'a, std::result::Result<Access, DriveError>> {
        Box::pin(async move { self.token_unless(Some(stale), &self.bearing()).await })
    }
}

const DEVICE_PAGE: &str = "https://www.google.com/device";

fn mark_beside(part: &Path) -> std::path::PathBuf {
    std::path::PathBuf::from(format!("{}.source", part.display()))
}

fn stamp_of(file: &files::File) -> Option<String> {
    let checksum = file.md5_checksum.as_deref().or(file.sha256_checksum.as_deref())?;
    Some(format!("{} {} {}", file.id, file.bytes().unwrap_or(0), checksum.trim().to_lowercase()))
}

async fn resume_at(part: &Path, mark: &Path, stamp: &str, whole: Option<u64>) -> u64 {
    let here = tokio::fs::metadata(part).await.map(|seen| seen.len()).unwrap_or(0);
    let usable = here > 0
        && whole.is_none_or(|whole| here < whole)
        && tokio::fs::read_to_string(mark).await.is_ok_and(|seen| seen.trim() == stamp);
    if usable {
        return here;
    }
    drop_half(part, mark).await;
    0
}

pub async fn drop_half(part: &Path, mark: &Path) {
    tokio::fs::remove_file(part).await.ok();
    tokio::fs::remove_file(mark).await.ok();
}

pub async fn drop_the_part(part: &Path) {
    drop_half(part, &mark_beside(part)).await;
}

fn offset_of(user: Id, window: Duration) -> Duration {
    let spread = user
        .to_string()
        .bytes()
        .fold(0u64, |sum, byte| sum.wrapping_mul(31).wrapping_add(byte as u64));
    Duration::from_secs(spread % window.as_secs())
}

fn not_ours(file_id: &str, ours: &str, came_down: &str, google_agreed: bool) -> String {
    let head = format!(
        "the file {file_id} in this Google Drive is not the archive the panel put there: it \
         went up as {ours} (md5) and what lies there now is {came_down}"
    );
    if google_agreed {
        return format!(
            "{head}. Google names that same checksum for it, so the transfer was sound and the \
             file itself was written over in the Drive"
        );
    }
    format!(
        "{head}. Google named no checksum for it this time, so a broken transfer would look the \
         same from here; a second attempt tells the two apart"
    )
}

fn strongest(sha256: Option<String>, md5: Option<String>) -> Option<(&'static str, String)> {
    if let Some(sha256) = sha256.filter(|named| !named.trim().is_empty()) {
        return Some(("sha256", sha256));
    }
    md5.filter(|named| !named.trim().is_empty()).map(|md5| ("md5", md5))
}

fn not_configured() -> Failure {
    Failure::conflict(
        "drive_not_configured",
        "the operator has not set up Google Drive on this panel",
    )
}

fn not_connected() -> Failure {
    Failure::conflict("drive_not_connected", "no Google Drive is connected to this account")
}

fn upstream_link(err: DriveError) -> Failure {
    let sentence = oauth::ending(&err).sentence;
    match err {
        DriveError::RateLimited => {
            Failure::new(StatusCode::TOO_MANY_REQUESTS, "upstream_rate_limited", sentence)
        }
        _ => Failure::new(StatusCode::BAD_GATEWAY, "drive_unavailable", sentence),
    }
}

fn upstream(err: DriveError) -> Failure {
    match err {
        DriveError::RateLimited => Failure::new(
            StatusCode::TOO_MANY_REQUESTS,
            "upstream_rate_limited",
            "Google is turning us away for the moment; try again shortly",
        ),
        DriveError::Throttled(detail) => {
            Failure::new(StatusCode::TOO_MANY_REQUESTS, "drive_throttled", detail)
        }
        DriveError::DayFull(detail) => {
            Failure::new(StatusCode::TOO_MANY_REQUESTS, "drive_day_full", detail)
        }
        DriveError::QuotaFull(detail) => Failure::new(
            StatusCode::INSUFFICIENT_STORAGE,
            "drive_quota_exceeded",
            format!("the Google Drive of this account is full: {detail}"),
        ),
        other => Failure::new(StatusCode::BAD_GATEWAY, "drive_unavailable", other.to_string()),
    }
}

#[cfg(test)]
mod offsets {
    use super::*;

    #[test]
    fn the_turns_of_two_users_do_not_fall_on_the_same_second() {
        let mut seen = std::collections::HashSet::new();
        for _ in 0..64 {
            let offset = offset_of(Id::new(), SWEEP_EVERY);
            assert!(offset < SWEEP_EVERY, "an offset outside the window is a skipped round");
            seen.insert(offset.as_secs());
        }
        assert!(seen.len() > 8, "sixty-four users landed on {} seconds", seen.len());

        let same = Id::new();
        assert_eq!(
            offset_of(same, SWEEP_EVERY),
            offset_of(same, SWEEP_EVERY),
            "a restart must not reshuffle the turns"
        );
    }
}
