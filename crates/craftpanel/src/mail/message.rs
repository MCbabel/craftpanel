use std::fmt;
use std::str::FromStr;
use std::time::Duration;

use serde::Serialize;

use super::resend::MailError;
use crate::model::{Id, Timestamp};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    VerifyEmail,
    AddressAlreadyRegistered,
    AccountAwaitingReview,
    AccountApproved,
    AccountRejected,
    ResetPassword,
    PasswordChanged,
    Test,
}

impl Kind {
    pub const ALL: &'static [Self] = &[
        Self::VerifyEmail,
        Self::AddressAlreadyRegistered,
        Self::AccountAwaitingReview,
        Self::AccountApproved,
        Self::AccountRejected,
        Self::ResetPassword,
        Self::PasswordChanged,
        Self::Test,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::VerifyEmail => "verify_email",
            Self::AddressAlreadyRegistered => "address_already_registered",
            Self::AccountAwaitingReview => "account_awaiting_review",
            Self::AccountApproved => "account_approved",
            Self::AccountRejected => "account_rejected",
            Self::ResetPassword => "reset_password",
            Self::PasswordChanged => "password_changed",
            Self::Test => "test",
        }
    }

    pub const fn subject(self) -> &'static str {
        match self {
            Self::VerifyEmail => "Confirm your email address",
            Self::AddressAlreadyRegistered => "Someone tried to sign up with your address",
            Self::AccountAwaitingReview => "A new account is waiting for you",
            Self::AccountApproved => "Your account is ready",
            Self::AccountRejected => "About your sign-up",
            Self::ResetPassword => "Reset your password",
            Self::PasswordChanged => "Your password was changed",
            Self::Test => "Test mail from your panel",
        }
    }

    pub const fn preheader(self) -> &'static str {
        match self {
            Self::VerifyEmail => "Confirm your address to finish signing up.",
            Self::AddressAlreadyRegistered => {
                "Your account is untouched — here is what happened and what to do."
            }
            Self::AccountAwaitingReview => "Somebody signed up and is waiting to be let in.",
            Self::AccountApproved => "You can sign in now.",
            Self::AccountRejected => "Your sign-up was not accepted.",
            Self::ResetPassword => "Set a new password — the link works once.",
            Self::PasswordChanged => "Your panel password was changed.",
            Self::Test => "If you can read this, your Resend setup works.",
        }
    }
}

impl fmt::Display for Kind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Kind {
    type Err = ();

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        Self::ALL.iter().copied().find(|kind| kind.as_str() == text).ok_or(())
    }
}

#[derive(Debug, Clone)]
pub struct Recipient {
    pub address: String,
    pub user: Option<Id>,
}

impl Recipient {
    pub fn account(user: Id, address: impl Into<String>) -> Self {
        Self { address: address.into(), user: Some(user) }
    }

    pub fn address(address: impl Into<String>) -> Self {
        Self { address: address.into(), user: None }
    }
}

#[derive(Debug, Clone)]
pub enum Message {
    VerifyEmail { to: Recipient, username: String, token: String, valid_for: Duration },
    AddressAlreadyRegistered { to: Recipient, username: String },
    AccountAwaitingReview { to: Recipient, applicant: String, email: String, when: Timestamp },
    AccountApproved { to: Recipient, username: String },
    AccountRejected { to: Recipient, username: String },
    ResetPassword { to: Recipient, username: String, token: String, valid_for: Duration },
    PasswordChanged { to: Recipient, username: String, when: Timestamp },
    Test { to: Recipient },
}

impl Message {
    pub fn kind(&self) -> Kind {
        match self {
            Self::VerifyEmail { .. } => Kind::VerifyEmail,
            Self::AddressAlreadyRegistered { .. } => Kind::AddressAlreadyRegistered,
            Self::AccountAwaitingReview { .. } => Kind::AccountAwaitingReview,
            Self::AccountApproved { .. } => Kind::AccountApproved,
            Self::AccountRejected { .. } => Kind::AccountRejected,
            Self::ResetPassword { .. } => Kind::ResetPassword,
            Self::PasswordChanged { .. } => Kind::PasswordChanged,
            Self::Test { .. } => Kind::Test,
        }
    }

    pub fn to(&self) -> &Recipient {
        match self {
            Self::VerifyEmail { to, .. }
            | Self::AddressAlreadyRegistered { to, .. }
            | Self::AccountAwaitingReview { to, .. }
            | Self::AccountApproved { to, .. }
            | Self::AccountRejected { to, .. }
            | Self::ResetPassword { to, .. }
            | Self::PasswordChanged { to, .. }
            | Self::Test { to } => to,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Sender {
    pub name: String,
    pub address: String,
    pub reply_to: Option<String>,
    pub link_base: Option<String>,
}

impl Sender {
    pub fn header(&self) -> String {
        if self.name.is_empty() {
            self.address.clone()
        } else {
            format!("{} <{}>", self.name, self.address)
        }
    }

    pub fn contact(&self) -> &str {
        self.reply_to.as_deref().unwrap_or(&self.address)
    }
}

#[derive(Debug)]
pub struct Values {
    pub slots: Vec<(&'static str, String)>,
    pub action_url: Option<String>,
    pub footer: String,
}

impl Message {
    pub fn values(&self, sender: &Sender, now: Timestamp) -> Result<Values, MailError> {
        let base = sender.link_base.as_deref();
        let mut slots: Vec<(&'static str, String)> = Vec::new();
        let mut action_url = None;

        match self {
            Self::VerifyEmail { username, token, valid_for, .. } => {
                let url = format!("{}/verify-email#{}", link(base, self.kind())?, encode(token));
                slots.push(("username", username.clone()));
                slots.push(("valid_for", spell(*valid_for)));
                slots.push(("action_url", url.clone()));
                action_url = Some(url);
            }
            Self::AddressAlreadyRegistered { username, .. } => {
                let base = link(base, self.kind())?;
                let url = format!("{base}/login");
                slots.push(("username", username.clone()));
                slots.push(("action_url", url.clone()));
                slots.push(("forgot_url", format!("{base}/forgot-password")));
                action_url = Some(url);
            }
            Self::AccountAwaitingReview { applicant, email, when, .. } => {
                let url = format!("{}/admin/registrations", link(base, self.kind())?);
                slots.push(("applicant", applicant.clone()));
                slots.push(("applicant_email", email.clone()));
                slots.push(("when", moment(*when)));
                slots.push(("action_url", url.clone()));
                action_url = Some(url);
            }
            Self::AccountApproved { username, .. } => {
                let url = format!("{}/login", link(base, self.kind())?);
                slots.push(("username", username.clone()));
                slots.push(("action_url", url.clone()));
                action_url = Some(url);
            }
            Self::AccountRejected { username, .. } => {
                slots.push(("username", username.clone()));
            }
            Self::ResetPassword { username, token, valid_for, .. } => {
                let url =
                    format!("{}/reset-password#{}", link(base, self.kind())?, encode(token));
                slots.push(("username", username.clone()));
                slots.push(("valid_for", spell(*valid_for)));
                slots.push(("action_url", url.clone()));
                action_url = Some(url);
            }
            Self::PasswordChanged { username, when, .. } => {
                slots.push(("username", username.clone()));
                slots.push(("when", moment(*when)));
                slots.push(("contact", sender.contact().to_owned()));
            }
            Self::Test { .. } => {
                slots.push(("from", sender.header()));
                slots.push(("when", moment(now)));
            }
        }

        Ok(Values { slots, action_url, footer: self.footer(base) })
    }

    fn footer(&self, base: Option<&str>) -> String {
        let signup = matches!(
            self,
            Self::VerifyEmail { .. } | Self::AddressAlreadyRegistered { .. }
        );
        let reason = if signup {
            "because this address was used to sign up there"
        } else {
            "because an account there uses this address"
        };
        match base {
            Some(base) => format!("This mail comes from the Minecraft panel at {base}, {reason}."),
            None => format!("This mail comes from a Minecraft panel, {reason}."),
        }
    }
}

fn link(base: Option<&str>, kind: Kind) -> Result<&str, MailError> {
    base.ok_or_else(|| {
        MailError::NoLinkBase(format!(
            "The {kind} mail carries a link, and this panel has no address to build one from. \
             Set the panel address under Administration → Mail."
        ))
    })
}

fn encode(token: &str) -> String {
    let mut out = String::with_capacity(token.len());
    for byte in token.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

fn spell(span: Duration) -> String {
    let minutes = span.as_secs() / 60;
    let (count, unit) = if minutes % (60 * 24) == 0 && minutes > 60 * 48 {
        (minutes / (60 * 24), "day")
    } else if minutes % 60 == 0 && minutes >= 60 {
        (minutes / 60, "hour")
    } else {
        (minutes.max(1), "minute")
    };
    if count == 1 {
        format!("1 {unit}")
    } else {
        format!("{count} {unit}s")
    }
}

fn moment(when: Timestamp) -> String {
    let at = when.as_datetime();
    format!(
        "{:04}-{:02}-{:02} at {:02}:{:02} UTC",
        at.year(),
        u8::from(at.month()),
        at.day(),
        at.hour(),
        at.minute()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sender(base: Option<&str>) -> Sender {
        Sender {
            name: "craftpanel".to_owned(),
            address: "onboarding@resend.dev".to_owned(),
            reply_to: None,
            link_base: base.map(str::to_owned),
        }
    }

    fn at(text: &str) -> Timestamp {
        text.parse().expect("a timestamp")
    }

    #[test]
    fn the_eight_kinds_spell_the_same_in_the_column_and_on_the_wire() {
        assert_eq!(Kind::ALL.len(), 8);
        for kind in Kind::ALL {
            assert_eq!(kind.as_str().parse::<Kind>().expect("a kind"), *kind);
            assert_eq!(serde_json::to_value(kind).unwrap(), kind.as_str());
            assert!(!kind.subject().is_empty());
            assert!(!kind.preheader().is_empty());
        }
        assert!("nonsense".parse::<Kind>().is_err());
    }

    #[test]
    fn the_confirmation_link_keeps_the_token_out_of_every_server_log() {
        let message = Message::VerifyEmail {
            to: Recipient::address("anna@example.com"),
            username: "anna".to_owned(),
            token: "abc-123_XYZ".to_owned(),
            valid_for: Duration::from_secs(24 * 60 * 60),
        };
        let values = message.values(&sender(Some("https://panel.example")), Timestamp::now())
            .expect("a link base is set");

        let url = values.action_url.expect("a link");
        assert_eq!(url, "https://panel.example/verify-email#abc-123_XYZ");
        assert!(!url.contains('?'), "a query would reach the server: {url}");
        assert!(values
            .slots
            .iter()
            .any(|(name, value)| *name == "valid_for" && value == "24 hours"));
    }

    #[test]
    fn the_reset_link_keeps_its_token_out_of_every_server_log_too() {
        let message = Message::ResetPassword {
            to: Recipient::address("anna@example.com"),
            username: "anna".to_owned(),
            token: "tok".to_owned(),
            valid_for: Duration::from_secs(30 * 60),
        };
        let values =
            message.values(&sender(Some("https://panel.example")), Timestamp::now()).expect("ok");
        let url = values.action_url.expect("a link");
        assert_eq!(url, "https://panel.example/reset-password#tok");
        assert!(!url.contains('?'), "a query would reach the server: {url}");
        assert!(values.slots.iter().any(|(n, v)| *n == "valid_for" && v == "30 minutes"));
    }

    #[test]
    fn without_a_panel_address_exactly_the_four_mails_with_a_link_refuse() {
        let now = at("2026-08-13T21:10:00Z");
        let sender = sender(None);
        let to = || Recipient::address("anna@example.com");

        let with_link: Vec<Message> = vec![
            Message::VerifyEmail {
                to: to(),
                username: "anna".to_owned(),
                token: "t".to_owned(),
                valid_for: Duration::from_secs(60),
            },
            Message::AddressAlreadyRegistered { to: to(), username: "anna".to_owned() },
            Message::AccountAwaitingReview {
                to: to(),
                applicant: "anna".to_owned(),
                email: "anna@example.com".to_owned(),
                when: now,
            },
            Message::AccountApproved { to: to(), username: "anna".to_owned() },
        ];
        for message in with_link {
            let refusal = message.values(&sender, now).expect_err("no base, no link");
            assert_eq!(refusal.code(), "mail_no_link_base", "{:?}", message.kind());
            assert!(refusal.message().contains(message.kind().as_str()));
        }

        let without_link: Vec<Message> = vec![
            Message::AccountRejected { to: to(), username: "anna".to_owned() },
            Message::ResetPassword {
                to: to(),
                username: "anna".to_owned(),
                token: "t".to_owned(),
                valid_for: Duration::from_secs(60),
            },
            Message::PasswordChanged { to: to(), username: "anna".to_owned(), when: now },
            Message::Test { to: to() },
        ];
        for message in without_link {
            let answer = message.values(&sender, now);
            if matches!(message, Message::ResetPassword { .. }) {
                assert_eq!(answer.expect_err("a link").code(), "mail_no_link_base");
            } else {
                let values = answer.expect("no link, no base needed");
                assert!(values.action_url.is_none());
                assert!(values.footer.contains("a Minecraft panel"), "{}", values.footer);
            }
        }
    }

    #[test]
    fn a_token_that_is_not_ours_cannot_leave_the_url() {
        assert_eq!(encode("plain-_.~AZ09"), "plain-_.~AZ09");
        assert_eq!(encode("a b"), "a%20b");
        assert_eq!(encode("\"onmouseover=x"), "%22onmouseover%3Dx");
        assert_eq!(encode("a#b?c"), "a%23b%3Fc");
    }

    #[test]
    fn spans_read_like_a_sentence() {
        assert_eq!(spell(Duration::from_secs(30 * 60)), "30 minutes");
        assert_eq!(spell(Duration::from_secs(60)), "1 minute");
        assert_eq!(spell(Duration::from_secs(2 * 60 * 60)), "2 hours");
        assert_eq!(spell(Duration::from_secs(24 * 60 * 60)), "24 hours");
        assert_eq!(spell(Duration::from_secs(7 * 24 * 60 * 60)), "7 days");
        assert_eq!(spell(Duration::from_secs(90 * 60)), "90 minutes");
        assert_eq!(spell(Duration::from_secs(5)), "1 minute");
    }

    #[test]
    fn the_from_header_takes_both_shapes_resend_accepts() {
        let mut sender = sender(None);
        assert_eq!(sender.header(), "craftpanel <onboarding@resend.dev>");
        assert_eq!(sender.contact(), "onboarding@resend.dev");

        sender.name.clear();
        assert_eq!(sender.header(), "onboarding@resend.dev");

        sender.reply_to = Some("hello@panel.example".to_owned());
        assert_eq!(sender.contact(), "hello@panel.example");
    }

    #[test]
    fn a_moment_is_written_out_for_a_reader() {
        assert_eq!(moment(at("2026-08-13T21:10:07Z")), "2026-08-13 at 21:10 UTC");
    }
}
