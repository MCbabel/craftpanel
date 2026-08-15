use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::model::{Id, Timestamp};

const WINDOW: Duration = Duration::from_secs(5);
const MIB: u64 = 1024 * 1024;
const DEFAULT_ROOT: &str = "/sys/fs/cgroup/system.slice/craftpanel-games";

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sample {
    pub memory_bytes: u64,
    pub used_cores: f64,
    pub pids: u32,
    pub measured_at: Timestamp,
}

#[derive(Debug, Clone, Copy)]
struct Reading {
    at: Instant,
    cpu_usec: u64,
    sample: Sample,
}

pub struct Cgroups {
    root: PathBuf,
    window: Duration,
    seen: Mutex<HashMap<Id, Reading>>,
}

pub fn shared() -> &'static Cgroups {
    static SHARED: OnceLock<Cgroups> = OnceLock::new();
    SHARED.get_or_init(|| {
        let root = std::env::var_os("CRAFTPANEL_CGROUP_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(DEFAULT_ROOT));
        Cgroups::at(root)
    })
}

impl Cgroups {
    pub fn at(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into(), window: WINDOW, seen: Mutex::new(HashMap::new()) }
    }

    pub fn sample(&self, user: Id) -> Sample {
        let now = Instant::now();
        let mut seen = self.seen.lock().expect("the usage cache outlives its panics");

        if let Some(previous) = seen.get(&user) {
            if now.duration_since(previous.at) < self.window {
                return previous.sample;
            }
        }

        let dir = self.root.join(format!("user-{user}"));
        let cpu_usec = cpu_usec(&dir);
        let used_cores = match seen.get(&user) {
            Some(previous) => cores_between(previous.cpu_usec, cpu_usec, now - previous.at),
            None => 0.0,
        };

        let sample = Sample {
            memory_bytes: number(&dir.join("memory.current")).unwrap_or(0),
            used_cores,
            pids: number(&dir.join("pids.current")).unwrap_or(0) as u32,
            measured_at: Timestamp::now(),
        };
        seen.insert(user, Reading { at: now, cpu_usec, sample });
        sample
    }

    pub fn total(&self, users: impl IntoIterator<Item = Id>) -> Sample {
        let mut total = Sample {
            memory_bytes: 0,
            used_cores: 0.0,
            pids: 0,
            measured_at: Timestamp::now(),
        };
        for user in users {
            let sample = self.sample(user);
            total.memory_bytes += sample.memory_bytes;
            total.used_cores += sample.used_cores;
            total.pids += sample.pids;
        }
        total
    }
}

fn cores_between(before: u64, after: u64, elapsed: Duration) -> f64 {
    let spent = after.saturating_sub(before) as f64;
    let window = elapsed.as_micros() as f64;
    if window <= 0.0 {
        return 0.0;
    }
    (spent / window).max(0.0)
}

fn cpu_usec(dir: &Path) -> u64 {
    let Ok(text) = std::fs::read_to_string(dir.join("cpu.stat")) else {
        return 0;
    };
    text.lines()
        .find_map(|line| line.strip_prefix("usage_usec "))
        .and_then(|value| value.trim().parse().ok())
        .unwrap_or(0)
}

fn number(path: &Path) -> Option<u64> {
    std::fs::read_to_string(path).ok()?.trim().parse().ok()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Host {
    pub cpu_cores: u32,
    pub memory_total_bytes: u64,
}

pub fn host() -> Host {
    static HOST: OnceLock<Host> = OnceLock::new();
    *HOST.get_or_init(Host::measure)
}

impl Host {
    pub fn measure() -> Self {
        Self {
            cpu_cores: std::thread::available_parallelism().map_or(1, |cores| cores.get() as u32),
            memory_total_bytes: total_memory_bytes().unwrap_or(0),
        }
    }

    pub fn memory_total_mib(self) -> u32 {
        (self.memory_total_bytes / MIB) as u32
    }

    pub fn reserved_memory_mib(self) -> u32 {
        (self.memory_total_mib() / 4).min(2048)
    }

    pub fn assignable_memory_mib(self) -> u32 {
        self.memory_total_mib().saturating_sub(self.reserved_memory_mib())
    }
}

fn total_memory_bytes() -> Option<u64> {
    let meminfo = std::fs::read_to_string("/proc/meminfo").ok()?;
    let kib: u64 = meminfo
        .lines()
        .find_map(|line| line.strip_prefix("MemTotal:"))?
        .split_whitespace()
        .next()?
        .parse()
        .ok()?;
    Some(kib * 1024)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a_cgroup(root: &Path, user: Id, memory: u64, cpu_usec: u64, pids: u32) {
        let dir = root.join(format!("user-{user}"));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("memory.current"), memory.to_string()).unwrap();
        std::fs::write(dir.join("pids.current"), pids.to_string()).unwrap();
        std::fs::write(
            dir.join("cpu.stat"),
            format!("usage_usec {cpu_usec}\nuser_usec 1\nsystem_usec 2\n"),
        )
        .unwrap();
    }

    struct Scratch(PathBuf);

    impl Scratch {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!("craftpanel-cgroup-{}", Id::new()));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn the_three_files_are_read_as_they_stand() {
        let scratch = Scratch::new();
        let user = Id::new();
        a_cgroup(&scratch.0, user, 3 * 1024 * MIB, 500_000, 137);

        let sample = Cgroups::at(&scratch.0).sample(user);
        assert_eq!(sample.memory_bytes, 3221225472);
        assert_eq!(sample.pids, 137);
        assert_eq!(sample.used_cores, 0.0, "one visit is not a rate");
    }

    #[test]
    fn a_missing_cgroup_reads_as_nothing_rather_than_failing() {
        let scratch = Scratch::new();
        let sample = Cgroups::at(&scratch.0).sample(Id::new());
        assert_eq!(sample.memory_bytes, 0);
        assert_eq!(sample.pids, 0);
        assert_eq!(sample.used_cores, 0.0);
    }

    #[test]
    fn the_second_visit_within_five_seconds_returns_the_first_answer() {
        let scratch = Scratch::new();
        let user = Id::new();
        a_cgroup(&scratch.0, user, 100, 0, 1);

        let cgroups = Cgroups::at(&scratch.0);
        let first = cgroups.sample(user);

        a_cgroup(&scratch.0, user, 999_999, 0, 42);
        assert_eq!(cgroups.sample(user), first, "the window has not passed");
    }

    #[test]
    fn cores_are_the_time_spent_over_the_time_that_passed() {
        assert_eq!(cores_between(0, 1_000_000, Duration::from_secs(1)), 1.0);
        assert_eq!(cores_between(500_000, 2_500_000, Duration::from_secs(1)), 2.0);
        assert_eq!(cores_between(0, 250_000, Duration::from_secs(1)), 0.25);
        assert_eq!(cores_between(0, 1_000_000, Duration::ZERO), 0.0, "no window, no rate");
        assert_eq!(cores_between(9, 0, Duration::from_secs(1)), 0.0, "a reset counter is not negative");
    }

    #[test]
    fn a_total_adds_up_what_the_accounts_use() {
        let scratch = Scratch::new();
        let (anna, max) = (Id::new(), Id::new());
        a_cgroup(&scratch.0, anna, 1024, 0, 3);
        a_cgroup(&scratch.0, max, 2048, 0, 4);

        let total = Cgroups::at(&scratch.0).total([anna, max]);
        assert_eq!(total.memory_bytes, 3072);
        assert_eq!(total.pids, 7);
    }

    #[test]
    fn the_reserve_is_a_quarter_of_a_small_box_and_two_gibibytes_of_a_large_one() {
        let small = Host { cpu_cores: 2, memory_total_bytes: 4 * 1024 * MIB };
        assert_eq!(small.reserved_memory_mib(), 1024);
        assert_eq!(small.assignable_memory_mib(), 3072);

        let large = Host { cpu_cores: 16, memory_total_bytes: 64 * 1024 * MIB };
        assert_eq!(large.reserved_memory_mib(), 2048);
        assert_eq!(large.assignable_memory_mib(), 65536 - 2048);
    }

    #[test]
    fn the_machine_answers_with_something_believable() {
        let host = Host::measure();
        assert!(host.cpu_cores >= 1);
        assert!(host.memory_total_bytes > 0, "/proc/meminfo should be readable here");
        assert!(host.assignable_memory_mib() < host.memory_total_mib());
    }

    #[test]
    fn the_shared_machine_is_the_measured_one() {
        assert_eq!(host(), Host::measure());
        assert_eq!(host(), host(), "read once and kept");
    }
}
