use std::path::Path;

use futures::future::BoxFuture;
use tokio::io::{AsyncReadExt, AsyncSeekExt};

use crate::backups::archive::Progress;

use super::day::Tally;
use super::http::{self, DriveError, Http, Result};
use super::oauth::{Access, Bearer};
use super::retry::Waiting;

pub const CHUNK: u64 = 8 * 1024 * 1024;

const ATTEMPTS: u32 = 5;

const RENEWALS: u32 = 1;

const READ_AT_ONCE: usize = 1024 * 1024;

#[derive(Clone, Default)]
pub struct Digests {
    md5: md5::Md5,
    sha256: sha2::Sha256,
}

impl Digests {
    pub fn take(&mut self, bytes: &[u8]) {
        md5::Digest::update(&mut self.md5, bytes);
        sha2::Digest::update(&mut self.sha256, bytes);
    }

    pub fn md5(&self) -> String {
        hex::encode(md5::Digest::finalize(self.md5.clone()))
    }

    pub fn sha256(&self) -> String {
        hex::encode(sha2::Digest::finalize(self.sha256.clone()))
    }
}

pub struct Prefix {
    pub carried: Digests,
    pub proof: String,
}

pub trait Ledger: Sync {
    fn offered(&self, upto: u64, proof: String) -> BoxFuture<'_, ()>;
}

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
    #[serde(default)]
    pub size: Option<String>,
    #[serde(default, rename = "md5Checksum")]
    pub md5_checksum: Option<String>,
    #[serde(default, rename = "sha256Checksum")]
    pub sha256_checksum: Option<String>,
}

impl Uploaded {
    pub fn bytes(&self) -> Option<u64> {
        super::files::number(self.size.as_deref())
    }
}

#[derive(Debug, Clone)]
pub struct Sent {
    pub file: Uploaded,
    pub md5: String,
    pub sha256: String,
    pub covered: u64,
}

impl Sent {
    pub fn of(file: Uploaded, ours: &Digests, covered: u64) -> Self {
        Self { file, md5: ours.md5(), sha256: ours.sha256(), covered }
    }
}

pub async fn begin(
    http: &Http,
    access: &Access,
    file: &NewFile,
    total: u64,
    over: &Waiting<'_>,
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

    let url = http.api_url(&http::with_query(
        "/upload/drive/v3/files",
        &[("uploadType", "resumable"), ("fields", "id,size,md5Checksum,sha256Checksum")],
    ));
    let response = http
        .send_again(over, || {
            http.client()
                .post(&url)
                .bearer_auth(access.expose())
                .header("X-Upload-Content-Type", super::ARCHIVE_TYPE)
                .header("X-Upload-Content-Length", total.to_string())
                .json(&metadata)
        })
        .await?;

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

pub struct Carry<'a> {
    pub bearer: &'a dyn Bearer,
    pub tally: &'a Tally,
    pub ledger: &'a dyn Ledger,
    pub progress: &'a Progress,
    pub over: Waiting<'a>,
}

pub async fn send(
    http: &Http,
    carry: &Carry<'_>,
    session: &mut String,
    path: &Path,
    total: u64,
    from: u64,
    carried: Digests,
) -> Result<Sent> {
    let mut file = tokio::fs::File::open(path).await.map_err(|err| {
        DriveError::Unreachable(format!("the archive could not be read back: {err}"))
    })?;

    if total == 0 {
        return Err(DriveError::Unreadable("the archive is empty".to_owned()));
    }
    if from > total {
        return Err(DriveError::Unreadable(format!(
            "Google holds {from} bytes of an archive that is only {total} long"
        )));
    }

    let progress = carry.progress;
    let mut offset = from;
    let mut buffer = vec![0u8; CHUNK as usize];
    let mut fruitless = 0u32;
    let mut renewals = 0u32;
    let mut ours = carried;
    progress.add_bytes(from);

    while offset < total {
        if progress.is_cancelled() {
            return Err(DriveError::Cancelled);
        }
        if carry.tally.full() {
            return Err(carry.tally.reached());
        }

        let access = carry.bearer.token().await?;
        let wanted = CHUNK.min(total - offset) as usize;
        file.seek(std::io::SeekFrom::Start(offset)).await.map_err(read_failed)?;
        file.read_exact(&mut buffer[..wanted]).await.map_err(read_failed)?;

        let mut ahead = ours.clone();
        ahead.take(&buffer[..wanted]);
        carry.ledger.offered(offset + wanted as u64, ahead.sha256()).await;

        let answer =
            one_chunk(http, &access, session, &buffer[..wanted], offset, total, &carry.over).await;
        let answer = match answer {
            Ok(answer) => answer,
            Err(DriveError::Revoked(why)) => {
                if renewals >= RENEWALS {
                    return Err(refused_again(why));
                }
                renewals += 1;
                tracing::info!(
                    offset,
                    "Google turned a chunk away as unauthorised; the access token is renewed \
                     once and the same chunk goes again"
                );
                carry.bearer.renew(&access).await?;
                continue;
            }
            Err(err) => return Err(err),
        };

        match answer {
            Chunk::Done(file) => {
                progress.add_bytes(wanted as u64);
                carry.tally.took(wanted as u64);
                return Ok(Sent::of(file, &ahead, offset + wanted as u64));
            }
            Chunk::More { received, moved } => {
                if let Some(fresh) = moved {
                    *session = fresh;
                }
                let sent = offset + wanted as u64;
                if received > sent {
                    return Err(DriveError::Unreadable(format!(
                        "Google claims {received} bytes of the archive when only {sent} have \
                         ever been sent"
                    )));
                }
                if received <= offset {
                    fruitless += 1;
                    if fruitless >= ATTEMPTS {
                        return Err(DriveError::Unreadable(format!(
                            "Google acknowledged nothing of the archive after {fruitless} tries \
                             at offset {offset}"
                        )));
                    }
                    if !carry.over.breathe(fruitless).await {
                        return Err(DriveError::Cancelled);
                    }
                    continue;
                }
                fruitless = 0;
                renewals = 0;
                let taken = (received - offset) as usize;
                if taken == wanted {
                    ours = ahead;
                } else {
                    ours.take(&buffer[..taken]);
                }
                progress.add_bytes(received - offset);
                carry.tally.took(received - offset);
                offset = received;
            }
        }
    }

    let access = carry.bearer.token().await?;
    match status_of(http, &access, session, total, &carry.over).await? {
        Chunk::Done(file) => Ok(Sent::of(file, &ours, total)),
        Chunk::More { .. } => Err(DriveError::Unreadable(
            "Google took every byte of the archive and did not confirm the file".to_owned(),
        )),
    }
}

fn refused_again(why: String) -> DriveError {
    DriveError::Refused {
        status: 401,
        reason: "unauthorized".to_owned(),
        detail: format!(
            "Google turned a chunk away as unauthorised even with a token minted a moment \
             earlier. The connection itself still stands, so this is not something reconnecting \
             fixes. Google said: {why}"
        ),
    }
}

pub async fn standing(
    http: &Http,
    access: &Access,
    session: &str,
    total: u64,
    over: &Waiting<'_>,
) -> Result<Chunk> {
    status_of(http, access, session, total, over).await
}

pub async fn prefix_of(path: &Path, keep: u64, upto: u64) -> Result<Prefix> {
    let mut file = tokio::fs::File::open(path).await.map_err(read_failed)?;
    let mut walked = Digests::default();
    let mut carried = Digests::default();
    let mut proof = walked.sha256();
    let mut buffer = vec![0u8; READ_AT_ONCE];
    let mut at = 0u64;

    while at < keep.max(upto) {
        let next = [keep, upto]
            .into_iter()
            .filter(|stop| *stop > at)
            .min()
            .unwrap_or_else(|| keep.max(upto));
        while at < next {
            let wanted = (next - at).min(buffer.len() as u64) as usize;
            file.read_exact(&mut buffer[..wanted]).await.map_err(read_failed)?;
            walked.take(&buffer[..wanted]);
            at += wanted as u64;
        }
        if at == keep {
            carried = walked.clone();
        }
        if at == upto {
            proof = walked.sha256();
        }
    }

    Ok(Prefix { carried, proof })
}

pub enum Chunk {
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
    over: &Waiting<'_>,
) -> Result<Chunk> {
    let range = format!("bytes {offset}-{}/{total}", offset + bytes.len() as u64 - 1);

    let response = http
        .send_again(over, || {
            http.upload_client()
                .put(session)
                .bearer_auth(access.expose())
                .header(reqwest::header::CONTENT_RANGE, &range)
                .header(reqwest::header::CONTENT_TYPE, super::ARCHIVE_TYPE)
                .body(bytes.to_vec())
        })
        .await?;
    read_answer(response).await
}

async fn status_of(
    http: &Http,
    access: &Access,
    session: &str,
    total: u64,
    over: &Waiting<'_>,
) -> Result<Chunk> {
    let response = http
        .send_again(over, || {
            http.upload_client()
                .put(session)
                .bearer_auth(access.expose())
                .header(reqwest::header::CONTENT_RANGE, format!("bytes */{total}"))
                .header(reqwest::header::CONTENT_LENGTH, "0")
        })
        .await?;
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
        return Err(DriveError::SessionOver);
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

    fn digests_of(bytes: &[u8]) -> Digests {
        let mut digests = Digests::default();
        digests.take(bytes);
        digests
    }

    #[tokio::test]
    async fn one_pass_over_the_archive_answers_both_questions_about_it() {
        let dir = super::super::harness::DataDir::new();
        tokio::fs::create_dir_all(dir.path()).await.expect("a place for the archive");
        let path = dir.path().join("monday.tar.zst");
        let whole: Vec<u8> = (0..5000u32).map(|at| (at % 251) as u8).collect();
        tokio::fs::write(&path, &whole).await.expect("an archive on the disk");

        let prefix = prefix_of(&path, 1000, 4000).await.expect("the prefix");
        assert_eq!(prefix.carried.md5(), digests_of(&whole[..1000]).md5());
        assert_eq!(prefix.carried.sha256(), digests_of(&whole[..1000]).sha256());
        assert_eq!(
            prefix.proof,
            digests_of(&whole[..4000]).sha256(),
            "the proof is about everything that was offered, not about where sending goes on"
        );

        let backwards = prefix_of(&path, 4000, 1000).await.expect("the prefix");
        assert_eq!(backwards.carried.md5(), digests_of(&whole[..4000]).md5());
        assert_eq!(backwards.proof, digests_of(&whole[..1000]).sha256());

        let nothing = prefix_of(&path, 0, 0).await.expect("the prefix");
        assert_eq!(nothing.carried.md5(), Digests::default().md5());
        assert_eq!(nothing.proof, Digests::default().sha256());

        let all = prefix_of(&path, whole.len() as u64, whole.len() as u64).await.expect("all");
        assert_eq!(all.carried.md5(), digests_of(&whole).md5());
        assert_eq!(all.proof, digests_of(&whole).sha256());
    }

    #[test]
    fn a_digest_read_off_halfway_leaves_the_running_one_alone() {
        let mut running = Digests::default();
        running.take(b"half a world");
        let halfway = running.md5();
        running.take(b" and the other half");

        assert_eq!(halfway, digests_of(b"half a world").md5());
        assert_eq!(running.md5(), digests_of(b"half a world and the other half").md5());
        assert_eq!(running.sha256(), digests_of(b"half a world and the other half").sha256());
    }
}
