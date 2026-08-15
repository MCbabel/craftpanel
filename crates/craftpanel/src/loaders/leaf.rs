use serde::Deserialize;

use super::checksum::Checksum;
use super::error::{LoaderError, Result};
use super::http::{self, Http};
use super::{Build, Channel, Version};

const SERVICE: &str = "LeafMC";
const LOADER: &str = "Leaf";
const BASE: &str = "https://api.leafmc.one/v2/projects/leaf";

#[derive(Deserialize)]
struct Project {
    versions: Vec<String>,
}

#[derive(Deserialize)]
struct Builds {
    builds: Vec<Entry>,
}

#[derive(Deserialize)]
struct Entry {
    build: u32,
    channel: String,
    downloads: Downloads,
}

#[derive(Deserialize)]
struct Downloads {
    primary: Artifact,
}

#[derive(Deserialize)]
struct Artifact {
    name: String,
    sha256: String,
}

pub async fn versions(http: &Http) -> Result<Vec<Version>> {
    parse_versions(&http.fetch(SERVICE, BASE).await?)
}

pub async fn builds(http: &Http, version: &str) -> Result<Vec<Build>> {
    let url = format!("{BASE}/versions/{version}/builds");
    let body = http.maybe_fetch(SERVICE, &url).await?.ok_or_else(|| {
        LoaderError::UnknownVersion { loader: LOADER, version: version.to_owned() }
    })?;
    parse_builds(&body, version)
}

fn parse_versions(body: &[u8]) -> Result<Vec<Version>> {
    let project: Project = http::parse(SERVICE, body)?;
    let mut ids = project.versions;
    super::newest_first(&mut ids);
    Ok(ids.into_iter().map(|id| Version { id, channel: Channel::Stable }).collect())
}

fn parse_builds(body: &[u8], version: &str) -> Result<Vec<Build>> {
    let builds: Builds = http::parse(SERVICE, body)?;
    Ok(builds
        .builds
        .into_iter()
        .rev()
        .map(|entry| Build {
            url: format!(
                "{BASE}/versions/{version}/builds/{}/downloads/{}",
                entry.build, entry.downloads.primary.name
            ),
            id: entry.build.to_string(),
            channel: channel_of(&entry.channel),
            filename: entry.downloads.primary.name,
            checksum: Some(Checksum::sha256(entry.downloads.primary.sha256)),
            size: None,
            java_major: None,
        })
        .collect())
}

fn channel_of(channel: &str) -> Channel {
    match channel {
        "default" | "stable" => Channel::Stable,
        _ => Channel::Experimental,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PROJECT: &[u8] = include_bytes!("testdata/leaf_project.json");
    const BUILDS: &[u8] = include_bytes!("testdata/leaf_builds.json");
    const EXPERIMENTAL: &[u8] = include_bytes!("testdata/leaf_builds_experimental.json");

    #[test]
    fn versions_are_sorted_by_number_not_by_the_text_order_leaf_sends() {
        let versions = parse_versions(PROJECT).unwrap();

        assert_eq!(
            versions.iter().map(|v| v.id.as_str()).collect::<Vec<_>>(),
            ["26.2", "26.1.2", "1.21.11", "1.21.8", "1.21.7", "1.21.6", "1.21.5", "1.21.4"]
        );
    }

    #[test]
    fn a_build_gets_the_download_path_assembled_from_its_own_file_name() {
        let builds = parse_builds(BUILDS, "1.21.8").unwrap();

        assert_eq!(builds.len(), 3);
        assert_eq!(builds[0].id, "5");
        assert_eq!(builds[0].filename, "leaf-1.21.8-5.jar");
        assert_eq!(
            builds[0].url,
            "https://api.leafmc.one/v2/projects/leaf/versions/1.21.8/builds/5/downloads/\
             leaf-1.21.8-5.jar"
        );
        assert_eq!(
            builds[0].checksum,
            Some(Checksum::sha256(
                "385f0c5d00725800221e4daaeae5c29683cf1c72d6d0466c67585c3c96fe77ab"
            ))
        );
        assert_eq!(builds[0].channel, Channel::Stable);
        assert_eq!(builds.last().unwrap().id, "3");
    }

    #[test]
    fn builds_ahead_of_the_paper_release_stay_experimental() {
        let builds = parse_builds(EXPERIMENTAL, "26.2").unwrap();

        assert_eq!(builds[0].id, "64");
        assert!(builds.iter().all(|build| build.channel == Channel::Experimental));
    }

    #[test]
    fn the_old_paper_v2_shape_is_not_mistaken_for_the_v3_one() {
        let err = parse_builds(b"[{\"id\":1}]", "1.21.8").unwrap_err();
        assert!(err.to_string().starts_with("LeafMC answered in a shape"), "{err}");
    }
}
