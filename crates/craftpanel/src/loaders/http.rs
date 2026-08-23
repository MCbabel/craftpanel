use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use reqwest::redirect;
use serde::de::DeserializeOwned;

use super::error::{LoaderError, Result};

const AGENT: &str = concat!("craftpanel/", env!("CARGO_PKG_VERSION"));
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const READ_TIMEOUT: Duration = Duration::from_secs(30);
const METADATA_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Clone)]
pub struct Http {
    client: reqwest::Client,
    allowed: Option<Arc<Vec<String>>>,
}

impl Http {
    pub fn new() -> Result<Self> {
        Self::built(None)
    }

    pub fn bound_to(origins: Vec<String>) -> Result<Self> {
        Self::built(Some(Arc::new(origins)))
    }

    fn built(allowed: Option<Arc<Vec<String>>>) -> Result<Self> {
        let mut builder = reqwest::Client::builder()
            .user_agent(AGENT)
            .connect_timeout(CONNECT_TIMEOUT)
            .read_timeout(READ_TIMEOUT);

        if let Some(allowed) = allowed.clone() {
            builder = builder.redirect(redirect::Policy::custom(move |attempt| {
                if admits(&allowed, attempt.url()) {
                    redirect::Policy::default().redirect(attempt)
                } else {
                    let detour = Detour(origin_of(attempt.url()));
                    attempt.error(detour)
                }
            }));
        }

        let client = builder.build().map_err(|err| LoaderError::Setup(err.to_string()))?;
        Ok(Self { client, allowed })
    }

    fn admitted(&self, service: &'static str, url: &str) -> Result<()> {
        let Some(allowed) = self.allowed.as_deref() else {
            return Ok(());
        };
        match reqwest::Url::parse(url) {
            Ok(parsed) if admits(allowed, &parsed) => Ok(()),
            Ok(parsed) => Err(LoaderError::Untrusted { service, origin: origin_of(&parsed) }),
            Err(_) => Err(LoaderError::Untrusted { service, origin: url.to_owned() }),
        }
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
        self.admitted(service, url)?;
        let response = self
            .client
            .get(url)
            .timeout(METADATA_TIMEOUT)
            .send()
            .await
            .map_err(|err| turned_away(service, err))?;

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
        self.admitted(service, url)?;
        let response = self
            .client
            .get(url)
            .send()
            .await
            .map_err(|err| turned_away(service, err))?;

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

fn origin_of(url: &reqwest::Url) -> String {
    url.origin().ascii_serialization()
}

fn admits(allowed: &[String], url: &reqwest::Url) -> bool {
    let origin = origin_of(url);
    allowed.iter().any(|named| *named == origin)
}

#[derive(Debug)]
struct Detour(String);

impl fmt::Display for Detour {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "the redirect leads to {}", self.0)
    }
}

impl std::error::Error for Detour {}

fn turned_away(service: &'static str, err: reqwest::Error) -> LoaderError {
    match detour(&err) {
        Some(origin) => LoaderError::Untrusted { service, origin },
        None => unreachable(service, err),
    }
}

fn detour(err: &reqwest::Error) -> Option<String> {
    let mut cause = std::error::Error::source(err);
    while let Some(step) = cause {
        if let Some(Detour(origin)) = step.downcast_ref::<Detour>() {
            return Some(origin.clone());
        }
        cause = step.source();
    }
    None
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
    fn a_bound_client_weighs_the_scheme_the_host_and_the_port_together() {
        let allowed = vec!["https://github.com".to_owned(), "http://127.0.0.1:8080".to_owned()];
        let admitted =
            |url: &str| admits(&allowed, &reqwest::Url::parse(url).expect("a readable url"));

        assert!(admitted("https://github.com/adoptium/temurin21-binaries/releases/x.tar.gz"));
        assert!(!admitted("http://github.com/adoptium/x.tar.gz"), "http is no https");
        assert!(!admitted("https://github.com.example.invalid/x.tar.gz"), "a longer name");
        assert!(!admitted("https://raw.githubusercontent.com/x.tar.gz"), "another host");
        assert!(!admitted("file:///etc/shadow"), "no scheme but https and what was named");
        assert!(admitted("http://127.0.0.1:8080/binaries/x.tar.gz"));
        assert!(!admitted("http://127.0.0.1:9090/binaries/x.tar.gz"), "another port");
    }

    #[test]
    fn the_client_the_loaders_share_is_bound_to_nothing() {
        let http = Http::new().expect("a client");

        assert!(http.admitted("Modrinth", "https://cdn.modrinth.com/data/x.jar").is_ok());
        assert!(http.admitted("PaperMC", "http://fill-data.papermc.io/x.jar").is_ok());
    }

    #[test]
    fn a_bound_client_names_the_place_it_will_not_go() {
        let http = Http::bound_to(vec!["https://api.adoptium.net".to_owned()]).expect("a client");

        let refusal = http
            .admitted("Adoptium", "https://evil.example.invalid/OpenJDK.tar.gz")
            .expect_err("a strange host must be refused");
        assert_eq!(
            refusal.to_string(),
            "Adoptium sent us to https://evil.example.invalid, which is not one of the hosts \
             its downloads come from"
        );
        assert!(http.admitted("Adoptium", "https://api.adoptium.net/v3/assets").is_ok());
    }

    #[test]
    fn the_agent_names_us_with_our_version() {
        assert!(AGENT.starts_with("craftpanel/0.1"), "{AGENT}");
    }
}
