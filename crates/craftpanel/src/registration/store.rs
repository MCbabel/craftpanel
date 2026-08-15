use sqlx::SqlitePool;

use crate::auth::secret;
use crate::model::{Id, Registration, RegistrationState, Timestamp};

#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct Row {
    pub id: Id,
    pub username: String,
    pub email: String,
    pub state: RegistrationState,
    pub token_expires_at: Option<Timestamp>,
    pub token_sent_at: Option<Timestamp>,
    pub tokens_sent: u32,
    pub signup_ip: Option<String>,
    pub verified_at: Option<Timestamp>,
    pub created_at: Timestamp,
}

const COLUMNS: &str = "id, username, email, state, token_expires_at, token_sent_at, \
                       tokens_sent, signup_ip, verified_at, created_at";

impl Row {
    pub fn as_wire(&self) -> Registration {
        Registration {
            id: self.id,
            username: self.username.clone(),
            email: self.email.clone(),
            state: self.state,
            signup_ip: self.signup_ip.clone(),
            created_at: self.created_at,
            verified_at: self.verified_at,
        }
    }
}

pub struct NewApplication<'a> {
    pub username: &'a str,
    pub email: &'a str,
    pub password_hash: String,
    pub signup_ip: Option<String>,
    pub token_hash: String,
    pub token_expires_at: Timestamp,
}

pub async fn insert(pool: &SqlitePool, new: NewApplication<'_>, now: Timestamp) -> sqlx::Result<Id> {
    let id = Id::new();
    sqlx::query(
        "INSERT INTO registrations (id, username, email, password_hash, state, token_hash, \
         token_expires_at, token_sent_at, tokens_sent, signup_ip, created_at, updated_at) \
         VALUES (?, ?, ?, ?, 'email_unverified', ?, ?, ?, 1, ?, ?, ?)",
    )
    .bind(id)
    .bind(new.username)
    .bind(new.email)
    .bind(&new.password_hash)
    .bind(&new.token_hash)
    .bind(new.token_expires_at)
    .bind(now)
    .bind(new.signup_ip.as_deref())
    .bind(now)
    .bind(now)
    .execute(pool)
    .await?;
    Ok(id)
}

pub async fn take_over(
    pool: &SqlitePool,
    id: Id,
    new: NewApplication<'_>,
    now: Timestamp,
) -> sqlx::Result<bool> {
    let written = sqlx::query(
        "UPDATE registrations SET username = ?, password_hash = ?, token_hash = ?, \
         token_expires_at = ?, token_sent_at = ?, tokens_sent = 1, signup_ip = ?, \
         created_at = ?, updated_at = ? \
         WHERE id = ? AND email = ? AND state = 'email_unverified'",
    )
    .bind(new.username)
    .bind(&new.password_hash)
    .bind(&new.token_hash)
    .bind(new.token_expires_at)
    .bind(now)
    .bind(new.signup_ip.as_deref())
    .bind(now)
    .bind(now)
    .bind(id)
    .bind(new.email)
    .execute(pool)
    .await?;
    Ok(written.rows_affected() == 1)
}

pub async fn by_id(pool: &SqlitePool, id: Id) -> sqlx::Result<Option<Row>> {
    sqlx::query_as::<_, Row>(&format!("SELECT {COLUMNS} FROM registrations WHERE id = ?"))
        .bind(id)
        .fetch_optional(pool)
        .await
}

pub async fn by_email(pool: &SqlitePool, email: &str) -> sqlx::Result<Option<Row>> {
    sqlx::query_as::<_, Row>(&format!("SELECT {COLUMNS} FROM registrations WHERE email = ?"))
        .bind(email)
        .fetch_optional(pool)
        .await
}

pub async fn by_token(pool: &SqlitePool, token: &str) -> sqlx::Result<Option<Row>> {
    sqlx::query_as::<_, Row>(&format!("SELECT {COLUMNS} FROM registrations WHERE token_hash = ?"))
        .bind(secret::digest(token))
        .fetch_optional(pool)
        .await
}

pub async fn credentials(
    pool: &SqlitePool,
    username: &str,
) -> sqlx::Result<Option<(String, RegistrationState)>> {
    sqlx::query_as::<_, (String, RegistrationState)>(
        "SELECT password_hash, state FROM registrations WHERE username = ?",
    )
    .bind(username)
    .fetch_optional(pool)
    .await
}

pub async fn password_hash(pool: &SqlitePool, id: Id) -> sqlx::Result<Option<String>> {
    sqlx::query_scalar("SELECT password_hash FROM registrations WHERE id = ?")
        .bind(id)
        .fetch_optional(pool)
        .await
}

pub async fn password_hash_for_token(
    pool: &SqlitePool,
    id: Id,
    token: &str,
) -> sqlx::Result<Option<String>> {
    sqlx::query_scalar("SELECT password_hash FROM registrations WHERE id = ? AND token_hash = ?")
        .bind(id)
        .bind(secret::digest(token))
        .fetch_optional(pool)
        .await
}

pub async fn replace_token(
    pool: &SqlitePool,
    id: Id,
    token_hash: &str,
    expires_at: Timestamp,
    now: Timestamp,
) -> sqlx::Result<Option<String>> {
    sqlx::query_scalar(
        "UPDATE registrations SET token_hash = ?, token_expires_at = ?, token_sent_at = ?, \
         tokens_sent = tokens_sent + 1, updated_at = ? WHERE id = ? RETURNING username",
    )
    .bind(token_hash)
    .bind(expires_at)
    .bind(now)
    .bind(now)
    .bind(id)
    .fetch_optional(pool)
    .await
}

pub async fn mark_verified(
    pool: &SqlitePool,
    id: Id,
    token: &str,
    now: Timestamp,
) -> sqlx::Result<bool> {
    let moved = sqlx::query(
        "UPDATE registrations SET state = 'awaiting_approval', verified_at = ?, updated_at = ? \
         WHERE id = ? AND token_hash = ? AND state = 'email_unverified'",
    )
    .bind(now)
    .bind(now)
    .bind(id)
    .bind(secret::digest(token))
    .execute(pool)
    .await?;
    Ok(moved.rows_affected() == 1)
}

pub async fn remove(pool: &SqlitePool, id: Id) -> sqlx::Result<()> {
    sqlx::query("DELETE FROM registrations WHERE id = ?").bind(id).execute(pool).await?;
    Ok(())
}

pub async fn page(pool: &SqlitePool, limit: u32, offset: u32) -> sqlx::Result<(Vec<Row>, u32)> {
    let rows = sqlx::query_as::<_, Row>(&format!(
        "SELECT {COLUMNS} FROM registrations ORDER BY created_at DESC, id DESC LIMIT ? OFFSET ?"
    ))
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await?;

    let total: i64 =
        sqlx::query_scalar("SELECT count(*) FROM registrations").fetch_one(pool).await?;
    Ok((rows, total.max(0) as u32))
}

pub async fn block(
    pool: &SqlitePool,
    email: &str,
    until: Option<Timestamp>,
    reason: Option<&str>,
    now: Timestamp,
) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO registration_blocks (email, until, reason, created_at) VALUES (?, ?, ?, ?) \
         ON CONFLICT(email) DO UPDATE SET until = excluded.until, reason = excluded.reason",
    )
    .bind(email)
    .bind(until)
    .bind(reason)
    .bind(now)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn is_blocked(pool: &SqlitePool, email: &str, now: Timestamp) -> sqlx::Result<bool> {
    let found: Option<Option<Timestamp>> =
        sqlx::query_scalar("SELECT until FROM registration_blocks WHERE email = ?")
            .bind(email)
            .fetch_optional(pool)
            .await?;
    Ok(match found {
        Some(None) => true,
        Some(Some(until)) => until > now,
        None => false,
    })
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Swept {
    pub unverified: u64,
    pub waiting: u64,
    pub blocks: u64,
}

impl Swept {
    pub fn any(self) -> bool {
        self.unverified > 0 || self.waiting > 0 || self.blocks > 0
    }
}

pub async fn sweep(
    pool: &SqlitePool,
    unverified_before: Timestamp,
    waiting_before: Timestamp,
    now: Timestamp,
) -> sqlx::Result<Swept> {
    let unverified = sqlx::query(
        "DELETE FROM registrations WHERE state = 'email_unverified' AND created_at < ?",
    )
    .bind(unverified_before)
    .execute(pool)
    .await?
    .rows_affected();

    let waiting = sqlx::query(
        "DELETE FROM registrations WHERE state = 'awaiting_approval' AND created_at < ?",
    )
    .bind(waiting_before)
    .execute(pool)
    .await?
    .rows_affected();

    let blocks =
        sqlx::query("DELETE FROM registration_blocks WHERE until IS NOT NULL AND until <= ?")
            .bind(now)
            .execute(pool)
            .await?
            .rows_affected();

    Ok(Swept { unverified, waiting, blocks })
}
