#![allow(dead_code)]

mod channels;
mod compat;
#[cfg(test)]
pub mod harness;
mod install;
mod modrinth;
mod mrpack;
mod multipart;
mod paths;
mod scan;
mod store;
pub mod types;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use axum::http::StatusCode;
use sqlx::SqlitePool;
use tokio::task::JoinHandle;

use crate::auth::access::Access;
use crate::auth::disk::{self, Disks};
use crate::auth::error::{Failure, Result};
use crate::helper::Helper;
use crate::model::{
    AlwaysFalse, ApiContentItem, ContentModpack, ContentProject, ContentSourceKind, ContentVersion, Id, LoaderId, ModpackSourceKind, ModrinthOwner, Operation,
    OperationError, OperationErrorStep, OperationKind, Permission, Timestamp, UpdateChannel,
};
use crate::ops::{ContentChangeReason, NewOperation, Operations, Step, WsMessage};

pub use self::modrinth::{Modrinth, Upstream};
pub use self::multipart::{
    boundary_of, collect as collect_parts, Content as PartBody, Part,
};

use self::compat::Target;
use self::install::Wanted;
use self::store::{ItemRow, ModpackRow};
use self::types::*;

const CHECK_AFTER: Duration = Duration::from_secs(6 * 60 * 60);
const SWEEP_EVERY: Duration = Duration::from_secs(15 * 60);
const RECENTLY: Duration = Duration::from_secs(7 * 24 * 60 * 60);
const TURNS: u32 = 600;
const TURN: Duration = Duration::from_millis(500);

pub struct Content {
    pool: SqlitePool,
    data_dir: PathBuf,
    helper: Helper,
    operations: Arc<Operations>,
    modrinth: Arc<Modrinth>,
    checking: Arc<std::sync::Mutex<BTreeSet<Id>>>,
    disks: Disks,
}

impl Content {
    pub fn new(
        pool: SqlitePool,
        data_dir: impl Into<PathBuf>,
        helper: Helper,
        operations: Arc<Operations>,
        disks: Disks,
    ) -> std::result::Result<Arc<Self>, Upstream> {
        let modrinth = Arc::new(Modrinth::new(pool.clone())?);
        Ok(Self::with_modrinth(pool, data_dir, helper, operations, modrinth, disks))
    }

    pub fn with_modrinth(
        pool: SqlitePool,
        data_dir: impl Into<PathBuf>,
        helper: Helper,
        operations: Arc<Operations>,
        modrinth: Arc<Modrinth>,
        disks: Disks,
    ) -> Arc<Self> {
        Arc::new(Self {
            pool,
            data_dir: data_dir.into(),
            helper,
            operations,
            modrinth,
            checking: Arc::default(),
            disks,
        })
    }

    pub fn modrinth(&self) -> &Modrinth {
        &self.modrinth
    }

    pub fn operations(&self) -> &Arc<Operations> {
        &self.operations
    }

    pub fn disks(&self) -> &Disks {
        &self.disks
    }

    pub async fn list(
        self: &Arc<Self>,
        access: Access,
        refresh: bool,
    ) -> Result<ContentListResponse> {
        let facts = self.facts(access.server_id).await?;
        let root = self.server_dir(&facts);
        let rows = scan::reconcile(&self.pool, facts.id, &root)
            .await
            .map_err(Failure::from)?;

        let mine: Vec<ItemRow> = rows
            .iter()
            .filter(|row| row.source_kind != ContentSourceKind::ModrinthModpack)
            .cloned()
            .collect();
        let truncated = scan::capped(rows.len());

        let mut items = Vec::with_capacity(mine.len());
        for row in &mine {
            items.push(self.item(row).await?);
        }

        let stale = facts
            .updates_checked_at
            .is_none_or(|when| when.as_datetime() + CHECK_AFTER < Timestamp::now().as_datetime());
        if refresh || stale {
            self.check_updates_later(facts.id);
        }

        Ok(ContentListResponse {
            content_type: facts.loader.content_type(),
            loader: facts.loader,
            loader_version: facts.loader_version.clone(),
            game_version: facts.game_version.clone(),
            update_channel: facts.update_channel,
            updates_checked_at: facts.updates_checked_at,
            permissions: ContentPermissions {
                can_read: true,
                can_write: access.allows(Permission::Setup),
            },
            modpack: self.modpack_card(facts.id).await?,
            items,
            truncated,
        })
    }

    pub async fn modpack_contents(&self, server: Id) -> Result<ModpackContentsResponse> {
        if store::modpack(&self.pool, server).await?.is_none() {
            return Err(Failure::conflict("modpack_not_linked", "no modpack on this server"));
        }
        let facts = self.facts(server).await?;
        scan::reconcile(&self.pool, server, &self.server_dir(&facts)).await?;
        let rows = store::of_kind(&self.pool, server, true).await?;
        let mut items = Vec::with_capacity(rows.len());
        for row in &rows {
            items.push(self.item(row).await?);
        }
        Ok(ModpackContentsResponse { items })
    }

    pub async fn dependents(&self, server: Id, ids: &[Id]) -> Result<ContentDependentsResponse> {
        let found = store::dependents_of(&self.pool, server, ids).await?;
        Ok(ContentDependentsResponse {
            dependents: found
                .into_iter()
                .map(|(id, depends_on)| ContentDependentEntry { id, depends_on })
                .collect(),
        })
    }

    pub async fn set_enabled(
        &self,
        server: Id,
        ids: &[Id],
        enabled: bool,
    ) -> Result<ContentMutationResponse> {
        let facts = self.facts(server).await?;
        let root = self.server_dir(&facts);
        let mut results = Vec::with_capacity(ids.len());
        let mut moved = false;

        for id in ids {
            let Some(row) = store::one(&self.pool, server, *id).await? else {
                results.push(ContentMutationResult::failed(*id, "content_not_found", "no such item"));
                continue;
            };
            if row.locked {
                results.push(ContentMutationResult::failed(
                    *id,
                    "forbidden",
                    "the loader jar cannot be switched off",
                ));
                continue;
            }
            if row.enabled == enabled {
                results.push(succeeded(&row));
                continue;
            }

            let wanted = store::toggled(&row.file_path, enabled);
            match rename_within(&root, &row.file_path, &wanted) {
                Ok(()) => {
                    store::move_to(&self.pool, row.id, &wanted, enabled).await?;
                    moved = true;
                    let mut after = row.clone();
                    after.file_path = wanted.clone();
                    after.file_name = store::file_name_of(&wanted);
                    after.enabled = enabled;
                    results.push(succeeded(&after));
                }
                Err(fault) => results.push(ContentMutationResult::failed(
                    *id,
                    fault.code(),
                    fault.message().to_owned(),
                )),
            }
        }

        if moved {
            self.give_back(&facts).await?;
            self.changed(server, ContentChangeReason::ExternalChange);
        }
        Ok(ContentMutationResponse { results })
    }

    pub async fn delete(&self, server: Id, ids: &[Id]) -> Result<ContentMutationResponse> {
        let facts = self.facts(server).await?;
        let root = self.server_dir(&facts);
        let mut results = Vec::with_capacity(ids.len());
        let mut removed = false;

        for id in ids {
            let Some(row) = store::one(&self.pool, server, *id).await? else {
                results.push(ContentMutationResult::failed(*id, "content_not_found", "no such item"));
                continue;
            };
            if row.locked {
                results.push(ContentMutationResult::failed(
                    *id,
                    "forbidden",
                    "the loader jar cannot be deleted here",
                ));
                continue;
            }

            let removal = match paths::resolve_leaf(&root, &row.file_path).map_err(Fault::from) {
                Ok(full) => {
                    self.let_the_panel_in(&facts, &full, &row.file_path).await;
                    paths::remove(&full).map_err(|err| Fault::of(&err))
                }
                Err(fault) => Err(fault),
            };
            match removal {
                Ok(()) => {
                    store::remove(&self.pool, row.id).await?;
                    removed = true;
                    results.push(ContentMutationResult {
                        id: row.id,
                        ok: true,
                        file_name: None,
                        file_path: None,
                        enabled: None,
                        error: None,
                        message: None,
                    });
                }
                Err(fault) => results.push(ContentMutationResult::failed(
                    *id,
                    fault.code(),
                    fault.message().to_owned(),
                )),
            }
        }

        if removed {
            self.changed(server, ContentChangeReason::ExternalChange);
        }
        Ok(ContentMutationResponse { results })
    }

    pub async fn adopt_uploads(
        &self,
        server: Id,
        uploads: Vec<(String, PathBuf, u64)>,
    ) -> Result<ContentUploadResponse> {
        let facts = self.facts(server).await?;
        let root = self.server_dir(&facts);
        let directory = install::directory_of(facts.loader);
        let mut results = Vec::with_capacity(uploads.len());
        let mut placed = false;

        for (file_name, staged, size) in uploads {
            if !allowed_upload(&file_name) {
                let _ = std::fs::remove_file(&staged);
                results.push(ContentUploadResult {
                    file_name,
                    ok: false,
                    id: None,
                    error: Some("unsupported_file_type".to_owned()),
                    message: Some("only .jar and .zip go here; a .mrpack is 8.10".to_owned()),
                });
                continue;
            }

            let relative = format!("{directory}/{file_name}");
            match install::place(&staged, &root, &relative) {
                Ok(()) => {
                    let mut row = self.row_for(&facts, &root, &relative).await?;
                    row.size_bytes = size as i64;
                    store::upsert(&self.pool, &row).await?;
                    placed = true;
                    results.push(ContentUploadResult {
                        file_name,
                        ok: true,
                        id: Some(row.id),
                        error: None,
                        message: None,
                    });
                }
                Err(err) => {
                    let _ = std::fs::remove_file(&staged);
                    results.push(ContentUploadResult {
                        file_name,
                        ok: false,
                        id: None,
                        error: Some("invalid_path".to_owned()),
                        message: Some(err.to_string()),
                    });
                }
            }
        }

        if placed {
            self.give_back(&facts).await?;
            self.changed(server, ContentChangeReason::ExternalChange);
        }
        Ok(ContentUploadResponse { results })
    }

    async fn row_for(&self, facts: &ServerFacts, root: &Path, relative: &str) -> Result<ItemRow> {
        let Some(mut row) = store::by_path(&self.pool, facts.id, relative).await? else {
            return Ok(ItemRow::fresh(facts.id, relative, facts.loader.content_type()));
        };
        if row.file_path != relative {
            if let Ok(full) = paths::resolve_leaf(root, &row.file_path) {
                self.let_the_panel_in(facts, &full, &row.file_path).await;
                let _ = paths::remove(&full);
            }
            row.file_path = relative.to_owned();
            row.file_name = store::file_name_of(relative);
            row.enabled = true;
        }
        Ok(row)
    }

    pub async fn unlink_modpack(&self, server: Id) -> Result<ModpackUnlinkResponse> {
        if store::modpack(&self.pool, server).await?.is_none() {
            return Err(Failure::conflict("modpack_not_linked", "no modpack on this server"));
        }
        let adopted = store::unlink(&self.pool, server).await?;
        self.changed(server, ContentChangeReason::ExternalChange);
        Ok(ModpackUnlinkResponse { unlinked: true, adopted_items: adopted })
    }

    pub async fn install(
        self: &Arc<Self>,
        server: Id,
        request: &ContentInstallRequest,
        started_by: Option<Id>,
    ) -> Result<ContentInstallResponse> {
        if request.items.is_empty() {
            return Err(Failure::invalid_request("no projects were named"));
        }
        self.operations.guard_write(server).await.map_err(fault_to_failure)?;

        let facts = self.facts(server).await?;
        disk::guard(&self.pool, &self.disks, facts.owner_id, 0).await?;
        let installed = store::list(&self.pool, server).await?;
        let asked: BTreeSet<String> =
            request.items.iter().map(|item| item.project_id.clone()).collect();
        let _ = self.modrinth.remember_projects(&asked).await;
        let plan = install::resolve(
            &self.modrinth,
            &facts.target(),
            facts.update_channel,
            &installed,
            &request.items,
            request.resolve_dependencies,
        )
        .await
        .map_err(upstream)?;

        if plan.wanted.is_empty() {
            let unresolvable = plan
                .skipped
                .iter()
                .any(|entry| entry.reason == ContentSkipReason::NoCompatibleVersion);
            return Err(if unresolvable {
                Failure::new(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "no_compatible_version",
                    "nothing here fits this loader and game version",
                )
            } else {
                Failure::invalid_request("everything asked for is already installed")
            });
        }

        disk::guard(&self.pool, &self.disks, facts.owner_id, wanted_bytes(&plan.wanted)).await?;

        let operation = self
            .start(server, OperationKind::InstallContent, started_by, plan.entries().len())
            .await?;
        let planned = plan.entries();
        self.clone().drive(
            facts,
            operation.id,
            Job::Install { wanted: plan.wanted, source: ContentSourceKind::ServerProject },
        );

        Ok(ContentInstallResponse { operation, planned, skipped: plan.skipped })
    }

    pub async fn update(
        self: &Arc<Self>,
        server: Id,
        request: &ContentUpdateRequest,
        started_by: Option<Id>,
    ) -> Result<ContentUpdateResponse> {
        self.operations.guard_write(server).await.map_err(fault_to_failure)?;
        let facts = self.facts(server).await?;
        disk::guard(&self.pool, &self.disks, facts.owner_id, 0).await?;
        let target = facts.target();

        let rows = if request.all {
            store::list(&self.pool, server).await?
        } else {
            let mut chosen = Vec::new();
            for item in &request.items {
                let row = store::one(&self.pool, server, item.id)
                    .await?
                    .ok_or_else(|| Failure::not_found("content_not_found", "no such item"))?;
                chosen.push(row);
            }
            chosen
        };

        let mut wanted = Vec::new();
        for row in &rows {
            let asked = request.items.iter().find(|item| item.id == row.id);
            let picked = match asked.and_then(|item| item.version_id.clone()) {
                Some(version_id) => {
                    Some(self.modrinth.version(&version_id).await.map_err(upstream)?)
                }
                None => install::update_for(&self.modrinth, row, &target, facts.update_channel)
                    .await
                    .map_err(upstream)?,
            };
            let Some(version) = picked else { continue };
            let Some(file) = version.primary_file().cloned() else { continue };
            wanted.push(Wanted {
                project_id: row.project_id.clone().unwrap_or_else(|| version.project_id.clone()),
                version,
                file,
                reason: PlanReason::Requested,
                replaces: Some(row.id),
            });
        }

        if wanted.is_empty() {
            return Err(Failure::new(
                StatusCode::UNPROCESSABLE_ENTITY,
                "no_compatible_version",
                "nothing here has a newer version for this server",
            ));
        }

        let total = wanted.len() as u32;
        let operation =
            self.start(server, OperationKind::UpdateContent, started_by, wanted.len()).await?;
        self.clone().drive(facts, operation.id, Job::Update { wanted });
        Ok(ContentUpdateResponse { operation, total })
    }

    pub async fn install_modpack(
        self: &Arc<Self>,
        server: Id,
        source: PackSource,
        keep_extra_content: bool,
        started_by: Option<Id>,
        running: bool,
    ) -> Result<Operation> {
        if running {
            return Err(Failure::conflict("server_running", "stop the server first"));
        }
        self.operations.guard_write(server).await.map_err(fault_to_failure)?;
        let facts = self.facts(server).await?;
        disk::guard(&self.pool, &self.disks, facts.owner_id, 0).await?;

        let operation = self.start(server, OperationKind::InstallModpack, started_by, 1).await?;
        self.clone().drive(facts, operation.id, Job::Modpack { source, keep_extra_content });
        Ok(operation)
    }

    pub async fn change_game_version(
        self: &Arc<Self>,
        server: Id,
        request: GameVersionChangeRequest,
        started_by: Option<Id>,
        running: bool,
    ) -> Result<Operation> {
        self.a_real_game_version(&request.game_version).await?;
        if running {
            return Err(Failure::conflict("server_running", "stop the server first"));
        }
        self.operations.guard_write(server).await.map_err(fault_to_failure)?;
        let facts = self.facts(server).await?;
        disk::guard(&self.pool, &self.disks, facts.owner_id, 0).await?;

        let operation =
            self.start(server, OperationKind::ChangeGameVersion, started_by, 1).await?;
        self.clone().drive(facts, operation.id, Job::GameVersion { request });
        Ok(operation)
    }

    pub async fn preview(
        &self,
        server: Id,
        query: &PreviewQuery,
    ) -> Result<GameVersionPreviewResponse> {
        self.a_real_game_version(&query.game_version).await?;
        let facts = self.facts(server).await?;
        let loader = query.loader.unwrap_or(facts.loader);
        let target = Target::new(query.game_version.clone(), loader);

        let mut changes = Vec::new();
        if query.game_version != facts.game_version {
            changes.push(note(
                GameVersionChangeDiffType::GameVersionUpdated,
                &facts.game_version,
                &query.game_version,
            ));
        }
        if loader != facts.loader {
            changes.push(note(
                GameVersionChangeDiffType::LoaderUpdated,
                facts.loader.as_str(),
                loader.as_str(),
            ));
        }
        if store::modpack(&self.pool, server).await?.is_some() {
            changes.push(bare(GameVersionChangeDiffType::ModpackUnlinked));
        }

        let mut has_unknown_content = false;
        for row in store::list(&self.pool, server).await? {
            let Some(project_id) = row.project_id.clone() else {
                has_unknown_content = true;
                continue;
            };
            let newer = install::update_for(&self.modrinth, &row, &target, facts.update_channel)
                .await
                .map_err(upstream)?;
            let fits = match &newer {
                Some(_) => true,
                None => self.still_fits(&row, &target).await?,
            };

            let project = self.modrinth.cached_project(&project_id).await.map_err(upstream)?;
            changes.push(GameVersionChangeEntry {
                kind: if fits {
                    GameVersionChangeDiffType::Updated
                } else {
                    GameVersionChangeDiffType::Removed
                },
                id: Some(row.id),
                file_name: Some(row.file_name.clone()),
                project_id: Some(project_id),
                project_title: project.as_ref().map(|project| project.title.clone()),
                project_icon_url: project.and_then(|project| project.icon_url),
                current_version: self.installed_version(&row).await?,
                new_version: newer.map(|version| GameVersionChangeVersion {
                    id: version.id,
                    version_number: version.version_number,
                }),
            });
        }

        Ok(GameVersionPreviewResponse {
            new_game_version: query.game_version.clone(),
            new_loader: loader,
            new_loader_version: query.loader_version.clone(),
            has_unknown_content,
            changes,
        })
    }

    async fn a_real_game_version(&self, game_version: &str) -> Result<()> {
        if game_version.trim().is_empty() {
            return Err(Failure::invalid_request("a game version is needed"));
        }
        let Some(known) = self.modrinth.game_versions().await else { return Ok(()) };
        if known.iter().any(|version| version == game_version) {
            return Ok(());
        }
        Err(Failure::invalid_request(format!("{game_version} is not a Minecraft version")))
    }

    async fn installed_version(&self, row: &ItemRow) -> Result<Option<GameVersionChangeVersion>> {
        let Some(id) = row.version_id.clone() else { return Ok(None) };
        let known = self.modrinth.cached_version(&id).await.map_err(upstream)?;
        Ok(Some(GameVersionChangeVersion {
            version_number: known
                .map(|version| version.version_number)
                .filter(|number| !number.is_empty())
                .unwrap_or_else(|| id.clone()),
            id,
        }))
    }

    async fn still_fits(&self, row: &ItemRow, target: &Target) -> Result<bool> {
        let (Some(project_id), Some(version_id)) = (&row.project_id, &row.version_id) else {
            return Ok(false);
        };
        let versions = match self.modrinth.versions(project_id).await {
            Ok(versions) => versions,
            Err(Upstream::NotFound(_)) => return Ok(false),
            Err(err) => return Err(upstream(err)),
        };
        Ok(versions
            .iter()
            .any(|version| &version.id == version_id && compat::matches(version, target)))
    }

    pub async fn check_updates(&self, server: Id) -> Result<()> {
        self.run_check(server, Pace::Ahead).await
    }

    pub async fn check_updates_gently(&self, server: Id) -> Result<()> {
        self.run_check(server, Pace::Behind).await
    }

    async fn run_check(&self, server: Id, pace: Pace) -> Result<()> {
        let facts = self.facts(server).await?;
        let target = facts.target();
        let rows = store::list(&self.pool, server).await?;
        let pack = store::modpack(&self.pool, server).await?;

        let mut projects: BTreeSet<String> =
            rows.iter().filter_map(|row| row.project_id.clone()).collect();
        projects.extend(pack.as_ref().and_then(|pack| pack.project_id.clone()));
        if pace == Pace::Behind {
            self.modrinth.pace_background().await;
        }
        let _ = self.modrinth.remember_projects(&projects).await;

        for row in &rows {
            if pace == Pace::Behind {
                self.modrinth.pace_background().await;
            }
            let newer = install::update_for(&self.modrinth, row, &target, facts.update_channel)
                .await
                .map_err(upstream)?;
            store::set_update(&self.pool, row.id, newer.as_ref().map(|version| version.id.as_str()))
                .await?;
        }

        if let Some(pack) = &pack {
            if let Some(project_id) = &pack.project_id {
                let newest = self.newest_pack_version(project_id, &facts).await?;
                let update = newest.filter(|version| Some(&version.id) != pack.version_id.as_ref());
                store::set_modpack_update(
                    &self.pool,
                    server,
                    update.as_ref().map(|version| version.id.as_str()),
                )
                .await?;
            }
        }

        store::checked_at(&self.pool, server, Timestamp::now()).await?;
        self.changed(server, ContentChangeReason::UpdatesChecked);
        Ok(())
    }

    fn check_updates_later(self: &Arc<Self>, server: Id) {
        let Some(turn) = OneAtATime::take(&self.checking, server) else {
            return;
        };
        let content = Arc::clone(self);
        tokio::spawn(async move {
            let _turn = turn;
            if !content.modrinth.allowed().await {
                return;
            }
            if let Err(failure) = content.check_updates(server).await {
                tracing::debug!("the update check on {server} stopped: {failure}");
            }
        });
    }

    async fn check_these(&self, facts: &ServerFacts, projects: &BTreeSet<String>) -> Result<()> {
        if projects.is_empty() {
            return Ok(());
        }
        let target = facts.target();
        for row in store::list(&self.pool, facts.id).await? {
            if !row.project_id.as_deref().is_some_and(|id| projects.contains(id)) {
                continue;
            }
            let newer = install::update_for(&self.modrinth, &row, &target, facts.update_channel)
                .await
                .map_err(upstream)?;
            store::set_update(&self.pool, row.id, newer.as_ref().map(|version| version.id.as_str()))
                .await?;
        }
        self.changed(facts.id, ContentChangeReason::UpdatesChecked);
        Ok(())
    }

    pub fn sweep_updates(self: &Arc<Self>, live: crate::auth::LiveServers) -> JoinHandle<()> {
        let content = Arc::clone(self);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(SWEEP_EVERY).await;
                match content.sweep_once(&live).await {
                    Ok(swept) if !swept.is_empty() => {
                        tracing::debug!("checked {} servers for content updates", swept.len())
                    }
                    Ok(_) => {}
                    Err(failure) => tracing::debug!("the background check stopped: {failure}"),
                }
            }
        })
    }

    pub async fn sweep_once(&self, live: &crate::auth::LiveServers) -> Result<Vec<Id>> {
        if !self.modrinth.allowed().await {
            return Ok(Vec::new());
        }
        let running = live.ids().await;
        let now = Timestamp::now().as_datetime();
        let servers: Vec<(Id, Option<Timestamp>, Timestamp)> = sqlx::query_as(
            "SELECT id, updates_checked_at, updated_at FROM servers WHERE status = 'available'",
        )
        .fetch_all(&self.pool)
        .await?;

        let mut swept = Vec::new();
        for (id, checked, touched) in servers {
            if checked.is_some_and(|when| when.as_datetime() + CHECK_AFTER > now) {
                continue;
            }
            if !running.contains(&id) && touched.as_datetime() + RECENTLY < now {
                continue;
            }
            if let Err(failure) = self.check_updates_gently(id).await {
                tracing::debug!("the check on {id} stopped: {failure}");
            }
            swept.push(id);
        }
        Ok(swept)
    }

    async fn newest_pack_version(
        &self,
        project_id: &str,
        facts: &ServerFacts,
    ) -> Result<Option<self::modrinth::MrVersion>> {
        let versions = match self.modrinth.versions(project_id).await {
            Ok(versions) => versions,
            Err(Upstream::NotFound(_)) => return Ok(None),
            Err(err) => return Err(upstream(err)),
        };
        let mut fitting: Vec<_> = versions
            .into_iter()
            .filter(|version| compat::matches_modpack(version, &facts.game_version))
            .filter(|version| {
                channels::allows(&version.version_type, facts.update_channel, None)
            })
            .collect();
        fitting.sort_by(|left, right| right.published().cmp(&left.published()));
        Ok(fitting.into_iter().next())
    }

    pub fn server_dir(&self, facts: &ServerFacts) -> PathBuf {
        self.data_dir
            .join("users")
            .join(facts.owner_id.to_string())
            .join("servers")
            .join(facts.id.to_string())
    }

    pub async fn facts(&self, server: Id) -> Result<ServerFacts> {
        let row: Option<(Id, Id, Option<LoaderId>, Option<String>, Option<String>, UpdateChannel, Option<Timestamp>)> =
            sqlx::query_as(
                "SELECT id, owner_id, loader, loader_version, game_version, update_channel,
                        updates_checked_at
                   FROM servers WHERE id = ?",
            )
            .bind(server)
            .fetch_optional(&self.pool)
            .await?;

        let (id, owner_id, loader, loader_version, game_version, update_channel, checked) =
            row.ok_or_else(|| Failure::not_found("server_not_found", "no such server"))?;

        Ok(ServerFacts {
            id,
            owner_id,
            loader: loader.unwrap_or(LoaderId::Vanilla),
            loader_version,
            game_version: game_version.unwrap_or_default(),
            update_channel,
            updates_checked_at: checked,
        })
    }

    pub async fn max_upload_bytes(&self) -> Result<u64> {
        let bytes: i64 =
            sqlx::query_scalar("SELECT max_upload_bytes FROM panel_settings WHERE id = 1")
                .fetch_one(&self.pool)
                .await?;
        Ok(bytes.max(0) as u64)
    }

    pub async fn linked(&self, server: Id) -> Result<Option<Option<String>>> {
        Ok(store::modpack(&self.pool, server).await?.map(|pack| pack.project_id))
    }

    async fn let_the_panel_in(&self, facts: &ServerFacts, full: &Path, relative: &str) {
        if !std::fs::symlink_metadata(full).is_ok_and(|meta| meta.is_dir()) {
            return;
        }
        let Ok(segments) = paths::normalise(relative) else { return };
        let steps = crate::helper::below_server(facts.id, &segments);
        if let Err(err) = self.helper.chown_tree(&facts.owner_id.to_string(), steps).await {
            tracing::warn!("{} was not handed back before deleting: {err:#}", full.display());
        }
    }

    async fn give_back(&self, facts: &ServerFacts) -> Result<()> {
        self.helper
            .chown_tree(&facts.owner_id.to_string(), crate::helper::in_servers(facts.id))
            .await
            .map_err(|err| Failure::internal(err.context("handing the files back to the account")))?;
        Ok(())
    }

    fn changed(&self, server: Id, reason: ContentChangeReason) {
        self.operations.bus().say(server, &WsMessage::ContentChanged { reason });
    }

    async fn start(
        &self,
        server: Id,
        kind: OperationKind,
        started_by: Option<Id>,
        total: usize,
    ) -> Result<Operation> {
        let mut new = NewOperation::new(server, kind, started_by);
        new.input = Some(serde_json::json!({ "total": total }));
        self.operations.create(new).await.map_err(fault_to_failure)
    }

    async fn item(&self, row: &ItemRow) -> Result<ApiContentItem> {
        let project = match &row.project_id {
            Some(project_id) => self.modrinth.cached_project(project_id).await.map_err(upstream)?,
            None => None,
        };
        let owner = match project.as_ref().and_then(|project| project.team.clone()) {
            Some(team) => self.cached_owner(&team).await?,
            None => None,
        };
        let known = match &row.version_id {
            Some(version_id) => self.modrinth.cached_version(version_id).await.map_err(upstream)?,
            None => None,
        };

        Ok(ApiContentItem {
            id: row.id,
            file_name: row.file_name.clone(),
            file_path: paths::leading_slash(&row.file_path),
            size: row.size_bytes.max(0) as u64,
            enabled: row.enabled,
            locked: row.locked,
            project_type: row.project_type,
            date_added: row.date_added,
            source_kind: row.source_kind,
            environment: row
                .environment
                .clone()
                .or_else(|| project.as_ref().and_then(|project| project.environment.clone())),
            pack_client_retained: AlwaysFalse,
            pack_client_depends: row.pack_client_depends,
            installing: false,
            external: row.external,
            external_url: row.external_url.clone(),
            has_update: row.has_update,
            update_version_id: row.update_version_id.clone(),
            project_id: row.project_id.clone(),
            project: project.as_ref().map(|project| ContentProject {
                id: project.id.clone(),
                slug: project.slug.clone(),
                title: project.title.clone(),
                icon_url: project.icon_url.clone(),
            }),
            version: row.version_id.clone().map(|id| ContentVersion {
                version_number: known
                    .as_ref()
                    .map(|version| version.version_number.clone())
                    .unwrap_or_else(|| id.clone()),
                id,
                file_name: store::base_path(&row.file_path)
                    .rsplit('/')
                    .next()
                    .unwrap_or_default()
                    .to_owned(),
                date_published: known.as_ref().and_then(|version| version.published()),
            }),
            owner,
        })
    }

    async fn cached_owner(&self, team: &str) -> Result<Option<ModrinthOwner>> {
        let row: Option<(String, String, String, Option<String>)> = sqlx::query_as(
            "SELECT owner_id, name, kind, avatar_url FROM modrinth_project_owner WHERE team_id = ?",
        )
        .bind(team)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(|(id, name, kind, avatar_url)| ModrinthOwner {
            id,
            name,
            kind: kind.parse().unwrap_or(crate::model::ModrinthOwnerKind::User),
            avatar_url,
        }))
    }

    async fn modpack_card(&self, server: Id) -> Result<Option<ContentModpack>> {
        let Some(pack) = store::modpack(&self.pool, server).await? else { return Ok(None) };
        let project = match &pack.project_id {
            Some(project_id) => self.modrinth.cached_project(project_id).await.map_err(upstream)?,
            None => None,
        };
        let owner = match project.as_ref().and_then(|project| project.team.clone()) {
            Some(team) => self.cached_owner(&team).await?,
            None => None,
        };

        Ok(Some(ContentModpack {
            source_kind: pack.source_kind,
            project_id: pack.project_id.clone(),
            slug: project.as_ref().and_then(|project| project.slug.clone()),
            title: pack.title.clone(),
            description: project.as_ref().and_then(|project| project.description.clone()),
            icon_url: project.as_ref().and_then(|project| project.icon_url.clone()),
            filename: pack.filename.clone(),
            downloads: project.as_ref().and_then(|project| project.downloads),
            followers: project.as_ref().and_then(|project| project.followers),
            owner,
            categories: project.map(|project| project.categories).unwrap_or_default(),
            version_id: pack.version_id.clone(),
            version_number: pack.version_number.clone(),
            date_published: pack.date_published,
            has_update: pack.has_update,
            update_version_id: pack.update_version_id.clone(),
        }))
    }
}

fn succeeded(row: &ItemRow) -> ContentMutationResult {
    ContentMutationResult {
        id: row.id,
        ok: true,
        file_name: Some(row.file_name.clone()),
        file_path: Some(paths::leading_slash(&row.file_path)),
        enabled: Some(row.enabled),
        error: None,
        message: None,
    }
}

fn wanted_bytes(wanted: &[Wanted]) -> u64 {
    wanted.iter().fold(0, |total, one| total.saturating_add(one.file.size))
}

fn allowed_upload(file_name: &str) -> bool {
    let lower = file_name.to_ascii_lowercase();
    (lower.ends_with(".jar") || lower.ends_with(".zip")) && !lower.ends_with(".mrpack")
}

fn note(kind: GameVersionChangeDiffType, from: &str, to: &str) -> GameVersionChangeEntry {
    GameVersionChangeEntry {
        kind,
        id: None,
        file_name: None,
        project_id: None,
        project_title: None,
        project_icon_url: None,
        current_version: Some(GameVersionChangeVersion {
            id: from.to_owned(),
            version_number: from.to_owned(),
        }),
        new_version: Some(GameVersionChangeVersion {
            id: to.to_owned(),
            version_number: to.to_owned(),
        }),
    }
}

fn bare(kind: GameVersionChangeDiffType) -> GameVersionChangeEntry {
    GameVersionChangeEntry {
        kind,
        id: None,
        file_name: None,
        project_id: None,
        project_title: None,
        project_icon_url: None,
        current_version: None,
        new_version: None,
    }
}

#[derive(Debug, Clone)]
pub struct ServerFacts {
    pub id: Id,
    pub owner_id: Id,
    pub loader: LoaderId,
    pub loader_version: Option<String>,
    pub game_version: String,
    pub update_channel: UpdateChannel,
    pub updates_checked_at: Option<Timestamp>,
}

impl ServerFacts {
    fn target(&self) -> Target {
        Target::new(self.game_version.clone(), self.loader)
    }
}

struct OneAtATime {
    list: Arc<std::sync::Mutex<BTreeSet<Id>>>,
    server: Id,
}

impl OneAtATime {
    fn take(list: &Arc<std::sync::Mutex<BTreeSet<Id>>>, server: Id) -> Option<Self> {
        let mut held = list.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        held.insert(server).then(|| Self { list: Arc::clone(list), server })
    }
}

impl Drop for OneAtATime {
    fn drop(&mut self) {
        self.list.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).remove(&self.server);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pace {
    Ahead,
    Behind,
}

#[derive(Debug, Clone)]
pub enum PackSource {
    Modrinth { project_id: String, version_id: Option<String> },
    Upload { archive: PathBuf, file_name: String },
}

#[derive(Debug)]
enum Fault {
    Path(paths::PathFault),
    Refused(String),
    Io(String),
}

impl Fault {
    fn of(err: &std::io::Error) -> Self {
        match err.raw_os_error() {
            Some(libc::EACCES) | Some(libc::EPERM) => Self::Refused(err.to_string()),
            _ => Self::Io(err.to_string()),
        }
    }

    fn code(&self) -> &'static str {
        match self {
            Self::Path(fault) => fault.code(),
            Self::Refused(_) => "file_not_accessible",
            Self::Io(_) => "internal",
        }
    }

    fn message(&self) -> &str {
        match self {
            Self::Path(fault) => fault.message(),
            Self::Refused(message) | Self::Io(message) => message,
        }
    }
}

impl From<paths::PathFault> for Fault {
    fn from(fault: paths::PathFault) -> Self {
        Self::Path(fault)
    }
}

fn rename_within(root: &Path, from: &str, to: &str) -> std::result::Result<(), Fault> {
    let source = paths::resolve_leaf(root, from)?;
    let destination = paths::resolve_leaf(root, to)?;
    std::fs::rename(source, destination).map_err(|err| Fault::of(&err))
}

pub fn upstream(err: Upstream) -> Failure {
    match err {
        Upstream::RateLimited => Failure::new(
            StatusCode::TOO_MANY_REQUESTS,
            "upstream_rate_limited",
            "Modrinth is rate limiting us; try again shortly",
        ),
        Upstream::NotFound(what) => {
            Failure::not_found("content_not_found", format!("{what} is not on Modrinth"))
        }
        Upstream::Database(err) => Failure::internal(err),
        Upstream::Io(err) => Failure::internal(anyhow::Error::from(err)),
        other => Failure::new(StatusCode::BAD_GATEWAY, "upstream_unavailable", other.to_string()),
    }
}

fn fault_to_failure(fault: crate::ops::Fault) -> Failure {
    Failure::new(fault.status(), fault.code(), fault.message().to_owned())
}

enum Job {
    Install { wanted: Vec<Wanted>, source: ContentSourceKind },
    Update { wanted: Vec<Wanted> },
    Modpack { source: PackSource, keep_extra_content: bool },
    GameVersion { request: GameVersionChangeRequest },
}

impl Job {
    fn touched(&self) -> Option<BTreeSet<String>> {
        match self {
            Self::Install { wanted, .. } | Self::Update { wanted } => {
                Some(wanted.iter().map(|item| item.project_id.clone()).collect())
            }
            Self::Modpack { .. } | Self::GameVersion { .. } => None,
        }
    }

    fn leftovers(&self) -> Option<PathBuf> {
        match self {
            Self::Modpack { source: PackSource::Upload { archive, .. }, .. } => {
                Some(archive.clone())
            }
            _ => None,
        }
    }
}

impl Content {
    fn drive(self: Arc<Self>, facts: ServerFacts, operation: Id, job: Job) {
        tokio::spawn(async move {
            let touched = job.touched();
            let leftovers = job.leftovers();
            if !self.take_turn(operation).await {
                clear_away(leftovers);
                return;
            }
            let outcome = match job {
                Job::Install { wanted, source } => {
                    self.lay_out(&facts, operation, wanted, source).await
                }
                Job::Update { wanted } => {
                    self.lay_out(&facts, operation, wanted, ContentSourceKind::ServerProject).await
                }
                Job::Modpack { source, keep_extra_content } => {
                    self.lay_out_pack(&facts, operation, source, keep_extra_content).await
                }
                Job::GameVersion { request } => {
                    self.move_game_version(&facts, operation, request).await
                }
            };

            match outcome {
                Ok(()) => {
                    let _ = self.operations.finish(operation).await;
                    let checked = match &touched {
                        Some(projects) => self.check_these(&facts, projects).await,
                        None => self.check_updates(facts.id).await,
                    };
                    if let Err(failure) = checked {
                        tracing::debug!("the check after {operation} stopped: {failure}");
                    }
                }
                Err(error) => {
                    let _ = self.hand_back(&facts).await;
                    tracing::warn!("content run {operation} failed: {}", error.message);
                    let _ = self.operations.fail(operation, error).await;
                }
            }
            clear_away(leftovers);
            self.changed(facts.id, ContentChangeReason::ExternalChange);
        });
    }

    async fn take_turn(&self, operation: Id) -> bool {
        for _ in 0..TURNS {
            match self.operations.begin(operation).await {
                Ok(Some(_)) => return true,
                Ok(None) => tokio::time::sleep(TURN).await,
                Err(fault) => {
                    tracing::error!("a content run could not start: {}", fault.message());
                    return false;
                }
            }
        }
        false
    }

    async fn lay_out(
        &self,
        facts: &ServerFacts,
        operation: Id,
        wanted: Vec<Wanted>,
        source: ContentSourceKind,
    ) -> std::result::Result<(), OperationError> {
        let root = self.server_dir(facts);
        let work = self.work_dir(operation).await?;
        std::fs::create_dir_all(&work).map_err(filesystem)?;

        let projects: BTreeSet<String> =
            wanted.iter().map(|item| item.project_id.clone()).collect();
        let _ = self.modrinth.remember_projects(&projects).await;

        let directory = install::directory_of(facts.loader);
        let count = wanted.len().max(1) as f64;

        for (done, item) in wanted.iter().enumerate() {
            let _ = self
                .operations
                .advance(
                    operation,
                    Step {
                        phase: Some(crate::model::OperationPhase::Addons),
                        progress: Some(done as f64 / count),
                        current_file: Some(item.file.filename.clone()),
                        files_processed: Some(done as u64),
                        ..Step::default()
                    },
                )
                .await;

            let staged = install::fetch(&self.modrinth, item, &work).await.map_err(download)?;
            let name = multipart::safe_file_name(&item.file.filename)
                .map_err(|_| filesystem(std::io::Error::other("that file name cannot be used")))?;
            let relative = format!("{directory}/{name}");

            if let Some(previous) = item.replaces {
                if let Ok(Some(old)) = store::one(&self.pool, facts.id, previous).await {
                    if old.file_path != relative {
                        if let Ok(full) = paths::resolve_leaf(&root, &old.file_path) {
                            self.let_the_panel_in(facts, &full, &old.file_path).await;
                            let _ = paths::remove(&full);
                        }
                    }
                }
            }

            install::place(&staged, &root, &relative).map_err(filesystem)?;
            self.write_row(facts, item, &relative, source, None).await?;
        }

        self.hand_back(facts).await?;
        let _ = std::fs::remove_dir_all(&work);
        Ok(())
    }

    async fn lay_out_pack(
        &self,
        facts: &ServerFacts,
        operation: Id,
        source: PackSource,
        keep_extra_content: bool,
    ) -> std::result::Result<(), OperationError> {
        let root = self.server_dir(facts);
        let work = self.work_dir(operation).await?;
        std::fs::create_dir_all(&work).map_err(filesystem)?;

        let _ = self
            .operations
            .advance(
                operation,
                Step {
                    phase: Some(crate::model::OperationPhase::InstallingPack),
                    progress: Some(0.05),
                    ..Step::default()
                },
            )
            .await;

        let (archive, card) = self.obtain_pack(&source, &work).await?;
        let mut pack = mrpack::Pack::open(&archive).map_err(pack_fault)?;
        let files: Vec<(String, mrpack::PackFile)> = pack
            .index
            .server_files()
            .map_err(pack_fault)?
            .into_iter()
            .map(|(path, file)| (path, file.clone()))
            .collect();
        let omitted = pack.index.omitted_projects();
        let title = pack.index.name.clone();
        let version_number = pack.index.version_id.clone();

        if !keep_extra_content {
            self.clear_extra(facts, &root).await?;
        }

        let overrides = work.join("overrides");
        std::fs::create_dir_all(&overrides).map_err(filesystem)?;
        let laid_out = pack.extract_overrides(&overrides).map_err(pack_fault)?;

        let count = files.len().max(1) as f64;
        let mut kept: Vec<String> = Vec::with_capacity(files.len());

        for (done, (path, file)) in files.iter().enumerate() {
            let _ = self
                .operations
                .advance(
                    operation,
                    Step {
                        progress: Some(0.1 + 0.8 * done as f64 / count),
                        current_file: Some(path.clone()),
                        files_processed: Some(done as u64),
                        ..Step::default()
                    },
                )
                .await;

            let Some(url) = file.downloads.first() else {
                return Err(OperationError {
                    code: "modpack_no_primary_file".to_owned(),
                    message: format!("{path} has no address to fetch it from"),
                    step: OperationErrorStep::Modpack,
                });
            };
            let staged = work.join(format!("pack-{}", Id::new()));
            self.modrinth.download(url, &staged, &file.hashes).await.map_err(download)?;
            install::place(&staged, &root, path).map_err(filesystem)?;
            kept.push(path.clone());
        }

        for relative in &laid_out {
            let staged = overrides.join(relative.replace('/', std::path::MAIN_SEPARATOR_STR));
            install::place(&staged, &root, relative).map_err(filesystem)?;
        }

        self.hand_back(facts).await?;
        self.record_pack(facts, &files, &omitted, &card, &title, &version_number).await?;
        self.drop_what_the_pack_no_longer_has(facts, &root, &kept).await?;
        let _ = std::fs::remove_dir_all(&work);
        Ok(())
    }

    async fn drop_what_the_pack_no_longer_has(
        &self,
        facts: &ServerFacts,
        root: &Path,
        kept: &[String],
    ) -> std::result::Result<(), OperationError> {
        for row in store::list(&self.pool, facts.id).await.map_err(internal)? {
            if row.source_kind != ContentSourceKind::ModrinthModpack {
                continue;
            }
            if kept.iter().any(|path| path == store::base_path(&row.file_path)) {
                continue;
            }
            if let Ok(full) = paths::resolve_leaf(root, &row.file_path) {
                self.let_the_panel_in(facts, &full, &row.file_path).await;
                let _ = paths::remove(&full);
            }
            store::remove(&self.pool, row.id).await.map_err(internal)?;
        }
        Ok(())
    }

    async fn move_game_version(
        &self,
        facts: &ServerFacts,
        operation: Id,
        request: GameVersionChangeRequest,
    ) -> std::result::Result<(), OperationError> {
        let loader = request.loader.unwrap_or(facts.loader);
        let target = Target::new(request.game_version.clone(), loader);
        let root = self.server_dir(facts);
        let rows = store::list(&self.pool, facts.id).await.map_err(internal)?;
        let count = rows.len().max(1) as f64;

        let mut wanted = Vec::new();
        let mut disable = Vec::new();
        for (done, row) in rows.iter().enumerate() {
            let _ = self
                .operations
                .advance(
                    operation,
                    Step {
                        phase: Some(crate::model::OperationPhase::Analyzing),
                        progress: Some(0.3 * done as f64 / count),
                        ..Step::default()
                    },
                )
                .await;

            if self.fits(row, &target).await {
                continue;
            }
            let replacement = match request.incompatible_content {
                IncompatiblePolicy::Keep => None,
                IncompatiblePolicy::Disable => None,
                IncompatiblePolicy::UpdateThenDisable => {
                    install::update_for(&self.modrinth, row, &target, facts.update_channel)
                        .await
                        .ok()
                        .flatten()
                }
            };

            match replacement {
                Some(version) => {
                    if let Some(file) = version.primary_file().cloned() {
                        wanted.push(Wanted {
                            project_id: row
                                .project_id
                                .clone()
                                .unwrap_or_else(|| version.project_id.clone()),
                            version,
                            file,
                            reason: PlanReason::Requested,
                            replaces: Some(row.id),
                        });
                    }
                }
                None if request.incompatible_content != IncompatiblePolicy::Keep => {
                    disable.push(row.clone())
                }
                None => {}
            }
        }

        if !wanted.is_empty() {
            self.lay_out(facts, operation, wanted, ContentSourceKind::ServerProject).await?;
        }

        let mut switched = false;
        for row in &disable {
            if !row.enabled {
                continue;
            }
            let off = store::toggled(&row.file_path, false);
            if rename_within(&root, &row.file_path, &off).is_ok() {
                store::move_to(&self.pool, row.id, &off, false).await.map_err(internal)?;
                switched = true;
            }
        }
        if switched {
            self.hand_back(facts).await?;
        }

        sqlx::query(
            "UPDATE servers SET game_version = ?, loader = ?, loader_version = ?,
                    restart_required = 1, updated_at = ? WHERE id = ?",
        )
        .bind(&request.game_version)
        .bind(loader)
        .bind(request.loader_version.as_deref().or(facts.loader_version.as_deref()))
        .bind(Timestamp::now())
        .bind(facts.id)
        .execute(&self.pool)
        .await
        .map_err(internal)?;

        Ok(())
    }

    async fn fits(&self, row: &ItemRow, target: &Target) -> bool {
        let (Some(project_id), Some(version_id)) = (&row.project_id, &row.version_id) else {
            return true;
        };
        let Ok(versions) = self.modrinth.versions(project_id).await else { return true };
        versions
            .iter()
            .any(|version| &version.id == version_id && compat::matches(version, target))
    }

    async fn obtain_pack(
        &self,
        source: &PackSource,
        work: &Path,
    ) -> std::result::Result<(PathBuf, PackCard), OperationError> {
        match source {
            PackSource::Upload { archive, file_name } => Ok((
                archive.clone(),
                PackCard {
                    source_kind: ModpackSourceKind::Local,
                    project_id: None,
                    version_id: None,
                    filename: Some(file_name.clone()),
                    date_published: None,
                },
            )),
            PackSource::Modrinth { project_id, version_id } => {
                let version = match version_id {
                    Some(id) => self.modrinth.version(id).await.map_err(download)?,
                    None => {
                        let mut versions =
                            self.modrinth.versions(project_id).await.map_err(download)?;
                        versions.sort_by(|left, right| right.published().cmp(&left.published()));
                        versions.into_iter().next().ok_or_else(|| OperationError {
                            code: "modpack_no_primary_file".to_owned(),
                            message: "this modpack has no published version".to_owned(),
                            step: OperationErrorStep::Modpack,
                        })?
                    }
                };
                let file = version.primary_file().cloned().ok_or_else(|| OperationError {
                    code: "modpack_no_primary_file".to_owned(),
                    message: "no primary file".to_owned(),
                    step: OperationErrorStep::Modpack,
                })?;

                let archive = work.join("pack.mrpack");
                self.modrinth
                    .download(&file.url, &archive, &file.hashes)
                    .await
                    .map_err(download)?;
                Ok((
                    archive,
                    PackCard {
                        source_kind: ModpackSourceKind::ModrinthModpack,
                        project_id: Some(project_id.clone()),
                        version_id: Some(version.id.clone()),
                        filename: Some(file.filename.clone()),
                        date_published: version.published(),
                    },
                ))
            }
        }
    }

    async fn clear_extra(
        &self,
        facts: &ServerFacts,
        root: &Path,
    ) -> std::result::Result<(), OperationError> {
        for row in store::list(&self.pool, facts.id).await.map_err(internal)? {
            if let Ok(full) = paths::resolve_leaf(root, &row.file_path) {
                self.let_the_panel_in(facts, &full, &row.file_path).await;
                let _ = paths::remove(&full);
            }
            store::remove(&self.pool, row.id).await.map_err(internal)?;
        }
        Ok(())
    }

    async fn record_pack(
        &self,
        facts: &ServerFacts,
        files: &[(String, mrpack::PackFile)],
        omitted: &BTreeSet<String>,
        card: &PackCard,
        title: &str,
        version_number: &str,
    ) -> std::result::Result<(), OperationError> {
        let known = store::list(&self.pool, facts.id).await.map_err(internal)?;

        for (path, file) in files {
            let ids = file.modrinth_ids();
            let mut row = known
                .iter()
                .find(|row| store::base_path(&row.file_path) == path)
                .cloned()
                .unwrap_or_else(|| {
                    ItemRow::fresh(facts.id, path, facts.loader.content_type())
                });
            row.file_path = path.clone();
            row.file_name = store::file_name_of(path);
            row.enabled = true;
            row.size_bytes = file.file_size as i64;
            row.source_kind = ContentSourceKind::ModrinthModpack;
            row.sha512 = file.hashes.sha512.clone();
            row.project_id = ids.as_ref().map(|(project, _)| project.clone());
            row.version_id = ids.as_ref().map(|(_, version)| version.clone());
            row.external = ids.is_none();
            row.external_url = file.external_url().map(str::to_owned);
            row.pack_client_depends = false;
            store::upsert(&self.pool, &row).await.map_err(internal)?;

            if let Some((_, version_id)) = &ids {
                if let Ok(version) = self.modrinth.version(version_id).await {
                    let dependencies: Vec<(String, String)> = version
                        .dependencies
                        .iter()
                        .filter_map(|dependency| {
                            Some((dependency.project_id.clone()?, dependency.dependency_type.clone()))
                        })
                        .collect();
                    row.pack_client_depends = dependencies
                        .iter()
                        .any(|(project, kind)| kind == "required" && omitted.contains(project));
                    store::set_dependencies(&self.pool, row.id, &dependencies)
                        .await
                        .map_err(internal)?;
                    if row.pack_client_depends {
                        store::upsert(&self.pool, &row).await.map_err(internal)?;
                    }
                }
            }
        }

        store::link(
            &self.pool,
            &ModpackRow {
                server_id: facts.id,
                source_kind: card.source_kind,
                project_id: card.project_id.clone(),
                version_id: card.version_id.clone(),
                title: if title.is_empty() { "Modpack".to_owned() } else { title.to_owned() },
                filename: card.filename.clone(),
                version_number: Some(version_number.to_owned()),
                date_published: card.date_published,
                has_update: false,
                update_version_id: None,
                linked_at: Timestamp::now(),
            },
        )
        .await
        .map_err(internal)?;
        Ok(())
    }

    async fn write_row(
        &self,
        facts: &ServerFacts,
        item: &Wanted,
        relative: &str,
        source: ContentSourceKind,
        sha512: Option<String>,
    ) -> std::result::Result<(), OperationError> {
        let existing = match item.replaces {
            Some(id) => store::one(&self.pool, facts.id, id).await.map_err(internal)?,
            None => None,
        };
        let mut row = match existing {
            Some(row) => row,
            None => self
                .row_for(facts, &self.server_dir(facts), relative)
                .await
                .map_err(|failure| OperationError {
                    code: "internal".to_owned(),
                    message: failure.to_string(),
                    step: OperationErrorStep::Internal,
                })?,
        };

        row.file_path = relative.to_owned();
        row.file_name = store::file_name_of(relative);
        row.enabled = true;
        row.size_bytes = item.file.size as i64;
        row.source_kind = source;
        row.project_id = Some(item.project_id.clone());
        row.version_id = Some(item.version.id.clone());
        row.sha512 = sha512.or_else(|| item.file.hashes.sha512.clone());
        row.has_update = false;
        row.update_version_id = None;
        if let Some(kind) = self
            .modrinth
            .cached_project(&item.project_id)
            .await
            .ok()
            .flatten()
            .and_then(|project| project.project_type)
            .as_deref()
            .and_then(install::kind_of)
        {
            row.project_type = kind;
        }
        store::upsert(&self.pool, &row).await.map_err(internal)?;
        let _ = self.modrinth.remember_version(&item.version).await;

        let dependencies: Vec<(String, String)> = item
            .version
            .dependencies
            .iter()
            .filter_map(|dependency| {
                Some((dependency.project_id.clone()?, dependency.dependency_type.clone()))
            })
            .collect();
        store::set_dependencies(&self.pool, row.id, &dependencies).await.map_err(internal)?;
        Ok(())
    }

    async fn work_dir(&self, operation: Id) -> std::result::Result<PathBuf, OperationError> {
        self.operations.work_dir(operation).await.map_err(|fault| OperationError {
            code: "internal".to_owned(),
            message: fault.message().to_owned(),
            step: OperationErrorStep::Internal,
        })
    }

    async fn hand_back(&self, facts: &ServerFacts) -> std::result::Result<(), OperationError> {
        let steps = crate::helper::in_servers(facts.id);
        self.helper.chown_tree(&facts.owner_id.to_string(), steps).await.map_err(|err| {
            OperationError {
                code: "internal".to_owned(),
                message: format!("the files could not be handed back: {err}"),
                step: OperationErrorStep::Filesystem,
            }
        })?;
        Ok(())
    }
}

struct PackCard {
    source_kind: ModpackSourceKind,
    project_id: Option<String>,
    version_id: Option<String>,
    filename: Option<String>,
    date_published: Option<Timestamp>,
}

fn download(err: Upstream) -> OperationError {
    let code = match err {
        Upstream::Damaged => "checksum_mismatch",
        Upstream::RateLimited | Upstream::Unavailable(_) => "upstream_unavailable",
        Upstream::Io(_) => "no_space",
        _ => "upstream_unavailable",
    };
    OperationError {
        code: code.to_owned(),
        message: err.to_string(),
        step: OperationErrorStep::Download,
    }
}

fn filesystem(err: std::io::Error) -> OperationError {
    let code = match err.kind() {
        std::io::ErrorKind::StorageFull | std::io::ErrorKind::QuotaExceeded => "no_space",
        _ => "invalid_path",
    };
    OperationError {
        code: code.to_owned(),
        message: err.to_string(),
        step: OperationErrorStep::Filesystem,
    }
}

fn pack_fault(err: mrpack::PackFault) -> OperationError {
    let step = match err {
        mrpack::PackFault::Io(_) => OperationErrorStep::Filesystem,
        _ => OperationErrorStep::Modpack,
    };
    OperationError { code: err.code().to_owned(), message: err.to_string(), step }
}

fn clear_away(staging: Option<PathBuf>) {
    let Some(archive) = staging else { return };
    let _ = std::fs::remove_file(&archive);
    if let Some(parent) = archive.parent() {
        let _ = std::fs::remove_dir(parent);
    }
}

fn internal(err: sqlx::Error) -> OperationError {
    OperationError {
        code: "internal".to_owned(),
        message: err.to_string(),
        step: OperationErrorStep::Internal,
    }
}

#[cfg(test)]
mod tests;
