use serde::Deserialize;

use super::error::{LoaderError, Result};
use super::http::{self, Http};
use super::{Build, Channel, Version};

const SERVICE: &str = "FabricMC";
const LOADER: &str = "Fabric";
const BASE: &str = "https://meta.fabricmc.net/v2/versions";

#[derive(Deserialize)]
struct Release {
    version: String,
    stable: bool,
}

pub async fn versions(http: &Http) -> Result<Vec<Version>> {
    parse_versions(&http.fetch(SERVICE, &format!("{BASE}/game")).await?)
}

pub async fn builds(http: &Http, version: &str) -> Result<Vec<Build>> {
    let games = http.fetch(SERVICE, &format!("{BASE}/game")).await?;
    let loaders = http.fetch(SERVICE, &format!("{BASE}/loader")).await?;
    let installers = http.fetch(SERVICE, &format!("{BASE}/installer")).await?;
    parse_builds(&games, &loaders, &installers, version)
}

fn parse_versions(body: &[u8]) -> Result<Vec<Version>> {
    let games: Vec<Release> = http::parse(SERVICE, body)?;
    Ok(games.into_iter().map(release_to_version).collect())
}

fn parse_builds(
    games: &[u8],
    loaders: &[u8],
    installers: &[u8],
    version: &str,
) -> Result<Vec<Build>> {
    let games: Vec<Release> = http::parse(SERVICE, games)?;
    if !games.iter().any(|game| game.version == version) {
        return Err(LoaderError::UnknownVersion { loader: LOADER, version: version.to_owned() });
    }

    let loaders: Vec<Release> = http::parse(SERVICE, loaders)?;
    let installers: Vec<Release> = http::parse(SERVICE, installers)?;
    let installer = newest_stable(&installers).ok_or_else(|| LoaderError::Unreadable {
        service: SERVICE,
        reason: "the installer list is empty".to_owned(),
    })?;

    Ok(loaders
        .into_iter()
        .map(|loader| Build {
            url: format!("{BASE}/loader/{version}/{}/{installer}/server/jar", loader.version),
            filename: format!(
                "fabric-server-mc.{version}-loader.{}-launcher.{installer}.jar",
                loader.version
            ),
            id: loader.version,
            channel: channel_of(loader.stable),
            checksum: None,
            size: None,
            java_major: None,
        })
        .collect())
}

fn newest_stable(releases: &[Release]) -> Option<&str> {
    releases
        .iter()
        .find(|release| release.stable)
        .or_else(|| releases.first())
        .map(|release| release.version.as_str())
}

fn release_to_version(release: Release) -> Version {
    Version { id: release.version, channel: channel_of(release.stable) }
}

fn channel_of(stable: bool) -> Channel {
    if stable {
        Channel::Stable
    } else {
        Channel::Experimental
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GAME: &[u8] = include_bytes!("testdata/fabric_game.json");
    const LOADERS: &[u8] = include_bytes!("testdata/fabric_loader.json");
    const INSTALLERS: &[u8] = include_bytes!("testdata/fabric_installer.json");

    #[test]
    fn game_versions_keep_their_order_and_their_stable_flag() {
        let versions = parse_versions(GAME).unwrap();

        assert_eq!(versions[0].id, "26.3-snapshot-8");
        assert_eq!(versions[0].channel, Channel::Experimental);
        assert_eq!(versions[3].id, "26.2");
        assert_eq!(versions[3].channel, Channel::Stable);
    }

    #[test]
    fn a_build_is_a_loader_version_pointing_at_the_finished_server_jar() {
        let builds = parse_builds(GAME, LOADERS, INSTALLERS, "26.2").unwrap();

        assert_eq!(builds.len(), 4);
        assert_eq!(builds[0].id, "0.19.3");
        assert_eq!(builds[0].channel, Channel::Stable);
        assert_eq!(
            builds[0].url,
            "https://meta.fabricmc.net/v2/versions/loader/26.2/0.19.3/1.1.2/server/jar"
        );
        assert_eq!(
            builds[0].filename,
            "fabric-server-mc.26.2-loader.0.19.3-launcher.1.1.2.jar"
        );
        assert_eq!(builds[0].checksum, None);
        assert_eq!(builds[1].channel, Channel::Experimental);
    }

    #[test]
    fn a_game_version_fabric_does_not_support_is_named_as_such() {
        let err = parse_builds(GAME, LOADERS, INSTALLERS, "1.99").unwrap_err();
        assert_eq!(err.to_string(), "Fabric has no version 1.99");
    }

    #[test]
    fn the_installer_is_pinned_to_the_newest_stable_one() {
        let installers: Vec<Release> = http::parse(SERVICE, INSTALLERS).unwrap();
        assert_eq!(newest_stable(&installers), Some("1.1.2"));
    }
}
