use std::time::Duration;

use serde::de::DeserializeOwned;

pub const OAUTH: &str = "https://oauth2.googleapis.com";
pub const API: &str = "https://www.googleapis.com";

const AGENT: &str = concat!("craftpanel/", env!("CARGO_PKG_VERSION"));
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const CALL_TIMEOUT: Duration = Duration::from_secs(20);
const CHUNK_TIMEOUT: Duration = Duration::from_secs(5 * 60);

#[derive(Debug, thiserror::Error)]
pub enum DriveError {
    #[error("Google could not be reached: {0}")]
    Unreachable(String),

    #[error("Google no longer accepts the connection: {0}")]
    Revoked(String),

    #[error("the Google Drive of this account is full: {0}")]
    QuotaFull(String),

    #[error("Google is turning us away for the moment")]
    RateLimited,

    #[error("nobody has confirmed the code yet")]
    Pending { slow_down: bool },

    #[error("the request was declined at google.com/device")]
    Denied,

    #[error("the code expired before it was confirmed")]
    Expired,

    #[error("Google does not know that any more")]
    Gone,

    #[error("Google refused the call: {detail}")]
    Refused { status: u16, reason: String, detail: String },

    #[error("the run was called off")]
    Cancelled,

    #[error("Google answered in a shape we do not understand: {0}")]
    Unreadable(String),

    #[error("the HTTP client could not be set up: {0}")]
    Setup(String),
}

impl DriveError {
    pub fn is_revoked(&self) -> bool {
        matches!(self, Self::Revoked(_))
    }

    pub fn is_worth_repeating(&self) -> bool {
        match self {
            Self::Unreachable(_) | Self::RateLimited => true,
            Self::Refused { status, .. } => *status >= 500,
            _ => false,
        }
    }

    pub fn operation_code(&self) -> &'static str {
        match self {
            Self::Revoked(_) => "drive_revoked",
            Self::QuotaFull(_) => "drive_quota_exceeded",
            Self::Gone => "drive_file_missing",
            _ => "drive_unavailable",
        }
    }
}

pub type Result<T> = std::result::Result<T, DriveError>;

#[derive(Clone)]
pub struct Http {
    client: reqwest::Client,
    uploads: reqwest::Client,
    oauth: String,
    api: String,
}

impl Http {
    pub fn against(oauth: impl Into<String>, api: impl Into<String>) -> Result<Self> {
        let build = |timeout: Duration| {
            reqwest::Client::builder()
                .user_agent(AGENT)
                .connect_timeout(CONNECT_TIMEOUT)
                .timeout(timeout)
                .build()
                .map_err(|err| DriveError::Setup(err.to_string()))
        };
        Ok(Self {
            client: build(CALL_TIMEOUT)?,
            uploads: build(CHUNK_TIMEOUT)?,
            oauth: oauth.into(),
            api: api.into(),
        })
    }

    pub fn oauth_url(&self, path: &str) -> String {
        format!("{}{path}", self.oauth)
    }

    pub fn api_url(&self, path: &str) -> String {
        format!("{}{path}", self.api)
    }

    pub fn client(&self) -> &reqwest::Client {
        &self.client
    }

    pub fn upload_client(&self) -> &reqwest::Client {
        &self.uploads
    }

    pub async fn form<T: DeserializeOwned>(
        &self,
        path: &str,
        fields: &[(&str, &str)],
    ) -> Result<T> {
        let response = self
            .client
            .post(self.oauth_url(path))
            .header(reqwest::header::CONTENT_TYPE, "application/x-www-form-urlencoded")
            .body(encode(fields))
            .send()
            .await
            .map_err(unreachable)?;

        let status = response.status().as_u16();
        let body = response.bytes().await.map_err(unreachable)?;
        if (200..300).contains(&status) {
            return read(&body);
        }
        Err(oauth_refusal(status, &body))
    }
}

pub fn encode(fields: &[(&str, &str)]) -> String {
    let mut out = String::new();
    for (name, value) in fields {
        if !out.is_empty() {
            out.push('&');
        }
        out.push_str(&escape(name));
        out.push('=');
        out.push_str(&escape(value));
    }
    out
}

pub fn with_query(path: &str, fields: &[(&str, &str)]) -> String {
    format!("{path}?{}", encode(fields))
}

fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(byte as char);
            }
            b' ' => out.push('+'),
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

pub fn oauth_refusal(status: u16, body: &[u8]) -> DriveError {
    #[derive(serde::Deserialize)]
    struct Refusal {
        error: Option<String>,
        error_code: Option<String>,
        error_description: Option<String>,
    }

    let parsed: Refusal = serde_json::from_slice(body).unwrap_or(Refusal {
        error: None,
        error_code: None,
        error_description: None,
    });
    let name = parsed.error.or(parsed.error_code).unwrap_or_default();
    let detail = parsed.error_description.unwrap_or_else(|| shorten_bytes(body));

    match name.as_str() {
        "authorization_pending" => DriveError::Pending { slow_down: false },
        "slow_down" => DriveError::Pending { slow_down: true },
        "access_denied" => DriveError::Denied,
        "expired_token" => DriveError::Expired,
        "invalid_grant" => DriveError::Revoked(shorten(&detail)),
        "rate_limit_exceeded" => DriveError::RateLimited,
        _ if status == 429 => DriveError::RateLimited,
        _ => DriveError::Refused { status, reason: name, detail: shorten(&detail) },
    }
}

pub fn api_refusal(status: u16, body: &[u8]) -> DriveError {
    #[derive(serde::Deserialize)]
    struct Envelope {
        error: Option<Fault>,
    }
    #[derive(serde::Deserialize)]
    struct Fault {
        message: Option<String>,
        #[serde(default)]
        errors: Vec<Detail>,
    }
    #[derive(serde::Deserialize)]
    struct Detail {
        reason: Option<String>,
    }

    let parsed: Envelope = serde_json::from_slice(body).unwrap_or(Envelope { error: None });
    let fault = parsed.error;
    let reason = fault
        .as_ref()
        .and_then(|fault| fault.errors.first())
        .and_then(|first| first.reason.clone())
        .unwrap_or_default();
    let detail = fault
        .as_ref()
        .and_then(|fault| fault.message.clone())
        .unwrap_or_else(|| shorten_bytes(body));

    match reason.as_str() {
        "storageQuotaExceeded" => DriveError::QuotaFull(shorten(&detail)),
        "rateLimitExceeded" | "userRateLimitExceeded" | "sharingRateLimitExceeded" => {
            DriveError::RateLimited
        }
        "uploadLimitExceeded" | "teamDriveFileLimitExceeded" => {
            DriveError::Refused { status, reason, detail: shorten(&detail) }
        }
        _ if status == 429 => DriveError::RateLimited,
        _ if status == 404 || status == 410 => DriveError::Gone,
        _ if status == 401 => DriveError::Revoked(shorten(&detail)),
        _ => DriveError::Refused { status, reason, detail: shorten(&detail) },
    }
}

pub async fn read_api<T: DeserializeOwned>(response: reqwest::Response) -> Result<T> {
    let status = response.status().as_u16();
    let body = response.bytes().await.map_err(unreachable)?;
    if (200..300).contains(&status) {
        return read(&body);
    }
    Err(api_refusal(status, &body))
}

fn read<T: DeserializeOwned>(body: &[u8]) -> Result<T> {
    serde_json::from_slice(body).map_err(|err| DriveError::Unreadable(shorten(&err.to_string())))
}

pub fn unreachable(err: reqwest::Error) -> DriveError {
    let reason = if err.is_timeout() {
        "it did not answer in time".to_owned()
    } else if err.is_connect() {
        "the connection could not be opened".to_owned()
    } else {
        shorten(&err.to_string())
    };
    DriveError::Unreachable(reason)
}

pub fn shorten(text: &str) -> String {
    text.chars().take(200).collect()
}

fn shorten_bytes(body: &[u8]) -> String {
    shorten(&String::from_utf8_lossy(body))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_full_drive_and_a_rate_limit_both_arrive_as_403() {
        let full = api_refusal(403, include_bytes!("testdata/storage_quota_exceeded.json"));
        assert!(matches!(full, DriveError::QuotaFull(_)), "{full:?}");
        assert!(!full.is_worth_repeating(), "a full Drive does not empty itself");
        assert_eq!(full.operation_code(), "drive_quota_exceeded");

        let busy = api_refusal(403, include_bytes!("testdata/rate_limit_exceeded.json"));
        assert!(matches!(busy, DriveError::RateLimited), "{busy:?}");
        assert!(busy.is_worth_repeating());
    }

    #[test]
    fn the_daily_upload_ceiling_is_not_repeated() {
        let err = api_refusal(403, include_bytes!("testdata/upload_limit_exceeded.json"));
        assert!(matches!(&err, DriveError::Refused { reason, .. } if reason == "uploadLimitExceeded"));
        assert!(!err.is_worth_repeating());
    }

    #[test]
    fn the_three_device_flow_answers_are_told_apart() {
        let pending = oauth_refusal(428, include_bytes!("testdata/authorization_pending.json"));
        assert!(matches!(pending, DriveError::Pending { slow_down: false }), "{pending:?}");

        let slower = oauth_refusal(403, include_bytes!("testdata/slow_down.json"));
        assert!(matches!(slower, DriveError::Pending { slow_down: true }), "{slower:?}");

        let denied = oauth_refusal(403, include_bytes!("testdata/access_denied.json"));
        assert!(matches!(denied, DriveError::Denied), "{denied:?}");

        let expired = oauth_refusal(400, include_bytes!("testdata/expired_token.json"));
        assert!(matches!(expired, DriveError::Expired), "{expired:?}");
    }

    #[test]
    fn the_quota_answer_of_the_code_endpoint_uses_a_field_of_its_own() {
        let full = oauth_refusal(403, include_bytes!("testdata/device_rate_limit_exceeded.json"));
        assert!(matches!(full, DriveError::RateLimited), "{full:?}");
        assert!(full.is_worth_repeating(), "their own advice is to back off and ask again");
    }

    #[test]
    fn the_refusals_of_the_token_endpoint_keep_googles_own_word() {
        let cases: &[(u16, &[u8], &str)] = &[
            (400, include_bytes!("testdata/admin_policy_enforced.json"), "admin_policy_enforced"),
            (403, include_bytes!("testdata/org_internal.json"), "org_internal"),
            (401, include_bytes!("testdata/invalid_client.json"), "invalid_client"),
            (400, include_bytes!("testdata/unsupported_grant_type.json"), "unsupported_grant_type"),
        ];

        for (status, body, word) in cases {
            match oauth_refusal(*status, body) {
                DriveError::Refused { reason, .. } => assert_eq!(reason, *word),
                other => panic!("{word} came back as {other:?}"),
            }
        }
    }

    #[test]
    fn a_withdrawn_connection_is_its_own_answer() {
        let err = oauth_refusal(400, include_bytes!("testdata/invalid_grant.json"));
        assert!(err.is_revoked(), "{err:?}");
        assert_eq!(err.operation_code(), "drive_revoked");
        assert!(!err.is_worth_repeating(), "a new token needs a person, not a retry");
    }

    #[test]
    fn an_answer_that_is_not_googles_at_all_is_kept_short() {
        let err = api_refusal(502, b"<html>502 Bad Gateway</html>");
        assert!(matches!(&err, DriveError::Refused { status: 502, .. }), "{err:?}");
        assert!(err.is_worth_repeating(), "their 5xx is the one thing worth repeating");

        let long = format!(r#"{{"error":{{"message":"{}"}}}}"#, "x".repeat(500));
        let err = api_refusal(400, long.as_bytes());
        match err {
            DriveError::Refused { detail, .. } => assert_eq!(detail.chars().count(), 200),
            other => panic!("expected a refusal, got {other:?}"),
        }
    }

    #[test]
    fn a_query_of_googles_own_language_survives_being_a_url() {
        let encoded = encode(&[("q", "appProperties has { key='panel' and value='craftpanel' }")]);
        assert!(!encoded.contains(' '), "a raw space is not a URL: {encoded}");
        assert!(!encoded.contains('{'), "a raw brace is not a URL: {encoded}");
        assert_eq!(
            encoded,
            "q=appProperties+has+%7B+key%3D%27panel%27+and+value%3D%27craftpanel%27+%7D"
        );

        assert_eq!(
            with_query("/drive/v3/files", &[("alt", "media")]),
            "/drive/v3/files?alt=media"
        );
        assert_eq!(encode(&[]), "");
    }

    #[test]
    fn the_agent_names_us_with_our_version() {
        assert!(AGENT.starts_with("craftpanel/0.1"), "{AGENT}");
    }
}
