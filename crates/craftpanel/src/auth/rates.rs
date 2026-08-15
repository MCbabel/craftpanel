use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

const HOUR: Duration = Duration::from_secs(60 * 60);
const DAY: Duration = Duration::from_secs(24 * 60 * 60);

const KEY_LENGTH: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limit {
    bucket: &'static str,
    allowance: u32,
    window: Duration,
}

impl Limit {
    pub const fn new(bucket: &'static str, allowance: u32, window: Duration) -> Self {
        Self { bucket, allowance, window }
    }

    pub const fn allowance(&self) -> u32 {
        self.allowance
    }
}

pub const REGISTER_PER_HOUR: Limit = Limit::new("register", 3, HOUR);
pub const REGISTER_PER_DAY: Limit = Limit::new("register_day", 10, DAY);

pub const RESEND_PER_ADDRESS: Limit = Limit::new("resend", 1, Duration::from_secs(5 * 60));

pub const RESET_ATTEMPTS: Limit = Limit::new("reset", 10, Duration::from_secs(15 * 60));

pub const ADMIN_RESET_PER_ACCOUNT: Limit = Limit::new("admin_reset", 5, HOUR);

#[derive(Default)]
pub struct Buckets {
    records: Mutex<HashMap<(&'static str, String), VecDeque<Instant>>>,
}

pub fn shared() -> Arc<Buckets> {
    static SHARED: OnceLock<Arc<Buckets>> = OnceLock::new();
    Arc::clone(SHARED.get_or_init(|| Arc::new(Buckets::default())))
}

impl Buckets {
    pub fn take(&self, limit: Limit, key: &str, now: Instant) -> Option<u64> {
        let mut records = self.records.lock().expect("the rate buckets outlive their panics");
        self.sweep(&mut records, now);

        let times = records.entry((limit.bucket, cut(key))).or_default();
        while times.front().is_some_and(|first| now.duration_since(*first) >= limit.window) {
            times.pop_front();
        }

        if times.len() as u32 >= limit.allowance {
            let oldest = *times.front().expect("a full bucket has a front");
            let left = limit.window.saturating_sub(now.duration_since(oldest));
            return Some(left.as_secs() + u64::from(left.subsec_nanos() > 0));
        }

        times.push_back(now);
        None
    }

    pub fn check(&self, limit: Limit, key: &str, now: Instant) -> Option<u64> {
        let records = self.records.lock().expect("the rate buckets outlive their panics");
        let Some(times) = records.get(&(limit.bucket, cut(key))) else {
            return None;
        };
        let inside =
            times.iter().filter(|at| now.duration_since(**at) < limit.window).count() as u32;
        if inside < limit.allowance {
            return None;
        }
        let oldest = times
            .iter()
            .find(|at| now.duration_since(**at) < limit.window)
            .copied()
            .unwrap_or(now);
        let left = limit.window.saturating_sub(now.duration_since(oldest));
        Some(left.as_secs() + u64::from(left.subsec_nanos() > 0))
    }

    pub fn note(&self, limit: Limit, key: &str, now: Instant) {
        let mut records = self.records.lock().expect("the rate buckets outlive their panics");
        self.sweep(&mut records, now);

        let times = records.entry((limit.bucket, cut(key))).or_default();
        while times.front().is_some_and(|first| now.duration_since(*first) >= limit.window) {
            times.pop_front();
        }
        times.push_back(now);
    }

    fn sweep(&self, records: &mut HashMap<(&'static str, String), VecDeque<Instant>>, now: Instant) {
        records.retain(|_, times| {
            times.back().is_some_and(|last| now.duration_since(*last) < DAY)
        });
    }

    #[cfg(test)]
    fn size(&self) -> usize {
        self.records.lock().unwrap().len()
    }
}

fn cut(key: &str) -> String {
    key.chars().take(KEY_LENGTH).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn three_sign_ups_an_hour_and_the_fourth_waits() {
        let buckets = Buckets::default();
        let now = Instant::now();

        for _ in 0..3 {
            assert_eq!(buckets.take(REGISTER_PER_HOUR, "198.51.100.7", now), None);
        }

        let wait = buckets.take(REGISTER_PER_HOUR, "198.51.100.7", now).expect("the fourth waits");
        assert_eq!(wait, HOUR.as_secs(), "the whole window, since all three were just now");
        assert_eq!(
            buckets.take(REGISTER_PER_HOUR, "203.0.113.9", now),
            None,
            "a different address is unaffected"
        );
    }

    #[test]
    fn the_wait_shrinks_as_the_window_moves_and_then_the_door_opens() {
        let buckets = Buckets::default();
        let now = Instant::now();
        for _ in 0..3 {
            buckets.take(REGISTER_PER_HOUR, "198.51.100.7", now);
        }

        let later = now + Duration::from_secs(20 * 60);
        assert_eq!(
            buckets.take(REGISTER_PER_HOUR, "198.51.100.7", later),
            Some(40 * 60),
            "forty minutes of the hour are left"
        );

        let after = now + HOUR + Duration::from_secs(1);
        assert_eq!(buckets.take(REGISTER_PER_HOUR, "198.51.100.7", after), None);
    }

    #[test]
    fn the_hourly_and_the_daily_allowance_count_apart() {
        let buckets = Buckets::default();
        let mut now = Instant::now();
        let mut allowed = 0;

        for _ in 0..4 {
            for _ in 0..3 {
                let hourly = buckets.take(REGISTER_PER_HOUR, "198.51.100.7", now);
                let daily = buckets.take(REGISTER_PER_DAY, "198.51.100.7", now);
                if hourly.is_none() && daily.is_none() {
                    allowed += 1;
                }
            }
            now += HOUR + Duration::from_secs(1);
        }

        assert_eq!(allowed, 10, "ten a day, whatever the hour says");
    }

    #[test]
    fn one_confirmation_mail_per_address_per_five_minutes() {
        let buckets = Buckets::default();
        let now = Instant::now();

        assert_eq!(buckets.take(RESEND_PER_ADDRESS, "max@example.test", now), None);
        assert_eq!(
            buckets.take(RESEND_PER_ADDRESS, "max@example.test", now + Duration::from_secs(299)),
            Some(1),
            "one second left, rounded up"
        );
        assert_eq!(
            buckets.take(RESEND_PER_ADDRESS, "max@example.test", now + Duration::from_secs(301)),
            None
        );
    }

    #[test]
    fn two_limits_on_one_key_do_not_count_each_other() {
        let buckets = Buckets::default();
        let now = Instant::now();

        buckets.take(RESEND_PER_ADDRESS, "max@example.test", now);
        assert_eq!(buckets.take(RESET_ATTEMPTS, "max@example.test", now), None);
        assert_eq!(buckets.size(), 2, "one record each");
    }

    #[test]
    fn asking_without_counting_leaves_the_allowance_alone() {
        let buckets = Buckets::default();
        let now = Instant::now();

        for _ in 0..20 {
            assert_eq!(buckets.check(RESET_ATTEMPTS, "max@example.test", now), None);
        }

        for _ in 0..10 {
            buckets.note(RESET_ATTEMPTS, "max@example.test", now);
        }
        assert!(
            buckets.check(RESET_ATTEMPTS, "max@example.test", now).is_some(),
            "ten failures shut it"
        );
    }

    #[test]
    fn a_key_nobody_could_have_typed_is_cut_before_it_is_remembered() {
        let buckets = Buckets::default();
        buckets.take(RESEND_PER_ADDRESS, &"a".repeat(1_000_000), Instant::now());

        let records = buckets.records.lock().unwrap();
        let (_, kept) = records.keys().next().unwrap();
        assert_eq!(kept.chars().count(), KEY_LENGTH);
    }

    #[test]
    fn yesterdays_records_do_not_pile_up() {
        let buckets = Buckets::default();
        let now = Instant::now();
        for last in 0..50u8 {
            buckets.take(REGISTER_PER_HOUR, &format!("198.51.100.{last}"), now);
        }
        assert_eq!(buckets.size(), 50);

        buckets.take(REGISTER_PER_HOUR, "203.0.113.9", now + DAY + Duration::from_secs(1));
        assert_eq!(buckets.size(), 1, "a day later only the new one is left");
    }
}
