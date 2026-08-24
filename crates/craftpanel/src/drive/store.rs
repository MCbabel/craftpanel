use sqlx::SqlitePool;

use crate::auth::error::Result;
use crate::model::{
    BackupLocation, BackupTargetPolicy, DriveAccountState, DriveFileState, DriveLinkState, Id,
    Timestamp,
};

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Settings {
    pub client_id: Option<String>,
    pub target_policy: BackupTargetPolicy,
    pub folder_name: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            client_id: None,
            target_policy: BackupTargetPolicy::UserChoice,
            folder_name: "craftpanel-backups".to_owned(),
        }
    }
}

pub async fn settings(pool: &SqlitePool) -> Result<Settings> {
    Ok(sqlx::query_as("SELECT client_id, target_policy, folder_name FROM drive_settings WHERE id = 1")
        .fetch_optional(pool)
        .await?
        .unwrap_or_default())
}

pub async fn save_settings(
    pool: &SqlitePool,
    client_id: Option<&str>,
    policy: BackupTargetPolicy,
    folder_name: &str,
    now: Timestamp,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO drive_settings (id, client_id, target_policy, folder_name, updated_at) \
         VALUES (1, ?, ?, ?, ?) \
         ON CONFLICT(id) DO UPDATE SET client_id = excluded.client_id, \
             target_policy = excluded.target_policy, folder_name = excluded.folder_name, \
             updated_at = excluded.updated_at",
    )
    .bind(client_id)
    .bind(policy)
    .bind(folder_name)
    .bind(now)
    .execute(pool)
    .await?;
    Ok(())
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Account {
    pub google_name: Option<String>,
    pub google_email: Option<String>,
    pub folder_id: Option<String>,
    pub state: Option<DriveAccountState>,
    pub storage_limit_bytes: Option<i64>,
    pub storage_usage_bytes: Option<i64>,
    pub link_user_code: Option<String>,
    pub link_state: Option<DriveLinkState>,
    pub link_started_at: Option<Timestamp>,
    pub link_expires_at: Option<Timestamp>,
    pub checked_at: Option<Timestamp>,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Link {
    pub user_code: String,
    pub state: DriveLinkState,
    pub started_at: Timestamp,
    pub expires_at: Timestamp,
}

impl Account {
    pub fn link(&self) -> Option<Link> {
        Some(Link {
            user_code: self.link_user_code.clone()?,
            state: self.link_state?,
            started_at: self.link_started_at?,
            expires_at: self.link_expires_at.unwrap_or(self.link_started_at?),
        })
    }
}

const COLUMNS: &str = "google_name, google_email, folder_id, state, storage_limit_bytes, \
     storage_usage_bytes, link_user_code, link_state, link_started_at, link_expires_at, \
     checked_at, last_error";

pub async fn account(pool: &SqlitePool, user: Id) -> Result<Option<Account>> {
    Ok(sqlx::query_as(&format!("SELECT {COLUMNS} FROM drive_accounts WHERE user_id = ?"))
        .bind(user)
        .fetch_optional(pool)
        .await?)
}

pub async fn connected(pool: &SqlitePool) -> Result<Vec<Id>> {
    Ok(sqlx::query_scalar("SELECT user_id FROM drive_accounts ORDER BY user_id")
        .fetch_all(pool)
        .await?)
}

pub async fn begin_link(pool: &SqlitePool, user: Id, link: &Link, now: Timestamp) -> Result<()> {
    sqlx::query(
        "INSERT INTO drive_accounts \
             (user_id, link_user_code, link_state, link_started_at, link_expires_at, \
              updated_at) \
         VALUES (?, ?, ?, ?, ?, ?) \
         ON CONFLICT(user_id) DO UPDATE SET link_user_code = excluded.link_user_code, \
             link_state = excluded.link_state, link_started_at = excluded.link_started_at, \
             link_expires_at = excluded.link_expires_at, last_error = NULL, \
             updated_at = excluded.updated_at",
    )
    .bind(user)
    .bind(&link.user_code)
    .bind(link.state)
    .bind(link.started_at)
    .bind(link.expires_at)
    .bind(now)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn advance_link(
    pool: &SqlitePool,
    user: Id,
    code: &str,
    state: DriveLinkState,
    why: &str,
    now: Timestamp,
) -> Result<()> {
    sqlx::query(
        "UPDATE drive_accounts SET link_state = ?, last_error = ?, updated_at = ? \
          WHERE user_id = ? AND link_user_code = ?",
    )
    .bind(state)
    .bind(why)
    .bind(now)
    .bind(user)
    .bind(code)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn clear_link(pool: &SqlitePool, user: Id, now: Timestamp) -> Result<bool> {
    let done = sqlx::query(
        "UPDATE drive_accounts SET link_user_code = NULL, link_state = NULL, \
             link_started_at = NULL, link_expires_at = NULL, updated_at = ? \
          WHERE user_id = ? AND link_user_code IS NOT NULL",
    )
    .bind(now)
    .bind(user)
    .execute(pool)
    .await?;
    Ok(done.rows_affected() > 0)
}

pub async fn connect(pool: &SqlitePool, user: Id, folder: Option<&str>, now: Timestamp) -> Result<()> {
    sqlx::query(
        "INSERT INTO drive_accounts (user_id, folder_id, state, updated_at) \
         VALUES (?, ?, 'connected', ?) \
         ON CONFLICT(user_id) DO UPDATE SET folder_id = coalesce(excluded.folder_id, folder_id), \
             state = 'connected', last_error = NULL, link_user_code = NULL, link_state = NULL, \
             link_started_at = NULL, link_expires_at = NULL, updated_at = excluded.updated_at",
    )
    .bind(user)
    .bind(folder)
    .bind(now)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn set_folder(pool: &SqlitePool, user: Id, folder: &str, now: Timestamp) -> Result<()> {
    sqlx::query("UPDATE drive_accounts SET folder_id = ?, updated_at = ? WHERE user_id = ?")
        .bind(folder)
        .bind(now)
        .bind(user)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn record_check(
    pool: &SqlitePool,
    user: Id,
    who: &Who,
    now: Timestamp,
) -> Result<()> {
    sqlx::query(
        "UPDATE drive_accounts SET google_name = ?, google_email = ?, storage_limit_bytes = ?, \
             storage_usage_bytes = ?, state = 'connected', last_error = NULL, checked_at = ?, \
             updated_at = ? \
          WHERE user_id = ?",
    )
    .bind(&who.name)
    .bind(&who.email)
    .bind(who.limit_bytes.map(|bytes| bytes as i64))
    .bind(who.usage_bytes.map(|bytes| bytes as i64))
    .bind(now)
    .bind(now)
    .bind(user)
    .execute(pool)
    .await?;
    Ok(())
}

#[derive(Debug, Clone, Default)]
pub struct Who {
    pub name: Option<String>,
    pub email: Option<String>,
    pub limit_bytes: Option<u64>,
    pub usage_bytes: Option<u64>,
}

pub async fn record_error(
    pool: &SqlitePool,
    user: Id,
    state: DriveAccountState,
    why: &str,
    now: Timestamp,
) -> Result<()> {
    sqlx::query(
        "UPDATE drive_accounts SET state = ?, last_error = ?, checked_at = ?, updated_at = ? \
          WHERE user_id = ?",
    )
    .bind(state)
    .bind(why)
    .bind(now)
    .bind(now)
    .bind(user)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn forget_account(pool: &SqlitePool, user: Id) -> Result<()> {
    sqlx::query("DELETE FROM drive_accounts WHERE user_id = ?")
        .bind(user)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn target(pool: &SqlitePool, server: Id) -> Result<BackupLocation> {
    Ok(sqlx::query_scalar("SELECT target FROM backup_targets WHERE server_id = ?")
        .bind(server)
        .fetch_optional(pool)
        .await?
        .unwrap_or(BackupLocation::Local))
}

pub async fn set_target(
    pool: &SqlitePool,
    server: Id,
    target: BackupLocation,
    now: Timestamp,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO backup_targets (server_id, target, updated_at) VALUES (?, ?, ?) \
         ON CONFLICT(server_id) DO UPDATE SET target = excluded.target, \
             updated_at = excluded.updated_at",
    )
    .bind(server)
    .bind(target)
    .bind(now)
    .execute(pool)
    .await?;
    Ok(())
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct DriveBackup {
    pub id: Id,
    pub server_id: Id,
    pub drive_file_id: Option<String>,
    pub drive_state: Option<DriveFileState>,
    pub drive_md5: Option<String>,
    pub drive_content_changed_at: Option<Timestamp>,
    pub size_bytes: i64,
}

pub async fn backups_of(pool: &SqlitePool, user: Id) -> Result<Vec<DriveBackup>> {
    Ok(sqlx::query_as(
        "SELECT b.id, b.server_id, b.drive_file_id, b.drive_state, b.drive_md5, \
                b.drive_content_changed_at, b.size_bytes \
           FROM backups b JOIN servers s ON s.id = b.server_id \
          WHERE s.owner_id = ? AND b.location = 'drive' \
          ORDER BY b.created_at DESC",
    )
    .bind(user)
    .fetch_all(pool)
    .await?)
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Usage {
    pub user_id: Id,
    pub backups: i64,
    pub backup_bytes: i64,
}

pub async fn usage(pool: &SqlitePool) -> Result<Vec<Usage>> {
    Ok(sqlx::query_as(
        "SELECT s.owner_id AS user_id, count(*) AS backups, \
                coalesce(sum(b.size_bytes), 0) AS backup_bytes \
           FROM backups b JOIN servers s ON s.id = b.server_id \
          WHERE b.location = 'drive' GROUP BY s.owner_id",
    )
    .fetch_all(pool)
    .await?)
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Overview {
    pub user_id: Id,
    pub username: String,
    pub state: Option<DriveAccountState>,
    pub google_email: Option<String>,
    pub storage_limit_bytes: Option<i64>,
    pub storage_usage_bytes: Option<i64>,
    pub last_error: Option<String>,
    pub checked_at: Option<Timestamp>,
}

pub async fn overview(pool: &SqlitePool) -> Result<Vec<Overview>> {
    Ok(sqlx::query_as(
        "SELECT a.user_id, u.username, a.state, a.google_email, a.storage_limit_bytes, \
                a.storage_usage_bytes, a.last_error, a.checked_at \
           FROM drive_accounts a JOIN users u ON u.id = a.user_id \
          ORDER BY u.username COLLATE NOCASE",
    )
    .fetch_all(pool)
    .await?)
}

pub async fn owner_of(pool: &SqlitePool, server: Id) -> Result<Option<Id>> {
    Ok(sqlx::query_scalar("SELECT owner_id FROM servers WHERE id = ?")
        .bind(server)
        .fetch_optional(pool)
        .await?)
}

pub async fn set_file_state(
    pool: &SqlitePool,
    backup: Id,
    state: DriveFileState,
    now: Timestamp,
) -> Result<()> {
    sqlx::query("UPDATE backups SET drive_state = ?, drive_checked_at = ? WHERE id = ?")
        .bind(state)
        .bind(now)
        .bind(backup)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn set_content_changed(
    pool: &SqlitePool,
    backup: Id,
    when: Option<Timestamp>,
) -> Result<()> {
    sqlx::query("UPDATE backups SET drive_content_changed_at = ? WHERE id = ?")
        .bind(when)
        .bind(backup)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn note_content_changed(pool: &SqlitePool, file: &str, now: Timestamp) -> Result<()> {
    sqlx::query(
        "UPDATE backups SET drive_content_changed_at = ? \
          WHERE drive_file_id = ? AND drive_content_changed_at IS NULL",
    )
    .bind(now)
    .bind(file)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn forget_drive_backups(pool: &SqlitePool, user: Id) -> Result<()> {
    sqlx::query(
        "DELETE FROM backups WHERE location = 'drive' \
           AND server_id IN (SELECT id FROM servers WHERE owner_id = ?)",
    )
    .bind(user)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn mark_unreachable(pool: &SqlitePool, user: Id, now: Timestamp) -> Result<()> {
    sqlx::query(
        "UPDATE backups SET drive_state = 'unreachable', drive_checked_at = ? \
          WHERE location = 'drive' \
            AND server_id IN (SELECT id FROM servers WHERE owner_id = ?)",
    )
    .bind(now)
    .bind(user)
    .execute(pool)
    .await?;
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Print {
    pub bytes: u64,
    pub mtime_ns: i64,
    pub inode: u64,
}

pub async fn print_of(path: &std::path::Path) -> Option<Print> {
    use std::os::unix::fs::MetadataExt;

    let seen = tokio::fs::metadata(path).await.ok()?;
    if !seen.is_file() {
        return None;
    }
    Some(Print {
        bytes: seen.len(),
        mtime_ns: seen.mtime().saturating_mul(1_000_000_000).saturating_add(seen.mtime_nsec()),
        inode: seen.ino(),
    })
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Upload {
    pub backup_id: Id,
    pub user_id: Id,
    pub total_bytes: i64,
    pub archive_mtime_ns: i64,
    pub archive_inode: i64,
    pub offered_bytes: i64,
    pub offered_sha256: Option<String>,
    pub opened_at: Timestamp,
}

impl Upload {
    pub fn print(&self) -> Print {
        Print {
            bytes: self.total_bytes.max(0) as u64,
            mtime_ns: self.archive_mtime_ns,
            inode: self.archive_inode.max(0) as u64,
        }
    }

    pub fn offer(&self) -> Option<(u64, &str)> {
        Some((self.offered_bytes.max(0) as u64, self.offered_sha256.as_deref()?))
    }
}

const UPLOAD_COLUMNS: &str = "backup_id, user_id, total_bytes, archive_mtime_ns, archive_inode, \
     offered_bytes, offered_sha256, opened_at";

pub async fn open_upload(
    pool: &SqlitePool,
    backup: Id,
    user: Id,
    print: Print,
    opened: Timestamp,
    now: Timestamp,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO drive_uploads \
             (backup_id, user_id, total_bytes, archive_mtime_ns, archive_inode, opened_at, \
              updated_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?) \
         ON CONFLICT(backup_id) DO UPDATE SET user_id = excluded.user_id, \
             total_bytes = excluded.total_bytes, \
             archive_mtime_ns = excluded.archive_mtime_ns, \
             archive_inode = excluded.archive_inode, offered_bytes = 0, \
             offered_sha256 = NULL, opened_at = excluded.opened_at, \
             updated_at = excluded.updated_at",
    )
    .bind(backup)
    .bind(user)
    .bind(print.bytes as i64)
    .bind(print.mtime_ns)
    .bind(print.inode as i64)
    .bind(opened)
    .bind(now)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn upload_of(pool: &SqlitePool, backup: Id) -> Result<Option<Upload>> {
    Ok(
        sqlx::query_as(&format!("SELECT {UPLOAD_COLUMNS} FROM drive_uploads WHERE backup_id = ?"))
            .bind(backup)
            .fetch_optional(pool)
            .await?,
    )
}

pub async fn note_offer(
    pool: &SqlitePool,
    backup: Id,
    offered: u64,
    proof: &str,
    now: Timestamp,
) -> Result<()> {
    sqlx::query(
        "UPDATE drive_uploads SET offered_bytes = ?, offered_sha256 = ?, updated_at = ? \
          WHERE backup_id = ?",
    )
    .bind(offered.min(i64::MAX as u64) as i64)
    .bind(proof)
    .bind(now)
    .bind(backup)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn forget_upload(pool: &SqlitePool, backup: Id) -> Result<()> {
    sqlx::query("DELETE FROM drive_uploads WHERE backup_id = ?")
        .bind(backup)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn forget_uploads_of(pool: &SqlitePool, user: Id) -> Result<Vec<Id>> {
    let held: Vec<Id> = sqlx::query_scalar("SELECT backup_id FROM drive_uploads WHERE user_id = ?")
        .bind(user)
        .fetch_all(pool)
        .await?;
    sqlx::query("DELETE FROM drive_uploads WHERE user_id = ?")
        .bind(user)
        .execute(pool)
        .await?;
    Ok(held)
}

pub async fn uploads(pool: &SqlitePool) -> Result<Vec<Upload>> {
    Ok(sqlx::query_as(&format!(
        "SELECT {UPLOAD_COLUMNS} FROM drive_uploads ORDER BY opened_at"
    ))
    .fetch_all(pool)
    .await?)
}

pub async fn sent_today(pool: &SqlitePool, user: Id, day: &str) -> Result<u64> {
    let bytes: Option<i64> =
        sqlx::query_scalar("SELECT bytes FROM drive_daily_uploads WHERE user_id = ? AND day = ?")
            .bind(user)
            .bind(day)
            .fetch_optional(pool)
            .await?;
    Ok(bytes.unwrap_or(0).max(0) as u64)
}

pub async fn note_sent(
    pool: &SqlitePool,
    user: Id,
    day: &str,
    bytes: u64,
    now: Timestamp,
) -> Result<()> {
    if bytes == 0 {
        return Ok(());
    }
    sqlx::query(
        "INSERT INTO drive_daily_uploads (user_id, day, bytes, updated_at) VALUES (?, ?, ?, ?) \
         ON CONFLICT(user_id, day) DO UPDATE SET bytes = bytes + excluded.bytes, \
             updated_at = excluded.updated_at",
    )
    .bind(user)
    .bind(day)
    .bind(bytes.min(i64::MAX as u64) as i64)
    .bind(now)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn forget_days_before(pool: &SqlitePool, user: Id, day: &str) -> Result<()> {
    sqlx::query("DELETE FROM drive_daily_uploads WHERE user_id = ? AND day < ?")
        .bind(user)
        .bind(day)
        .execute(pool)
        .await?;
    Ok(())
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Today {
    pub user_id: Id,
    pub bytes: i64,
}

pub async fn sent_today_by_everybody(pool: &SqlitePool, day: &str) -> Result<Vec<Today>> {
    Ok(
        sqlx::query_as("SELECT user_id, bytes FROM drive_daily_uploads WHERE day = ?")
            .bind(day)
            .fetch_all(pool)
            .await?,
    )
}

pub async fn note_holdup(pool: &SqlitePool, user: Id, why: &str, now: Timestamp) -> Result<()> {
    sqlx::query("UPDATE drive_accounts SET last_error = ?, updated_at = ? WHERE user_id = ?")
        .bind(why)
        .bind(now)
        .bind(user)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn uploads_opened_before(pool: &SqlitePool, moment: Timestamp) -> Result<Vec<Upload>> {
    Ok(sqlx::query_as(&format!(
        "SELECT {UPLOAD_COLUMNS} FROM drive_uploads WHERE opened_at < ? ORDER BY opened_at"
    ))
    .bind(moment)
    .fetch_all(pool)
    .await?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::PanelRole;
    use crate::ops::testing::{a_server, a_user, schema};

    #[tokio::test]
    async fn an_untouched_panel_reads_as_not_set_up_and_user_choice() {
        let pool = schema().await;

        let settings = settings(&pool).await.expect("the seed row");
        assert_eq!(settings.client_id, None, "no client id is the normal state");
        assert_eq!(settings.target_policy, BackupTargetPolicy::UserChoice);
        assert_eq!(settings.folder_name, "craftpanel-backups");

        let anna = a_user(&pool, PanelRole::User).await;
        assert!(account(&pool, anna).await.expect("no error").is_none(), "nobody has connected");
        assert!(connected(&pool).await.expect("no error").is_empty());
    }

    #[tokio::test]
    async fn a_server_without_a_row_backs_up_locally() {
        let pool = schema().await;
        let anna = a_user(&pool, PanelRole::User).await;
        let server = a_server(&pool, anna).await;

        assert_eq!(target(&pool, server).await.expect("no error"), BackupLocation::Local);

        set_target(&pool, server, BackupLocation::Drive, Timestamp::now()).await.expect("a target");
        assert_eq!(target(&pool, server).await.expect("no error"), BackupLocation::Drive);

        set_target(&pool, server, BackupLocation::Local, Timestamp::now()).await.expect("again");
        assert_eq!(target(&pool, server).await.expect("no error"), BackupLocation::Local);
    }

    #[tokio::test]
    async fn an_attempt_is_written_and_cleared_as_a_whole() {
        let pool = schema().await;
        let anna = a_user(&pool, PanelRole::User).await;
        let now = Timestamp::now();

        let link = Link {
            user_code: "GQVQ-JKEC".to_owned(),
            state: DriveLinkState::Waiting,
            started_at: now,
            expires_at: Timestamp::at(now.as_datetime() + time::Duration::seconds(1800)),
        };
        begin_link(&pool, anna, &link, now).await.expect("an attempt");

        let row = account(&pool, anna).await.expect("no error").expect("a row");
        let seen = row.link().expect("the attempt");
        assert_eq!(seen.user_code, "GQVQ-JKEC");
        assert_eq!(seen.state, DriveLinkState::Waiting);
        assert_eq!(row.state, None, "an account that is connecting has no connection state");
        assert_eq!(row.last_error, None, "and nothing to complain about either");

        advance_link(&pool, anna, "GQVQ-JKEC", DriveLinkState::Denied, "Google said no", now)
            .await
            .expect("advancing");
        let row = account(&pool, anna).await.expect("no error").expect("a row");
        assert_eq!(row.link().expect("the attempt").state, DriveLinkState::Denied);
        assert_eq!(row.last_error.as_deref(), Some("Google said no"), "and it says why");
        assert_eq!(row.state, None, "a failed attempt is still not a broken connection");

        assert!(clear_link(&pool, anna, now).await.expect("clearing"));
        let row = account(&pool, anna).await.expect("no error").expect("a row");
        assert!(row.link().is_none(), "all four columns go together");
        assert!(!clear_link(&pool, anna, now).await.expect("no error"), "and only once");
    }

    #[tokio::test]
    async fn connecting_clears_the_attempt() {
        let pool = schema().await;
        let anna = a_user(&pool, PanelRole::User).await;
        let now = Timestamp::now();

        let link = Link {
            user_code: "AAAA-BBBB".to_owned(),
            state: DriveLinkState::Waiting,
            started_at: now,
            expires_at: now,
        };
        begin_link(&pool, anna, &link, now).await.expect("an attempt");
        connect(&pool, anna, Some("folder-1"), now).await.expect("connecting");

        let row = account(&pool, anna).await.expect("no error").expect("a row");
        assert_eq!(row.state, Some(DriveAccountState::Connected));
        assert_eq!(row.folder_id.as_deref(), Some("folder-1"));
        assert!(row.link().is_none(), "the attempt is over");

        connect(&pool, anna, None, now).await.expect("connecting again");
        let row = account(&pool, anna).await.expect("no error").expect("a row");
        assert_eq!(row.folder_id.as_deref(), Some("folder-1"));
    }

    #[tokio::test]
    async fn a_check_that_worked_wipes_the_last_complaint() {
        let pool = schema().await;
        let anna = a_user(&pool, PanelRole::User).await;
        let now = Timestamp::now();
        connect(&pool, anna, None, now).await.expect("connecting");

        record_error(&pool, anna, DriveAccountState::Revoked, "Google said no", now)
            .await
            .expect("an error");
        let row = account(&pool, anna).await.expect("no error").expect("a row");
        assert_eq!(row.state, Some(DriveAccountState::Revoked));
        assert_eq!(row.last_error.as_deref(), Some("Google said no"));

        record_check(
            &pool,
            anna,
            &Who {
                name: Some("Anna".to_owned()),
                email: Some("anna@example.com".to_owned()),
                limit_bytes: Some(16_106_127_360),
                usage_bytes: Some(2_147_483_648),
            },
            now,
        )
        .await
        .expect("a check");

        let row = account(&pool, anna).await.expect("no error").expect("a row");
        assert_eq!(row.state, Some(DriveAccountState::Connected));
        assert_eq!(row.last_error, None, "a working check is what clears it");
        assert_eq!(row.google_email.as_deref(), Some("anna@example.com"));
        assert_eq!(row.storage_limit_bytes, Some(16_106_127_360));
    }

    #[tokio::test]
    async fn a_real_complaint_always_carries_a_sentence_and_the_moment_it_happened() {
        let pool = schema().await;
        let anna = a_user(&pool, PanelRole::User).await;
        let now = Timestamp::now();
        connect(&pool, anna, None, now).await.expect("connecting");

        record_error(&pool, anna, DriveAccountState::Error, "Google did not answer", now)
            .await
            .expect("an error");

        let row = account(&pool, anna).await.expect("no error").expect("a row");
        assert_eq!(row.state, Some(DriveAccountState::Error));
        assert!(row.last_error.is_some(), "0013 reads a missing sentence as 'never connected'");
        assert!(row.checked_at.is_some(), "and a missing moment as the same thing");
    }

    #[tokio::test]
    async fn an_upload_session_is_remembered_by_the_archive_it_belongs_to() {
        let pool = schema().await;
        let anna = a_user(&pool, PanelRole::User).await;
        let server = a_server(&pool, anna).await;
        let now = Timestamp::now();
        let backup =
            crate::backups::store::insert(&pool, server, "Monday", false, BackupLocation::Drive)
                .await
                .expect("a backup")
                .id;

        assert!(upload_of(&pool, backup).await.expect("no error").is_none(), "nothing is under way");

        let print = Print { bytes: 2_147_483_648, mtime_ns: 1_755_000_000_123_456_789, inode: 4711 };
        open_upload(&pool, backup, anna, print, now, now).await.expect("a session");

        let row = upload_of(&pool, backup).await.expect("no error").expect("a row");
        assert_eq!(row.user_id, anna, "the session says whose Drive it writes into");
        assert_eq!(row.total_bytes, 2_147_483_648, "and the size Google was promised");
        assert_eq!(row.print(), print, "and the archive it was opened for, to the nanosecond");
        assert_eq!(row.opened_at, now, "and when Google's week began");

        forget_upload(&pool, backup).await.expect("letting go");
        assert!(upload_of(&pool, backup).await.expect("no error").is_none());
    }

    #[tokio::test]
    async fn what_was_offered_to_google_is_written_down_and_a_new_session_forgets_it() {
        let pool = schema().await;
        let anna = a_user(&pool, PanelRole::User).await;
        let server = a_server(&pool, anna).await;
        let now = Timestamp::now();
        let backup =
            crate::backups::store::insert(&pool, server, "Monday", false, BackupLocation::Drive)
                .await
                .expect("a backup")
                .id;
        let print = Print { bytes: 24, mtime_ns: 5, inode: 9 };
        open_upload(&pool, backup, anna, print, now, now).await.expect("a session");

        let row = upload_of(&pool, backup).await.expect("no error").expect("a row");
        assert_eq!(row.offer(), None, "a session nothing has gone into proves nothing");

        note_offer(&pool, backup, 16, "beef", now).await.expect("the mark");
        let row = upload_of(&pool, backup).await.expect("no error").expect("a row");
        assert_eq!(
            row.offer(),
            Some((16, "beef")),
            "the mark says how far this session was fed and what the archive hashed to there"
        );

        open_upload(&pool, backup, anna, print, now, now).await.expect("a second session");
        let row = upload_of(&pool, backup).await.expect("no error").expect("a row");
        assert_eq!(
            row.offer(),
            None,
            "a mark left over from the session before could vouch for the wrong archive"
        );
    }

    #[tokio::test]
    async fn a_session_cannot_outlive_the_backup_it_was_opened_for() {
        let pool = schema().await;
        let anna = a_user(&pool, PanelRole::User).await;
        let server = a_server(&pool, anna).await;
        let now = Timestamp::now();
        let backup =
            crate::backups::store::insert(&pool, server, "Monday", false, BackupLocation::Drive)
                .await
                .expect("a backup")
                .id;
        let print = Print { bytes: 17, mtime_ns: 5, inode: 9 };
        open_upload(&pool, backup, anna, print, now, now).await.expect("a session");

        sqlx::query("DELETE FROM backups WHERE id = ?")
            .bind(backup)
            .execute(&pool)
            .await
            .expect("deleting the backup");

        assert!(
            upload_of(&pool, backup).await.expect("no error").is_none(),
            "a session that points at a backup nobody has any more is a lie"
        );
    }

    #[tokio::test]
    async fn only_the_sessions_past_googles_week_are_swept() {
        let pool = schema().await;
        let anna = a_user(&pool, PanelRole::User).await;
        let server = a_server(&pool, anna).await;
        let now = Timestamp::now();
        let print = Print { bytes: 17, mtime_ns: 5, inode: 9 };

        let mut made = Vec::new();
        for (name, age) in [("old", 8), ("young", 1)] {
            let backup =
                crate::backups::store::insert(&pool, server, name, false, BackupLocation::Drive)
                    .await
                    .expect("a backup")
                    .id;
            let opened = Timestamp::at(now.as_datetime() - time::Duration::days(age));
            open_upload(&pool, backup, anna, print, opened, now).await.expect("a session");
            made.push(backup);
        }

        let cutoff = Timestamp::at(now.as_datetime() - time::Duration::days(6));
        let stale = uploads_opened_before(&pool, cutoff).await.expect("the old ones");
        assert_eq!(stale.len(), 1, "only one of the two is past the week");
        assert_eq!(stale[0].backup_id, made[0]);
        assert_eq!(uploads(&pool).await.expect("all of them").len(), 2);

        let let_go = forget_uploads_of(&pool, anna).await.expect("letting go of hers");
        assert_eq!(let_go.len(), 2, "the caller is told which addresses to wipe off the disk");
        assert!(uploads(&pool).await.expect("no error").is_empty());
    }

    #[tokio::test]
    async fn the_days_bytes_add_up_and_yesterdays_are_swept_off() {
        let pool = schema().await;
        let anna = a_user(&pool, PanelRole::User).await;
        let bert = a_user(&pool, PanelRole::User).await;
        let now = Timestamp::now();
        connect(&pool, anna, None, now).await.expect("a connected account");
        connect(&pool, bert, None, now).await.expect("a connected account");

        assert_eq!(
            sent_today(&pool, anna, "2026-08-15").await.expect("no row"),
            0,
            "an account that has sent nothing today reads as nothing, not as an error"
        );

        note_sent(&pool, anna, "2026-08-15", 1_000, now).await.expect("a first archive");
        note_sent(&pool, anna, "2026-08-15", 2_500, now).await.expect("a second one");
        note_sent(&pool, anna, "2026-08-14", 9_000, now).await.expect("yesterday");
        note_sent(&pool, bert, "2026-08-15", 7, now).await.expect("somebody else");
        note_sent(&pool, anna, "2026-08-15", 0, now).await.expect("a run that sent nothing");

        assert_eq!(sent_today(&pool, anna, "2026-08-15").await.expect("today"), 3_500);
        assert_eq!(
            sent_today(&pool, anna, "2026-08-14").await.expect("yesterday"),
            9_000,
            "one day was added to another"
        );
        assert_eq!(
            sent_today(&pool, bert, "2026-08-15").await.expect("his own day"),
            7,
            "one account was charged for another's upload"
        );

        let everybody = sent_today_by_everybody(&pool, "2026-08-15").await.expect("the day");
        assert_eq!(everybody.len(), 2);

        forget_days_before(&pool, anna, "2026-08-15").await.expect("the sweep");
        assert_eq!(sent_today(&pool, anna, "2026-08-14").await.expect("swept"), 0);
        assert_eq!(sent_today(&pool, anna, "2026-08-15").await.expect("today"), 3_500);
    }

    #[tokio::test]
    async fn a_holdup_is_a_sentence_and_not_a_broken_connection() {
        let pool = schema().await;
        let anna = a_user(&pool, PanelRole::User).await;
        let now = Timestamp::now();
        connect(&pool, anna, None, now).await.expect("a connected account");

        note_holdup(&pool, anna, "Google takes nothing more today", now).await.expect("a note");

        let row = account(&pool, anna).await.expect("no error").expect("the row");
        assert_eq!(row.last_error.as_deref(), Some("Google takes nothing more today"));
        assert_eq!(
            row.state,
            Some(DriveAccountState::Connected),
            "a Google that is throttling is not a connection anybody has to make again"
        );
    }

    #[tokio::test]
    async fn the_overview_carries_no_user_code_at_all() {
        let pool = schema().await;
        let anna = a_user(&pool, PanelRole::User).await;
        let now = Timestamp::now();

        begin_link(
            &pool,
            anna,
            &Link {
                user_code: "SECRET-CODE".to_owned(),
                state: DriveLinkState::Waiting,
                started_at: now,
                expires_at: now,
            },
            now,
        )
        .await
        .expect("an attempt");
        connect(&pool, anna, None, now).await.expect("connecting");

        let lines = overview(&pool).await.expect("the overview");
        assert_eq!(lines.len(), 1);
        let rendered = format!("{:?}", lines[0]);
        assert!(
            !rendered.contains("SECRET-CODE"),
            "an admin line must not carry a way into somebody's Google account: {rendered}"
        );
    }
}
