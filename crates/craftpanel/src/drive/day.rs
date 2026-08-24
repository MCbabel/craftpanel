use std::sync::atomic::{AtomicU64, Ordering};

use crate::model::Timestamp;

pub const CEILING: u64 = 750 * 1000 * 1000 * 1000;

pub fn day_of(now: Timestamp) -> String {
    now.as_datetime().date().to_string()
}

#[derive(Debug)]
pub struct Tally {
    ceiling: u64,
    before: u64,
    added: AtomicU64,
}

impl Tally {
    pub fn of(before: u64) -> Self {
        Self { ceiling: CEILING, before, added: AtomicU64::new(0) }
    }

    #[cfg(test)]
    pub fn up_to(ceiling: u64, before: u64) -> Self {
        Self { ceiling, before, added: AtomicU64::new(0) }
    }

    pub fn took(&self, bytes: u64) {
        self.added.fetch_add(bytes, Ordering::Relaxed);
    }

    pub fn added(&self) -> u64 {
        self.added.load(Ordering::Relaxed)
    }

    pub fn today(&self) -> u64 {
        self.before.saturating_add(self.added())
    }

    pub fn ceiling(&self) -> u64 {
        self.ceiling
    }

    pub fn room(&self) -> u64 {
        self.ceiling.saturating_sub(self.today())
    }

    pub fn full(&self) -> bool {
        self.room() == 0
    }

    pub fn reached(&self) -> super::http::DriveError {
        super::http::DriveError::DayFull(format!(
            "{} bytes have gone up from this account today and Google takes 750 GB a day, so \
             the rest waits. Google does not write down when the day turns over for an account; \
             this panel counts from midnight UTC, and the run is worth starting again after that",
            self.today()
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_ceiling_reads_googles_750_gb_as_the_smaller_of_the_two_prefixes() {
        assert_eq!(CEILING, 750_000_000_000);
        assert!(
            CEILING < 750 * 1024 * 1024 * 1024,
            "the binary reading would let this panel past Google's own limit"
        );
    }

    #[test]
    fn a_day_is_a_day_in_utc_and_reads_as_one() {
        let noon: Timestamp = "2026-08-15T12:00:00Z".parse().expect("a moment");
        let midnight: Timestamp = "2026-08-15T23:59:59Z".parse().expect("a moment");
        let after: Timestamp = "2026-08-16T00:00:01Z".parse().expect("a moment");

        assert_eq!(day_of(noon), "2026-08-15");
        assert_eq!(day_of(midnight), day_of(noon), "one day, one row");
        assert_ne!(day_of(after), day_of(noon), "and the next day starts over");
    }

    #[test]
    fn what_is_already_spent_counts_towards_the_ceiling() {
        let tally = Tally::up_to(100, 60);
        assert_eq!(tally.room(), 40);
        assert!(!tally.full());

        tally.took(30);
        assert_eq!(tally.added(), 30);
        assert_eq!(tally.today(), 90);
        assert_eq!(tally.room(), 10);
        assert!(!tally.full());

        tally.took(50);
        assert_eq!(tally.room(), 0, "past the ceiling is not a negative amount of room");
        assert!(tally.full());
        assert_eq!(tally.reached().operation_code(), "drive_day_full");
        assert!(
            !tally.reached().is_worth_repeating(),
            "a day's ceiling is not waited out inside one run"
        );
    }

    #[test]
    fn an_account_that_has_sent_nothing_has_the_whole_day_in_front_of_it() {
        let tally = Tally::of(0);
        assert_eq!(tally.room(), CEILING);
        assert_eq!(tally.ceiling(), CEILING);
        assert!(!tally.full());
    }
}
