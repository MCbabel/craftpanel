use std::sync::Arc;
use std::time::Duration;

use crate::auth::error::{Failure, Result};
use crate::model::{BackupSchedule, BackupScheduleStatus, Id, Timestamp};

use super::store;
use super::Backups;

const TICK: Duration = Duration::from_secs(60);

pub const MIN_INTERVAL: u32 = 1;
pub const MAX_INTERVAL: u32 = 168;

#[derive(Debug, Clone, serde::Deserialize)]
pub struct UpdateBackupScheduleRequest {
    pub enabled: bool,
    pub interval_hours: u32,
    pub hour_utc: u8,
    pub keep_last: u32,
}

impl UpdateBackupScheduleRequest {
    pub fn check(&self, max_backups: u32) -> Result<()> {
        let refuse = |message: String| Err(Failure::bad_request("invalid_schedule", message));
        if !(MIN_INTERVAL..=MAX_INTERVAL).contains(&self.interval_hours) {
            return refuse(format!(
                "interval_hours is {} and has to be between {MIN_INTERVAL} and {MAX_INTERVAL}",
                self.interval_hours
            ));
        }
        if self.hour_utc > 23 {
            return refuse(format!("hour_utc is {} and has to be between 0 and 23", self.hour_utc));
        }
        if self.keep_last < 1 || self.keep_last > store::MAX_QUOTA {
            return refuse(format!(
                "keep_last is {} and has to be between 1 and {}",
                self.keep_last,
                store::MAX_QUOTA
            ));
        }
        if self.keep_last > max_backups {
            return refuse(format!(
                "keep_last is {} and cannot be more than the quota of {max_backups}",
                self.keep_last
            ));
        }
        Ok(())
    }

    pub fn into_schedule(self, next_run_at: Option<Timestamp>) -> BackupSchedule {
        BackupSchedule {
            enabled: self.enabled,
            interval_hours: self.interval_hours,
            hour_utc: self.hour_utc,
            keep_last: self.keep_last,
            next_run_at,
            last_run_at: None,
            last_status: None,
            last_error: None,
        }
    }
}

pub fn next_after(from: Timestamp, interval_hours: u32, hour_utc: u8) -> Timestamp {
    let from = from.as_datetime();
    if interval_hours % 24 == 0 {
        let days = (interval_hours / 24).max(1) as i64;
        let at = time::Time::from_hms(hour_utc.min(23), 0, 0).expect("an hour below 24");
        let mut candidate = from.replace_time(at);
        while candidate <= from {
            candidate += time::Duration::days(days);
        }
        Timestamp::at(candidate)
    } else {
        Timestamp::at(from + time::Duration::hours(interval_hours as i64))
    }
}

impl Backups {
    pub async fn tick(self: &Arc<Self>, now: Timestamp) -> Vec<(Id, BackupScheduleStatus)> {
        let due = match store::due(self.pool(), now).await {
            Ok(due) => due,
            Err(err) => {
                tracing::error!("the backup schedule could not be read: {}", err);
                return Vec::new();
            }
        };

        let mut done = Vec::new();
        for server in due {
            match self.run_scheduled(server, now).await {
                Ok(status) => done.push((server, status)),
                Err(err) => tracing::error!(%server, "the scheduled backup failed: {err}"),
            }
        }
        done
    }

    async fn run_scheduled(
        self: &Arc<Self>,
        server: Id,
        now: Timestamp,
    ) -> Result<BackupScheduleStatus> {
        let schedule = store::schedule(self.pool(), server).await?;
        let next = next_after(now, schedule.interval_hours, schedule.hour_utc);

        store::reserve(self.pool(), server, next).await?;

        let settle = |status, error: Option<String>| {
            let pool = self.pool().clone();
            async move {
                store::record_run(&pool, server, now, status, error.as_deref(), Some(next)).await
            }
        };

        if store::count(self.pool(), server).await? >= store::quota(self.pool()).await? {
            settle(BackupScheduleStatus::SkippedLimit, None).await?;
            return Ok(BackupScheduleStatus::SkippedLimit);
        }
        if !self.changed_since_last_automatic(server).await? {
            settle(BackupScheduleStatus::SkippedUnchanged, None).await?;
            return Ok(BackupScheduleStatus::SkippedUnchanged);
        }

        let name = format!("Automatic backup {now}");
        let queued = self.create(server, &name, None, true).await?;
        self.run(queued.operation).await;

        let outcome = store::newest_run(self.pool(), queued.backup).await?;
        let (status, error) = match outcome.map(|run| run.state) {
            Some(crate::model::BackupOperationState::Completed) => {
                self.dismiss(server, queued.operation).await;
                (BackupScheduleStatus::Completed, None)
            }
            Some(crate::model::BackupOperationState::TimedOut) => {
                (BackupScheduleStatus::TimedOut, Some("the run timed out".to_owned()))
            }
            _ => (BackupScheduleStatus::Failed, Some("the run did not finish".to_owned())),
        };
        settle(status, error).await?;

        if status == BackupScheduleStatus::Completed {
            self.prune(server, schedule.keep_last).await?;
        }
        Ok(status)
    }

    async fn changed_since_last_automatic(&self, server: Id) -> Result<bool> {
        let Some(finished) = store::newest_automatic_finish(self.pool(), server).await? else {
            return Ok(true);
        };
        let directory = self.server_dir(server).await?;
        let plan = tokio::task::spawn_blocking(move || super::archive::survey(&directory))
            .await
            .map_err(|err| Failure::internal(anyhow::anyhow!("{err}")))?
            .map_err(Failure::internal)?;
        let Some(newest) = plan.newest else {
            return Ok(false);
        };
        Ok(Timestamp::at(newest.into()) > finished)
    }

    pub async fn prune(self: &Arc<Self>, server: Id, keep_last: u32) -> Result<usize> {
        let doomed = store::automatic_over(self.pool(), server, keep_last).await?;
        let mut gone = 0;
        for backup in doomed {
            if store::is_busy(self.pool(), backup).await? {
                continue;
            }
            self.forget(server, backup).await?;
            gone += 1;
        }
        if gone > 0 {
            self.announce(server).await;
        }
        Ok(gone)
    }

    pub fn spawn_scheduler(self: &Arc<Self>) -> tokio::task::JoinHandle<()> {
        let backups = Arc::clone(self);
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(TICK);
            loop {
                tick.tick().await;
                backups.tick(Timestamp::now()).await;
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backups::testing::FakeServer;
    use crate::model::BackupStatus;

    fn at(text: &str) -> Timestamp {
        text.parse().expect("a timestamp")
    }

    #[test]
    fn a_daily_schedule_keeps_its_hour_and_an_odd_interval_just_counts_hours() {
        assert_eq!(
            next_after(at("2026-08-12T02:00:00Z"), 24, 4),
            at("2026-08-12T04:00:00Z"),
            "later today"
        );
        assert_eq!(
            next_after(at("2026-08-12T04:00:00Z"), 24, 4),
            at("2026-08-13T04:00:00Z"),
            "the hour has just gone, so tomorrow"
        );
        assert_eq!(
            next_after(at("2026-08-12T05:00:00Z"), 48, 4),
            at("2026-08-14T04:00:00Z"),
            "two whole days, still at the hour"
        );
        assert_eq!(
            next_after(at("2026-08-12T05:30:00Z"), 6, 4),
            at("2026-08-12T11:30:00Z"),
            "10.10: hour_utc is only read when the interval is whole days"
        );
    }

    #[test]
    fn the_three_limits_of_10_10_name_the_field_they_are_about() {
        let good = |interval, hour, keep| UpdateBackupScheduleRequest {
            enabled: true,
            interval_hours: interval,
            hour_utc: hour,
            keep_last: keep,
        };
        assert!(good(24, 4, 5).check(10).is_ok());

        for (request, field) in [
            (good(0, 4, 5), "interval_hours"),
            (good(169, 4, 5), "interval_hours"),
            (good(24, 24, 5), "hour_utc"),
            (good(24, 4, 0), "keep_last"),
            (good(24, 4, 51), "keep_last"),
        ] {
            let refused = request.check(50).expect_err("out of bounds");
            assert_eq!(refused.code(), "invalid_schedule");
            assert!(format!("{refused}").contains(field), "{refused} should name {field}");
        }

        let over_quota = good(24, 4, 20).check(10).expect_err("keep_last above the quota");
        assert!(format!("{over_quota}").contains("quota"));
    }

    #[tokio::test]
    async fn a_due_schedule_makes_an_automatic_backup_and_does_not_prompt_about_it() {
        let game = FakeServer::stopped().await;
        game.file("world/level.dat", b"a world");
        enable(&game, 24, 5).await;

        let now = Timestamp::now();
        let done = game.backups.tick(now).await;
        assert_eq!(done, vec![(game.server, BackupScheduleStatus::Completed)]);

        let list = game.backups.list(game.server).await.expect("a list");
        assert_eq!(list.backups.len(), 1);
        let made = &list.backups[0];
        assert!(made.automated, "the pill 'Auto' has to find something");
        assert_eq!(made.status, BackupStatus::Done);
        assert!(
            !made.history[0].should_prompt,
            "10.1: a successful automatic backup must not greet its owner every morning"
        );

        let schedule = game.backups.schedule(game.server).await.expect("a schedule");
        assert_eq!(schedule.last_status, Some(BackupScheduleStatus::Completed));
        assert!(schedule.next_run_at.expect("a next time") > now);
    }

    #[tokio::test]
    async fn a_server_that_has_not_changed_is_skipped_and_a_changed_one_is_not() {
        let game = FakeServer::stopped().await;
        game.file("world/level.dat", b"a world");
        enable(&game, 1, 5).await;

        assert_eq!(
            game.backups.tick(Timestamp::now()).await,
            vec![(game.server, BackupScheduleStatus::Completed)]
        );

        due_now(&game).await;
        assert_eq!(
            game.backups.tick(Timestamp::now()).await,
            vec![(game.server, BackupScheduleStatus::SkippedUnchanged)],
            "nothing moved, so a second copy of the same bytes is waste"
        );
        assert_eq!(game.backup_rows().await, 1);

        tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
        game.file("world/region/r.0.0.mca", b"new chunks");
        due_now(&game).await;
        assert_eq!(
            game.backups.tick(Timestamp::now()).await,
            vec![(game.server, BackupScheduleStatus::Completed)]
        );
        assert_eq!(game.backup_rows().await, 2);
    }

    #[tokio::test]
    async fn keep_last_takes_automatic_backups_and_never_a_hand_made_one() {
        let game = FakeServer::stopped().await;
        game.file("world/level.dat", b"a world");
        let by_hand = game.a_finished_backup("mine").await;

        for index in 0..3 {
            let queued = game
                .backups
                .create(game.server, &format!("auto {index}"), None, true)
                .await
                .expect("an automatic backup");
            game.backups.run(queued.operation).await;
        }
        assert_eq!(game.backup_rows().await, 4, "one by hand and three automatic ones");

        let gone = game.backups.prune(game.server, 1).await.expect("a prune");
        assert_eq!(gone, 2);

        let left = game.backups.list(game.server).await.expect("a list");
        assert_eq!(left.backups.len(), 2);
        assert!(
            left.backups.iter().any(|backup| backup.id == by_hand),
            "10.12: a rule never takes away what somebody made by hand"
        );
        assert_eq!(left.backups.iter().filter(|backup| backup.automated).count(), 1);
    }

    #[tokio::test]
    async fn a_failed_automatic_backup_does_prompt_and_says_so_in_the_schedule() {
        let game = FakeServer::stopped().await;
        game.file("world/level.dat", b"a world");
        enable(&game, 24, 5).await;

        std::fs::create_dir_all(game.backups.root()).expect("the backup root");
        std::fs::write(game.backups.dir_of(game.server), b"in the way").expect("an obstacle");

        assert_eq!(
            game.backups.tick(Timestamp::now()).await,
            vec![(game.server, BackupScheduleStatus::Failed)]
        );

        let list = game.backups.list(game.server).await.expect("a list");
        assert_eq!(list.backups[0].status, BackupStatus::Error);
        assert!(
            list.backups[0].history[0].should_prompt,
            "10.1: that the schedule is not running has to reach somebody"
        );

        let schedule = game.backups.schedule(game.server).await.expect("a schedule");
        assert_eq!(schedule.last_status, Some(BackupScheduleStatus::Failed));
        assert!(schedule.last_error.is_some());
    }

    #[tokio::test]
    async fn a_tick_that_cannot_do_its_work_still_gives_up_its_slot() {
        let game = FakeServer::stopped().await;
        game.file("world/level.dat", b"a world");
        enable(&game, 24, 5).await;
        game.backups.create(game.server, "by hand", None, false).await.expect("a queued backup");

        let before = Timestamp::now();
        assert!(game.backups.tick(before).await.is_empty(), "nothing was made");

        let schedule = game.backups.schedule(game.server).await.expect("a schedule");
        assert!(schedule.next_run_at.expect("a next time") > before);
        assert_eq!(game.backup_rows().await, 1, "and no half made row was left behind");
    }

    #[tokio::test]
    async fn a_full_quota_is_recorded_and_nothing_is_deleted_to_make_room() {
        let game = FakeServer::stopped().await;
        game.file("world/level.dat", b"a world");
        game.a_finished_backup("mine").await;
        game.set_quota(1).await;
        enable(&game, 1, 0).await;

        assert_eq!(
            game.backups.tick(Timestamp::now()).await,
            vec![(game.server, BackupScheduleStatus::SkippedLimit)]
        );
        assert_eq!(game.backup_rows().await, 1);
    }

    async fn enable(game: &FakeServer, interval_hours: u32, keep_last: u32) {
        let wanted = UpdateBackupScheduleRequest {
            enabled: true,
            interval_hours,
            hour_utc: 4,
            keep_last: keep_last.max(1),
        };
        game.backups.write_schedule(game.server, wanted).await.expect("a schedule");
        due_now(game).await;
    }

    async fn due_now(game: &FakeServer) {
        sqlx::query("UPDATE backup_schedules SET next_run_at = '2020-01-01T00:00:00Z'")
            .execute(game.pool())
            .await
            .expect("a due schedule");
    }
}
