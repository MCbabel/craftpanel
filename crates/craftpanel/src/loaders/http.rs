use std::time::Duration;

use serde::de::DeserializeOwned;

use super::error::{LoaderError, Result};

const AGENT: &str = concat!("craftpanel/", env!("CARGO_PKG_VERSION"));
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const READ_TIMEOUT: Duration = Duration::from_secs(30);
const METADATA_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Clone)]
pub struct Http {
    client: reqwest::Client,
}

impl Http {
    pub fn new() -> Result<Self> {
        let client = reqwest::Client::builder()
            .user_agent(AGENT)
            .connect_timeout(CONNECT_TIMEOUT)
            .read_timeout(READ_TIMEOUT)
            .build()
            .map_err(|err| LoaderError::Setup(err.to_string()))?;
        Ok(Self { client })
    }

    pub async fn fetch(&self, service: &'static str, url: &str) -> Result<Vec<u8>> {
        self.maybe_fetch(service, url)
            .await?
            .ok_or_else(|| LoaderError::Refused {
                service,
                status: 404,
                detail: format!("nothing at {url}"),
            })
    }

    pub async fn maybe_fetch(&self, service: &'static str, url: &str) -> Result<Option<Vec<u8>>> {
        let response = self
            .client
            .get(url)
            .timeout(METADATA_TIMEOUT)
            .send()
            .await
            .map_err(|err| unreachable(service, err))?;

        if response.status() == 404 {
            return Ok(None);
        }
        if !response.status().is_success() {
            return Err(refused(service, response).await);
        }

        response
            .bytes()
            .await
            .map(|body| Some(body.to_vec()))
            .map_err(|err| unreachable(service, err))
    }

    pub async fn stream(&self, service: &'static str, url: &str) -> Result<reqwest::Response> {
        let response = self
            .client
            .get(url)
            .send()
            .await
            .map_err(|err| unreachable(service, err))?;

        if !response.status().is_success() {
            return Err(refused(service, response).await);
        }
        Ok(response)
    }
}

pub fn parse<T: DeserializeOwned>(service: &'static str, body: &[u8]) -> Result<T> {
    serde_json::from_slice(body)
        .map_err(|err| LoaderError::Unreadable { service, reason: err.to_string() })
}

fn unreachable(service: &'static str, err: reqwest::Error) -> LoaderError {
    let reason = if err.is_timeout() {
        "it did not answer in time".to_owned()
    } else if err.is_connect() {
        "the connection could not be opened".to_owned()
    } else {
        err.to_string()
    };
    LoaderError::Unreachable { service, reason }
}

async fn refused(service: &'static str, response: reqwest::Response) -> LoaderError {
    let status = response.status().as_u16();
    let body = response.text().await.unwrap_or_default();
    LoaderError::Refused { service, status, detail: detail(&body) }
}

fn detail(body: &str) -> String {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|value| {
            ["message", "error"]
                .iter()
                .find_map(|key| value.get(key)?.as_str().map(str::to_owned))
        })
        .unwrap_or_else(|| body.chars().take(120).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_rejection_keeps_the_sentence_the_service_sent() {
        let body = r#"{"ok":false,"error":"version_not_found","message":"No version was found with the given identifier."}"#;
        assert_eq!(detail(body), "No version was found with the given identifier.");
        assert_eq!(detail(r#"{"error":"version not found"}"#), "version not found");
        assert_eq!(detail("<html>gateway timeout</html>"), "<html>gateway timeout</html>");
    }

    #[test]
    fn the_agent_names_us_with_our_version() {
        assert!(AGENT.starts_with("craftpanel/0.1"), "{AGENT}");
    }
}
