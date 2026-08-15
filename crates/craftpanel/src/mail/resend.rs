use std::fmt;
use std::time::Duration;

use serde::Serialize;

pub const BASE: &str = "https://api.resend.com";

const AGENT: &str = concat!("craftpanel/", env!("CARGO_PKG_VERSION"));
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const CALL_TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Clone, PartialEq, Eq)]
pub struct ApiKey(String);

impl ApiKey {
    pub fn parse(text: &str) -> Option<Self> {
        let trimmed = text.trim();
        if trimmed.is_empty() || trimmed.chars().any(|c| c.is_whitespace() || c.is_control()) {
            return None;
        }
        Some(Self(trimmed.to_owned()))
    }

    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for ApiKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ApiKey(hidden)")
    }
}

#[derive(Debug, Clone)]
pub struct Outgoing<'a> {
    pub from: &'a str,
    pub to: &'a str,
    pub reply_to: Option<&'a str>,
    pub subject: &'a str,
    pub html: &'a str,
    pub text: &'a str,
    pub kind: &'a str,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{}", self.message())]
pub enum MailError {
    NotConfigured,
    NoLinkBase(String),
    Braked(String),
    QuotaReached(String),
    KeyRejected(String),
    SenderRejected(String),
    Refused(String),
    Upstream(String),
    Unreadable(String),
}

impl MailError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::NotConfigured => "mail_not_configured",
            Self::NoLinkBase(_) => "mail_no_link_base",
            Self::Braked(_) => "mail_rate_limited",
            Self::QuotaReached(_) => "mail_quota_reached",
            Self::KeyRejected(_) => "mail_key_rejected",
            Self::SenderRejected(_) => "mail_sender_rejected",
            Self::Refused(_) => "mail_refused",
            Self::Upstream(_) => "mail_upstream",
            Self::Unreadable(_) => "mail_unreadable",
        }
    }

    pub fn status(&self) -> axum::http::StatusCode {
        use axum::http::StatusCode;
        match self {
            Self::NotConfigured | Self::NoLinkBase(_) => StatusCode::CONFLICT,
            Self::Braked(_) | Self::QuotaReached(_) => StatusCode::TOO_MANY_REQUESTS,
            _ => StatusCode::BAD_GATEWAY,
        }
    }

    pub fn transient(&self) -> bool {
        matches!(self, Self::QuotaReached(_) | Self::Upstream(_))
    }

    pub fn until_tomorrow(&self) -> bool {
        matches!(self, Self::QuotaReached(text) if text.contains("daily"))
    }

    pub fn message(&self) -> String {
        match self {
            Self::NotConfigured => "Mail is not set up. Put a Resend key in under \
                 Administration → Mail; until then the panel sends nothing."
                .to_owned(),
            Self::NoLinkBase(text)
            | Self::Braked(text)
            | Self::QuotaReached(text)
            | Self::KeyRejected(text)
            | Self::SenderRejected(text)
            | Self::Refused(text)
            | Self::Upstream(text)
            | Self::Unreadable(text) => text.clone(),
        }
    }
}

pub type Result<T> = std::result::Result<T, MailError>;

#[derive(Clone)]
pub struct Resend {
    client: reqwest::Client,
    base: String,
}

impl Resend {
    pub fn new() -> anyhow::Result<Self> {
        Self::against(BASE)
    }

    pub fn against(base: impl Into<String>) -> anyhow::Result<Self> {
        let client = reqwest::Client::builder()
            .user_agent(AGENT)
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(CALL_TIMEOUT)
            .build()?;
        Ok(Self { client, base: base.into() })
    }

    pub async fn send(
        &self,
        key: &ApiKey,
        mail: &Outgoing<'_>,
        idempotency: &str,
    ) -> Result<String> {
        let body = Body {
            from: mail.from,
            to: [mail.to],
            reply_to: mail.reply_to,
            subject: mail.subject,
            html: mail.html,
            text: mail.text,
            tags: [Tag { name: "kind", value: mail.kind }],
        };

        let response = self
            .client
            .post(format!("{}/emails", self.base))
            .header(reqwest::header::AUTHORIZATION, format!("Bearer {}", key.expose()))
            .header("Idempotency-Key", idempotency)
            .json(&body)
            .send()
            .await
            .map_err(unreachable)?;

        let status = response.status().as_u16();
        let bytes = response.bytes().await.map_err(unreachable)?;
        interpret(status, &bytes, mail.from)
    }
}

#[derive(Serialize)]
struct Body<'a> {
    from: &'a str,
    to: [&'a str; 1],
    #[serde(skip_serializing_if = "Option::is_none")]
    reply_to: Option<&'a str>,
    subject: &'a str,
    html: &'a str,
    text: &'a str,
    tags: [Tag<'a>; 1],
}

#[derive(Serialize)]
struct Tag<'a> {
    name: &'a str,
    value: &'a str,
}

#[derive(serde::Deserialize)]
struct Sent {
    id: String,
}

#[derive(serde::Deserialize, Default)]
struct Fault {
    #[serde(default)]
    name: String,
    #[serde(default)]
    message: String,
}

pub fn interpret(status: u16, body: &[u8], from: &str) -> Result<String> {
    if (200..300).contains(&status) {
        return match serde_json::from_slice::<Sent>(body) {
            Ok(sent) if !sent.id.is_empty() => Ok(sent.id),
            _ => Err(MailError::Unreadable(
                "Resend answered in a shape we do not understand.".to_owned(),
            )),
        };
    }

    let fault = serde_json::from_slice::<Fault>(body).unwrap_or_default();
    let name = fault.name.as_str();
    let text = shorten(&fault.message);
    let lower = text.to_lowercase();

    match status {
        401 | 403 if name == "missing_api_key" => Err(MailError::KeyRejected(
            "Resend saw no key at all. Put one in under Administration → Mail.".to_owned(),
        )),
        401 | 403 if name == "restricted_api_key" => Err(MailError::KeyRejected(
            "This key may not send. Create one with \"Sending access\" at \
             resend.com/api-keys and put it in here."
                .to_owned(),
        )),
        401 => Err(MailError::KeyRejected(
            "Resend does not know this key (any more). Create a new one at \
             resend.com/api-keys and put it in here."
                .to_owned(),
        )),
        403 if lower.contains("not verified") => Err(MailError::SenderRejected(format!(
            "The domain of {from} is not verified at Resend. Verify it at resend.com/domains — \
             or use onboarding@resend.dev for a first attempt and send only to the address your \
             Resend account was opened with."
        ))),
        403 if lower.contains("own email address") => Err(MailError::SenderRejected(
            "Without a verified domain Resend only accepts the address your Resend account was \
             opened with. Real users need a verified domain (resend.com/domains)."
                .to_owned(),
        )),
        403 if name == "invalid_api_key" => Err(MailError::KeyRejected(
            "Resend does not know this key (any more). Create a new one at \
             resend.com/api-keys and put it in here."
                .to_owned(),
        )),
        422 if name == "invalid_from_address" => Err(MailError::SenderRejected(format!(
            "Resend does not read {from} as an address. The form is name@domain.tld or \
             Name <name@domain.tld>."
        ))),
        429 if name == "daily_quota_exceeded" => Err(MailError::QuotaReached(
            "Resend's daily allowance is used up (free tier: 100 a day). The mail waits until \
             tomorrow."
                .to_owned(),
        )),
        429 if name == "monthly_quota_exceeded" => Err(MailError::QuotaReached(
            "Resend's monthly allowance is used up (free tier: 3,000). No mail goes out until \
             the month turns."
                .to_owned(),
        )),
        429 => Err(MailError::Upstream(
            "Resend is throttling us (10 requests a second). The mail waits and is tried again."
                .to_owned(),
        )),
        451 => Err(MailError::Refused(
            "Resend refused the mail as a security risk. The text has to change; ask Resend's \
             support if in doubt."
                .to_owned(),
        )),
        409 if name == "concurrent_idempotent_requests" => Err(MailError::Upstream(
            "The same mail is already on its way. It will be looked at again shortly.".to_owned(),
        )),
        409 => Err(MailError::Refused(
            "The same send key with a different body — the mail is queued again.".to_owned(),
        )),
        500..=599 => Err(MailError::Upstream(format!(
            "Resend had an error (HTTP {status}). The mail waits and is tried again."
        ))),
        _ => Err(MailError::Refused(format!(
            "Resend refused the request: {text} That is a fault of the panel, not a setting."
        ))),
    }
}

fn shorten(text: &str) -> String {
    let mut out: String = text.chars().take(200).collect();
    if !out.is_empty() && !out.ends_with('.') {
        out.push('.');
    }
    out
}

fn unreachable(err: reqwest::Error) -> MailError {
    let reason = if err.is_timeout() {
        "it did not answer in time".to_owned()
    } else if err.is_connect() {
        "the connection could not be opened".to_owned()
    } else {
        shorten(&err.to_string())
    };
    MailError::Upstream(format!("api.resend.com could not be reached: {reason}. The mail waits."))
}

#[cfg(test)]
mod tests {
    use super::*;

    const MISSING_KEY: &[u8] = include_bytes!("testdata/missing_api_key.json");
    const INVALID_KEY: &[u8] = include_bytes!("testdata/invalid_api_key.json");
    const RESTRICTED_KEY: &[u8] = include_bytes!("testdata/restricted_api_key.json");
    const NOT_VERIFIED: &[u8] = include_bytes!("testdata/domain_not_verified.json");
    const ONLY_TO_SELF: &[u8] = include_bytes!("testdata/testing_only_to_self.json");
    const BAD_FROM: &[u8] = include_bytes!("testdata/invalid_from_address.json");
    const THROTTLED: &[u8] = include_bytes!("testdata/rate_limit_exceeded.json");
    const DAILY: &[u8] = include_bytes!("testdata/daily_quota_exceeded.json");
    const MONTHLY: &[u8] = include_bytes!("testdata/monthly_quota_exceeded.json");
    const SECURITY: &[u8] = include_bytes!("testdata/security_error.json");
    const MISSING_FIELD: &[u8] = include_bytes!("testdata/missing_required_field.json");
    const BAD_PARAMETER: &[u8] = include_bytes!("testdata/invalid_parameter.json");
    const CONCURRENT: &[u8] = include_bytes!("testdata/concurrent_idempotent_requests.json");
    const REUSED_KEY: &[u8] = include_bytes!("testdata/invalid_idempotent_request.json");
    const THEIR_FAULT: &[u8] = include_bytes!("testdata/internal_server_error.json");
    const SENT: &[u8] = include_bytes!("testdata/sent.json");

    const FROM: &str = "craftpanel <panel@panel.example>";

    fn err(status: u16, body: &[u8]) -> MailError {
        interpret(status, body, FROM).expect_err("this fixture is a refusal")
    }

    #[test]
    fn every_answer_of_theirs_becomes_the_code_and_the_sentence_of_19_11() {
        let cases: &[(u16, &[u8], &str, &str)] = &[
            (401, MISSING_KEY, "mail_key_rejected", "saw no key"),
            (401, INVALID_KEY, "mail_key_rejected", "does not know this key"),
            (401, RESTRICTED_KEY, "mail_key_rejected", "Sending access"),
            (403, NOT_VERIFIED, "mail_sender_rejected", "resend.com/domains"),
            (403, ONLY_TO_SELF, "mail_sender_rejected", "opened with"),
            (422, BAD_FROM, "mail_sender_rejected", "name@domain.tld"),
            (429, THROTTLED, "mail_upstream", "throttling"),
            (429, DAILY, "mail_quota_reached", "daily allowance"),
            (429, MONTHLY, "mail_quota_reached", "monthly allowance"),
            (451, SECURITY, "mail_refused", "security risk"),
            (400, MISSING_FIELD, "mail_refused", "missing the `to` field"),
            (422, BAD_PARAMETER, "mail_refused", "ISO 8601"),
            (409, CONCURRENT, "mail_upstream", "already on its way"),
            (409, REUSED_KEY, "mail_refused", "queued again"),
            (500, THEIR_FAULT, "mail_upstream", "HTTP 500"),
            (503, b"<html>502 Bad Gateway</html>", "mail_upstream", "HTTP 503"),
        ];

        for (status, body, code, needle) in cases {
            let failure = err(*status, body);
            assert_eq!(failure.code(), *code, "{status} {}", String::from_utf8_lossy(body));
            assert!(
                failure.message().contains(needle),
                "{status}: {:?} does not mention {needle:?}",
                failure.message()
            );
        }
    }

    #[test]
    fn one_name_two_faults_and_the_status_tells_them_apart() {
        assert_eq!(err(401, INVALID_KEY).code(), "mail_key_rejected");
        assert_eq!(err(403, NOT_VERIFIED).code(), "mail_sender_rejected");
    }

    #[test]
    fn only_waiting_helps_with_some_of_them() {
        assert!(err(500, THEIR_FAULT).transient());
        assert!(err(429, THROTTLED).transient());
        assert!(err(429, DAILY).transient());
        assert!(err(429, DAILY).until_tomorrow());
        assert!(!err(429, MONTHLY).until_tomorrow());

        assert!(!err(401, INVALID_KEY).transient());
        assert!(!err(403, NOT_VERIFIED).transient());
        assert!(!err(451, SECURITY).transient());
    }

    #[test]
    fn a_refusal_carries_the_sender_so_the_admin_knows_which_address_is_meant() {
        assert!(err(403, NOT_VERIFIED).message().contains(FROM));
        assert!(err(422, BAD_FROM).message().contains(FROM));
    }

    #[test]
    fn a_success_is_resends_own_mail_id_and_nothing_else() {
        assert_eq!(interpret(200, SENT, FROM).unwrap(), "49a3999c-0ce1-4ea6-ab68-afcd6dc2e794");

        let empty = interpret(200, b"{}", FROM).unwrap_err();
        assert_eq!(empty.code(), "mail_unreadable");
        let blank = interpret(200, br#"{"id":""}"#, FROM).unwrap_err();
        assert_eq!(blank.code(), "mail_unreadable");
    }

    #[test]
    fn a_foreign_essay_does_not_get_to_fill_a_column() {
        let long = format!(r#"{{"name":"validation_error","message":"{}"}}"#, "x".repeat(500));
        let message = err(400, long.as_bytes()).message();
        assert!(message.len() < 300, "{} characters", message.len());
    }

    #[test]
    fn the_key_never_shows_itself() {
        let key = ApiKey::parse("re_pretend_this_is_real").expect("a key");
        assert_eq!(format!("{key:?}"), "ApiKey(hidden)");
        assert_eq!(key.expose(), "re_pretend_this_is_real");

        assert!(ApiKey::parse("   ").is_none());
        assert!(ApiKey::parse("").is_none());
        assert!(ApiKey::parse("re_with a space").is_none());
        assert_eq!(ApiKey::parse("re_x\n").expect("a key").expose(), "re_x");
    }

    #[test]
    fn the_body_is_the_five_fields_resend_asks_for() {
        let mail = Outgoing {
            from: FROM,
            to: "anna@example.com",
            reply_to: None,
            subject: "Confirm your email address",
            html: "<p>hi</p>",
            text: "hi",
            kind: "verify_email",
        };
        let body = Body {
            from: mail.from,
            to: [mail.to],
            reply_to: mail.reply_to,
            subject: mail.subject,
            html: mail.html,
            text: mail.text,
            tags: [Tag { name: "kind", value: mail.kind }],
        };

        let json = serde_json::to_value(&body).expect("a body");
        assert_eq!(json["from"], FROM);
        assert_eq!(json["to"], serde_json::json!(["anna@example.com"]));
        assert_eq!(json["tags"][0]["value"], "verify_email");
        assert!(json.get("reply_to").is_none(), "an absent reply_to is not sent at all");
    }

    #[tokio::test]
    #[ignore = "talks to api.resend.com"]
    async fn the_real_service_still_refuses_in_the_shape_we_parse() {
        let resend = Resend::new().expect("a client");
        let mail = Outgoing {
            from: "onboarding@resend.dev",
            to: "nobody@example.com",
            reply_to: None,
            subject: "craftpanel test",
            html: "<p>x</p>",
            text: "x",
            kind: "test",
        };

        let bogus = ApiKey::parse("re_notarealkey_000000000000").expect("a key");
        let refused = resend.send(&bogus, &mail, "01TESTTESTTESTTESTTESTTEST").await.unwrap_err();
        assert_eq!(refused.code(), "mail_key_rejected", "{refused:?}");

        if let Some(real) = std::env::var("CRAFTPANEL_RESEND_KEY").ok().and_then(|k| ApiKey::parse(&k))
        {
            let answer = resend.send(&real, &mail, "01TESTTESTTESTTESTTESTTES2").await;
            match answer {
                Ok(id) => assert!(!id.is_empty()),
                Err(err) => assert_eq!(err.code(), "mail_sender_rejected", "{err:?}"),
            }
        }
    }
}
