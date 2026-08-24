use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use futures::future::BoxFuture;

use crate::model::{DriveLinkState, Timestamp};

use super::http::{DriveError, Http, Result};
use super::retry::Waiting;

pub const SCOPE: &str = "https://www.googleapis.com/auth/drive.file";

const GRANT: &str = "urn:ietf:params:oauth:grant-type:device_code";

const EARLY: Duration = Duration::from_secs(60);

#[derive(Clone, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    pub fn parse(text: &str) -> Option<Self> {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return None;
        }
        Some(Self(trimmed.to_owned()))
    }

    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(hidden)")
    }
}

#[derive(Debug, Clone)]
pub struct Credentials {
    pub client_id: String,
    pub client_secret: Secret,
}

#[derive(Debug, Clone)]
pub struct Access {
    token: Secret,
    expires_at: Timestamp,
    minted: u64,
}

static MINTED: AtomicU64 = AtomicU64::new(0);

impl Access {
    pub fn expose(&self) -> &str {
        self.token.expose()
    }

    pub fn usable(&self, now: Timestamp) -> bool {
        now.as_datetime() + time::Duration::seconds(EARLY.as_secs() as i64)
            < self.expires_at.as_datetime()
    }

    pub fn newer_than(&self, other: &Self) -> bool {
        self.minted > other.minted
    }
}

pub trait Bearer: Sync {
    fn token(&self) -> BoxFuture<'_, Result<Access>>;

    fn renew<'a>(&'a self, stale: &'a Access) -> BoxFuture<'a, Result<Access>>;
}

#[derive(Debug, Clone)]
pub struct Device {
    pub device_code: Secret,
    pub user_code: String,
    pub verification_url: String,
    pub expires_at: Timestamp,
    pub interval: Duration,
}

#[derive(serde::Deserialize)]
struct DeviceAnswer {
    device_code: String,
    user_code: String,
    #[serde(default)]
    verification_url: Option<String>,
    #[serde(default)]
    verification_uri: Option<String>,
    expires_in: i64,
    #[serde(default)]
    interval: Option<u64>,
}

#[derive(serde::Deserialize)]
struct TokenAnswer {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    expires_in: Option<i64>,
}

const DEVICE_PAGE: &str = "https://www.google.com/device";

pub async fn begin(http: &Http, credentials: &Credentials) -> Result<Device> {
    let answer: DeviceAnswer = http
        .form("/device/code", &[("client_id", &credentials.client_id), ("scope", SCOPE)])
        .await?;

    let device_code = Secret::parse(&answer.device_code)
        .ok_or_else(|| DriveError::Unreadable("Google sent an empty device code".to_owned()))?;
    if answer.user_code.trim().is_empty() {
        return Err(DriveError::Unreadable("Google sent an empty user code".to_owned()));
    }

    Ok(Device {
        device_code,
        user_code: answer.user_code,
        verification_url: answer
            .verification_url
            .or(answer.verification_uri)
            .unwrap_or_else(|| DEVICE_PAGE.to_owned()),
        expires_at: in_seconds(answer.expires_in.max(0)),
        interval: Duration::from_secs(answer.interval.unwrap_or(5).clamp(1, 60)),
    })
}

pub async fn poll(http: &Http, credentials: &Credentials, device: &Secret) -> Result<Tokens> {
    let answer: TokenAnswer = http
        .form(
            "/token",
            &[
                ("client_id", &credentials.client_id),
                ("client_secret", credentials.client_secret.expose()),
                ("device_code", device.expose()),
                ("grant_type", GRANT),
            ],
        )
        .await?;

    let refresh = answer.refresh_token.as_deref().and_then(Secret::parse).ok_or_else(|| {
        DriveError::Unreadable("Google accepted the code but sent no refresh token".to_owned())
    })?;

    Ok(Tokens { refresh, access: access_of(&answer)? })
}

#[derive(Debug)]
pub struct Tokens {
    pub refresh: Secret,
    pub access: Access,
}

pub async fn refresh(
    http: &Http,
    credentials: &Credentials,
    token: &Secret,
    over: &Waiting<'_>,
) -> Result<Access> {
    let answer: TokenAnswer = http
        .form_again(
            over,
            "/token",
            &[
                ("client_id", &credentials.client_id),
                ("client_secret", credentials.client_secret.expose()),
                ("refresh_token", token.expose()),
                ("grant_type", "refresh_token"),
            ],
        )
        .await?;
    access_of(&answer)
}

pub async fn revoke(http: &Http, token: &Secret) -> Result<()> {
    let _: serde_json::Value = http
        .form("/revoke", &[("token", token.expose())])
        .await
        .or_else(|err| match err {
            DriveError::Unreadable(_) => Ok(serde_json::Value::Null),
            other => Err(other),
        })?;
    Ok(())
}

fn access_of(answer: &TokenAnswer) -> Result<Access> {
    let token = Secret::parse(&answer.access_token)
        .ok_or_else(|| DriveError::Unreadable("Google sent an empty access token".to_owned()))?;
    Ok(Access {
        token,
        expires_at: in_seconds(answer.expires_in.unwrap_or(3600).max(0)),
        minted: MINTED.fetch_add(1, Ordering::Relaxed),
    })
}

fn in_seconds(seconds: i64) -> Timestamp {
    Timestamp::at(Timestamp::now().as_datetime() + time::Duration::seconds(seconds))
}

pub fn looks_like_a_testing_project(connected_at: Timestamp, now: Timestamp) -> bool {
    now.as_datetime() - connected_at.as_datetime() < time::Duration::days(10)
}

pub const TESTING_HINT: &str = "Google withdrew this connection after a few days. That is what \
    happens while the OAuth consent screen of the panel's Google project is still on \"Testing\" — \
    ask the operator to publish it (\"Publish app\" → In production).";

const WHERE_TO_FIX_IT: &str = "Google Cloud console → APIs & Services → OAuth consent screen → \
    Audience: either add the account under \"Test users\", or press \"Publish app\" (In \
    production)";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ending {
    pub state: DriveLinkState,
    pub sentence: String,
}

pub fn ending(err: &DriveError) -> Ending {
    match err {
        DriveError::Denied => Ending {
            state: DriveLinkState::Denied,
            sentence: format!(
                "Google turned the request down (access_denied). Either it was declined on the \
                 consent screen, or the OAuth consent screen of the panel's Google project is \
                 still on \"Testing\" and this Google account is not one of its test users — then \
                 Google refuses without asking anybody. The operator fixes the second one: \
                 {WHERE_TO_FIX_IT}."
            ),
        },

        DriveError::Expired | DriveError::Pending { .. } => Ending {
            state: DriveLinkState::Expired,
            sentence: format!(
                "The code ran out before it was confirmed (expired_token). If Google's own page \
                 said an error had occurred instead of asking for permission, its consent screen \
                 is still on \"Testing\" and this Google account is not one of its test users: \
                 {WHERE_TO_FIX_IT}."
            ),
        },

        DriveError::RateLimited => Ending {
            state: DriveLinkState::Expired,
            sentence: "Google is handing out no more codes for this client at the moment \
                       (rate_limit_exceeded). Try again in a few minutes."
                .to_owned(),
        },

        DriveError::Revoked(detail) => Ending {
            state: DriveLinkState::Expired,
            sentence: format!(
                "Google will not take this code any more (invalid_grant): it has been used \
                 already, or it is not valid. Ask for a new code. Google said: {detail}"
            ),
        },

        DriveError::Unreachable(detail) => Ending {
            state: DriveLinkState::Expired,
            sentence: format!(
                "Google could not be reached while the code was waiting ({detail}). Nothing is \
                 connected; press again once the connection is back."
            ),
        },

        DriveError::Refused { status, reason, detail } => refused(*status, reason, detail),

        other => Ending {
            state: DriveLinkState::Expired,
            sentence: format!("The connection to Google did not come good: {other}"),
        },
    }
}

fn refused(status: u16, reason: &str, detail: &str) -> Ending {
    let sentence = match reason {
        "admin_policy_enforced" => "This Google account is not allowed to grant the panel access \
             to Drive (admin_policy_enforced): its Google Workspace administrator has to let the \
             panel's OAuth client through first. A private Google account has no such rule."
            .to_owned(),
        "org_internal" => "The panel's Google project only admits accounts of one Google Cloud \
             organisation (org_internal). The operator sets its user type to \"External\": Google \
             Cloud console → APIs & Services → OAuth consent screen → Audience."
            .to_owned(),
        "invalid_client" => "Google does not accept the panel's Google client (invalid_client). \
             Either the client id or the client secret is wrong, or the client is not of type \
             \"TVs and Limited Input devices\" — the operator checks both under Administration → \
             Google Drive."
            .to_owned(),
        "unsupported_grant_type" => "Google refused the way the panel asked \
             (unsupported_grant_type). That is a fault of the panel, not a setting."
            .to_owned(),
        "" => format!("Google refused the request (HTTP {status}): {detail}"),
        _ => format!("Google refused the request ({reason}): {detail}"),
    };
    let state = match reason {
        "admin_policy_enforced" | "org_internal" => DriveLinkState::Denied,
        _ => DriveLinkState::Expired,
    };
    Ending { state, sentence }
}

#[cfg(test)]
mod tests {
    use super::*;

    use super::super::http::oauth_refusal;

    const ACCESS_DENIED: &[u8] = include_bytes!("testdata/access_denied.json");
    const EXPIRED: &[u8] = include_bytes!("testdata/expired_token.json");
    const ADMIN_POLICY: &[u8] = include_bytes!("testdata/admin_policy_enforced.json");
    const ORG_INTERNAL: &[u8] = include_bytes!("testdata/org_internal.json");
    const BAD_CLIENT: &[u8] = include_bytes!("testdata/invalid_client.json");
    const BAD_GRANT_TYPE: &[u8] = include_bytes!("testdata/unsupported_grant_type.json");
    const SPENT_CODE: &[u8] = include_bytes!("testdata/invalid_grant.json");
    const TOO_MANY_CODES: &[u8] = include_bytes!("testdata/device_rate_limit_exceeded.json");

    #[test]
    fn a_secret_does_not_show_itself_in_a_log_line() {
        let secret = Secret::parse("1//the-refresh-token").expect("a secret");
        assert_eq!(format!("{secret:?}"), "Secret(hidden)");
        assert_eq!(secret.expose(), "1//the-refresh-token");

        assert!(Secret::parse("   ").is_none(), "whitespace is not a secret");
        assert_eq!(Secret::parse(" abc \n").expect("trimmed").expose(), "abc");
    }

    #[test]
    fn googles_own_field_name_is_the_one_that_is_read() {
        let answer: DeviceAnswer =
            serde_json::from_slice(include_bytes!("testdata/device_code.json")).expect("json");
        assert_eq!(answer.verification_url.as_deref(), Some("https://www.google.com/device"));
        assert_eq!(answer.verification_uri, None);
        assert_eq!(answer.user_code, "GQVQ-JKEC");
        assert_eq!(answer.interval, Some(5));
    }

    #[test]
    fn an_access_token_is_used_a_minute_before_it_dies_and_not_after() {
        let answer: TokenAnswer =
            serde_json::from_slice(include_bytes!("testdata/token.json")).expect("json");
        let access = access_of(&answer).expect("a token");

        let now = Timestamp::now();
        assert!(access.usable(now), "an hour of life is usable");

        let almost = Timestamp::at(now.as_datetime() + time::Duration::seconds(3599 - 30));
        assert!(!access.usable(almost), "thirty seconds left is not worth starting a chunk with");
    }

    #[test]
    fn every_refusal_of_theirs_becomes_a_state_and_a_sentence() {
        let cases: &[(u16, &[u8], DriveLinkState, &str)] = &[
            (403, ACCESS_DENIED, DriveLinkState::Denied, "access_denied"),
            (400, EXPIRED, DriveLinkState::Expired, "expired_token"),
            (400, ADMIN_POLICY, DriveLinkState::Denied, "admin_policy_enforced"),
            (403, ORG_INTERNAL, DriveLinkState::Denied, "org_internal"),
            (401, BAD_CLIENT, DriveLinkState::Expired, "invalid_client"),
            (400, BAD_GRANT_TYPE, DriveLinkState::Expired, "unsupported_grant_type"),
            (400, SPENT_CODE, DriveLinkState::Expired, "invalid_grant"),
            (403, TOO_MANY_CODES, DriveLinkState::Expired, "rate_limit_exceeded"),
        ];

        for (status, body, state, word) in cases {
            let end = ending(&oauth_refusal(*status, body));
            assert_eq!(end.state, *state, "{status} {}", String::from_utf8_lossy(body));
            assert!(
                end.sentence.contains(word),
                "{status}: {:?} does not say Google's own word {word:?}",
                end.sentence
            );
        }
    }

    #[test]
    fn the_testing_trap_names_the_page_the_operator_changes_it_on() {
        for body in [ACCESS_DENIED, EXPIRED] {
            let sentence = ending(&oauth_refusal(403, body)).sentence;
            assert!(sentence.contains("Testing"), "{sentence}");
            assert!(sentence.contains("Test users"), "{sentence}");
            assert!(sentence.contains("Publish app"), "{sentence}");
            assert!(sentence.contains("OAuth consent screen"), "{sentence}");
        }
    }

    #[test]
    fn even_a_nameless_answer_ends_with_a_sentence() {
        let html = ending(&oauth_refusal(502, b"<html>502 Bad Gateway</html>"));
        assert_eq!(html.state, DriveLinkState::Expired);
        assert!(html.sentence.contains("502"), "{}", html.sentence);

        let dead = ending(&DriveError::Unreachable("it did not answer in time".to_owned()));
        assert!(dead.sentence.contains("did not answer in time"), "{}", dead.sentence);

        let waiting = ending(&DriveError::Pending { slow_down: false });
        assert_eq!(waiting.state, DriveLinkState::Expired);
        assert!(!waiting.sentence.is_empty());
    }

    #[test]
    fn the_seven_day_trap_is_only_guessed_at_while_it_could_be_true() {
        let now = Timestamp::now();
        let week = Timestamp::at(now.as_datetime() - time::Duration::days(7));
        assert!(looks_like_a_testing_project(week, now), "seven days is the trap of 22.2");

        let ages = Timestamp::at(now.as_datetime() - time::Duration::days(200));
        assert!(
            !looks_like_a_testing_project(ages, now),
            "a connection that ran for months was withdrawn by its owner, not by a setting"
        );
    }
}
