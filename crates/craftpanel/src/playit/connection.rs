use std::sync::{Arc, Mutex};

use sqlx::SqlitePool;
use tokio::sync::Semaphore;
use tokio::task::JoinHandle;

use crate::auth::error::{Failure, Result};
use crate::model::{Id, Timestamp};

use super::agent::{Agent, AgentStatus};
use super::claim::{ClaimState, Step};
use super::http::{Http, PlayitError};
use super::store::{self, Claimed, TunnelState};
use super::tunnels::{self, Form};
use super::{
    claim, expires, limit_of, not_configured, upstream, view_of, PlayitClaim, PlayitStatus, Ports,
    Secret, ServerTunnel, Tunnels, CLAIM_TICK, SETTLE_LIMIT, SETTLE_TICK, SYNC_CEILING, SYNC_TICK,
};

pub struct Connection {
    pool: SqlitePool,
    user: Id,
    http: Http,
    agent: Arc<Agent>,
    secret: tokio::sync::RwLock<Option<Secret>>,
    form: Arc<Mutex<Form>>,
    claim_slots: Arc<Semaphore>,
    claiming: tokio::sync::Mutex<Option<JoinHandle<()>>>,
    syncing: tokio::sync::Mutex<Option<JoinHandle<()>>>,
    gate: tokio::sync::Mutex<()>,
}

impl Connection {
    pub fn new(
        pool: SqlitePool,
        user: Id,
        http: Http,
        agent: Arc<Agent>,
        form: Arc<Mutex<Form>>,
        claim_slots: Arc<Semaphore>,
    ) -> Arc<Self> {
        Arc::new(Self {
            pool,
            user,
            http,
            agent,
            secret: tokio::sync::RwLock::default(),
            form,
            claim_slots,
            claiming: tokio::sync::Mutex::default(),
            syncing: tokio::sync::Mutex::default(),
            gate: tokio::sync::Mutex::default(),
        })
    }

    pub fn agent_status(&self) -> AgentStatus {
        self.agent.status()
    }

    pub fn secret_path(&self) -> std::path::PathBuf {
        self.agent.secret_path()
    }

    pub async fn wake(self: &Arc<Self>) -> anyhow::Result<()> {
        let known = store::account(&self.pool, self.user).await?;
        if let Some(claim) = known.and_then(|row| row.claim) {
            if Timestamp::now() < expires(claim.started_at) {
                self.watch_claim(claim.code, claim.started_at).await;
            } else {
                store::clear_claim(&self.pool, self.user).await?;
            }
        }

        if let Some(secret) = self.agent.read_secret().await {
            *self.secret.write().await = Some(secret);
            self.engage().await;
        }
        Ok(())
    }

    pub async fn status(&self) -> Result<PlayitStatus> {
        let account = store::account(&self.pool, self.user).await?.unwrap_or_default();

        Ok(PlayitStatus {
            configured: self.secret().await.is_some(),
            agent_id: account.agent_id,
            account_status: account.account_status.map(tunnels::AccountStatus::as_str),
            is_self_managed: account.is_self_managed,
            has_premium: account.has_premium,
            agent: self.agent.status(),
            binary: self.agent.binary(),
            ports: self.ports(account.has_premium).await?,
            claim: account.claim.map(view_of),
            last_error: account.last_error,
            checked_at: account.checked_at,
        })
    }

    pub async fn ports(&self, has_premium: bool) -> Result<Ports> {
        Ok(Ports {
            used: store::used(&self.pool, self.user).await?,
            limit: limit_of(has_premium),
            for_others: store::for_others(&self.pool, self.user).await?,
        })
    }

    pub async fn claim(&self) -> Result<Option<PlayitClaim>> {
        Ok(store::account(&self.pool, self.user)
            .await?
            .and_then(|row| row.claim)
            .map(view_of))
    }

    pub async fn configured(&self) -> bool {
        self.secret().await.is_some()
    }

    pub async fn has_secret(&self) -> bool {
        tokio::fs::metadata(self.agent.secret_path()).await.is_ok()
    }

    pub async fn begin_claim(self: &Arc<Self>) -> Result<PlayitClaim> {
        self.require_external().await?;
        if self.secret().await.is_some() {
            return Err(Failure::conflict(
                "playit_already_claimed",
                "you are already signed in to playit.gg",
            ));
        }

        self.stop_claim().await;

        let code = claim::generate();
        let started_at = Timestamp::now();
        claim::setup(&self.http, &code).await.map_err(upstream)?;

        let row = store::Claim {
            code: code.clone(),
            state: ClaimState::WaitingForVisit,
            started_at,
        };
        store::begin_claim(&self.pool, self.user, &row).await?;
        self.watch_claim(code, started_at).await;

        Ok(view_of(row))
    }

    pub async fn cancel_claim(&self) -> Result<()> {
        if !store::clear_claim(&self.pool, self.user).await? {
            return Err(Failure::not_found(
                "playit_claim_not_found",
                "no sign-up is under way",
            ));
        }
        self.stop_claim().await;
        Ok(())
    }

    async fn watch_claim(self: &Arc<Self>, code: String, started_at: Timestamp) {
        let mut claiming = self.claiming.lock().await;
        if let Some(task) = claiming.take() {
            task.abort();
        }
        *claiming = Some(tokio::spawn(Arc::clone(self).poll_claim(code, started_at)));
    }

    async fn stop_claim(&self) {
        if let Some(task) = self.claiming.lock().await.take() {
            task.abort();
        }
    }

    async fn poll_claim(self: Arc<Self>, mut code: String, started_at: Timestamp) {
        let Ok(_turn) = Arc::clone(&self.claim_slots).acquire_owned().await else { return };
        let deadline = expires(started_at);
        let mut wait = CLAIM_TICK;

        loop {
            tokio::time::sleep(wait).await;
            wait = CLAIM_TICK;

            if Timestamp::now() >= deadline {
                let _ = store::clear_claim(&self.pool, self.user).await;
                return;
            }
            let running = store::account(&self.pool, self.user).await.is_ok_and(|account| {
                account.is_some_and(|row| row.claim.is_some_and(|claim| claim.code == code))
            });
            if !running {
                return;
            }

            let outcome = claim::setup(&self.http, &code).await;
            if matches!(outcome, Err(PlayitError::RateLimited)) {
                wait = SYNC_CEILING;
            }
            if let Ok(state) = &outcome {
                let _ = store::advance_claim(&self.pool, self.user, &code, *state).await;
            }

            let step = match claim::after_setup(&outcome) {
                Step::Fetch => match claim::exchange(&self.http, &code).await {
                    Ok(secret) => {
                        self.adopt(secret).await;
                        return;
                    }
                    Err(err) => claim::after_exchange(&err),
                },
                other => other,
            };

            match step {
                Step::Wait | Step::Fetch => continue,
                Step::Renew => {
                    code = claim::generate();
                    let row = store::Claim {
                        code: code.clone(),
                        state: ClaimState::WaitingForVisit,
                        started_at,
                    };
                    if store::begin_claim(&self.pool, self.user, &row).await.is_err() {
                        return;
                    }
                }
                Step::Stop(why) => {
                    let _ = store::record_error(&self.pool, self.user, &why).await;
                    let _ = store::clear_claim(&self.pool, self.user).await;
                    return;
                }
            }
        }
    }

    async fn adopt(self: &Arc<Self>, secret: Secret) {
        if let Err(err) = self.agent.write_secret(&secret).await {
            let _ = store::record_error(
                &self.pool,
                self.user,
                &format!("the playit key could not be stored: {err}"),
            )
            .await;
            return;
        }

        *self.secret.write().await = Some(secret.clone());
        let _ = store::clear_claim(&self.pool, self.user).await;
        tracing::info!(user = %self.user, "playit.gg accepted the sign-up");

        let _ = self.agent_id(&secret).await;

        self.agent.stop().await;
        self.engage().await;
    }

    pub async fn restart_agent(self: &Arc<Self>) -> Result<()> {
        self.require_external().await?;
        if self.secret().await.is_none() {
            return Err(not_configured());
        }
        self.agent.stop().await;
        self.tune_agent().await;
        Ok(())
    }

    pub async fn disconnect(self: &Arc<Self>, mode: Tunnels) -> Result<()> {
        let rows = store::tunnels(&self.pool, self.user).await?;

        if mode == Tunnels::Refuse && !rows.is_empty() {
            return Err(Failure::conflict(
                "playit_has_tunnels",
                "servers still have a public address; say whether to delete those \
                 tunnels on playit.gg or leave them there",
            ));
        }

        if mode == Tunnels::Delete {
            self.require_external().await?;
            let secret = self.secret().await.ok_or_else(not_configured)?;
            for row in rows.iter().filter_map(|row| row.tunnel_id.as_deref()) {
                tunnels::delete(&self.http, &secret, row).await.map_err(upstream)?;
            }
        }

        self.stop_claim().await;
        self.stop_sync().await;
        self.agent.stop().await;
        self.agent
            .forget_secret()
            .await
            .map_err(|err| Failure::internal(anyhow::anyhow!("{err}")))?;
        *self.secret.write().await = None;

        store::forget_all(&self.pool, self.user).await?;
        store::abandon_released(&self.pool, self.user).await?;
        store::forget_account(&self.pool, self.user).await?;
        Ok(())
    }

    async fn tune_agent(self: &Arc<Self>) {
        let _one_at_a_time = self.gate.lock().await;

        let wanted = self.secret().await.is_some()
            && self.external_on().await
            && store::used(&self.pool, self.user).await.is_ok_and(|used| used > 0);

        if wanted {
            self.agent.start().await;
        } else {
            self.agent.stop().await;
        }
    }

    async fn engage(self: &Arc<Self>) {
        self.tune_agent().await;

        let mut syncing = self.syncing.lock().await;
        if syncing.as_ref().is_some_and(|task| !task.is_finished()) {
            return;
        }
        *syncing = Some(tokio::spawn(Arc::clone(self).sync_forever()));
    }

    async fn stop_sync(&self) {
        if let Some(task) = self.syncing.lock().await.take() {
            task.abort();
        }
    }

    async fn sync_forever(self: Arc<Self>) {
        tokio::time::sleep(offset_of(self.user)).await;
        let mut wait = SYNC_TICK;

        loop {
            let Some(secret) = self.secret().await else { return };
            self.tune_agent().await;

            if !self.external_on().await || !self.owes_anything().await {
                tokio::time::sleep(SYNC_TICK).await;
                continue;
            }

            match self.sync(&secret).await {
                Ok(()) => wait = SYNC_TICK,
                Err(err) if err.is_invalid_key() => {
                    let _ = store::record_error(
                        &self.pool,
                        self.user,
                        "playit.gg no longer accepts your key; connect again",
                    )
                    .await;
                    wait = SYNC_CEILING;
                }
                Err(err) => {
                    let _ = store::record_error(&self.pool, self.user, &err.to_string()).await;
                    wait = SYNC_CEILING.min(wait * 2);
                }
            }

            tokio::time::sleep(wait).await;
        }
    }

    async fn owes_anything(&self) -> bool {
        store::used(&self.pool, self.user).await.is_ok_and(|used| used > 0)
            || store::released(&self.pool, self.user).await.is_ok_and(|owed| !owed.is_empty())
    }

    async fn sync(&self, secret: &Secret) -> std::result::Result<(), PlayitError> {
        let run = tunnels::rundata(&self.http, secret).await?;
        let _ = store::record_identity(
            &self.pool,
            self.user,
            &run.agent_id,
            run.permissions.account_status,
            run.permissions.is_self_managed,
            run.permissions.has_premium,
        )
        .await;

        let theirs = tunnels::list(&self.http, secret).await?;
        let ours = store::tunnels(&self.pool, self.user)
            .await
            .map_err(|err| PlayitError::Unreadable(err.to_string()))?;

        for row in ours {
            let Some(id) = row.tunnel_id.as_deref() else { continue };

            let (state, addresses, detail) = match theirs.iter().find(|view| view.id == id) {
                Some(view) if view.is_online() => {
                    (TunnelState::Online, view.addresses.clone(), None)
                }
                Some(view) => (TunnelState::Offline, view.addresses.clone(), view.detail()),
                None => (
                    TunnelState::Missing,
                    Vec::new(),
                    Some("The tunnel was removed on playit.gg.".to_owned()),
                ),
            };
            let _ =
                store::set_state(&self.pool, row.server_id, state, &addresses, detail.as_deref())
                    .await;
        }

        self.hand_back(secret).await;
        Ok(())
    }

    async fn hand_back(&self, secret: &Secret) {
        let Ok(owed) = store::released(&self.pool, self.user).await else { return };

        for tunnel_id in owed {
            match tunnels::delete(&self.http, secret, &tunnel_id).await {
                Ok(()) => {
                    let _ = store::settled(&self.pool, &tunnel_id).await;
                }
                Err(_) => return,
            }
        }
    }

    pub async fn request_tunnel(
        self: &Arc<Self>,
        server: Id,
        name: &str,
        local_port: u16,
    ) -> Result<ServerTunnel> {
        self.require_external().await?;
        if self.secret().await.is_none() {
            return Err(not_configured());
        }

        let account = store::account(&self.pool, self.user).await?.unwrap_or_default();
        let limit = limit_of(account.has_premium);
        match store::claim_slot(&self.pool, self.user, server, local_port, limit).await? {
            Claimed::Taken => {
                return Err(Failure::conflict(
                    "playit_tunnel_exists",
                    "this server already has a public address",
                ))
            }
            Claimed::Full => {
                return Err(Failure::conflict(
                    "playit_port_limit",
                    format!(
                        "your playit.gg account has no free port left ({limit} of {limit} in use)"
                    ),
                ))
            }
            Claimed::Ok => {}
        }

        self.engage().await;

        tokio::spawn(Arc::clone(self).build(server, name.to_owned(), local_port));
        self.tunnel(server).await
    }

    pub async fn tunnel(&self, server: Id) -> Result<ServerTunnel> {
        Ok(store::tunnel(&self.pool, server)
            .await?
            .map_or_else(ServerTunnel::none, Into::into))
    }

    pub async fn drop_tunnel(self: &Arc<Self>, server: Id) -> Result<()> {
        self.require_external().await?;

        let Some(row) = store::tunnel(&self.pool, server).await? else {
            return Err(Failure::not_found(
                "playit_tunnel_not_found",
                "this server has no public address",
            ));
        };

        if let Some(id) = row.tunnel_id.as_deref() {
            let secret = self.secret().await.ok_or_else(not_configured)?;
            tunnels::delete(&self.http, &secret, id).await.map_err(upstream)?;
            store::forget(&self.pool, server).await?;
            store::settled(&self.pool, id).await?;
        } else {
            store::forget(&self.pool, server).await?;
        }

        self.tune_agent().await;
        Ok(())
    }

    async fn build(self: Arc<Self>, server: Id, name: String, local_port: u16) {
        let Some(secret) = self.secret().await else {
            self.give_up(server, "you are not signed in to playit.gg").await;
            return;
        };

        let agent_id = match self.agent_id(&secret).await {
            Ok(agent_id) => agent_id,
            Err(err) => {
                self.give_up(server, &err.to_string()).await;
                return;
            }
        };

        let id = match self.create(&secret, &agent_id, &name, local_port).await {
            Ok(id) => id,
            Err(err) => {
                self.give_up(server, &err.to_string()).await;
                return;
            }
        };

        match store::attach(&self.pool, server, &id).await {
            Ok(true) => {}
            _ => {
                let _ = store::release(&self.pool, self.user, &id).await;
                return;
            }
        }

        self.settle(server, &secret, &id).await;
    }

    async fn create(
        &self,
        secret: &Secret,
        agent_id: &str,
        name: &str,
        local_port: u16,
    ) -> std::result::Result<String, PlayitError> {
        let form = *self.form.lock().expect("the create form lock");
        let first = tunnels::create(&self.http, secret, form, agent_id, name, local_port).await;

        let Err(err) = first else { return first };

        if err.is_not_self_managed() {
            tracing::error!(
                user = %self.user,
                "playit.gg says this agent is not self-managed, so it may not create \
                 tunnels; the account has to be connected again"
            );
        }
        if !err.is_validation() || form != Form::Rust {
            return Err(err);
        }

        let second =
            tunnels::create(&self.http, secret, Form::Java, agent_id, name, local_port).await;
        if second.is_ok() {
            tracing::info!("playit.gg wants the older create body; keeping to it");
            *self.form.lock().expect("the create form lock") = Form::Java;
        }
        second
    }

    async fn settle(&self, server: Id, secret: &Secret, id: &str) {
        let give_up = tokio::time::Instant::now() + SETTLE_LIMIT;
        let mut reason = None;

        while tokio::time::Instant::now() < give_up {
            tokio::time::sleep(SETTLE_TICK).await;

            let Ok(theirs) = tunnels::list(&self.http, secret).await else { continue };
            let Some(view) = theirs.iter().find(|view| view.id == id) else { continue };

            if view.is_online() {
                let _ = store::set_state(
                    &self.pool,
                    server,
                    TunnelState::Online,
                    &view.addresses,
                    None,
                )
                .await;
                return;
            }
            reason = view.detail();
        }

        let (state, detail) = match reason {
            Some(reason) => (TunnelState::Offline, reason),
            None => (
                TunnelState::Failed,
                "playit.gg did not hand out an address for this tunnel".to_owned(),
            ),
        };
        let _ = store::set_state(&self.pool, server, state, &[], Some(&detail)).await;
    }

    async fn give_up(&self, server: Id, why: &str) {
        let _ =
            store::set_state(&self.pool, server, TunnelState::Failed, &[], Some(why)).await;
    }

    async fn agent_id(&self, secret: &Secret) -> std::result::Result<String, PlayitError> {
        if let Ok(Some(id)) =
            store::account(&self.pool, self.user).await.map(|row| row.and_then(|row| row.agent_id))
        {
            return Ok(id);
        }

        let run = tunnels::rundata(&self.http, secret).await?;
        let _ = store::record_identity(
            &self.pool,
            self.user,
            &run.agent_id,
            run.permissions.account_status,
            run.permissions.is_self_managed,
            run.permissions.has_premium,
        )
        .await;
        Ok(run.agent_id)
    }

    pub async fn dispose(self: &Arc<Self>) {
        if self.disconnect(Tunnels::Delete).await.is_err() {
            let _ = self.disconnect(Tunnels::Keep).await;
        }
        self.stop_claim().await;
        self.stop_sync().await;
        self.agent.stop().await;
        if let Err(err) = self.agent.forget_everything().await {
            tracing::warn!(user = %self.user, "the playit directory is still there: {err}");
        }
    }

    pub async fn hand_back_one(&self, server: Id) {
        let Ok(Some(row)) = store::tunnel(&self.pool, server).await else { return };
        if row.user_id != self.user {
            return;
        }
        let Some(id) = row.tunnel_id.clone() else {
            let _ = store::forget(&self.pool, server).await;
            return;
        };

        let handed = match self.secret().await {
            Some(secret) if self.external_on().await => {
                tunnels::delete(&self.http, &secret, &id).await.is_ok()
            }
            _ => false,
        };

        let _ = store::forget(&self.pool, server).await;
        if handed {
            let _ = store::settled(&self.pool, &id).await;
        }
    }

    async fn secret(&self) -> Option<Secret> {
        if let Some(secret) = self.secret.read().await.clone() {
            return Some(secret);
        }

        let secret = self.agent.read_secret().await?;
        *self.secret.write().await = Some(secret.clone());
        Some(secret)
    }

    #[cfg(test)]
    pub async fn write_secret(&self, secret: &Secret) -> std::io::Result<()> {
        self.agent.write_secret(secret).await?;
        *self.secret.write().await = Some(secret.clone());
        Ok(())
    }

    async fn external_on(&self) -> bool {
        crate::auth::settings::load(&self.pool)
            .await
            .map(|settings| settings.external_services_enabled)
            .unwrap_or(false)
    }

    async fn require_external(&self) -> Result<()> {
        if self.external_on().await {
            return Ok(());
        }
        Err(Failure::conflict(
            "external_services_disabled",
            "this panel does not call out to other services",
        ))
    }
}

fn offset_of(user: Id) -> std::time::Duration {
    let text = user.to_string();
    let spread =
        text.bytes().fold(0u64, |sum, byte| sum.wrapping_mul(31).wrapping_add(byte as u64));
    std::time::Duration::from_secs(spread % SYNC_TICK.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_turns_of_two_users_do_not_fall_on_the_same_second() {
        let mut seen = std::collections::HashSet::new();
        let mut spread = 0;
        for _ in 0..64 {
            let offset = offset_of(Id::new());
            assert!(offset < SYNC_TICK, "an offset outside the tick is a skipped round");
            if seen.insert(offset.as_secs()) {
                spread += 1;
            }
        }
        assert!(spread > 8, "sixty-four users landed on {spread} different seconds");

        let same = Id::new();
        assert_eq!(offset_of(same), offset_of(same), "a restart must not reshuffle the turns");
    }
}
