use serde::Serialize;
use sqlx::{Sqlite, SqlitePool, Transaction};

use crate::model::{
    BusyReasonCode, Id, Operation, OperationError, OperationErrorStep, OperationKind,
    OperationPhase, OperationState, ServerStatus, Timestamp,
};

use super::fault::{Answer, Fault};

const COLUMNS: &str = "id, server_id, kind, state, phase, progress, message, src,
     bytes_processed, files_processed, current_file, error_code, error_message, error_step,
     cancellable, target_id, started_by, created_at, started_at, finished_at, dismissed_at";

pub const SNAPSHOT_LIMIT: u32 = 200;

#[derive(Debug, Clone)]
pub struct NewOperation {
    pub server_id: Id,
    pub kind: OperationKind,
    pub started_by: Option<Id>,
    pub src: Option<String>,
    pub target_id: Option<Id>,
    pub parent_operation_id: Option<Id>,
    pub input: Option<serde_json::Value>,
    pub expects_payload: bool,
    pub message: Option<String>,
}

impl NewOperation {
    pub fn new(server_id: Id, kind: OperationKind, started_by: Option<Id>) -> Self {
        Self {
            server_id,
            kind,
            started_by,
            src: None,
            target_id: None,
            parent_operation_id: None,
            input: None,
            expects_payload: false,
            message: None,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct Step {
    pub phase: Option<OperationPhase>,
    pub progress: Option<f64>,
    pub message: Option<String>,
    pub bytes_processed: Option<u64>,
    pub files_processed: Option<u64>,
    pub current_file: Option<String>,
    pub cancellable: Option<bool>,
}

impl Step {
    pub fn is_urgent(&self) -> bool {
        self.phase.is_some() || self.cancellable.is_some()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Snapshot {
    pub revision: i64,
    pub operations: Vec<Operation>,
    pub busy_reasons: Vec<BusyReasonCode>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, sqlx::Type)]
#[sqlx(type_name = "TEXT", rename_all = "snake_case")]
pub enum Payload {
    None,
    Expected,
    Delivered,
}

#[derive(sqlx::FromRow)]
struct Row {
    id: Id,
    server_id: Id,
    kind: OperationKind,
    state: OperationState,
    phase: Option<OperationPhase>,
    progress: f64,
    message: Option<String>,
    src: Option<String>,
    bytes_processed: Option<i64>,
    files_processed: Option<i64>,
    current_file: Option<String>,
    error_code: Option<String>,
    error_message: Option<String>,
    error_step: Option<OperationErrorStep>,
    cancellable: bool,
    target_id: Option<Id>,
    started_by: Option<Id>,
    created_at: Timestamp,
    started_at: Option<Timestamp>,
    finished_at: Option<Timestamp>,
    dismissed_at: Option<Timestamp>,
}

impl From<Row> for Operation {
    fn from(row: Row) -> Self {
        let error = match (row.error_code, row.error_message, row.error_step) {
            (Some(code), Some(message), Some(step)) => Some(OperationError { code, message, step }),
            _ => None,
        };
        Self {
            id: row.id,
            server_id: row.server_id,
            kind: row.kind,
            state: row.state,
            phase: row.phase,
            progress: row.progress,
            message: row.message,
            src: row.src,
            bytes_processed: row.bytes_processed.map(|n| n as u64),
            files_processed: row.files_processed.map(|n| n as u64),
            current_file: row.current_file,
            error,
            cancellable: row.cancellable,
            target_id: row.target_id,
            started_by: row.started_by,
            created_at: row.created_at,
            started_at: row.started_at,
            finished_at: row.finished_at,
            dismissed_at: row.dismissed_at,
        }
    }
}

async fn write_tx(pool: &SqlitePool) -> Answer<Transaction<'static, Sqlite>> {
    Ok(pool.begin_with("BEGIN IMMEDIATE").await?)
}

pub async fn insert(pool: &SqlitePool, new: &NewOperation) -> Answer<Operation> {
    let mut tx = write_tx(pool).await?;
    let operation = insert_in(&mut tx, new).await?;
    tx.commit().await?;
    Ok(operation)
}

pub async fn insert_in(
    tx: &mut Transaction<'_, Sqlite>,
    new: &NewOperation,
) -> Answer<Operation> {
    let id = Id::new();
    let now = Timestamp::now();
    let payload = if new.expects_payload { Payload::Expected } else { Payload::None };

    let written = sqlx::query(
        "INSERT INTO operations (id, server_id, kind, state, progress, message, src,
                                 cancellable, target_id, parent_operation_id, started_by,
                                 input, payload, created_at)
         VALUES (?, ?, ?, 'queued', 0, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(id)
    .bind(new.server_id)
    .bind(new.kind)
    .bind(new.message.as_deref())
    .bind(new.src.as_deref())
    .bind(new.kind.is_cancellable())
    .bind(new.target_id)
    .bind(new.parent_operation_id)
    .bind(new.started_by)
    .bind(new.input.as_ref().map(|value| value.to_string()))
    .bind(payload)
    .bind(now)
    .execute(&mut **tx)
    .await;
    if let Err(err) = written {
        return Err(collision_or(err));
    }

    bump_revision(tx, new.server_id).await?;
    fetch_in(tx, id).await
}

pub async fn fetch(pool: &SqlitePool, id: Id) -> Answer<Operation> {
    let row: Option<Row> = sqlx::query_as(&format!("SELECT {COLUMNS} FROM operations WHERE id = ?"))
        .bind(id)
        .fetch_optional(pool)
        .await?;
    row.map(Operation::from).ok_or_else(operation_not_found)
}

async fn fetch_in(tx: &mut Transaction<'_, Sqlite>, id: Id) -> Answer<Operation> {
    let row: Option<Row> = sqlx::query_as(&format!("SELECT {COLUMNS} FROM operations WHERE id = ?"))
        .bind(id)
        .fetch_optional(&mut **tx)
        .await?;
    row.map(Operation::from).ok_or_else(operation_not_found)
}

pub async fn fetch_of_server(pool: &SqlitePool, server: Id, id: Id) -> Answer<Operation> {
    let operation = fetch(pool, id).await?;
    if operation.server_id != server {
        return Err(operation_not_found());
    }
    Ok(operation)
}

pub fn operation_not_found() -> Fault {
    Fault::not_found("operation_not_found", "no such operation")
}

fn collision_or(err: sqlx::Error) -> Fault {
    let collided = matches!(&err, sqlx::Error::Database(db)
        if db.message().contains("UNIQUE constraint failed"));
    if collided {
        Fault::conflict("server_busy", "a backup run is already open on this server")
    } else {
        Fault::from(err)
    }
}

const STANDING_FAILURE: &str = "(kind = 'server_delete' AND state = 'failed'
           AND EXISTS (SELECT 1 FROM servers s
                        WHERE s.id = operations.server_id AND s.status = 'deleting'))";

pub async fn list_for_server(
    pool: &SqlitePool,
    server: Id,
    active_only: bool,
    include_dismissed: bool,
    limit: u32,
    before: Option<Id>,
) -> Answer<Vec<Operation>> {
    let mut sql = format!("SELECT {COLUMNS} FROM operations WHERE server_id = ?");
    if active_only {
        sql.push_str(" AND state IN ('queued', 'ongoing')");
    }
    if !include_dismissed {
        sql.push_str(" AND (dismissed_at IS NULL OR ");
        sql.push_str(STANDING_FAILURE);
        sql.push(')');
    }
    if before.is_some() {
        sql.push_str(" AND id < ?");
    }
    sql.push_str(" ORDER BY id DESC LIMIT ?");

    let mut query = sqlx::query_as::<_, Row>(&sql).bind(server);
    if let Some(before) = before {
        query = query.bind(before);
    }
    let rows = query.bind(limit).fetch_all(pool).await?;
    Ok(rows.into_iter().map(Operation::from).collect())
}

pub async fn list_for_servers(
    pool: &SqlitePool,
    servers: &[Id],
    active_only: bool,
    limit: u32,
    before: Option<Id>,
) -> Answer<Vec<Operation>> {
    if servers.is_empty() {
        return Ok(Vec::new());
    }
    let mut sql = format!("SELECT {COLUMNS} FROM operations WHERE dismissed_at IS NULL AND ");
    sql.push_str(&in_clause("server_id", servers.len()));
    if active_only {
        sql.push_str(" AND state IN ('queued', 'ongoing')");
    }
    if before.is_some() {
        sql.push_str(" AND id < ?");
    }
    sql.push_str(" ORDER BY id DESC LIMIT ?");

    let mut query = sqlx::query_as::<_, Row>(&sql);
    for server in servers {
        query = query.bind(*server);
    }
    if let Some(before) = before {
        query = query.bind(before);
    }
    let rows = query.bind(limit).fetch_all(pool).await?;
    Ok(rows.into_iter().map(Operation::from).collect())
}

fn in_clause(column: &str, count: usize) -> String {
    let marks = std::iter::repeat_n("?", count).collect::<Vec<_>>().join(", ");
    format!("{column} IN ({marks})")
}

pub async fn snapshot(pool: &SqlitePool, server: Id) -> Answer<Snapshot> {
    Ok(Snapshot {
        revision: revision(pool, server).await?,
        operations: list_for_server(pool, server, false, false, SNAPSHOT_LIMIT, None).await?,
        busy_reasons: busy_reasons(pool, server).await?,
    })
}

pub async fn revision(pool: &SqlitePool, server: Id) -> Answer<i64> {
    let row: Option<(i64,)> =
        sqlx::query_as("SELECT operations_revision FROM servers WHERE id = ?")
            .bind(server)
            .fetch_optional(pool)
            .await?;
    Ok(row.map(|(revision,)| revision).unwrap_or_default())
}

async fn bump_revision(tx: &mut Transaction<'_, Sqlite>, server: Id) -> Answer<()> {
    sqlx::query("UPDATE servers SET operations_revision = operations_revision + 1 WHERE id = ?")
        .bind(server)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

pub async fn busy_reasons(pool: &SqlitePool, server: Id) -> Answer<Vec<BusyReasonCode>> {
    let rows: Vec<(OperationKind,)> = sqlx::query_as(
        "SELECT DISTINCT kind FROM operations
          WHERE server_id = ? AND state IN ('queued', 'ongoing')",
    )
    .bind(server)
    .fetch_all(pool)
    .await?;
    Ok(reasons_of(rows.into_iter().map(|(kind,)| kind)))
}

pub async fn busy_reasons_by_server(
    pool: &SqlitePool,
    servers: &[Id],
) -> Answer<std::collections::BTreeMap<Id, Vec<BusyReasonCode>>> {
    let mut out = std::collections::BTreeMap::new();
    if servers.is_empty() {
        return Ok(out);
    }
    let sql = format!(
        "SELECT DISTINCT server_id, kind FROM operations
          WHERE state IN ('queued', 'ongoing') AND {}",
        in_clause("server_id", servers.len())
    );
    let mut query = sqlx::query_as::<_, (Id, OperationKind)>(&sql);
    for server in servers {
        query = query.bind(*server);
    }

    let mut kinds: std::collections::BTreeMap<Id, Vec<OperationKind>> = Default::default();
    for (server, kind) in query.fetch_all(pool).await? {
        kinds.entry(server).or_default().push(kind);
    }
    for (server, kinds) in kinds {
        let reasons = reasons_of(kinds.into_iter());
        if !reasons.is_empty() {
            out.insert(server, reasons);
        }
    }
    Ok(out)
}

fn reasons_of(kinds: impl Iterator<Item = OperationKind>) -> Vec<BusyReasonCode> {
    let mut reasons: Vec<BusyReasonCode> = kinds.filter_map(OperationKind::busy_reason).collect();
    reasons.sort();
    reasons.dedup();
    reasons
}

pub async fn begin(pool: &SqlitePool, id: Id) -> Answer<Option<Operation>> {
    let mut tx = write_tx(pool).await?;

    let row: Option<(Id, OperationState, Payload)> =
        sqlx::query_as("SELECT server_id, state, payload FROM operations WHERE id = ?")
            .bind(id)
            .fetch_optional(&mut *tx)
            .await?;
    let Some((server, state, payload)) = row else {
        return Err(operation_not_found());
    };
    if state != OperationState::Queued || payload == Payload::Expected {
        return Ok(None);
    }

    let (mine,): (i64,) = sqlx::query_as(
        "SELECT count(*) FROM operations WHERE server_id = ? AND state = 'ongoing'",
    )
    .bind(server)
    .fetch_one(&mut *tx)
    .await?;
    let (running,): (i64,) =
        sqlx::query_as("SELECT count(*) FROM operations WHERE state = 'ongoing'")
            .fetch_one(&mut *tx)
            .await?;
    let (width,): (i64,) =
        sqlx::query_as("SELECT max_concurrent_operations FROM panel_settings WHERE id = 1")
            .fetch_one(&mut *tx)
            .await?;

    if mine > 0 || running >= width {
        return Ok(None);
    }

    let now = Timestamp::now();
    sqlx::query(
        "UPDATE operations SET state = 'ongoing', started_at = ?, progressed_at = ?
          WHERE id = ? AND state = 'queued'",
    )
    .bind(now)
    .bind(now)
    .bind(id)
    .execute(&mut *tx)
    .await?;

    bump_revision(&mut tx, server).await?;
    let operation = fetch_in(&mut tx, id).await?;
    tx.commit().await?;
    Ok(Some(operation))
}

pub async fn runnable(pool: &SqlitePool) -> Answer<Vec<Id>> {
    let rows: Vec<(Id, Id)> = sqlx::query_as(
        "SELECT id, server_id FROM operations
          WHERE state = 'queued' AND payload <> 'expected'
            AND server_id NOT IN (SELECT server_id FROM operations WHERE state = 'ongoing')
          ORDER BY id",
    )
    .fetch_all(pool)
    .await?;
    let (running,): (i64,) =
        sqlx::query_as("SELECT count(*) FROM operations WHERE state = 'ongoing'")
            .fetch_one(pool)
            .await?;
    let (width,): (i64,) =
        sqlx::query_as("SELECT max_concurrent_operations FROM panel_settings WHERE id = 1")
            .fetch_one(pool)
            .await?;

    let free = (width - running).max(0) as usize;
    let mut seen = std::collections::BTreeSet::new();
    Ok(rows
        .into_iter()
        .filter(|(_, server)| seen.insert(*server))
        .map(|(id, _)| id)
        .take(free)
        .collect())
}

pub async fn advance(pool: &SqlitePool, id: Id, step: &Step) -> Answer<Operation> {
    let mut tx = write_tx(pool).await?;
    let server = server_of(&mut tx, id).await?;

    sqlx::query(
        "UPDATE operations
            SET phase = coalesce(?, phase),
                progress = coalesce(?, progress),
                message = coalesce(?, message),
                bytes_processed = coalesce(?, bytes_processed),
                files_processed = coalesce(?, files_processed),
                current_file = coalesce(?, current_file),
                cancellable = coalesce(?, cancellable),
                progressed_at = ?
          WHERE id = ?",
    )
    .bind(step.phase)
    .bind(step.progress.map(|value| value.clamp(0.0, 1.0)))
    .bind(step.message.as_deref())
    .bind(step.bytes_processed.map(|n| n as i64))
    .bind(step.files_processed.map(|n| n as i64))
    .bind(step.current_file.as_deref())
    .bind(step.cancellable)
    .bind(Timestamp::now())
    .bind(id)
    .execute(&mut *tx)
    .await?;

    bump_revision(&mut tx, server).await?;
    let operation = fetch_in(&mut tx, id).await?;
    tx.commit().await?;
    Ok(operation)
}

pub async fn settle(
    pool: &SqlitePool,
    id: Id,
    state: OperationState,
    error: Option<OperationError>,
) -> Answer<Operation> {
    debug_assert!(state.is_terminal());
    let mut tx = write_tx(pool).await?;
    let server = server_of(&mut tx, id).await?;

    sqlx::query(
        "UPDATE operations
            SET state = ?, finished_at = ?, cancellable = 0,
                progress = CASE WHEN ? = 'done' THEN 1 ELSE progress END,
                error_code = ?, error_message = ?, error_step = ?
          WHERE id = ?",
    )
    .bind(state)
    .bind(Timestamp::now())
    .bind(state)
    .bind(error.as_ref().map(|error| error.code.as_str()))
    .bind(error.as_ref().map(|error| error.message.as_str()))
    .bind(error.as_ref().map(|error| error.step))
    .bind(id)
    .execute(&mut *tx)
    .await?;

    bump_revision(&mut tx, server).await?;
    let operation = fetch_in(&mut tx, id).await?;
    tx.commit().await?;
    Ok(operation)
}

pub async fn request_cancel(pool: &SqlitePool, id: Id) -> Answer<Operation> {
    let mut tx = write_tx(pool).await?;
    let server = server_of(&mut tx, id).await?;
    sqlx::query("UPDATE operations SET cancel_requested = 1 WHERE id = ?")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    bump_revision(&mut tx, server).await?;
    let operation = fetch_in(&mut tx, id).await?;
    tx.commit().await?;
    Ok(operation)
}

pub async fn cancel_requested(pool: &SqlitePool, id: Id) -> Answer<bool> {
    let row: Option<(bool,)> =
        sqlx::query_as("SELECT cancel_requested FROM operations WHERE id = ?")
            .bind(id)
            .fetch_optional(pool)
            .await?;
    Ok(row.is_some_and(|(requested,)| requested))
}

pub async fn dismiss(pool: &SqlitePool, id: Id) -> Answer<()> {
    let mut tx = write_tx(pool).await?;
    let server = server_of(&mut tx, id).await?;
    sqlx::query("UPDATE operations SET dismissed_at = ? WHERE id = ? AND dismissed_at IS NULL")
        .bind(Timestamp::now())
        .bind(id)
        .execute(&mut *tx)
        .await?;
    bump_revision(&mut tx, server).await?;
    tx.commit().await?;
    Ok(())
}

pub async fn payload_state(pool: &SqlitePool, id: Id) -> Answer<Payload> {
    let row: Option<(Payload,)> = sqlx::query_as("SELECT payload FROM operations WHERE id = ?")
        .bind(id)
        .fetch_optional(pool)
        .await?;
    Ok(row.ok_or_else(operation_not_found)?.0)
}

pub async fn set_payload(pool: &SqlitePool, id: Id, payload: Payload) -> Answer<Operation> {
    let mut tx = write_tx(pool).await?;
    let server = server_of(&mut tx, id).await?;
    sqlx::query("UPDATE operations SET payload = ? WHERE id = ?")
        .bind(payload)
        .bind(id)
        .execute(&mut *tx)
        .await?;
    bump_revision(&mut tx, server).await?;
    let operation = fetch_in(&mut tx, id).await?;
    tx.commit().await?;
    Ok(operation)
}

pub async fn inputs_of(pool: &SqlitePool, id: Id) -> Answer<NewOperation> {
    let row: Option<(Id, OperationKind, Option<String>, Option<Id>, Option<String>, Payload)> =
        sqlx::query_as(
            "SELECT server_id, kind, src, target_id, input, payload FROM operations WHERE id = ?",
        )
        .bind(id)
        .fetch_optional(pool)
        .await?;
    let (server_id, kind, src, target_id, input, payload) = row.ok_or_else(operation_not_found)?;
    Ok(NewOperation {
        server_id,
        kind,
        started_by: None,
        src,
        target_id,
        parent_operation_id: None,
        input: input.and_then(|text| serde_json::from_str(&text).ok()),
        expects_payload: payload != Payload::None,
        message: None,
    })
}

async fn server_of(tx: &mut Transaction<'_, Sqlite>, id: Id) -> Answer<Id> {
    let row: Option<(Id,)> = sqlx::query_as("SELECT server_id FROM operations WHERE id = ?")
        .bind(id)
        .fetch_optional(&mut **tx)
        .await?;
    Ok(row.ok_or_else(operation_not_found)?.0)
}

pub async fn recover(pool: &SqlitePool) -> Answer<Vec<Operation>> {
    let open: Vec<(Id, Id, OperationKind, Option<Timestamp>, Option<Id>)> = sqlx::query_as(
        "SELECT id, server_id, kind, applied_at, target_id FROM operations
          WHERE state IN ('queued', 'ongoing') AND kind <> 'server_delete'
          ORDER BY id",
    )
    .fetch_all(pool)
    .await?;

    let mut failed = Vec::new();
    for (id, server, kind, applied_at, target) in open {
        let error = interruption(kind, applied_at.is_some());
        failed.push(settle(pool, id, OperationState::Failed, Some(error)).await?);
        clean_up_after(pool, kind, server, target).await?;
    }
    Ok(failed)
}

fn interruption(kind: OperationKind, applied: bool) -> OperationError {
    match kind {
        OperationKind::BackupRestore => OperationError {
            code: "restore_interrupted".to_owned(),
            message: "the panel stopped while this backup was being restored".to_owned(),
            step: OperationErrorStep::Filesystem,
        },
        OperationKind::Unarchive if applied => OperationError {
            code: "interrupted_while_applying".to_owned(),
            message: "the panel stopped while entries were being moved into place".to_owned(),
            step: OperationErrorStep::Filesystem,
        },
        _ => OperationError {
            code: "panel_restarted".to_owned(),
            message: "the panel restarted while this was running".to_owned(),
            step: OperationErrorStep::Internal,
        },
    }
}

async fn clean_up_after(
    pool: &SqlitePool,
    kind: OperationKind,
    server: Id,
    target: Option<Id>,
) -> Answer<()> {
    use OperationKind::*;
    match kind {
        ServerCreate => {
            sqlx::query("UPDATE servers SET status = ?, flows_intro = 1 WHERE id = ?")
                .bind(ServerStatus::Broken)
                .bind(server)
                .execute(pool)
                .await?;
        }
        InstallLoader | RepairContent | ResetServer | InstallModpack | ChangeGameVersion
        | BackupRestore => {
            sqlx::query("UPDATE servers SET status = ? WHERE id = ?")
                .bind(ServerStatus::Broken)
                .bind(server)
                .execute(pool)
                .await?;
        }
        BackupCreate => {
            if let Some(backup) = target {
                sqlx::query("DELETE FROM backups WHERE id = ?")
                    .bind(backup)
                    .execute(pool)
                    .await?;
            }
        }
        InstallContent | UpdateContent | InstallJava | Unarchive | ServerDelete => {}
    }
    Ok(())
}

pub async fn purge_finished(pool: &SqlitePool, before: Timestamp) -> Answer<u64> {
    let done = sqlx::query(&format!(
        "DELETE FROM operations
          WHERE state IN ('done', 'failed', 'cancelled') AND finished_at < ?
            AND NOT {STANDING_FAILURE}"
    ))
    .bind(before)
    .execute(pool)
    .await?;
    Ok(done.rows_affected())
}

pub async fn sweep_timeouts(
    pool: &SqlitePool,
    stalled_before: Timestamp,
    unpaid_before: Timestamp,
) -> Answer<Vec<Operation>> {
    let stalled: Vec<(Id,)> = sqlx::query_as(
        "SELECT id FROM operations
          WHERE state = 'ongoing' AND coalesce(progressed_at, started_at, created_at) < ?",
    )
    .bind(stalled_before)
    .fetch_all(pool)
    .await?;

    let unpaid: Vec<(Id,)> = sqlx::query_as(
        "SELECT id FROM operations
          WHERE state = 'queued' AND payload = 'expected' AND created_at < ?",
    )
    .bind(unpaid_before)
    .fetch_all(pool)
    .await?;

    let mut ended = Vec::new();
    for (id,) in stalled {
        let error = OperationError {
            code: "timeout".to_owned(),
            message: "no progress for ten minutes".to_owned(),
            step: OperationErrorStep::Internal,
        };
        ended.push(settle(pool, id, OperationState::Failed, Some(error)).await?);
    }
    for (id,) in unpaid {
        let error = OperationError {
            code: "payload_timeout".to_owned(),
            message: "the upload never arrived".to_owned(),
            step: OperationErrorStep::Internal,
        };
        ended.push(settle(pool, id, OperationState::Failed, Some(error)).await?);
    }
    Ok(ended)
}

pub async fn open_ids(pool: &SqlitePool) -> Answer<Vec<(Id, Id, Id)>> {
    let rows: Vec<(Id, Id, Id)> = sqlx::query_as(
        "SELECT operations.id, servers.id, servers.owner_id
           FROM operations JOIN servers ON servers.id = operations.server_id
          WHERE operations.state IN ('queued', 'ongoing')",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

pub async fn all_server_directories(pool: &SqlitePool) -> Answer<Vec<(Id, Id)>> {
    Ok(sqlx::query_as("SELECT id, owner_id FROM servers").fetch_all(pool).await?)
}
