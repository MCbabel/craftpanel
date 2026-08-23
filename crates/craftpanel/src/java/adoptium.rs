use serde::Deserialize;

use crate::loaders::{http, Checksum, Http};

use super::error::{JavaError, Result};

pub const BASE: &str = "https://api.adoptium.net";
pub const SERVICE: &str = "Adoptium";

const ORIGINS: [&str; 3] = [
    "https://api.adoptium.net",
    "https://github.com",
    "https://release-assets.githubusercontent.com",
];

pub fn origins(base: &str) -> Vec<String> {
    let mut allowed: Vec<String> = ORIGINS.iter().map(|origin| (*origin).to_owned()).collect();
    if let Ok(own) = reqwest::Url::parse(base).map(|url| url.origin().ascii_serialization()) {
        if !allowed.contains(&own) {
            allowed.push(own);
        }
    }
    allowed
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Arch {
    X64,
    Aarch64,
}

impl Arch {
    pub fn here() -> Option<Self> {
        Self::of(std::env::consts::ARCH)
    }

    pub fn of(name: &str) -> Option<Self> {
        match name {
            "x86_64" | "x64" | "amd64" => Some(Self::X64),
            "aarch64" | "arm64" => Some(Self::Aarch64),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::X64 => "x64",
            Self::Aarch64 => "aarch64",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Release {
    pub name: String,
    pub semver: String,
    pub filename: String,
    pub url: String,
    pub checksum: Checksum,
    pub size: u64,
}

#[derive(Deserialize)]
struct Asset {
    binary: Binary,
    release_name: String,
    version: Version,
}

#[derive(Deserialize)]
struct Binary {
    package: Package,
}

#[derive(Deserialize)]
struct Package {
    link: String,
    name: String,
    checksum: Option<String>,
    size: Option<u64>,
}

#[derive(Deserialize)]
struct Version {
    major: u32,
    semver: String,
}

pub fn url(base: &str, major: u32, arch: Arch) -> String {
    format!(
        "{}/v3/assets/latest/{major}/hotspot\
         ?architecture={}&image_type=jre&os=linux&vendor=eclipse",
        base.trim_end_matches('/'),
        arch.as_str()
    )
}

pub async fn latest(http: &Http, base: &str, major: u32, arch: Arch) -> Result<Release> {
    let body = http
        .maybe_fetch(SERVICE, &url(base, major, arch))
        .await?
        .ok_or(JavaError::NoRelease { major, arch: arch.as_str() })?;
    parse(&body, major, arch)
}

fn parse(body: &[u8], major: u32, arch: Arch) -> Result<Release> {
    let assets: Vec<Asset> = http::parse(SERVICE, body)?;
    let asset = assets
        .into_iter()
        .find(|asset| asset.version.major == major)
        .ok_or(JavaError::NoRelease { major, arch: arch.as_str() })?;

    let checksum = asset
        .binary
        .package
        .checksum
        .filter(|value| !value.trim().is_empty())
        .ok_or(JavaError::NoChecksum { major })?;

    Ok(Release {
        name: asset.release_name,
        semver: asset.version.semver,
        filename: asset.binary.package.name,
        url: asset.binary.package.link,
        checksum: Checksum::sha256(checksum),
        size: asset.binary.package.size.unwrap_or_default(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const LATEST_8: &[u8] = include_bytes!("testdata/adoptium_latest_8.json");

    #[test]
    fn the_two_machines_we_serve_are_recognised_under_every_name_they_go_by() {
        assert_eq!(Arch::of("x86_64"), Some(Arch::X64));
        assert_eq!(Arch::of("amd64"), Some(Arch::X64));
        assert_eq!(Arch::of("aarch64"), Some(Arch::Aarch64));
        assert_eq!(Arch::of("arm64"), Some(Arch::Aarch64));
        assert_eq!(Arch::of("armv7l"), None);
        assert_eq!(Arch::of("riscv64"), None);
        assert_eq!(Arch::X64.as_str(), "x64");
    }

    #[test]
    fn a_java_may_come_from_adoptium_and_the_two_hosts_its_links_lead_to() {
        assert_eq!(
            origins(BASE),
            [
                "https://api.adoptium.net",
                "https://github.com",
                "https://release-assets.githubusercontent.com"
            ]
        );
        assert!(origins(BASE).iter().all(|origin| origin.starts_with("https://")));
    }

    #[test]
    fn whatever_base_we_were_given_may_serve_its_own_downloads() {
        let allowed = origins("http://127.0.0.1:8080");

        assert_eq!(allowed.len(), 4);
        assert!(allowed.contains(&"http://127.0.0.1:8080".to_owned()));
        assert_eq!(origins("https://api.adoptium.net/").len(), 3, "the base is already in there");
    }

    #[test]
    fn the_query_names_a_headless_linux_jre_of_eclipse() {
        assert_eq!(
            url("https://api.adoptium.net/", 21, Arch::Aarch64),
            "https://api.adoptium.net/v3/assets/latest/21/hotspot\
             ?architecture=aarch64&image_type=jre&os=linux&vendor=eclipse"
        );
    }

    #[test]
    fn the_answer_adoptium_gave_for_java_8_carries_the_link_the_size_and_the_sha256() {
        let release = parse(LATEST_8, 8, Arch::X64).expect("a readable answer");

        assert_eq!(release.name, "jdk8u502-b07");
        assert_eq!(release.semver, "8.0.502+7");
        assert_eq!(release.filename, "OpenJDK8U-jre_x64_linux_hotspot_8u502b07.tar.gz");
        assert_eq!(
            release.url,
            "https://github.com/adoptium/temurin8-binaries/releases/download/jdk8u502-b07/\
             OpenJDK8U-jre_x64_linux_hotspot_8u502b07.tar.gz"
        );
        assert_eq!(
            release.checksum,
            Checksum::sha256("f1a7bea0804bfa5627dac412fe7a0d751c4228592e356d6a32a30da54a48ed7a")
        );
        assert_eq!(release.size, 41_851_657);
    }

    #[test]
    fn a_major_nobody_builds_says_so_instead_of_taking_the_first_thing_offered() {
        let err = parse(b"[]", 26, Arch::X64).unwrap_err();
        assert_eq!(err.to_string(), "Adoptium has no Java 26 runtime for linux/x64");
        assert_eq!(err.code(), "java_download_unavailable");

        let other = parse(LATEST_8, 21, Arch::X64).unwrap_err();
        assert_eq!(other.to_string(), "Adoptium has no Java 21 runtime for linux/x64");
    }

    #[test]
    fn an_answer_without_a_checksum_is_no_answer_at_all() {
        let stripped = String::from_utf8(LATEST_8.to_vec())
            .unwrap()
            .replace("\"checksum\":", "\"unchecked\":");

        let err = parse(stripped.as_bytes(), 8, Arch::X64).unwrap_err();
        assert_eq!(
            err.to_string(),
            "Adoptium named no checksum for its Java 8 runtime, so nothing was installed"
        );
    }

    #[test]
    fn a_shape_we_do_not_understand_names_the_service() {
        let err = parse(b"{\"message\":\"nope\"}", 8, Arch::X64).unwrap_err();
        assert!(err.to_string().starts_with("Adoptium answered in a shape"), "{err}");
        assert_eq!(err.code(), "java_download_failed");
    }
}
