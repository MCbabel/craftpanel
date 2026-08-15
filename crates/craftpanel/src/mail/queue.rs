use std::sync::Arc;
use std::time::Duration;

use crate::model::Timestamp;

use super::Mail;

pub const PACE: Duration = Duration::from_millis(500);
pub const TICK: Duration = Duration::from_secs(30);

pub const BACKOFF: [Duration; 5] = [
    Duration::from_secs(30),
    Duration::from_secs(2 * 60),
    Duration::from_secs(8 * 60),
    Duration::from_secs(30 * 60),
    Duration::from_secs(2 * 60 * 60),
];

pub fn next_attempt(attempts: u32, now: Timestamp) -> Option<Timestamp> {
    let wait = BACKOFF.get(attempts.checked_sub(1)? as usize)?;
    Some(Timestamp::at(now.as_datetime() + *wait))
}

pub fn next_utc_day(now: Timestamp) -> Timestamp {
    let tomorrow = now.as_datetime().date().next_day().unwrap_or(now.as_datetime().date());
    Timestamp::at(tomorrow.midnight().assume_utc())
}

pub fn spawn(mail: Arc<Mail>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        match mail.requeue_stuck().await {
            Ok(0) => {}
            Ok(count) => tracing::info!(count, "mail left mid-send by a restart is queued again"),
            Err(err) => tracing::error!("the mail queue could not be picked up: {err}"),
        }

        loop {
            match mail.deliver_next(Timestamp::now()).await {
                Ok(true) => tokio::time::sleep(PACE).await,
                Ok(false) => mail.wait_for_work(TICK).await,
                Err(err) => {
                    tracing::error!("the mail worker stumbled: {err}");
                    mail.wait_for_work(TICK).await;
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(text: &str) -> Timestamp {
        text.parse().expect("a timestamp")
    }

    #[test]
    fn the_waits_grow_and_then_stop() {
        let now = at("2026-08-13T21:10:00Z");
        assert_eq!(next_attempt(1, now), Some(at("2026-08-13T21:10:30Z")));
        assert_eq!(next_attempt(2, now), Some(at("2026-08-13T21:12:00Z")));
        assert_eq!(next_attempt(3, now), Some(at("2026-08-13T21:18:00Z")));
        assert_eq!(next_attempt(4, now), Some(at("2026-08-13T21:40:00Z")));
        assert_eq!(next_attempt(5, now), Some(at("2026-08-13T23:10:00Z")));
        assert_eq!(next_attempt(6, now), None, "five waits and the row is written off");
        assert_eq!(next_attempt(0, now), None);
    }

    #[test]
    fn the_window_is_the_two_and_a_half_hours_the_contract_promises() {
        let total: Duration = BACKOFF.iter().sum();
        assert!(total <= Duration::from_secs(3 * 60 * 60), "{total:?}");
        assert!(total >= Duration::from_secs(2 * 60 * 60), "{total:?}");
    }

    #[test]
    fn a_used_up_daily_quota_waits_for_midnight_and_not_a_second_longer() {
        assert_eq!(next_utc_day(at("2026-08-13T21:10:00Z")), at("2026-08-14T00:00:00Z"));
        assert_eq!(next_utc_day(at("2026-08-13T00:00:00Z")), at("2026-08-14T00:00:00Z"));
        assert_eq!(next_utc_day(at("2026-12-31T23:59:59Z")), at("2027-01-01T00:00:00Z"));
    }
}
