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
    pub size_bytes: i64,
}

pub async fn backups_of(pool: &SqlitePool, user: Id) -> Result<Vec<DriveBackup>> {
    Ok(sqlx::query_as(
        "SELECT b.id, b.server_id, b.drive_file_id, b.drive_state, b.size_bytes \
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
