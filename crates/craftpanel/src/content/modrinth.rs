use std::collections::BTreeSet;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures::StreamExt;
use serde::{Deserialize, Serialize};
use sha1::Sha1;
use sha2::{Digest, Sha512};

use sqlx::SqlitePool;
use tokio::io::AsyncWriteExt;
use tokio::sync::Mutex;

use crate::model::{ModrinthOwner, ModrinthOwnerKind, Timestamp};

pub const AGENT: &str = concat!(
    "craftpanel/",
    env!("CARGO_PKG_VERSION"),
    " (+https://github.com/MCbabel/MinecraftServerManager)"
);

pub const BASE: &str = "https://api.modrinth.com";

const REQUESTS_PER_MINUTE: f64 = 300.0;
const BACKGROUND_PER_MINUTE: f64 = 60.0;
const VERSIONS_TTL: Duration = Duration::from_secs(6 * 60 * 60);
const VERSION_TTL: Duration = Duration::from_secs(24 * 60 * 60);
const PROJECT_TTL: Duration = Duration::from_secs(24 * 60 * 60);
const OWNER_TTL: Duration = Duration::from_secs(7 * 24 * 60 * 60);
const TAGS_TTL: Duration = Duration::from_secs(24 * 60 * 60);

const PROJECT_BITE: usize = 100;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const READ_TIMEOUT: Duration = Duration::from_secs(30);
const RETRY_ON: [u16; 6] = [408, 429, 500, 502, 503, 504];
const ATTEMPTS: u32 = 4;

#[derive(Debug, thiserror::Error)]
pub enum Upstream {
    #[error("Modrinth is rate limiting us")]
    RateLimited,
    #[error("Modrinth did not answer: {0}")]
    Unavailable(String),
    #[error("Modrinth answered something we cannot read: {0}")]
    Unreadable(String),
    #[error("{0} is not on Modrinth")]
    NotFound(String),
    #[error("the file does not match the checksum Modrinth published for it")]
    Damaged,
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Database(#[from] sqlx::Error),
}

pub type Result<T> = std::result::Result<T, Upstream>;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MrHashes {
    #[serde(default)]
    pub sha1: Option<String>,
    #[serde(default)]
    pub sha512: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MrFile {
    pub hashes: MrHashes,
    pub url: String,
    pub filename: String,
    #[serde(default)]
    pub primary: bool,
    #[serde(default)]
    pub size: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MrDependency {
    #[serde(default)]
    pub project_id: Option<String>,
    #[serde(default)]
    pub version_id: Option<String>,
    pub dependency_type: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MrVersion {
    pub id: String,
    pub project_id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub version_number: String,
    #[serde(default = "release")]
    pub version_type: String,
    #[serde(default)]
    pub game_versions: Vec<String>,
    #[serde(default)]
    pub loaders: Vec<String>,
    #[serde(default)]
    pub date_published: Option<String>,
    #[serde(default)]
    pub dependencies: Vec<MrDependency>,
    #[serde(default)]
    pub files: Vec<MrFile>,
}

fn release() -> String {
    "release".to_owned()
}

impl MrVersion {
    pub fn primary_file(&self) -> Option<&MrFile> {
        self.files.iter().find(|file| file.primary).or_else(|| self.files.first())
    }

    pub fn published(&self) -> Option<Timestamp> {
        self.date_published.as_deref().and_then(|text| text.parse().ok())
    }

    pub fn requires(&self) -> impl Iterator<Item = &str> {
        self.dependencies
            .iter()
            .filter(|dependency| dependency.dependency_type == "required")
            .filter_map(|dependency| dependency.project_id.as_deref())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MrProject {
    pub id: String,
    #[serde(default)]
    pub slug: Option<String>,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub icon_url: Option<String>,
    #[serde(default)]
    pub project_type: Option<String>,
    #[serde(default)]
    pub downloads: Option<u64>,
    #[serde(default)]
    pub followers: Option<u64>,
    #[serde(default)]
    pub team: Option<String>,
    #[serde(default, deserialize_with = "one_environment")]
    pub environment: Option<String>,
    #[serde(default)]
    pub categories: Vec<String>,
}

fn one_environment<'de, D>(deserializer: D) -> std::result::Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum OneOrMany {
        One(String),
        Many(Vec<String>),
        Nothing,
    }

    Ok(match OneOrMany::deserialize(deserializer)? {
        OneOrMany::One(value) => Some(value),
        OneOrMany::Many(values) => values.into_iter().next(),
        OneOrMany::Nothing => None,
    })
}

#[derive(Debug, Deserialize)]
struct GameVersionTag {
    version: String,
}

#[derive(Debug, Deserialize)]
struct TeamMember {
    user: TeamUser,
    #[serde(default)]
    role: String,
    #[serde(default)]
    is_owner: Option<bool>,
    #[serde(default)]
    organization_permissions: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct TeamUser {
    id: String,
    username: String,
    #[serde(default)]
    avatar_url: Option<String>,
}

pub struct Passthrough {
    pub status: u16,
    pub body: Vec<u8>,
    pub content_type: Option<String>,
}

pub struct Modrinth {
    http: reqwest::Client,
    base: String,
    pool: SqlitePool,
    bucket: Arc<Mutex<Bucket>>,
    background: Arc<Mutex<Bucket>>,
    tags: Arc<Mutex<Option<(Instant, Arc<[String]>)>>>,
    backoff: Duration,
}

impl Modrinth {
    pub fn new(pool: SqlitePool) -> Result<Self> {
        Self::with_base(pool, BASE)
    }

    pub fn with_base(pool: SqlitePool, base: impl Into<String>) -> Result<Self> {
        let http = reqwest::Client::builder()
            .user_agent(AGENT)
            .connect_timeout(CONNECT_TIMEOUT)
            .read_timeout(READ_TIMEOUT)
            .build()
            .map_err(|err| Upstream::Unavailable(err.to_string()))?;
        Ok(Self {
            http,
            base: base.into().trim_end_matches('/').to_owned(),
            pool,
            bucket: Arc::new(Mutex::new(Bucket::new(REQUESTS_PER_MINUTE))),
            background: Arc::new(Mutex::new(Bucket::new(BACKGROUND_PER_MINUTE))),
            tags: Arc::new(Mutex::new(None)),
            backoff: Duration::from_millis(400),
        })
    }

    pub async fn game_versions(&self) -> Option<Arc<[String]>> {
        if let Some((fetched, known)) = self.tags.lock().await.as_ref() {
            if fetched.elapsed() < TAGS_TTL {
                return Some(Arc::clone(known));
            }
        }

        let answer = self.call("GET", "/v2/tag/game_version", None, None).await.ok()?;
        if answer.status != 200 {
            return None;
        }
        let tags: Vec<GameVersionTag> = parse(&answer.body).ok()?;
        let known: Arc<[String]> = tags.into_iter().map(|tag| tag.version).collect();
        *self.tags.lock().await = Some((Instant::now(), Arc::clone(&known)));
        Some(known)
    }

    pub async fn pace_background(&self) {
        self.background.lock().await.take().await;
    }

    pub fn with_backoff(mut self, first: Duration) -> Self {
        self.backoff = first;
        self
    }

    pub async fn allowed(&self) -> bool {
        sqlx::query_scalar::<_, bool>(
            "SELECT external_services_enabled FROM panel_settings WHERE id = 1",
        )
        .fetch_optional(&self.pool)
        .await
        .ok()
        .flatten()
        .unwrap_or(true)
    }

    pub async fn versions(&self, project_id: &str) -> Result<Vec<MrVersion>> {
        let cached: Option<(String, Option<String>, Timestamp)> = sqlx::query_as(
            "SELECT payload, etag, expires_at FROM modrinth_project_versions WHERE project_id = ?",
        )
        .bind(project_id)
        .fetch_optional(&self.pool)
        .await?;

        let now = Timestamp::now();
        if let Some((payload, _, expires_at)) = &cached {
            if *expires_at > now {
                return parse(payload.as_bytes());
            }
        }

        let etag = cached.as_ref().and_then(|(_, etag, _)| etag.clone());
        let fetched = self
            .call("GET", &format!("/v2/project/{project_id}/version"), None, etag.as_deref())
            .await;

        let (body, new_etag) = match fetched {
            Ok(answer) if answer.status == 304 => {
                let payload = cached.as_ref().map(|(payload, ..)| payload.clone());
                let Some(payload) = payload else {
                    return Err(Upstream::Unreadable("a 304 without anything cached".to_owned()));
                };
                (payload.into_bytes(), etag)
            }
            Ok(answer) if answer.status == 404 => {
                return Err(Upstream::NotFound(project_id.to_owned()))
            }
            Ok(answer) => (answer.body, answer.etag),
            Err(err) => match cached {
                Some((payload, ..)) => return parse(payload.as_bytes()),
                None => return Err(err),
            },
        };

        let versions: Vec<MrVersion> = parse(&body)?;
        let text = String::from_utf8_lossy(&body).into_owned();
        sqlx::query(
            "INSERT INTO modrinth_project_versions (project_id, payload, etag, fetched_at, expires_at)
             VALUES (?, ?, ?, ?, ?)
             ON CONFLICT (project_id) DO UPDATE
                SET payload = excluded.payload, etag = excluded.etag,
                    fetched_at = excluded.fetched_at, expires_at = excluded.expires_at",
        )
        .bind(project_id)
        .bind(&text)
        .bind(new_etag)
        .bind(now)
        .bind(Timestamp::at(now.as_datetime() + VERSIONS_TTL))
        .execute(&self.pool)
        .await?;

        Ok(versions)
    }

    pub async fn version(&self, version_id: &str) -> Result<MrVersion> {
        if let Some((payload,)) = sqlx::query_as::<_, (String,)>(
            "SELECT payload FROM modrinth_version WHERE version_id = ? AND expires_at > ?",
        )
        .bind(version_id)
        .bind(Timestamp::now())
        .fetch_optional(&self.pool)
        .await?
        {
            return parse(payload.as_bytes());
        }

        let answer = self.call("GET", &format!("/v2/version/{version_id}"), None, None).await?;
        if answer.status == 404 {
            return Err(Upstream::NotFound(version_id.to_owned()));
        }
        let version: MrVersion = parse(&answer.body)?;
        self.remember_version(&version).await?;
        Ok(version)
    }

    pub async fn remember_version(&self, version: &MrVersion) -> Result<()> {
        let now = Timestamp::now();
        sqlx::query(
            "INSERT INTO modrinth_version (version_id, project_id, payload, fetched_at, expires_at)
             VALUES (?, ?, ?, ?, ?)
             ON CONFLICT (version_id) DO UPDATE
                SET payload = excluded.payload, fetched_at = excluded.fetched_at,
                    expires_at = excluded.expires_at",
        )
        .bind(&version.id)
        .bind(&version.project_id)
        .bind(serde_json::to_string(version).unwrap_or_default())
        .bind(now)
        .bind(Timestamp::at(now.as_datetime() + VERSION_TTL))
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn cached_version(&self, version_id: &str) -> Result<Option<MrVersion>> {
        let row: Option<(String,)> =
            sqlx::query_as("SELECT payload FROM modrinth_version WHERE version_id = ?")
                .bind(version_id)
                .fetch_optional(&self.pool)
                .await?;
        row.map(|(payload,)| parse(payload.as_bytes())).transpose()
    }

    pub async fn project(&self, project_id: &str) -> Result<MrProject> {
        if let Some(project) = self.cached_project(project_id).await? {
            return Ok(project);
        }

        let answer = self.call("GET", &format!("/v2/project/{project_id}"), None, None).await?;
        if answer.status == 404 {
            return Err(Upstream::NotFound(project_id.to_owned()));
        }
        let project = one_project(parse(&answer.body)?)?;
        self.remember_project(&project).await?;
        Ok(project)
    }

    pub async fn remember_projects(&self, ids: &BTreeSet<String>) -> Result<()> {
        let mut wanted = Vec::new();
        for id in ids {
            if !self.project_is_fresh(id).await? {
                wanted.push(id.as_str());
            }
        }

        for bite in wanted.chunks(PROJECT_BITE) {
            let list = serde_json::to_string(bite).unwrap_or_default();
            let answer =
                self.call("GET", "/v2/projects", Some(&format!("ids={list}")), None).await?;
            if answer.status == 404 {
                continue;
            }
            for value in parse::<Vec<serde_json::Value>>(&answer.body)? {
                self.remember_project(&one_project(value)?).await?;
            }
        }
        Ok(())
    }

    async fn remember_project(&self, project: &MrProject) -> Result<()> {
        let now = Timestamp::now();
        sqlx::query(
            "INSERT INTO modrinth_project (project_id, slug, title, description, icon_url,
                                           project_type, downloads, followers, team, environment,
                                           fetched_at, expires_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT (project_id) DO UPDATE
                SET slug = excluded.slug, title = excluded.title,
                    description = excluded.description, icon_url = excluded.icon_url,
                    project_type = excluded.project_type, downloads = excluded.downloads,
                    followers = excluded.followers, team = excluded.team,
                    environment = excluded.environment, fetched_at = excluded.fetched_at,
                    expires_at = excluded.expires_at",
        )
        .bind(&project.id)
        .bind(&project.slug)
        .bind(&project.title)
        .bind(&project.description)
        .bind(&project.icon_url)
        .bind(&project.project_type)
        .bind(project.downloads.map(|n| n as i64))
        .bind(project.followers.map(|n| n as i64))
        .bind(&project.team)
        .bind(&project.environment)
        .bind(now)
        .bind(Timestamp::at(now.as_datetime() + PROJECT_TTL))
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    pub async fn cached_project(&self, project_id: &str) -> Result<Option<MrProject>> {
        let row: Option<(
            String,
            Option<String>,
            String,
            Option<String>,
            Option<String>,
            Option<String>,
            Option<i64>,
            Option<i64>,
            Option<String>,
            Option<String>,
            Timestamp,
        )> = sqlx::query_as(
            "SELECT project_id, slug, title, description, icon_url, project_type, downloads,
                    followers, team, environment, expires_at
               FROM modrinth_project WHERE project_id = ?",
        )
        .bind(project_id)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(|row| MrProject {
            id: row.0,
            slug: row.1,
            title: row.2,
            description: row.3,
            icon_url: row.4,
            project_type: row.5,
            downloads: row.6.map(|n| n as u64),
            followers: row.7.map(|n| n as u64),
            team: row.8,
            environment: row.9,
            categories: Vec::new(),
        }))
    }

    pub async fn project_is_fresh(&self, project_id: &str) -> Result<bool> {
        Ok(sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM modrinth_project WHERE project_id = ? AND expires_at > ?",
        )
        .bind(project_id)
        .bind(Timestamp::now())
        .fetch_one(&self.pool)
        .await?
            > 0)
    }

    pub async fn owner(&self, team_id: &str) -> Result<Option<ModrinthOwner>> {
        let cached: Option<(String, String, String, Option<String>, Timestamp)> = sqlx::query_as(
            "SELECT owner_id, name, kind, avatar_url, expires_at
               FROM modrinth_project_owner WHERE team_id = ?",
        )
        .bind(team_id)
        .fetch_optional(&self.pool)
        .await?;

        if let Some((id, name, kind, avatar_url, expires_at)) = &cached {
            if *expires_at > Timestamp::now() {
                return Ok(Some(ModrinthOwner {
                    id: id.clone(),
                    name: name.clone(),
                    kind: kind_of(kind),
                    avatar_url: avatar_url.clone(),
                }));
            }
        }

        let answer = self.call("GET", &format!("/v2/team/{team_id}/members"), None, None).await?;
        if answer.status == 404 {
            return Ok(None);
        }
        let members: Vec<TeamMember> = parse(&answer.body)?;
        let Some(member) = members
            .iter()
            .find(|member| member.is_owner == Some(true))
            .or_else(|| members.iter().find(|member| member.role.eq_ignore_ascii_case("owner")))
            .or(members.first())
        else {
            return Ok(None);
        };

        let owner = ModrinthOwner {
            id: member.user.id.clone(),
            name: member.user.username.clone(),
            kind: if member.organization_permissions.is_some() {
                ModrinthOwnerKind::Organization
            } else {
                ModrinthOwnerKind::User
            },
            avatar_url: member.user.avatar_url.clone(),
        };

        let now = Timestamp::now();
        sqlx::query(
            "INSERT INTO modrinth_project_owner (team_id, owner_id, name, kind, avatar_url,
                                                 fetched_at, expires_at)
             VALUES (?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT (team_id) DO UPDATE
                SET owner_id = excluded.owner_id, name = excluded.name, kind = excluded.kind,
                    avatar_url = excluded.avatar_url, fetched_at = excluded.fetched_at,
                    expires_at = excluded.expires_at",
        )
        .bind(team_id)
        .bind(&owner.id)
        .bind(&owner.name)
        .bind(owner.kind.as_str())
        .bind(&owner.avatar_url)
        .bind(now)
        .bind(Timestamp::at(now.as_datetime() + OWNER_TTL))
        .execute(&self.pool)
        .await?;

        Ok(Some(owner))
    }

    pub async fn passthrough(&self, path: &str, query: Option<&str>) -> Result<Passthrough> {
        let answer = self.call("GET", path, query, None).await?;
        Ok(Passthrough {
            status: answer.status,
            body: answer.body,
            content_type: answer.content_type,
        })
    }

    pub async fn download(
        &self,
        url: &str,
        dest: &Path,
        expected: &MrHashes,
    ) -> Result<u64> {
        self.bucket.lock().await.take().await;
        if let Some(parent) = dest.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        let response = self
            .http
            .get(url)
            .send()
            .await
            .map_err(|err| Upstream::Unavailable(err.to_string()))?;
        if response.status().as_u16() == 429 {
            return Err(Upstream::RateLimited);
        }
        if !response.status().is_success() {
            return Err(Upstream::Unavailable(format!("{url} answered {}", response.status())));
        }

        let mut file = tokio::fs::File::create(dest).await?;
        let mut sha1 = Sha1::default();
        let mut sha512 = Sha512::default();
        let mut written = 0u64;
        let mut stream = response.bytes_stream();

        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|err| Upstream::Unavailable(err.to_string()))?;
            sha1::Digest::update(&mut sha1, &chunk);
            Digest::update(&mut sha512, &chunk);
            file.write_all(&chunk).await?;
            written += chunk.len() as u64;
        }
        file.sync_all().await?;

        let matches = |expected: &Option<String>, actual: String| -> bool {
            expected.as_ref().is_none_or(|value| value.trim().eq_ignore_ascii_case(&actual))
        };
        let whole = matches(&expected.sha512, hex::encode(sha512.finalize()))
            && matches(&expected.sha1, hex::encode(sha1::Digest::finalize(sha1)));
        if !whole {
            let _ = tokio::fs::remove_file(dest).await;
            return Err(Upstream::Damaged);
        }

        Ok(written)
    }

    async fn call(
        &self,
        method: &str,
        path: &str,
        query: Option<&str>,
        etag: Option<&str>,
    ) -> Result<Answer> {
        let mut url = format!("{}{}", self.base, path);
        if let Some(query) = query.filter(|query| !query.is_empty()) {
            url.push('?');
            url.push_str(query);
        }

        let mut wait = self.backoff;
        let mut last = Upstream::Unavailable("no attempt was made".to_owned());

        for attempt in 0..ATTEMPTS {
            self.bucket.lock().await.take().await;

            let mut request = self.http.request(
                method.parse().unwrap_or(reqwest::Method::GET),
                &url,
            );
            if let Some(etag) = etag {
                request = request.header(reqwest::header::IF_NONE_MATCH, etag);
            }

            match request.send().await {
                Ok(response) => {
                    let status = response.status().as_u16();
                    if let Some(remaining) = header_number(&response, "x-ratelimit-remaining") {
                        self.bucket.lock().await.observe(remaining);
                    }
                    if RETRY_ON.contains(&status) && attempt + 1 < ATTEMPTS {
                        last = if status == 429 {
                            Upstream::RateLimited
                        } else {
                            Upstream::Unavailable(format!("Modrinth answered {status}"))
                        };
                        tokio::time::sleep(wait).await;
                        wait *= 3;
                        continue;
                    }
                    if status == 429 {
                        return Err(Upstream::RateLimited);
                    }
                    if !(200..300).contains(&status) && status != 304 && status != 404 {
                        return Err(Upstream::Unavailable(format!(
                            "Modrinth answered {status}"
                        )));
                    }

                    let content_type = response
                        .headers()
                        .get(reqwest::header::CONTENT_TYPE)
                        .and_then(|value| value.to_str().ok())
                        .map(str::to_owned);
                    let etag = response
                        .headers()
                        .get(reqwest::header::ETAG)
                        .and_then(|value| value.to_str().ok())
                        .map(str::to_owned);
                    let body = response
                        .bytes()
                        .await
                        .map_err(|err| Upstream::Unavailable(err.to_string()))?
                        .to_vec();
                    return Ok(Answer { status, body, etag, content_type });
                }
                Err(err) => {
                    last = Upstream::Unavailable(err.to_string());
                    if attempt + 1 == ATTEMPTS {
                        break;
                    }
                    tokio::time::sleep(wait).await;
                    wait *= 3;
                }
            }
        }

        Err(last)
    }
}

struct Answer {
    status: u16,
    body: Vec<u8>,
    etag: Option<String>,
    content_type: Option<String>,
}

fn header_number(response: &reqwest::Response, name: &str) -> Option<f64> {
    response.headers().get(name)?.to_str().ok()?.trim().parse().ok()
}

fn kind_of(text: &str) -> ModrinthOwnerKind {
    match text {
        "organization" => ModrinthOwnerKind::Organization,
        _ => ModrinthOwnerKind::User,
    }
}

fn one_project(value: serde_json::Value) -> Result<MrProject> {
    let folded = environment_of(&value);
    let mut project: MrProject =
        serde_json::from_value(value).map_err(|err| Upstream::Unreadable(err.to_string()))?;
    project.environment = project.environment.or(folded);
    Ok(project)
}

fn environment_of(value: &serde_json::Value) -> Option<String> {
    let client = value.get("client_side")?.as_str()?;
    let server = value.get("server_side")?.as_str()?;
    Some(match (client, server) {
        ("required" | "optional", "unsupported") => "client_only",
        ("unsupported", "required" | "optional") => "server_only",
        ("required", "required") => "client_and_server",
        _ => "singleplayer_only",
    })
    .filter(|_| !(client == "unknown" && server == "unknown"))
    .map(str::to_owned)
}

fn parse<T: serde::de::DeserializeOwned>(body: &[u8]) -> Result<T> {
    serde_json::from_slice(body).map_err(|err| Upstream::Unreadable(err.to_string()))
}

struct Bucket {
    tokens: f64,
    per_minute: f64,
    last: Instant,
}

impl Bucket {
    fn new(per_minute: f64) -> Self {
        Self { tokens: per_minute, per_minute, last: Instant::now() }
    }

    async fn take(&mut self) {
        loop {
            self.refill();
            if self.tokens >= 1.0 {
                self.tokens -= 1.0;
                return;
            }
            let missing = 1.0 - self.tokens;
            let seconds = missing * 60.0 / self.per_minute;
            tokio::time::sleep(Duration::from_secs_f64(seconds.min(60.0))).await;
        }
    }

    fn refill(&mut self) {
        let elapsed = self.last.elapsed().as_secs_f64();
        self.last = Instant::now();
        self.tokens = (self.tokens + elapsed * self.per_minute / 60.0).min(self.per_minute);
    }

    fn observe(&mut self, remaining: f64) {
        self.tokens = self.tokens.min(remaining.max(0.0));
    }
}

#[cfg(test)]
pub fn a_version(id: &str, project: &str, kind: &str, published: &str) -> MrVersion {
    MrVersion {
        id: id.to_owned(),
        project_id: project.to_owned(),
        name: id.to_owned(),
        version_number: id.to_owned(),
        version_type: kind.to_owned(),
        game_versions: vec!["1.21.1".to_owned()],
        loaders: vec!["fabric".to_owned()],
        date_published: Some(published.to_owned()),
        dependencies: Vec::new(),
        files: vec![MrFile {
            hashes: MrHashes { sha1: None, sha512: None },
            url: format!("https://cdn.invalid/{id}.jar"),
            filename: format!("{id}.jar"),
            primary: true,
            size: 1,
        }],
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::content::harness::{client, fake_modrinth, schema};

    #[test]
    fn the_agent_names_us_and_our_version() {
        assert!(AGENT.starts_with("craftpanel/0.1"), "{AGENT}");
        assert!(AGENT.contains("+https://"), "8.16 wants a way back to us: {AGENT}");
    }

    #[test]
    fn the_primary_file_is_the_one_the_loader_reads() {
        let mut version = a_version("aaa", "P1", "release", "2026-01-01T00:00:00Z");
        version.files.insert(
            0,
            MrFile {
                hashes: MrHashes { sha1: None, sha512: None },
                url: "https://cdn.invalid/sources.jar".to_owned(),
                filename: "sources.jar".to_owned(),
                primary: false,
                size: 1,
            },
        );
        assert_eq!(version.primary_file().expect("a file").filename, "aaa.jar");
    }

    #[test]
    fn the_environment_is_read_whether_it_arrives_as_a_word_or_as_a_list() {
        let list = br#"{"id":"P","environment":["client_or_server_prefers_both"]}"#;
        let project: MrProject = serde_json::from_slice(list).expect("the v2 shape");
        assert_eq!(project.environment.as_deref(), Some("client_or_server_prefers_both"));

        let word = br#"{"id":"P","environment":"client_only"}"#;
        let project: MrProject = serde_json::from_slice(word).expect("the v3 shape");
        assert_eq!(project.environment.as_deref(), Some("client_only"));

        let absent = br#"{"id":"P"}"#;
        let project: MrProject = serde_json::from_slice(absent).expect("neither");
        assert!(project.environment.is_none());
    }

    #[test]
    fn a_v2_answer_still_yields_the_environment_the_warning_triangle_reads() {
        let client_only = json!({ "client_side": "required", "server_side": "unsupported" });
        assert_eq!(environment_of(&client_only).as_deref(), Some("client_only"));
        let both = json!({ "client_side": "required", "server_side": "required" });
        assert_eq!(environment_of(&both).as_deref(), Some("client_and_server"));
        assert_eq!(environment_of(&json!({})), None);
    }

    #[tokio::test]
    async fn a_version_list_is_fetched_once_and_read_from_the_cache_after() {
        let pool = schema().await;
        let upstream = fake_modrinth().await;
        let modrinth = client(&pool, &upstream);

        let first = modrinth.versions("P1").await.expect("a list");
        let second = modrinth.versions("P1").await.expect("a list");
        assert_eq!(first.len(), second.len());
        assert_eq!(upstream.calls(), 1, "8.16: six hours, and the second read is free");
    }

    #[tokio::test]
    async fn a_page_of_projects_is_fetched_in_one_call_and_read_from_the_cache_after() {
        let pool = schema().await;
        let upstream = fake_modrinth().await;
        let modrinth = client(&pool, &upstream);
        upstream.set_project(
            "P2",
            json!({
                "id": "P2",
                "slug": "sodium",
                "title": "Sodium",
                "project_type": "mod",
                "client_side": "unsupported",
                "server_side": "required"
            }),
        );
        let wanted = BTreeSet::from(["P1".to_owned(), "P2".to_owned(), "P3".to_owned()]);

        modrinth.remember_projects(&wanted).await.expect("three projects");
        assert_eq!(upstream.calls(), 1, "8.16: forty mods must not cost forty calls");
        let cached = modrinth.cached_project("P2").await.expect("a read").expect("P2 is known");
        assert_eq!(cached.title, "Sodium");
        assert_eq!(cached.slug.as_deref(), Some("sodium"));
        assert_eq!(
            cached.environment.as_deref(),
            Some("server_only"),
            "a list answer folds out the two v2 fields like a single one does"
        );
        assert!(modrinth.cached_project("P3").await.expect("a read").is_some());

        modrinth.remember_projects(&wanted).await.expect("nothing left to ask for");
        assert_eq!(upstream.calls(), 1, "what is fresh is not fetched twice");

        sqlx::query("UPDATE modrinth_project SET expires_at = '2000-01-01T00:00:00Z'")
            .execute(&pool)
            .await
            .expect("an expired cache");
        modrinth.remember_projects(&wanted).await.expect("a refresh");
        assert_eq!(upstream.calls(), 2, "a day old is asked again — and again in one call");
    }

    #[tokio::test]
    async fn a_project_modrinth_no_longer_has_leaves_the_rest_of_the_page_alone() {
        let pool = schema().await;
        let upstream = fake_modrinth().await;
        let modrinth = client(&pool, &upstream);
        let gone = crate::content::harness::UNKNOWN;
        let wanted = BTreeSet::from([gone.to_owned(), "P1".to_owned()]);

        modrinth.remember_projects(&wanted).await.expect("the one that is left");
        assert_eq!(upstream.calls(), 1);
        assert!(modrinth.cached_project("P1").await.expect("a read").is_some());
        assert!(modrinth.cached_project(gone).await.expect("a read").is_none());
    }

    #[tokio::test]
    async fn a_stale_list_is_revalidated_with_if_none_match_and_the_304_costs_nothing() {
        let pool = schema().await;
        let upstream = fake_modrinth().await;
        let modrinth = client(&pool, &upstream);

        modrinth.versions("P1").await.expect("a list");
        sqlx::query("UPDATE modrinth_project_versions SET expires_at = '2000-01-01T00:00:00Z'")
            .execute(&pool)
            .await
            .expect("an expired row");

        let again = modrinth.versions("P1").await.expect("a list");
        assert_eq!(again.len(), 2);
        assert_eq!(upstream.calls(), 2);
        assert_eq!(upstream.conditional(), 1, "the second call carried the etag");
    }

    #[tokio::test]
    async fn a_source_that_falls_over_is_answered_from_the_stale_copy() {
        let pool = schema().await;
        let upstream = fake_modrinth().await;
        let modrinth = client(&pool, &upstream);
        modrinth.versions("P1").await.expect("a list");

        sqlx::query("UPDATE modrinth_project_versions SET expires_at = '2000-01-01T00:00:00Z'")
            .execute(&pool)
            .await
            .expect("an expired row");
        upstream.break_down();

        let stale = modrinth.versions("P1").await.expect("the page still opens");
        assert_eq!(stale.len(), 2);
    }

    #[tokio::test]
    async fn a_500_is_retried_and_a_429_is_told_apart_from_a_dead_service() {
        let pool = schema().await;
        let upstream = fake_modrinth().await;
        let modrinth = client(&pool, &upstream);

        upstream.answer_with(500, 2);
        let versions = modrinth.versions("P1").await.expect("the third attempt lands");
        assert_eq!(versions.len(), 2);
        assert_eq!(upstream.calls(), 3);

        upstream.answer_with(429, 99);
        let refusal = modrinth.versions("P2").await.expect_err("a rate limit");
        assert!(matches!(refusal, Upstream::RateLimited), "{refusal:?}");
    }

    #[tokio::test]
    async fn a_download_whose_checksum_is_wrong_leaves_no_file_behind() {
        let pool = schema().await;
        let upstream = fake_modrinth().await;
        let modrinth = client(&pool, &upstream);
        let dir = std::env::temp_dir().join(format!("craftpanel-dl-{}", crate::model::Id::new()));
        let dest = dir.join("mod.jar");

        let wrong = MrHashes { sha1: None, sha512: Some("00".repeat(64)) };
        let err = modrinth
            .download(&format!("{}/file", upstream.base()), &dest, &wrong)
            .await
            .expect_err("a mismatch");
        assert!(matches!(err, Upstream::Damaged), "{err:?}");
        assert!(!dest.exists(), "a damaged download must not look like a finished one");

        let right = MrHashes {
            sha1: None,
            sha512: Some(hex::encode(Sha512::digest(crate::content::harness::FILE_BODY))),
        };
        let written = modrinth
            .download(&format!("{}/file", upstream.base()), &dest, &right)
            .await
            .expect("the file lands");
        assert_eq!(written, crate::content::harness::FILE_BODY.len() as u64);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn a_background_run_spends_its_own_budget_and_not_the_one_the_page_waits_on() {
        let pool = schema().await;
        let modrinth = Modrinth::new(pool).expect("a client");

        for _ in 0..BACKGROUND_PER_MINUTE as usize {
            modrinth.pace_background().await;
        }

        assert!(modrinth.background.lock().await.tokens < 1.0, "a minute's worth is a minute's");
        assert!(
            modrinth.bucket.lock().await.tokens > REQUESTS_PER_MINUTE - 1.0,
            "whoever is at the screen still has the whole bucket"
        );
    }

    #[tokio::test]
    async fn the_bucket_hands_out_what_modrinth_says_is_left() {
        let mut bucket = Bucket::new(600.0);
        bucket.take().await;
        bucket.observe(0.0);
        assert!(bucket.tokens < 1.0, "a spent budget is a spent budget");

        let started = Instant::now();
        bucket.take().await;
        assert!(started.elapsed() >= Duration::from_millis(50), "it waited instead of racing on");
    }

    #[tokio::test]
    #[ignore = "talks to api.modrinth.com"]
    async fn live_fabric_api_has_versions_a_project_and_an_owner() {
        let pool = schema().await;
        let modrinth = Modrinth::new(pool).expect("a client");

        let versions = modrinth.versions("P7dR8mSH").await.expect("Fabric API has versions");
        assert!(versions.len() > 10);
        assert!(versions.iter().all(|version| version.project_id == "P7dR8mSH"));

        let project = modrinth.project("P7dR8mSH").await.expect("the project");
        assert_eq!(project.slug.as_deref(), Some("fabric-api"));
        let team = project.team.clone().expect("a team");
        assert!(modrinth.owner(&team).await.expect("an owner").is_some());
    }
}
