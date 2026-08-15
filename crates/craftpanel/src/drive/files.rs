use std::path::Path;

use md5::Digest;
use tokio::io::AsyncWriteExt;

use crate::backups::archive::Progress;

use super::http::{self, DriveError, Http, Result};
use super::oauth::Access;
use super::store::Who;

const FOLDER_TYPE: &str = "application/vnd.google-apps.folder";

pub async fn about(http: &Http, access: &Access) -> Result<Who> {
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

    let response = http
        .client()
        .get(http.api_url(&http::with_query(
            "/drive/v3/about",
            &[("fields", "user(displayName,emailAddress),storageQuota(limit,usage)")],
        )))
        .bearer_auth(access.expose())
        .send()
        .await
        .map_err(http::unreachable)?;

    let about: About = http::read_api(response).await?;
    Ok(Who {
        name: about.user.as_ref().and_then(|user| user.display_name.clone()),
        email: about.user.as_ref().and_then(|user| user.email.clone()),
        limit_bytes: about.quota.as_ref().and_then(|quota| number(quota.limit.as_deref())),
        usage_bytes: about.quota.as_ref().and_then(|quota| number(quota.usage.as_deref())),
    })
}

pub async fn ensure_folder(http: &Http, access: &Access, name: &str) -> Result<String> {
    if let Some(id) = find_folder(http, access, name).await? {
        return Ok(id);
    }

    #[derive(serde::Deserialize)]
    struct Made {
        id: String,
    }

    let response = http
        .client()
        .post(http.api_url(&http::with_query("/drive/v3/files", &[("fields", "id")])))
        .bearer_auth(access.expose())
        .json(&serde_json::json!({
            "name": name,
            "mimeType": FOLDER_TYPE,
            "appProperties": { "panel": super::PANEL_TAG },
        }))
        .send()
        .await
        .map_err(http::unreachable)?;

    let made: Made = http::read_api(response).await?;
    Ok(made.id)
}

async fn find_folder(http: &Http, access: &Access, name: &str) -> Result<Option<String>> {
    let query = format!(
        "mimeType = '{FOLDER_TYPE}' and name = '{}' and trashed = false \
         and appProperties has {{ key='panel' and value='{}' }}",
        escape(name),
        super::PANEL_TAG
    );
    let listed = list(http, access, &query, "files(id)").await?;
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

pub async fn ours(http: &Http, access: &Access) -> Result<Vec<File>> {
    let query = format!(
        "appProperties has {{ key='panel' and value='{}' }}",
        super::PANEL_TAG
    );
    list(
        http,
        access,
        &query,
        "nextPageToken,files(id,name,size,trashed,md5Checksum,appProperties)",
    )
    .await
}

async fn list(http: &Http, access: &Access, query: &str, fields: &str) -> Result<Vec<File>> {
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
        let request = http
            .client()
            .get(http.api_url(&http::with_query("/drive/v3/files", &fields)))
            .bearer_auth(access.expose());

        let page: Page =
            http::read_api(request.send().await.map_err(http::unreachable)?).await?;
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

pub async fn get(http: &Http, access: &Access, id: &str) -> Result<File> {
    let response = http
        .client()
        .get(http.api_url(&http::with_query(
            &format!("/drive/v3/files/{}", segment(id)),
            &[("fields", "id,name,size,trashed,md5Checksum")],
        )))
        .bearer_auth(access.expose())
        .send()
        .await
        .map_err(http::unreachable)?;
    http::read_api(response).await
}

pub async fn delete(http: &Http, access: &Access, id: &str) -> Result<()> {
    let response = http
        .client()
        .delete(http.api_url(&format!("/drive/v3/files/{}", segment(id))))
        .bearer_auth(access.expose())
        .send()
        .await
        .map_err(http::unreachable)?;

    let status = response.status().as_u16();
    if (200..300).contains(&status) || status == 404 {
        return Ok(());
    }
    let body = response.bytes().await.map_err(http::unreachable)?;
    Err(http::api_refusal(status, &body))
}

pub async fn download(
    http: &Http,
    access: &Access,
    id: &str,
    into: &Path,
    progress: &Progress,
) -> Result<String> {
    use futures::StreamExt;

    let response = http
        .upload_client()
        .get(http.api_url(&http::with_query(
            &format!("/drive/v3/files/{}", segment(id)),
            &[("alt", "media")],
        )))
        .bearer_auth(access.expose())
        .send()
        .await
        .map_err(http::unreachable)?;

    let status = response.status().as_u16();
    if !(200..300).contains(&status) {
        let body = response.bytes().await.map_err(http::unreachable)?;
        return Err(http::api_refusal(status, &body));
    }

    if let Some(parent) = into.parent() {
        tokio::fs::create_dir_all(parent).await.map_err(write_failed)?;
    }
    let mut file = tokio::fs::File::create(into).await.map_err(write_failed)?;
    let mut digest = md5::Md5::new();
    let mut stream = response.bytes_stream();

    while let Some(piece) = stream.next().await {
        if progress.is_cancelled() {
            drop(file);
            tokio::fs::remove_file(into).await.ok();
            return Err(DriveError::Cancelled);
        }
        let piece = piece.map_err(http::unreachable)?;
        digest.update(&piece);
        file.write_all(&piece).await.map_err(write_failed)?;
        progress.add_bytes(piece.len() as u64);
    }
    file.sync_all().await.map_err(write_failed)?;

    Ok(hex::encode(digest.finalize()))
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

fn number(text: Option<&str>) -> Option<u64> {
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
