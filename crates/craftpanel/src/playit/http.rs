use std::time::Duration;

use serde::de::DeserializeOwned;
use serde::Serialize;

use super::Secret;

pub const BASE: &str = "https://api.playit.gg";

const AGENT: &str = concat!("craftpanel/", env!("CARGO_PKG_VERSION"));
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const CALL_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Debug, thiserror::Error)]
pub enum PlayitError {
    #[error("playit.gg could not be reached: {0}")]
    Unreachable(String),

    #[error("playit.gg answered {0}")]
    Fail(String),

    #[error("playit.gg refused the call: {detail}")]
    Refused { kind: String, detail: String },

    #[error("playit.gg is turning us away for now (HTTP 429)")]
    RateLimited,

    #[error("playit.gg answered in a shape we do not understand: {0}")]
    Unreadable(String),

    #[error("the HTTP client could not be set up: {0}")]
    Setup(String),
}

impl PlayitError {
    pub fn is_invalid_key(&self) -> bool {
        matches!(self, Self::Refused { kind, detail } if kind == "auth"
            && (detail.contains("InvalidAgentKey") || detail.contains("AuthRequired")))
    }

    pub fn is_not_self_managed(&self) -> bool {
        matches!(self, Self::Refused { kind, detail } if kind == "auth"
            && detail.contains("AgentNotSelfManaged"))
    }

    pub fn is_validation(&self) -> bool {
        matches!(self, Self::Refused { kind, .. } if kind == "validation")
    }

    pub fn named(&self, name: &str) -> bool {
        matches!(self, Self::Fail(code) if code == name)
    }
}

pub type Result<T> = std::result::Result<T, PlayitError>;

#[derive(Clone)]
pub struct Http {
    client: reqwest::Client,
    base: String,
}

impl Http {
    pub fn against(base: impl Into<String>) -> Result<Self> {
        let client = reqwest::Client::builder()
            .user_agent(AGENT)
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(CALL_TIMEOUT)
            .build()
            .map_err(|err| PlayitError::Setup(err.to_string()))?;
        Ok(Self { client, base: base.into() })
    }

    pub async fn call<B: Serialize, T: DeserializeOwned>(
        &self,
        path: &str,
        body: &B,
        secret: Option<&Secret>,
    ) -> Result<T> {
        let mut request = self.client.post(format!("{}{path}", self.base)).json(body);
        if let Some(secret) = secret {
            request = request.header(
                reqwest::header::AUTHORIZATION,
                format!("Agent-Key {}", secret.expose()),
            );
        }

        let response = request.send().await.map_err(unreachable)?;
        if response.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
            return Err(PlayitError::RateLimited);
        }

        let body = response.bytes().await.map_err(unreachable)?;
        decode(&body)
    }
}

#[derive(serde::Deserialize)]
#[serde(tag = "status", content = "data")]
enum Answer<T> {
    #[serde(rename = "success")]
    Success(T),
    #[serde(rename = "fail")]
    Fail(serde_json::Value),
    #[serde(rename = "error")]
    Error(Fault),
}

#[derive(serde::Deserialize)]
struct Fault {
    #[serde(rename = "type")]
    kind: String,
    message: serde_json::Value,
}

pub fn decode<T: DeserializeOwned>(body: &[u8]) -> Result<T> {
    match serde_json::from_slice::<Answer<T>>(body) {
        Ok(Answer::Success(value)) => Ok(value),
        Ok(Answer::Fail(data)) => Err(PlayitError::Fail(sentence(&data))),
        Ok(Answer::Error(fault)) => {
            Err(PlayitError::Refused { kind: fault.kind, detail: sentence(&fault.message) })
        }
        Err(err) => Err(PlayitError::Unreadable(shorten(&err.to_string()))),
    }
}

fn sentence(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(text) => shorten(text),
        other => shorten(&other.to_string()),
    }
}

fn shorten(text: &str) -> String {
    text.chars().take(200).collect()
}

fn unreachable(err: reqwest::Error) -> PlayitError {
    let reason = if err.is_timeout() {
        "it did not answer in time".to_owned()
    } else if err.is_connect() {
        "the connection could not be opened".to_owned()
    } else {
        shorten(&err.to_string())
    };
    PlayitError::Unreachable(reason)
}

#[cfg(test)]
mod tests {
    use super::*;

    const AUTH_REQUIRED: &[u8] = include_bytes!("testdata/error_auth_required.json");
    const INVALID_KEY: &[u8] = include_bytes!("testdata/error_invalid_agent_key.json");
    const PATH_NOT_FOUND: &[u8] = include_bytes!("testdata/error_path_not_found.json");
    const VALIDATION: &[u8] = include_bytes!("testdata/error_validation.json");
    const WAITING: &[u8] = include_bytes!("testdata/claim_setup_waiting_for_visit.json");
    const INVALID_CODE: &[u8] = include_bytes!("testdata/claim_setup_invalid_code.json");
    const POPS: &[u8] = include_bytes!("testdata/info_pops.json");

    #[test]
    fn a_404_carries_its_message_as_an_object_and_still_parses() {
        let err = decode::<serde_json::Value>(PATH_NOT_FOUND).unwrap_err();

        match err {
            PlayitError::Refused { kind, detail } => {
                assert_eq!(kind, "path-not-found");
                assert_eq!(detail, r#"{"path":"/nope/nothing"}"#);
            }
            other => panic!("expected a refusal, got {other:?}"),
        }
    }

    #[test]
    fn the_two_auth_refusals_are_told_apart_from_everything_else() {
        let missing = decode::<serde_json::Value>(AUTH_REQUIRED).unwrap_err();
        let invalid = decode::<serde_json::Value>(INVALID_KEY).unwrap_err();

        assert!(missing.is_invalid_key(), "{missing:?}");
        assert!(invalid.is_invalid_key(), "{invalid:?}");
        assert_eq!(invalid.to_string(), "playit.gg refused the call: InvalidAgentKey");

        let validation = decode::<serde_json::Value>(VALIDATION).unwrap_err();
        assert!(!validation.is_invalid_key(), "{validation:?}");
        assert!(validation.is_validation(), "{validation:?}");
        assert_eq!(
            validation.to_string(),
            "playit.gg refused the call: failed to parse body"
        );
    }

    #[test]
    fn a_fail_keeps_the_name_playit_gave_it() {
        let state: String = decode(WAITING).unwrap();
        assert_eq!(state, "WaitingForUserVisit");

        let err = decode::<String>(INVALID_CODE).unwrap_err();
        assert!(err.named("InvalidCode"), "{err:?}");
        assert!(!err.is_validation(), "a fail is not an error branch: {err:?}");
    }

    #[test]
    fn a_success_hands_over_only_the_fields_we_asked_for() {
        #[derive(serde::Deserialize)]
        struct Pops {
            pops: Vec<Pop>,
        }
        #[derive(serde::Deserialize)]
        struct Pop {
            name: String,
        }

        let pops: Pops = decode(POPS).unwrap();
        assert_eq!(pops.pops.len(), 22);
        assert_eq!(pops.pops[0].name, "Los Angeles, California");
    }

    #[test]
    fn an_answer_that_is_not_theirs_at_all_is_unreadable_and_kept_short() {
        let err = decode::<String>(b"<html>502 Bad Gateway</html>").unwrap_err();
        assert!(matches!(err, PlayitError::Unreadable(_)), "{err:?}");

        let long = format!(r#"{{"status":"fail","data":"{}"}}"#, "x".repeat(500));
        let err = decode::<String>(long.as_bytes()).unwrap_err();
        assert_eq!(err.to_string().len(), "playit.gg answered ".len() + 200);
    }

    #[test]
    fn the_agent_names_us_with_our_version() {
        assert!(AGENT.starts_with("craftpanel/0.1"), "{AGENT}");
    }

    #[tokio::test]
    #[ignore = "talks to api.playit.gg"]
    async fn the_real_service_still_answers_in_the_three_shapes_we_parse() {
        #[derive(serde::Deserialize)]
        struct Pops {
            pops: Vec<serde_json::Value>,
        }

        let http = Http::against(BASE).unwrap();
        let empty = serde_json::json!({});

        let pops: Pops = http.call("/info/pops", &empty, None).await.unwrap();
        assert!(!pops.pops.is_empty());

        let refused = http
            .call::<_, serde_json::Value>("/v1/tunnels/list", &empty, None)
            .await
            .unwrap_err();
        assert!(refused.is_invalid_key(), "{refused:?}");

        let missing = http
            .call::<_, serde_json::Value>("/nope/nothing", &empty, None)
            .await
            .unwrap_err();
        assert!(
            matches!(&missing, PlayitError::Refused { kind, .. } if kind == "path-not-found"),
            "{missing:?}"
        );

        let too_short: std::result::Result<String, _> = http
            .call(
                "/claim/setup",
                &serde_json::json!({
                    "code": "ab",
                    "agent_type": "self-managed",
                    "version": "craftpanel test",
                }),
                None,
            )
            .await;
        assert!(too_short.unwrap_err().named("InvalidCode"));
    }
}
