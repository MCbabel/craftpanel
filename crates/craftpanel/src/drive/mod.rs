#![allow(dead_code)]

pub mod files;
pub mod http;
pub mod keys;
pub mod oauth;
pub mod store;
pub mod upload;

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
use self::oauth::{Access, Credentials, Secret};

pub const PANEL_TAG: &str = "craftpanel";

pub const ARCHIVE_TYPE: &str = "application/zstd";

const LINK_LOOPS: usize = 4;

const SWEEP_EVERY: Duration = Duration::from_secs(60 * 60);

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
        Ok(())
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
    ) -> std::result::Result<String, DriveError> {
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
    ) -> std::result::Result<u64, DriveError> {
        let owner = self
            .owner_of(server)
            .await
            .map_err(|_| DriveError::Unreachable("this server has no owner any more".to_owned()))?;
        self.of(owner).fetch(file_id, into, progress).await
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
    linking: tokio::sync::Mutex<Option<JoinHandle<()>>>,
    sweeping: tokio::sync::Mutex<Option<JoinHandle<()>>>,
}

impl Account {
    fn new(shared: Arc<Shared>, user: Id) -> Arc<Self> {
        Arc::new(Self {
            shared,
            user,
            access: tokio::sync::RwLock::default(),
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
        let access = self.access().await?;
        let who = files::about(&self.shared.http, &access).await?;
        let now = Timestamp::now();
        let _ = store::record_check(&self.shared.pool, self.user, &who, now).await;
        self.take_stock(&access, now).await
    }

    async fn take_stock(
        &self,
        access: &Access,
        now: Timestamp,
    ) -> std::result::Result<(), DriveError> {
        let theirs = files::ours(&self.shared.http, access).await?;
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
            let _ = files::delete(&self.shared.http, access, &file.id).await;
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
            let access = self.access().await.map_err(upstream)?;
            for row in &mine {
                if let Some(file) = row.drive_file_id.as_deref() {
                    files::delete(&self.shared.http, &access, file).await.map_err(upstream)?;
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
    ) -> std::result::Result<String, DriveError> {
        let access = self.access().await?;
        let folder = self.folder(&access).await?;
        let session = upload::begin(
            &self.shared.http,
            &access,
            &upload::NewFile {
                name: name.to_owned(),
                parent: Some(folder),
                server_id: server.to_string(),
                backup_id: backup.to_string(),
            },
            size,
        )
        .await?;

        let uploaded =
            upload::send(&self.shared.http, &access, &session, path, size, progress).await?;
        Ok(uploaded.id)
    }

    async fn fetch(
        &self,
        file_id: &str,
        into: &Path,
        progress: &Progress,
    ) -> std::result::Result<u64, DriveError> {
        let access = self.access().await?;
        let known = files::get(&self.shared.http, &access, file_id).await?;
        if known.trashed {
            return Err(DriveError::Gone);
        }

        let digest = files::download(&self.shared.http, &access, file_id, into, progress).await?;
        if let Some(expected) = known.md5_checksum.as_deref() {
            if !digest.eq_ignore_ascii_case(expected) {
                tokio::fs::remove_file(into).await.ok();
                return Err(DriveError::Unreadable(
                    "the archive that came down from Google is not the one it says it is"
                        .to_owned(),
                ));
            }
        }
        Ok(known.bytes().unwrap_or(0))
    }

    async fn describe(&self, file_id: &str) -> std::result::Result<files::File, DriveError> {
        let access = self.access().await?;
        files::get(&self.shared.http, &access, file_id).await
    }

    async fn remove(&self, file_id: &str) -> std::result::Result<(), DriveError> {
        let access = self.access().await?;
        files::delete(&self.shared.http, &access, file_id).await
    }

    async fn folder(&self, access: &Access) -> std::result::Result<String, DriveError> {
        let row = store::account(&self.shared.pool, self.user).await.ok().flatten();
        if let Some(folder) = row.and_then(|row| row.folder_id) {
            return Ok(folder);
        }
        let name = store::settings(&self.shared.pool)
            .await
            .map(|settings| settings.folder_name)
            .unwrap_or_else(|_| "craftpanel-backups".to_owned());
        let folder = files::ensure_folder(&self.shared.http, access, &name).await?;
        let _ = store::set_folder(&self.shared.pool, self.user, &folder, Timestamp::now()).await;
        Ok(folder)
    }

    async fn access(&self) -> std::result::Result<Access, DriveError> {
        if let Some(access) = self.access.read().await.clone() {
            if access.usable(Timestamp::now()) {
                return Ok(access);
            }
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

        match oauth::refresh(&self.shared.http, &credentials, &token).await {
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

const DEVICE_PAGE: &str = "https://www.google.com/device";

fn offset_of(user: Id, window: Duration) -> Duration {
    let spread = user
        .to_string()
        .bytes()
        .fold(0u64, |sum, byte| sum.wrapping_mul(31).wrapping_add(byte as u64));
    Duration::from_secs(spread % window.as_secs())
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
