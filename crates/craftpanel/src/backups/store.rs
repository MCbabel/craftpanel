use sqlx::SqlitePool;

use crate::auth::error::{Failure, Result};
use crate::model::{
    AlwaysFalse, Backup, BackupActiveOperation, BackupLocation, BackupOperation,
    BackupOperationState, BackupOperationType, BackupSchedule, BackupScheduleStatus, BackupStatus,
    DriveFileState, Id, OperationKind, OperationState, Timestamp, UserRef,
};

pub const HISTORY: usize = 20;

pub const MAX_QUOTA: u32 = 50;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Row {
    pub id: Id,
    pub server_id: Id,
    pub name: String,
    pub automated: bool,
    pub size_bytes: i64,
    pub created_at: Timestamp,
    pub location: BackupLocation,
    pub drive_file_id: Option<String>,
    pub drive_state: Option<DriveFileState>,
    pub drive_md5: Option<String>,
    pub drive_content_changed_at: Option<Timestamp>,
}

const COLUMNS: &str = "id, server_id, name, automated, size_bytes, created_at, location, \
     drive_file_id, drive_state, drive_md5, drive_content_changed_at";

#[derive(Debug, Clone, sqlx::FromRow)]
struct RunRow {
    backup_id: Id,
    operation_id: Id,
    kind: OperationKind,
    state: OperationState,
    error_code: Option<String>,
    error_message: Option<String>,
    created_at: Timestamp,
    started_at: Option<Timestamp>,
    finished_at: Option<Timestamp>,
    dismissed_at: Option<Timestamp>,
    has_parent: bool,
    started_by: Option<Id>,
    username: Option<String>,
}

impl RunRow {
    fn state(&self) -> BackupOperationState {
        BackupOperationState::of(self.state, self.error_code.as_deref())
    }

    fn user(&self) -> Option<UserRef> {
        Some(UserRef {
            id: self.started_by?,
            username: self.username.clone()?,
            avatar_url: None,
        })
    }

    fn operation_type(&self) -> BackupOperationType {
        BackupOperationType::of(self.kind).unwrap_or(BackupOperationType::Create)
    }
}

impl From<&RunRow> for BackupOperation {
    fn from(row: &RunRow) -> Self {
        Self {
            operation_type: row.operation_type(),
            operation_id: row.operation_id,
            state: row.state(),
            scheduled_for: row.created_at,
            started_at: row.started_at,
            completed_at: row.finished_at,
            has_parent: row.has_parent,
            error: row.error_message.clone(),
            should_prompt: row.dismissed_at.is_none(),
            synthetic_legacy: AlwaysFalse,
            user_info: row.user(),
        }
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct BackupListResponse {
    pub active_operations: Vec<BackupActiveOperation>,
    pub backups: Vec<Backup>,
}

pub async fn list(pool: &SqlitePool, server: Id) -> Result<BackupListResponse> {
    let rows: Vec<Row> = sqlx::query_as(&format!(
        "SELECT {COLUMNS} FROM backups WHERE server_id = ? ORDER BY created_at DESC, id DESC"
    ))
    .bind(server)
    .fetch_all(pool)
    .await?;

    let runs = runs_of(pool, server).await?;
    let mut active = Vec::new();
    let mut backups = Vec::with_capacity(rows.len());

    for row in rows {
        let mine: Vec<&RunRow> = runs.iter().filter(|run| run.backup_id == row.id).collect();
        for run in mine.iter().filter(|run| run.state.is_open()) {
            active.push(BackupActiveOperation {
                backup_id: row.id,
                operation_type: run.operation_type(),
                operation_id: run.operation_id,
                has_parent: run.has_parent,
                scheduled_for: run.created_at,
                started_at: run.started_at,
                synthetic_legacy: AlwaysFalse,
                user_info: run.user(),
            });
        }
        backups.push(backup_of(&row, &mine));
    }

    Ok(BackupListResponse { active_operations: active, backups })
}

pub async fn one(pool: &SqlitePool, backup: Id) -> Result<Backup> {
    let row = find(pool, backup).await?;
    let runs = runs_of(pool, row.server_id).await?;
    let mine: Vec<&RunRow> = runs.iter().filter(|run| run.backup_id == backup).collect();
    Ok(backup_of(&row, &mine))
}

fn backup_of(row: &Row, runs: &[&RunRow]) -> Backup {
    let history: Vec<BackupOperation> =
        runs.iter().take(HISTORY).map(|run| BackupOperation::from(*run)).collect();
    let status = status_of(row, runs);
    Backup {
        id: row.id,
        name: row.name.clone(),
        created_at: row.created_at,
        status,
        locked: AlwaysFalse,
        automated: row.automated,
        location: row.location,
        drive_state: row.drive_state,
        drive_verified: (row.location == BackupLocation::Drive)
            .then_some(row.drive_md5.is_some()),
        drive_content_changed: (row.location == BackupLocation::Drive)
            .then_some(row.drive_content_changed_at.is_some()),
        drive_web_link: row
            .drive_file_id
            .as_deref()
            .map(crate::drive::files::web_link),
        size_bytes: match status {
            BackupStatus::Done => row.size_bytes.max(0) as u64,
            _ => 0,
        },
        history,
    }
}

fn status_of(row: &Row, runs: &[&RunRow]) -> BackupStatus {
    match runs.first() {
        Some(newest) => BackupStatus::of(newest.state()),
        None if row.size_bytes > 0 => BackupStatus::Done,
        None => BackupStatus::Error,
    }
}

async fn runs_of(pool: &SqlitePool, server: Id) -> Result<Vec<RunRow>> {
    Ok(sqlx::query_as(
        "SELECT o.target_id AS backup_id, o.id AS operation_id, o.kind, o.state, \
                o.error_code, o.error_message, o.created_at, o.started_at, o.finished_at, \
                o.dismissed_at, o.parent_operation_id IS NOT NULL AS has_parent, \
                o.started_by, u.username \
           FROM operations o LEFT JOIN users u ON u.id = o.started_by \
          WHERE o.server_id = ? AND o.target_id IS NOT NULL \
            AND o.kind IN ('backup_create', 'backup_restore') \
          ORDER BY o.created_at DESC, o.id DESC",
    )
    .bind(server)
    .fetch_all(pool)
    .await?)
}

pub async fn newest_run(pool: &SqlitePool, backup: Id) -> Result<Option<BackupOperation>> {
    let row: Option<RunRow> = sqlx::query_as(
        "SELECT o.target_id AS backup_id, o.id AS operation_id, o.kind, o.state, \
                o.error_code, o.error_message, o.created_at, o.started_at, o.finished_at, \
                o.dismissed_at, o.parent_operation_id IS NOT NULL AS has_parent, \
                o.started_by, u.username \
           FROM operations o LEFT JOIN users u ON u.id = o.started_by \
          WHERE o.target_id = ? AND o.kind IN ('backup_create', 'backup_restore') \
          ORDER BY o.created_at DESC, o.id DESC LIMIT 1",
    )
    .bind(backup)
    .fetch_optional(pool)
    .await?;
    Ok(row.as_ref().map(BackupOperation::from))
}

pub async fn interrupted_creates(pool: &SqlitePool) -> Result<Vec<(Id, Id)>> {
    Ok(sqlx::query_as(
        "SELECT server_id, target_id FROM operations \
          WHERE kind = 'backup_create' AND state IN ('queued', 'ongoing') \
            AND target_id IS NOT NULL",
    )
    .fetch_all(pool)
    .await?)
}

pub async fn safety_copy_for(pool: &SqlitePool, backup: Id) -> Result<Option<Id>> {
    Ok(sqlx::query_scalar(
        "SELECT copy.target_id FROM operations copy \
           JOIN operations restore ON restore.id = copy.parent_operation_id \
          WHERE copy.kind = 'backup_create' AND restore.kind = 'backup_restore' \
            AND restore.target_id = ? AND copy.target_id IS NOT NULL \
          ORDER BY copy.created_at DESC, copy.id DESC LIMIT 1",
    )
    .bind(backup)
    .fetch_optional(pool)
    .await?
    .flatten())
}

pub async fn is_busy(pool: &SqlitePool, backup: Id) -> Result<bool> {
    let open: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM operations \
          WHERE target_id = ? AND state IN ('queued', 'ongoing')",
    )
    .bind(backup)
    .fetch_one(pool)
    .await?;
    Ok(open > 0)
}

pub async fn find(pool: &SqlitePool, backup: Id) -> Result<Row> {
    let row: Option<Row> =
        sqlx::query_as(&format!("SELECT {COLUMNS} FROM backups WHERE id = ?"))
    .bind(backup)
    .fetch_optional(pool)
    .await?;
    row.ok_or_else(|| Failure::not_found("backup_not_found", "no such backup"))
}

pub async fn count(pool: &SqlitePool, server: Id) -> Result<u32> {
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM backups WHERE server_id = ?")
        .bind(server)
        .fetch_one(pool)
        .await?;
    Ok(count as u32)
}

pub async fn quota(pool: &SqlitePool) -> Result<u32> {
    let quota: i64 =
        sqlx::query_scalar("SELECT max_backups_per_server FROM panel_settings WHERE id = 1")
            .fetch_one(pool)
            .await?;
    Ok((quota as u32).min(MAX_QUOTA))
}

pub async fn newest_automatic_finish(pool: &SqlitePool, server: Id) -> Result<Option<Timestamp>> {
    Ok(sqlx::query_scalar(
        "SELECT o.finished_at FROM operations o JOIN backups b ON b.id = o.target_id \
          WHERE o.server_id = ? AND o.kind = 'backup_create' AND o.state = 'done' \
            AND b.automated = 1 \
          ORDER BY o.finished_at DESC LIMIT 1",
    )
    .bind(server)
    .fetch_optional(pool)
    .await?
    .flatten())
}

pub async fn automatic_over(pool: &SqlitePool, server: Id, keep: u32) -> Result<Vec<Id>> {
    let rows: Vec<Id> = sqlx::query_scalar(
        "SELECT id FROM backups WHERE server_id = ? AND automated = 1 \
          ORDER BY created_at DESC, id DESC",
    )
    .bind(server)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().skip(keep as usize).collect())
}

pub async fn insert(
    pool: &SqlitePool,
    server: Id,
    name: &str,
    automated: bool,
    location: BackupLocation,
) -> Result<Row> {
    let row = Row {
        id: Id::new(),
        server_id: server,
        name: name.to_owned(),
        automated,
        size_bytes: 0,
        created_at: Timestamp::now(),
        location,
        drive_file_id: None,
        drive_state: None,
        drive_md5: None,
        drive_content_changed_at: None,
    };
    sqlx::query(
        "INSERT INTO backups (id, server_id, name, automated, size_bytes, created_at, location) \
         VALUES (?, ?, ?, ?, 0, ?, ?)",
    )
    .bind(row.id)
    .bind(row.server_id)
    .bind(&row.name)
    .bind(row.automated)
    .bind(row.created_at)
    .bind(row.location)
    .execute(pool)
    .await?;
    Ok(row)
}

pub async fn finish_upload(
    pool: &SqlitePool,
    backup: Id,
    file_id: &str,
    size: u64,
    md5: Option<&str>,
    now: Timestamp,
) -> Result<()> {
    sqlx::query(
        "UPDATE backups SET size_bytes = ?, drive_file_id = ?, drive_md5 = ?, \
             drive_state = 'present', drive_content_changed_at = NULL, \
             drive_checked_at = ? WHERE id = ?",
    )
    .bind(size as i64)
    .bind(file_id)
    .bind(md5)
    .bind(now)
    .bind(backup)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn rename(pool: &SqlitePool, backup: Id, name: &str) -> Result<()> {
    sqlx::query("UPDATE backups SET name = ? WHERE id = ?")
        .bind(name)
        .bind(backup)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn set_size(pool: &SqlitePool, backup: Id, bytes: u64) -> Result<()> {
    sqlx::query("UPDATE backups SET size_bytes = ? WHERE id = ?")
        .bind(bytes as i64)
        .bind(backup)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn remove(pool: &SqlitePool, backup: Id) -> Result<()> {
    sqlx::query("DELETE FROM backups WHERE id = ?").bind(backup).execute(pool).await?;
    Ok(())
}

pub async fn newest_manual_request(pool: &SqlitePool, server: Id) -> Result<Option<Timestamp>> {
    Ok(sqlx::query_scalar(
        "SELECT o.created_at FROM operations o JOIN backups b ON b.id = o.target_id \
          WHERE o.server_id = ? AND o.kind = 'backup_create' AND b.automated = 0 \
            AND o.parent_operation_id IS NULL \
          ORDER BY o.created_at DESC LIMIT 1",
    )
    .bind(server)
    .fetch_optional(pool)
    .await?
    .flatten())
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct ScheduleRow {
    enabled: bool,
    interval_hours: i64,
    hour_utc: i64,
    keep_last: i64,
    next_run_at: Option<Timestamp>,
    last_run_at: Option<Timestamp>,
    last_status: Option<BackupScheduleStatus>,
    last_error: Option<String>,
}

impl From<ScheduleRow> for BackupSchedule {
    fn from(row: ScheduleRow) -> Self {
        Self {
            enabled: row.enabled,
            interval_hours: row.interval_hours as u32,
            hour_utc: row.hour_utc as u8,
            keep_last: row.keep_last as u32,
            next_run_at: row.next_run_at,
            last_run_at: row.last_run_at,
            last_status: row.last_status,
            last_error: row.last_error,
        }
    }
}

pub async fn schedule(pool: &SqlitePool, server: Id) -> Result<BackupSchedule> {
    let row: Option<ScheduleRow> = sqlx::query_as(
        "SELECT enabled, interval_hours, hour_utc, keep_last, next_run_at, last_run_at, \
                last_status, last_error FROM backup_schedules WHERE server_id = ?",
    )
    .bind(server)
    .fetch_optional(pool)
    .await?;

    Ok(row.map(BackupSchedule::from).unwrap_or(BackupSchedule {
        enabled: false,
        interval_hours: 24,
        hour_utc: 4,
        keep_last: 5,
        next_run_at: None,
        last_run_at: None,
        last_status: None,
        last_error: None,
    }))
}

pub async fn save_schedule(
    pool: &SqlitePool,
    server: Id,
    wanted: &BackupSchedule,
) -> Result<BackupSchedule> {
    sqlx::query(
        "INSERT INTO backup_schedules \
             (server_id, enabled, interval_hours, hour_utc, keep_last, next_run_at) \
         VALUES (?, ?, ?, ?, ?, ?) \
         ON CONFLICT(server_id) DO UPDATE SET \
             enabled = excluded.enabled, interval_hours = excluded.interval_hours, \
             hour_utc = excluded.hour_utc, keep_last = excluded.keep_last, \
             next_run_at = excluded.next_run_at",
    )
    .bind(server)
    .bind(wanted.enabled)
    .bind(wanted.interval_hours)
    .bind(wanted.hour_utc)
    .bind(wanted.keep_last)
    .bind(wanted.next_run_at)
    .execute(pool)
    .await?;
    schedule(pool, server).await
}

pub async fn reserve(pool: &SqlitePool, server: Id, next: Timestamp) -> Result<()> {
    sqlx::query("UPDATE backup_schedules SET next_run_at = ? WHERE server_id = ?")
        .bind(next)
        .bind(server)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn record_run(
    pool: &SqlitePool,
    server: Id,
    at: Timestamp,
    status: BackupScheduleStatus,
    error: Option<&str>,
    next: Option<Timestamp>,
) -> Result<()> {
    sqlx::query(
        "UPDATE backup_schedules \
            SET last_run_at = ?, last_status = ?, last_error = ?, next_run_at = ? \
          WHERE server_id = ?",
    )
    .bind(at)
    .bind(status)
    .bind(error)
    .bind(next)
    .bind(server)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn due(pool: &SqlitePool, now: Timestamp) -> Result<Vec<Id>> {
    Ok(sqlx::query_scalar(
        "SELECT server_id FROM backup_schedules \
          WHERE enabled = 1 AND next_run_at IS NOT NULL AND next_run_at <= ? \
          ORDER BY next_run_at",
    )
    .bind(now)
    .fetch_all(pool)
    .await?)
}
