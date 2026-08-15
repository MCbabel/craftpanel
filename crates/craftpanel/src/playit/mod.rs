#![allow(dead_code)]

pub mod agent;
pub mod claim;
pub mod connection;
pub mod http;
pub mod legacy;
pub mod store;
pub mod tunnels;

use std::collections::HashMap;
use std::fmt;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::http::StatusCode;
use serde::Serialize;
use sqlx::SqlitePool;
use tokio::sync::Semaphore;

use crate::auth::error::{Failure, Result};
use crate::config::Config;
use crate::model::{Id, Timestamp};

use self::agent::{Agent, AgentStatus, Binary, BinaryStatus};
use self::claim::ClaimState;
use self::connection::Connection;
use self::http::{Http, PlayitError};
use self::tunnels::{Address, Form};

const CLAIM_DEADLINE: Duration = Duration::from_secs(15 * 60);
const CLAIM_TICK: Duration = Duration::from_secs(2);
const SYNC_TICK: Duration = Duration::from_secs(30);
const SYNC_CEILING: Duration = Duration::from_secs(5 * 60);
const SETTLE_LIMIT: Duration = Duration::from_secs(60);
const SETTLE_TICK: Duration = Duration::from_secs(2);

const CLAIM_LOOPS: usize = 4;

const FREE_PORTS: u32 = 4;
const PREMIUM_PORTS: u32 = 16;

#[derive(Clone)]
pub struct Secret(String);

impl Secret {
    pub fn parse(text: &str) -> http::Result<Self> {
        let trimmed = text.trim();
        if trimmed.is_empty() || hex::decode(trimmed).is_err() {
            return Err(PlayitError::Unreadable(
                "playit.gg sent a key that is not hexadecimal".to_owned(),
            ));
        }
        Ok(Self(trimmed.to_owned()))
    }

    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(hidden)")
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct PlayitStatus {
    pub configured: bool,
    pub agent_id: Option<String>,
    pub account_status: Option<&'static str>,
    pub is_self_managed: bool,
    pub has_premium: bool,
    pub agent: AgentStatus,
    pub binary: BinaryStatus,
    pub ports: Ports,
    pub claim: Option<PlayitClaim>,
    pub last_error: Option<String>,
    pub checked_at: Option<Timestamp>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PlayitOverview {
    pub user_id: Id,
    pub username: Option<String>,
    pub configured: bool,
    pub account_status: Option<&'static str>,
    pub is_self_managed: bool,
    pub has_premium: bool,
    pub agent: AgentStatus,
    pub ports: Ports,
    pub last_error: Option<String>,
    pub checked_at: Option<Timestamp>,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct Ports {
    pub used: u32,
    pub limit: u32,
    pub for_others: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct PlayitClaim {
    pub code: String,
    pub url: String,
    pub state: ClaimState,
    pub started_at: Timestamp,
    pub expires_at: Timestamp,
}

#[derive(Debug, Clone, Serialize)]
pub struct ServerTunnel {
    pub state: &'static str,
    pub addresses: Vec<Address>,
    pub local_port: Option<u16>,
    pub detail: Option<String>,
    pub created_at: Option<Timestamp>,
    pub checked_at: Option<Timestamp>,
}

impl ServerTunnel {
    pub fn none() -> Self {
        Self {
            state: "none",
            addresses: Vec::new(),
            local_port: None,
            detail: None,
            created_at: None,
            checked_at: None,
        }
    }
}

impl From<store::Tunnel> for ServerTunnel {
    fn from(row: store::Tunnel) -> Self {
        Self {
            state: row.state.as_str(),
            addresses: row.addresses,
            local_port: Some(row.local_port),
            detail: row.detail,
            created_at: Some(row.created_at),
            checked_at: row.checked_at,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tunnels {
    Refuse,
    Delete,
    Keep,
}

pub struct Playit {
    pool: SqlitePool,
    dir: PathBuf,
    http: Http,
    binary: Arc<Binary>,
    form: Arc<Mutex<Form>>,
    claim_slots: Arc<Semaphore>,
    users: Mutex<HashMap<Id, Arc<Connection>>>,
}

impl Playit {
    pub fn new(pool: SqlitePool, config: Arc<Config>) -> anyhow::Result<Arc<Self>> {
        let http = Http::against(http::BASE).map_err(|err| anyhow::anyhow!("{err}"))?;
        let binary = Binary::new(&config.data_dir).map_err(|err| anyhow::anyhow!("{err}"))?;
        Ok(Self::with(pool, config, http, binary))
    }

    #[cfg(test)]
    pub(crate) fn against(
        pool: SqlitePool,
        config: Arc<Config>,
        base: &str,
    ) -> anyhow::Result<Arc<Self>> {
        let http = Http::against(base).map_err(|err| anyhow::anyhow!("{err}"))?;
        let binary =
            Binary::from_source(&config.data_dir, base).map_err(|err| anyhow::anyhow!("{err}"))?;
        Ok(Self::with(pool, config, http, binary))
    }

    fn with(
        pool: SqlitePool,
        config: Arc<Config>,
        http: Http,
        binary: Arc<Binary>,
    ) -> Arc<Self> {
        Arc::new(Self {
            pool,
            dir: config.data_dir.join("playit"),
            http,
            binary,
            form: Arc::new(Mutex::new(Form::Rust)),
            claim_slots: Arc::new(Semaphore::new(CLAIM_LOOPS)),
            users: Mutex::default(),
        })
    }

    pub fn of(&self, user: Id) -> Arc<Connection> {
        let mut users = self.users.lock().expect("the playit users lock");
        Arc::clone(users.entry(user).or_insert_with(|| {
            Connection::new(
                self.pool.clone(),
                user,
                self.http.clone(),
                Agent::new(self.dir.join(user.to_string()), Arc::clone(&self.binary)),
                Arc::clone(&self.form),
                Arc::clone(&self.claim_slots),
            )
        }))
    }

    pub fn start(self: &Arc<Self>) {
        let this = Arc::clone(self);
        tokio::spawn(async move {
            if let Err(err) = this.pick_up().await {
                tracing::warn!("playit could not be picked up: {err}");
            }
        });
    }

    async fn pick_up(self: &Arc<Self>) -> anyhow::Result<()> {
        legacy::adopt(&self.pool, &self.dir).await?;

        let mut woken = 0;
        for user in store::connected(&self.pool).await? {
            let connection = self.of(user);
            match connection.wake().await {
                Ok(()) => {
                    if connection.configured().await {
                        woken += 1;
                    }
                }
                Err(err) => tracing::warn!(%user, "playit could not be picked up: {err}"),
            }
        }
        if woken > 0 {
            tracing::info!(accounts = woken, "playit.gg accounts picked up");
        }
        Ok(())
    }

    pub async fn overview(&self) -> Result<Vec<PlayitOverview>> {
        let rows = store::overview(&self.pool).await?;
        let mut lines = Vec::with_capacity(rows.len());

        for row in rows {
            let connection = self.of(row.user_id);
            lines.push(PlayitOverview {
                user_id: row.user_id,
                username: row.username,
                configured: connection.has_secret().await,
                account_status: row.account_status.map(tunnels::AccountStatus::as_str),
                is_self_managed: row.is_self_managed,
                has_premium: row.has_premium,
                agent: connection.agent_status(),
                ports: Ports {
                    used: row.used,
                    limit: limit_of(row.has_premium),
                    for_others: row.for_others,
                },
                last_error: row.last_error,
                checked_at: row.checked_at,
            });
        }
        Ok(lines)
    }

    pub async fn tunnel(&self, server: Id) -> Result<ServerTunnel> {
        Ok(store::tunnel(&self.pool, server)
            .await?
            .map_or_else(ServerTunnel::none, Into::into))
    }

    pub async fn drop_tunnel(&self, server: Id) -> Result<()> {
        let Some(row) = store::tunnel(&self.pool, server).await? else {
            return Err(Failure::not_found(
                "playit_tunnel_not_found",
                "this server has no public address",
            ));
        };
        self.of(row.user_id).drop_tunnel(server).await
    }

    pub async fn dispose_of(&self, user: Id) {
        self.of(user).dispose().await;
        self.users.lock().expect("the playit users lock").remove(&user);
    }

    pub async fn release_tunnel(&self, server: Id) {
        let Ok(Some(row)) = store::tunnel(&self.pool, server).await else { return };
        self.of(row.user_id).hand_back_one(server).await;
    }
}

fn limit_of(has_premium: bool) -> u32 {
    if has_premium {
        PREMIUM_PORTS
    } else {
        FREE_PORTS
    }
}

fn expires(started_at: Timestamp) -> Timestamp {
    Timestamp::at(
        started_at.as_datetime() + time::Duration::seconds(CLAIM_DEADLINE.as_secs() as i64),
    )
}

fn view_of(claim: store::Claim) -> PlayitClaim {
    PlayitClaim {
        url: claim::url(&claim.code),
        code: claim.code,
        state: claim.state,
        started_at: claim.started_at,
        expires_at: expires(claim.started_at),
    }
}

fn not_configured() -> Failure {
    Failure::conflict("playit_not_configured", "no playit.gg account is connected")
}

fn upstream(err: PlayitError) -> Failure {
    match err {
        PlayitError::RateLimited => Failure::new(
            StatusCode::TOO_MANY_REQUESTS,
            "upstream_rate_limited",
            "playit.gg is turning us away for the moment; try again shortly",
        ),
        other => Failure::new(StatusCode::BAD_GATEWAY, "upstream_unavailable", other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::harness::{a_server, a_user, test_pool};
    use crate::model::Timestamp;

    fn service(pool: &SqlitePool, dir: &std::path::Path) -> Arc<Playit> {
        let mut config = Config::default();
        config.data_dir = dir.to_path_buf();
        Playit::against(pool.clone(), Arc::new(config), "http://127.0.0.1:1").unwrap()
    }

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("craftpanel-playit-{name}-{}", Id::new()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn the_key_does_not_print_itself() {
        let secret = Secret::parse("deadbeefcafe0011").unwrap();

        assert_eq!(format!("{secret:?}"), "Secret(hidden)");
        assert!(!format!("{secret:?}").contains("deadbeef"));
        assert_eq!(secret.expose(), "deadbeefcafe0011");
    }

    #[test]
    fn a_key_that_is_not_a_key_is_refused_before_it_is_written_anywhere() {
        assert!(Secret::parse("").is_err());
        assert!(Secret::parse("   ").is_err());
        for bad in ["not-hex", "zzzz", "abc"] {
            let err = Secret::parse(bad).unwrap_err();
            assert!(!err.to_string().contains(bad), "the refusal echoed it back: {err}");
        }
        assert_eq!(Secret::parse(" abcdef \n").unwrap().expose(), "abcdef");
    }

    #[tokio::test]
    async fn a_user_who_never_heard_of_playit_says_so_quietly() {
        let pool = test_pool().await;
        let dir = scratch("quiet");
        let playit = service(&pool, &dir);
        let anna = a_user(&pool, "anna").await;

        let status = playit.of(anna).status().await.unwrap();
        assert!(!status.configured);
        assert!(status.claim.is_none());
        assert!(status.agent_id.is_none());
        assert_eq!(status.ports.used, 0);
        assert_eq!(status.ports.limit, FREE_PORTS);
        assert_eq!(status.ports.for_others, 0);
        assert_eq!(status.agent.state, agent::AgentState::Absent);
        assert_eq!(status.binary.state, agent::BinaryState::Absent);
        assert!(playit.overview().await.unwrap().is_empty(), "nobody has connected anything");

        let json = serde_json::to_value(&status).unwrap();
        assert_eq!(json["binary"]["arch"], std::env::consts::ARCH);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn one_users_key_is_his_own_and_nowhere_near_the_others() {
        let pool = test_pool().await;
        let dir = scratch("two-keys");
        let playit = service(&pool, &dir);
        let anna = a_user(&pool, "anna").await;
        let ben = a_user(&pool, "ben").await;

        let hers = playit.of(anna);
        let his = playit.of(ben);
        assert_ne!(hers.secret_path(), his.secret_path());
        assert!(hers.secret_path().starts_with(dir.join("playit").join(anna.to_string())));

        hers.write_secret(&Secret::parse("aaaaaaaa").unwrap()).await.unwrap();

        assert!(hers.configured().await);
        assert!(!his.configured().await, "ben is signed in with anna's key");
        assert!(!his.secret_path().exists());
        assert!(hers.status().await.unwrap().configured);
        assert!(!his.status().await.unwrap().configured);

        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(hers.secret_path()).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);

        assert!(Arc::ptr_eq(&hers, &playit.of(anna)));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn a_full_account_does_not_stop_anybody_else_from_getting_his_first_address() {
        let pool = test_pool().await;
        let dir = scratch("ports");
        let playit = service(&pool, &dir);
        let anna = a_user(&pool, "anna").await;
        let ben = a_user(&pool, "ben").await;

        for index in 0..4u16 {
            let server = a_server(&pool, anna, &format!("anna-{index}"), 1024).await;
            store::claim_slot(&pool, anna, server, 25565 + index, 4).await.unwrap();
        }

        let hers = playit.of(anna).status().await.unwrap();
        assert_eq!((hers.ports.used, hers.ports.limit), (4, FREE_PORTS));
        let his = playit.of(ben).status().await.unwrap();
        assert_eq!((his.ports.used, his.ports.limit), (0, FREE_PORTS));

        let bens = a_server(&pool, ben, "survival", 1024).await;
        assert!(matches!(
            store::claim_slot(&pool, ben, bens, 25600, 4).await.unwrap(),
            store::Claimed::Ok
        ));

        let verified = tunnels::AccountStatus::Verified;
        store::record_identity(&pool, anna, "annas-agent", verified, true, true).await.unwrap();
        assert_eq!(playit.of(anna).status().await.unwrap().ports.limit, PREMIUM_PORTS);
        assert_eq!(
            playit.of(ben).status().await.unwrap().ports.limit,
            FREE_PORTS,
            "ben was given anna's plan"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn the_refusal_when_the_ports_are_gone_names_whose_account_is_full() {
        let pool = test_pool().await;
        let dir = scratch("full");
        let playit = service(&pool, &dir);
        let anna = a_user(&pool, "anna").await;
        let hers = playit.of(anna);
        hers.write_secret(&Secret::parse("aaaaaaaa").unwrap()).await.unwrap();

        for index in 0..4u16 {
            let server = a_server(&pool, anna, &format!("anna-{index}"), 1024).await;
            store::claim_slot(&pool, anna, server, 25565 + index, 4).await.unwrap();
        }

        let fifth = a_server(&pool, anna, "fifth", 1024).await;
        let refused = hers.request_tunnel(fifth, "fifth", 25569).await.unwrap_err();
        assert_eq!(refused.code(), "playit_port_limit");
        assert!(refused.to_string().contains("your playit.gg account"), "{refused}");
        assert!(refused.to_string().contains("4 of 4"), "{refused}");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn a_server_without_a_tunnel_is_a_state_and_not_an_error() {
        let pool = test_pool().await;
        let dir = scratch("none");
        let playit = service(&pool, &dir);
        let anna = a_user(&pool, "anna").await;
        let server = a_server(&pool, anna, "survival", 1024).await;

        let view = playit.tunnel(server).await.unwrap();
        assert_eq!(view.state, "none");
        assert!(view.addresses.is_empty());
        assert!(view.local_port.is_none());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn nothing_that_changes_anything_runs_while_outside_services_are_off() {
        let pool = test_pool().await;
        let dir = scratch("offline");
        let playit = service(&pool, &dir);
        sqlx::query("UPDATE panel_settings SET external_services_enabled = 0 WHERE id = 1")
            .execute(&pool)
            .await
            .unwrap();
        let anna = a_user(&pool, "anna").await;
        let hers = playit.of(anna);

        let refused = hers.begin_claim().await.unwrap_err();
        assert_eq!(refused.code(), "external_services_disabled");
        assert_eq!(refused.status(), StatusCode::CONFLICT);

        let server = a_server(&pool, anna, "survival", 1024).await;
        let refused = hers.request_tunnel(server, "survival", 25565).await.unwrap_err();
        assert_eq!(refused.code(), "external_services_disabled");

        store::claim_slot(&pool, anna, server, 25565, 4).await.unwrap();
        store::attach(&pool, server, "c0ffee11").await.unwrap();
        let refused = playit.drop_tunnel(server).await.unwrap_err();
        assert_eq!(refused.code(), "external_services_disabled");

        let refused = hers.disconnect(Tunnels::Delete).await.unwrap_err();
        assert_eq!(refused.code(), "external_services_disabled");

        hers.disconnect(Tunnels::Keep).await.unwrap();
        assert_eq!(store::used(&pool, anna).await.unwrap(), 0);

        assert!(hers.status().await.is_ok());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn asking_for_an_address_without_signing_up_first_names_the_step_missed() {
        let pool = test_pool().await;
        let dir = scratch("unclaimed");
        let playit = service(&pool, &dir);
        let anna = a_user(&pool, "anna").await;
        let server = a_server(&pool, anna, "survival", 1024).await;

        let refused = playit.of(anna).request_tunnel(server, "survival", 25565).await.unwrap_err();
        assert_eq!(refused.code(), "playit_not_configured");
        assert_eq!(store::used(&pool, anna).await.unwrap(), 0, "nothing was written");

        let refused = playit.drop_tunnel(server).await.unwrap_err();
        assert_eq!(refused.code(), "playit_tunnel_not_found");
        assert_eq!(refused.status(), StatusCode::NOT_FOUND);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn cancelling_a_sign_up_that_is_not_running_is_a_404() {
        let pool = test_pool().await;
        let dir = scratch("nocancel");
        let playit = service(&pool, &dir);
        let anna = a_user(&pool, "anna").await;

        let refused = playit.of(anna).cancel_claim().await.unwrap_err();
        assert_eq!(refused.code(), "playit_claim_not_found");
        assert!(playit.of(anna).claim().await.unwrap().is_none());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn a_sign_up_that_playit_never_answers_is_a_gateway_error_and_leaves_no_row() {
        let pool = test_pool().await;
        let dir = scratch("unreachable");
        let playit = service(&pool, &dir);
        let anna = a_user(&pool, "anna").await;

        let refused = playit.of(anna).begin_claim().await.unwrap_err();
        assert_eq!(refused.code(), "upstream_unavailable");
        assert_eq!(refused.status(), StatusCode::BAD_GATEWAY);
        assert!(playit.of(anna).claim().await.unwrap().is_none(), "a failed start leaves nothing");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn a_connected_account_does_not_stop_the_next_person_from_signing_up() {
        let pool = test_pool().await;
        let dir = scratch("second-claim");
        let playit = service(&pool, &dir);
        let anna = a_user(&pool, "anna").await;
        let ben = a_user(&pool, "ben").await;

        playit.of(anna).write_secret(&Secret::parse("aaaaaaaa").unwrap()).await.unwrap();

        let hers = playit.of(anna).begin_claim().await.unwrap_err();
        assert_eq!(hers.code(), "playit_already_claimed", "anna has one and needs no second");

        let his = playit.of(ben).begin_claim().await.unwrap_err();
        assert_eq!(his.code(), "upstream_unavailable", "ben was refused for anna's account");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn the_deadline_is_fifteen_minutes_after_the_code_was_made() {
        let started = Timestamp::now();
        let claim = view_of(store::Claim {
            code: "34ddf358a8".to_owned(),
            state: ClaimState::WaitingForVisit,
            started_at: started,
        });

        assert_eq!(claim.url, "https://playit.gg/claim/34ddf358a8");
        assert_eq!(
            claim.expires_at.unix_seconds() - started.unix_seconds(),
            CLAIM_DEADLINE.as_secs() as i64
        );
    }

    #[tokio::test]
    async fn disconnecting_with_tunnels_still_on_the_books_asks_first() {
        let pool = test_pool().await;
        let dir = scratch("disconnect");
        let playit = service(&pool, &dir);
        let anna = a_user(&pool, "anna").await;
        let server = a_server(&pool, anna, "survival", 1024).await;

        store::claim_slot(&pool, anna, server, 25565, 4).await.unwrap();
        store::attach(&pool, server, "c0ffee11").await.unwrap();

        let refused = playit.of(anna).disconnect(Tunnels::Refuse).await.unwrap_err();
        assert_eq!(refused.code(), "playit_has_tunnels");

        playit.of(anna).disconnect(Tunnels::Keep).await.unwrap();
        assert_eq!(store::used(&pool, anna).await.unwrap(), 0);
        assert!(store::released(&pool, anna).await.unwrap().is_empty());
        assert!(!playit.of(anna).status().await.unwrap().configured);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn one_user_disconnecting_leaves_everybody_elses_key_and_rows_alone() {
        let pool = test_pool().await;
        let dir = scratch("keep-others");
        let playit = service(&pool, &dir);
        let anna = a_user(&pool, "anna").await;
        let ben = a_user(&pool, "ben").await;
        let hers = a_server(&pool, anna, "survival", 1024).await;
        let his = a_server(&pool, ben, "creative", 1024).await;

        playit.of(anna).write_secret(&Secret::parse("aaaaaaaa").unwrap()).await.unwrap();
        playit.of(ben).write_secret(&Secret::parse("bbbbbbbb").unwrap()).await.unwrap();
        store::claim_slot(&pool, anna, hers, 25565, 4).await.unwrap();
        store::claim_slot(&pool, ben, his, 25566, 4).await.unwrap();
        let verified = tunnels::AccountStatus::Verified;
        store::record_identity(&pool, ben, "bens-agent", verified, true, false).await.unwrap();

        playit.of(anna).disconnect(Tunnels::Keep).await.unwrap();

        assert!(!playit.of(anna).configured().await);
        assert!(playit.of(ben).configured().await, "ben's key went with anna's");
        assert!(playit.of(ben).secret_path().exists());
        assert_eq!(store::used(&pool, ben).await.unwrap(), 1, "ben's address was taken away");
        assert!(store::account(&pool, ben).await.unwrap().is_some());
        assert!(store::account(&pool, anna).await.unwrap().is_none());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn deleting_an_account_takes_its_playit_directory_with_it() {
        let pool = test_pool().await;
        let dir = scratch("dispose");
        let playit = service(&pool, &dir);
        let anna = a_user(&pool, "anna").await;
        let ben = a_user(&pool, "ben").await;
        let hers = a_server(&pool, anna, "survival", 1024).await;

        playit.of(anna).write_secret(&Secret::parse("aaaaaaaa").unwrap()).await.unwrap();
        playit.of(ben).write_secret(&Secret::parse("bbbbbbbb").unwrap()).await.unwrap();
        store::claim_slot(&pool, anna, hers, 25565, 4).await.unwrap();
        let annas_dir = dir.join("playit").join(anna.to_string());
        assert!(annas_dir.exists());

        playit.dispose_of(anna).await;

        assert!(!annas_dir.exists(), "her key is still on the disk");
        assert_eq!(store::used(&pool, anna).await.unwrap(), 0);
        assert!(playit.of(ben).secret_path().exists(), "ben's key went with hers");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn playits_own_words_reach_the_user() {
        let refused = upstream(PlayitError::Refused {
            kind: "auth".to_owned(),
            detail: "AgentNotSelfManaged".to_owned(),
        });
        assert_eq!(refused.status(), StatusCode::BAD_GATEWAY);
        assert!(refused.to_string().contains("AgentNotSelfManaged"), "{refused}");

        let limited = upstream(PlayitError::RateLimited);
        assert_eq!(limited.code(), "upstream_rate_limited");
        assert_eq!(limited.status(), StatusCode::TOO_MANY_REQUESTS);
    }
}
