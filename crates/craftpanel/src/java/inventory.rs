use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use axum::http::StatusCode;
use serde::Serialize;
use sqlx::SqlitePool;
use time::OffsetDateTime;

use super::{Arch, Installed, Progress, Runtimes};
use crate::auth::error::{Failure, Result};
use crate::auth::LiveServers;
use crate::model::{Id, JreVendor, Timestamp};
use crate::settings::runtimes::{self, JavaRuntime, Search, Source};

pub const MAJORS: [u32; 4] = [8, 17, 21, 25];

#[derive(Debug, Clone, Serialize)]
pub struct RuntimeOverview {
    pub auto_install: bool,
    pub architecture: Option<&'static str>,
    pub directory: String,
    pub total_bytes: u64,
    pub majors: Vec<MajorEntry>,
}

#[derive(Debug, Clone, Serialize)]
pub struct MajorEntry {
    pub major: u32,
    pub fetchable: bool,
    pub runtime: Option<LaidRuntime>,
    pub system: Option<SystemRuntime>,
    pub job: Option<JobView>,
    pub servers: u32,
    pub running: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct LaidRuntime {
    pub vendor: JreVendor,
    pub version: String,
    pub path: String,
    pub directory: String,
    pub size_bytes: u64,
    pub laid_at: Option<Timestamp>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SystemRuntime {
    pub vendor: JreVendor,
    pub version: String,
    pub path: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct JobView {
    pub stage: &'static str,
    pub running: bool,
    pub done_bytes: u64,
    pub total_bytes: u64,
    pub share: f64,
    pub failure: Option<String>,
    pub failure_code: Option<&'static str>,
}

#[derive(Clone)]
struct Attempt {
    progress: Arc<Progress>,
    running: bool,
    failure: Option<(&'static str, String)>,
}

#[derive(Default)]
struct Use {
    servers: u32,
    running: Vec<String>,
}

pub struct Inventory {
    pool: SqlitePool,
    runtimes: Arc<Runtimes>,
    data_dir: PathBuf,
    search: Search,
    attempts: Mutex<HashMap<u32, Attempt>>,
}

impl Inventory {
    pub fn new(
        pool: SqlitePool,
        runtimes: Arc<Runtimes>,
        data_dir: impl Into<PathBuf>,
        search: Search,
    ) -> Self {
        Self { pool, runtimes, data_dir: data_dir.into(), search, attempts: Mutex::default() }
    }

    pub async fn start(self: &Arc<Self>, major: u32, live: &LiveServers) -> Result<()> {
        if !MAJORS.contains(&major) {
            let named = MAJORS.map(|offered| offered.to_string()).join(", ");
            return Err(Failure::not_found(
                "java_major_unknown",
                format!("the panel fetches Java {named}, and Java {major} is none of them"),
            ));
        }
        self.idle(major)?;
        self.undisturbed(major, live).await?;

        let progress = self.runtimes.watch(major);
        {
            let mut attempts = self.attempts.lock().expect("the attempts outlive their panics");
            if attempts.get(&major).is_some_and(|attempt| attempt.running) {
                return Err(already_running(major));
            }
            attempts.insert(major, Attempt { progress, running: true, failure: None });
        }

        let inventory = Arc::clone(self);
        tokio::spawn(async move {
            let laid = inventory.runtimes.reinstall(major).await;
            inventory.settle(major, laid);
        });
        Ok(())
    }

    pub async fn remove(&self, major: u32, live: &LiveServers) -> Result<()> {
        if self.runtimes.present(major).is_none() {
            return Err(Failure::not_found(
                "java_runtime_not_here",
                format!("the panel has laid no Java {major} down"),
            ));
        }
        self.idle(major)?;
        self.undisturbed(major, live).await?;

        let home = self.runtimes.home(major);
        std::fs::remove_dir_all(&home).map_err(|err| {
            Failure::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "java_runtime_unwritable",
                format!("removing {} failed: {err}", home.display()),
            )
        })?;
        runtimes::forget(&self.data_dir);
        self.attempts.lock().expect("the attempts outlive their panics").remove(&major);
        Ok(())
    }

    pub async fn overview(&self, live: &LiveServers) -> Result<RuntimeOverview> {
        let auto_install = crate::auth::settings::load(&self.pool).await?.java_auto_install;
        let mut used = self.usage(live).await?;
        let attempts = self.attempts.lock().expect("the attempts outlive their panics").clone();
        let found = runtimes::cached(&self.data_dir, &self.search);

        let mut wanted: Vec<u32> = MAJORS.to_vec();
        let asked_for = found
            .iter()
            .filter(|runtime| runtime.source == Source::Managed)
            .map(|runtime| runtime.major)
            .chain(used.keys().copied());
        for major in asked_for {
            if !wanted.contains(&major) {
                wanted.push(major);
            }
        }
        wanted.sort_unstable();

        let mut total_bytes = 0;
        let mut majors = Vec::with_capacity(wanted.len());
        for major in wanted {
            let home = self.runtimes.home(major);
            let laid = found.iter().find(|runtime| under(runtime, major, &home)).map(|runtime| {
                let measured = crate::files::measure(&home);
                total_bytes += measured.bytes;
                LaidRuntime {
                    vendor: runtime.vendor,
                    version: runtime.version.clone(),
                    path: runtime.path.clone().unwrap_or_default(),
                    directory: home.to_string_lossy().into_owned(),
                    size_bytes: measured.bytes,
                    laid_at: laid_at(&home),
                }
            });
            let system = found
                .iter()
                .find(|runtime| runtime.source == Source::System && runtime.major == major)
                .map(|runtime| SystemRuntime {
                    vendor: runtime.vendor,
                    version: runtime.version.clone(),
                    path: runtime.path.clone().unwrap_or_default(),
                });
            let seen = used.remove(&major).unwrap_or_default();

            majors.push(MajorEntry {
                major,
                fetchable: MAJORS.contains(&major),
                runtime: laid,
                system,
                job: attempts.get(&major).map(view),
                servers: seen.servers,
                running: seen.running,
            });
        }

        Ok(RuntimeOverview {
            auto_install,
            architecture: Arch::here().map(Arch::as_str),
            directory: self.data_dir.join("runtimes").to_string_lossy().into_owned(),
            total_bytes,
            majors,
        })
    }

    fn idle(&self, major: u32) -> Result<()> {
        let attempts = self.attempts.lock().expect("the attempts outlive their panics");
        match attempts.get(&major) {
            Some(attempt) if attempt.running => Err(already_running(major)),
            _ => Ok(()),
        }
    }

    async fn undisturbed(&self, major: u32, live: &LiveServers) -> Result<()> {
        if self.runtimes.present(major).is_none() {
            return Ok(());
        }
        let running = self.usage(live).await?.remove(&major).unwrap_or_default().running;
        if running.is_empty() {
            return Ok(());
        }

        Err(Failure::conflict(
            "java_runtime_in_use",
            format!(
                "Java {major} is what {} is running on right now, so stop {} first: replacing \
                 or removing a runtime takes its files out from under a live server",
                running.join(", "),
                if running.len() == 1 { "it" } else { "them" },
            ),
        ))
    }

    async fn usage(&self, live: &LiveServers) -> Result<HashMap<u32, Use>> {
        #[derive(sqlx::FromRow)]
        struct Row {
            id: Id,
            name: String,
            java_major: Option<u32>,
            game_version: Option<String>,
        }

        let rows = sqlx::query_as::<_, Row>(
            "SELECT id, name, java_major, game_version FROM servers ORDER BY name",
        )
        .fetch_all(&self.pool)
        .await?;
        let alive = live.ids().await;

        let mut used: HashMap<u32, Use> = HashMap::new();
        for row in rows {
            let Some(major) = row
                .java_major
                .or_else(|| runtimes::default_major(row.game_version.as_deref()?))
            else {
                continue;
            };
            let seen = used.entry(major).or_default();
            seen.servers += 1;
            if alive.contains(&row.id) {
                seen.running.push(row.name);
            }
        }
        Ok(used)
    }

    fn settle(&self, major: u32, laid: super::Result<Installed>) {
        let mut attempts = self.attempts.lock().expect("the attempts outlive their panics");
        match laid {
            Ok(_) => {
                attempts.remove(&major);
            }
            Err(err) => {
                tracing::warn!("could not lay Java {major} down: {err}");
                if let Some(attempt) = attempts.get_mut(&major) {
                    attempt.running = false;
                    attempt.failure = Some((err.code(), err.to_string()));
                }
            }
        }
    }
}

fn under(runtime: &JavaRuntime, major: u32, home: &Path) -> bool {
    runtime.source == Source::Managed
        && runtime.major == major
        && runtime.path.as_deref().is_some_and(|path| Path::new(path).starts_with(home))
}

fn already_running(major: u32) -> Failure {
    Failure::conflict("java_install_running", format!("Java {major} is being fetched already"))
}

fn view(attempt: &Attempt) -> JobView {
    JobView {
        stage: attempt.progress.stage().as_str(),
        running: attempt.running,
        done_bytes: attempt.progress.done(),
        total_bytes: attempt.progress.total(),
        share: attempt.progress.share(),
        failure: attempt.failure.as_ref().map(|(_, said)| said.clone()),
        failure_code: attempt.failure.as_ref().map(|(code, _)| *code),
    }
}

fn laid_at(home: &Path) -> Option<Timestamp> {
    let modified = std::fs::metadata(home).ok()?.modified().ok()?;
    Some(Timestamp::at(OffsetDateTime::from(modified)))
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::super::harness::{a_data_dir, Scratch};
    use super::*;
    use crate::auth::harness::{a_server, a_user, test_pool};

    fn a_runtime(data_dir: &Path, major: u32, version: &str) {
        let home = data_dir.join("runtimes").join(format!("java-{major}"));
        std::fs::create_dir_all(home.join("bin")).expect("a runtime directory");
        std::fs::write(home.join("bin").join("java"), "#!/bin/sh\n").expect("a launcher");
        std::fs::write(
            home.join("release"),
            format!("IMPLEMENTOR=\"Eclipse Adoptium\"\nJAVA_VERSION=\"{version}\"\n"),
        )
        .expect("a release file");
    }

    fn an_inventory(pool: &SqlitePool, dir: &Scratch) -> Arc<Inventory> {
        let runtimes =
            Arc::new(Runtimes::with_base(dir.path(), "http://127.0.0.1:1").expect("a client"));
        Arc::new(Inventory::new(pool.clone(), runtimes, dir.path(), Search::nowhere()))
    }

    async fn a_server_on(pool: &SqlitePool, name: &str, game_version: &str) -> Id {
        let owner = a_user(pool, &format!("owner-of-{name}")).await;
        let server = a_server(pool, owner, name, 2048).await;
        sqlx::query("UPDATE servers SET game_version = ? WHERE id = ?")
            .bind(game_version)
            .bind(server)
            .execute(pool)
            .await
            .expect("a game version");
        server
    }

    fn entry(seen: &RuntimeOverview, major: u32) -> &MajorEntry {
        seen.majors.iter().find(|entry| entry.major == major).expect("a row for that major")
    }

    #[tokio::test]
    async fn every_major_the_panel_fetches_has_a_row_whether_it_is_there_or_not() {
        let pool = test_pool().await;
        let dir = a_data_dir();
        a_runtime(dir.path(), 21, "21.0.12+7");

        let seen = an_inventory(&pool, &dir).overview(&LiveServers::none()).await.unwrap();

        assert_eq!(
            seen.majors.iter().map(|row| row.major).collect::<Vec<_>>(),
            MAJORS.to_vec(),
            "one row per major, and in order"
        );
        assert!(seen.auto_install, "0015 leaves it on");
        assert!(seen.directory.ends_with("runtimes"));

        let laid = entry(&seen, 21).runtime.as_ref().expect("the one on disk");
        assert_eq!(laid.version, "21.0.12+7");
        assert_eq!(laid.vendor, JreVendor::Temurin);
        assert!(laid.directory.ends_with("java-21"));
        assert!(laid.size_bytes > 0, "it was measured, not guessed");
        assert_eq!(seen.total_bytes, laid.size_bytes);
        assert!(entry(&seen, 8).runtime.is_none());
        assert!(entry(&seen, 8).fetchable);
    }

    #[tokio::test]
    async fn the_row_counts_the_servers_that_want_it_and_names_the_ones_running() {
        let pool = test_pool().await;
        let dir = a_data_dir();
        a_runtime(dir.path(), 8, "1.8.0_422");

        let old = a_server_on(&pool, "aether", "1.12.2").await;
        a_server_on(&pool, "beacon", "1.7.10").await;
        a_server_on(&pool, "current", "1.21.8").await;

        let seen = an_inventory(&pool, &dir)
            .overview(&LiveServers::fixed([old]))
            .await
            .unwrap();

        assert_eq!(entry(&seen, 8).servers, 2, "both old worlds ask for Java 8");
        assert_eq!(entry(&seen, 8).running, vec!["aether".to_owned()]);
        assert_eq!(entry(&seen, 21).servers, 1);
        assert!(entry(&seen, 21).running.is_empty());
    }

    #[tokio::test]
    async fn a_runtime_a_server_is_running_on_is_neither_replaced_nor_removed() {
        let pool = test_pool().await;
        let dir = a_data_dir();
        a_runtime(dir.path(), 8, "1.8.0_422");
        let old = a_server_on(&pool, "aether", "1.12.2").await;

        let inventory = an_inventory(&pool, &dir);
        let live = LiveServers::fixed([old]);

        let refused = inventory.start(8, &live).await.unwrap_err();
        assert_eq!(refused.code(), "java_runtime_in_use");
        assert!(refused.to_string().contains("aether"), "{refused}");

        let same = inventory.remove(8, &live).await.unwrap_err();
        assert_eq!(same.code(), "java_runtime_in_use");
        assert!(dir.path().join("runtimes").join("java-8").is_dir(), "nothing was taken away");

        assert!(
            inventory.remove(8, &LiveServers::none()).await.is_ok(),
            "and once it is stopped it goes"
        );
    }

    #[tokio::test]
    async fn a_major_that_is_not_there_cannot_be_removed_and_says_so() {
        let pool = test_pool().await;
        let dir = a_data_dir();
        let inventory = an_inventory(&pool, &dir);

        let refused = inventory.remove(17, &LiveServers::none()).await.unwrap_err();
        assert_eq!(refused.code(), "java_runtime_not_here");
    }

    #[tokio::test]
    async fn removing_takes_the_tree_and_the_cache_with_it() {
        let pool = test_pool().await;
        let dir = a_data_dir();
        a_runtime(dir.path(), 17, "17.0.13+11");
        let inventory = an_inventory(&pool, &dir);

        let before = inventory.overview(&LiveServers::none()).await.unwrap();
        assert!(entry(&before, 17).runtime.is_some());

        inventory.remove(17, &LiveServers::none()).await.unwrap();

        assert!(!dir.path().join("runtimes").join("java-17").exists());
        let after = inventory.overview(&LiveServers::none()).await.unwrap();
        assert!(entry(&after, 17).runtime.is_none(), "the minute-long cache was told to forget");
        assert_eq!(after.total_bytes, 0);
    }

    #[tokio::test]
    async fn a_version_adoptium_is_never_asked_for_is_refused_before_anything_is_touched() {
        let pool = test_pool().await;
        let dir = a_data_dir();
        let inventory = an_inventory(&pool, &dir);

        let refused = inventory.start(11, &LiveServers::none()).await.unwrap_err();
        assert_eq!(refused.code(), "java_major_unknown");
        assert!(refused.to_string().contains("21"), "it says which ones it does fetch: {refused}");
    }

    #[tokio::test]
    async fn a_version_only_a_server_asks_for_gets_a_row_of_its_own() {
        let pool = test_pool().await;
        let dir = a_data_dir();
        let owner = a_user(&pool, "max").await;
        let server = a_server(&pool, owner, "elevenish", 2048).await;
        sqlx::query("UPDATE servers SET java_major = 11 WHERE id = ?")
            .bind(server)
            .execute(&pool)
            .await
            .unwrap();

        let seen = an_inventory(&pool, &dir).overview(&LiveServers::none()).await.unwrap();

        assert_eq!(
            seen.majors.iter().map(|row| row.major).collect::<Vec<_>>(),
            vec![8, 11, 17, 21, 25],
            "a major nobody can fetch still has to be visible while a server wants it"
        );
        let eleven = entry(&seen, 11);
        assert!(!eleven.fetchable);
        assert_eq!(eleven.servers, 1);
    }

    #[tokio::test]
    async fn the_overview_reports_the_switch_the_administrator_set() {
        let pool = test_pool().await;
        let dir = a_data_dir();
        sqlx::query("UPDATE panel_settings SET java_auto_install = 0 WHERE id = 1")
            .execute(&pool)
            .await
            .unwrap();

        let seen = an_inventory(&pool, &dir).overview(&LiveServers::none()).await.unwrap();
        assert!(!seen.auto_install);
    }
}
