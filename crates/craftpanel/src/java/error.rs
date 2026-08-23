use std::path::{Path, PathBuf};

use crate::loaders::LoaderError;

#[derive(Debug, thiserror::Error)]
pub enum JavaError {
    #[error(
        "no Java can be downloaded for this machine: Adoptium builds Linux runtimes for \
         x64 and aarch64, and this one is {arch}. Install Java {major} with the package \
         manager instead (apt install openjdk-{major}-jre-headless)."
    )]
    UnsupportedMachine { arch: String, major: u32 },

    #[error("Adoptium has no Java {major} runtime for linux/{arch}")]
    NoRelease { major: u32, arch: &'static str },

    #[error("Adoptium named no checksum for its Java {major} runtime, so nothing was installed")]
    NoChecksum { major: u32 },

    #[error(
        "Adoptium announced {announced} bytes for its Java {major} runtime, more than the \
         {ceiling} a runtime may be, so it was never asked for"
    )]
    AnnouncedTooLarge { major: u32, announced: u64, ceiling: u64 },

    #[error("the Java {major} archive is not built the way we expect: {reason}")]
    Malformed { major: u32, reason: String },

    #[error("the Java archive holds {0}")]
    Escapes(String),

    #[error("the unpacked Java {major} cannot be used: {reason}")]
    Incomplete { major: u32, reason: String },

    #[error(
        "a game account cannot get through {path}: it is mode {mode:04o} and belongs to uid \
         {owner}, so a Java {major} laid down behind it could never be started. Nothing was \
         installed. Run `chmod o+rx {path}`, or give the directory to the account the panel \
         runs as and let it put the mode right itself."
    )]
    Unreachable { major: u32, path: PathBuf, mode: u32, owner: u32 },

    #[error(
        "{path} is mode {mode:04o}: any account on this machine could write into it and put \
         its own program where Java {major} goes. Nothing was installed. Run \
         `chmod o-w {path}` and install again."
    )]
    Exposed { major: u32, path: PathBuf, mode: u32 },

    #[error("writing {path} failed: {reason}")]
    Write { path: PathBuf, reason: String },

    #[error("laying down Java {major} broke off: {reason}")]
    Interrupted { major: u32, reason: String },

    #[error(transparent)]
    Download(#[from] LoaderError),
}

impl JavaError {
    pub fn write(path: &Path, reason: std::io::Error) -> Self {
        Self::Write { path: path.to_owned(), reason: reason.to_string() }
    }

    pub fn code(&self) -> &'static str {
        match self {
            Self::UnsupportedMachine { .. } => "java_download_unsupported",
            Self::NoRelease { .. } | Self::NoChecksum { .. } => "java_download_unavailable",
            Self::AnnouncedTooLarge { .. } => "java_download_announced_oversized",
            Self::Malformed { .. } | Self::Escapes(_) => "java_archive_rejected",
            Self::Incomplete { .. } => "java_runtime_incomplete",
            Self::Unreachable { .. } => "java_runtime_unreachable",
            Self::Exposed { .. } => "java_runtime_exposed",
            Self::Write { .. } => "java_runtime_unwritable",
            Self::Interrupted { .. } => "internal",
            Self::Download(LoaderError::Damaged { .. }) => "java_download_damaged",
            Self::Download(LoaderError::Untrusted { .. }) => "java_download_untrusted",
            Self::Download(LoaderError::TooLarge { .. }) => "java_download_oversized",
            Self::Download(_) => "java_download_failed",
        }
    }
}

pub type Result<T> = std::result::Result<T, JavaError>;
