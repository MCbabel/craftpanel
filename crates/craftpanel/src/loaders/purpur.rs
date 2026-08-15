use serde::Deserialize;

use super::checksum::Checksum;
use super::error::{LoaderError, Result};
use super::http::{self, Http};
use super::{Build, Channel, Version, Wanted};

const SERVICE: &str = "Purpur";
const LOADER: &str = "Purpur";
const BASE: &str = "https://api.purpurmc.org/v2/purpur";

#[derive(Deserialize)]
struct Project {
    versions: Vec<String>,
}

#[derive(Deserialize)]
struct VersionDetail {
    builds: BuildList,
}

#[derive(Deserialize)]
struct BuildList {
    all: Vec<String>,
}

#[derive(Deserialize)]
struct BuildDetail {
    build: String,
    result: String,
    md5: Option<String>,
}

pub async fn versions(http: &Http) -> Result<Vec<Version>> {
    parse_versions(&http.fetch(SERVICE, BASE).await?)
}

pub async fn builds(http: &Http, version: &str) -> Result<Vec<Build>> {
    let url = format!("{BASE}/{version}");
    let body = http
        .maybe_fetch(SERVICE, &url)
        .await?
        .ok_or_else(|| unknown_version(version))?;
    parse_builds(&body, version)
}

pub async fn resolve(http: &Http, version: &str, wanted: &Wanted) -> Result<Build> {
    let (url, missing) = match wanted {
        Wanted::Build(build) => (
            format!("{BASE}/{version}/{build}"),
            LoaderError::UnknownBuild {
                loader: LOADER,
                version: version.to_owned(),
                build: build.clone(),
            },
        ),
        Wanted::Latest | Wanted::LatestStable => {
            (format!("{BASE}/{version}/latest"), unknown_version(version))
        }
    };

    let body = http.maybe_fetch(SERVICE, &url).await?.ok_or(missing)?;
    parse_build(&body, version)
}

fn parse_versions(body: &[u8]) -> Result<Vec<Version>> {
    let project: Project = http::parse(SERVICE, body)?;
    let mut ids = project.versions;
    super::newest_first(&mut ids);
    Ok(ids.into_iter().map(|id| Version { id, channel: Channel::Stable }).collect())
}

fn parse_builds(body: &[u8], version: &str) -> Result<Vec<Build>> {
    let detail: VersionDetail = http::parse(SERVICE, body)?;
    Ok(detail
        .builds
        .all
        .into_iter()
        .rev()
        .map(|build| entry(version, &build, None))
        .collect())
}

fn parse_build(body: &[u8], version: &str) -> Result<Build> {
    let detail: BuildDetail = http::parse(SERVICE, body)?;
    if detail.result != "SUCCESS" {
        return Err(LoaderError::BrokenBuild {
            loader: LOADER,
            version: version.to_owned(),
            build: detail.build,
        });
    }
    Ok(entry(version, &detail.build, detail.md5.map(Checksum::md5)))
}

fn entry(version: &str, build: &str, checksum: Option<Checksum>) -> Build {
    Build {
        id: build.to_owned(),
        channel: Channel::Stable,
        url: format!("{BASE}/{version}/{build}/download"),
        filename: format!("purpur-{version}-{build}.jar"),
        checksum,
        size: None,
        java_major: None,
    }
}

fn unknown_version(version: &str) -> LoaderError {
    LoaderError::UnknownVersion { loader: LOADER, version: version.to_owned() }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PROJECT: &[u8] = include_bytes!("testdata/purpur_project.json");
    const VERSION: &[u8] = include_bytes!("testdata/purpur_version.json");
    const BUILD: &[u8] = include_bytes!("testdata/purpur_build.json");

    #[test]
    fn versions_are_turned_around_so_the_newest_leads() {
        let versions = parse_versions(PROJECT).unwrap();

        assert_eq!(versions[0].id, "26.2");
        assert_eq!(versions[1].id, "26.1.2");
        assert_eq!(versions.last().unwrap().id, "1.14.1");
        assert!(versions.iter().all(|v| v.channel == Channel::Stable));
    }

    #[test]
    fn the_build_list_gives_download_urls_but_no_checksums_yet() {
        let builds = parse_builds(VERSION, "1.21.8").unwrap();

        assert_eq!(builds.len(), 20);
        assert_eq!(builds[0].id, "2497");
        assert_eq!(builds[0].url, "https://api.purpurmc.org/v2/purpur/1.21.8/2497/download");
        assert_eq!(builds[0].filename, "purpur-1.21.8-2497.jar");
        assert_eq!(builds[0].checksum, None);
        assert_eq!(builds.last().unwrap().id, "2478");
    }

    #[test]
    fn a_resolved_build_carries_the_md5_purpur_publishes() {
        let build = parse_build(BUILD, "1.21.8").unwrap();

        assert_eq!(build.id, "2497");
        assert_eq!(build.checksum, Some(Checksum::md5("b8b2802525f5b85f986af49ac4b095aa")));
        assert_eq!(build.filename, "purpur-1.21.8-2497.jar");
    }

    #[test]
    fn a_build_that_failed_on_their_side_is_not_offered() {
        let failed = String::from_utf8(BUILD.to_vec()).unwrap().replace("SUCCESS", "FAILURE");

        let err = parse_build(failed.as_bytes(), "1.21.8").unwrap_err();
        assert_eq!(
            err.to_string(),
            "Purpur build 2497 for 1.21.8 did not finish successfully"
        );
    }

    #[test]
    fn an_unknown_version_names_the_version() {
        assert_eq!(unknown_version("1.99").to_string(), "Purpur has no version 1.99");
    }
}
