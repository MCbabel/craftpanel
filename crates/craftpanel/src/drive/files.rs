use std::path::Path;

use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};

use crate::backups::archive::Progress;

use super::http::{self, DriveError, Http, Result};
use super::oauth::Access;
use super::retry::Waiting;
use super::store::Who;

const FOLDER_TYPE: &str = "application/vnd.google-apps.folder";

pub async fn about(http: &Http, access: &Access, over: &Waiting<'_>) -> Result<Who> {
    #[derive(serde::Deserialize)]
    struct About {
        #[serde(default)]
        user: Option<User>,
        #[serde(default, rename = "storageQuota")]
        quota: Option<Quota>,
    }
    #[derive(serde::Deserialize)]
    struct User {
        #[serde(default, rename = "displayName")]
        display_name: Option<String>,
        #[serde(default, rename = "emailAddress")]
        email: Option<String>,
    }
    #[derive(serde::Deserialize)]
    struct Quota {
        #[serde(default)]
        limit: Option<String>,
        #[serde(default)]
        usage: Option<String>,
    }

    let url = http.api_url(&http::with_query(
        "/drive/v3/about",
        &[("fields", "user(displayName,emailAddress),storageQuota(limit,usage)")],
    ));
    let response =
        http.send_again(over, || http.client().get(&url).bearer_auth(access.expose())).await?;

    let about: About = http::read_api(response).await?;
    Ok(Who {
        name: about.user.as_ref().and_then(|user| user.display_name.clone()),
        email: about.user.as_ref().and_then(|user| user.email.clone()),
        limit_bytes: about.quota.as_ref().and_then(|quota| number(quota.limit.as_deref())),
        usage_bytes: about.quota.as_ref().and_then(|quota| number(quota.usage.as_deref())),
    })
}

pub async fn ensure_folder(
    http: &Http,
    access: &Access,
    name: &str,
    over: &Waiting<'_>,
) -> Result<String> {
    if let Some(id) = find_folder(http, access, name, over).await? {
        return Ok(id);
    }

    #[derive(serde::Deserialize)]
    struct Made {
        id: String,
    }

    let url = http.api_url(&http::with_query("/drive/v3/files", &[("fields", "id")]));
    let wanted = serde_json::json!({
        "name": name,
        "mimeType": FOLDER_TYPE,
        "appProperties": { "panel": super::PANEL_TAG },
    });
    let response = http
        .send_again(over, || {
            http.client().post(&url).bearer_auth(access.expose()).json(&wanted)
        })
        .await?;

    let made: Made = http::read_api(response).await?;
    Ok(made.id)
}

async fn find_folder(
    http: &Http,
    access: &Access,
    name: &str,
    over: &Waiting<'_>,
) -> Result<Option<String>> {
    let query = format!(
        "mimeType = '{FOLDER_TYPE}' and name = '{}' and trashed = false \
         and appProperties has {{ key='panel' and value='{}' }}",
        escape(name),
        super::PANEL_TAG
    );
    let listed = list(http, access, &query, "files(id)", over).await?;
    Ok(listed.into_iter().next().map(|file| file.id))
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct File {
    pub id: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub trashed: bool,
    #[serde(default)]
    pub size: Option<String>,
    #[serde(default, rename = "md5Checksum")]
    pub md5_checksum: Option<String>,
    #[serde(default, rename = "sha256Checksum")]
    pub sha256_checksum: Option<String>,
    #[serde(default, rename = "isAppAuthorized")]
    pub is_app_authorized: Option<bool>,
    #[serde(default, rename = "appProperties")]
    pub app_properties: Option<Properties>,
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct Properties {
    #[serde(default)]
    pub panel: Option<String>,
    #[serde(default)]
    pub server_id: Option<String>,
    #[serde(default)]
    pub backup_id: Option<String>,
}

impl File {
    pub fn backup_id(&self) -> Option<&str> {
        self.app_properties.as_ref()?.backup_id.as_deref()
    }

    pub fn bytes(&self) -> Option<u64> {
        number(self.size.as_deref())
    }
}

pub async fn ours(http: &Http, access: &Access, over: &Waiting<'_>) -> Result<Vec<File>> {
    let query = format!(
        "appProperties has {{ key='panel' and value='{}' }}",
        super::PANEL_TAG
    );
    list(
        http,
        access,
        &query,
        "nextPageToken,files(id,name,size,trashed,md5Checksum,appProperties)",
        over,
    )
    .await
}

async fn list(
    http: &Http,
    access: &Access,
    query: &str,
    fields: &str,
    over: &Waiting<'_>,
) -> Result<Vec<File>> {
    #[derive(serde::Deserialize)]
    struct Page {
        #[serde(default)]
        files: Vec<File>,
        #[serde(default, rename = "nextPageToken")]
        next: Option<String>,
    }

    let mut found = Vec::new();
    let mut token: Option<String> = None;
    for _ in 0..50 {
        let mut fields = vec![("q", query), ("fields", fields), ("pageSize", "100")];
        if let Some(token) = token.as_deref() {
            fields.push(("pageToken", token));
        }
        let url = http.api_url(&http::with_query("/drive/v3/files", &fields));
        let response =
            http.send_again(over, || http.client().get(&url).bearer_auth(access.expose())).await?;

        let page: Page = http::read_api(response).await?;
        found.extend(page.files);
        match page.next {
            Some(next) => token = Some(next),
            None => return Ok(found),
        }
    }
    Err(DriveError::Unreadable(
        "Google keeps handing out more pages of files than we asked to see".to_owned(),
    ))
}

pub async fn get(http: &Http, access: &Access, id: &str, over: &Waiting<'_>) -> Result<File> {
    let url = http.api_url(&http::with_query(
        &format!("/drive/v3/files/{}", segment(id)),
        &[(
            "fields",
            "id,name,size,trashed,md5Checksum,sha256Checksum,isAppAuthorized",
        )],
    ));
    let response =
        http.send_again(over, || http.client().get(&url).bearer_auth(access.expose())).await?;
    http::read_api(response).await
}

pub async fn delete(http: &Http, access: &Access, id: &str, over: &Waiting<'_>) -> Result<()> {
    let url = http.api_url(&format!("/drive/v3/files/{}", segment(id)));
    let response = http
        .send_again(over, || http.client().delete(&url).bearer_auth(access.expose()))
        .await?;

    let status = response.status().as_u16();
    if (200..300).contains(&status) || status == 404 {
        return Ok(());
    }
    let body = response.bytes().await.map_err(http::unreachable)?;
    Err(http::api_refusal(status, &body))
}

#[derive(Debug, Clone, Copy)]
pub struct Fetch<'a> {
    pub id: &'a str,
    pub into: &'a Path,
    pub from: u64,
    pub acknowledge_abuse: bool,
}

#[derive(Debug, Clone)]
pub struct Fetched {
    pub md5: String,
    pub sha256: String,
}

impl Fetched {
    pub fn holds(&self, file: &File) -> Option<bool> {
        if let Some(theirs) = file.md5_checksum.as_deref() {
            return Some(theirs.trim().eq_ignore_ascii_case(&self.md5));
        }
        let theirs = file.sha256_checksum.as_deref()?;
        Some(theirs.trim().eq_ignore_ascii_case(&self.sha256))
    }
}

struct Both {
    md5: md5::Md5,
    sha256: sha2::Sha256,
}

impl Both {
    fn new() -> Self {
        Self {
            md5: <md5::Md5 as md5::Digest>::new(),
            sha256: <sha2::Sha256 as sha2::Digest>::new(),
        }
    }

    fn update(&mut self, bytes: &[u8]) {
        md5::Digest::update(&mut self.md5, bytes);
        sha2::Digest::update(&mut self.sha256, bytes);
    }

    fn finish(self) -> Fetched {
        Fetched {
            md5: hex::encode(md5::Digest::finalize(self.md5)),
            sha256: hex::encode(sha2::Digest::finalize(self.sha256)),
        }
    }

    async fn take_in(&mut self, file: &mut tokio::fs::File, upto: u64) -> Result<()> {
        file.seek(std::io::SeekFrom::Start(0)).await.map_err(write_failed)?;
        let mut buffer = vec![0u8; 1024 * 1024];
        let mut left = upto;
        while left > 0 {
            let wanted = left.min(buffer.len() as u64) as usize;
            file.read_exact(&mut buffer[..wanted]).await.map_err(write_failed)?;
            self.update(&buffer[..wanted]);
            left -= wanted as u64;
        }
        Ok(())
    }
}

pub async fn download(
    http: &Http,
    access: &Access,
    fetch: Fetch<'_>,
    progress: &Progress,
    over: &Waiting<'_>,
) -> Result<Fetched> {
    use futures::StreamExt;

    let mut query = vec![("alt", "media")];
    if fetch.acknowledge_abuse {
        query.push(("acknowledgeAbuse", "true"));
    }
    let url = http
        .api_url(&http::with_query(&format!("/drive/v3/files/{}", segment(fetch.id)), &query));
    let response = http
        .send_again(over, || {
            let asking = http.upload_client().get(&url).bearer_auth(access.expose());
            match fetch.from {
                0 => asking,
                from => asking.header(reqwest::header::RANGE, format!("bytes={from}-")),
            }
        })
        .await?;

    let status = response.status().as_u16();
    if !(200..300).contains(&status) {
        let body = response.bytes().await.map_err(http::unreachable)?;
        return Err(http::api_refusal(status, &body));
    }

    if let Some(parent) = fetch.into.parent() {
        tokio::fs::create_dir_all(parent).await.map_err(write_failed)?;
    }
    let carrying_on = fetch.from > 0 && status == 206;
    let mut digests = Both::new();
    let mut file = if carrying_on {
        let mut open = tokio::fs::File::options()
            .read(true)
            .write(true)
            .open(fetch.into)
            .await
            .map_err(write_failed)?;
        open.set_len(fetch.from).await.map_err(write_failed)?;
        digests.take_in(&mut open, fetch.from).await?;
        open.seek(std::io::SeekFrom::Start(fetch.from)).await.map_err(write_failed)?;
        progress.add_bytes(fetch.from);
        open
    } else {
        tokio::fs::File::create(fetch.into).await.map_err(write_failed)?
    };
    let mut stream = response.bytes_stream();

    while let Some(piece) = stream.next().await {
        if progress.is_cancelled() {
            file.flush().await.ok();
            return Err(DriveError::Cancelled);
        }
        let piece = match piece {
            Ok(piece) => piece,
            Err(err) => {
                file.flush().await.ok();
                file.sync_all().await.ok();
                return Err(http::unreachable(err));
            }
        };
        digests.update(&piece);
        file.write_all(&piece).await.map_err(write_failed)?;
        progress.add_bytes(piece.len() as u64);
    }
    file.sync_all().await.map_err(write_failed)?;

    Ok(digests.finish())
}

fn write_failed(err: std::io::Error) -> DriveError {
    DriveError::Unreachable(format!("the archive could not be written to disk: {err}"))
}

pub fn web_link(id: &str) -> String {
    format!("https://drive.google.com/file/d/{id}/view")
}

fn escape(text: &str) -> String {
    text.replace('\\', "\\\\").replace('\'', "\\'")
}

fn segment(id: &str) -> String {
    id.chars()
        .flat_map(|letter| {
            if letter.is_ascii_alphanumeric() || matches!(letter, '-' | '_' | '.') {
                vec![letter]
            } else {
                format!("%{:02X}", letter as u32 as u8).chars().collect()
            }
        })
        .collect()
}

pub(super) fn number(text: Option<&str>) -> Option<u64> {
    text?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_storage_figures_arrive_as_strings() {
        #[derive(serde::Deserialize)]
        struct About {
            #[serde(rename = "storageQuota")]
            quota: Quota,
        }
        #[derive(serde::Deserialize)]
        struct Quota {
            limit: String,
            usage: String,
        }

        let about: About =
            serde_json::from_slice(include_bytes!("testdata/about.json")).expect("json");
        assert_eq!(about.quota.limit, "16106127360");
        assert_eq!(number(Some(&about.quota.limit)), Some(16_106_127_360));
        assert_eq!(number(Some(&about.quota.usage)), Some(2_147_483_648));
        assert_eq!(number(None), None, "no limit is a Workspace account, not zero");
        assert_eq!(number(Some("")), None);
    }

    #[test]
    fn an_apostrophe_in_a_folder_name_cannot_break_out_of_the_query() {
        assert_eq!(escape("anna's backups"), r"anna\'s backups");
        assert_eq!(escape(r"back\slash"), r"back\\slash");
        assert_eq!(escape("plain"), "plain");
    }

    #[test]
    fn a_file_id_is_encoded_before_it_goes_into_a_path() {
        assert_eq!(segment("1a2B-_c.d"), "1a2B-_c.d");
        assert_eq!(segment("../etc/passwd"), "..%2Fetc%2Fpasswd");
        assert_eq!(segment("a b"), "a%20b");
    }

    #[test]
    fn the_web_link_is_the_one_the_owner_can_open() {
        assert_eq!(web_link("abc123"), "https://drive.google.com/file/d/abc123/view");
    }
}
