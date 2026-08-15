use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures::StreamExt;
use serde::Serialize;
use sha2::Digest;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::task::JoinHandle;

use super::Secret;

pub const RELEASE: &str = "v1.0.10";

const BINARIES: &[Build] = &[
    Build {
        arch: "x86_64",
        asset: "playit-linux-amd64",
        sha256: "2df7d9f10227ab312b1ad341853db4e8a8243df5cfcdbae58713a4271711c339",
    },
    Build {
        arch: "aarch64",
        asset: "playit-linux-aarch64",
        sha256: "4c0db3e7b3a8158e249441c2f0b73f54e83429395890c7b1ca45fd7a6303d763",
    },
    Build {
        arch: "arm",
        asset: "playit-linux-armv7",
        sha256: "92ec60988b1246e07ac090c663128bd04bdc0d7ff388db520e1ff7bb4e5003e0",
    },
    Build {
        arch: "x86",
        asset: "playit-linux-i686",
        sha256: "d7215f3995e486bc231b3b542aa5f1ac6b0d604f8dae97bb14a9a64b49b3ed50",
    },
];

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(300);
const STEADY: Duration = Duration::from_secs(60);
const GIVE_UP_AFTER: u32 = 5;
const BACKOFF_CEILING: Duration = Duration::from_secs(60);

const WORKER_THREADS: &str = "2";

struct Build {
    arch: &'static str,
    asset: &'static str,
    sha256: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentState {
    Absent,
    Starting,
    Running,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BinaryState {
    Absent,
    Fetching,
    Ready,
    Failed,
}

#[derive(Debug, Clone, Serialize)]
pub struct AgentStatus {
    pub state: AgentState,
    pub version: Option<String>,
    pub detail: Option<String>,
}

impl AgentStatus {
    pub fn absent() -> Self {
        Self { state: AgentState::Absent, version: None, detail: None }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct BinaryStatus {
    pub state: BinaryState,
    pub version: Option<String>,
    pub arch: String,
    pub detail: Option<String>,
}

pub struct Binary {
    cache: PathBuf,
    http: reqwest::Client,
    source: String,
    build: Option<&'static Build>,
    status: Mutex<BinaryStatus>,
    gate: tokio::sync::Mutex<()>,
}

impl Binary {
    pub fn new(data_dir: &Path) -> Result<Arc<Self>, String> {
        Self::from_source(
            data_dir,
            &format!("https://github.com/playit-cloud/playit-agent/releases/download/{RELEASE}"),
        )
    }

    pub(super) fn from_source(data_dir: &Path, source: &str) -> Result<Arc<Self>, String> {
        Self::against(
            data_dir.join("cache").join("playit"),
            source.to_owned(),
            for_this_machine(),
        )
    }

    fn against(
        cache: PathBuf,
        source: String,
        build: Option<&'static Build>,
    ) -> Result<Arc<Self>, String> {
        let http = reqwest::Client::builder()
            .user_agent(concat!("craftpanel/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(CONNECT_TIMEOUT)
            .build()
            .map_err(|err| err.to_string())?;

        Ok(Arc::new(Self {
            cache,
            http,
            source,
            build,
            status: Mutex::new(BinaryStatus {
                state: BinaryState::Absent,
                version: None,
                arch: std::env::consts::ARCH.to_owned(),
                detail: None,
            }),
            gate: tokio::sync::Mutex::default(),
        }))
    }

    pub fn status(&self) -> BinaryStatus {
        self.status.lock().expect("the binary status lock").clone()
    }

    pub async fn ensure(&self) -> Result<PathBuf, Broken> {
        let _one_at_a_time = self.gate.lock().await;

        let wanted = self.build.ok_or_else(|| {
            Broken::Damaged(format!(
                "playit.gg publishes no tunnel daemon for {}",
                std::env::consts::ARCH
            ))
        })?;

        let path = self.cache.join(format!("playit-{RELEASE}-{}", wanted.arch));
        if digest_of(&path).await.as_deref() == Some(wanted.sha256) {
            self.set(BinaryState::Ready, None);
            return Ok(path);
        }

        self.set(BinaryState::Fetching, None);
        tokio::fs::create_dir_all(&self.cache)
            .await
            .map_err(|err| Broken::Unreachable(err.to_string()))?;
        let _ = set_mode(&self.cache, 0o700).await;

        let url = format!("{}/{}", self.source, wanted.asset);
        self.download(&url, &path, wanted.sha256).await?;
        let _ = set_mode(&path, 0o700).await;
        self.sweep_old(&path).await;

        self.set(BinaryState::Ready, None);
        Ok(path)
    }

    async fn download(&self, url: &str, dest: &Path, sha256: &str) -> Result<(), Broken> {
        let partial = dest.with_extension("part");
        let outcome = self.collect(url, &partial, sha256).await;
        if outcome.is_err() {
            let _ = tokio::fs::remove_file(&partial).await;
            return outcome;
        }
        tokio::fs::rename(&partial, dest)
            .await
            .map_err(|err| Broken::Unreachable(err.to_string()))
    }

    async fn collect(&self, url: &str, partial: &Path, sha256: &str) -> Result<(), Broken> {
        let response = self
            .http
            .get(url)
            .timeout(DOWNLOAD_TIMEOUT)
            .send()
            .await
            .map_err(|err| Broken::Unreachable(err.to_string()))?;
        if !response.status().is_success() {
            return Err(Broken::Unreachable(format!(
                "GitHub answered {} for {url}",
                response.status()
            )));
        }

        let mut file = tokio::fs::File::create(partial)
            .await
            .map_err(|err| Broken::Unreachable(err.to_string()))?;
        let mut digest = sha2::Sha256::new();
        let mut stream = response.bytes_stream();

        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|err| Broken::Unreachable(err.to_string()))?;
            digest.update(&chunk);
            file.write_all(&chunk)
                .await
                .map_err(|err| Broken::Unreachable(err.to_string()))?;
        }
        file.sync_all().await.map_err(|err| Broken::Unreachable(err.to_string()))?;

        let actual = hex::encode(digest.finalize());
        if actual != sha256 {
            return Err(Broken::Damaged(format!(
                "the tunnel daemon from GitHub hashes to {actual}, but {sha256} was expected"
            )));
        }
        Ok(())
    }

    async fn sweep_old(&self, keep: &Path) {
        let Ok(mut entries) = tokio::fs::read_dir(&self.cache).await else {
            return;
        };
        while let Ok(Some(entry)) = entries.next_entry().await {
            let path = entry.path();
            if path != keep
                && path.file_name().is_some_and(|name| {
                    name.to_string_lossy().starts_with("playit-")
                })
            {
                let _ = tokio::fs::remove_file(path).await;
            }
        }
    }

    fn set(&self, state: BinaryState, detail: Option<String>) {
        let mut status = self.status.lock().expect("the binary status lock");
        status.state = state;
        status.version = matches!(state, BinaryState::Ready).then(|| RELEASE.to_owned());
        status.detail = detail;
    }
}

pub struct Agent {
    dir: PathBuf,
    binary: Arc<Binary>,
    status: Mutex<AgentStatus>,
    running: tokio::sync::Mutex<Option<JoinHandle<()>>>,
}

impl Agent {
    pub fn new(dir: PathBuf, binary: Arc<Binary>) -> Arc<Self> {
        Arc::new(Self {
            dir,
            binary,
            status: Mutex::new(AgentStatus::absent()),
            running: tokio::sync::Mutex::default(),
        })
    }

    pub fn status(&self) -> AgentStatus {
        self.status.lock().expect("the agent status lock").clone()
    }

    pub fn binary(&self) -> BinaryStatus {
        self.binary.status()
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn secret_path(&self) -> PathBuf {
        self.dir.join("secret")
    }

    fn socket_path(&self) -> PathBuf {
        self.dir.join("playitd.sock")
    }

    pub async fn write_secret(&self, secret: &Secret) -> std::io::Result<()> {
        tokio::fs::create_dir_all(&self.dir).await?;
        set_mode(&self.dir, 0o700).await?;

        let path = self.secret_path();
        let partial = path.with_extension("part");
        let mut file = tokio::fs::File::create(&partial).await?;
        set_mode(&partial, 0o600).await?;
        file.write_all(secret.expose().as_bytes()).await?;
        file.write_all(b"\n").await?;
        file.sync_all().await?;
        drop(file);

        tokio::fs::rename(&partial, &path).await
    }

    pub async fn read_secret(&self) -> Option<Secret> {
        let text = tokio::fs::read_to_string(self.secret_path()).await.ok()?;
        Secret::parse(&text).ok()
    }

    pub async fn forget_secret(&self) -> std::io::Result<()> {
        match tokio::fs::remove_file(self.secret_path()).await {
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
            other => other,
        }
    }

    pub async fn forget_everything(&self) -> std::io::Result<()> {
        match tokio::fs::remove_dir_all(&self.dir).await {
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
            other => other,
        }
    }

    pub async fn start(self: &Arc<Self>) {
        let mut running = self.running.lock().await;
        if running.as_ref().is_some_and(|task| !task.is_finished()) {
            return;
        }
        if matches!(self.status().state, AgentState::Failed) {
            return;
        }
        *running = Some(tokio::spawn(Arc::clone(self).supervise()));
    }

    pub async fn is_running(&self) -> bool {
        self.running.lock().await.as_ref().is_some_and(|task| !task.is_finished())
    }

    pub async fn stop(&self) {
        if let Some(task) = self.running.lock().await.take() {
            task.abort();
        }
        self.set_agent(AgentState::Absent, None);
    }

    pub async fn restart(self: &Arc<Self>) {
        self.stop().await;
        self.start().await;
    }

    async fn supervise(self: Arc<Self>) {
        let mut failures = 0u32;

        loop {
            let binary = match self.binary.ensure().await {
                Ok(path) => path,
                Err(Broken::Damaged(detail)) => {
                    self.binary.set(BinaryState::Failed, Some(detail.clone()));
                    self.set_agent(AgentState::Failed, Some(detail));
                    return;
                }
                Err(Broken::Unreachable(detail)) => {
                    self.binary.set(BinaryState::Absent, Some(detail.clone()));
                    failures += 1;
                    if failures >= GIVE_UP_AFTER {
                        self.set_agent(AgentState::Failed, Some(detail));
                        return;
                    }
                    tokio::time::sleep(backoff(failures)).await;
                    continue;
                }
            };

            self.set_agent(AgentState::Starting, None);
            let started = Instant::now();

            match self.run_once(&binary).await {
                Ok(ended) => {
                    if started.elapsed() >= STEADY {
                        failures = 0;
                    }
                    failures += 1;
                    self.set_agent(AgentState::Starting, Some(ended.clone()));
                    if failures >= GIVE_UP_AFTER {
                        self.set_agent(AgentState::Failed, Some(ended));
                        return;
                    }
                }
                Err(detail) => {
                    failures += 1;
                    if failures >= GIVE_UP_AFTER {
                        self.set_agent(AgentState::Failed, Some(detail));
                        return;
                    }
                    self.set_agent(AgentState::Starting, Some(detail));
                }
            }

            tokio::time::sleep(backoff(failures)).await;
        }
    }

    async fn run_once(&self, binary: &Path) -> Result<String, String> {
        let mut child = tokio::process::Command::new(binary)
            .arg("--secret-path")
            .arg(self.secret_path())
            .arg("--socket-path")
            .arg(self.socket_path())
            .current_dir(&self.dir)
            .env("TOKIO_WORKER_THREADS", WORKER_THREADS)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|err| format!("the tunnel daemon would not start: {err}"))?;

        let last = Arc::new(Mutex::new(String::new()));
        for stream in [
            child.stdout.take().map(|out| Box::new(out) as Box<dyn tokio::io::AsyncRead + Send + Unpin>),
            child.stderr.take().map(|err| Box::new(err) as Box<dyn tokio::io::AsyncRead + Send + Unpin>),
        ]
        .into_iter()
        .flatten()
        {
            let last = Arc::clone(&last);
            tokio::spawn(async move {
                let mut lines = BufReader::new(stream).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    let line = plain(&line);
                    if line.is_empty() {
                        continue;
                    }
                    tracing::debug!(target: "playitd", "{line}");
                    *last.lock().expect("the daemon output lock") = line;
                }
            });
        }

        self.set_agent(AgentState::Running, None);
        let status = child
            .wait()
            .await
            .map_err(|err| format!("the tunnel daemon could not be waited on: {err}"))?;

        let said = last.lock().expect("the daemon output lock").clone();
        Ok(if said.is_empty() {
            format!("the tunnel daemon ended with {status}")
        } else {
            said
        })
    }

    fn set_agent(&self, state: AgentState, detail: Option<String>) {
        let mut status = self.status.lock().expect("the agent status lock");
        status.state = state;
        status.version = matches!(state, AgentState::Running).then(|| RELEASE.to_owned());
        status.detail = detail;
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Broken {
    Unreachable(String),
    Damaged(String),
}

impl std::fmt::Display for Broken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unreachable(detail) | Self::Damaged(detail) => f.write_str(detail),
        }
    }
}

fn for_this_machine() -> Option<&'static Build> {
    BINARIES.iter().find(|binary| binary.arch == std::env::consts::ARCH)
}

fn plain(line: &str) -> String {
    let mut kept = String::with_capacity(line.len());
    let mut rest = line.chars();

    while let Some(letter) = rest.next() {
        if letter != '\u{1b}' {
            kept.push(letter);
            continue;
        }
        for end in rest.by_ref() {
            if end.is_ascii_alphabetic() {
                break;
            }
        }
    }
    kept.trim().to_owned()
}

fn backoff(failures: u32) -> Duration {
    BACKOFF_CEILING.min(Duration::from_secs(1 << failures.clamp(1, 7) - 1))
}

async fn digest_of(path: &Path) -> Option<String> {
    let bytes = tokio::fs::read(path).await.ok()?;
    Some(hex::encode(sha2::Sha256::digest(&bytes)))
}

async fn set_mode(path: &Path, mode: u32) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    tokio::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Id;

    const RELEASE_JSON: &[u8] = include_bytes!("testdata/github_release_v1_0_10.json");

    const PRETEND: &[u8] = b"pretend this is playitd";
    static PRETEND_BUILD: Build = Build {
        arch: "testarch",
        asset: "playit-linux-amd64",
        sha256: "d381349639b163ed5c221e53d91af3922b0956270e3ebdc41f2428efb260fb49",
    };

    #[derive(serde::Deserialize)]
    struct Release {
        tag_name: String,
        assets: Vec<Asset>,
    }

    #[derive(serde::Deserialize)]
    struct Asset {
        name: String,
        digest: String,
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("craftpanel-playit-{name}-{}", Id::new()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn a_binary(dir: &Path) -> Arc<Binary> {
        Binary::new(dir).unwrap()
    }

    #[test]
    fn every_built_in_checksum_is_the_one_github_published_for_this_release() {
        let release: Release = serde_json::from_slice(RELEASE_JSON).unwrap();
        assert_eq!(release.tag_name, RELEASE);

        for binary in BINARIES {
            let asset = release
                .assets
                .iter()
                .find(|asset| asset.name == binary.asset)
                .unwrap_or_else(|| panic!("{} is not in the release", binary.asset));
            assert_eq!(
                asset.digest,
                format!("sha256:{}", binary.sha256),
                "{} has drifted",
                binary.asset
            );
        }
    }

    #[test]
    fn the_table_names_the_daemon_and_not_the_command_line_tool() {
        for binary in BINARIES {
            assert!(binary.asset.starts_with("playit-linux-"), "{}", binary.asset);
            assert_eq!(binary.sha256.len(), 64);
        }
        assert_eq!(BINARIES.len(), 4);
        assert!(
            for_this_machine().is_some(),
            "no daemon for {} — the test machine cannot run one either",
            std::env::consts::ARCH
        );
    }

    #[test]
    fn the_wait_between_restarts_grows_and_then_stops_growing() {
        let waits: Vec<u64> = (1..=9).map(|n| backoff(n).as_secs()).collect();
        assert_eq!(waits, [1, 2, 4, 8, 16, 32, 60, 60, 60]);
        assert!(waits.iter().all(|wait| *wait <= BACKOFF_CEILING.as_secs()));

        assert_eq!(backoff(GIVE_UP_AFTER - 1).as_secs(), 8);
    }

    #[test]
    fn the_daemons_own_words_arrive_without_their_colours() {
        let said = "\u{1b}[2m2026-08-13T02:03:53.100172Z\u{1b}[0m \u{1b}[33m WARN\u{1b}[0m \
                    \u{1b}[2mplayitd::daemon\u{1b}[0m\u{1b}[2m:\u{1b}[0m configured agent \
                    secret is no longer valid";

        let line = plain(said);
        assert!(!line.contains('\u{1b}'), "{line}");
        assert!(line.ends_with("configured agent secret is no longer valid"), "{line}");
        assert!(line.starts_with("2026-08-13T02:03:53.100172Z  WARN"), "{line}");

        assert_eq!(plain("  plain and short  "), "plain and short");
        assert_eq!(plain("\u{1b}[0m"), "");
    }

    #[tokio::test]
    async fn the_key_is_written_readable_only_by_us_and_read_back_whole() {
        use std::os::unix::fs::PermissionsExt;

        let dir = scratch("key");
        let agent = Agent::new(dir.join("playit").join("anna"), a_binary(&dir));
        let secret = Secret::parse("deadbeefcafe").unwrap();

        agent.write_secret(&secret).await.unwrap();

        let mode = std::fs::metadata(agent.secret_path()).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "the key is a secret, not a public file");
        assert_eq!(std::fs::metadata(agent.dir()).unwrap().permissions().mode() & 0o777, 0o700);

        let read = agent.read_secret().await.expect("the key comes back");
        assert_eq!(read.expose(), "deadbeefcafe");

        agent.forget_secret().await.unwrap();
        assert!(agent.read_secret().await.is_none());
        agent.forget_secret().await.expect("forgetting twice is not an error");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn two_users_have_two_directories_and_neither_can_see_the_others_key() {
        let dir = scratch("two-keys");
        let binary = a_binary(&dir);
        let anna = Agent::new(dir.join("playit").join("anna"), Arc::clone(&binary));
        let ben = Agent::new(dir.join("playit").join("ben"), binary);

        assert_ne!(anna.secret_path(), ben.secret_path());
        anna.write_secret(&Secret::parse("aaaaaaaa").unwrap()).await.unwrap();

        assert!(anna.secret_path().exists());
        assert!(!ben.secret_path().exists(), "ben got anna's key");
        assert!(ben.read_secret().await.is_none());
        assert_ne!(anna.socket_path(), ben.socket_path(), "two daemons on one socket");

        ben.write_secret(&Secret::parse("bbbbbbbb").unwrap()).await.unwrap();
        assert_eq!(anna.read_secret().await.unwrap().expose(), "aaaaaaaa");
        assert_eq!(ben.read_secret().await.unwrap().expose(), "bbbbbbbb");

        ben.forget_everything().await.unwrap();
        assert!(!ben.dir().exists());
        assert!(anna.secret_path().exists());
        ben.forget_everything().await.expect("a directory that is gone is not an error");

        let _ = std::fs::remove_dir_all(&dir);
    }

    async fn downloads(count: usize, body: &'static [u8]) -> (String, Arc<Mutex<usize>>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let served = Arc::new(Mutex::new(0usize));

        let counted = Arc::clone(&served);
        tokio::spawn(async move {
            for _ in 0..count {
                let Ok((mut stream, _)) = listener.accept().await else { return };
                let mut head = [0u8; 1024];
                let _ = tokio::io::AsyncReadExt::read(&mut stream, &mut head).await;
                let _ = stream
                    .write_all(
                        format!("HTTP/1.1 200 OK\r\ncontent-length: {}\r\n\r\n", body.len())
                            .as_bytes(),
                    )
                    .await;
                let _ = stream.write_all(body).await;
                *counted.lock().expect("the counter") += 1;
            }
        });

        (base, served)
    }

    #[tokio::test]
    async fn a_daemon_that_hashes_wrong_is_thrown_away_and_not_fetched_again() {
        let dir = scratch("bad");
        let (source, _) = downloads(1, b"not the tunnel daemon").await;
        let binary =
            Binary::against(dir.clone(), source, Some(&PRETEND_BUILD)).unwrap();
        std::fs::create_dir_all(&dir).unwrap();

        let outcome = binary.ensure().await;
        let dest = dir.join(format!("playit-{RELEASE}-testarch"));

        match outcome {
            Err(Broken::Damaged(said)) => {
                assert!(said.contains(PRETEND_BUILD.sha256), "{said}");
                assert!(said.contains("hashes to"), "{said}");
            }
            other => panic!("wrong bytes were let through: {other:?}"),
        }
        assert!(!dest.exists(), "the file that hashes wrong stayed put");
        assert!(!dest.with_extension("part").exists(), "a half-download was left lying about");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn two_agents_share_one_program_file_and_fetch_it_once() {
        let dir = scratch("shared");
        let (source, served) = downloads(1, PRETEND).await;
        let binary = Binary::against(dir.clone(), source, Some(&PRETEND_BUILD)).unwrap();

        let one = Arc::clone(&binary);
        let two = Arc::clone(&binary);
        let (first, second) =
            tokio::join!(async move { one.ensure().await }, async move { two.ensure().await });

        let path = first.expect("the first fetch");
        assert_eq!(second.expect("the second caller"), path);
        assert_eq!(*served.lock().unwrap(), 1, "the file was fetched twice");
        assert_eq!(std::fs::read(&path).unwrap(), PRETEND);
        assert_eq!(binary.status().state, BinaryState::Ready);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn bytes_that_hash_right_are_kept_and_answered_from_the_cache() {
        let dir = scratch("good");
        let (source, served) = downloads(1, PRETEND).await;
        let binary = Binary::against(dir.clone(), source, Some(&PRETEND_BUILD)).unwrap();

        let path = binary.ensure().await.expect("the digest matches");
        assert_eq!(std::fs::read(&path).unwrap(), PRETEND);
        assert_eq!(digest_of(&path).await.as_deref(), Some(PRETEND_BUILD.sha256));

        assert_eq!(binary.ensure().await.unwrap(), path);
        assert_eq!(*served.lock().unwrap(), 1);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn an_architecture_playit_does_not_publish_for_is_not_worth_a_retry() {
        let dir = scratch("noarch");
        let binary = Binary::against(dir.clone(), "http://127.0.0.1:1".to_owned(), None).unwrap();

        match binary.ensure().await {
            Err(Broken::Damaged(said)) => assert!(said.contains("publishes no tunnel daemon")),
            other => panic!("{other:?}"),
        }

        let _ = std::fs::remove_dir_all(&dir);
    }
}
