use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use craftpanel_proto::ResourceLimits;

const PERIOD_MICROS: u32 = 100_000;

pub fn ensure(root: &Path, user_id: &str) -> Result<PathBuf> {
    if !root.exists() {
        std::fs::create_dir_all(root)
            .with_context(|| format!("creating cgroup root {}", root.display()))?;
    }
    delegate_controllers(root);

    let dir = root.join(format!("user-{user_id}"));
    if !dir.exists() {
        std::fs::create_dir(&dir)
            .with_context(|| format!("creating cgroup {}", dir.display()))?;
    }
    Ok(dir)
}

fn delegate_controllers(root: &Path) {
    let available = std::fs::read_to_string(root.join("cgroup.controllers"))
        .unwrap_or_default();
    let wanted: Vec<&str> = ["cpu", "memory", "pids"]
        .into_iter()
        .filter(|c| available.split_whitespace().any(|a| a == *c))
        .collect();

    if wanted.is_empty() {
        return;
    }

    let line: String = wanted.iter().map(|c| format!("+{c} ")).collect();
    if let Err(err) = std::fs::write(root.join("cgroup.subtree_control"), line.trim()) {
        tracing::warn!(
            root = %root.display(),
            "the controllers stay off, so no memory, cpu or pid ceiling reaches the kernel: {err}"
        );
    }
}

pub fn open_roll(cgroup: &Path) -> Result<std::fs::File> {
    let path = cgroup.join("cgroup.procs");
    std::fs::OpenOptions::new()
        .write(true)
        .open(&path)
        .with_context(|| format!("opening {}", path.display()))
}

pub fn apply(root: &Path, user_id: &str, limits: &ResourceLimits) -> Result<()> {
    let dir = ensure(root, user_id)?;

    if !dir.join("memory.max").exists() {
        tracing::warn!(
            cgroup = %dir.display(),
            "no controller files here — this account runs without a memory, cpu or pid ceiling"
        );
    }

    write_limit(&dir, "memory.high", limits.memory_high_bytes.map(|v| v.to_string()))?;
    write_limit(&dir, "memory.max", limits.memory_max_bytes.map(|v| v.to_string()))?;
    write_limit(&dir, "pids.max", limits.pids_max.map(|v| v.to_string()))?;

    let cpu = limits.cpu_quota_percent.map(|percent| {
        let quota = (u64::from(PERIOD_MICROS) * u64::from(percent)) / 100;
        format!("{quota} {PERIOD_MICROS}")
    });
    write_limit(&dir, "cpu.max", cpu)?;

    Ok(())
}

fn write_limit(dir: &Path, file: &str, value: Option<String>) -> Result<()> {
    let path = dir.join(file);
    if !path.exists() {
        return Ok(());
    }
    let value = value.unwrap_or_else(|| "max".to_owned());
    std::fs::write(&path, &value)
        .with_context(|| format!("writing {value} to {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpu_percent_becomes_quota_over_period() {
        let limits = ResourceLimits { cpu_quota_percent: Some(200), ..Default::default() };
        let percent = limits.cpu_quota_percent.unwrap();
        let quota = (u64::from(PERIOD_MICROS) * u64::from(percent)) / 100;
        assert_eq!(format!("{quota} {PERIOD_MICROS}"), "200000 100000");
    }
}
