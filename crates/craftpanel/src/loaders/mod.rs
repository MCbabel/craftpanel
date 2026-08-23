#![allow(dead_code)]

pub(crate) mod checksum;
mod error;
mod fabric;
mod fill;
pub(crate) mod http;
mod leaf;
mod purpur;
mod vanilla;

use std::fmt;
use std::path::Path;

use serde::{Deserialize, Serialize};

pub use checksum::{Algorithm, Checksum};
pub use error::{LoaderError, Result};
pub use http::Http;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Loader {
    Vanilla,
    Paper,
    Folia,
    Purpur,
    Leaf,
    Fabric,
    Velocity,
}

impl Loader {
    pub const ALL: [Self; 7] = [
        Self::Vanilla,
        Self::Paper,
        Self::Folia,
        Self::Purpur,
        Self::Leaf,
        Self::Fabric,
        Self::Velocity,
    ];

    pub fn id(self) -> &'static str {
        match self {
            Self::Vanilla => "vanilla",
            Self::Paper => "paper",
            Self::Folia => "folia",
            Self::Purpur => "purpur",
            Self::Leaf => "leaf",
            Self::Fabric => "fabric",
            Self::Velocity => "velocity",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Vanilla => "Vanilla",
            Self::Paper => "Paper",
            Self::Folia => "Folia",
            Self::Purpur => "Purpur",
            Self::Leaf => "Leaf",
            Self::Fabric => "Fabric",
            Self::Velocity => "Velocity",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|loader| loader.id() == id)
    }
}

impl fmt::Display for Loader {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Channel {
    Stable,
    Experimental,
}

impl Channel {
    fn name(self) -> &'static str {
        match self {
            Self::Stable => "stable",
            Self::Experimental => "experimental",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Version {
    pub id: String,
    pub channel: Channel,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Build {
    pub id: String,
    pub channel: Channel,
    pub url: String,
    pub filename: String,
    pub checksum: Option<Checksum>,
    pub size: Option<u64>,
    pub java_major: Option<u32>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Wanted {
    #[default]
    LatestStable,
    Latest,
    Build(String),
}

pub struct Sources {
    http: Http,
}

impl Sources {
    pub fn new() -> Result<Self> {
        Ok(Self { http: Http::new()? })
    }

    pub fn with_http(http: Http) -> Self {
        Self { http }
    }

    pub async fn versions(&self, loader: Loader) -> Result<Vec<Version>> {
        match loader {
            Loader::Vanilla => vanilla::versions(&self.http).await,
            Loader::Paper | Loader::Folia | Loader::Velocity => {
                fill::versions(&self.http, loader).await
            }
            Loader::Purpur => purpur::versions(&self.http).await,
            Loader::Leaf => leaf::versions(&self.http).await,
            Loader::Fabric => fabric::versions(&self.http).await,
        }
    }

    pub async fn builds(&self, loader: Loader, version: &str) -> Result<Vec<Build>> {
        match loader {
            Loader::Vanilla => vanilla::builds(&self.http, version).await,
            Loader::Paper | Loader::Folia | Loader::Velocity => {
                fill::builds(&self.http, loader, version).await
            }
            Loader::Purpur => purpur::builds(&self.http, version).await,
            Loader::Leaf => leaf::builds(&self.http, version).await,
            Loader::Fabric => fabric::builds(&self.http, version).await,
        }
    }

    pub async fn resolve(&self, loader: Loader, version: &str, wanted: &Wanted) -> Result<Build> {
        if loader == Loader::Purpur {
            return purpur::resolve(&self.http, version, wanted).await;
        }
        pick(self.builds(loader, version).await?, loader, version, wanted)
    }

    pub async fn download(&self, loader: Loader, build: &Build, dest: &Path) -> Result<u64> {
        let response = self.http.stream(loader.label(), &build.url).await?;
        checksum::write_verified(response.bytes_stream(), dest, build.checksum.as_ref(), &build.url)
            .await
    }
}

fn pick(builds: Vec<Build>, loader: Loader, version: &str, wanted: &Wanted) -> Result<Build> {
    match wanted {
        Wanted::Build(id) => builds.into_iter().find(|build| build.id == *id).ok_or_else(|| {
            LoaderError::UnknownBuild {
                loader: loader.label(),
                version: version.to_owned(),
                build: id.clone(),
            }
        }),
        Wanted::Latest => builds.into_iter().next().ok_or_else(|| no_build(loader, version, None)),
        Wanted::LatestStable => builds
            .into_iter()
            .find(|build| build.channel == Channel::Stable)
            .ok_or_else(|| no_build(loader, version, Some(Channel::Stable))),
    }
}

fn no_build(loader: Loader, version: &str, channel: Option<Channel>) -> LoaderError {
    LoaderError::NoBuild {
        loader: loader.label(),
        version: version.to_owned(),
        channel: channel.map_or("published", Channel::name),
    }
}

fn version_key(id: &str) -> (Vec<u64>, u8, Vec<u64>) {
    let (base, suffix) = id.split_once('-').unwrap_or((id, ""));
    let numbers = |text: &str| -> Vec<u64> {
        text.split(|c: char| !c.is_ascii_digit())
            .filter(|part| !part.is_empty())
            .map(|part| part.parse().unwrap_or(0))
            .collect()
    };

    let release = u8::from(suffix.is_empty() && !base.contains(|c: char| c.is_ascii_alphabetic()));
    (numbers(base), release, numbers(suffix))
}

fn newest_first(ids: &mut [String]) {
    ids.sort_by(|left, right| version_key(right).cmp(&version_key(left)));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build(id: &str, channel: Channel) -> Build {
        Build {
            id: id.to_owned(),
            channel,
            url: format!("https://example.invalid/{id}.jar"),
            filename: format!("{id}.jar"),
            checksum: None,
            size: None,
            java_major: None,
        }
    }

    #[test]
    fn every_loader_has_a_stable_id_and_survives_the_round_trip() {
        for loader in Loader::ALL {
            assert_eq!(Loader::from_id(loader.id()), Some(loader));
        }
        assert_eq!(Loader::from_id("spigot"), None);
        assert_eq!(Loader::ALL.len(), 7);
    }

    #[test]
    fn the_default_is_the_newest_stable_build() {
        let builds = vec![
            build("120", Channel::Experimental),
            build("119", Channel::Stable),
            build("118", Channel::Stable),
        ];

        assert_eq!(Wanted::default(), Wanted::LatestStable);
        let picked = pick(builds.clone(), Loader::Paper, "1.21.8", &Wanted::default()).unwrap();
        assert_eq!(picked.id, "119");

        let newest = pick(builds, Loader::Paper, "1.21.8", &Wanted::Latest).unwrap();
        assert_eq!(newest.id, "120");
    }

    #[test]
    fn a_version_with_nothing_but_experimental_builds_says_so() {
        let builds = vec![build("7", Channel::Experimental)];

        let err = pick(builds, Loader::Leaf, "26.2", &Wanted::LatestStable).unwrap_err();
        assert_eq!(err.to_string(), "Leaf has no stable build for 26.2 yet");
    }

    #[test]
    fn asking_for_a_build_that_is_not_there_names_the_build() {
        let err = pick(
            vec![build("60", Channel::Stable)],
            Loader::Paper,
            "1.21.8",
            &Wanted::Build("999".to_owned()),
        )
        .unwrap_err();

        assert_eq!(err.to_string(), "Paper has no build 999 for 1.21.8");
    }

    #[tokio::test]
    #[ignore = "talks to the four upstream services"]
    async fn live_every_source_names_a_stable_build() {
        let sources = Sources::new().unwrap();

        for loader in Loader::ALL {
            let versions = sources.versions(loader).await.expect(loader.label());
            let stable: Vec<&Version> =
                versions.iter().filter(|version| version.channel == Channel::Stable).collect();
            assert!(!stable.is_empty(), "{loader} lists no stable version");

            let mut resolved = None;
            for version in stable.iter().take(3) {
                if let Ok(build) = sources.resolve(loader, &version.id, &Wanted::default()).await {
                    resolved = Some((version.id.clone(), build));
                    break;
                }
            }

            let (version, build) = resolved
                .unwrap_or_else(|| panic!("{loader} has no stable build in its newest versions"));
            assert!(build.url.starts_with("https://"), "{loader}: {}", build.url);
            assert!(build.filename.ends_with(".jar"), "{loader}: {}", build.filename);
            println!("{loader} {version} build {} -> {}", build.id, build.filename);
        }
    }

    #[tokio::test]
    #[ignore = "downloads 13 MB from PaperMC"]
    async fn live_download_verifies_the_published_sha256() {
        let sources = Sources::new().unwrap();
        let build = sources
            .resolve(Loader::Velocity, "1.0.10", &Wanted::default())
            .await
            .unwrap();
        let checksum = build.checksum.clone().expect("PaperMC publishes a sha256");

        let dir = std::env::temp_dir().join("craftpanel-live-download");
        let dest = dir.join(&build.filename);
        let written = sources.download(Loader::Velocity, &build, &dest).await.unwrap();

        assert_eq!(Some(written), build.size);
        assert_eq!(checksum.algorithm, Algorithm::Sha256);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn version_ordering_beats_the_text_ordering_the_services_use() {
        let mut ids: Vec<String> = ["1.21.11", "1.21.4", "1.21.8", "26.1.2", "26.2", "1.9.4"]
            .iter()
            .map(|id| (*id).to_owned())
            .collect();
        newest_first(&mut ids);

        assert_eq!(ids, ["26.2", "26.1.2", "1.21.11", "1.21.8", "1.21.4", "1.9.4"]);
    }

    #[test]
    fn a_release_outranks_its_own_pre_releases() {
        let mut ids: Vec<String> = ["1.21.11-pre3", "1.21.11", "1.21.11-rc1", "4.0.0-SNAPSHOT"]
            .iter()
            .map(|id| (*id).to_owned())
            .collect();
        newest_first(&mut ids);

        assert_eq!(ids[0], "4.0.0-SNAPSHOT");
        assert_eq!(ids[1], "1.21.11");
        assert!(ids[2..].iter().all(|id| id.contains('-')), "{ids:?}");
    }
}
