use std::fmt;
use std::path::{Path, PathBuf};

use futures::{Stream, StreamExt};
use md5::Md5;
use serde::{Deserialize, Serialize};
use sha1::Sha1;
use sha2::Sha256;
use tokio::io::AsyncWriteExt;

use super::error::{LoaderError, Result};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Algorithm {
    Sha1,
    Sha256,
    Md5,
}

impl fmt::Display for Algorithm {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Sha1 => "sha1",
            Self::Sha256 => "sha256",
            Self::Md5 => "md5",
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Checksum {
    pub algorithm: Algorithm,
    pub value: String,
}

impl Checksum {
    pub fn sha1(value: impl Into<String>) -> Self {
        Self { algorithm: Algorithm::Sha1, value: value.into() }
    }

    pub fn sha256(value: impl Into<String>) -> Self {
        Self { algorithm: Algorithm::Sha256, value: value.into() }
    }

    pub fn md5(value: impl Into<String>) -> Self {
        Self { algorithm: Algorithm::Md5, value: value.into() }
    }

    pub fn matches(&self, digest: &str) -> bool {
        self.value.trim().eq_ignore_ascii_case(digest)
    }
}

enum Digester {
    Sha1(Sha1),
    Sha256(Sha256),
    Md5(Md5),
}

impl Digester {
    fn new(algorithm: Algorithm) -> Self {
        match algorithm {
            Algorithm::Sha1 => Self::Sha1(Sha1::default()),
            Algorithm::Sha256 => Self::Sha256(Sha256::default()),
            Algorithm::Md5 => Self::Md5(Md5::default()),
        }
    }

    fn update(&mut self, chunk: &[u8]) {
        match self {
            Self::Sha1(state) => sha1::Digest::update(state, chunk),
            Self::Sha256(state) => sha2::Digest::update(state, chunk),
            Self::Md5(state) => md5::Digest::update(state, chunk),
        }
    }

    fn finish(self) -> String {
        match self {
            Self::Sha1(state) => hex::encode(sha1::Digest::finalize(state)),
            Self::Sha256(state) => hex::encode(sha2::Digest::finalize(state)),
            Self::Md5(state) => hex::encode(md5::Digest::finalize(state)),
        }
    }
}

pub fn digest(algorithm: Algorithm, bytes: &[u8]) -> String {
    let mut digester = Digester::new(algorithm);
    digester.update(bytes);
    digester.finish()
}

pub async fn write_verified<S, B, E>(
    stream: S,
    dest: &Path,
    expected: Option<&Checksum>,
    origin: &str,
) -> Result<u64>
where
    S: Stream<Item = std::result::Result<B, E>>,
    B: AsRef<[u8]>,
    E: fmt::Display,
{
    write_capped(stream, dest, expected, origin, u64::MAX).await
}

pub async fn write_capped<S, B, E>(
    stream: S,
    dest: &Path,
    expected: Option<&Checksum>,
    origin: &str,
    ceiling: u64,
) -> Result<u64>
where
    S: Stream<Item = std::result::Result<B, E>>,
    B: AsRef<[u8]>,
    E: fmt::Display,
{
    if let Some(parent) = dest.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|err| LoaderError::write(parent, err))?;
    }

    let mut partial = dest.as_os_str().to_owned();
    partial.push(".part");
    let partial = PathBuf::from(partial);

    match collect(stream, &partial, expected, origin, ceiling).await {
        Ok(written) => {
            tokio::fs::rename(&partial, dest)
                .await
                .map_err(|err| LoaderError::write(dest, err))?;
            Ok(written)
        }
        Err(err) => {
            let _ = tokio::fs::remove_file(&partial).await;
            Err(err)
        }
    }
}

async fn collect<S, B, E>(
    stream: S,
    partial: &Path,
    expected: Option<&Checksum>,
    origin: &str,
    ceiling: u64,
) -> Result<u64>
where
    S: Stream<Item = std::result::Result<B, E>>,
    B: AsRef<[u8]>,
    E: fmt::Display,
{
    let mut file = tokio::fs::File::create(partial)
        .await
        .map_err(|err| LoaderError::write(partial, err))?;
    let mut digester = expected.map(|checksum| Digester::new(checksum.algorithm));
    let mut written = 0u64;
    let mut stream = std::pin::pin!(stream);

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|err| LoaderError::Interrupted {
            origin: origin.to_owned(),
            reason: err.to_string(),
        })?;
        let chunk = chunk.as_ref();
        if written.saturating_add(chunk.len() as u64) > ceiling {
            return Err(LoaderError::TooLarge { origin: origin.to_owned(), ceiling });
        }

        if let Some(digester) = digester.as_mut() {
            digester.update(chunk);
        }
        file.write_all(chunk)
            .await
            .map_err(|err| LoaderError::write(partial, err))?;
        written += chunk.len() as u64;
    }

    file.sync_all()
        .await
        .map_err(|err| LoaderError::write(partial, err))?;

    if let (Some(digester), Some(expected)) = (digester, expected) {
        let actual = digester.finish();
        if !expected.matches(&actual) {
            return Err(LoaderError::Damaged {
                origin: origin.to_owned(),
                algorithm: expected.algorithm,
                expected: expected.value.clone(),
                actual,
            });
        }
    }

    Ok(written)
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    use super::*;

    const MOJANG_FIXTURE: &[u8] = include_bytes!("testdata/vanilla_version_client_only.json");
    const MOJANG_SHA1: &str = "1c888e4d8aed380db25aeb3835f5918297bb5e3a";
    const FIXTURE_SHA256: &str =
        "8b26f498a33ef3083426c4a1cd8245883b03cbea2479b2338d73d294223cf0c2";
    const FIXTURE_MD5: &str = "013df5c4378dff039b8e658c5f05ae52";

    fn chunked(bytes: &'static [u8]) -> impl Stream<Item = std::io::Result<&'static [u8]>> {
        futures::stream::iter(bytes.chunks(1024).map(Ok))
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("craftpanel-loaders-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn every_algorithm_hashes_the_fixture_the_way_the_command_line_tools_do() {
        assert_eq!(digest(Algorithm::Sha1, MOJANG_FIXTURE), MOJANG_SHA1);
        assert_eq!(digest(Algorithm::Sha256, MOJANG_FIXTURE), FIXTURE_SHA256);
        assert_eq!(digest(Algorithm::Md5, MOJANG_FIXTURE), FIXTURE_MD5);
    }

    #[test]
    fn a_checksum_compares_case_insensitively() {
        assert!(Checksum::sha1(MOJANG_SHA1.to_uppercase()).matches(MOJANG_SHA1));
        assert!(!Checksum::sha1(MOJANG_SHA1).matches(&MOJANG_SHA1.replace('1', "2")));
    }

    #[tokio::test]
    async fn a_download_that_matches_the_published_sha1_lands_in_place() {
        let dir = temp_dir("good");
        let dest = dir.join("nested").join("server.jar");

        let written = write_verified(
            chunked(MOJANG_FIXTURE),
            &dest,
            Some(&Checksum::sha1(MOJANG_SHA1)),
            "the test fixture",
        )
        .await
        .expect("the fixture matches the checksum Mojang publishes for it");

        assert_eq!(written, MOJANG_FIXTURE.len() as u64);
        assert_eq!(std::fs::read(&dest).unwrap(), MOJANG_FIXTURE);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn a_wrong_checksum_is_an_error_and_leaves_nothing_behind() {
        let dir = temp_dir("damaged");
        let dest = dir.join("server.jar");
        let announced = "0000000000000000000000000000000000000000";

        let err = write_verified(
            chunked(MOJANG_FIXTURE),
            &dest,
            Some(&Checksum::sha1(announced)),
            "https://example.invalid/server.jar",
        )
        .await
        .expect_err("a mismatch must fail the download");

        match err {
            LoaderError::Damaged { algorithm, expected, actual, .. } => {
                assert_eq!(algorithm, Algorithm::Sha1);
                assert_eq!(expected, announced);
                assert_eq!(actual, MOJANG_SHA1);
            }
            other => panic!("expected a damaged file, got {other:?}"),
        }

        assert!(!dest.exists());
        assert!(!dest.with_extension("jar.part").exists());
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn a_source_without_a_checksum_still_writes_the_file() {
        let dir = temp_dir("unchecked");
        let dest = dir.join("fabric.jar");

        write_verified(chunked(MOJANG_FIXTURE), &dest, None, "fabric").await.unwrap();

        assert_eq!(std::fs::read(&dest).unwrap().len(), MOJANG_FIXTURE.len());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn a_stream_that_grows_past_its_ceiling_stops_at_the_chunk_that_bursts_it() {
        const CHUNK: &[u8] = &[0u8; 1024];
        let dir = temp_dir("capped");
        let dest = dir.join("archive.tar.gz");
        let pulled = Arc::new(AtomicUsize::new(0));
        let counted = {
            let pulled = Arc::clone(&pulled);
            futures::stream::iter((0..1024).map(|_| Ok::<&[u8], std::io::Error>(CHUNK)))
                .inspect(move |_| {
                    pulled.fetch_add(1, Ordering::Relaxed);
                })
        };

        let err = write_capped(counted, &dest, None, "https://example.invalid/archive", 4096)
            .await
            .expect_err("the ceiling must hold");

        match err {
            LoaderError::TooLarge { ceiling, .. } => assert_eq!(ceiling, 4096),
            other => panic!("expected a burst ceiling, got {other:?}"),
        }
        assert_eq!(
            pulled.load(Ordering::Relaxed),
            5,
            "it read one chunk past the ceiling and not the whole megabyte"
        );
        assert!(!dest.exists());
        assert!(!dest.with_extension("gz.part").exists());
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn a_download_that_stays_under_its_ceiling_lands_in_place() {
        let dir = temp_dir("under");
        let dest = dir.join("server.jar");

        let written = write_capped(
            chunked(MOJANG_FIXTURE),
            &dest,
            Some(&Checksum::sha1(MOJANG_SHA1)),
            "the test fixture",
            MOJANG_FIXTURE.len() as u64,
        )
        .await
        .expect("exactly the announced size is still inside the ceiling");

        assert_eq!(written, MOJANG_FIXTURE.len() as u64);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn a_connection_that_breaks_mid_file_leaves_nothing_behind() {
        let dir = temp_dir("interrupted");
        let dest = dir.join("server.jar");
        let stream = futures::stream::iter(vec![
            Ok(&b"the first half"[..]),
            Err(std::io::Error::other("connection reset")),
        ]);

        let err = write_verified(stream, &dest, None, "https://example.invalid/server.jar")
            .await
            .expect_err("a broken stream must fail");

        assert!(matches!(err, LoaderError::Interrupted { .. }), "{err:?}");
        assert!(!dest.exists());
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
