use axum_extra::extract::cookie::{Cookie, SameSite};
use sqlx::SqlitePool;
use time::Duration;

use super::secret::{digest, fresh as fresh_secret};
use crate::model::{Id, Timestamp};

pub const COOKIE: &str = "craft_session";
pub const LIFETIME: Duration = Duration::days(30);
const REFRESH_AFTER: Duration = Duration::hours(1);
const USER_AGENT_LIMIT: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct Session {
    pub id: Id,
    pub user_id: Id,
    pub expires_at: Timestamp,
    pub last_seen: Timestamp,
}

pub async fn open(
    pool: &SqlitePool,
    user_id: Id,
    user_agent: Option<&str>,
    now: Timestamp,
) -> sqlx::Result<(Session, String)> {
    let secret = fresh_secret();
    let session = Session {
        id: Id::new(),
        user_id,
        expires_at: shift(now, LIFETIME),
        last_seen: now,
    };
    let agent = user_agent.map(|value| value.chars().take(USER_AGENT_LIMIT).collect::<String>());

    sqlx::query(
        "INSERT INTO sessions (id, user_id, token_hash, created_at, expires_at, last_seen, \
         user_agent) VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(session.id)
    .bind(session.user_id)
    .bind(digest(&secret))
    .bind(now)
    .bind(session.expires_at)
    .bind(session.last_seen)
    .bind(agent)
    .execute(pool)
    .await?;

    Ok((session, secret))
}

pub async fn lookup(
    pool: &SqlitePool,
    secret: &str,
    now: Timestamp,
) -> sqlx::Result<Option<Session>> {
    let session = sqlx::query_as::<_, Session>(
        "SELECT id, user_id, expires_at, last_seen FROM sessions WHERE token_hash = ?",
    )
    .bind(digest(secret))
    .fetch_optional(pool)
    .await?;

    Ok(session.filter(|session| session.expires_at > now))
}

pub async fn refresh(
    pool: &SqlitePool,
    session: &Session,
    now: Timestamp,
) -> sqlx::Result<Option<Session>> {
    if shift(session.last_seen, REFRESH_AFTER) > now {
        return Ok(None);
    }

    let slid = Session { expires_at: shift(now, LIFETIME), last_seen: now, ..session.clone() };
    sqlx::query("UPDATE sessions SET expires_at = ?, last_seen = ? WHERE id = ?")
        .bind(slid.expires_at)
        .bind(slid.last_seen)
        .bind(slid.id)
        .execute(pool)
        .await?;

    Ok(Some(slid))
}

pub async fn close(pool: &SqlitePool, id: Id) -> sqlx::Result<()> {
    sqlx::query("DELETE FROM sessions WHERE id = ?").bind(id).execute(pool).await?;
    Ok(())
}

pub async fn close_all_of(pool: &SqlitePool, user_id: Id, except: Option<Id>) -> sqlx::Result<u64> {
    let closed = sqlx::query("DELETE FROM sessions WHERE user_id = ? AND id IS NOT ?")
        .bind(user_id)
        .bind(except)
        .execute(pool)
        .await?;
    Ok(closed.rows_affected())
}

pub async fn count_active(pool: &SqlitePool, user_id: Id, now: Timestamp) -> sqlx::Result<u32> {
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM sessions WHERE user_id = ? AND expires_at > ?",
    )
    .bind(user_id)
    .bind(now)
    .fetch_one(pool)
    .await?;
    Ok(count.max(0) as u32)
}

pub async fn purge_expired(pool: &SqlitePool, now: Timestamp) -> sqlx::Result<u64> {
    let gone = sqlx::query("DELETE FROM sessions WHERE expires_at <= ?")
        .bind(now)
        .execute(pool)
        .await?;
    Ok(gone.rows_affected())
}

pub fn cookie(secret: String, secure: bool) -> Cookie<'static> {
    Cookie::build((COOKIE, secret))
        .http_only(true)
        .same_site(SameSite::Lax)
        .path("/")
        .secure(secure)
        .max_age(LIFETIME)
        .build()
}

pub fn cleared_cookie(secure: bool) -> Cookie<'static> {
    Cookie::build((COOKIE, ""))
        .http_only(true)
        .same_site(SameSite::Lax)
        .path("/")
        .secure(secure)
        .max_age(Duration::ZERO)
        .build()
}

fn shift(from: Timestamp, by: Duration) -> Timestamp {
    Timestamp::at(from.as_datetime() + by)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::harness::{a_user, test_pool};

    #[tokio::test]
    async fn the_cookie_itself_is_nowhere_in_the_database() {
        let pool = test_pool().await;
        let user = a_user(&pool, "max").await;
        let (_, secret) = open(&pool, user, None, Timestamp::now()).await.unwrap();

        let stored: String = sqlx::query_scalar("SELECT token_hash FROM sessions")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_ne!(stored, secret);
        assert_eq!(stored, digest(&secret));
        assert_eq!(stored.len(), 64, "sha-256 in hex");
    }

    #[tokio::test]
    async fn a_session_that_ran_out_is_no_session() {
        let pool = test_pool().await;
        let user = a_user(&pool, "max").await;
        let now = Timestamp::now();
        let (session, secret) = open(&pool, user, None, now).await.unwrap();

        assert_eq!(lookup(&pool, &secret, now).await.unwrap(), Some(session.clone()));

        let a_second_late = shift(session.expires_at, Duration::seconds(1));
        assert_eq!(
            lookup(&pool, &secret, a_second_late).await.unwrap(),
            None,
            "the cookie is right, the clock is not"
        );
    }

    #[tokio::test]
    async fn the_thirty_days_slide_but_only_once_an_hour() {
        let pool = test_pool().await;
        let user = a_user(&pool, "max").await;
        let opened = Timestamp::now();
        let (session, _) = open(&pool, user, None, opened).await.unwrap();

        let soon = shift(opened, Duration::minutes(59));
        assert_eq!(refresh(&pool, &session, soon).await.unwrap(), None, "too soon to write");

        let later = shift(opened, Duration::minutes(61));
        let slid = refresh(&pool, &session, later).await.unwrap().expect("an hour has passed");
        assert_eq!(slid.expires_at, shift(later, LIFETIME));
        assert!(slid.expires_at > session.expires_at);

        let stored: Timestamp = sqlx::query_scalar("SELECT expires_at FROM sessions WHERE id = ?")
            .bind(session.id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(stored, slid.expires_at);
    }

    #[tokio::test]
    async fn purging_takes_the_stale_rows_and_leaves_the_rest() {
        let pool = test_pool().await;
        let user = a_user(&pool, "max").await;
        let now = Timestamp::now();
        let (stale, _) = open(&pool, user, None, shift(now, -LIFETIME - Duration::days(1))).await.unwrap();
        let (fresh, _) = open(&pool, user, None, now).await.unwrap();

        assert_eq!(purge_expired(&pool, now).await.unwrap(), 1);

        let left: Vec<Id> = sqlx::query_scalar("SELECT id FROM sessions").fetch_all(&pool).await.unwrap();
        assert_eq!(left, vec![fresh.id]);
        assert!(!left.contains(&stale.id));
    }

    #[tokio::test]
    async fn closing_the_others_spares_exactly_one() {
        let pool = test_pool().await;
        let user = a_user(&pool, "max").await;
        let now = Timestamp::now();
        let (keep, _) = open(&pool, user, None, now).await.unwrap();
        open(&pool, user, None, now).await.unwrap();
        open(&pool, user, None, now).await.unwrap();

        assert_eq!(close_all_of(&pool, user, Some(keep.id)).await.unwrap(), 2);
        assert_eq!(count_active(&pool, user, now).await.unwrap(), 1);

        assert_eq!(close_all_of(&pool, user, None).await.unwrap(), 1);
        assert_eq!(count_active(&pool, user, now).await.unwrap(), 0);
    }

    #[test]
    fn the_cookie_says_what_1_2_demands() {
        let baked = cookie("abc".to_owned(), false).to_string();
        assert!(baked.starts_with("craft_session=abc"), "{baked}");
        assert!(baked.contains("HttpOnly"), "{baked}");
        assert!(baked.contains("SameSite=Lax"), "{baked}");
        assert!(baked.contains("Path=/"), "{baked}");
        assert!(baked.contains("Max-Age=2592000"), "thirty days: {baked}");
        assert!(!baked.contains("Secure"), "plain http: {baked}");

        assert!(cookie("abc".to_owned(), true).to_string().contains("Secure"));
        assert!(cleared_cookie(false).to_string().contains("Max-Age=0"));
    }
}
