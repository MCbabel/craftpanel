use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::Serialize;

use crate::auth::error::{Failure, Result};
use crate::loaders::{self, Channel, LoaderError, Sources};
use crate::model::{LoaderId, Timestamp};

const LIST_TTL: Duration = Duration::from_secs(30 * 60);
const MANIFEST_TTL: Duration = Duration::from_secs(10 * 60);
pub const MAX_BUILDS: usize = 500;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Vanilla,
    Server,
    Modloader,
    Proxy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InstallKind {
    Download,
    Installer,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Origin {
    Mojang,
    Papermc,
    Purpurmc,
    Leafmc,
    Fabricmc,
    Neoforged,
    Quiltmc,
    Minecraftforge,
}

impl Origin {
    fn service(self) -> &'static str {
        match self {
            Self::Mojang => "launchermeta.mojang.com",
            Self::Papermc => "fill.papermc.io",
            Self::Purpurmc => "api.purpurmc.org",
            Self::Leafmc => "api.leafmc.one",
            Self::Fabricmc => "meta.fabricmc.net",
            Self::Neoforged => "maven.neoforged.net",
            Self::Quiltmc => "meta.quiltmc.org",
            Self::Minecraftforge => "files.minecraftforge.net",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LoaderInfo {
    pub id: LoaderId,
    pub name: &'static str,
    pub kind: Kind,
    pub install_kind: InstallKind,
    pub has_loader_versions: bool,
    pub supports_properties: bool,
    pub supports_content: bool,
    pub source: Origin,
    pub wave: u8,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LoaderList {
    pub loaders: &'static [LoaderInfo],
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GameVersionEntry {
    pub version: String,
    pub version_type: VersionType,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum VersionType {
    Release,
    Snapshot,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GameVersionList {
    pub loader: LoaderId,
    pub game_versions: Vec<GameVersionEntry>,
    pub cached_until: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LoaderBuild {
    pub id: String,
    pub label: String,
    pub stable: bool,
    pub channel_tag: Option<&'static str>,
    pub released: Option<Timestamp>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LoaderBuildList {
    pub loader: LoaderId,
    pub game_version: String,
    pub builds: Vec<LoaderBuild>,
    pub truncated: bool,
    pub cached_until: Timestamp,
}

macro_rules! loader {
    ($id:ident, $name:literal, $kind:ident, $install:ident, $versions:literal, $props:literal,
     $source:ident, $wave:literal) => {
        LoaderInfo {
            id: LoaderId::$id,
            name: $name,
            kind: Kind::$kind,
            install_kind: InstallKind::$install,
            has_loader_versions: $versions,
            supports_properties: $props,
            supports_content: true,
            source: Origin::$source,
            wave: $wave,
        }
    };
}

pub const LOADERS: &[LoaderInfo] = &[
    loader!(Vanilla, "Vanilla", Vanilla, Download, false, true, Mojang, 1),
    loader!(Paper, "Paper", Server, Download, true, true, Papermc, 1),
    loader!(Folia, "Folia", Server, Download, true, true, Papermc, 1),
    loader!(Purpur, "Purpur", Server, Download, true, true, Purpurmc, 1),
    loader!(Leaf, "Leaf", Server, Download, true, true, Leafmc, 1),
    loader!(Fabric, "Fabric", Modloader, Download, true, true, Fabricmc, 1),
    loader!(Velocity, "Velocity", Proxy, Download, true, false, Papermc, 1),
    loader!(NeoForge, "NeoForge", Modloader, Installer, true, true, Neoforged, 2),
    loader!(Quilt, "Quilt", Modloader, Installer, true, true, Quiltmc, 2),
    loader!(Forge, "Forge", Modloader, Installer, true, true, Minecraftforge, 2),
];

pub fn info(id: LoaderId) -> &'static LoaderInfo {
    LOADERS.iter().find(|entry| entry.id == id).expect("the catalogue holds all ten")
}

struct Cached<T> {
    at: Instant,
    until: Timestamp,
    value: T,
}

pub struct Catalog {
    sources: Sources,
    versions: Mutex<HashMap<LoaderId, Cached<Vec<GameVersionEntry>>>>,
    builds: Mutex<HashMap<(LoaderId, String), Cached<Vec<LoaderBuild>>>>,
}

impl Catalog {
    pub fn new() -> std::result::Result<Self, LoaderError> {
        Ok(Self::with(Sources::new()?))
    }

    pub fn with(sources: Sources) -> Self {
        Self { sources, versions: Mutex::default(), builds: Mutex::default() }
    }

    pub async fn game_versions(&self, id: LoaderId) -> Result<GameVersionList> {
        if let Some(hit) = self.cached_versions(id) {
            return Ok(GameVersionList { loader: id, game_versions: hit.0, cached_until: hit.1 });
        }

        let source = wired(id)?;
        let fetched = self.sources.versions(source).await.map_err(|err| upstream(id, err))?;
        let entries: Vec<GameVersionEntry> = fetched
            .into_iter()
            .map(|version| GameVersionEntry {
                version_type: version_type(id, &version.id, version.channel),
                version: version.id,
            })
            .collect();

        let until = self.remember_versions(id, entries.clone());
        Ok(GameVersionList { loader: id, game_versions: entries, cached_until: until })
    }

    pub async fn builds(
        &self,
        id: LoaderId,
        game_version: &str,
        installed: &[String],
    ) -> Result<LoaderBuildList> {
        check_version_name(game_version)?;

        if id == LoaderId::Vanilla {
            return Ok(LoaderBuildList {
                loader: id,
                game_version: game_version.to_owned(),
                builds: Vec::new(),
                truncated: false,
                cached_until: expiry(MANIFEST_TTL),
            });
        }

        let (builds, until) = self.fetched_builds(id, game_version).await?;
        let (builds, truncated) = assemble(builds, installed);

        Ok(LoaderBuildList {
            loader: id,
            game_version: game_version.to_owned(),
            builds,
            truncated,
            cached_until: until,
        })
    }

    async fn fetched_builds(
        &self,
        id: LoaderId,
        game_version: &str,
    ) -> Result<(Vec<LoaderBuild>, Timestamp)> {
        let key = (id, game_version.to_owned());
        if let Some(hit) = self.cached_builds(&key) {
            return Ok(hit);
        }

        let source = wired(id)?;
        let fetched =
            self.sources.builds(source, game_version).await.map_err(|err| upstream(id, err))?;
        let mapped: Vec<LoaderBuild> = fetched.into_iter().map(build_of).collect();
        let until = self.remember_builds(key, mapped.clone());
        Ok((mapped, until))
    }

    pub async fn knows_build(
        &self,
        id: LoaderId,
        game_version: &str,
        build: &str,
    ) -> Result<bool> {
        check_version_name(game_version)?;
        check_version_name(build)?;
        let (builds, _) = self.fetched_builds(id, game_version).await?;
        Ok(builds.iter().any(|entry| entry.id == build))
    }

    pub async fn has_stable_build(&self, id: LoaderId, game_version: &str) -> Result<bool> {
        check_version_name(game_version)?;
        let (builds, _) = self.fetched_builds(id, game_version).await?;
        Ok(builds.iter().any(|entry| entry.stable))
    }

    pub async fn resolve(
        &self,
        id: LoaderId,
        game_version: &str,
        build: Option<&str>,
    ) -> Result<loaders::Build> {
        check_version_name(game_version)?;
        if let Some(build) = build {
            check_version_name(build)?;
        }
        let source = wired(id)?;
        let wanted = match build {
            Some(build) => loaders::Wanted::Build(build.to_owned()),
            None => loaders::Wanted::LatestStable,
        };
        self.sources
            .resolve(source, game_version, &wanted)
            .await
            .map_err(|err| upstream(id, err))
    }

    pub fn sources(&self) -> &Sources {
        &self.sources
    }

    fn cached_versions(&self, id: LoaderId) -> Option<(Vec<GameVersionEntry>, Timestamp)> {
        let held = self.versions.lock().expect("the catalogue cache");
        let entry = held.get(&id)?;
        (entry.at.elapsed() < ttl(id)).then(|| (entry.value.clone(), entry.until))
    }

    fn remember_versions(&self, id: LoaderId, value: Vec<GameVersionEntry>) -> Timestamp {
        let until = expiry(ttl(id));
        self.versions
            .lock()
            .expect("the catalogue cache")
            .insert(id, Cached { at: Instant::now(), until, value });
        until
    }

    fn cached_builds(&self, key: &(LoaderId, String)) -> Option<(Vec<LoaderBuild>, Timestamp)> {
        let held = self.builds.lock().expect("the catalogue cache");
        let entry = held.get(key)?;
        (entry.at.elapsed() < LIST_TTL).then(|| (entry.value.clone(), entry.until))
    }

    fn remember_builds(&self, key: (LoaderId, String), value: Vec<LoaderBuild>) -> Timestamp {
        let until = expiry(LIST_TTL);
        self.builds
            .lock()
            .expect("the catalogue cache")
            .insert(key, Cached { at: Instant::now(), until, value });
        until
    }

    #[cfg(test)]
    fn seed_builds(&self, id: LoaderId, game_version: &str, value: Vec<LoaderBuild>) {
        self.remember_builds((id, game_version.to_owned()), value);
    }
}

fn ttl(id: LoaderId) -> Duration {
    if id == LoaderId::Vanilla {
        MANIFEST_TTL
    } else {
        LIST_TTL
    }
}

fn expiry(ttl: Duration) -> Timestamp {
    Timestamp::at(Timestamp::now().as_datetime() + ttl)
}

fn version_type(id: LoaderId, version: &str, channel: Channel) -> VersionType {
    let from_the_source = matches!(id, LoaderId::Vanilla | LoaderId::Fabric);
    if from_the_source {
        return match channel {
            Channel::Stable => VersionType::Release,
            Channel::Experimental => VersionType::Snapshot,
        };
    }

    let lowered = version.to_ascii_lowercase();
    let prerelease =
        ["-pre", "-rc", "snapshot", "-exp"].iter().any(|marker| lowered.contains(marker));
    if prerelease || channel == Channel::Experimental {
        VersionType::Snapshot
    } else {
        VersionType::Release
    }
}

fn check_version_name(name: &str) -> Result<()> {
    let ordinary = |letter: char| {
        letter.is_ascii_alphanumeric() || matches!(letter, '.' | '-' | '_' | '+')
    };
    if name.is_empty() || name.len() > 64 || !name.chars().all(ordinary) {
        return Err(Failure::new(
            axum::http::StatusCode::UNPROCESSABLE_ENTITY,
            "unsupported_game_version",
            format!("{name:?} is not the name of a version"),
        ));
    }
    Ok(())
}

fn wired(id: LoaderId) -> Result<loaders::Loader> {
    id.source().ok_or_else(|| {
        Failure::new(
            axum::http::StatusCode::BAD_GATEWAY,
            "upstream_unavailable",
            format!("{} is not wired up in this build yet", info(id).source.service()),
        )
    })
}

fn assemble(builds: Vec<LoaderBuild>, installed: &[String]) -> (Vec<LoaderBuild>, bool) {
    let runs_here = |id: &str| installed.iter().any(|wanted| wanted == id);

    let mut forgotten: Vec<LoaderBuild> = installed
        .iter()
        .filter(|wanted| !builds.iter().any(|known| &&known.id == wanted))
        .map(|id| kept_build(id))
        .collect();

    let over = forgotten.len() > MAX_BUILDS;
    forgotten.truncate(MAX_BUILDS);
    let room = MAX_BUILDS - forgotten.len();

    let offered = builds.len();
    let mut shown = builds;
    let mut late: Vec<LoaderBuild> = if shown.len() > room {
        shown.split_off(room).into_iter().filter(|build| runs_here(&build.id)).collect()
    } else {
        Vec::new()
    };
    late.truncate(room);

    shown.truncate(room - late.len());
    shown.extend(late);

    let truncated = over || shown.len() < offered;
    shown.extend(forgotten);

    (shown, truncated)
}

fn build_of(build: loaders::Build) -> LoaderBuild {
    let stable = build.channel == Channel::Stable;
    LoaderBuild {
        label: label_of(&build.id),
        id: build.id,
        stable,
        channel_tag: if stable { None } else { Some("ALPHA") },
        released: None,
    }
}

fn kept_build(id: &str) -> LoaderBuild {
    LoaderBuild {
        label: label_of(id),
        id: id.to_owned(),
        stable: true,
        channel_tag: None,
        released: None,
    }
}

fn label_of(id: &str) -> String {
    if id.chars().all(|letter| letter.is_ascii_digit()) {
        format!("Build {id}")
    } else {
        id.to_owned()
    }
}

fn upstream(id: LoaderId, err: LoaderError) -> Failure {
    match err {
        LoaderError::UnknownVersion { .. } => Failure::new(
            axum::http::StatusCode::UNPROCESSABLE_ENTITY,
            "unsupported_game_version",
            err.to_string(),
        ),
        LoaderError::UnknownBuild { .. } | LoaderError::NoBuild { .. } => {
            Failure::not_found("build_not_found", err.to_string())
        }
        other => Failure::new(
            axum::http::StatusCode::BAD_GATEWAY,
            "upstream_unavailable",
            format!("{}: {other}", info(id).source.service()),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::loaders::Http;

    fn catalog() -> Catalog {
        Catalog::with(Sources::with_http(Http::new().expect("a client")))
    }

    fn build(id: &str, stable: bool) -> LoaderBuild {
        LoaderBuild {
            label: label_of(id),
            id: id.to_owned(),
            stable,
            channel_tag: if stable { None } else { Some("ALPHA") },
            released: None,
        }
    }

    #[test]
    fn the_catalogue_is_ten_lower_case_ids_with_display_names_of_their_own() {
        assert_eq!(LOADERS.len(), 10);

        let ids: Vec<&str> = LOADERS.iter().map(|entry| entry.id.as_str()).collect();
        assert_eq!(
            ids,
            [
                "vanilla", "paper", "folia", "purpur", "leaf", "fabric", "velocity", "neoforge",
                "quilt", "forge"
            ]
        );
        assert!(ids.iter().all(|id| *id == id.to_lowercase()), "the layout lower-cases everything");
        assert_eq!(info(LoaderId::NeoForge).name, "NeoForge", "not what formatLoaderLabel makes");

        for entry in LOADERS {
            assert_eq!(entry.id.family() == crate::model::LoaderFamily::Proxy, !entry.supports_properties);
            assert_eq!(
                entry.wave == 2,
                entry.install_kind == InstallKind::Installer,
                "{} — the second wave is exactly the installers",
                entry.id
            );
        }
        assert!(!info(LoaderId::Vanilla).has_loader_versions, "vanilla has one axis");
        assert!(!info(LoaderId::Velocity).supports_properties, "a proxy reads no properties");
    }

    #[test]
    fn a_second_wave_loader_names_its_source_rather_than_pretending_to_be_empty() {
        let refusal = wired(LoaderId::NeoForge).unwrap_err();
        assert_eq!(refusal.code(), "upstream_unavailable");
        assert!(refusal.to_string().contains("maven.neoforged.net"), "{refusal}");

        assert!(wired(LoaderId::Paper).is_ok());
    }

    #[tokio::test]
    async fn the_installed_build_is_in_the_list_even_when_the_source_forgot_it() {
        let catalog = catalog();
        catalog.seed_builds(LoaderId::Paper, "1.21.8", vec![build("62", true), build("61", true)]);

        let answer = catalog
            .builds(LoaderId::Paper, "1.21.8", &["7".to_owned()])
            .await
            .unwrap();

        let ids: Vec<&str> = answer.builds.iter().map(|entry| entry.id.as_str()).collect();
        assert_eq!(ids, ["62", "61", "7"], "9.13: cancelEditing has to find it");
        assert!(!answer.truncated);

        let already = catalog.builds(LoaderId::Paper, "1.21.8", &["61".to_owned()]).await.unwrap();
        assert_eq!(already.builds.len(), 2, "and it is not put in twice");
    }

    #[tokio::test]
    async fn five_hundred_is_the_ceiling_and_the_answer_says_so() {
        let catalog = catalog();
        let many: Vec<LoaderBuild> =
            (0..600).rev().map(|number| build(&number.to_string(), true)).collect();
        catalog.seed_builds(LoaderId::Leaf, "1.21.8", many);

        let answer = catalog.builds(LoaderId::Leaf, "1.21.8", &[]).await.unwrap();
        assert_eq!(answer.builds.len(), MAX_BUILDS);
        assert!(answer.truncated);
        assert_eq!(answer.builds[0].id, "599", "newest first survives the cut");

        let with_old = catalog
            .builds(LoaderId::Leaf, "1.21.8", &["7".to_owned(), "8".to_owned()])
            .await
            .unwrap();
        assert_eq!(with_old.builds.len(), MAX_BUILDS, "the ceiling still holds");
        assert!(with_old.truncated);
        let last: Vec<&str> =
            with_old.builds[MAX_BUILDS - 2..].iter().map(|build| build.id.as_str()).collect();
        assert_eq!(last, ["8", "7"], "9.13: the cut cannot take away the installed build");
        assert_eq!(with_old.builds[0].id, "599", "and the newest still leads");
    }

    #[test]
    fn the_ceiling_of_five_hundred_holds_and_a_cut_is_always_reported() {
        let full: Vec<LoaderBuild> =
            (0..MAX_BUILDS).rev().map(|n| build(&n.to_string(), true)).collect();
        let (shown, truncated) = assemble(full, &["out-of-the-source".to_owned()]);
        assert_eq!(shown.len(), MAX_BUILDS);
        assert!(truncated, "one of the five hundred gave way for the installed build");
        assert_eq!(shown.last().unwrap().id, "out-of-the-source");

        let many: Vec<String> = (0..MAX_BUILDS + 100).map(|n| format!("gone-{n}")).collect();
        let (crowded, over) = assemble(vec![build("1", true)], &many);
        assert_eq!(crowded.len(), MAX_BUILDS, "the ceiling is not a suggestion");
        assert!(over);

        let (kept, quiet) = assemble(vec![build("2", true), build("1", true)], &["1".to_owned()]);
        assert_eq!(kept.len(), 2);
        assert!(!quiet);
    }

    #[test]
    fn a_release_candidate_is_a_snapshot_wherever_the_source_will_not_say_so() {
        for id in [LoaderId::Purpur, LoaderId::Leaf, LoaderId::Paper, LoaderId::Velocity] {
            assert_eq!(version_type(id, "1.21.9-rc1", Channel::Stable), VersionType::Snapshot, "{id}");
            assert_eq!(version_type(id, "1.21.8", Channel::Stable), VersionType::Release, "{id}");
            assert_eq!(
                version_type(id, "3.5.1-SNAPSHOT", Channel::Stable),
                VersionType::Snapshot,
                "{id}"
            );
        }

        assert_eq!(
            version_type(LoaderId::Vanilla, "24w14a", Channel::Experimental),
            VersionType::Snapshot
        );
        assert_eq!(version_type(LoaderId::Vanilla, "1.21.8", Channel::Stable), VersionType::Release);
        assert_eq!(version_type(LoaderId::Fabric, "24w14a", Channel::Stable), VersionType::Release);
    }

    #[test]
    fn a_version_name_is_a_version_name_and_not_a_piece_of_a_url() {
        for bad in [
            "../../velocity/versions/3.5.1",
            "1.21.8?x=y",
            "1.21.8#frag",
            "1.21.8/builds",
            "",
            &"1".repeat(65),
        ] {
            let refusal = check_version_name(bad).unwrap_err();
            assert_eq!(refusal.code(), "unsupported_game_version", "{bad:?}");
            assert_eq!(refusal.status(), axum::http::StatusCode::UNPROCESSABLE_ENTITY);
        }
        for good in ["1.21.8", "24w14a", "3.5.1-SNAPSHOT", "0.16.9", "1.21.9-pre2", "1.7.10_pre4"] {
            assert!(check_version_name(good).is_ok(), "{good}");
        }
    }

    #[tokio::test]
    async fn a_build_past_the_cut_is_still_a_build_the_panel_knows() {
        let catalog = catalog();
        let many: Vec<LoaderBuild> =
            (0..600).rev().map(|number| build(&number.to_string(), true)).collect();
        catalog.seed_builds(LoaderId::Paper, "1.16.5", many);

        let shown = catalog.builds(LoaderId::Paper, "1.16.5", &[]).await.unwrap();
        assert!(!shown.builds.iter().any(|entry| entry.id == "7"), "past the five hundred");

        assert!(catalog.knows_build(LoaderId::Paper, "1.16.5", "7").await.unwrap());
        assert!(!catalog.knows_build(LoaderId::Paper, "1.16.5", "601").await.unwrap());
    }

    #[tokio::test]
    async fn a_line_that_is_all_pre_releases_has_no_newest_stable_build() {
        let catalog = catalog();
        catalog.seed_builds(
            LoaderId::Paper,
            "1.21.5",
            vec![build("114", false), build("113", false)],
        );
        catalog.seed_builds(LoaderId::Paper, "1.21.8", vec![build("60", true)]);

        assert!(!catalog.has_stable_build(LoaderId::Paper, "1.21.5").await.unwrap());
        assert!(catalog.has_stable_build(LoaderId::Paper, "1.21.8").await.unwrap());
    }

    #[test]
    fn a_short_list_is_handed_over_as_it_came() {
        let many: Vec<LoaderBuild> = (0..3).rev().map(|n| build(&n.to_string(), true)).collect();
        let (shown, truncated) = assemble(many, &[]);

        let ids: Vec<&str> = shown.iter().map(|build| build.id.as_str()).collect();
        assert_eq!(ids, ["2", "1", "0"]);
        assert!(!truncated);
    }

    #[tokio::test]
    async fn a_cached_list_is_answered_without_asking_anybody() {
        let catalog = catalog();
        catalog.seed_builds(LoaderId::Purpur, "1.21.8", vec![build("2500", false)]);

        let answer = catalog.builds(LoaderId::Purpur, "1.21.8", &[]).await.unwrap();
        assert_eq!(answer.builds[0].channel_tag, Some("ALPHA"));
        assert!(!answer.builds[0].stable);
        assert!(answer.cached_until > Timestamp::now(), "9.13 puts the expiry in the answer");
    }


    #[test]
    fn a_number_is_a_build_and_a_loader_version_keeps_its_own_name() {
        assert_eq!(label_of("60"), "Build 60");
        assert_eq!(label_of("0.16.9"), "0.16.9");
        assert_eq!(label_of("3.5.1-SNAPSHOT"), "3.5.1-SNAPSHOT");
    }

    #[test]
    fn an_upstream_that_is_merely_unhappy_is_not_a_missing_build() {
        let unknown = upstream(
            LoaderId::Paper,
            LoaderError::UnknownVersion { loader: "Paper", version: "1.0".to_owned() },
        );
        assert_eq!(unknown.code(), "unsupported_game_version");
        assert_eq!(unknown.status(), axum::http::StatusCode::UNPROCESSABLE_ENTITY);

        let missing = upstream(
            LoaderId::Paper,
            LoaderError::UnknownBuild {
                loader: "Paper",
                version: "1.21.8".to_owned(),
                build: "9".to_owned(),
            },
        );
        assert_eq!(missing.code(), "build_not_found");

        let down = upstream(
            LoaderId::Purpur,
            LoaderError::Unreachable { service: "Purpur", reason: "timeout".to_owned() },
        );
        assert_eq!(down.code(), "upstream_unavailable");
        assert_eq!(down.status(), axum::http::StatusCode::BAD_GATEWAY);
        assert!(down.to_string().contains("api.purpurmc.org"), "the message names the source");
    }
}
