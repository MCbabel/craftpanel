#![allow(dead_code)]

mod adoptium;
mod error;
mod inventory;
mod progress;
pub mod report;
mod unpack;

#[cfg(test)]
mod attack;
#[cfg(test)]
pub(crate) mod harness;
#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use futures::TryStreamExt;

use crate::loaders::{checksum, Http};
use crate::settings::runtimes::{self, JavaRuntime, Search, Source};

#[allow(unused_imports)]
pub use self::{
    adoptium::Arch,
    error::{JavaError, Result},
    inventory::{Inventory, RuntimeOverview, MAJORS},
    progress::{Progress, Stage},
};

const ARCHIVE: &str = "archive.tar.gz";
const TREE: &str = "tree";
const READY: &str = "ready";
const PREVIOUS: &str = "previous";
const ARCHIVE_CEILING: u64 = 128 * 1024 * 1024;
const REACHABLE: u32 = 0o755;
const PASSABLE: u32 = 0o001;
const WRITABLE_BY_ANYONE: u32 = 0o002;
const STAGING_MODE: u32 = 0o700;
const ARCHIVE_MODE: u32 = 0o600;
const ANNOUNCED_SLACK: u64 = 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Installed {
    pub runtime: JavaRuntime,
    pub home: PathBuf,
    pub fresh: bool,
}

struct Job {
    gate: tokio::sync::Mutex<()>,
    progress: Arc<Progress>,
}

pub struct Runtimes {
    http: Http,
    base: String,
    data_dir: PathBuf,
    jobs: Mutex<HashMap<u32, Arc<Job>>>,
}

impl Runtimes {
    pub fn new(data_dir: impl Into<PathBuf>) -> Result<Self> {
        Self::with_base(data_dir, adoptium::BASE)
    }

    pub fn with_base(data_dir: impl Into<PathBuf>, base: impl Into<String>) -> Result<Self> {
        let base = base.into().trim_end_matches('/').to_owned();
        Ok(Self {
            http: Http::bound_to(adoptium::origins(&base))?,
            base,
            data_dir: data_dir.into(),
            jobs: Mutex::default(),
        })
    }

    pub fn home(&self, major: u32) -> PathBuf {
        self.data_dir.join("runtimes").join(format!("java-{major}"))
    }

    pub fn present(&self, major: u32) -> Option<JavaRuntime> {
        let home = self.home(major);
        runtimes::discover(&self.data_dir, &Search::nowhere()).into_iter().find(|runtime| {
            runtime.source == Source::Managed
                && runtime.major == major
                && runtime.path.as_deref().is_some_and(|path| Path::new(path).starts_with(&home))
        })
    }

    pub fn watch(&self, major: u32) -> Arc<Progress> {
        Arc::clone(&self.job(major).progress)
    }

    pub async fn install(&self, major: u32) -> Result<Installed> {
        let job = self.job(major);
        let _turn = job.gate.lock().await;
        let outcome = self.lay_out(major, &job.progress).await;
        self.retire(major, &job);
        outcome
    }

    pub async fn reinstall(&self, major: u32) -> Result<Installed> {
        let job = self.job(major);
        let _turn = job.gate.lock().await;
        let outcome = self.replace(major, &job.progress).await;
        self.retire(major, &job);
        outcome
    }

    fn job(&self, major: u32) -> Arc<Job> {
        let mut jobs = self.jobs.lock().expect("the runtime jobs outlive their panics");
        Arc::clone(jobs.entry(major).or_insert_with(|| {
            Arc::new(Job { gate: tokio::sync::Mutex::default(), progress: Arc::default() })
        }))
    }

    fn retire(&self, major: u32, job: &Arc<Job>) {
        let mut jobs = self.jobs.lock().expect("the runtime jobs outlive their panics");
        if Arc::strong_count(job) <= 2 {
            jobs.remove(&major);
        }
    }

    async fn lay_out(&self, major: u32, progress: &Arc<Progress>) -> Result<Installed> {
        stand_back_up(&self.home(major), &self.staging(major))?;
        make_reachable(&self.data_dir, major)?;
        if let Some(runtime) = self.present(major) {
            progress.settled();
            return Ok(Installed { runtime, home: self.home(major), fresh: false });
        }
        self.replace(major, progress).await
    }

    async fn replace(&self, major: u32, progress: &Arc<Progress>) -> Result<Installed> {
        let home = self.home(major);
        let arch = Arch::here().ok_or_else(|| JavaError::UnsupportedMachine {
            arch: std::env::consts::ARCH.to_owned(),
            major,
        })?;

        let staging = self.staging(major);
        stand_back_up(&home, &staging)?;
        make_reachable(&self.data_dir, major)?;
        empty_out(&staging)?;
        let laid = self.fetch(major, arch, &staging, &home, progress).await;
        let _ = std::fs::remove_dir_all(&staging);
        laid?;

        runtimes::forget(&self.data_dir);
        let runtime = self.present(major).ok_or_else(|| JavaError::Incomplete {
            major,
            reason: format!("{} holds nothing that reads as a Java runtime", home.display()),
        })?;
        progress.settled();

        Ok(Installed { runtime, home, fresh: true })
    }

    async fn fetch(
        &self,
        major: u32,
        arch: Arch,
        staging: &Path,
        home: &Path,
        progress: &Arc<Progress>,
    ) -> Result<()> {
        progress.asking();
        let release = adoptium::latest(&self.http, &self.base, major, arch).await?;
        if release.size > ARCHIVE_CEILING {
            return Err(JavaError::AnnouncedTooLarge {
                major,
                announced: release.size,
                ceiling: ARCHIVE_CEILING,
            });
        }

        let archive = staging.join(ARCHIVE);
        let tree = staging.join(TREE);

        progress.downloading(release.size);
        let response = self.http.stream(adoptium::SERVICE, &release.url).await?;
        let counted = response
            .bytes_stream()
            .inspect_ok(|chunk| progress.advanced(chunk.len() as u64));
        checksum::write_capped(
            counted,
            &archive,
            Some(&release.checksum),
            &release.url,
            ceiling(release.size),
        )
        .await?;
        std::fs::set_permissions(&archive, std::fs::Permissions::from_mode(ARCHIVE_MODE))
            .map_err(|err| JavaError::write(&archive, err))?;

        progress.unpacking();
        let watched = Arc::clone(progress);
        let into = tree.clone();
        tokio::task::spawn_blocking(move || unpack::tree(&archive, &into, major, &watched))
            .await
            .map_err(|err| JavaError::Interrupted {
                major,
                reason: format!("the unpacking task died: {err}"),
            })??;

        usable(&tree, major)?;
        swap_in(&tree, home, staging)
    }

    fn staging(&self, major: u32) -> PathBuf {
        self.data_dir.join("runtimes").join(format!(".java-{major}.new"))
    }
}

fn ceiling(announced: u64) -> u64 {
    match announced {
        0 => ARCHIVE_CEILING,
        size => size.saturating_add(ANNOUNCED_SLACK).min(ARCHIVE_CEILING),
    }
}

fn usable(home: &Path, major: u32) -> Result<()> {
    let refuse = |reason: String| JavaError::Incomplete { major, reason };
    let binary = home.join("bin").join("java");

    let meta = std::fs::metadata(&binary)
        .map_err(|err| refuse(format!("bin/java is not there: {err}")))?;
    if !meta.is_file() {
        return Err(refuse("bin/java is no file".to_owned()));
    }
    if meta.permissions().mode() & 0o111 == 0 {
        return Err(refuse("bin/java cannot be run".to_owned()));
    }

    let found = runtimes::read_home(home, Source::Managed)
        .ok_or_else(|| refuse("it carries no readable release file".to_owned()))?;
    if found.major != major {
        return Err(refuse(format!(
            "it is Java {}, not the Java {major} that was asked for",
            found.major
        )));
    }
    Ok(())
}

fn stand_back_up(home: &Path, staging: &Path) -> Result<()> {
    if !home.exists() {
        let left = [READY, PREVIOUS]
            .into_iter()
            .map(|name| staging.join(name))
            .find(|candidate| candidate.exists());
        match left {
            Some(left) if ours_alone(staging) && ours_alone(&left) => {
                std::fs::rename(&left, home).map_err(|err| JavaError::write(home, err))?;
            }
            Some(left) => tracing::warn!(
                "{} is not ours alone, so it is swept up rather than put back",
                left.display()
            ),
            None => {}
        }
    }
    sweep(staging)
}

fn ours_alone(path: &Path) -> bool {
    let Ok(found) = std::fs::symlink_metadata(path) else {
        return false;
    };
    found.is_dir()
        && found.uid() == unsafe { libc::geteuid() }
        && found.permissions().mode() & 0o022 == 0
}

fn sweep(path: &Path) -> Result<()> {
    match std::fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(JavaError::write(path, err)),
    }
}

fn empty_out(path: &Path) -> Result<()> {
    sweep(path)?;
    std::fs::DirBuilder::new()
        .mode(STAGING_MODE)
        .create(path)
        .map_err(|err| JavaError::write(path, err))
}

fn make_reachable(data_dir: &Path, major: u32) -> Result<()> {
    let above = data_dir.join("runtimes");
    if !above.is_dir() {
        std::fs::create_dir_all(&above).map_err(|err| JavaError::write(&above, err))?;
        set_mode(&above, REACHABLE)?;
    }

    let found = std::fs::metadata(&above).map_err(|err| JavaError::write(&above, err))?;
    let mode = found.permissions().mode() & 0o7777;
    if mode & WRITABLE_BY_ANYONE != 0 {
        return Err(JavaError::Exposed { major, path: above, mode });
    }
    if mode & PASSABLE != PASSABLE && found.uid() == unsafe { libc::geteuid() } {
        set_mode(&above, mode | REACHABLE)?;
    }
    strangers_pass(&above, major)?;
    strangers_pass(data_dir, major)
}

fn strangers_pass(path: &Path, major: u32) -> Result<()> {
    let found = std::fs::metadata(path).map_err(|err| JavaError::write(path, err))?;
    let mode = found.permissions().mode() & 0o7777;
    if mode & PASSABLE == PASSABLE {
        return Ok(());
    }
    Err(JavaError::Unreachable { major, path: path.to_owned(), mode, owner: found.uid() })
}

fn set_mode(path: &Path, mode: u32) -> Result<()> {
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
        .map_err(|err| JavaError::write(path, err))
}

fn swap_in(tree: &Path, home: &Path, staging: &Path) -> Result<()> {
    let ready = staging.join(READY);
    let aside = staging.join(PREVIOUS);
    std::fs::rename(tree, &ready).map_err(|err| JavaError::write(&ready, err))?;

    let stood_there = match std::fs::rename(home, &aside) {
        Ok(()) => true,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => false,
        Err(err) => return Err(JavaError::write(home, err)),
    };

    match std::fs::rename(&ready, home) {
        Ok(()) => Ok(()),
        Err(err) => {
            if stood_there {
                let _ = std::fs::rename(&aside, home);
            }
            Err(JavaError::write(home, err))
        }
    }
}
