use serde::{Deserialize, Serialize};

use super::http::{Http, PlayitError, Result};
use super::Secret;

const VERSION: &str = concat!("craftpanel ", env!("CARGO_PKG_VERSION"));

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClaimState {
    WaitingForVisit,
    WaitingForUser,
    Accepted,
    Rejected,
}

impl ClaimState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::WaitingForVisit => "waiting_for_visit",
            Self::WaitingForUser => "waiting_for_user",
            Self::Accepted => "accepted",
            Self::Rejected => "rejected",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "waiting_for_visit" => Some(Self::WaitingForVisit),
            "waiting_for_user" => Some(Self::WaitingForUser),
            "accepted" => Some(Self::Accepted),
            "rejected" => Some(Self::Rejected),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    Wait,
    Fetch,
    Renew,
    Stop(String),
}

pub fn generate() -> String {
    hex::encode(rand::random::<[u8; 5]>())
}

pub fn url(code: &str) -> String {
    format!("https://playit.gg/claim/{code}")
}

pub async fn setup(http: &Http, code: &str) -> Result<ClaimState> {
    let body = serde_json::json!({
        "code": code,
        "agent_type": "self-managed",
        "version": VERSION,
    });

    let state: String = http.call("/claim/setup", &body, None).await?;
    match state.as_str() {
        "WaitingForUserVisit" => Ok(ClaimState::WaitingForVisit),
        "WaitingForUser" => Ok(ClaimState::WaitingForUser),
        "UserAccepted" => Ok(ClaimState::Accepted),
        "UserRejected" => Ok(ClaimState::Rejected),
        other => Err(PlayitError::Unreadable(format!("unknown claim state {other:?}"))),
    }
}

pub async fn exchange(http: &Http, code: &str) -> Result<Secret> {
    #[derive(Deserialize)]
    struct AgentSecretKey {
        secret_key: String,
    }

    let body = serde_json::json!({ "code": code });
    let key: AgentSecretKey = http.call("/claim/exchange", &body, None).await?;
    Secret::parse(&key.secret_key)
}

pub fn after_setup(outcome: &Result<ClaimState>) -> Step {
    match outcome {
        Ok(ClaimState::WaitingForVisit | ClaimState::WaitingForUser) => Step::Wait,
        Ok(ClaimState::Accepted) => Step::Fetch,
        Ok(ClaimState::Rejected) => {
            Step::Stop("the sign-up was declined on playit.gg".to_owned())
        }
        Err(err) if err.named("CodeExpired") => Step::Renew,
        Err(err) if err.named("InvalidCode") => {
            Step::Stop("playit.gg would not take the code we made".to_owned())
        }
        Err(_) => Step::Wait,
    }
}

pub fn after_exchange(err: &PlayitError) -> Step {
    match err {
        err if err.named("CodeNotFound") => Step::Wait,
        err if err.named("NotAccepted") => Step::Wait,
        err if err.named("NotSetup") => Step::Wait,
        err if err.named("CodeExpired") => Step::Renew,
        err if err.named("UserRejected") => {
            Step::Stop("the sign-up was declined on playit.gg".to_owned())
        }
        other => Step::Stop(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::playit::http::decode;

    const NOT_READY: &[u8] = include_bytes!("testdata/claim_exchange_not_ready.json");
    const WAITING: &[u8] = include_bytes!("testdata/claim_setup_waiting_for_visit.json");
    const INVALID_CODE: &[u8] = include_bytes!("testdata/claim_setup_invalid_code.json");
    const VERSION_TOO_LONG: &[u8] = include_bytes!("testdata/claim_setup_version_too_long.json");

    fn state_of(body: &[u8]) -> Result<ClaimState> {
        let state: String = decode(body)?;
        match state.as_str() {
            "WaitingForUserVisit" => Ok(ClaimState::WaitingForVisit),
            "WaitingForUser" => Ok(ClaimState::WaitingForUser),
            "UserAccepted" => Ok(ClaimState::Accepted),
            "UserRejected" => Ok(ClaimState::Rejected),
            other => Err(PlayitError::Unreadable(other.to_owned())),
        }
    }

    #[test]
    fn code_not_found_from_exchange_means_not_yet_and_never_gives_up() {
        let err = decode::<serde_json::Value>(NOT_READY).unwrap_err();
        assert!(err.named("CodeNotFound"), "the fixture is the measured answer: {err:?}");

        assert_eq!(after_exchange(&err), Step::Wait);
    }

    #[test]
    fn the_three_ways_a_claim_really_ends_are_told_apart() {
        assert_eq!(
            after_exchange(&PlayitError::Fail("UserRejected".to_owned())),
            Step::Stop("the sign-up was declined on playit.gg".to_owned())
        );
        assert_eq!(after_exchange(&PlayitError::Fail("CodeExpired".to_owned())), Step::Renew);
        assert_eq!(after_exchange(&PlayitError::Fail("NotAccepted".to_owned())), Step::Wait);
    }

    #[test]
    fn a_fresh_code_waits_and_a_rejected_one_stops() {
        assert_eq!(after_setup(&state_of(WAITING)), Step::Wait);
        assert_eq!(after_setup(&Ok(ClaimState::WaitingForUser)), Step::Wait);
        assert_eq!(after_setup(&Ok(ClaimState::Accepted)), Step::Fetch);
        assert!(matches!(after_setup(&Ok(ClaimState::Rejected)), Step::Stop(_)));
    }

    #[test]
    fn a_code_playit_will_not_take_stops_but_a_dead_line_does_not() {
        assert!(matches!(after_setup(&state_of(INVALID_CODE)), Step::Stop(_)));
        assert_eq!(after_setup(&state_of(VERSION_TOO_LONG)), Step::Wait);
        assert_eq!(
            after_setup(&Err(PlayitError::Unreachable("no route to host".to_owned()))),
            Step::Wait
        );
        assert_eq!(after_setup(&Err(PlayitError::RateLimited)), Step::Wait);
    }

    #[test]
    fn a_code_is_ten_hex_characters_and_not_the_same_one_twice() {
        let first = generate();
        assert_eq!(first.len(), 10, "{first}");
        assert!(first.chars().all(|c| c.is_ascii_hexdigit() && !c.is_uppercase()), "{first}");
        assert!(hex::decode(&first).is_ok());
        assert_ne!(first, generate());
    }

    #[test]
    fn the_claim_url_carries_the_code_and_nothing_else() {
        assert_eq!(url("34ddf358a8"), "https://playit.gg/claim/34ddf358a8");
    }

    #[test]
    fn the_state_survives_a_round_trip_through_the_column() {
        for state in [
            ClaimState::WaitingForVisit,
            ClaimState::WaitingForUser,
            ClaimState::Accepted,
            ClaimState::Rejected,
        ] {
            assert_eq!(ClaimState::parse(state.as_str()), Some(state));
            assert_eq!(
                serde_json::to_value(state).unwrap(),
                serde_json::Value::String(state.as_str().to_owned())
            );
        }
        assert_eq!(ClaimState::parse("nonsense"), None);
    }
}
