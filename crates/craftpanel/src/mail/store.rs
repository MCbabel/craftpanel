use std::str::FromStr;

use serde::Serialize;
use sqlx::SqlitePool;

use super::message::Kind;
use crate::model::{Id, Timestamp};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    Queued,
    Sending,
    Sent,
    Failed,
}

impl State {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Sending => "sending",
            Self::Sent => "sent",
            Self::Failed => "failed",
        }
    }
}

impl FromStr for State {
    type Err = ();

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        match text {
            "queued" => Ok(Self::Queued),
            "sending" => Ok(Self::Sending),
            "sent" => Ok(Self::Sent),
            "failed" => Ok(Self::Failed),
            _ => Err(()),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Settings {
    pub from_address: String,
    pub from_name: String,
    pub reply_to: Option<String>,
    pub link_base: Option<String>,
    pub daily_limit: u32,
    pub key_set_at: Option<Timestamp>,
    pub last_test_at: Option<Timestamp>,
    pub last_error: Option<String>,
    pub last_error_at: Option<Timestamp>,
}

#[derive(Debug, Clone)]
pub struct Form {
    pub from_address: String,
    pub from_name: String,
    pub reply_to: Option<String>,
    pub link_base: Option<String>,
    pub daily_limit: u32,
}

type SettingsRow = (
    String,
    String,
    Option<String>,
    Option<String>,
    i64,
    Option<Timestamp>,
    Option<Timestamp>,
    Option<String>,
    Option<Timestamp>,
);

pub async fn load(pool: &SqlitePool) -> sqlx::Result<Settings> {
    let row: SettingsRow = sqlx::query_as(
        "SELECT from_address, from_name, reply_to, link_base, daily_limit, key_set_at, \
                last_test_at, last_error, last_error_at \
           FROM mail_settings WHERE id = 1",
    )
    .fetch_one(pool)
    .await?;

    Ok(Settings {
        from_address: row.0,
        from_name: row.1,
        reply_to: row.2,
        link_base: row.3,
        daily_limit: row.4.clamp(0, i64::from(u32::MAX)) as u32,
        key_set_at: row.5,
        last_test_at: row.6,
        last_error: row.7,
        last_error_at: row.8,
    })
}

pub async fn save(pool: &SqlitePool, form: &Form, now: Timestamp) -> sqlx::Result<()> {
    sqlx::query(
        "UPDATE mail_settings SET from_address = ?, from_name = ?, reply_to = ?, link_base = ?, \
                daily_limit = ?, updated_at = ? WHERE id = 1",
    )
    .bind(&form.from_address)
    .bind(&form.from_name)
    .bind(form.reply_to.as_deref())
    .bind(form.link_base.as_deref())
    .bind(i64::from(form.daily_limit))
    .bind(now)
    .execute(pool)
    .await
    .map(|_| ())
}

pub async fn mark_key(pool: &SqlitePool, when: Option<Timestamp>) -> sqlx::Result<()> {
    sqlx::query("UPDATE mail_settings SET key_set_at = ?, updated_at = ? WHERE id = 1")
        .bind(when)
        .bind(Timestamp::now())
        .execute(pool)
        .await
        .map(|_| ())
}

pub async fn mark_test(pool: &SqlitePool, when: Timestamp) -> sqlx::Result<()> {
    sqlx::query("UPDATE mail_settings SET last_test_at = ?, updated_at = ? WHERE id = 1")
        .bind(when)
        .bind(when)
        .execute(pool)
        .await
        .map(|_| ())
}

pub async fn mark_error(
    pool: &SqlitePool,
    error: Option<&str>,
    now: Timestamp,
) -> sqlx::Result<()> {
    sqlx::query(
        "UPDATE mail_settings SET last_error = ?, last_error_at = ?, updated_at = ? WHERE id = 1",
    )
    .bind(error)
    .bind(error.map(|_| now))
    .bind(now)
    .execute(pool)
    .await
    .map(|_| ())
}

#[derive(Debug, Clone)]
pub struct Queued {
    pub id: Id,
    pub kind: Kind,
    pub to_address: String,
    pub subject: String,
    pub html: String,
    pub text: String,
    pub attempts: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct Entry {
    pub id: Id,
    pub kind: Kind,
    pub to_address: String,
    pub subject: String,
    pub state: State,
    pub attempts: u32,
    pub next_attempt_at: Option<Timestamp>,
    pub provider_id: Option<String>,
    pub last_error: Option<String>,
    pub has_content: bool,
    pub created_at: Timestamp,
    pub sent_at: Option<Timestamp>,
}

pub struct NewMail<'a> {
    pub id: Id,
    pub kind: Kind,
    pub user: Option<Id>,
    pub to_address: &'a str,
    pub subject: &'a str,
    pub html: &'a str,
    pub text: &'a str,
    pub state: State,
    pub created_at: Timestamp,
}

pub async fn insert(pool: &SqlitePool, mail: &NewMail<'_>) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO mail_outbox (id, kind, user_id, to_address, subject, html, text, state, \
                                  attempts, next_attempt_at, created_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, 0, NULL, ?)",
    )
    .bind(mail.id)
    .bind(mail.kind.as_str())
    .bind(mail.user)
    .bind(mail.to_address)
    .bind(mail.subject)
    .bind(mail.html)
    .bind(mail.text)
    .bind(mail.state.as_str())
    .bind(mail.created_at)
    .execute(pool)
    .await
    .map(|_| ())
}

type QueuedRow = (Id, String, String, String, Option<String>, Option<String>, i64);

pub async fn claim_due(pool: &SqlitePool, now: Timestamp) -> sqlx::Result<Option<Queued>> {
    let row: Option<QueuedRow> = sqlx::query_as(
        "UPDATE mail_outbox SET state = 'sending' \
          WHERE id = (SELECT id FROM mail_outbox \
                       WHERE state = 'queued' \
                         AND (next_attempt_at IS NULL OR next_attempt_at <= ?) \
                       ORDER BY created_at LIMIT 1) \
      RETURNING id, kind, to_address, subject, html, text, attempts",
    )
    .bind(now)
    .fetch_optional(pool)
    .await?;

    Ok(row.and_then(|row| {
        Some(Queued {
            id: row.0,
            kind: row.1.parse().ok()?,
            to_address: row.2,
            subject: row.3,
            html: row.4?,
            text: row.5?,
            attempts: row.6.max(0) as u32,
        })
    }))
}

pub async fn mark_sent(
    pool: &SqlitePool,
    id: Id,
    provider_id: &str,
    now: Timestamp,
) -> sqlx::Result<()> {
    sqlx::query(
        "UPDATE mail_outbox SET state = 'sent', sent_at = ?, provider_id = ?, last_error = NULL, \
                attempts = attempts + 1, next_attempt_at = NULL, html = NULL, text = NULL \
          WHERE id = ?",
    )
    .bind(now)
    .bind(provider_id)
    .bind(id)
    .execute(pool)
    .await
    .map(|_| ())
}

pub async fn mark_waiting(
    pool: &SqlitePool,
    id: Id,
    next_attempt_at: Timestamp,
    error: &str,
) -> sqlx::Result<()> {
    sqlx::query(
        "UPDATE mail_outbox SET state = 'queued', attempts = attempts + 1, next_attempt_at = ?, \
                last_error = ? WHERE id = ?",
    )
    .bind(next_attempt_at)
    .bind(error)
    .bind(id)
    .execute(pool)
    .await
    .map(|_| ())
}

pub async fn mark_failed(pool: &SqlitePool, id: Id, error: &str) -> sqlx::Result<()> {
    sqlx::query(
        "UPDATE mail_outbox SET state = 'failed', attempts = attempts + 1, \
                next_attempt_at = NULL, last_error = ? WHERE id = ?",
    )
    .bind(error)
    .bind(id)
    .execute(pool)
    .await
    .map(|_| ())
}

pub async fn requeue_stuck(pool: &SqlitePool) -> sqlx::Result<u64> {
    let done = sqlx::query(
        "UPDATE mail_outbox SET state = 'queued', next_attempt_at = NULL \
          WHERE state = 'sending' AND html IS NOT NULL",
    )
    .execute(pool)
    .await?;
    Ok(done.rows_affected())
}

pub async fn requeue(pool: &SqlitePool, id: Id) -> sqlx::Result<bool> {
    let done = sqlx::query(
        "UPDATE mail_outbox SET state = 'queued', attempts = 0, next_attempt_at = NULL \
          WHERE id = ? AND state = 'failed' AND html IS NOT NULL",
    )
    .bind(id)
    .execute(pool)
    .await?;
    Ok(done.rows_affected() == 1)
}

#[derive(Debug, Clone, Copy, Default, Serialize)]
pub struct Counts {
    pub sent_today: u32,
    pub queued: u32,
    pub failed: u32,
}

pub async fn counts(pool: &SqlitePool, since: Timestamp) -> sqlx::Result<Counts> {
    let row: (i64, i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM mail_outbox \
                  WHERE state IN ('queued', 'sending', 'sent') AND created_at > ?), \
                (SELECT count(*) FROM mail_outbox WHERE state IN ('queued', 'sending')), \
                (SELECT count(*) FROM mail_outbox WHERE state = 'failed')",
    )
    .bind(since)
    .fetch_one(pool)
    .await?;

    Ok(Counts {
        sent_today: row.0.max(0) as u32,
        queued: row.1.max(0) as u32,
        failed: row.2.max(0) as u32,
    })
}

pub async fn spent_since(pool: &SqlitePool, since: Timestamp) -> sqlx::Result<u32> {
    let (count,): (i64,) = sqlx::query_as(
        "SELECT count(*) FROM mail_outbox \
          WHERE state IN ('queued', 'sending', 'sent') AND created_at > ?",
    )
    .bind(since)
    .fetch_one(pool)
    .await?;
    Ok(count.max(0) as u32)
}

pub async fn recent_to(
    pool: &SqlitePool,
    address: &str,
    kind: Kind,
    since: Timestamp,
) -> sqlx::Result<u32> {
    let (count,): (i64,) = sqlx::query_as(
        "SELECT count(*) FROM mail_outbox WHERE to_address = ? AND kind = ? AND created_at > ?",
    )
    .bind(address)
    .bind(kind.as_str())
    .bind(since)
    .fetch_one(pool)
    .await?;
    Ok(count.max(0) as u32)
}

pub async fn recent_of_kind(
    pool: &SqlitePool,
    kind: Kind,
    since: Timestamp,
) -> sqlx::Result<u32> {
    let (count,): (i64,) = sqlx::query_as(
        "SELECT count(*) FROM mail_outbox WHERE kind = ? AND created_at > ?",
    )
    .bind(kind.as_str())
    .bind(since)
    .fetch_one(pool)
    .await?;
    Ok(count.max(0) as u32)
}

type EntryRow = (
    Id,
    String,
    String,
    String,
    String,
    i64,
    Option<Timestamp>,
    Option<String>,
    Option<String>,
    bool,
    Timestamp,
    Option<Timestamp>,
);

pub async fn page(
    pool: &SqlitePool,
    limit: u32,
    state: Option<State>,
) -> sqlx::Result<(Vec<Entry>, u32)> {
    let filter = state.map(State::as_str);

    let rows: Vec<EntryRow> = sqlx::query_as(
        "SELECT id, kind, to_address, subject, state, attempts, next_attempt_at, provider_id, \
                last_error, html IS NOT NULL, created_at, sent_at \
           FROM mail_outbox WHERE (? IS NULL OR state = ?) \
           ORDER BY created_at DESC, id DESC LIMIT ?",
    )
    .bind(filter)
    .bind(filter)
    .bind(limit)
    .fetch_all(pool)
    .await?;

    let (total,): (i64,) =
        sqlx::query_as("SELECT count(*) FROM mail_outbox WHERE (? IS NULL OR state = ?)")
            .bind(filter)
            .bind(filter)
            .fetch_one(pool)
            .await?;

    let mails = rows
        .into_iter()
        .filter_map(|row| {
            Some(Entry {
                id: row.0,
                kind: row.1.parse().ok()?,
                to_address: row.2,
                subject: row.3,
                state: row.4.parse().ok()?,
                attempts: row.5.max(0) as u32,
                next_attempt_at: row.6,
                provider_id: row.7,
                last_error: row.8,
                has_content: row.9,
                created_at: row.10,
                sent_at: row.11,
            })
        })
        .collect();

    Ok((mails, total.max(0) as u32))
}

pub async fn content(pool: &SqlitePool, id: Id) -> sqlx::Result<Option<Option<String>>> {
    let row: Option<(Option<String>,)> =
        sqlx::query_as("SELECT html FROM mail_outbox WHERE id = ?")
            .bind(id)
            .fetch_optional(pool)
            .await?;
    Ok(row.map(|row| row.0))
}

pub async fn purge(pool: &SqlitePool, cutoff: Timestamp) -> sqlx::Result<u64> {
    let gone = sqlx::query("DELETE FROM mail_outbox WHERE created_at < ?")
        .bind(cutoff)
        .execute(pool)
        .await?;
    Ok(gone.rows_affected())
}
