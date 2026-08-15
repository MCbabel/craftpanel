use std::collections::HashMap;

use serde::Deserialize;

use super::checksum::Checksum;
use super::error::{LoaderError, Result};
use super::http::{self, Http};
use super::{Build, Channel, Loader, Version};

const SERVICE: &str = "PaperMC";
const BASE: &str = "https://fill.papermc.io/v3/projects";
const SERVER_DOWNLOAD: &str = "server:default";

#[derive(Deserialize)]
struct Project {
    versions: HashMap<String, Vec<String>>,
}

#[derive(Deserialize)]
struct FillBuild {
    id: u32,
    channel: String,
    downloads: HashMap<String, Download>,
}

#[derive(Deserialize)]
struct Download {
    name: String,
    url: String,
    size: Option<u64>,
    #[serde(default)]
    checksums: HashMap<String, String>,
}

pub async fn versions(http: &Http, loader: Loader) -> Result<Vec<Version>> {
    let url = format!("{BASE}/{}", loader.id());
    parse_versions(&http.fetch(SERVICE, &url).await?)
}

pub async fn builds(http: &Http, loader: Loader, version: &str) -> Result<Vec<Build>> {
    let url = format!("{BASE}/{}/versions/{version}/builds", loader.id());
    let body = http
        .maybe_fetch(SERVICE, &url)
        .await?
        .ok_or_else(|| LoaderError::UnknownVersion {
            loader: loader.label(),
            version: version.to_owned(),
        })?;
    parse_builds(&body, loader, version)
}

fn parse_versions(body: &[u8]) -> Result<Vec<Version>> {
    let project: Project = http::parse(SERVICE, body)?;

    let mut groups: Vec<(String, Vec<String>)> = project.versions.into_iter().collect();
    groups.sort_by(|left, right| {
        super::version_key(&right.0).cmp(&super::version_key(&left.0))
    });

    Ok(groups
        .into_iter()
        .flat_map(|(_, versions)| versions)
        .map(|id| Version { channel: version_channel(&id), id })
        .collect())
}

fn parse_builds(body: &[u8], loader: Loader, version: &str) -> Result<Vec<Build>> {
    let builds: Vec<FillBuild> = http::parse(SERVICE, body)?;

    builds
        .into_iter()
        .map(|build| {
            let mut download = server_download(build.downloads).ok_or_else(|| {
                LoaderError::NoServerDownload {
                    loader: loader.label(),
                    version: version.to_owned(),
                }
            })?;
            Ok(Build {
                id: build.id.to_string(),
                channel: build_channel(&build.channel),
                url: download.url,
                filename: download.name,
                checksum: download.checksums.remove("sha256").map(Checksum::sha256),
                size: download.size,
                java_major: None,
            })
        })
        .collect()
}

fn server_download(mut downloads: HashMap<String, Download>) -> Option<Download> {
    downloads
        .remove(SERVER_DOWNLOAD)
        .or_else(|| downloads.into_values().next())
}

fn version_channel(id: &str) -> Channel {
    let lowered = id.to_ascii_lowercase();
    let prerelease = ["-pre", "-rc", "snapshot", "-exp"]
        .iter()
        .any(|marker| lowered.contains(marker));
    if prerelease {
        Channel::Experimental
    } else {
        Channel::Stable
    }
}

fn build_channel(channel: &str) -> Channel {
    match channel.to_ascii_uppercase().as_str() {
        "STABLE" | "RECOMMENDED" | "DEFAULT" => Channel::Stable,
        _ => Channel::Experimental,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAPER: &[u8] = include_bytes!("testdata/fill_paper_project.json");
    const FOLIA: &[u8] = include_bytes!("testdata/fill_folia_project.json");
    const VELOCITY: &[u8] = include_bytes!("testdata/fill_velocity_project.json");
    const PAPER_BUILDS: &[u8] = include_bytes!("testdata/fill_paper_builds.json");
    const PAPER_MIXED: &[u8] = include_bytes!("testdata/fill_paper_builds_mixed.json");
    const VELOCITY_BUILDS: &[u8] = include_bytes!("testdata/fill_velocity_builds.json");

    #[test]
    fn version_groups_come_out_newest_first_even_though_the_json_is_a_map() {
        let versions = parse_versions(PAPER).unwrap();
        let ids: Vec<&str> = versions.iter().map(|v| v.id.as_str()).collect();

        assert_eq!(&ids[..4], ["26.2", "26.2-rc-2", "26.1.2", "26.1.1"]);
        assert_eq!(ids[4], "1.21.11");
        assert_eq!(*ids.last().unwrap(), "1.7.10");
        assert!(
            ids.iter().position(|id| *id == "1.9.4") < ids.iter().position(|id| *id == "1.8.8"),
            "1.9 must not sort below 1.8: {ids:?}"
        );
    }

    #[test]
    fn a_release_candidate_is_not_offered_as_stable() {
        let versions = parse_versions(PAPER).unwrap();
        let by_id = |id: &str| versions.iter().find(|v| v.id == id).unwrap().channel;

        assert_eq!(by_id("26.2"), Channel::Stable);
        assert_eq!(by_id("26.2-rc-2"), Channel::Experimental);
        assert_eq!(by_id("1.21.11-pre5"), Channel::Experimental);
    }

    #[test]
    fn folia_starts_at_the_version_where_folia_started() {
        let versions = parse_versions(FOLIA).unwrap();
        let ids: Vec<&str> = versions.iter().map(|v| v.id.as_str()).collect();

        assert_eq!(ids.first(), Some(&"26.2"));
        assert_eq!(ids.last(), Some(&"1.19.4"));
    }

    #[test]
    fn velocity_lists_proxy_versions_not_game_versions() {
        let versions = parse_versions(VELOCITY).unwrap();
        let ids: Vec<&str> = versions.iter().map(|v| v.id.as_str()).collect();

        assert_eq!(&ids[..3], ["4.1.0-SNAPSHOT", "4.0.0", "4.0.0-SNAPSHOT"]);
        assert_eq!(versions[0].channel, Channel::Experimental);
        assert_eq!(versions[1].channel, Channel::Stable);
    }

    #[test]
    fn a_paper_build_carries_its_jar_and_a_sha256() {
        let builds = parse_builds(PAPER_BUILDS, Loader::Paper, "1.21.8").unwrap();

        assert_eq!(builds.len(), 3);
        assert_eq!(builds[0].id, "60");
        assert_eq!(builds[0].channel, Channel::Stable);
        assert_eq!(builds[0].filename, "paper-1.21.8-60.jar");
        assert_eq!(
            builds[0].url,
            "https://fill-data.papermc.io/v1/objects/\
             8de7c52c3b02403503d16fac58003f1efef7dd7a0256786843927fa92ee57f1e/paper-1.21.8-60.jar"
        );
        assert_eq!(
            builds[0].checksum,
            Some(Checksum::sha256(
                "8de7c52c3b02403503d16fac58003f1efef7dd7a0256786843927fa92ee57f1e"
            ))
        );
        assert_eq!(builds[0].size, Some(52811717));
    }

    #[test]
    fn alpha_and_beta_builds_are_experimental() {
        let builds = parse_builds(PAPER_MIXED, Loader::Paper, "1.21.11").unwrap();

        let channels: Vec<Channel> = builds.iter().map(|build| build.channel).collect();
        assert_eq!(
            channels,
            [Channel::Stable, Channel::Experimental, Channel::Experimental]
        );
    }

    #[test]
    fn velocitys_recommended_channel_counts_as_stable() {
        let builds = parse_builds(VELOCITY_BUILDS, Loader::Velocity, "3.5.1").unwrap();

        assert_eq!(builds.len(), 1);
        assert_eq!(builds[0].id, "615");
        assert_eq!(builds[0].channel, Channel::Stable);
        assert_eq!(builds[0].filename, "velocity-3.5.1-615.jar");
    }

    #[test]
    fn an_unreadable_answer_names_the_service() {
        let err = parse_builds(b"not json at all", Loader::Paper, "1.21.8").unwrap_err();
        assert!(err.to_string().starts_with("PaperMC answered in a shape"), "{err}");
    }
}
