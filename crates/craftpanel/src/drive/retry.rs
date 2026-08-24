use std::future::Future;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use crate::backups::archive::Progress;

use super::http::{DriveError, Result};

const SLICE: Duration = Duration::from_millis(200);

const LONGEST_HINT: Duration = Duration::from_secs(10 * 60);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Backoff {
    pub tries: u32,
    pub first: Duration,
    pub ceiling: Duration,
    pub budget: Duration,
}

impl Backoff {
    pub const ONCE: Self = Self {
        tries: 1,
        first: Duration::ZERO,
        ceiling: Duration::ZERO,
        budget: Duration::ZERO,
    };

    pub const BRIEF: Self = Self {
        tries: 3,
        first: Duration::from_secs(1),
        ceiling: Duration::from_secs(4),
        budget: Duration::from_secs(10),
    };

    pub const PATIENT: Self = Self {
        tries: 7,
        first: Duration::from_secs(1),
        ceiling: Duration::from_secs(64),
        budget: Duration::from_secs(5 * 60),
    };

    pub const HURRIED: Self = Self {
        tries: 4,
        first: Duration::from_millis(10),
        ceiling: Duration::from_millis(50),
        budget: Duration::from_secs(3),
    };
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pace {
    pub brief: Backoff,
    pub patient: Backoff,
    pub run: Duration,
}

impl Pace {
    pub const REAL: Self =
        Self { brief: Backoff::BRIEF, patient: Backoff::PATIENT, run: Duration::from_secs(15 * 60) };

    pub const HURRIED: Self =
        Self { brief: Backoff::HURRIED, patient: Backoff::HURRIED, run: Duration::from_secs(2) };
}

#[derive(Debug)]
pub struct Spent {
    budget: Duration,
    waited: AtomicU64,
}

impl Spent {
    pub fn of(budget: Duration) -> Self {
        Self { budget, waited: AtomicU64::new(0) }
    }

    pub fn budget(&self) -> Duration {
        self.budget
    }

    pub fn waited(&self) -> Duration {
        Duration::from_millis(self.waited.load(Ordering::Relaxed))
    }

    fn charge(&self, pause: Duration) -> Duration {
        let millis = pause.as_millis() as u64;
        Duration::from_millis(self.waited.fetch_add(millis, Ordering::Relaxed) + millis)
    }
}

#[derive(Debug)]
pub struct Setback {
    pub error: DriveError,
    pub after: Option<Duration>,
}

impl Setback {
    pub fn plain(error: DriveError) -> Self {
        Self { error, after: None }
    }
}

#[derive(Clone, Copy)]
pub struct Waiting<'a> {
    plan: Backoff,
    watch: Option<&'a Progress>,
    run: Option<&'a Spent>,
    doing: &'a str,
}

impl<'a> Waiting<'a> {
    pub fn on(plan: Backoff, doing: &'a str) -> Self {
        Self { plan, watch: None, run: None, doing }
    }

    pub fn once() -> Waiting<'static> {
        Waiting { plan: Backoff::ONCE, watch: None, run: None, doing: "the call" }
    }

    pub fn watched_by(self, watch: &'a Progress) -> Self {
        Self { watch: Some(watch), ..self }
    }

    pub fn within(self, run: &'a Spent) -> Self {
        Self { run: Some(run), ..self }
    }

    pub fn doing(self, doing: &'a str) -> Self {
        Self { doing, ..self }
    }

    pub fn is_cancelled(&self) -> bool {
        self.watch.is_some_and(Progress::is_cancelled)
    }

    pub async fn keep_trying<T, F, Fut>(&self, call: F) -> Result<T>
    where
        F: Fn() -> Fut,
        Fut: Future<Output = std::result::Result<T, Setback>>,
    {
        let mut tried = 0u32;
        let mut spent = Duration::ZERO;

        loop {
            if self.is_cancelled() {
                self.moving_again();
                return Err(DriveError::Cancelled);
            }

            let setback = match call().await {
                Ok(value) => {
                    self.moving_again();
                    return Ok(value);
                }
                Err(setback) => setback,
            };

            tried += 1;
            if tried >= self.plan.tries || !setback.error.is_worth_repeating() {
                self.moving_again();
                return Err(self.gave_up(setback.error, tried, spent));
            }

            let pause = pause_after(tried, setback.after, self.plan);
            if spent + pause > self.plan.budget {
                self.moving_again();
                return Err(self.gave_up(setback.error, tried, spent + pause));
            }
            if let Some(run) = self.run {
                let over_all = run.charge(pause);
                if over_all > run.budget {
                    self.moving_again();
                    return Err(self.throttled(over_all, run.budget, setback.after));
                }
            }
            spent += pause;

            tracing::warn!(
                doing = self.doing,
                try_number = tried,
                pause_ms = pause.as_millis() as u64,
                "Google turned us away, waiting before the next try: {}",
                setback.error
            );
            self.hold(pause, tried);

            if !linger(pause, self.watch).await {
                self.moving_again();
                return Err(DriveError::Cancelled);
            }
        }
    }

    pub async fn breathe(&self, tried: u32) -> bool {
        let pause = pause_after(tried.max(1), None, self.plan);
        self.hold(pause, tried);
        let carried = linger(pause, self.watch).await;
        self.moving_again();
        carried
    }

    fn throttled(&self, waited: Duration, budget: Duration, said: Option<Duration>) -> DriveError {
        tracing::warn!(
            doing = self.doing,
            waited_ms = waited.as_millis() as u64,
            "Google has turned this run away for longer than one run may wait; it stops here"
        );
        let again = match said {
            Some(after) => format!("Google last asked for {} before the next call", spoken(after)),
            None => "Google named no time of its own, and its documentation names none either; \
                     an hour is the usual pause before another run is worth starting"
                .to_owned(),
        };
        DriveError::Throttled(format!(
            "Google turned this account away again and again. One run may spend {} waiting on \
             it, and this one spent {}, so it stopped rather than carry on for hours. {again}.",
            spoken(budget),
            spoken(waited)
        ))
    }

    fn gave_up(&self, error: DriveError, tried: u32, spent: Duration) -> DriveError {
        if tried > 1 {
            tracing::warn!(
                doing = self.doing,
                tries = tried,
                waited_ms = spent.as_millis() as u64,
                "giving up on Google: {error}"
            );
        }
        error
    }

    fn hold(&self, pause: Duration, tried: u32) {
        let Some(watch) = self.watch else { return };
        watch.waiting(format!(
            "{}: Google is turning us away, so the next try ({} of {}) is in {}",
            self.doing,
            tried + 1,
            self.plan.tries,
            spoken(pause)
        ));
    }

    fn moving_again(&self) {
        if let Some(watch) = self.watch {
            watch.moving_again();
        }
    }
}

fn spoken(pause: Duration) -> String {
    match pause.as_secs() {
        0 => "a moment".to_owned(),
        1 => "1 second".to_owned(),
        seconds if seconds < 120 => format!("{seconds} seconds"),
        seconds => format!("{} minutes", seconds / 60),
    }
}

fn pause_after(tried: u32, said: Option<Duration>, plan: Backoff) -> Duration {
    let grown = plan.first.saturating_mul(1u32 << (tried - 1).min(16));
    let ours = (grown + jitter(plan.first)).min(plan.ceiling);
    ours.max(said.unwrap_or(Duration::ZERO).min(LONGEST_HINT))
}

fn jitter(first: Duration) -> Duration {
    let span = first.as_millis().min(1_000) as u64;
    if span == 0 {
        return Duration::ZERO;
    }
    Duration::from_millis(u64::from(u16::from_le_bytes(rand::random())) % span)
}

async fn linger(pause: Duration, watch: Option<&Progress>) -> bool {
    let mut left = pause;
    while !left.is_zero() {
        if watch.is_some_and(Progress::is_cancelled) {
            return false;
        }
        let slice = left.min(SLICE);
        tokio::time::sleep(slice).await;
        left -= slice;
    }
    !watch.is_some_and(Progress::is_cancelled)
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;
    use std::time::Instant;

    fn setback(status: u16) -> Setback {
        Setback::plain(DriveError::Refused {
            status,
            reason: String::new(),
            detail: String::new(),
        })
    }

    #[test]
    fn the_wait_doubles_the_way_google_writes_it_down_and_then_stops() {
        let plan = Backoff::PATIENT;
        for (tried, floor) in [(1u32, 1u64), (2, 2), (3, 4), (4, 8), (5, 16), (6, 32)] {
            let pause = pause_after(tried, None, plan);
            assert!(
                pause >= Duration::from_secs(floor),
                "try {tried} waited {pause:?}, less than the 2^n of the algorithm"
            );
            assert!(
                pause < Duration::from_secs(floor) + Duration::from_millis(1_000),
                "try {tried} waited {pause:?}: the random part is at most a second"
            );
        }
        assert_eq!(pause_after(20, None, plan), plan.ceiling, "it climbs to the ceiling and stops");
    }

    #[test]
    fn two_accounts_do_not_walk_into_the_wall_in_step() {
        let seen: std::collections::HashSet<u128> = (0..64)
            .map(|_| pause_after(3, None, Backoff::PATIENT).as_millis())
            .collect();
        assert!(seen.len() > 8, "sixty-four waits landed on {} lengths", seen.len());
    }

    #[test]
    fn a_retry_after_of_googles_is_an_order_and_not_a_suggestion() {
        let plan = Backoff::PATIENT;
        let said = Duration::from_secs(30);
        assert!(
            pause_after(1, Some(said), plan) >= said,
            "a header that says thirty seconds must not be undercut"
        );
        assert!(
            pause_after(1, Some(Duration::from_millis(1)), plan) >= plan.first,
            "a header of one millisecond must not shorten our own backoff"
        );
        assert_eq!(
            pause_after(1, Some(Duration::from_secs(60 * 60 * 24)), plan),
            LONGEST_HINT,
            "a day is not a wait, it is a hang"
        );
    }

    #[tokio::test]
    async fn what_google_calls_hopeless_is_asked_exactly_once() {
        let asked = Arc::new(AtomicU32::new(0));
        let count = Arc::clone(&asked);
        let over = Waiting::on(Backoff::HURRIED, "the upload");

        let err = over
            .keep_trying(|| {
                let count = Arc::clone(&count);
                async move {
                    count.fetch_add(1, Ordering::Relaxed);
                    Err::<(), _>(Setback::plain(DriveError::QuotaFull("full".to_owned())))
                }
            })
            .await
            .expect_err("a full Drive is not a bad moment");

        assert_eq!(err.operation_code(), "drive_quota_exceeded");
        assert_eq!(asked.load(Ordering::Relaxed), 1, "a full Drive was asked more than once");
    }

    #[tokio::test]
    async fn a_bad_moment_is_ridden_out_and_a_long_one_is_given_up_on() {
        let asked = Arc::new(AtomicU32::new(0));
        let count = Arc::clone(&asked);
        let over = Waiting::on(Backoff::HURRIED, "the upload");

        let value = over
            .keep_trying(|| {
                let count = Arc::clone(&count);
                async move {
                    if count.fetch_add(1, Ordering::Relaxed) < 2 {
                        return Err(setback(503));
                    }
                    Ok(7u8)
                }
            })
            .await
            .expect("the third try got through");
        assert_eq!(value, 7);
        assert_eq!(asked.load(Ordering::Relaxed), 3);

        let asked = Arc::new(AtomicU32::new(0));
        let count = Arc::clone(&asked);
        over.keep_trying(|| {
            let count = Arc::clone(&count);
            async move {
                count.fetch_add(1, Ordering::Relaxed);
                Err::<(), _>(setback(503))
            }
        })
        .await
        .expect_err("a Google that is down all evening is a failed run");
        assert_eq!(
            asked.load(Ordering::Relaxed),
            Backoff::HURRIED.tries,
            "the ceiling on the number of tries is not held to"
        );
    }

    #[tokio::test]
    async fn the_whole_wait_is_capped_and_not_only_each_single_one() {
        let plan =
            Backoff { tries: 100, budget: Duration::from_millis(300), ..Backoff::HURRIED };
        let asked = Arc::new(AtomicU32::new(0));
        let count = Arc::clone(&asked);

        let started = Instant::now();
        Waiting::on(plan, "the upload")
            .keep_trying(|| {
                let count = Arc::clone(&count);
                async move {
                    count.fetch_add(1, Ordering::Relaxed);
                    Err::<(), _>(setback(503))
                }
            })
            .await
            .expect_err("a hundred tries are not a plan");

        assert!(
            started.elapsed() < plan.budget * 3,
            "the run was held for {:?} on a budget of {:?}",
            started.elapsed(),
            plan.budget
        );
        assert!(asked.load(Ordering::Relaxed) < 100, "the budget did not stop the tries");
    }

    #[tokio::test]
    async fn the_budget_of_a_run_outlives_the_budget_of_a_single_call() {
        let run = Spent::of(Duration::from_millis(200));
        let plan = Backoff {
            tries: 3,
            first: Duration::from_millis(50),
            ceiling: Duration::from_millis(50),
            budget: Duration::from_secs(60),
        };

        let mut endings = Vec::new();
        for _ in 0..3 {
            endings.push(
                Waiting::on(plan, "the upload")
                    .within(&run)
                    .keep_trying(|| async { Err::<(), _>(setback(503)) })
                    .await
                    .expect_err("Google is down all evening"),
            );
        }

        assert!(
            matches!(endings[0], DriveError::Refused { .. }),
            "the first call may spend its own budget: {:?}",
            endings[0]
        );
        assert!(
            matches!(endings[2], DriveError::Throttled(_)),
            "the third call was handed a budget of its own all over again: {:?}",
            endings[2]
        );
        assert!(
            endings[2].to_string().contains("worth starting"),
            "the sentence does not say when to come back: {}",
            endings[2]
        );
        assert!(!endings[2].is_worth_repeating());
        assert!(run.waited() > run.budget(), "{:?} of {:?}", run.waited(), run.budget());
    }

    #[tokio::test]
    async fn a_run_that_never_waits_never_spends_its_budget() {
        let run = Spent::of(Duration::from_millis(1));
        let value = Waiting::on(Backoff::HURRIED, "the upload")
            .within(&run)
            .keep_trying(|| async { Ok::<u8, Setback>(7) })
            .await
            .expect("Google answered at once");

        assert_eq!(value, 7);
        assert_eq!(run.waited(), Duration::ZERO, "a call that got through was charged for waiting");
    }

    #[tokio::test]
    async fn a_person_who_calls_it_off_is_not_kept_waiting_for_the_backoff() {
        let progress = Arc::new(Progress::default());
        let plan = Backoff { first: Duration::from_secs(30), ..Backoff::PATIENT };
        let watched = Arc::clone(&progress);
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            watched.cancel();
        });

        let started = Instant::now();
        let err = Waiting::on(plan, "the upload")
            .watched_by(&progress)
            .keep_trying(|| async { Err::<(), _>(setback(503)) })
            .await
            .expect_err("it was called off");

        assert!(matches!(err, DriveError::Cancelled), "{err:?}");
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "the cancel waited {:?} for a thirty second backoff to run out",
            started.elapsed()
        );
        assert_eq!(progress.holdup(), None, "the run is over and still says it is waiting");
    }

    #[tokio::test]
    async fn a_wait_says_that_it_is_waiting_instead_of_standing_still() {
        let progress = Arc::new(Progress::default());
        let seen: Arc<std::sync::Mutex<Vec<String>>> = Arc::default();

        let watcher = {
            let progress = Arc::clone(&progress);
            let seen = Arc::clone(&seen);
            tokio::spawn(async move {
                loop {
                    if let Some(holdup) = progress.holdup() {
                        seen.lock().expect("the notes").push(holdup);
                    }
                    tokio::time::sleep(Duration::from_millis(2)).await;
                }
            })
        };

        let plan = Backoff { first: Duration::from_millis(80), ..Backoff::HURRIED };
        let _ = Waiting::on(plan, "the upload")
            .watched_by(&progress)
            .doing("the upload")
            .keep_trying(|| async { Err::<(), _>(setback(503)) })
            .await;
        watcher.abort();

        let notes = seen.lock().expect("the notes").clone();
        let first = notes.first().expect("a bar that does not move has to say why");
        assert!(first.starts_with("the upload"), "{first}");
        assert!(first.contains("next try"), "{first}");
        assert!(first.contains("2 of 4"), "a person wants to know how many are left: {first}");
        assert_eq!(progress.holdup(), None, "the note outlived the wait");
    }
}
