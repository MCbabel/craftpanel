use std::path::Path;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncSeekExt};

use crate::backups::archive::Progress;

use super::http::{self, DriveError, Http, Result};
use super::oauth::Access;

pub const CHUNK: u64 = 8 * 1024 * 1024;

const ATTEMPTS: u32 = 5;

const BACKOFF_CEILING: Duration = Duration::from_secs(64);

#[derive(Debug, Clone)]
pub struct NewFile {
    pub name: String,
    pub parent: Option<String>,
    pub server_id: String,
    pub backup_id: String,
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct Uploaded {
    pub id: String,
    #[serde(default, rename = "md5Checksum")]
    pub md5_checksum: Option<String>,
}

pub async fn begin(
    http: &Http,
    access: &Access,
    file: &NewFile,
    total: u64,
) -> Result<String> {
    let metadata = serde_json::json!({
        "name": file.name,
        "parents": file.parent.as_ref().map(|id| vec![id]).unwrap_or_default(),
        "appProperties": {
            "panel": super::PANEL_TAG,
            "server_id": file.server_id,
            "backup_id": file.backup_id,
        },
    });

    let response = http
        .client()
        .post(http.api_url("/upload/drive/v3/files?uploadType=resumable"))
        .bearer_auth(access.expose())
        .header("X-Upload-Content-Type", super::ARCHIVE_TYPE)
        .header("X-Upload-Content-Length", total.to_string())
        .json(&metadata)
        .send()
        .await
        .map_err(http::unreachable)?;

    let status = response.status().as_u16();
    let session = response
        .headers()
        .get(reqwest::header::LOCATION)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);

    if !(200..300).contains(&status) {
        let body = response.bytes().await.map_err(http::unreachable)?;
        return Err(http::api_refusal(status, &body));
    }
    session.ok_or_else(|| {
        DriveError::Unreadable("Google opened an upload session without giving its address".to_owned())
    })
}

pub async fn send(
    http: &Http,
    access: &Access,
    session: &str,
    path: &Path,
    total: u64,
    progress: &Progress,
) -> Result<Uploaded> {
    let mut file = tokio::fs::File::open(path).await.map_err(|err| {
        DriveError::Unreachable(format!("the archive could not be read back: {err}"))
    })?;
    let mut session = session.to_owned();
    let mut offset = 0u64;
    let mut buffer = vec![0u8; CHUNK as usize];
    let mut fruitless = 0u32;

    if total == 0 {
        return Err(DriveError::Unreadable("the archive is empty".to_owned()));
    }

    while offset < total {
        if progress.is_cancelled() {
            return Err(DriveError::Cancelled);
        }

        let wanted = CHUNK.min(total - offset) as usize;
        file.seek(std::io::SeekFrom::Start(offset)).await.map_err(read_failed)?;
        file.read_exact(&mut buffer[..wanted]).await.map_err(read_failed)?;

        match one_chunk(http, access, &session, &buffer[..wanted], offset, total).await? {
            Chunk::Done(uploaded) => {
                progress.add_bytes(wanted as u64);
                return Ok(uploaded);
            }
            Chunk::More { received, moved } => {
                if let Some(fresh) = moved {
                    session = fresh;
                }
                if received <= offset {
                    fruitless += 1;
                    if fruitless >= ATTEMPTS {
                        return Err(DriveError::Unreadable(format!(
                            "Google acknowledged nothing of the archive after {fruitless} tries \
                             at offset {offset}"
                        )));
                    }
                    tokio::time::sleep(backoff(fruitless)).await;
                    continue;
                }
                fruitless = 0;
                progress.add_bytes(received - offset);
                offset = received;
            }
        }
    }

    match status_of(http, access, &session, total).await? {
        Chunk::Done(uploaded) => Ok(uploaded),
        Chunk::More { .. } => Err(DriveError::Unreadable(
            "Google took every byte of the archive and did not confirm the file".to_owned(),
        )),
    }
}

enum Chunk {
    Done(Uploaded),
    More { received: u64, moved: Option<String> },
}

async fn one_chunk(
    http: &Http,
    access: &Access,
    session: &str,
    bytes: &[u8],
    offset: u64,
    total: u64,
) -> Result<Chunk> {
    let range = format!("bytes {offset}-{}/{total}", offset + bytes.len() as u64 - 1);
    let mut attempt = 0;

    loop {
        let response = http
            .upload_client()
            .put(session)
            .bearer_auth(access.expose())
            .header(reqwest::header::CONTENT_RANGE, &range)
            .header(reqwest::header::CONTENT_TYPE, super::ARCHIVE_TYPE)
            .body(bytes.to_vec())
            .send()
            .await;

        let outcome = match response {
            Ok(response) => read_answer(response).await,
            Err(err) => Err(http::unreachable(err)),
        };

        match outcome {
            Ok(chunk) => return Ok(chunk),
            Err(err) if err.is_worth_repeating() && attempt + 1 < ATTEMPTS => {
                attempt += 1;
                tracing::warn!(%range, attempt, "a chunk did not go up: {err}");
                tokio::time::sleep(backoff(attempt)).await;
            }
            Err(err) => return Err(err),
        }
    }
}

async fn status_of(http: &Http, access: &Access, session: &str, total: u64) -> Result<Chunk> {
    let response = http
        .client()
        .put(session)
        .bearer_auth(access.expose())
        .header(reqwest::header::CONTENT_RANGE, format!("bytes */{total}"))
        .header(reqwest::header::CONTENT_LENGTH, "0")
        .send()
        .await
        .map_err(http::unreachable)?;
    read_answer(response).await
}

async fn read_answer(response: reqwest::Response) -> Result<Chunk> {
    let status = response.status().as_u16();
    let moved = response
        .headers()
        .get(reqwest::header::LOCATION)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let range = response
        .headers()
        .get(reqwest::header::RANGE)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);

    if status == 308 {
        return Ok(Chunk::More { received: after(range.as_deref()), moved });
    }
    if status == 404 {
        return Err(DriveError::Gone);
    }

    let body = response.bytes().await.map_err(http::unreachable)?;
    if !(200..300).contains(&status) {
        return Err(http::api_refusal(status, &body));
    }

    let uploaded: Uploaded = serde_json::from_slice(&body)
        .map_err(|err| DriveError::Unreadable(http::shorten(&err.to_string())))?;
    Ok(Chunk::Done(uploaded))
}

fn after(range: Option<&str>) -> u64 {
    let Some(range) = range else { return 0 };
    let Some((_, span)) = range.split_once('=') else { return 0 };
    let Some((_, last)) = span.rsplit_once('-') else { return 0 };
    last.trim().parse::<u64>().map(|end| end + 1).unwrap_or(0)
}

fn backoff(attempt: u32) -> Duration {
    let base = Duration::from_secs(1u64 << attempt.min(6));
    let jitter = Duration::from_millis(u64::from(u16::from_le_bytes(rand::random())) % 1_000);
    (base + jitter).min(BACKOFF_CEILING)
}

fn read_failed(err: std::io::Error) -> DriveError {
    DriveError::Unreachable(format!("the archive could not be read back: {err}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_range_header_says_what_arrived_not_where_to_go() {
        assert_eq!(after(Some("bytes=0-8388607")), 8_388_608);
        assert_eq!(after(Some("bytes=0-0")), 1);
        assert_eq!(after(None), 0, "an absent Range must not read as 'everything'");
        assert_eq!(after(Some("nonsense")), 0);
        assert_eq!(after(Some("bytes=*")), 0);
    }

    #[test]
    fn the_chunk_is_a_multiple_of_the_256_kib_google_demands() {
        assert_eq!(CHUNK % (256 * 1024), 0);
        assert_eq!(CHUNK, 8 * 1024 * 1024);
    }

    #[test]
    fn the_backoff_climbs_and_then_stops_climbing() {
        for attempt in 1..=4 {
            let waited = backoff(attempt);
            assert!(
                waited >= Duration::from_secs(1u64 << attempt),
                "attempt {attempt} waited {waited:?}"
            );
            assert!(waited <= BACKOFF_CEILING);
        }
        assert_eq!(backoff(20).min(BACKOFF_CEILING), BACKOFF_CEILING);
    }
}
