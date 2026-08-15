#![allow(dead_code)]

pub mod cli;
pub mod harness;
pub mod key;
pub mod message;
pub mod queue;
pub mod render;
pub mod resend;
pub mod sink;
pub mod store;

use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;
use sqlx::SqlitePool;
use tokio::sync::Notify;

use crate::auth::error::Failure;
use crate::config::Config;
use crate::model::{Id, Timestamp};

pub use message::{Kind, Message, Recipient};
pub use resend::MailError;
pub use store::State;

use self::key::KeyFile;
use self::message::Sender;
use self::render::Rendered;
use self::resend::{ApiKey, Outgoing, Resend};
use self::sink::Sink;
use self::store::{Counts, Entry, Form, NewMail, Queued, Settings};

const PER_ADDRESS_AND_KIND: u32 = 5;
const TEST_MAILS: u32 = 10;
const BRAKE_WINDOW: Duration = Duration::from_secs(60 * 60);
const DAY: Duration = Duration::from_secs(24 * 60 * 60);

pub const RETENTION: time::Duration = time::Duration::days(30);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ServiceState {
    NotConfigured,
    Configured,
    FileSink,
}

#[derive(Debug, Clone, Serialize)]
pub struct MailSettings {
    pub provider: &'static str,
    pub state: ServiceState,
    pub key_set_at: Option<Timestamp>,
    pub from_address: String,
    pub from_name: String,
    pub reply_to: Option<String>,
    pub link_base: Option<String>,
    pub example_link: Option<String>,
    pub sink_path: Option<String>,
    pub daily_limit: u32,
    pub sent_today: u32,
    pub queued: u32,
    pub failed: u32,
    pub last_test_at: Option<Timestamp>,
    pub last_error: Option<String>,
    pub last_error_at: Option<Timestamp>,
}

#[derive(Debug, Clone, Serialize)]
pub struct MailOutboxList {
    pub mails: Vec<Entry>,
    pub total: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct TestSent {
    pub id: String,
    pub to: String,
}

#[derive(Debug, Clone)]
pub enum KeyChange {
    Keep,
    Remove,
    Replace(String),
}

enum Checked {
    Keep,
    Remove,
    Write(ApiKey),
}

enum Provider {
    Nowhere,
    Files,
    Resend(ApiKey),
}

pub struct Mail {
    pool: SqlitePool,
    keys: KeyFile,
    resend: Resend,
    sink: Option<Sink>,
    wake: Notify,
}

impl Mail {
    pub fn new(pool: SqlitePool, config: Arc<Config>) -> anyhow::Result<Arc<Self>> {
        let sink = Sink::from_env();
        if let Some(sink) = &sink {
            tracing::warn!(
                path = %sink.dir().display(),
                "{} is set: mail goes to files, not to Resend",
                sink::VARIABLE
            );
        }
        Ok(Arc::new(Self {
            pool,
            keys: KeyFile::in_dir(config.data_dir.join("mail")),
            resend: Resend::new()?,
            sink,
            wake: Notify::new(),
        }))
    }

    #[cfg(test)]
    pub(crate) fn against(
        pool: SqlitePool,
        dir: std::path::PathBuf,
        base: &str,
        sink: Option<Sink>,
    ) -> Arc<Self> {
        Arc::new(Self {
            pool,
            keys: KeyFile::in_dir(dir.join("mail")),
            resend: Resend::against(base).expect("a test client"),
            sink,
            wake: Notify::new(),
        })
    }

    pub fn start(self: &Arc<Self>) -> tokio::task::JoinHandle<()> {
        queue::spawn(Arc::clone(self))
    }

    pub async fn configured(&self) -> bool {
        !matches!(self.provider().await, Provider::Nowhere)
    }

    pub async fn can_link(&self) -> bool {
        if !self.configured().await {
            return false;
        }
        matches!(store::load(&self.pool).await, Ok(settings) if settings.link_base.is_some())
    }

    pub async fn send(&self, message: Message) -> Result<Id, MailError> {
        let kind = message.kind();
        let to = message.to().address.clone();
        match self.enqueue(message, Timestamp::now()).await {
            Ok(id) => Ok(id),
            Err(err) => {
                tracing::warn!(
                    %kind, to = %to, code = err.code(),
                    "no mail was queued: {}", err.message()
                );
                Err(err)
            }
        }
    }

    pub async fn notify(&self, message: Message) {
        let _ = self.send(message).await;
    }

    pub async fn settings(&self) -> Result<MailSettings, Failure> {
        self.settings_at(Timestamp::now()).await
    }

    pub async fn settings_at(&self, now: Timestamp) -> Result<MailSettings, Failure> {
        let row = store::load(&self.pool).await?;
        Ok(self.overview(row, now).await?)
    }

    pub async fn save(
        &self,
        form: Form,
        key: KeyChange,
        now: Timestamp,
    ) -> Result<MailSettings, Failure> {
        let form = clean(form)?;
        let key = match key {
            KeyChange::Keep => Checked::Keep,
            KeyChange::Remove => Checked::Remove,
            KeyChange::Replace(text) => Checked::Write(ApiKey::parse(&text).ok_or_else(|| {
                Failure::invalid_request(
                    "That is not a key. Resend's keys begin with `re_` and carry no spaces.",
                )
            })?),
        };

        store::save(&self.pool, &form, now).await?;
        match key {
            Checked::Keep => {}
            Checked::Remove => self.forget_key().await?,
            Checked::Write(key) => {
                self.keys.write(&key).await.map_err(|err| {
                    Failure::internal(anyhow::anyhow!("writing the mail key: {err}"))
                })?;
                store::mark_key(&self.pool, Some(now)).await?;
            }
        }

        self.settings_at(now).await
    }

    pub async fn forget_key(&self) -> Result<(), Failure> {
        self.keys
            .forget()
            .await
            .map_err(|err| Failure::internal(anyhow::anyhow!("removing the mail key: {err}")))?;
        store::mark_key(&self.pool, None).await?;
        Ok(())
    }

    pub async fn send_test(&self, to: &str, now: Timestamp) -> Result<TestSent, MailError> {
        let settings = store::load(&self.pool).await.map_err(database)?;
        let provider = self.provider().await;
        if matches!(provider, Provider::Nowhere) {
            tracing::warn!(to = %to, "a test mail was asked for while mail is not set up");
            return Err(MailError::NotConfigured);
        }

        let message = Message::Test { to: Recipient::address(to) };
        self.brake(&message, &settings, now).await?;

        let sender = sender_of(&settings);
        let mail = render::render(&message, &sender, now)?;
        let id = Id::new();

        store::insert(
            &self.pool,
            &NewMail {
                id,
                kind: Kind::Test,
                user: None,
                to_address: to,
                subject: &mail.subject,
                html: &mail.html,
                text: &mail.text,
                state: State::Sending,
                created_at: now,
            },
        )
        .await
        .map_err(database)?;

        match self.hand_over(&provider, id, Kind::Test, to, &sender, &mail, now).await {
            Ok(receipt) => {
                store::mark_sent(&self.pool, id, &receipt, now).await.map_err(database)?;
                store::mark_test(&self.pool, now).await.map_err(database)?;
                store::mark_error(&self.pool, None, now).await.map_err(database)?;
                Ok(TestSent { id: receipt, to: to.to_owned() })
            }
            Err(err) => {
                let sentence = err.message();
                store::mark_failed(&self.pool, id, &sentence).await.map_err(database)?;
                store::mark_error(&self.pool, Some(&sentence), now).await.map_err(database)?;
                Err(err)
            }
        }
    }

    pub async fn outbox(
        &self,
        limit: u32,
        state: Option<State>,
    ) -> Result<MailOutboxList, Failure> {
        let (mails, total) = store::page(&self.pool, limit, state).await?;
        Ok(MailOutboxList { mails, total })
    }

    pub async fn content(&self, id: Id) -> Result<String, Failure> {
        match store::content(&self.pool, id).await? {
            None => Err(Failure::not_found("mail_not_found", "no such mail")),
            Some(None) => Err(Failure::not_found(
                "mail_content_gone",
                "This mail was delivered, so its body was cleared — it carried a link that is a \
                 secret while it is in flight.",
            )),
            Some(Some(html)) => Ok(html),
        }
    }

    pub async fn retry(&self, id: Id) -> Result<(), Failure> {
        if store::requeue(&self.pool, id).await? {
            self.wake.notify_one();
            return Ok(());
        }

        match store::content(&self.pool, id).await? {
            None => Err(Failure::not_found("mail_not_found", "no such mail")),
            Some(None) => Err(Failure::not_found(
                "mail_content_gone",
                "This mail has no body any more — it was delivered, and there is nothing left to \
                 send again.",
            )),
            Some(Some(_)) => Err(Failure::conflict(
                "invalid_state",
                "Only a mail that failed can be tried again.",
            )),
        }
    }

    pub(crate) async fn deliver_next(&self, now: Timestamp) -> Result<bool, sqlx::Error> {
        let Some(row) = store::claim_due(&self.pool, now).await? else {
            return Ok(false);
        };

        let settings = store::load(&self.pool).await?;
        let sender = sender_of(&settings);
        let provider = self.provider().await;
        let mail = Rendered {
            subject: row.subject.clone(),
            html: row.html.clone(),
            text: row.text.clone(),
        };

        let outcome = self
            .hand_over(&provider, row.id, row.kind, &row.to_address, &sender, &mail, now)
            .await;

        match outcome {
            Ok(receipt) => {
                store::mark_sent(&self.pool, row.id, &receipt, now).await?;
                store::mark_error(&self.pool, None, now).await?;
                tracing::info!(kind = %row.kind, to = %row.to_address, "mail sent");
            }
            Err(err) => self.write_off(&row, err, now).await?,
        }

        Ok(true)
    }

    async fn write_off(
        &self,
        row: &Queued,
        err: MailError,
        now: Timestamp,
    ) -> Result<(), sqlx::Error> {
        let sentence = err.message();
        let attempts = row.attempts + 1;

        let again = if err.until_tomorrow() {
            Some(queue::next_utc_day(now))
        } else if err.transient() {
            queue::next_attempt(attempts, now)
        } else {
            None
        };

        match again {
            Some(due) => {
                store::mark_waiting(&self.pool, row.id, due, &sentence).await?;
                tracing::warn!(
                    kind = %row.kind, to = %row.to_address, attempts, due = %due,
                    "mail waits: {sentence}"
                );
            }
            None => {
                store::mark_failed(&self.pool, row.id, &sentence).await?;
                tracing::error!(
                    kind = %row.kind, to = %row.to_address, attempts, code = err.code(),
                    "mail failed for good: {sentence}"
                );
            }
        }

        store::mark_error(&self.pool, Some(&sentence), now).await
    }

    pub(crate) async fn requeue_stuck(&self) -> Result<u64, sqlx::Error> {
        store::requeue_stuck(&self.pool).await
    }

    pub(crate) async fn wait_for_work(&self, tick: Duration) {
        tokio::select! {
            _ = self.wake.notified() => {}
            _ = tokio::time::sleep(tick) => {}
        }
    }

    async fn provider(&self) -> Provider {
        if self.sink.is_some() {
            return Provider::Files;
        }
        match self.keys.read().await {
            Some(key) => Provider::Resend(key),
            None => Provider::Nowhere,
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn hand_over(
        &self,
        provider: &Provider,
        id: Id,
        kind: Kind,
        to: &str,
        sender: &Sender,
        mail: &Rendered,
        now: Timestamp,
    ) -> Result<String, MailError> {
        match provider {
            Provider::Nowhere => Err(MailError::NotConfigured),
            Provider::Files => {
                let sink = self.sink.as_ref().expect("the sink is what made this the provider");
                sink.write(id, kind, now, mail).await.map(|stem| format!("file:{stem}")).map_err(
                    |err| MailError::Upstream(format!("the mail could not be written: {err}")),
                )
            }
            Provider::Resend(key) => {
                let from = sender.header();
                let outgoing = Outgoing {
                    from: &from,
                    to,
                    reply_to: sender.reply_to.as_deref(),
                    subject: &mail.subject,
                    html: &mail.html,
                    text: &mail.text,
                    kind: kind.as_str(),
                };
                self.resend.send(key, &outgoing, &id.to_string()).await
            }
        }
    }

    async fn enqueue(&self, message: Message, now: Timestamp) -> Result<Id, MailError> {
        let settings = store::load(&self.pool).await.map_err(database)?;
        if matches!(self.provider().await, Provider::Nowhere) {
            return Err(MailError::NotConfigured);
        }

        self.brake(&message, &settings, now).await?;

        let mail = render::render(&message, &sender_of(&settings), now)?;
        let id = Id::new();
        let to = message.to();

        store::insert(
            &self.pool,
            &NewMail {
                id,
                kind: message.kind(),
                user: to.user,
                to_address: &to.address,
                subject: &mail.subject,
                html: &mail.html,
                text: &mail.text,
                state: State::Queued,
                created_at: now,
            },
        )
        .await
        .map_err(database)?;

        self.wake.notify_one();
        Ok(id)
    }

    async fn brake(
        &self,
        message: &Message,
        settings: &Settings,
        now: Timestamp,
    ) -> Result<(), MailError> {
        let kind = message.kind();
        let address = &message.to().address;
        let hour_ago = Timestamp::at(now.as_datetime() - BRAKE_WINDOW);

        let to_address =
            store::recent_to(&self.pool, address, kind, hour_ago).await.map_err(database)?;
        if to_address >= PER_ADDRESS_AND_KIND {
            return Err(MailError::Braked(format!(
                "{PER_ADDRESS_AND_KIND} mails of this kind went to this address in the last hour. \
                 Try again later."
            )));
        }

        if kind == Kind::Test {
            let tests =
                store::recent_of_kind(&self.pool, kind, hour_ago).await.map_err(database)?;
            if tests >= TEST_MAILS {
                return Err(MailError::Braked(format!(
                    "{TEST_MAILS} test mails in an hour is enough — every one of them is a real \
                     mail on Resend's bill. Try again later."
                )));
            }
        }

        if settings.daily_limit > 0 {
            let day_ago = Timestamp::at(now.as_datetime() - DAY);
            let spent = store::spent_since(&self.pool, day_ago).await.map_err(database)?;
            if spent >= settings.daily_limit {
                return Err(MailError::QuotaReached(format!(
                    "The panel's own daily limit of {} mails is used up. Raise it under \
                     Administration → Mail, or wait — Resend's free tier gives 100 a day.",
                    settings.daily_limit
                )));
            }
        }

        Ok(())
    }

    async fn overview(&self, row: Settings, now: Timestamp) -> Result<MailSettings, sqlx::Error> {
        let state = match self.provider().await {
            Provider::Nowhere => ServiceState::NotConfigured,
            Provider::Files => ServiceState::FileSink,
            Provider::Resend(_) => ServiceState::Configured,
        };
        let Counts { sent_today, queued, failed } =
            store::counts(&self.pool, Timestamp::at(now.as_datetime() - DAY)).await?;

        Ok(MailSettings {
            provider: "resend",
            state,
            example_link: row.link_base.as_deref().map(|base| format!("{base}/verify-email#…")),
            sink_path: self.sink.as_ref().map(|sink| sink.dir().display().to_string()),
            key_set_at: row.key_set_at,
            from_address: row.from_address,
            from_name: row.from_name,
            reply_to: row.reply_to,
            link_base: row.link_base,
            daily_limit: row.daily_limit,
            sent_today,
            queued,
            failed,
            last_test_at: row.last_test_at,
            last_error: row.last_error,
            last_error_at: row.last_error_at,
        })
    }
}

pub fn spawn_purge(pool: SqlitePool) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(DAY);
        loop {
            tick.tick().await;
            let cutoff = Timestamp::at(Timestamp::now().as_datetime() - RETENTION);
            match store::purge(&pool, cutoff).await {
                Ok(0) => {}
                Ok(gone) => tracing::info!(gone, "mail older than 30 days removed"),
                Err(err) => tracing::error!("the mail outbox could not be swept: {err}"),
            }
        }
    })
}

fn sender_of(settings: &Settings) -> Sender {
    Sender {
        name: settings.from_name.clone(),
        address: settings.from_address.clone(),
        reply_to: settings.reply_to.clone(),
        link_base: settings.link_base.clone(),
    }
}

fn database(err: sqlx::Error) -> MailError {
    MailError::Upstream(format!("the mail queue could not be reached: {err}"))
}

impl From<MailError> for Failure {
    fn from(err: MailError) -> Self {
        Failure::new(err.status(), err.code(), err.message())
    }
}

pub fn clean_header(text: &str) -> String {
    text.chars().filter(|ch| !ch.is_control()).collect::<String>().trim().to_owned()
}

pub fn clean_name(text: &str) -> String {
    clean_header(text).chars().filter(|ch| !matches!(ch, '<' | '>' | '"' | ',')).collect()
}

pub fn plausible_address(address: &str) -> bool {
    let mut parts = address.split('@');
    let (Some(local), Some(domain), None) = (parts.next(), parts.next(), parts.next()) else {
        return false;
    };
    !local.is_empty()
        && !domain.is_empty()
        && domain.contains('.')
        && address.len() <= 254
        && !address.chars().any(char::is_whitespace)
}

fn clean(form: Form) -> Result<Form, Failure> {
    let from_address = clean_header(&form.from_address);
    if !plausible_address(&from_address) {
        return Err(Failure::invalid_request(
            "The sender needs to be an address of the form name@domain.tld.",
        ));
    }

    let reply_to = match form.reply_to.map(|text| clean_header(&text)) {
        None => None,
        Some(text) if text.is_empty() => None,
        Some(text) if plausible_address(&text) => Some(text),
        Some(_) => {
            return Err(Failure::invalid_request(
                "The reply address needs to be an address of the form name@domain.tld, or empty.",
            ))
        }
    };

    let link_base = match form.link_base.map(|text| clean_header(&text)) {
        None => None,
        Some(text) if text.is_empty() => None,
        Some(text) => {
            let base = text.trim_end_matches('/').to_owned();
            if !base.starts_with("https://") && !base.starts_with("http://") {
                return Err(Failure::invalid_request(
                    "The panel address needs a scheme: https://panel.example (or http:// on a \
                     home network).",
                ));
            }
            Some(base)
        }
    };

    Ok(Form {
        from_address,
        from_name: clean_name(&form.from_name),
        reply_to,
        link_base,
        daily_limit: form.daily_limit,
    })
}

#[cfg(test)]
mod tests;
