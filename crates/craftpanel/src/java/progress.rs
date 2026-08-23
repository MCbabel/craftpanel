use std::sync::atomic::{AtomicU64, AtomicU8, Ordering};

const DOWNLOAD_SHARE: f64 = 0.9;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    Waiting,
    Asking,
    Downloading,
    Unpacking,
    Done,
}

impl Stage {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Waiting => "waiting",
            Self::Asking => "asking",
            Self::Downloading => "downloading",
            Self::Unpacking => "unpacking",
            Self::Done => "done",
        }
    }

    fn mark(self) -> u8 {
        match self {
            Self::Waiting => 0,
            Self::Asking => 1,
            Self::Downloading => 2,
            Self::Unpacking => 3,
            Self::Done => 4,
        }
    }

    fn of(mark: u8) -> Self {
        match mark {
            1 => Self::Asking,
            2 => Self::Downloading,
            3 => Self::Unpacking,
            4 => Self::Done,
            _ => Self::Waiting,
        }
    }
}

#[derive(Debug, Default)]
pub struct Progress {
    stage: AtomicU8,
    total: AtomicU64,
    done: AtomicU64,
}

impl Progress {
    pub fn stage(&self) -> Stage {
        Stage::of(self.stage.load(Ordering::Acquire))
    }

    pub fn total(&self) -> u64 {
        self.total.load(Ordering::Relaxed)
    }

    pub fn done(&self) -> u64 {
        self.done.load(Ordering::Relaxed)
    }

    pub fn share(&self) -> f64 {
        let part = match self.total() {
            0 => 0.0,
            total => (self.done() as f64 / total as f64).clamp(0.0, 1.0),
        };
        match self.stage() {
            Stage::Waiting | Stage::Asking => 0.0,
            Stage::Downloading => part * DOWNLOAD_SHARE,
            Stage::Unpacking => DOWNLOAD_SHARE + part * (1.0 - DOWNLOAD_SHARE),
            Stage::Done => 1.0,
        }
    }

    pub(super) fn asking(&self) {
        self.total.store(0, Ordering::Relaxed);
        self.done.store(0, Ordering::Relaxed);
        self.stage.store(Stage::Asking.mark(), Ordering::Release);
    }

    pub(super) fn downloading(&self, total: u64) {
        self.total.store(total, Ordering::Relaxed);
        self.done.store(0, Ordering::Relaxed);
        self.stage.store(Stage::Downloading.mark(), Ordering::Release);
    }

    pub(super) fn unpacking(&self) {
        self.done.store(0, Ordering::Relaxed);
        self.stage.store(Stage::Unpacking.mark(), Ordering::Release);
    }

    pub(super) fn advanced(&self, bytes: u64) {
        self.done.fetch_add(bytes, Ordering::Relaxed);
    }

    pub(super) fn settled(&self) {
        self.stage.store(Stage::Done.mark(), Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_stage_survives_the_trip_through_the_atomic() {
        let all =
            [Stage::Waiting, Stage::Asking, Stage::Downloading, Stage::Unpacking, Stage::Done];
        for stage in all {
            assert_eq!(Stage::of(stage.mark()), stage);
        }
        assert_eq!(Stage::of(200), Stage::Waiting);
    }

    #[test]
    fn the_share_runs_once_from_nothing_to_one_across_both_halves() {
        let progress = Progress::default();
        assert_eq!(progress.share(), 0.0);

        progress.asking();
        assert_eq!(progress.share(), 0.0);
        assert_eq!(progress.total(), 0);

        progress.downloading(1000);
        assert_eq!(progress.share(), 0.0);
        progress.advanced(500);
        assert!((progress.share() - 0.45).abs() < 1e-9, "{}", progress.share());
        progress.advanced(500);
        assert!((progress.share() - 0.9).abs() < 1e-9, "{}", progress.share());
        assert_eq!(progress.done(), 1000);

        progress.unpacking();
        assert_eq!(progress.done(), 0, "the unpacking reads the same archive again");
        assert!((progress.share() - 0.9).abs() < 1e-9);
        progress.advanced(1000);
        assert!((progress.share() - 1.0).abs() < 1e-9);

        progress.settled();
        assert_eq!(progress.stage(), Stage::Done);
        assert_eq!(progress.share(), 1.0);
        assert_eq!(progress.stage().as_str(), "done");
    }

    #[test]
    fn more_bytes_than_announced_do_not_push_the_share_past_one() {
        let progress = Progress::default();
        progress.downloading(100);
        progress.advanced(400);
        assert!((progress.share() - 0.9).abs() < 1e-9, "{}", progress.share());
    }
}
