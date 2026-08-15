use serde::Deserialize;

use super::checksum::Checksum;
use super::error::{LoaderError, Result};
use super::http::{self, Http};
use super::{Build, Channel, Version};

const SERVICE: &str = "Mojang";
const LOADER: &str = "Vanilla";
const MANIFEST: &str = "https://launchermeta.mojang.com/mc/game/version_manifest_v2.json";

#[derive(Deserialize)]
struct Manifest {
    versions: Vec<Entry>,
}

#[derive(Deserialize)]
struct Entry {
    id: String,
    #[serde(rename = "type")]
    kind: String,
    url: String,
}

#[derive(Deserialize)]
struct Detail {
    downloads: Downloads,
    #[serde(rename = "javaVersion")]
    java_version: Option<JavaVersion>,
}

#[derive(Deserialize)]
struct JavaVersion {
    #[serde(rename = "majorVersion")]
    major_version: u32,
}

#[derive(Deserialize)]
struct Downloads {
    server: Option<Artifact>,
}

#[derive(Deserialize)]
struct Artifact {
    url: String,
    sha1: String,
    size: Option<u64>,
}

pub async fn versions(http: &Http) -> Result<Vec<Version>> {
    parse_versions(&http.fetch(SERVICE, MANIFEST).await?)
}

pub async fn builds(http: &Http, version: &str) -> Result<Vec<Build>> {
    let manifest = http.fetch(SERVICE, MANIFEST).await?;
    let (url, channel) = locate(&manifest, version)?;
    let detail = http.fetch(SERVICE, &url).await?;
    Ok(vec![parse_build(&detail, version, channel)?])
}

fn parse_versions(body: &[u8]) -> Result<Vec<Version>> {
    let manifest: Manifest = http::parse(SERVICE, body)?;
    Ok(manifest
        .versions
        .into_iter()
        .map(|entry| Version { channel: channel_of(&entry.kind), id: entry.id })
        .collect())
}

fn locate(body: &[u8], version: &str) -> Result<(String, Channel)> {
    let manifest: Manifest = http::parse(SERVICE, body)?;
    manifest
        .versions
        .into_iter()
        .find(|entry| entry.id == version)
        .map(|entry| (entry.url, channel_of(&entry.kind)))
        .ok_or_else(|| LoaderError::UnknownVersion { loader: LOADER, version: version.to_owned() })
}

fn parse_build(body: &[u8], version: &str, channel: Channel) -> Result<Build> {
    let detail: Detail = http::parse(SERVICE, body)?;
    let server = detail.downloads.server.ok_or_else(|| LoaderError::NoServerDownload {
        loader: LOADER,
        version: version.to_owned(),
    })?;

    Ok(Build {
        id: version.to_owned(),
        channel,
        url: server.url,
        filename: format!("minecraft_server.{version}.jar"),
        checksum: Some(Checksum::sha1(server.sha1)),
        size: server.size,
        java_major: detail.java_version.map(|java| java.major_version),
    })
}

fn channel_of(kind: &str) -> Channel {
    if kind == "release" {
        Channel::Stable
    } else {
        Channel::Experimental
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MANIFEST_BODY: &[u8] = include_bytes!("testdata/vanilla_manifest.json");
    const VERSION_BODY: &[u8] = include_bytes!("testdata/vanilla_version.json");
    const CLIENT_ONLY: &[u8] = include_bytes!("testdata/vanilla_version_client_only.json");

    #[test]
    fn the_manifest_keeps_its_own_order_and_marks_snapshots_experimental() {
        let versions = parse_versions(MANIFEST_BODY).unwrap();

        assert_eq!(
            versions.iter().map(|v| v.id.as_str()).collect::<Vec<_>>(),
            ["26.3-snapshot-8", "26.2", "1.21.11", "1.21.8", "a1.2.6"]
        );
        assert_eq!(versions[0].channel, Channel::Experimental);
        assert_eq!(versions[1].channel, Channel::Stable);
        assert_eq!(versions[4].channel, Channel::Experimental);
    }

    #[test]
    fn a_version_carries_its_server_jar_with_mojangs_sha1() {
        let build = parse_build(VERSION_BODY, "26.2", Channel::Stable).unwrap();

        assert_eq!(build.id, "26.2");
        assert_eq!(
            build.url,
            "https://piston-data.mojang.com/v1/objects/\
             823e2250d24b3ddac457a60c92a6a941943fcd6a/server.jar"
        );
        assert_eq!(build.filename, "minecraft_server.26.2.jar");
        assert_eq!(
            build.checksum,
            Some(Checksum::sha1("823e2250d24b3ddac457a60c92a6a941943fcd6a"))
        );
        assert_eq!(build.size, Some(60894273));
    }

    #[test]
    fn versions_from_before_multiplayer_say_so_instead_of_failing_to_parse() {
        let err = parse_build(CLIENT_ONLY, "a1.2.6", Channel::Experimental).unwrap_err();
        assert_eq!(err.to_string(), "Vanilla offers no server download for a1.2.6");
    }

    #[test]
    fn an_unknown_version_names_the_version() {
        let err = locate(MANIFEST_BODY, "1.99").unwrap_err();
        assert_eq!(err.to_string(), "Vanilla has no version 1.99");
    }

    #[test]
    fn a_known_version_yields_the_url_of_its_own_json() {
        let (url, channel) = locate(MANIFEST_BODY, "1.21.8").unwrap();
        assert!(url.starts_with("https://piston-meta.mojang.com/v1/packages/"), "{url}");
        assert_eq!(channel, Channel::Stable);
    }
}
