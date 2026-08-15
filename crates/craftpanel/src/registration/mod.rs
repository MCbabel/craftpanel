pub mod address;
pub mod store;

#[cfg(test)]
mod tests;

use std::net::IpAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use sqlx::SqlitePool;
use time::Duration as Span;

use crate::auth::error::{Failure, Result};
use crate::auth::users::{NewUser, UserRow};
use crate::auth::{password, rates, secret, settings, users, Disks, LiveServers};
use crate::helper::Helper;
use crate::mail::{Mail, Message, Recipient};
use crate::model::{
    AccountOrigin, AuthOptions, Id, PanelRole, PanelUser, Registration, RegistrationList,
    RegistrationState, Timestamp,
};

const TOKEN_LIFE: Span = Span::hours(24);
const TOKEN_LIFE_FOR_MAIL: Duration = Duration::from_secs(24 * 60 * 60);

const TOKENS_PER_APPLICATION: u32 = 5;

const BLOCK_AFTER_REJECTION: Span = Span::days(30);

const KEEP_UNVERIFIED: Span = Span::days(7);
const KEEP_WAITING: Span = Span::days(30);
const SWEEP_EVERY: Duration = Duration::from_secs(6 * 60 * 60);

const VERIFY_ATTEMPTS: rates::Limit =
    rates::Limit::new("verify_email", 30, Duration::from_secs(60 * 60));

pub struct Registrations {
    pool: SqlitePool,
    mail: Arc<Mail>,
    helper: Helper,
    rates: Arc<rates::Buckets>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Verified {
    Active,
    AwaitingApproval,
}

struct Applicant {
    username: String,
    email: String,
    password: String,
}

impl Registrations {
    pub fn new(pool: SqlitePool, mail: Arc<Mail>, helper: Helper) -> Arc<Self> {
        Arc::new(Self { pool, mail, helper, rates: rates::shared() })
    }

    #[cfg(test)]
    pub fn with_own_rates(pool: SqlitePool, mail: Arc<Mail>, helper: Helper) -> Arc<Self> {
        Arc::new(Self { pool, mail, helper, rates: Arc::new(rates::Buckets::default()) })
    }

    pub async fn options(&self) -> Result<AuthOptions> {
        let switches = settings::load(&self.pool).await?;
        let mail_ready = self.mail.can_link().await;
        Ok(AuthOptions {
            registration_enabled: switches.registration_enabled && mail_ready,
            registration_requires_approval: switches.registration_requires_approval,
            password_reset_enabled: mail_ready,
        })
    }

    async fn require_open(&self) -> Result<bool> {
        let open = self.options().await?;
        if !open.registration_enabled {
            return Err(Failure::conflict(
                "registration_disabled",
                "this panel does not take sign-ups at the moment",
            ));
        }
        Ok(open.registration_requires_approval)
    }

    pub async fn apply(
        &self,
        username: &str,
        email: &str,
        chosen: &str,
        from: Option<IpAddr>,
        now: Timestamp,
    ) -> Result<()> {
        self.require_open().await?;

        let applicant = Applicant {
            username: username.trim().to_owned(),
            email: address::normalise(email)?,
            password: chosen.to_owned(),
        };
        password::check_strength(&applicant.password)?;
        users::check_username(&applicant.username)?;

        if let Some(from) = from {
            let key = from.to_string();
            for limit in [rates::REGISTER_PER_HOUR, rates::REGISTER_PER_DAY] {
                if let Some(seconds) = self.rates.take(limit, &key, Instant::now()) {
                    return Err(Failure::rate_limited(seconds));
                }
            }
        }

        let open = store::by_email(&self.pool, &applicant.email).await?;
        let unconfirmed = open
            .as_ref()
            .filter(|row| row.state == RegistrationState::EmailUnverified)
            .map(|row| row.id);

        users::claim_name_for_sign_up(&self.pool, &applicant.username, unconfirmed).await?;

        let hash = password::hash(&applicant.password)?;

        if store::is_blocked(&self.pool, &applicant.email, now).await? {
            tracing::info!(username = %applicant.username, "a blocked address applied again");
            return Ok(());
        }

        if let Some(existing) = users::by_email(&self.pool, &applicant.email).await? {
            self.mail
                .notify(Message::AddressAlreadyRegistered {
                    to: Recipient::account(existing.id, applicant.email.clone()),
                    username: existing.username,
                })
                .await;
            return Ok(());
        }

        if open.is_some_and(|row| row.state == RegistrationState::AwaitingApproval) {
            tracing::info!(
                username = %applicant.username,
                "an address that is waiting for approval was applied for again"
            );
            return Ok(());
        }

        let token = secret::fresh();
        let application = store::NewApplication {
            username: &applicant.username,
            email: &applicant.email,
            password_hash: hash,
            signup_ip: from.map(|from| from.to_string()),
            token_hash: secret::digest(&token),
            token_expires_at: shift(now, TOKEN_LIFE),
        };

        let written = match unconfirmed {
            Some(id) => store::take_over(&self.pool, id, application, now)
                .await
                .map(|taken| taken.then_some(id)),
            None => store::insert(&self.pool, application, now).await.map(Some),
        };

        let id = match written {
            Ok(Some(id)) => id,
            Ok(None) => return Ok(()),
            Err(err) => return match users::map_taken(err) {
                failure if failure.code() == "username_taken" => Err(failure),
                _ => Ok(()),
            },
        };

        if let Err(refused) = self
            .mail
            .send(Message::VerifyEmail {
                to: Recipient::address(applicant.email.clone()),
                username: applicant.username.clone(),
                token,
                valid_for: TOKEN_LIFE_FOR_MAIL,
            })
            .await
        {
            store::remove(&self.pool, id).await?;
            tracing::warn!(
                %id, code = refused.code(),
                "a sign-up was dropped again: its confirmation mail was refused"
            );
            return Ok(());
        }

        self.rates.note(rates::RESEND_PER_ADDRESS, &applicant.email, Instant::now());

        tracing::info!(
            %id, username = %applicant.username, replaced = unconfirmed.is_some(),
            "a new sign-up is waiting for its address to be confirmed"
        );
        Ok(())
    }

    pub async fn verify(
        &self,
        token: &str,
        from: Option<IpAddr>,
        live: &LiveServers,
        disks: &Disks,
        now: Timestamp,
    ) -> Result<Verified> {
        let requires_approval = self.require_open().await?;
        let attempts = from.map(|from| from.to_string());

        if let Some(key) = &attempts {
            if let Some(seconds) = self.rates.check(VERIFY_ATTEMPTS, key, Instant::now()) {
                return Err(Failure::rate_limited(seconds));
            }
        }

        let Some(row) = store::by_token(&self.pool, token).await? else {
            if let Some(key) = &attempts {
                self.rates.note(VERIFY_ATTEMPTS, key, Instant::now());
            }
            return Err(dead_link());
        };

        if row.state == RegistrationState::AwaitingApproval {
            return Ok(Verified::AwaitingApproval);
        }
        if row.token_expires_at.is_some_and(|until| until <= now) {
            return Err(Failure::new(
                axum::http::StatusCode::GONE,
                "token_expired",
                "this confirmation link has run out; ask for a new one",
            ));
        }

        if !requires_approval {
            let Some(hash) = store::password_hash_for_token(&self.pool, row.id, token).await?
            else {
                return Err(dead_link());
            };

            let admitted = self.admit(&row, hash, live, disks).await?;
            tracing::info!(user = %admitted.id, username = %admitted.username, "a sign-up confirmed its address and was let in");
            return Ok(Verified::Active);
        }

        if store::mark_verified(&self.pool, row.id, token, now).await? {
            self.tell_the_admins(&row, now).await;
            tracing::info!(registration = %row.id, username = %row.username, "a sign-up confirmed its address and is waiting for approval");
            return Ok(Verified::AwaitingApproval);
        }

        match store::by_token(&self.pool, token).await? {
            Some(_) => Ok(Verified::AwaitingApproval),
            None => Err(dead_link()),
        }
    }

    pub async fn resend(&self, email: &str, now: Timestamp) -> Result<()> {
        self.require_open().await?;

        let Ok(email) = address::normalise(email) else { return Ok(()) };

        let Some(row) = store::by_email(&self.pool, &email).await? else { return Ok(()) };
        if row.state != RegistrationState::EmailUnverified {
            return Ok(());
        }
        self.mint_and_send(&row, now).await;
        Ok(())
    }

    async fn mint_and_send(&self, row: &store::Row, now: Timestamp) {
        if let Some(seconds) =
            self.rates.take(rates::RESEND_PER_ADDRESS, &row.email, Instant::now())
        {
            tracing::info!(
                registration = %row.id, seconds,
                "no second confirmation mail yet; the five minutes are not up"
            );
            return;
        }
        if row.tokens_sent >= TOKENS_PER_APPLICATION {
            tracing::info!(
                registration = %row.id,
                "this application has had its {TOKENS_PER_APPLICATION} links"
            );
            return;
        }

        let token = secret::fresh();
        let expires = shift(now, TOKEN_LIFE);
        let written =
            store::replace_token(&self.pool, row.id, &secret::digest(&token), expires, now).await;
        let applicant = match written {
            Ok(Some(applicant)) => applicant,
            Ok(None) => return,
            Err(err) => {
                tracing::error!(registration = %row.id, "a fresh confirmation token was not written: {err}");
                return;
            }
        };

        let _ = self
            .mail
            .send(Message::VerifyEmail {
                to: Recipient::address(row.email.clone()),
                username: applicant,
                token,
                valid_for: TOKEN_LIFE_FOR_MAIL,
            })
            .await;
    }

    pub async fn queue(&self, limit: u32, offset: u32) -> Result<RegistrationList> {
        let (rows, total) = store::page(&self.pool, limit, offset).await?;
        let registrations: Vec<Registration> = rows.iter().map(store::Row::as_wire).collect();
        Ok(RegistrationList { registrations, total })
    }

    pub async fn approve(
        &self,
        id: Id,
        live: &LiveServers,
        disks: &Disks,
    ) -> Result<PanelUser> {
        let row = self.application(id).await?;
        if row.state != RegistrationState::AwaitingApproval {
            return Err(Failure::conflict(
                "invalid_state",
                "this application has not confirmed its address yet",
            ));
        }

        let hash = store::password_hash(&self.pool, row.id)
            .await?
            .ok_or_else(|| Failure::not_found("registration_not_found", "no such application"))?;

        let admitted = self.admit(&row, hash, live, disks).await?;
        self.mail
            .notify(Message::AccountApproved {
                to: Recipient::account(admitted.id, row.email.clone()),
                username: row.username.clone(),
            })
            .await;
        tracing::info!(user = %admitted.id, username = %row.username, "an application was approved");
        Ok(admitted)
    }

    pub async fn reject(&self, id: Id, reason: Option<&str>, now: Timestamp) -> Result<()> {
        let row = self.application(id).await?;

        store::remove(&self.pool, row.id).await?;
        store::block(&self.pool, &row.email, Some(shift(now, BLOCK_AFTER_REJECTION)), reason, now)
            .await?;

        self.mail
            .notify(Message::AccountRejected {
                to: Recipient::address(row.email.clone()),
                username: row.username.clone(),
            })
            .await;
        tracing::info!(
            registration = %row.id, username = %row.username, reason = reason.unwrap_or("—"),
            "an application was rejected and its address blocked for thirty days"
        );
        Ok(())
    }

    async fn application(&self, id: Id) -> Result<store::Row> {
        store::by_id(&self.pool, id)
            .await?
            .ok_or_else(|| Failure::not_found("registration_not_found", "no such application"))
    }

    async fn admit(
        &self,
        row: &store::Row,
        hash: String,
        live: &LiveServers,
        disks: &Disks,
    ) -> Result<PanelUser> {
        users::claim_name_in_users(&self.pool, &row.username, None).await.map_err(|taken| {
            if taken.code() == "username_taken" {
                Failure::conflict(
                    "username_taken",
                    format!("{} has been taken in the meantime; please sign up again", row.username),
                )
            } else {
                taken
            }
        })?;

        let user = users::insert(
            &self.pool,
            NewUser {
                username: &row.username,
                email: Some(row.email.clone()),
                origin: AccountOrigin::Registration,
                password_hash: hash,
                role: PanelRole::User,
                must_change_password: false,
                limits: settings::load(&self.pool).await?.default_limits,
            },
        )
        .await
        .map_err(users::map_taken)?;

        store::remove(&self.pool, row.id).await?;

        let system = users::provision(&self.pool, &self.helper, &user).await?;
        if let Some(complaint) = &system.error_message {
            tracing::warn!(user = %user.id, "the helper could not set up the account: {complaint}");
        }
        let user = UserRow {
            system_state: system.state,
            system_uid: system.uid,
            system_error_message: system.error_message,
            ..user
        };

        Ok(users::panel_user(&self.pool, &user, live, disks).await?)
    }

    async fn tell_the_admins(&self, row: &store::Row, now: Timestamp) {
        let admins: Vec<(Id, String)> = match sqlx::query_as(
            "SELECT id, email FROM users WHERE role = 'admin' AND email IS NOT NULL",
        )
        .fetch_all(&self.pool)
        .await
        {
            Ok(found) => found,
            Err(err) => {
                tracing::warn!("the administrators could not be told about a sign-up: {err}");
                return;
            }
        };

        if admins.is_empty() {
            tracing::info!(
                registration = %row.id,
                "an application is waiting, and no administrator has an address to be told at"
            );
            return;
        }

        for (id, email) in admins {
            self.mail
                .notify(Message::AccountAwaitingReview {
                    to: Recipient::account(id, email),
                    applicant: row.username.clone(),
                    email: row.email.clone(),
                    when: now,
                })
                .await;
        }
    }

    pub async fn sweep(&self, now: Timestamp) -> sqlx::Result<store::Swept> {
        store::sweep(&self.pool, shift(now, -KEEP_UNVERIFIED), shift(now, -KEEP_WAITING), now).await
    }
}

pub fn spawn_sweep(service: Arc<Registrations>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(SWEEP_EVERY);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tick.tick().await;
            match service.sweep(Timestamp::now()).await {
                Ok(swept) if swept.any() => tracing::info!(
                    unverified = swept.unverified,
                    waiting = swept.waiting,
                    blocks = swept.blocks,
                    "stale sign-ups removed"
                ),
                Ok(_) => {}
                Err(err) => tracing::warn!("the sign-ups could not be swept: {err}"),
            }
        }
    })
}

fn shift(from: Timestamp, by: Span) -> Timestamp {
    Timestamp::at(from.as_datetime() + by)
}

fn dead_link() -> Failure {
    Failure::not_found(
        "invalid_token",
        "this confirmation link is not valid. If you have already confirmed, sign in.",
    )
}
