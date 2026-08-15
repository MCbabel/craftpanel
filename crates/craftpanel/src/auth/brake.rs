use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use super::error::{Failure, Result};

const LIMIT: u32 = 10;
const WINDOW: Duration = Duration::from_secs(15 * 60);
const KEY_LENGTH: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum Key {
    Account(String),
    Address(IpAddr),
}

#[derive(Debug, Clone, Copy)]
struct Record {
    failures: u32,
    since: Instant,
    blocked_until: Option<Instant>,
}

#[derive(Default)]
pub struct Brake {
    records: Mutex<HashMap<Key, Record>>,
}

pub fn shared() -> &'static Brake {
    static SHARED: OnceLock<Brake> = OnceLock::new();
    SHARED.get_or_init(Brake::default)
}

impl Brake {
    pub fn check(&self, username: &str, address: Option<IpAddr>, now: Instant) -> Result<()> {
        let records = self.records.lock().expect("the login brake outlives its panics");
        for key in keys(username, address) {
            if let Some(record) = records.get(&key) {
                if record.blocked_until.is_some_and(|until| until > now) {
                    return Err(Failure::new(
                        axum::http::StatusCode::TOO_MANY_REQUESTS,
                        "too_many_attempts",
                        "too many failed sign-ins; try again in a few minutes",
                    ));
                }
            }
        }
        Ok(())
    }

    pub fn note_failure(&self, username: &str, address: Option<IpAddr>, now: Instant) {
        let mut records = self.records.lock().expect("the login brake outlives its panics");
        records.retain(|_, record| alive(record, now));

        for key in keys(username, address) {
            let record = records.entry(key).or_insert(Record {
                failures: 0,
                since: now,
                blocked_until: None,
            });
            if now.duration_since(record.since) > WINDOW {
                *record = Record { failures: 0, since: now, blocked_until: None };
            }
            record.failures += 1;
            if record.failures >= LIMIT {
                record.blocked_until = Some(now + WINDOW);
            }
        }
    }

    pub fn forget(&self, username: &str, address: Option<IpAddr>) {
        let mut records = self.records.lock().expect("the login brake outlives its panics");
        for key in keys(username, address) {
            records.remove(&key);
        }
    }
}

fn keys(username: &str, address: Option<IpAddr>) -> Vec<Key> {
    let account = username.to_lowercase().chars().take(KEY_LENGTH).collect();
    let mut keys = vec![Key::Account(account)];
    keys.extend(address.map(Key::Address));
    keys
}

fn alive(record: &Record, now: Instant) -> bool {
    record.blocked_until.is_some_and(|until| until > now)
        || now.duration_since(record.since) <= WINDOW
}

#[cfg(test)]
mod tests {
    use super::*;

    fn address() -> Option<IpAddr> {
        Some("198.51.100.7".parse().unwrap())
    }

    #[test]
    fn nine_wrong_tries_are_free_and_the_tenth_shuts_the_door() {
        let brake = Brake::default();
        let now = Instant::now();

        for _ in 0..9 {
            brake.note_failure("max", None, now);
            assert!(brake.check("max", None, now).is_ok());
        }
        brake.note_failure("max", None, now);

        let refusal = brake.check("max", None, now).unwrap_err();
        assert_eq!(refusal.code(), "too_many_attempts");
        assert_eq!(refusal.status(), axum::http::StatusCode::TOO_MANY_REQUESTS);
    }

    #[test]
    fn the_door_opens_again_after_fifteen_minutes() {
        let brake = Brake::default();
        let now = Instant::now();
        for _ in 0..LIMIT {
            brake.note_failure("max", None, now);
        }

        assert!(brake.check("max", None, now + Duration::from_secs(14 * 60)).is_err());
        assert!(brake.check("max", None, now + Duration::from_secs(15 * 60 + 1)).is_ok());
    }

    #[test]
    fn the_count_starts_over_when_the_window_passes_without_reaching_ten() {
        let brake = Brake::default();
        let now = Instant::now();
        for _ in 0..9 {
            brake.note_failure("max", None, now);
        }

        let later = now + WINDOW + Duration::from_secs(1);
        for _ in 0..9 {
            brake.note_failure("max", None, later);
        }
        assert!(brake.check("max", None, later).is_ok(), "eighteen tries, never ten in a row");
    }

    #[test]
    fn one_address_cannot_walk_through_the_name_list() {
        let brake = Brake::default();
        let now = Instant::now();
        for name in ["a1", "b2", "c3", "d4", "e5", "f6", "g7", "h8", "i9", "j0"] {
            brake.note_failure(name, address(), now);
        }

        assert_eq!(
            brake.check("k1", address(), now).unwrap_err().code(),
            "too_many_attempts",
            "ten names, one machine"
        );
        assert!(brake.check("k1", None, now).is_ok(), "a different machine is unaffected");
    }

    #[test]
    fn one_name_cannot_be_worked_on_from_ten_machines() {
        let brake = Brake::default();
        let now = Instant::now();
        for last in 0..10u8 {
            let from: IpAddr = format!("198.51.100.{last}").parse().unwrap();
            brake.note_failure("max", Some(from), now);
        }

        let fresh: IpAddr = "203.0.113.9".parse().unwrap();
        assert_eq!(brake.check("max", Some(fresh), now).unwrap_err().code(), "too_many_attempts");
        assert!(brake.check("anna", Some(fresh), now).is_ok());
    }

    #[test]
    fn a_password_that_works_clears_the_count() {
        let brake = Brake::default();
        let now = Instant::now();
        for _ in 0..9 {
            brake.note_failure("max", address(), now);
        }
        brake.forget("max", address());

        for _ in 0..9 {
            brake.note_failure("max", address(), now);
        }
        assert!(brake.check("max", address(), now).is_ok());
    }

    #[test]
    fn the_name_is_counted_without_regard_to_case() {
        let brake = Brake::default();
        let now = Instant::now();
        for _ in 0..LIMIT {
            brake.note_failure("MAX", None, now);
        }
        assert!(brake.check("max", None, now).is_err());
    }

    #[test]
    fn a_name_nobody_could_have_is_cut_before_it_is_remembered() {
        let brake = Brake::default();
        let now = Instant::now();
        brake.note_failure(&"a".repeat(1_000_000), None, now);

        let records = brake.records.lock().unwrap();
        let Key::Account(kept) = records.keys().next().unwrap() else { panic!("no account key") };
        assert_eq!(kept.chars().count(), KEY_LENGTH, "a megabyte of it is not worth keeping");
    }

    #[test]
    fn spent_records_do_not_pile_up() {
        let brake = Brake::default();
        let now = Instant::now();
        for last in 0..50u8 {
            let from: IpAddr = format!("198.51.100.{last}").parse().unwrap();
            brake.note_failure("max", Some(from), now);
        }
        assert_eq!(brake.records.lock().unwrap().len(), 51, "one per name, one per address");

        brake.note_failure("anna", None, now + WINDOW + Duration::from_secs(1));
        let left = brake.records.lock().unwrap().len();
        assert_eq!(left, 1, "everything from before the window is gone, {left} left");
    }
}
