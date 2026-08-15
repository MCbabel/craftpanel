use serde::Serialize;
use sqlx::SqlitePool;

use super::claim::ClaimState;
use super::tunnels::{AccountStatus, Address};
use crate::model::{Id, Timestamp};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TunnelState {
    Pending,
    Online,
    Offline,
    Missing,
    Failed,
}

impl TunnelState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Online => "online",
            Self::Offline => "offline",
            Self::Missing => "missing",
            Self::Failed => "failed",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "pending" => Some(Self::Pending),
            "online" => Some(Self::Online),
            "offline" => Some(Self::Offline),
            "missing" => Some(Self::Missing),
            "failed" => Some(Self::Failed),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct Account {
    pub agent_id: Option<String>,
    pub account_status: Option<AccountStatus>,
    pub is_self_managed: bool,
    pub has_premium: bool,
    pub claim: Option<Claim>,
    pub checked_at: Option<Timestamp>,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Claim {
    pub code: String,
    pub state: ClaimState,
    pub started_at: Timestamp,
}

#[derive(Debug, Clone)]
pub struct Tunnel {
    pub server_id: Id,
    pub user_id: Id,
    pub tunnel_id: Option<String>,
    pub local_port: u16,
    pub state: TunnelState,
    pub addresses: Vec<Address>,
    pub detail: Option<String>,
    pub created_at: Timestamp,
    pub checked_at: Option<Timestamp>,
}

#[derive(Debug, Clone)]
pub struct Connected {
    pub user_id: Id,
    pub username: Option<String>,
    pub agent_id: Option<String>,
    pub account_status: Option<AccountStatus>,
    pub is_self_managed: bool,
    pub has_premium: bool,
    pub used: u32,
    pub for_others: u32,
    pub last_error: Option<String>,
    pub checked_at: Option<Timestamp>,
}

type Row = (
    Option<String>,
    Option<String>,
    bool,
    bool,
    Option<String>,
    Option<String>,
    Option<Timestamp>,
    Option<Timestamp>,
    Option<String>,
);

pub async fn account(pool: &SqlitePool, user: Id) -> sqlx::Result<Option<Account>> {
    let row: Option<Row> = sqlx::query_as(
        "SELECT agent_id, account_status, is_self_managed, has_premium, claim_code, \
                claim_state, claim_started_at, checked_at, last_error \
           FROM playit_accounts WHERE user_id = ?",
    )
    .bind(user)
    .fetch_optional(pool)
    .await?;

    Ok(row.map(|row| {
        let (agent_id, status, self_managed, premium, code, state, started, checked, error) = row;
        let claim = match (code, state.as_deref().and_then(ClaimState::parse), started) {
            (Some(code), Some(state), Some(started_at)) => Some(Claim { code, state, started_at }),
            _ => None,
        };

        Account {
            agent_id,
            account_status: status.as_deref().and_then(AccountStatus::parse),
            is_self_managed: self_managed,
            has_premium: premium,
            claim,
            checked_at: checked,
            last_error: error,
        }
    }))
}

pub async fn connected(pool: &SqlitePool) -> sqlx::Result<Vec<Id>> {
    sqlx::query_scalar("SELECT user_id FROM playit_accounts ORDER BY user_id").fetch_all(pool).await
}

pub async fn overview(pool: &SqlitePool) -> sqlx::Result<Vec<Connected>> {
    type OverviewRow = (
        Id,
        Option<String>,
        Option<String>,
        Option<String>,
        bool,
        bool,
        Option<String>,
        Option<Timestamp>,
        i64,
        i64,
    );

    let rows: Vec<OverviewRow> = sqlx::query_as(
        "SELECT a.user_id, u.username, a.agent_id, a.account_status, a.is_self_managed, \
                a.has_premium, a.last_error, a.checked_at, \
                (SELECT count(*) FROM playit_tunnels t WHERE t.user_id = a.user_id), \
                (SELECT count(*) FROM playit_tunnels t JOIN servers s ON s.id = t.server_id \
                  WHERE t.user_id = a.user_id AND s.owner_id <> a.user_id) \
           FROM playit_accounts a LEFT JOIN users u ON u.id = a.user_id \
          ORDER BY u.username",
    )
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|row| Connected {
            user_id: row.0,
            username: row.1,
            agent_id: row.2,
            account_status: row.3.as_deref().and_then(AccountStatus::parse),
            is_self_managed: row.4,
            has_premium: row.5,
            last_error: row.6,
            checked_at: row.7,
            used: row.8 as u32,
            for_others: row.9 as u32,
        })
        .collect())
}

pub async fn begin_claim(pool: &SqlitePool, user: Id, claim: &Claim) -> sqlx::Result<()> {
    let now = Timestamp::now();
    sqlx::query(
        "INSERT INTO playit_accounts \
             (user_id, claim_code, claim_state, claim_started_at, updated_at) \
         VALUES (?, ?, ?, ?, ?) \
         ON CONFLICT(user_id) DO UPDATE \
            SET claim_code = excluded.claim_code, claim_state = excluded.claim_state, \
                claim_started_at = excluded.claim_started_at, last_error = NULL, \
                updated_at = excluded.updated_at",
    )
    .bind(user)
    .bind(&claim.code)
    .bind(claim.state.as_str())
    .bind(claim.started_at)
    .bind(now)
    .execute(pool)
    .await
    .map(drop)
}

pub async fn advance_claim(
    pool: &SqlitePool,
    user: Id,
    code: &str,
    state: ClaimState,
) -> sqlx::Result<bool> {
    let done = sqlx::query(
        "UPDATE playit_accounts SET claim_state = ?, updated_at = ? \
          WHERE user_id = ? AND claim_code = ?",
    )
    .bind(state.as_str())
    .bind(Timestamp::now())
    .bind(user)
    .bind(code)
    .execute(pool)
    .await?;

    Ok(done.rows_affected() == 1)
}

pub async fn clear_claim(pool: &SqlitePool, user: Id) -> sqlx::Result<bool> {
    let done = sqlx::query(
        "UPDATE playit_accounts \
            SET claim_code = NULL, claim_state = NULL, claim_started_at = NULL, updated_at = ? \
          WHERE user_id = ? AND claim_code IS NOT NULL",
    )
    .bind(Timestamp::now())
    .bind(user)
    .execute(pool)
    .await?;

    Ok(done.rows_affected() == 1)
}

pub async fn record_identity(
    pool: &SqlitePool,
    user: Id,
    agent_id: &str,
    status: AccountStatus,
    is_self_managed: bool,
    has_premium: bool,
) -> sqlx::Result<()> {
    let now = Timestamp::now();
    sqlx::query(
        "INSERT INTO playit_accounts \
             (user_id, agent_id, account_status, is_self_managed, has_premium, checked_at, \
              updated_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?) \
         ON CONFLICT(user_id) DO UPDATE \
            SET agent_id = excluded.agent_id, account_status = excluded.account_status, \
                is_self_managed = excluded.is_self_managed, \
                has_premium = excluded.has_premium, checked_at = excluded.checked_at, \
                last_error = NULL, updated_at = excluded.updated_at",
    )
    .bind(user)
    .bind(agent_id)
    .bind(status.as_str())
    .bind(is_self_managed)
    .bind(has_premium)
    .bind(now)
    .bind(now)
    .execute(pool)
    .await
    .map(drop)
}

pub async fn record_error(pool: &SqlitePool, user: Id, message: &str) -> sqlx::Result<()> {
    let now = Timestamp::now();
    sqlx::query(
        "INSERT INTO playit_accounts (user_id, last_error, updated_at) VALUES (?, ?, ?) \
         ON CONFLICT(user_id) DO UPDATE \
            SET last_error = excluded.last_error, updated_at = excluded.updated_at",
    )
    .bind(user)
    .bind(message)
    .bind(now)
    .execute(pool)
    .await
    .map(drop)
}

pub async fn forget_account(pool: &SqlitePool, user: Id) -> sqlx::Result<()> {
    sqlx::query("DELETE FROM playit_accounts WHERE user_id = ?")
        .bind(user)
        .execute(pool)
        .await
        .map(drop)
}

type TunnelRow = (
    Id,
    Id,
    Option<String>,
    i64,
    String,
    String,
    Option<String>,
    Timestamp,
    Option<Timestamp>,
);

const SELECT: &str = "SELECT server_id, user_id, tunnel_id, local_port, state, addresses, detail, \
                             created_at, checked_at \
                        FROM playit_tunnels";

pub async fn tunnel(pool: &SqlitePool, server: Id) -> sqlx::Result<Option<Tunnel>> {
    let row: Option<TunnelRow> = sqlx::query_as(&format!("{SELECT} WHERE server_id = ?"))
        .bind(server)
        .fetch_optional(pool)
        .await?;
    Ok(row.map(read))
}

pub async fn tunnels(pool: &SqlitePool, user: Id) -> sqlx::Result<Vec<Tunnel>> {
    let rows: Vec<TunnelRow> =
        sqlx::query_as(&format!("{SELECT} WHERE user_id = ? ORDER BY created_at"))
            .bind(user)
            .fetch_all(pool)
            .await?;
    Ok(rows.into_iter().map(read).collect())
}

fn read(row: TunnelRow) -> Tunnel {
    let (server_id, user_id, tunnel_id, local_port, state, addresses, detail, created, checked) =
        row;
    Tunnel {
        server_id,
        user_id,
        tunnel_id,
        local_port: local_port as u16,
        state: TunnelState::parse(&state).unwrap_or(TunnelState::Failed),
        addresses: serde_json::from_str(&addresses).unwrap_or_default(),
        detail,
        created_at: created,
        checked_at: checked,
    }
}

pub async fn used(pool: &SqlitePool, user: Id) -> sqlx::Result<u32> {
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM playit_tunnels WHERE user_id = ?")
        .bind(user)
        .fetch_one(pool)
        .await?;
    Ok(count as u32)
}

pub async fn for_others(pool: &SqlitePool, user: Id) -> sqlx::Result<u32> {
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM playit_tunnels t JOIN servers s ON s.id = t.server_id \
          WHERE t.user_id = ? AND s.owner_id <> ?",
    )
    .bind(user)
    .bind(user)
    .fetch_one(pool)
    .await?;
    Ok(count as u32)
}

pub enum Claimed {
    Ok,
    Taken,
    Full,
}

pub async fn claim_slot(
    pool: &SqlitePool,
    user: Id,
    server: Id,
    local_port: u16,
    limit: u32,
) -> sqlx::Result<Claimed> {
    let mut tx = pool.begin().await?;

    let used: i64 = sqlx::query_scalar("SELECT count(*) FROM playit_tunnels WHERE user_id = ?")
        .bind(user)
        .fetch_one(&mut *tx)
        .await?;
    if used as u32 >= limit {
        return Ok(Claimed::Full);
    }

    let taken: i64 = sqlx::query_scalar("SELECT count(*) FROM playit_tunnels WHERE server_id = ?")
        .bind(server)
        .fetch_one(&mut *tx)
        .await?;
    if taken > 0 {
        return Ok(Claimed::Taken);
    }

    sqlx::query(
        "INSERT INTO playit_tunnels (server_id, user_id, local_port, state, addresses, created_at) \
         VALUES (?, ?, ?, 'pending', '[]', ?)",
    )
    .bind(server)
    .bind(user)
    .bind(local_port)
    .bind(Timestamp::now())
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok(Claimed::Ok)
}

pub async fn attach(pool: &SqlitePool, server: Id, tunnel_id: &str) -> sqlx::Result<bool> {
    let done = sqlx::query(
        "UPDATE playit_tunnels SET tunnel_id = ? WHERE server_id = ? AND tunnel_id IS NULL",
    )
    .bind(tunnel_id)
    .bind(server)
    .execute(pool)
    .await?;

    Ok(done.rows_affected() == 1)
}

pub async fn set_state(
    pool: &SqlitePool,
    server: Id,
    state: TunnelState,
    addresses: &[Address],
    detail: Option<&str>,
) -> sqlx::Result<()> {
    sqlx::query(
        "UPDATE playit_tunnels SET state = ?, addresses = ?, detail = ?, checked_at = ? \
          WHERE server_id = ?",
    )
    .bind(state.as_str())
    .bind(serde_json::to_string(addresses).unwrap_or_else(|_| "[]".to_owned()))
    .bind(detail)
    .bind(Timestamp::now())
    .bind(server)
    .execute(pool)
    .await
    .map(drop)
}

pub async fn forget(pool: &SqlitePool, server: Id) -> sqlx::Result<bool> {
    let done = sqlx::query("DELETE FROM playit_tunnels WHERE server_id = ?")
        .bind(server)
        .execute(pool)
        .await?;

    Ok(done.rows_affected() == 1)
}

pub async fn forget_all(pool: &SqlitePool, user: Id) -> sqlx::Result<()> {
    sqlx::query("DELETE FROM playit_tunnels WHERE user_id = ?")
        .bind(user)
        .execute(pool)
        .await
        .map(drop)
}

pub async fn release(pool: &SqlitePool, user: Id, tunnel_id: &str) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT OR IGNORE INTO playit_released (user_id, tunnel_id, released_at) \
         VALUES (?, ?, ?)",
    )
    .bind(user)
    .bind(tunnel_id)
    .bind(Timestamp::now())
    .execute(pool)
    .await
    .map(drop)
}

pub async fn released(pool: &SqlitePool, user: Id) -> sqlx::Result<Vec<String>> {
    sqlx::query_scalar(
        "SELECT tunnel_id FROM playit_released WHERE user_id = ? ORDER BY released_at",
    )
    .bind(user)
    .fetch_all(pool)
    .await
}

pub async fn settled(pool: &SqlitePool, tunnel_id: &str) -> sqlx::Result<()> {
    sqlx::query("DELETE FROM playit_released WHERE tunnel_id = ?")
        .bind(tunnel_id)
        .execute(pool)
        .await
        .map(drop)
}

pub async fn abandon_released(pool: &SqlitePool, user: Id) -> sqlx::Result<()> {
    sqlx::query("DELETE FROM playit_released WHERE user_id = ?")
        .bind(user)
        .execute(pool)
        .await
        .map(drop)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::harness::{a_server, a_user, test_pool};
    use crate::playit::tunnels::AddressKind;

    fn an_address() -> Address {
        Address {
            address: "quiet-forest.gl.at.ply.gg".to_owned(),
            kind: AddressKind::Auto,
        }
    }

    #[tokio::test]
    async fn a_user_who_connected_nothing_has_no_row_and_that_is_not_an_error() {
        let pool = test_pool().await;
        let anna = a_user(&pool, "anna").await;

        assert!(account(&pool, anna).await.unwrap().is_none());
        assert_eq!(used(&pool, anna).await.unwrap(), 0);
        assert!(connected(&pool).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_claim_outlives_the_request_that_started_it() {
        let pool = test_pool().await;
        let anna = a_user(&pool, "anna").await;
        let started = Timestamp::now();

        begin_claim(
            &pool,
            anna,
            &Claim {
                code: "34ddf358a8".to_owned(),
                state: ClaimState::WaitingForVisit,
                started_at: started,
            },
        )
        .await
        .unwrap();

        let claim =
            account(&pool, anna).await.unwrap().unwrap().claim.expect("the claim is on disk");
        assert_eq!(claim.code, "34ddf358a8");
        assert_eq!(claim.state, ClaimState::WaitingForVisit);
        assert_eq!(claim.started_at, started);

        assert!(advance_claim(&pool, anna, "34ddf358a8", ClaimState::Accepted).await.unwrap());
        assert_eq!(
            account(&pool, anna).await.unwrap().unwrap().claim.unwrap().state,
            ClaimState::Accepted
        );

        assert!(clear_claim(&pool, anna).await.unwrap());
        assert!(account(&pool, anna).await.unwrap().unwrap().claim.is_none());
        assert!(!clear_claim(&pool, anna).await.unwrap(), "there is nothing left to clear");
    }

    #[tokio::test]
    async fn one_users_claim_is_not_another_users_claim() {
        let pool = test_pool().await;
        let anna = a_user(&pool, "anna").await;
        let ben = a_user(&pool, "ben").await;

        begin_claim(
            &pool,
            anna,
            &Claim {
                code: "aaaaaaaaaa".to_owned(),
                state: ClaimState::WaitingForVisit,
                started_at: Timestamp::now(),
            },
        )
        .await
        .unwrap();

        assert!(account(&pool, ben).await.unwrap().is_none(), "ben has nothing of anna's");
        assert!(!clear_claim(&pool, ben).await.unwrap());
        assert!(
            !advance_claim(&pool, ben, "aaaaaaaaaa", ClaimState::Accepted).await.unwrap(),
            "ben moved anna's sign-up along"
        );
        assert_eq!(
            account(&pool, anna).await.unwrap().unwrap().claim.unwrap().state,
            ClaimState::WaitingForVisit
        );
    }

    #[tokio::test]
    async fn a_stale_loop_cannot_write_over_a_newer_claim() {
        let pool = test_pool().await;
        let anna = a_user(&pool, "anna").await;
        begin_claim(
            &pool,
            anna,
            &Claim {
                code: "bbbbbbbbbb".to_owned(),
                state: ClaimState::WaitingForVisit,
                started_at: Timestamp::now(),
            },
        )
        .await
        .unwrap();

        assert!(!advance_claim(&pool, anna, "aaaaaaaaaa", ClaimState::Accepted).await.unwrap());
        assert_eq!(
            account(&pool, anna).await.unwrap().unwrap().claim.unwrap().state,
            ClaimState::WaitingForVisit
        );
    }

    #[tokio::test]
    async fn the_free_plans_four_ports_are_counted_before_the_fifth_is_let_in() {
        let pool = test_pool().await;
        let anna = a_user(&pool, "anna").await;

        for index in 0..4 {
            let server = a_server(&pool, anna, &format!("s{index}"), 1024).await;
            assert!(matches!(
                claim_slot(&pool, anna, server, 25565 + index, 4).await.unwrap(),
                Claimed::Ok
            ));
        }

        let fifth = a_server(&pool, anna, "fifth", 1024).await;
        assert!(matches!(claim_slot(&pool, anna, fifth, 25569, 4).await.unwrap(), Claimed::Full));
        assert_eq!(used(&pool, anna).await.unwrap(), 4);
    }

    #[tokio::test]
    async fn a_full_account_is_only_full_for_its_own_owner() {
        let pool = test_pool().await;
        let anna = a_user(&pool, "anna").await;
        let ben = a_user(&pool, "ben").await;

        for index in 0..4 {
            let server = a_server(&pool, anna, &format!("anna-{index}"), 1024).await;
            claim_slot(&pool, anna, server, 25565 + index, 4).await.unwrap();
        }

        let bens = a_server(&pool, ben, "survival", 1024).await;
        assert!(matches!(claim_slot(&pool, ben, bens, 25600, 4).await.unwrap(), Claimed::Ok));
        assert_eq!(used(&pool, anna).await.unwrap(), 4);
        assert_eq!(used(&pool, ben).await.unwrap(), 1);
        assert_eq!(tunnels(&pool, ben).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn one_server_gets_one_tunnel_and_not_two() {
        let pool = test_pool().await;
        let anna = a_user(&pool, "anna").await;
        let server = a_server(&pool, anna, "survival", 1024).await;

        assert!(matches!(claim_slot(&pool, anna, server, 25565, 4).await.unwrap(), Claimed::Ok));
        assert!(matches!(claim_slot(&pool, anna, server, 25565, 4).await.unwrap(), Claimed::Taken));
    }

    #[tokio::test]
    async fn a_tunnel_keeps_its_addresses_through_the_column() {
        let pool = test_pool().await;
        let anna = a_user(&pool, "anna").await;
        let server = a_server(&pool, anna, "survival", 1024).await;

        claim_slot(&pool, anna, server, 25565, 4).await.unwrap();
        assert!(attach(&pool, server, "c0ffee11").await.unwrap());
        assert!(!attach(&pool, server, "second").await.unwrap(), "the id is written once");

        set_state(&pool, server, TunnelState::Online, &[an_address()], None).await.unwrap();

        let row = tunnel(&pool, server).await.unwrap().expect("the row");
        assert_eq!(row.state, TunnelState::Online);
        assert_eq!(row.addresses, vec![an_address()]);
        assert_eq!(row.local_port, 25565);
        assert_eq!(row.user_id, anna);
        assert_eq!(row.tunnel_id.as_deref(), Some("c0ffee11"));
        assert!(row.checked_at.is_some());
    }

    #[tokio::test]
    async fn deleting_the_server_leaves_the_tunnel_id_behind_to_be_handed_back() {
        let pool = test_pool().await;
        let anna = a_user(&pool, "anna").await;
        let server = a_server(&pool, anna, "survival", 1024).await;

        claim_slot(&pool, anna, server, 25565, 4).await.unwrap();
        attach(&pool, server, "c0ffee11").await.unwrap();

        sqlx::query("DELETE FROM servers WHERE id = ?")
            .bind(server)
            .execute(&pool)
            .await
            .unwrap();

        assert!(tunnel(&pool, server).await.unwrap().is_none());
        assert_eq!(released(&pool, anna).await.unwrap(), vec!["c0ffee11".to_owned()]);

        settled(&pool, "c0ffee11").await.unwrap();
        assert!(released(&pool, anna).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_deleted_user_leaves_his_own_debt_behind_under_his_own_name() {
        let pool = test_pool().await;
        let anna = a_user(&pool, "anna").await;
        let ben = a_user(&pool, "ben").await;
        let published = a_server(&pool, ben, "survival", 1024).await;
        let his = a_server(&pool, ben, "creative", 1024).await;

        claim_slot(&pool, anna, published, 25565, 4).await.unwrap();
        attach(&pool, published, "annas-tunnel").await.unwrap();
        claim_slot(&pool, ben, his, 25566, 4).await.unwrap();
        attach(&pool, his, "bens-tunnel").await.unwrap();

        sqlx::query("DELETE FROM users WHERE id = ?").bind(anna).execute(&pool).await.unwrap();

        assert_eq!(used(&pool, anna).await.unwrap(), 0, "her rows went with her");
        assert!(tunnel(&pool, published).await.unwrap().is_none());
        assert_eq!(released(&pool, anna).await.unwrap(), vec!["annas-tunnel".to_owned()]);
        assert!(
            released(&pool, ben).await.unwrap().is_empty(),
            "ben was handed a debt that is not his"
        );
        assert_eq!(used(&pool, ben).await.unwrap(), 1, "his tunnel went with hers");
    }

    #[tokio::test]
    async fn a_tunnel_that_never_got_an_id_leaves_nothing_to_hand_back() {
        let pool = test_pool().await;
        let anna = a_user(&pool, "anna").await;
        let server = a_server(&pool, anna, "survival", 1024).await;

        claim_slot(&pool, anna, server, 25565, 4).await.unwrap();
        assert!(forget(&pool, server).await.unwrap());

        assert!(released(&pool, anna).await.unwrap().is_empty());
        assert!(!forget(&pool, server).await.unwrap());
    }

    #[tokio::test]
    async fn what_rundata_says_lands_in_the_row_and_clears_the_last_complaint() {
        let pool = test_pool().await;
        let anna = a_user(&pool, "anna").await;

        record_error(&pool, anna, "playit.gg could not be reached").await.unwrap();
        assert!(account(&pool, anna).await.unwrap().unwrap().last_error.is_some());

        record_identity(&pool, anna, "11112222", AccountStatus::Guest, true, false).await.unwrap();

        let known = account(&pool, anna).await.unwrap().unwrap();
        assert_eq!(known.agent_id.as_deref(), Some("11112222"));
        assert_eq!(known.account_status, Some(AccountStatus::Guest));
        assert!(known.is_self_managed);
        assert!(known.last_error.is_none());
        assert!(known.checked_at.is_some());

        forget_account(&pool, anna).await.unwrap();
        assert!(account(&pool, anna).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn the_overview_counts_the_ports_of_each_account_and_the_ones_spent_for_others() {
        let pool = test_pool().await;
        let anna = a_user(&pool, "anna").await;
        let ben = a_user(&pool, "ben").await;
        let hers = a_server(&pool, anna, "survival", 1024).await;
        let his = a_server(&pool, ben, "creative", 1024).await;

        let verified = AccountStatus::Verified;
        record_identity(&pool, anna, "agent-a", verified, true, false).await.unwrap();
        claim_slot(&pool, anna, hers, 25565, 4).await.unwrap();
        claim_slot(&pool, anna, his, 25566, 4).await.unwrap();

        let rows = overview(&pool).await.unwrap();
        assert_eq!(rows.len(), 1, "ben has connected nothing and is not in the list");

        let line = &rows[0];
        assert_eq!(line.user_id, anna);
        assert_eq!(line.username.as_deref(), Some("anna"));
        assert_eq!((line.used, line.for_others), (2, 1));
        assert_eq!(for_others(&pool, anna).await.unwrap(), 1);
        assert_eq!(for_others(&pool, ben).await.unwrap(), 0);
        assert_eq!(line.account_status, Some(AccountStatus::Verified));
    }

    #[tokio::test]
    async fn every_tunnel_state_survives_the_column_it_is_stored_in() {
        for state in [
            TunnelState::Pending,
            TunnelState::Online,
            TunnelState::Offline,
            TunnelState::Missing,
            TunnelState::Failed,
        ] {
            assert_eq!(TunnelState::parse(state.as_str()), Some(state));
            assert_eq!(
                serde_json::to_value(state).unwrap(),
                serde_json::Value::String(state.as_str().to_owned())
            );
        }
        assert_eq!(TunnelState::parse("none"), None, "'none' is the absence of a row");
    }
}
