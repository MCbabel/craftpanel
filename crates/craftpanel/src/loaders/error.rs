use std::path::{Path, PathBuf};

use super::Algorithm;

#[derive(Debug, thiserror::Error)]
pub enum LoaderError {
    #[error("{loader} has no version {version}")]
    UnknownVersion { loader: &'static str, version: String },

    #[error("{loader} has no build {build} for {version}")]
    UnknownBuild { loader: &'static str, version: String, build: String },

    #[error("{loader} has no {channel} build for {version} yet")]
    NoBuild { loader: &'static str, version: String, channel: &'static str },

    #[error("{loader} build {build} for {version} did not finish successfully")]
    BrokenBuild { loader: &'static str, version: String, build: String },

    #[error("{loader} offers no server download for {version}")]
    NoServerDownload { loader: &'static str, version: String },

    #[error("{service} is not reachable: {reason}")]
    Unreachable { service: &'static str, reason: String },

    #[error("{service} refused the request (HTTP {status}): {detail}")]
    Refused { service: &'static str, status: u16, detail: String },

    #[error("{service} answered in a shape we do not understand: {reason}")]
    Unreadable { service: &'static str, reason: String },

    #[error("{service} sent us to {origin}, which is not one of the hosts its downloads come from")]
    Untrusted { service: &'static str, origin: String },

    #[error("the download from {origin} broke off: {reason}")]
    Interrupted { origin: String, reason: String },

    #[error("the download from {origin} is longer than the {ceiling} bytes it may be")]
    TooLarge { origin: String, ceiling: u64 },

    #[error(
        "the file from {origin} is damaged: its {algorithm} is {actual}, \
         but {expected} was announced"
    )]
    Damaged { origin: String, algorithm: Algorithm, expected: String, actual: String },

    #[error("writing {path} failed: {reason}")]
    Write { path: PathBuf, reason: String },

    #[error("the HTTP client could not be set up: {0}")]
    Setup(String),
}

impl LoaderError {
    pub fn write(path: &Path, reason: std::io::Error) -> Self {
        Self::Write { path: path.to_owned(), reason: reason.to_string() }
    }
}

pub type Result<T> = std::result::Result<T, LoaderError>;
