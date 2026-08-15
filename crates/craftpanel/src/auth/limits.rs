use craftpanel_proto::ResourceLimits;

use super::error::{Failure, Result};
use crate::model::{CpuMode, PanelRole, UserLimits};

const MIB: u64 = 1024 * 1024;
const EMERGENCY_FACTOR: f64 = 1.25;

pub const MIN_MEMORY_MIB: u32 = 512;
pub const MIN_PIDS: u32 = 64;
pub const MIN_DISK_MIB: u32 = 1024;

pub fn check(limits: &UserLimits) -> Result<()> {
    if limits.memory_mib < MIN_MEMORY_MIB {
        return Err(Failure::invalid_request(format!(
            "memory_mib is at least {MIN_MEMORY_MIB}"
        )));
    }
    if !(limits.cpu_cores > 0.0) || !limits.cpu_cores.is_finite() {
        return Err(Failure::invalid_request("cpu_cores is above zero"));
    }
    if limits.pids_max < MIN_PIDS {
        return Err(Failure::invalid_request(format!("pids_max is at least {MIN_PIDS}")));
    }
    if limits.disk_mib < MIN_DISK_MIB {
        return Err(Failure::invalid_request(format!("disk_mib is at least {MIN_DISK_MIB}")));
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Budget {
    Unlimited,
    Held(UserLimits),
}

impl Budget {
    pub fn of(role: PanelRole, limits: UserLimits) -> Self {
        match role {
            PanelRole::Admin => Self::Unlimited,
            PanelRole::User => Self::Held(limits),
        }
    }

    pub fn is_unlimited(self) -> bool {
        matches!(self, Self::Unlimited)
    }

    pub fn limits(self) -> Option<UserLimits> {
        match self {
            Self::Unlimited => None,
            Self::Held(limits) => Some(limits),
        }
    }

    pub fn memory_mib(self) -> Option<u32> {
        self.limits().map(|limits| limits.memory_mib)
    }

    pub fn cpu_cores(self) -> Option<f64> {
        self.limits().map(|limits| limits.cpu_cores)
    }

    pub fn pids_max(self) -> Option<u32> {
        self.limits().map(|limits| limits.pids_max)
    }

    pub fn disk_mib(self) -> Option<u32> {
        self.limits().map(|limits| limits.disk_mib)
    }

    pub fn disk_limit_bytes(self) -> Option<u64> {
        self.disk_mib().map(|mib| u64::from(mib) * MIB)
    }

    pub fn exceeded_by(self, allocated_mib: u32) -> bool {
        self.memory_mib().is_some_and(|limit| allocated_mib > limit)
    }

    pub fn has_room_for(self, allocated_mib: u32, wanted_mib: u32) -> bool {
        self.memory_mib().is_none_or(|limit| allocated_mib.saturating_add(wanted_mib) <= limit)
    }

    pub fn disk_exceeded_by(self, used_bytes: u64) -> bool {
        self.disk_limit_bytes().is_some_and(|limit| used_bytes > limit)
    }

    pub fn has_disk_room_for(self, used_bytes: u64, wanted_bytes: u64) -> bool {
        self.disk_limit_bytes().is_none_or(|limit| used_bytes.saturating_add(wanted_bytes) <= limit)
    }

    pub fn to_cgroup(self) -> ResourceLimits {
        match self {
            Self::Unlimited => ResourceLimits {
                memory_high_bytes: None,
                memory_max_bytes: None,
                cpu_quota_percent: None,
                pids_max: None,
            },
            Self::Held(limits) => {
                let high = u64::from(limits.memory_mib) * MIB;
                ResourceLimits {
                    memory_high_bytes: Some(high),
                    memory_max_bytes: Some((high as f64 * EMERGENCY_FACTOR) as u64),
                    cpu_quota_percent: match limits.cpu_mode {
                        CpuMode::Cap => Some((limits.cpu_cores * 100.0).round() as u32),
                        CpuMode::Share => None,
                    },
                    pids_max: Some(limits.pids_max),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limits(memory_mib: u32, mode: CpuMode, cores: f64, pids: u32) -> UserLimits {
        UserLimits { memory_mib, cpu_mode: mode, cpu_cores: cores, pids_max: pids, disk_mib: 51200 }
    }

    fn held(memory_mib: u32, mode: CpuMode, cores: f64, pids: u32) -> Budget {
        Budget::of(PanelRole::User, limits(memory_mib, mode, cores, pids))
    }

    #[test]
    fn the_three_bounds_of_12_8_are_refused_by_name() {
        assert!(check(&limits(512, CpuMode::Cap, 1.0, 64)).is_ok());

        let too_small = check(&limits(511, CpuMode::Cap, 1.0, 64)).unwrap_err();
        assert_eq!(too_small.code(), "invalid_request");
        assert!(too_small.to_string().contains("memory_mib"));

        assert!(check(&limits(512, CpuMode::Cap, 0.0, 64)).is_err());
        assert!(check(&limits(512, CpuMode::Cap, -1.0, 64)).is_err());
        assert!(check(&limits(512, CpuMode::Cap, f64::NAN, 64)).is_err());
        assert!(check(&limits(512, CpuMode::Cap, f64::INFINITY, 64)).is_err());
        assert!(check(&limits(512, CpuMode::Cap, 1.0, 63)).is_err());

        let thin = check(&UserLimits { disk_mib: 1023, ..limits(512, CpuMode::Cap, 1.0, 64) })
            .unwrap_err();
        assert_eq!(thin.code(), "invalid_request");
        assert!(thin.to_string().contains("disk_mib"), "{thin}");
        assert!(check(&UserLimits { disk_mib: 1024, ..limits(512, CpuMode::Cap, 1.0, 64) }).is_ok());
    }

    #[test]
    fn the_emergency_brake_sits_a_quarter_above_the_promise() {
        let applied = held(4096, CpuMode::Cap, 2.0, 512).to_cgroup();
        assert_eq!(applied.memory_high_bytes, Some(4096 * MIB));
        assert_eq!(applied.memory_max_bytes, Some(5120 * MIB));
        assert!(applied.memory_max_bytes > applied.memory_high_bytes, "never below the promise");
    }

    #[test]
    fn a_cap_becomes_a_quota_and_a_share_becomes_none() {
        assert_eq!(held(4096, CpuMode::Cap, 2.5, 512).to_cgroup().cpu_quota_percent, Some(250));
        assert_eq!(held(4096, CpuMode::Cap, 0.5, 512).to_cgroup().cpu_quota_percent, Some(50));
        assert_eq!(
            held(4096, CpuMode::Share, 2.5, 512).to_cgroup().cpu_quota_percent,
            None,
            "a share sets no ceiling"
        );
    }

    #[test]
    fn the_process_count_travels_unchanged() {
        assert_eq!(held(4096, CpuMode::Cap, 2.0, 777).to_cgroup().pids_max, Some(777));
    }

    #[test]
    fn an_administrator_gets_no_ceiling_at_all() {
        let stored = limits(4096, CpuMode::Cap, 2.0, 512);
        let applied = Budget::of(PanelRole::Admin, stored).to_cgroup();

        assert_eq!(applied.memory_high_bytes, None, "memory.high is written as max");
        assert_eq!(applied.memory_max_bytes, None, "memory.max is written as max");
        assert_eq!(applied.cpu_quota_percent, None, "cpu.max is written as max");
        assert_eq!(applied.pids_max, None, "pids.max is written as max");
    }

    #[test]
    fn the_disk_is_measured_and_never_reaches_the_kernel() {
        let capped =
            Budget::Held(UserLimits { disk_mib: 1024, ..limits(4096, CpuMode::Cap, 2.0, 512) });
        assert_eq!(capped.disk_mib(), Some(1024));
        assert_eq!(capped.disk_limit_bytes(), Some(1024 * MIB));
        assert!(!capped.disk_exceeded_by(1024 * MIB), "at the limit is not over it");
        assert!(capped.disk_exceeded_by(1024 * MIB + 1));
        assert!(capped.has_disk_room_for(1000 * MIB, 24 * MIB));
        assert!(!capped.has_disk_room_for(1000 * MIB, 25 * MIB));
        assert!(!capped.has_disk_room_for(u64::MAX, 1), "a wrapping sum is not free room");

        let unlimited = Budget::of(PanelRole::Admin, limits(4096, CpuMode::Cap, 2.0, 512));
        assert_eq!(unlimited.disk_mib(), None);
        assert!(!unlimited.disk_exceeded_by(u64::MAX));
        assert!(unlimited.has_disk_room_for(u64::MAX, u64::MAX));
    }

    #[test]
    fn a_promotion_takes_the_ceilings_off_and_a_demotion_puts_them_back() {
        let stored = limits(4096, CpuMode::Cap, 2.0, 512);

        let promoted = Budget::of(PanelRole::Admin, stored);
        assert!(promoted.is_unlimited());
        assert_eq!(promoted.limits(), None);
        assert_eq!(promoted.memory_mib(), None);
        assert_eq!(promoted.cpu_cores(), None);
        assert_eq!(promoted.pids_max(), None);
        assert_eq!(promoted.disk_mib(), None);

        let demoted = Budget::of(PanelRole::User, stored);
        assert_eq!(demoted, Budget::Held(stored));
        assert_eq!(demoted.to_cgroup().memory_high_bytes, Some(4096 * MIB));
        assert_eq!(demoted.to_cgroup().cpu_quota_percent, Some(200));
    }

    #[test]
    fn a_budget_without_a_ceiling_is_never_exceeded_and_always_has_room() {
        let unlimited = Budget::of(PanelRole::Admin, limits(4096, CpuMode::Cap, 2.0, 512));
        assert!(!unlimited.exceeded_by(u32::MAX));
        assert!(unlimited.has_room_for(u32::MAX, u32::MAX), "not even a sum that would wrap");

        let capped = held(4096, CpuMode::Cap, 2.0, 512);
        assert!(!capped.exceeded_by(4096), "at the limit is not over it");
        assert!(capped.exceeded_by(4097));
        assert!(capped.has_room_for(2048, 2048));
        assert!(!capped.has_room_for(2048, 2049));
        assert!(!capped.has_room_for(u32::MAX, 1), "a wrapping sum is not free room");
    }

    #[test]
    fn the_numbers_of_an_administrator_are_still_checked() {
        assert!(check(&limits(128, CpuMode::Cap, 2.0, 512)).is_err(), "12.8 holds for every row");
    }
}
