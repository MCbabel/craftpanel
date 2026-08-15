use std::net::IpAddr;
use std::sync::Arc;
use std::time::{Duration as Wait, Instant};

use sqlx::SqlitePool;
use time::Duration;

use super::error::{Failure, Result};
use super::{password, rates, secret, session, users};
use crate::mail::{Mail, Message, Recipient};
use crate::model::{Id, Timestamp};

pub const LIFETIME: Duration = Duration::minutes(30);

const COOL_DOWN: Duration = Duration::seconds(60);
const PER_HOUR: i64 = 5;

const KEEP_SPENT: Duration = Duration::hours(24);

pub struct Recovery {
    pool: SqlitePool,
    mail: Arc<Mail>,
    rates: Arc<rates::Buckets>,
    #[cfg(test)]
    gate: Option<Arc<Gate>>,
}

#[cfg(test)]
#[derive(Default)]
pub struct Gate {
    pub open: tokio::sync::Notify,
    pub finished: tokio::sync::Notify,
}

impl Recovery {
    pub fn new(pool: SqlitePool, mail: Arc<Mail>) -> Arc<Self> {
        Arc::new(Self {
            pool,
            mail,
            rates: rates::shared(),
            #[cfg(test)]
            gate: None,
        })
    }

    #[cfg(test)]
    pub fn gated(pool: SqlitePool, mail: Arc<Mail>, gate: Arc<Gate>) -> Arc<Self> {
        Arc::new(Self {
            pool,
            mail,
            rates: Arc::new(rates::Buckets::default()),
            gate: Some(gate),
        })
    }

    pub fn note_request(&self, email: &str, from: Option<IpAddr>) -> Result<()> {
        let now = Instant::now();
        let mut keys = vec![email.trim().to_lowercase()];
        keys.extend(from.map(|from| from.to_string()));

        for key in keys {
            if self.rates.take(rates::RESET_ATTEMPTS, &key, now).is_some() {
                return Err(too_many());
            }
        }
        Ok(())
    }

    pub async fn begin(
        &self,
        typed: &str,
        from: Option<IpAddr>,
        user_agent: Option<String>,
        now: Timestamp,
    ) {
        #[cfg(test)]
        if let Some(gate) = &self.gate {
            gate.open.notified().await;
        }

        if let Err(err) = self.mint(typed, from, user_agent, now).await {
            tracing::warn!("a password reset could not be prepared: {err}");
        }

        #[cfg(test)]
        if let Some(gate) = &self.gate {
            gate.finished.notify_one();
        }
    }

    async fn mint(
        &self,
        typed: &str,
        from: Option<IpAddr>,
        user_agent: Option<String>,
        now: Timestamp,
    ) -> Result<()> {
        purge(&self.pool, now).await?;

        let Ok(email) = crate::registration::address::normalise(typed) else { return Ok(()) };

        let Some(user) = users::by_email(&self.pool, &email).await? else {
            tracing::info!("a password reset was asked for an address with no account");
            return Ok(());
        };

        if !self.mail.can_link().await {
            tracing::warn!(user = %user.id, "a password reset was asked for and no mail is set up");
            return Ok(());
        }

        if let Some(waited) = self.too_soon(user.id, now).await? {
            tracing::info!(user = %user.id, waited, "no second reset mail yet");
            return Ok(());
        }

        forget_all(&self.pool, user.id).await?;

        let token = secret::fresh();
        let id = Id::new();
        sqlx::query(
            "INSERT INTO password_resets (id, user_id, token_hash, created_at, expires_at, \
             requested_ip, user_agent) VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(id)
        .bind(user.id)
        .bind(secret::digest(&token))
        .bind(now)
        .bind(shift(now, LIFETIME))
        .bind(from.map(|from| from.to_string()))
        .bind(user_agent)
        .execute(&self.pool)
        .await?;

        let sent = self
            .mail
            .send(Message::ResetPassword {
                to: Recipient::account(user.id, email),
                username: user.username.clone(),
                token,
                valid_for: Wait::from_secs(LIFETIME.whole_seconds().max(0) as u64),
            })
            .await;

        if let Err(err) = sent {
            sqlx::query("DELETE FROM password_resets WHERE id = ?")
                .bind(id)
                .execute(&self.pool)
                .await?;
            tracing::warn!(
                user = %user.id, code = err.code(),
                "the reset mail was refused, so the link was thrown away: {}", err.message()
            );
        }
        Ok(())
    }

    async fn too_soon(&self, user: Id, now: Timestamp) -> Result<Option<i64>> {
        let newest: Option<Timestamp> = sqlx::query_scalar(
            "SELECT created_at FROM password_resets WHERE user_id = ? ORDER BY created_at DESC \
             LIMIT 1",
        )
        .bind(user)
        .fetch_optional(&self.pool)
        .await?;

        if let Some(last) = newest {
            let since = now.unix_seconds() - last.unix_seconds();
            if since < COOL_DOWN.whole_seconds() {
                return Ok(Some(COOL_DOWN.whole_seconds() - since));
            }
        }

        let in_the_hour: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM password_resets WHERE user_id = ? AND created_at > ?",
        )
        .bind(user)
        .bind(shift(now, -Duration::hours(1)))
        .fetch_one(&self.pool)
        .await?;

        Ok((in_the_hour >= PER_HOUR).then_some(COOL_DOWN.whole_seconds()))
    }

    pub async fn whose(
        &self,
        token: &str,
        from: Option<IpAddr>,
        now: Timestamp,
    ) -> Result<String> {
        self.guard(from)?;
        match self.living(token, now).await? {
            Some((_, username)) => Ok(username),
            None => Err(self.refuse(from)),
        }
    }

    pub async fn confirm(
        &self,
        token: &str,
        chosen: &str,
        from: Option<IpAddr>,
        now: Timestamp,
    ) -> Result<()> {
        self.guard(from)?;

        let Some((user, username)) = self.living(token, now).await? else {
            return Err(self.refuse(from));
        };

        let hash = password::hash(chosen)?;

        sqlx::query(
            "UPDATE users SET password_hash = ?, must_change_password = 0, updated_at = ? \
             WHERE id = ?",
        )
        .bind(hash)
        .bind(now)
        .bind(user)
        .execute(&self.pool)
        .await?;

        let digest = secret::digest(token);
        sqlx::query("UPDATE password_resets SET used_at = ? WHERE token_hash = ?")
            .bind(now)
            .bind(&digest)
            .execute(&self.pool)
            .await?;
        forget_others(&self.pool, user, &digest).await?;

        let closed = session::close_all_of(&self.pool, user, None).await?;
        tracing::info!(%user, closed, "a password was set through a reset link");

        if let Some(row) = users::find(&self.pool, user).await? {
            if let Some(email) = row.email {
                self.mail
                    .notify(Message::PasswordChanged {
                        to: Recipient::account(user, email),
                        username,
                        when: now,
                    })
                    .await;
            }
        }
        Ok(())
    }

    pub async fn on_behalf_of(&self, user: &users::UserRow, now: Timestamp) -> Result<()> {
        let Some(email) = user.email.clone() else {
            return Err(Failure::conflict(
                "no_email_address",
                "this account has no address to send anything to",
            ));
        };
        if !self.mail.configured().await {
            return Err(Failure::conflict(
                "mail_not_configured",
                "set up mail first, or use `craftpanel admin reset-link`",
            ));
        }
        if !self.mail.can_link().await {
            return Err(Failure::conflict(
                "mail_no_link_base",
                "this panel does not know its own address, so no link can be built: set it under \
                 Administration → Mail, or use `craftpanel admin reset-link`",
            ));
        }
        if let Some(seconds) =
            self.rates.take(rates::ADMIN_RESET_PER_ACCOUNT, &user.id.to_string(), Instant::now())
        {
            return Err(too_many_links(seconds));
        }

        purge(&self.pool, now).await?;
        forget_all(&self.pool, user.id).await?;
        let token = mint_for(&self.pool, user.id, None, None, now).await?;

        let sent = self
            .mail
            .send(Message::ResetPassword {
                to: Recipient::account(user.id, email),
                username: user.username.clone(),
                token,
                valid_for: Wait::from_secs(LIFETIME.whole_seconds().max(0) as u64),
            })
            .await;

        if let Err(refused) = sent {
            forget_all(&self.pool, user.id).await?;
            return Err(refused.into());
        }
        tracing::info!(user = %user.id, "an administrator sent a reset link");
        Ok(())
    }

    async fn living(
        &self,
        token: &str,
        now: Timestamp,
    ) -> Result<Option<(Id, String)>> {
        Ok(sqlx::query_as::<_, (Id, String)>(
            "SELECT r.user_id, u.username FROM password_resets r JOIN users u ON u.id = r.user_id \
             WHERE r.token_hash = ? AND r.used_at IS NULL AND r.expires_at > ?",
        )
        .bind(secret::digest(token))
        .bind(now)
        .fetch_optional(&self.pool)
        .await?)
    }

    fn guard(&self, from: Option<IpAddr>) -> Result<()> {
        let Some(from) = from else { return Ok(()) };
        match self.rates.check(rates::RESET_ATTEMPTS, &from.to_string(), Instant::now()) {
            Some(_) => Err(too_many()),
            None => Ok(()),
        }
    }

    fn refuse(&self, from: Option<IpAddr>) -> Failure {
        if let Some(from) = from {
            self.rates.note(rates::RESET_ATTEMPTS, &from.to_string(), Instant::now());
        }
        Failure::bad_request("invalid_reset_token", "this link is no longer valid")
    }
}

fn too_many() -> Failure {
    Failure::new(
        axum::http::StatusCode::TOO_MANY_REQUESTS,
        "too_many_attempts",
        "too many attempts; try again in a few minutes",
    )
}

fn too_many_links(seconds: u64) -> Failure {
    let allowed = rates::ADMIN_RESET_PER_ACCOUNT.allowance();
    let minutes = seconds.div_ceil(60);
    Failure::new(
        axum::http::StatusCode::TOO_MANY_REQUESTS,
        "too_many_attempts",
        format!(
            "{allowed} reset links for this account in an hour is enough — each one makes the \
             older ones stop working. Try again in {minutes} minute(s), or set a password here."
        ),
    )
}

pub async fn forget_all(pool: &SqlitePool, user: Id) -> sqlx::Result<u64> {
    let gone = sqlx::query("DELETE FROM password_resets WHERE user_id = ?")
        .bind(user)
        .execute(pool)
        .await?;
    Ok(gone.rows_affected())
}

pub async fn forget_others(pool: &SqlitePool, user: Id, keep: &str) -> sqlx::Result<u64> {
    let gone = sqlx::query("DELETE FROM password_resets WHERE user_id = ? AND token_hash != ?")
        .bind(user)
        .bind(keep)
        .execute(pool)
        .await?;
    Ok(gone.rows_affected())
}

pub async fn mint_for(
    pool: &SqlitePool,
    user: Id,
    life: Option<Duration>,
    from: Option<IpAddr>,
    now: Timestamp,
) -> sqlx::Result<String> {
    let token = secret::fresh();
    sqlx::query(
        "INSERT INTO password_resets (id, user_id, token_hash, created_at, expires_at, \
         requested_ip) VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(Id::new())
    .bind(user)
    .bind(secret::digest(&token))
    .bind(now)
    .bind(shift(now, life.unwrap_or(LIFETIME)))
    .bind(from.map(|from| from.to_string()))
    .execute(pool)
    .await?;
    Ok(token)
}

async fn purge(pool: &SqlitePool, now: Timestamp) -> sqlx::Result<u64> {
    let cutoff = shift(now, -KEEP_SPENT);
    let gone = sqlx::query(
        "DELETE FROM password_resets WHERE (used_at IS NOT NULL AND used_at < ?) \
         OR expires_at < ?",
    )
    .bind(cutoff)
    .bind(cutoff)
    .execute(pool)
    .await?;
    Ok(gone.rows_affected())
}

fn shift(from: Timestamp, by: Duration) -> Timestamp {
    Timestamp::at(from.as_datetime() + by)
}
