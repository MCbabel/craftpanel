use sqlx::SqlitePool;

use super::disk::Disks;
use super::error::{Failure, Result};
use super::live::LiveServers;
use super::{limits, usage};
use crate::helper::Helper;
use crate::model::{
    AccountOrigin, Capabilities, CpuMode, CpuUsage, DiskUsage, Id, LimitDimension, MemoryUsage,
    PanelRole, PanelUser, PidsUsage, ServerCounts, SystemUser, SystemUserState, Timestamp,
    UserLimits, UserRef, UserUsage,
};

pub const NAME_LENGTH: std::ops::RangeInclusive<usize> = 3..=39;

#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct UserRow {
    pub id: Id,
    pub username: String,
    pub email: Option<String>,
    pub origin: AccountOrigin,
    pub password_hash: String,
    pub role: PanelRole,
    pub system_uid: Option<u32>,
    pub system_state: SystemUserState,
    pub system_error_message: Option<String>,
    pub busy: bool,
    pub must_change_password: bool,
    pub created_at: Timestamp,
    pub last_login_at: Option<Timestamp>,
    pub memory_mib: u32,
    pub cpu_mode: CpuMode,
    pub cpu_cores: f64,
    pub pids_max: u32,
    pub disk_mib: u32,
}

const COLUMNS: &str = "id, username, email, origin, password_hash, role, system_uid, \
                       system_state, system_error_message, busy, must_change_password, \
                       created_at, last_login_at, memory_mib, cpu_mode, cpu_cores, pids_max, \
                       disk_mib";

impl UserRow {
    pub fn limits(&self) -> UserLimits {
        UserLimits {
            memory_mib: self.memory_mib,
            cpu_mode: self.cpu_mode,
            cpu_cores: self.cpu_cores,
            pids_max: self.pids_max,
            disk_mib: self.disk_mib,
        }
    }

    pub fn budget(&self) -> limits::Budget {
        limits::Budget::of(self.role, self.limits())
    }

    pub fn system_user(&self) -> SystemUser {
        SystemUser {
            state: self.system_state,
            name: craftpanel_proto::system_username(&self.id.to_string()),
            uid: self.system_uid,
            error_message: self.system_error_message.clone(),
        }
    }

    pub fn reference(&self) -> UserRef {
        UserRef { id: self.id, username: self.username.clone(), avatar_url: None }
    }

    pub fn is_admin(&self) -> bool {
        self.role == PanelRole::Admin
    }
}

pub async fn find(pool: &SqlitePool, id: Id) -> sqlx::Result<Option<UserRow>> {
    sqlx::query_as::<_, UserRow>(&format!("SELECT {COLUMNS} FROM users WHERE id = ?"))
        .bind(id)
        .fetch_optional(pool)
        .await
}

pub async fn load(pool: &SqlitePool, id: Id) -> Result<UserRow> {
    find(pool, id).await?.ok_or_else(|| Failure::not_found("user_not_found", "no such user"))
}

pub async fn by_name(pool: &SqlitePool, username: &str) -> sqlx::Result<Option<UserRow>> {
    sqlx::query_as::<_, UserRow>(&format!("SELECT {COLUMNS} FROM users WHERE username = ?"))
        .bind(username.to_lowercase())
        .fetch_optional(pool)
        .await
}

pub async fn by_email(pool: &SqlitePool, email: &str) -> sqlx::Result<Option<UserRow>> {
    sqlx::query_as::<_, UserRow>(&format!("SELECT {COLUMNS} FROM users WHERE email = ?"))
        .bind(email)
        .fetch_optional(pool)
        .await
}

pub async fn page(
    pool: &SqlitePool,
    query: Option<&str>,
    limit: u32,
    offset: u32,
) -> sqlx::Result<(Vec<UserRow>, u32)> {
    let pattern = match query.map(str::trim).filter(|query| !query.is_empty()) {
        Some(query) => format!("%{}%", escape_like(&query.to_lowercase())),
        None => "%".to_owned(),
    };
    let rows = sqlx::query_as::<_, UserRow>(&format!(
        "SELECT {COLUMNS} FROM users WHERE username LIKE ? ESCAPE '\\' \
         ORDER BY username LIMIT ? OFFSET ?"
    ))
    .bind(&pattern)
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await?;

    let total: i64 =
        sqlx::query_scalar("SELECT count(*) FROM users WHERE username LIKE ? ESCAPE '\\'")
            .bind(&pattern)
            .fetch_one(pool)
            .await?;

    Ok((rows, total.max(0) as u32))
}

pub async fn search(pool: &SqlitePool, query: &str, limit: u32) -> sqlx::Result<Vec<UserRef>> {
    sqlx::query_as::<_, (Id, String)>(
        "SELECT id, username FROM users WHERE username LIKE ? ESCAPE '\\' \
         ORDER BY username LIMIT ?",
    )
    .bind(format!("{}%", escape_like(&query.to_lowercase())))
    .bind(limit)
    .fetch_all(pool)
    .await
    .map(|rows| {
        rows.into_iter().map(|(id, username)| UserRef { id, username, avatar_url: None }).collect()
    })
}

fn escape_like(input: &str) -> String {
    input.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_")
}

pub fn check_username(name: &str) -> Result<()> {
    let length = name.chars().count();
    if !NAME_LENGTH.contains(&length) {
        return Err(Failure::invalid_request(format!(
            "a username is {} to {} characters",
            NAME_LENGTH.start(),
            NAME_LENGTH.end()
        )));
    }
    if !name.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
    {
        return Err(Failure::invalid_request(
            "a username holds lower case letters, digits, '-' and '_'",
        ));
    }
    Ok(())
}

pub async fn claim_name_in_users(pool: &SqlitePool, name: &str, except: Option<Id>) -> Result<()> {
    check_username(name)?;
    let taken: Option<Id> = sqlx::query_scalar("SELECT id FROM users WHERE username = ?")
        .bind(name)
        .fetch_optional(pool)
        .await?;

    match taken {
        Some(id) if Some(id) != except => Err(name_taken(name)),
        _ => Ok(()),
    }
}

pub async fn claim_name(pool: &SqlitePool, name: &str, except: Option<Id>) -> Result<()> {
    claim_name_in_users(pool, name, except).await?;
    claim_name_in_applications(pool, name, None).await
}

pub async fn claim_name_for_sign_up(
    pool: &SqlitePool,
    name: &str,
    replacing: Option<Id>,
) -> Result<()> {
    claim_name_in_users(pool, name, None).await?;
    claim_name_in_applications(pool, name, replacing).await
}

async fn claim_name_in_applications(
    pool: &SqlitePool,
    name: &str,
    replacing: Option<Id>,
) -> Result<()> {
    let promised: Option<Id> = sqlx::query_scalar("SELECT id FROM registrations WHERE username = ?")
        .bind(name)
        .fetch_optional(pool)
        .await?;

    match promised {
        Some(id) if Some(id) != replacing => Err(Failure::conflict(
            "username_taken",
            format!("{name} is spoken for by an open sign-up"),
        )),
        _ => Ok(()),
    }
}

pub async fn claim_email(pool: &SqlitePool, email: &str, except: Option<Id>) -> Result<()> {
    let taken: Option<Id> = sqlx::query_scalar("SELECT id FROM users WHERE email = ?")
        .bind(email)
        .fetch_optional(pool)
        .await?;
    if taken.is_some_and(|id| Some(id) != except) {
        return Err(email_taken());
    }

    let applied: Option<String> =
        sqlx::query_scalar("SELECT email FROM registrations WHERE email = ?")
            .bind(email)
            .fetch_optional(pool)
            .await?;
    if applied.is_some() {
        return Err(email_taken());
    }
    Ok(())
}

fn name_taken(name: &str) -> Failure {
    Failure::conflict("username_taken", format!("{name} is taken"))
}

fn email_taken() -> Failure {
    Failure::conflict("email_taken", "that address is already on an account or an open sign-up")
}

pub fn map_taken(err: sqlx::Error) -> Failure {
    match &err {
        sqlx::Error::Database(database) if database.is_unique_violation() => {
            let complaint = database.message();
            if complaint.contains("users.username") {
                name_taken("that name")
            } else if complaint.contains("users.email") {
                email_taken()
            } else {
                Failure::internal(err)
            }
        }
        _ => Failure::internal(err),
    }
}

pub async fn admin_count(pool: &SqlitePool) -> sqlx::Result<u32> {
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM users WHERE role = 'admin'")
        .fetch_one(pool)
        .await?;
    Ok(count.max(0) as u32)
}

pub async fn is_last_admin(pool: &SqlitePool, row: &UserRow) -> sqlx::Result<bool> {
    Ok(row.is_admin() && admin_count(pool).await? <= 1)
}

#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct OwnedServer {
    pub id: Id,
    pub name: String,
    pub memory_mib: u32,
}

pub async fn owned_servers(pool: &SqlitePool, owner: Id) -> sqlx::Result<Vec<OwnedServer>> {
    sqlx::query_as::<_, OwnedServer>(
        "SELECT id, name, memory_mib FROM servers WHERE owner_id = ? ORDER BY name",
    )
    .bind(owner)
    .fetch_all(pool)
    .await
}

pub async fn measure(
    pool: &SqlitePool,
    row: &UserRow,
    live: &LiveServers,
    disks: &Disks,
) -> sqlx::Result<UserUsage> {
    let owned = owned_servers(pool, row.id).await?;
    let allocated_mib = owned.iter().map(|server| server.memory_mib).sum();
    let ids: Vec<Id> = owned.iter().map(|server| server.id).collect();
    let running = live.among(&ids).await.len() as u32;

    let sample = usage::shared().sample(row.id);
    let space = disks.of(row.id).await;
    let budget = row.budget();

    let mut over = Vec::new();
    if budget.exceeded_by(allocated_mib) {
        over.push(LimitDimension::Memory);
    }
    if budget.disk_exceeded_by(space.used_bytes()) {
        over.push(LimitDimension::Disk);
    }

    Ok(UserUsage {
        memory: MemoryUsage {
            limit_mib: budget.memory_mib(),
            allocated_mib,
            used_bytes: sample.memory_bytes,
        },
        cpu: CpuUsage { limit_cores: budget.cpu_cores(), used_cores: sample.used_cores },
        pids: PidsUsage { limit: budget.pids_max(), used: sample.pids },
        disk: DiskUsage {
            limit_mib: budget.disk_mib(),
            used_bytes: space.used_bytes(),
            servers_bytes: space.servers_bytes,
            backups_bytes: space.backups_bytes,
            complete: space.complete,
        },
        servers: ServerCounts { total: owned.len() as u32, running },
        over_limit: !over.is_empty(),
        over_limit_dimensions: over,
        measured_at: sample.measured_at,
    })
}

pub fn capabilities(row: &UserRow, usage: &UserUsage) -> Capabilities {
    let blocked = if row.system_state != SystemUserState::Ready {
        Some(crate::model::BlockedReason::SystemUserNotReady)
    } else if usage.over_limit {
        Some(crate::model::BlockedReason::OverLimit)
    } else {
        None
    };
    let only_the_disk = usage.over_limit_dimensions == [LimitDimension::Disk];

    Capabilities {
        can_create_servers: blocked.is_none(),
        can_start_servers: blocked.is_none()
            || (only_the_disk && row.system_state == SystemUserState::Ready),
        can_manage_panel_users: row.is_admin(),
        blocked_reason: blocked,
    }
}

pub async fn panel_user(
    pool: &SqlitePool,
    row: &UserRow,
    live: &LiveServers,
    disks: &Disks,
) -> sqlx::Result<PanelUser> {
    Ok(PanelUser {
        id: row.id,
        username: row.username.clone(),
        avatar_url: None,
        panel_role: row.role,
        email: row.email.clone(),
        origin: row.origin,
        created_at: row.created_at,
        last_login_at: row.last_login_at,
        must_change_password: row.must_change_password,
        system_user: row.system_user(),
        limits: row.budget().limits(),
        usage: measure(pool, row, live, disks).await?,
    })
}

pub struct NewUser<'a> {
    pub username: &'a str,
    pub email: Option<String>,
    pub origin: AccountOrigin,
    pub password_hash: String,
    pub role: PanelRole,
    pub must_change_password: bool,
    pub limits: UserLimits,
}

pub async fn insert(pool: &SqlitePool, new: NewUser<'_>) -> sqlx::Result<UserRow> {
    let now = Timestamp::now();
    let row = UserRow {
        id: Id::new(),
        username: new.username.to_owned(),
        email: new.email,
        origin: new.origin,
        password_hash: new.password_hash,
        role: new.role,
        system_uid: None,
        system_state: SystemUserState::Provisioning,
        system_error_message: None,
        busy: false,
        must_change_password: new.must_change_password,
        created_at: now,
        last_login_at: None,
        memory_mib: new.limits.memory_mib,
        cpu_mode: new.limits.cpu_mode,
        cpu_cores: new.limits.cpu_cores,
        pids_max: new.limits.pids_max,
        disk_mib: new.limits.disk_mib,
    };

    sqlx::query(
        "INSERT INTO users (id, username, email, origin, password_hash, role, system_state, \
         must_change_password, memory_mib, cpu_mode, cpu_cores, pids_max, disk_mib, \
         created_at, updated_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(row.id)
    .bind(&row.username)
    .bind(row.email.as_deref())
    .bind(row.origin)
    .bind(&row.password_hash)
    .bind(row.role)
    .bind(row.system_state)
    .bind(row.must_change_password)
    .bind(row.memory_mib)
    .bind(row.cpu_mode)
    .bind(row.cpu_cores)
    .bind(row.pids_max)
    .bind(row.disk_mib)
    .bind(row.created_at)
    .bind(row.created_at)
    .execute(pool)
    .await?;

    Ok(row)
}

pub async fn set_busy(pool: &SqlitePool, id: Id, busy: bool) -> sqlx::Result<()> {
    sqlx::query("UPDATE users SET busy = ?, updated_at = ? WHERE id = ?")
        .bind(busy)
        .bind(Timestamp::now())
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn claim_busy(pool: &SqlitePool, id: Id) -> sqlx::Result<bool> {
    let taken = sqlx::query("UPDATE users SET busy = 1, updated_at = ? WHERE id = ? AND busy = 0")
        .bind(Timestamp::now())
        .bind(id)
        .execute(pool)
        .await?;
    Ok(taken.rows_affected() == 1)
}

pub fn busy() -> Failure {
    Failure::conflict("user_busy", "another change to this account is still running")
}

pub fn refuse_if_busy(row: &UserRow) -> Result<()> {
    if row.busy {
        return Err(busy());
    }
    Ok(())
}

pub async fn provision(pool: &SqlitePool, helper: &Helper, row: &UserRow) -> sqlx::Result<SystemUser> {
    let id = row.id.to_string();

    let (state, uid, message) = match helper.create_user(&id).await {
        Ok(created_uid) => {
            let complaint = helper
                .apply_limits(&id, row.budget().to_cgroup())
                .await
                .err()
                .map(|err| format!("limits were not applied: {err:#}"));
            (SystemUserState::Ready, Some(created_uid), complaint)
        }
        Err(err) => (SystemUserState::Error, row.system_uid, Some(format!("{err:#}"))),
    };

    sqlx::query(
        "UPDATE users SET system_state = ?, system_uid = ?, system_error_message = ?, \
         updated_at = ? WHERE id = ?",
    )
    .bind(state)
    .bind(uid)
    .bind(message.as_deref())
    .bind(Timestamp::now())
    .bind(row.id)
    .execute(pool)
    .await?;

    Ok(SystemUser {
        state,
        name: craftpanel_proto::system_username(&id),
        uid,
        error_message: message,
    })
}

pub async fn reconcile(pool: &SqlitePool, helper: &Helper) -> sqlx::Result<u32> {
    let unstuck = sqlx::query("UPDATE users SET busy = 0 WHERE busy = 1").execute(pool).await?;
    if unstuck.rows_affected() > 0 {
        tracing::info!(accounts = unstuck.rows_affected(), "cleared a stale busy flag");
    }

    let waiting = sqlx::query_as::<_, UserRow>(&format!(
        "SELECT {COLUMNS} FROM users WHERE system_state = 'provisioning'"
    ))
    .fetch_all(pool)
    .await?;

    let mut ready = 0;
    for row in &waiting {
        if provision(pool, helper, row).await?.state == SystemUserState::Ready {
            ready += 1;
        }
    }

    rewrite_limits(pool, helper).await?;
    Ok(ready)
}

async fn rewrite_limits(pool: &SqlitePool, helper: &Helper) -> sqlx::Result<()> {
    let accounts = sqlx::query_as::<_, UserRow>(&format!(
        "SELECT {COLUMNS} FROM users WHERE system_state = 'ready'"
    ))
    .fetch_all(pool)
    .await?;

    for row in &accounts {
        if let Err(err) = helper.apply_limits(&row.id.to_string(), row.budget().to_cgroup()).await {
            tracing::warn!(user = %row.id, "the cgroup was not written at start: {err:#}");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::harness::{a_server, a_user, an_admin, test_pool, FakeHelper};

    #[test]
    fn a_username_is_three_to_thirty_nine_lower_case_characters() {
        assert!(check_username("max").is_ok());
        assert!(check_username("a-b_c9").is_ok());
        assert!(check_username(&"a".repeat(39)).is_ok());

        assert_eq!(check_username("ma").unwrap_err().code(), "invalid_request");
        assert_eq!(check_username(&"a".repeat(40)).unwrap_err().code(), "invalid_request");
        assert_eq!(check_username("Max").unwrap_err().code(), "invalid_request");
        assert_eq!(check_username("andré").unwrap_err().code(), "invalid_request");
        assert_eq!(check_username("max lang").unwrap_err().code(), "invalid_request");
        assert_eq!(check_username("max.lang").unwrap_err().code(), "invalid_request");
        assert_eq!(check_username("../etc").unwrap_err().code(), "invalid_request");
    }

    #[test]
    fn a_wildcard_in_a_search_stays_a_character() {
        assert_eq!(escape_like("100%_a"), "100\\%\\_a");
        assert_eq!(escape_like("plain"), "plain");
    }

    #[tokio::test]
    async fn the_system_account_is_named_after_the_id() {
        let pool = test_pool().await;
        let id = a_user(&pool, "max").await;
        let row = load(&pool, id).await.unwrap();

        assert_eq!(row.system_user().name, format!("craft-{}", id.to_string().to_lowercase()));
        assert!(!row.system_user().name.contains("max"), "names change, ids do not");
    }

    #[tokio::test]
    async fn a_taken_name_is_only_taken_by_somebody_else() {
        let pool = test_pool().await;
        let max = a_user(&pool, "max").await;
        a_user(&pool, "anna").await;

        assert_eq!(claim_name(&pool, "anna", None).await.unwrap_err().code(), "username_taken");
        assert_eq!(
            claim_name(&pool, "anna", Some(max)).await.unwrap_err().code(),
            "username_taken"
        );
        assert!(claim_name(&pool, "max", Some(max)).await.is_ok(), "his own name is his");
        assert!(claim_name(&pool, "andre", None).await.is_ok());
    }

    #[tokio::test]
    async fn losing_the_race_for_a_name_is_still_username_taken() {
        let pool = test_pool().await;
        let taken = |name: &'static str, email: Option<&str>| NewUser {
            username: name,
            email: email.map(str::to_owned),
            origin: AccountOrigin::Admin,
            password_hash: crate::auth::password::hash("a-good-password").unwrap(),
            role: PanelRole::User,
            must_change_password: false,
            limits: super::super::harness::some_limits(),
        };

        insert(&pool, taken("anna", Some("anna@example.test"))).await.unwrap();
        let second = insert(&pool, taken("anna", None)).await.unwrap_err();
        assert_eq!(map_taken(second).code(), "username_taken");

        let same_address = insert(&pool, taken("berta", Some("anna@example.test"))).await.unwrap_err();
        assert_eq!(map_taken(same_address).code(), "email_taken");

        let elsewhere = sqlx::Error::RowNotFound;
        assert_eq!(map_taken(elsewhere).code(), "internal", "only those two indexes");
    }

    #[tokio::test]
    async fn a_search_finds_prefixes_and_ignores_case() {
        let pool = test_pool().await;
        a_user(&pool, "anna").await;
        a_user(&pool, "andre").await;
        a_user(&pool, "max").await;

        let found = search(&pool, "AN", 25).await.unwrap();
        let names: Vec<&str> = found.iter().map(|user| user.username.as_str()).collect();
        assert_eq!(names, vec!["andre", "anna"]);
        assert!(found.iter().all(|user| user.avatar_url.is_none()));
    }

    #[tokio::test]
    async fn the_allocation_counts_every_server_running_or_not() {
        let pool = test_pool().await;
        let max = a_user(&pool, "max").await;
        let running = a_server(&pool, max, "one", 4096).await;
        a_server(&pool, max, "two", 2048).await;

        let row = load(&pool, max).await.unwrap();
        let usage =
            measure(&pool, &row, &LiveServers::fixed([running]), &Disks::none()).await.unwrap();

        assert_eq!(usage.memory.allocated_mib, 6144);
        assert_eq!(usage.servers, ServerCounts { total: 2, running: 1 });
        assert_eq!(usage.memory.limit_mib, Some(4096));
        assert!(usage.over_limit, "6 GiB handed out against a 4 GiB limit");
        assert_eq!(usage.over_limit_dimensions, vec![LimitDimension::Memory]);
    }

    #[tokio::test]
    async fn being_over_the_limit_stops_the_next_server_and_nothing_else() {
        let pool = test_pool().await;
        let max = a_user(&pool, "max").await;
        a_server(&pool, max, "one", 8192).await;

        let row = load(&pool, max).await.unwrap();
        let usage = measure(&pool, &row, &LiveServers::none(), &Disks::none()).await.unwrap();
        let can = capabilities(&row, &usage);

        assert!(!can.can_create_servers);
        assert!(!can.can_start_servers);
        assert_eq!(can.blocked_reason, Some(crate::model::BlockedReason::OverLimit));
        assert!(!can.can_manage_panel_users);
    }

    #[tokio::test]
    async fn a_missing_system_user_outranks_being_over_the_limit() {
        let pool = test_pool().await;
        let max = a_user(&pool, "max").await;
        a_server(&pool, max, "one", 8192).await;
        sqlx::query("UPDATE users SET system_state = 'error' WHERE id = ?")
            .bind(max)
            .execute(&pool)
            .await
            .unwrap();

        let row = load(&pool, max).await.unwrap();
        let usage = measure(&pool, &row, &LiveServers::none(), &Disks::none()).await.unwrap();
        assert!(usage.over_limit);
        assert_eq!(
            capabilities(&row, &usage).blocked_reason,
            Some(crate::model::BlockedReason::SystemUserNotReady)
        );
    }

    #[tokio::test]
    async fn only_an_admin_may_manage_panel_users() {
        let pool = test_pool().await;
        let anna = an_admin(&pool, "anna").await;
        let row = load(&pool, anna).await.unwrap();
        let usage = measure(&pool, &row, &LiveServers::none(), &Disks::none()).await.unwrap();

        assert!(capabilities(&row, &usage).can_manage_panel_users);
    }

    #[tokio::test]
    async fn the_last_admin_is_recognised_as_such() {
        let pool = test_pool().await;
        let anna = an_admin(&pool, "anna").await;
        let max = a_user(&pool, "max").await;

        let only = load(&pool, anna).await.unwrap();
        assert!(is_last_admin(&pool, &only).await.unwrap());
        assert!(!is_last_admin(&pool, &load(&pool, max).await.unwrap()).await.unwrap());

        an_admin(&pool, "bea").await;
        assert!(!is_last_admin(&pool, &only).await.unwrap());
    }

    #[tokio::test]
    async fn a_helper_that_answers_makes_the_account_ready() {
        let pool = test_pool().await;
        let max = a_user(&pool, "max").await;
        sqlx::query("UPDATE users SET system_state = 'provisioning', system_uid = NULL WHERE id = ?")
            .bind(max)
            .execute(&pool)
            .await
            .unwrap();

        let fake = FakeHelper::obliging().await;
        let row = load(&pool, max).await.unwrap();
        let system = provision(&pool, &Helper::new(fake.socket()), &row).await.unwrap();

        assert_eq!(system.state, SystemUserState::Ready);
        assert_eq!(system.uid, Some(6100));
        assert_eq!(system.error_message, None);
        assert_eq!(load(&pool, max).await.unwrap().system_state, SystemUserState::Ready);

        let asked: Vec<&str> = fake
            .calls()
            .iter()
            .map(|call| match call {
                craftpanel_proto::HelperRequest::CreateUser { .. } => "create",
                craftpanel_proto::HelperRequest::ApplyLimits { .. } => "limits",
                _ => "other",
            })
            .collect();
        assert_eq!(asked, vec!["create", "limits"], "the account and then its cgroup");
    }

    #[tokio::test]
    async fn a_helper_that_refuses_leaves_a_readable_reason() {
        let pool = test_pool().await;
        let max = a_user(&pool, "max").await;
        let fake = FakeHelper::refusing().await;
        let row = load(&pool, max).await.unwrap();

        let system = provision(&pool, &Helper::new(fake.socket()), &row).await.unwrap();
        assert_eq!(system.state, SystemUserState::Error);
        assert!(
            system.error_message.as_deref().unwrap().contains("UID range exhausted"),
            "{system:?}"
        );

        let stored = load(&pool, max).await.unwrap();
        assert_eq!(stored.system_state, SystemUserState::Error);
        assert!(stored.system_error_message.is_some());
    }

    #[tokio::test]
    async fn a_restart_clears_the_busy_flag_and_finishes_what_was_pending() {
        let pool = test_pool().await;
        let stuck = a_user(&pool, "max").await;
        let waiting = a_user(&pool, "anna").await;
        sqlx::query("UPDATE users SET busy = 1 WHERE id = ?").bind(stuck).execute(&pool).await.unwrap();
        sqlx::query("UPDATE users SET system_state = 'provisioning' WHERE id = ?")
            .bind(waiting)
            .execute(&pool)
            .await
            .unwrap();

        let fake = FakeHelper::obliging().await;
        assert_eq!(reconcile(&pool, &Helper::new(fake.socket())).await.unwrap(), 1);

        assert!(!load(&pool, stuck).await.unwrap().busy);
        assert_eq!(load(&pool, waiting).await.unwrap().system_state, SystemUserState::Ready);
    }

    #[tokio::test]
    async fn a_restart_writes_every_budget_again_and_an_administrator_gets_none() {
        let pool = test_pool().await;
        let boss = an_admin(&pool, "boss").await;
        let max = a_user(&pool, "max").await;

        let fake = FakeHelper::obliging().await;
        reconcile(&pool, &Helper::new(fake.socket())).await.unwrap();

        let written: Vec<(String, craftpanel_proto::ResourceLimits)> = fake
            .calls()
            .into_iter()
            .filter_map(|call| match call {
                craftpanel_proto::HelperRequest::ApplyLimits { user_id, limits } => {
                    Some((user_id, limits))
                }
                _ => None,
            })
            .collect();
        let of = |who: Id| {
            written
                .iter()
                .find(|(user, _)| *user == who.to_string())
                .unwrap_or_else(|| panic!("{who} was written"))
                .1
        };

        assert_eq!(of(boss).memory_high_bytes, None, "the administrator keeps no ceiling");
        assert_eq!(of(boss).memory_max_bytes, None);
        assert_eq!(of(boss).cpu_quota_percent, None);
        assert_eq!(of(boss).pids_max, None, "nor this one");

        assert_eq!(of(max).memory_high_bytes, Some(4096 * 1024 * 1024));
        assert_eq!(of(max).cpu_quota_percent, Some(200));
    }

    #[tokio::test]
    async fn a_failed_account_is_left_for_the_admin_to_retry() {
        let pool = test_pool().await;
        let failed = a_user(&pool, "max").await;
        sqlx::query("UPDATE users SET system_state = 'error' WHERE id = ?")
            .bind(failed)
            .execute(&pool)
            .await
            .unwrap();

        let fake = FakeHelper::obliging().await;
        reconcile(&pool, &Helper::new(fake.socket())).await.unwrap();

        assert!(fake.calls().is_empty(), "12.9 is the way back from 'error', not a reboot");
        assert_eq!(load(&pool, failed).await.unwrap().system_state, SystemUserState::Error);
    }

    #[tokio::test]
    async fn a_page_carries_the_total_behind_it() {
        let pool = test_pool().await;
        for name in ["anna", "andre", "max"] {
            a_user(&pool, name).await;
        }

        let (rows, total) = page(&pool, None, 2, 0).await.unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(total, 3, "the total counts past the page");

        let (filtered, total) = page(&pool, Some("an"), 50, 0).await.unwrap();
        assert_eq!(filtered.len(), 2);
        assert_eq!(total, 2);

        let (second, _) = page(&pool, None, 2, 2).await.unwrap();
        assert_eq!(second.len(), 1);
    }

    #[tokio::test]
    async fn the_admin_list_looks_inside_a_name_and_the_invite_search_at_its_start() {
        let pool = test_pool().await;
        a_user(&pool, "hans").await;
        a_user(&pool, "anna").await;

        let (found, total) = page(&pool, Some("an"), 50, 0).await.unwrap();
        assert_eq!(total, 2, "'hans' holds an 'an' too");
        assert_eq!(found.len(), 2);

        let invited = search(&pool, "an", 25).await.unwrap();
        assert_eq!(invited.len(), 1, "3.5 says prefix");
        assert_eq!(invited[0].username, "anna");
    }
}
