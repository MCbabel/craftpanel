use std::collections::HashMap;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use sqlx::SqlitePool;

use super::error::{Failure, Result};
use super::users;
use crate::model::{Id, Timestamp};

pub const WINDOW: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Space {
    pub servers_bytes: u64,
    pub backups_bytes: u64,
    pub complete: bool,
    pub measured_at: Timestamp,
}

impl Space {
    pub fn used_bytes(self) -> u64 {
        self.servers_bytes.saturating_add(self.backups_bytes)
    }
}

type Answer = Pin<Box<dyn Future<Output = Space> + Send>>;

#[derive(Clone)]
pub struct Disks(Arc<dyn Fn(Id) -> Answer + Send + Sync>);

impl Disks {
    pub fn none() -> Self {
        Self::fixed(0, 0)
    }

    pub fn fixed(servers_bytes: u64, backups_bytes: u64) -> Self {
        Self::from_fn(move |_| async move {
            Space {
                servers_bytes,
                backups_bytes,
                complete: true,
                measured_at: Timestamp::now(),
            }
        })
    }

    pub fn from_fn<F, Fut>(read: F) -> Self
    where
        F: Fn(Id) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Space> + Send + 'static,
    {
        Self(Arc::new(move |user| Box::pin(read(user))))
    }

    pub fn over(
        pool: SqlitePool,
        data_dir: PathBuf,
        window: Duration,
        helper: crate::helper::Helper,
    ) -> Self {
        Self::over_walk(pool, data_dir, window, helper, Arc::new(crate::files::measure))
    }

    fn over_walk(
        pool: SqlitePool,
        data_dir: PathBuf,
        window: Duration,
        helper: crate::helper::Helper,
        walk: Walk,
    ) -> Self {
        let seen = Mutex::default();
        let meter = Arc::new(Meter { pool, data_dir, window, helper, walk, seen });
        Self::from_fn(move |user| {
            let meter = Arc::clone(&meter);
            async move { meter.of(user).await }
        })
    }

    pub async fn of(&self, user: Id) -> Space {
        (self.0)(user).await
    }
}

impl std::fmt::Debug for Disks {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Disks")
    }
}

type Walk = Arc<dyn Fn(&std::path::Path) -> crate::files::Measured + Send + Sync>;

struct Meter {
    pool: SqlitePool,
    data_dir: PathBuf,
    window: Duration,
    helper: crate::helper::Helper,
    walk: Walk,
    seen: Mutex<HashMap<Id, (Instant, Space)>>,
}

impl Meter {
    async fn of(&self, user: Id) -> Space {
        if let Some((at, space)) = self.remembered(user) {
            if at.elapsed() < self.window {
                return space;
            }
        }

        let dir = self.data_dir.join("users").join(user.to_string()).join("servers");
        let mut servers = self.count(&dir).await;
        if !servers.complete() {
            servers = self.count_again(user, &dir, servers).await;
        }
        let backups_bytes = backups_bytes(&self.pool, user).await.unwrap_or_else(|err| {
            tracing::warn!(user = %user, "the backup sizes could not be read: {err}");
            0
        });

        let space = Space {
            servers_bytes: servers.bytes,
            backups_bytes,
            complete: servers.complete(),
            measured_at: Timestamp::now(),
        };
        self.seen
            .lock()
            .expect("the disk cache outlives its panics")
            .insert(user, (Instant::now(), space));
        space
    }

    async fn count(&self, dir: &std::path::Path) -> crate::files::Measured {
        let walk = Arc::clone(&self.walk);
        let dir = dir.to_path_buf();
        tokio::task::spawn_blocking(move || walk(&dir)).await.unwrap_or_default()
    }

    async fn count_again(
        &self,
        user: Id,
        dir: &std::path::Path,
        first: crate::files::Measured,
    ) -> crate::files::Measured {
        tracing::warn!(
            user = %user,
            "{} directories of this account are closed to the panel; asking the helper to hand \
             them back so that what they hold can be counted",
            first.unreadable
        );
        let steps = crate::helper::all_servers();
        if let Err(err) = self.helper.chown_tree(&user.to_string(), steps).await {
            tracing::warn!(user = %user, "the tree could not be handed back: {err:#}");
            return first;
        }

        let again = self.count(dir).await;
        if !again.complete() {
            tracing::warn!(
                user = %user,
                "{} directories are still closed after the hand-back; this account's figure is a \
                 floor and nothing new may be written",
                again.unreadable
            );
        }
        again
    }

    fn remembered(&self, user: Id) -> Option<(Instant, Space)> {
        self.seen.lock().expect("the disk cache outlives its panics").get(&user).copied()
    }
}

async fn backups_bytes(pool: &SqlitePool, user: Id) -> sqlx::Result<u64> {
    let bytes: i64 = sqlx::query_scalar(
        "SELECT coalesce(sum(b.size_bytes), 0) FROM backups b \
         JOIN servers s ON s.id = b.server_id \
         WHERE s.owner_id = ? AND b.location = 'local'",
    )
    .bind(user)
    .fetch_one(pool)
    .await?;
    Ok(bytes.max(0) as u64)
}

pub fn spawn_sweep(pool: SqlitePool, disks: Disks) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(WINDOW);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tick.tick().await;
            let everyone: Vec<Id> = match sqlx::query_scalar("SELECT id FROM users")
                .fetch_all(&pool)
                .await
            {
                Ok(ids) => ids,
                Err(err) => {
                    tracing::warn!("the accounts could not be listed for the disk sweep: {err}");
                    continue;
                }
            };
            for user in everyone {
                disks.of(user).await;
            }
        }
    })
}

pub async fn guard(pool: &SqlitePool, disks: &Disks, owner: Id, wanted_bytes: u64) -> Result<()> {
    let row = users::load(pool, owner).await?;
    let budget = row.budget();
    let Some(limit) = budget.disk_limit_bytes() else { return Ok(()) };

    let space = disks.of(owner).await;
    let used = space.used_bytes();
    if !budget.has_disk_room_for(used, wanted_bytes) {
        return Err(Failure::conflict(
            "disk_limit_reached",
            format!(
                "{} MiB is the whole disk budget of this account, and {} MiB of it is used",
                limit / MIB,
                used / MIB
            ),
        ));
    }

    if !space.complete {
        return Err(Failure::conflict(
            "disk_usage_unknown",
            format!(
                "a directory of this account stayed closed even to the helper, so at least {} \
                 MiB of the {} MiB budget is used and how much more cannot be said. Nothing new \
                 is written until it can be counted; an administrator can look at it",
                used / MIB,
                limit / MIB
            ),
        ));
    }

    Ok(())
}

const MIB: u64 = 1024 * 1024;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::harness::{a_server, a_user, an_admin, test_pool, FakeHelper};
    use crate::helper::Helper;

    struct Scratch(PathBuf);

    impl Scratch {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!("craftpanel-disk-{}", Id::new()));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn a_file(dir: &std::path::Path, name: &str, bytes: usize) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join(name), vec![b'x'; bytes]).unwrap();
    }

    fn a_meter(pool: &SqlitePool, scratch: &Scratch, window: Duration) -> Disks {
        let nobody = Helper::new(scratch.0.join("no-helper.sock"));
        Disks::over(pool.clone(), scratch.0.clone(), window, nobody)
    }

    async fn a_backup(pool: &SqlitePool, server: Id, size_bytes: u64) {
        sqlx::query(
            "INSERT INTO backups (id, server_id, name, size_bytes, created_at) \
             VALUES (?, ?, 'a backup', ?, ?)",
        )
        .bind(Id::new())
        .bind(server)
        .bind(size_bytes as i64)
        .bind(Timestamp::now())
        .execute(pool)
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn the_servers_and_the_backups_are_counted_separately() {
        let pool = test_pool().await;
        let scratch = Scratch::new();
        let max = a_user(&pool, "max").await;
        let one = a_server(&pool, max, "one", 2048).await;
        let two = a_server(&pool, max, "two", 2048).await;

        let mine = scratch.0.join("users").join(max.to_string()).join("servers");
        a_file(&mine.join(one.to_string()), "world.dat", 1000);
        a_file(&mine.join(one.to_string()).join("plugins"), "a.jar", 500);
        a_file(&mine.join(two.to_string()), "world.dat", 300);
        let anna = a_user(&pool, "anna").await;
        a_file(&scratch.0.join("users").join(anna.to_string()).join("servers"), "x", 99);

        a_backup(&pool, one, 7000).await;
        a_backup(&pool, two, 3000).await;

        let disks = a_meter(&pool, &scratch, Duration::ZERO);
        let space = disks.of(max).await;
        assert_eq!(space.servers_bytes, 1800);
        assert_eq!(space.backups_bytes, 10_000);
        assert_eq!(space.used_bytes(), 11_800);

        assert_eq!(disks.of(anna).await.servers_bytes, 99);
        assert_eq!(disks.of(anna).await.backups_bytes, 0);
    }

    #[tokio::test]
    async fn a_second_question_inside_the_window_gets_the_first_answer() {
        let pool = test_pool().await;
        let scratch = Scratch::new();
        let max = a_user(&pool, "max").await;
        let mine = scratch.0.join("users").join(max.to_string()).join("servers");
        a_file(&mine, "world.dat", 100);

        let cached = a_meter(&pool, &scratch, Duration::from_secs(600));
        assert_eq!(cached.of(max).await.servers_bytes, 100);
        a_file(&mine, "world.dat", 900);
        assert_eq!(cached.of(max).await.servers_bytes, 100, "the window has not passed");

        let fresh = a_meter(&pool, &scratch, Duration::ZERO);
        assert_eq!(fresh.of(max).await.servers_bytes, 900, "without a window it looks again");
    }

    #[tokio::test]
    async fn an_account_with_nothing_on_disk_reads_as_zero_rather_than_failing() {
        let pool = test_pool().await;
        let scratch = Scratch::new();
        let max = a_user(&pool, "max").await;

        let space = a_meter(&pool, &scratch, Duration::ZERO).of(max).await;
        assert_eq!(space.used_bytes(), 0);
    }

    fn a_walk_refused_first(
        first: crate::files::Measured,
        then: crate::files::Measured,
    ) -> (Walk, Arc<Mutex<u32>>) {
        let walks = Arc::new(Mutex::new(0u32));
        let counted = Arc::clone(&walks);
        let walk: Walk = Arc::new(move |_| {
            let mut done = counted.lock().unwrap();
            *done += 1;
            if *done == 1 {
                first
            } else {
                then
            }
        });
        (walk, walks)
    }

    fn chown_calls(helper: &FakeHelper) -> Vec<(String, Vec<String>)> {
        helper
            .calls()
            .into_iter()
            .filter_map(|call| match call {
                craftpanel_proto::HelperRequest::ChownTree { user_id, steps } => {
                    Some((user_id, steps))
                }
                _ => None,
            })
            .collect()
    }

    #[tokio::test]
    async fn a_directory_the_panel_was_refused_is_handed_back_and_then_counted() {
        let pool = test_pool().await;
        let scratch = Scratch::new();
        let max = a_user(&pool, "max").await;
        let helper = FakeHelper::obliging().await;

        let shut = crate::files::Measured { bytes: 100, unreadable: 1 };
        let open = crate::files::Measured { bytes: 900, unreadable: 0 };
        let (walk, walks) = a_walk_refused_first(shut, open);
        let disks = Disks::over_walk(
            pool.clone(),
            scratch.0.clone(),
            Duration::ZERO,
            Helper::new(helper.socket()),
            walk,
        );

        let space = disks.of(max).await;

        assert_eq!(space.servers_bytes, 900, "the figure is the one from after the hand-back");
        assert!(space.complete, "and it is a figure and not a floor");
        assert_eq!(*walks.lock().unwrap(), 2);
        assert_eq!(
            chown_calls(&helper),
            vec![(max.to_string(), crate::helper::all_servers())],
            "this account's servers directory, and nobody else's"
        );
    }

    #[tokio::test]
    async fn a_door_that_stays_shut_leaves_the_figure_a_floor() {
        let pool = test_pool().await;
        let scratch = Scratch::new();
        let max = a_user(&pool, "max").await;
        let helper = FakeHelper::obliging().await;

        let shut = crate::files::Measured { bytes: 100, unreadable: 2 };
        let (walk, walks) = a_walk_refused_first(shut, shut);
        let disks = Disks::over_walk(
            pool.clone(),
            scratch.0.clone(),
            Duration::ZERO,
            Helper::new(helper.socket()),
            walk,
        );

        let space = disks.of(max).await;

        assert_eq!(space.servers_bytes, 100);
        assert!(!space.complete, "what could not be counted is not passed over in silence");
        assert_eq!(*walks.lock().unwrap(), 2, "counted twice, not until it gives in");
        assert_eq!(chown_calls(&helper).len(), 1);
    }

    #[tokio::test]
    async fn a_helper_that_does_not_answer_leaves_the_floor_standing() {
        let pool = test_pool().await;
        let scratch = Scratch::new();
        let max = a_user(&pool, "max").await;

        let shut = crate::files::Measured { bytes: 100, unreadable: 1 };
        let (walk, walks) = a_walk_refused_first(shut, crate::files::Measured::default());
        let disks = Disks::over_walk(
            pool.clone(),
            scratch.0.clone(),
            Duration::ZERO,
            Helper::new(scratch.0.join("no-helper.sock")),
            walk,
        );

        let space = disks.of(max).await;

        assert_eq!(space.servers_bytes, 100, "the floor, not a zero");
        assert!(!space.complete);
        assert_eq!(*walks.lock().unwrap(), 1, "no second walk without a hand-back");
    }

    #[tokio::test]
    async fn the_door_refuses_what_would_not_fit_and_names_the_reason() {
        let pool = test_pool().await;
        let max = a_user(&pool, "max").await;
        sqlx::query("UPDATE users SET disk_mib = 1024 WHERE id = ?")
            .bind(max)
            .execute(&pool)
            .await
            .unwrap();

        let nearly_full = Disks::fixed(1000 * MIB, 0);
        assert!(guard(&pool, &nearly_full, max, 24 * MIB).await.is_ok(), "exactly full still fits");

        let refusal = guard(&pool, &nearly_full, max, 25 * MIB).await.unwrap_err();
        assert_eq!(refusal.code(), "disk_limit_reached");
        assert_eq!(refusal.status(), axum::http::StatusCode::CONFLICT);
        assert!(refusal.to_string().contains("1024"), "{refusal}");

        let over = Disks::fixed(2048 * MIB, 0);
        assert_eq!(
            guard(&pool, &over, max, 0).await.unwrap_err().code(),
            "disk_limit_reached",
            "asking for nothing while already over is still refused"
        );
    }

    #[tokio::test]
    async fn the_door_refuses_while_the_figure_is_only_a_floor() {
        let pool = test_pool().await;
        let max = a_user(&pool, "max").await;
        sqlx::query("UPDATE users SET disk_mib = 1024 WHERE id = ?")
            .bind(max)
            .execute(&pool)
            .await
            .unwrap();

        let floor = a_floor_of(100 * MIB);
        let refusal = guard(&pool, &floor, max, 1).await.unwrap_err();
        assert_eq!(refusal.code(), "disk_usage_unknown");
        assert_eq!(refusal.status(), axum::http::StatusCode::CONFLICT);
        assert!(refusal.to_string().contains("at least 100"), "{refusal}");

        assert!(
            guard(&pool, &Disks::fixed(100 * MIB, 0), max, 1).await.is_ok(),
            "the same 100 MiB counted all the way through leaves room"
        );

        assert_eq!(
            guard(&pool, &a_floor_of(2048 * MIB), max, 0).await.unwrap_err().code(),
            "disk_limit_reached"
        );
    }

    fn a_floor_of(servers_bytes: u64) -> Disks {
        Disks::from_fn(move |_| async move {
            Space {
                servers_bytes,
                backups_bytes: 0,
                complete: false,
                measured_at: Timestamp::now(),
            }
        })
    }

    #[tokio::test]
    async fn an_administrator_passes_the_door_with_anything() {
        let pool = test_pool().await;
        let boss = an_admin(&pool, "boss").await;

        let full = Disks::fixed(u64::MAX / 2, u64::MAX / 2);
        assert!(guard(&pool, &full, boss, u64::MAX).await.is_ok());
        assert!(
            guard(&pool, &a_floor_of(100 * MIB), boss, u64::MAX).await.is_ok(),
            "and has no limit to be unsure about either"
        );
    }
}
