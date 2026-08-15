use std::time::Duration;

use super::message::{Kind, Message, Recipient, Sender, Values};
use super::resend::MailError;
use crate::model::Timestamp;

const SHELL_HTML: &str = include_str!("templates/shell.html");
const SHELL_TEXT: &str = include_str!("templates/shell.txt");
const MANUAL_LINK: &str = include_str!("templates/manual_link.html");

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rendered {
    pub subject: String,
    pub html: String,
    pub text: String,
}

pub fn render(
    message: &Message,
    sender: &Sender,
    now: Timestamp,
) -> Result<Rendered, MailError> {
    let values = message.values(sender, now)?;
    Ok(assemble(message.kind(), &values))
}

pub fn sample(kind: Kind) -> Rendered {
    let sender = Sender {
        name: "craftpanel".to_owned(),
        address: "onboarding@resend.dev".to_owned(),
        reply_to: None,
        link_base: Some("https://panel.example".to_owned()),
    };
    let to = Recipient::address("anna@example.com");
    let day = Duration::from_secs(24 * 60 * 60);
    let half_hour = Duration::from_secs(30 * 60);
    let when: Timestamp = "2026-08-13T21:10:00Z".parse().expect("a fixed example moment");

    let message = match kind {
        Kind::VerifyEmail => Message::VerifyEmail {
            to,
            username: "anna".to_owned(),
            token: SAMPLE_TOKEN.to_owned(),
            valid_for: day,
        },
        Kind::AddressAlreadyRegistered => {
            Message::AddressAlreadyRegistered { to, username: "anna".to_owned() }
        }
        Kind::AccountAwaitingReview => Message::AccountAwaitingReview {
            to,
            applicant: "anna".to_owned(),
            email: "anna@example.com".to_owned(),
            when,
        },
        Kind::AccountApproved => Message::AccountApproved { to, username: "anna".to_owned() },
        Kind::AccountRejected => Message::AccountRejected { to, username: "anna".to_owned() },
        Kind::ResetPassword => Message::ResetPassword {
            to,
            username: "anna".to_owned(),
            token: SAMPLE_TOKEN.to_owned(),
            valid_for: half_hour,
        },
        Kind::PasswordChanged => {
            Message::PasswordChanged { to, username: "anna".to_owned(), when }
        }
        Kind::Test => Message::Test { to },
    };

    render(&message, &sender, when).expect("the example sender has a link base")
}

const SAMPLE_TOKEN: &str = "EXAMPLE-TOKEN-2W4y8LqR6nT1vJ0bC7sD3fG9hK5mP";

fn assemble(kind: Kind, values: &Values) -> Rendered {
    let escaped: Vec<(&str, String)> =
        values.slots.iter().map(|(name, value)| (*name, escape(value))).collect();

    let manual = match &values.action_url {
        Some(url) => fill(MANUAL_LINK, &[("action_url", escape(url))]),
        None => String::new(),
    };

    let html = fill(
        SHELL_HTML,
        &[
            ("subject", escape(kind.subject())),
            ("preheader", escape(kind.preheader())),
            ("body", fill(body_html(kind), &escaped)),
            ("footer", escape(&values.footer)),
            ("manual_link", manual),
        ],
    );

    let text = fill(
        SHELL_TEXT,
        &[
            ("body", wrap(&fill(body_text(kind), &values.slots))),
            ("footer", wrap(&values.footer)),
        ],
    );

    Rendered { subject: kind.subject().to_owned(), html, text }
}

fn body_html(kind: Kind) -> &'static str {
    match kind {
        Kind::VerifyEmail => include_str!("templates/verify_email.html"),
        Kind::AddressAlreadyRegistered => {
            include_str!("templates/address_already_registered.html")
        }
        Kind::AccountAwaitingReview => include_str!("templates/account_awaiting_review.html"),
        Kind::AccountApproved => include_str!("templates/account_approved.html"),
        Kind::AccountRejected => include_str!("templates/account_rejected.html"),
        Kind::ResetPassword => include_str!("templates/reset_password.html"),
        Kind::PasswordChanged => include_str!("templates/password_changed.html"),
        Kind::Test => include_str!("templates/test.html"),
    }
}

fn body_text(kind: Kind) -> &'static str {
    match kind {
        Kind::VerifyEmail => include_str!("templates/verify_email.txt"),
        Kind::AddressAlreadyRegistered => include_str!("templates/address_already_registered.txt"),
        Kind::AccountAwaitingReview => include_str!("templates/account_awaiting_review.txt"),
        Kind::AccountApproved => include_str!("templates/account_approved.txt"),
        Kind::AccountRejected => include_str!("templates/account_rejected.txt"),
        Kind::ResetPassword => include_str!("templates/reset_password.txt"),
        Kind::PasswordChanged => include_str!("templates/password_changed.txt"),
        Kind::Test => include_str!("templates/test.txt"),
    }
}

const TEXT_WIDTH: usize = 78;

fn wrap(text: &str) -> String {
    let paragraphs: Vec<String> = text.split("\n\n").map(flow).filter(|p| !p.is_empty()).collect();
    paragraphs.join("\n\n")
}

fn flow(paragraph: &str) -> String {
    let mut out = String::with_capacity(paragraph.len() + 8);
    let mut column = 0;
    for word in paragraph.split_whitespace() {
        let length = word.chars().count();
        if column == 0 {
            column = length;
        } else if column + 1 + length <= TEXT_WIDTH {
            out.push(' ');
            column += 1 + length;
        } else {
            out.push('\n');
            column = length;
        }
        out.push_str(word);
    }
    out
}

fn fill(template: &str, slots: &[(&str, String)]) -> String {
    let mut out = String::with_capacity(template.len() + 256);
    let mut rest = template;

    while let Some(start) = rest.find("{{") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = after.find("}}") else {
            out.push_str(&rest[start..]);
            return out;
        };
        let name = &after[..end];
        match slots.iter().find(|(slot, _)| *slot == name) {
            Some((_, value)) => out.push_str(value),
            None => out.push_str(&format!("{{{{{name}}}}}")),
        }
        rest = &after[end + 2..];
    }

    out.push_str(rest);
    out
}

fn escape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            other => out.push(other),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    fn all_html() -> String {
        let mut all = String::new();
        all.push_str(SHELL_HTML);
        all.push_str(MANUAL_LINK);
        for kind in Kind::ALL {
            all.push_str(body_html(*kind));
        }
        all
    }

    fn placeholders(template: &str) -> BTreeSet<String> {
        let mut found = BTreeSet::new();
        let mut rest = template;
        while let Some(start) = rest.find("{{") {
            let after = &rest[start + 2..];
            let Some(end) = after.find("}}") else { break };
            found.insert(after[..end].to_owned());
            rest = &after[end + 2..];
        }
        found
    }

    #[test]
    fn every_mail_renders_with_nothing_left_unfilled() {
        for kind in Kind::ALL {
            let mail = sample(*kind);
            assert_eq!(mail.subject, kind.subject());
            assert!(!mail.html.contains("{{"), "{kind} html: {}", mail.html);
            assert!(!mail.text.contains("{{"), "{kind} text: {}", mail.text);
            assert!(mail.html.contains(&format!("<title>{}</title>", kind.subject())));
            assert!(mail.html.contains(kind.preheader()), "{kind} has no preview line");
            assert!(mail.text.trim().starts_with(kind.subject()), "{kind} text: {}", mail.text);
            assert!(mail.text.contains("Minecraft panel at https://panel.example"));
        }
    }

    #[test]
    fn the_html_and_the_text_of_a_mail_ask_for_the_same_values() {
        for kind in Kind::ALL {
            assert_eq!(
                placeholders(body_html(*kind)),
                placeholders(body_text(*kind)),
                "{kind}: the two templates disagree"
            );
        }
        assert_eq!(
            placeholders(SHELL_TEXT),
            BTreeSet::from(["body".to_owned(), "footer".to_owned()])
        );
        assert_eq!(
            placeholders(SHELL_HTML),
            BTreeSet::from([
                "body".to_owned(),
                "footer".to_owned(),
                "manual_link".to_owned(),
                "preheader".to_owned(),
                "subject".to_owned(),
            ])
        );
    }

    #[test]
    fn the_four_mails_with_a_link_repeat_it_as_text_and_the_others_have_no_block() {
        let with_link = [
            Kind::VerifyEmail,
            Kind::AddressAlreadyRegistered,
            Kind::AccountAwaitingReview,
            Kind::AccountApproved,
            Kind::ResetPassword,
        ];
        for kind in Kind::ALL {
            let mail = sample(*kind);
            if with_link.contains(kind) {
                assert!(mail.html.contains("copy and paste this address"), "{kind}");
                assert!(mail.html.contains("https://panel.example/"), "{kind}");
                assert!(mail.text.contains("https://panel.example/"), "{kind}");
            } else {
                assert!(!mail.html.contains("copy and paste this address"), "{kind}");
            }
        }
    }

    #[test]
    fn no_line_of_a_text_part_is_wider_than_the_window() {
        for kind in Kind::ALL {
            for line in sample(*kind).text.lines() {
                let columns = line.chars().count();
                let unbreakable = line.split_whitespace().count() == 1;
                assert!(columns <= 78 || unbreakable, "{kind}, {columns} columns: {line}");
            }
        }
    }

    #[test]
    fn each_link_of_a_mail_with_two_of_them_stands_at_the_end_of_its_own_line() {
        let text = sample(Kind::AddressAlreadyRegistered).text;
        for link in ["https://panel.example/login", "https://panel.example/forgot-password"] {
            let line = text
                .lines()
                .find(|line| line.contains(link))
                .unwrap_or_else(|| panic!("{link} is nowhere in: {text}"));
            assert!(line.ends_with(link), "{link} is not the end of its line: {line}");
        }
    }

    #[test]
    fn a_value_longer_than_its_placeholder_does_not_break_the_lines_around_it() {
        assert!(
            sample(Kind::PasswordChanged).text.contains("Every other session was signed out."),
            "{}",
            sample(Kind::PasswordChanged).text
        );

        let sender = Sender {
            name: "craftpanel".to_owned(),
            address: "panel@panel.example".to_owned(),
            reply_to: None,
            link_base: Some("http://minecraft.in-the-back-cellar.example:8099".to_owned()),
        };
        let message = Message::AccountRejected {
            to: Recipient::address("anna@example.com"),
            username: "anna".to_owned(),
        };

        let mail = render(&message, &sender, Timestamp::now()).expect("a link base is set");
        for line in mail.text.lines() {
            let columns = line.chars().count();
            assert!(columns <= 78 || line.split_whitespace().count() == 1, "{columns}: {line}");
        }
    }

    #[test]
    fn a_name_that_looks_like_html_is_escaped_in_the_html_and_raw_in_the_text() {
        let sender = Sender {
            name: "craftpanel".to_owned(),
            address: "panel@panel.example".to_owned(),
            reply_to: None,
            link_base: Some("https://panel.example".to_owned()),
        };
        let message = Message::AccountApproved {
            to: Recipient::address("anna@example.com"),
            username: "<script>alert('x')</script> & \"quoted\"".to_owned(),
        };

        let mail = render(&message, &sender, Timestamp::now()).expect("a link base is set");
        assert!(mail.html.contains("&lt;script&gt;alert(&#39;x&#39;)&lt;/script&gt; &amp;"));
        assert!(!mail.html.contains("<script>"), "{}", mail.html);
        assert!(mail.text.contains("<script>alert('x')</script> & \"quoted\""));
    }

    #[test]
    fn a_value_that_looks_like_a_placeholder_stays_a_value() {
        let filled = fill("a {{one}} b", &[("one", "{{two}}".to_owned()), ("two", "!".to_owned())]);
        assert_eq!(filled, "a {{two}} b");

        let unknown = fill("{{nobody}}", &[]);
        assert_eq!(unknown, "{{nobody}}", "an unknown name stays visible to the test");
    }

    #[test]
    fn every_link_in_an_href_is_http_or_https() {
        for kind in Kind::ALL {
            let html = sample(*kind).html;
            let mut rest = html.as_str();
            while let Some(at) = rest.find("href=\"") {
                let after = &rest[at + 6..];
                let end = after.find('"').expect("a closed attribute");
                let url = &after[..end];
                assert!(
                    url.starts_with("https://") || url.starts_with("http://"),
                    "{kind} links to {url}"
                );
                rest = &after[end..];
            }
        }
    }

    #[test]
    fn nothing_of_somebody_elses_brand_and_no_image_at_all() {
        let html = all_html();
        for forbidden in [
            "<img",
            "cdn.modrinth.com",
            "cdn-raw.modrinth.com",
            "modrinth",
            "Modrinth",
            "Rinth, Inc",
            "800 N King",
            "fonts.googleapis.com",
        ] {
            assert!(!html.contains(forbidden), "{forbidden:?} is in a mail template");
        }
        assert!(html.contains("font-family: Inter,"));
    }

    #[test]
    fn the_mail_says_it_is_readable_in_the_dark_without_needing_to_be() {
        assert!(SHELL_HTML.contains("@media (prefers-color-scheme: dark)"));
        assert!(SHELL_HTML.contains("background-color: #ebebeb"), "light on the element");
        assert!(SHELL_HTML.contains("content=\"light dark\""));
        assert!(SHELL_HTML.contains(".ExternalClass"));
        assert!(SHELL_HTML.contains("x-apple-data-detectors"));
        assert!(SHELL_HTML.contains("mso-hide: all"), "the preview line stays hidden in Outlook");
    }

    #[test]
    fn the_palette_is_the_one_the_interface_uses() {
        const SCSS: &str = include_str!("../../../../vendor/modrinth/assets/styles/variables.scss");

        let light = theme(SCSS, &[".light-properties", "\nhtml {"]);
        let dark = theme(SCSS, &[".light-properties", "\nhtml {", "\n.dark-mode,"]);
        let html = all_html();

        let wanted: &[(&str, &str, &str)] = &[
            ("--color-bg", "#ebebeb", "#16181c"),
            ("--color-raised-bg", "#f8f8f8", "#27292e"),
            ("--color-divider", "#dddddd", "#34363c"),
            ("--color-contrast", "#1a202c", "#ffffff"),
            ("--color-base", "#2c2e31", "#b0bac5"),
            ("--color-secondary", "#484d54", "#96a2b0"),
            ("--color-brand", "#00af5c", "#1bd96a"),
            ("--color-link", "#1f68c0", "#4f9cff"),
            ("--color-accent-contrast", "#ffffff", "#000000"),
        ];

        for (token, ours_light, ours_dark) in wanted {
            assert_eq!(
                resolve(&light, token).as_deref(),
                Some(*ours_light),
                "{token} in the light theme"
            );
            assert_eq!(
                resolve(&dark, token).as_deref(),
                Some(*ours_dark),
                "{token} in the dark theme"
            );
            assert!(html.contains(ours_light), "{token}: {ours_light} is in no template");
            assert!(
                SHELL_HTML.contains(ours_dark),
                "{token}: {ours_dark} is in no dark-mode rule"
            );
        }

        assert_eq!(resolve(&light, "--radius-md").as_deref(), Some("0.75rem"));
        assert!(html.contains("border-radius: 12px"));
    }

    #[test]
    fn no_mail_sets_type_smaller_than_the_interfaces_smallest_step() {
        const DEFAULTS: &str = include_str!("../../../../vendor/modrinth/assets/styles/defaults.scss");

        let smallest = declarations(block(DEFAULTS, "\nbody {"))
            .into_iter()
            .filter(|(name, _)| name.starts_with("--font-size-"))
            .filter_map(|(_, value)| value.strip_suffix("rem").and_then(|rem| rem.parse::<f64>().ok()))
            .map(|rem| rem * 16.0)
            .fold(f64::INFINITY, f64::min);
        assert_eq!(smallest, 10.0, "the smallest step of the scale moved");

        let floor = 12.0;
        for (label, template) in
            [("shell", SHELL_HTML), ("manual link", MANUAL_LINK)].into_iter().chain(
                Kind::ALL.iter().map(|kind| (kind.as_str(), body_html(*kind))),
            )
        {
            for size in font_sizes(template) {
                assert!(size >= floor, "{label} sets {size} px, under the {floor} px floor");
            }
        }
    }

    fn font_sizes(template: &str) -> Vec<f64> {
        let mut found = Vec::new();
        let mut rest = template;
        while let Some(at) = rest.find("font-size:") {
            rest = &rest[at + "font-size:".len()..];
            let value: String = rest.trim_start().chars().take_while(|c| c.is_ascii_digit()).collect();
            match value.parse::<f64>() {
                Ok(px) if px > 0.0 => found.push(px),
                _ => {}
            }
        }
        found
    }

    fn theme(scss: &str, selectors: &[&str]) -> Vec<(String, String)> {
        let mut all: Vec<(String, String)> = Vec::new();
        for selector in selectors {
            for (name, value) in declarations(block(scss, selector)) {
                all.retain(|(known, _)| *known != name);
                all.push((name, value));
            }
        }
        all
    }

    fn block<'a>(scss: &'a str, selector: &str) -> &'a str {
        let start =
            scss.find(selector).unwrap_or_else(|| panic!("{selector} is in the stylesheet"));
        let open = start + scss[start..].find('{').expect("an opening brace") + 1;
        let end = open + scss[open..].find("\n}").expect("a closing brace");
        &scss[open..end]
    }

    fn declarations(block: &str) -> Vec<(String, String)> {
        let mut found = Vec::new();
        for line in block.lines() {
            let line = line.trim();
            let Some(rest) = line.strip_prefix("--") else { continue };
            let Some((name, value)) = rest.split_once(':') else { continue };
            let value = value
                .split("//")
                .next()
                .unwrap_or_default()
                .replace("!important", "")
                .trim()
                .trim_end_matches(';')
                .trim()
                .to_owned();
            found.push((format!("--{}", name.trim()), value));
        }
        found
    }

    fn resolve(theme: &[(String, String)], token: &str) -> Option<String> {
        let mut name = token.to_owned();
        for _ in 0..8 {
            let value = theme.iter().rev().find(|(known, _)| *known == name)?.1.clone();
            match value.strip_prefix("var(").and_then(|rest| rest.strip_suffix(')')) {
                Some(indirection) => name = indirection.trim().to_owned(),
                None => return Some(value),
            }
        }
        None
    }

    #[test]
    fn the_resolver_follows_a_chain_of_names_to_a_colour() {
        let theme = vec![
            ("--surface-1".to_owned(), "#ebebeb".to_owned()),
            ("--color-bg".to_owned(), "var(--surface-1)".to_owned()),
            ("--color-page".to_owned(), "var(--color-bg)".to_owned()),
        ];
        assert_eq!(resolve(&theme, "--color-page").as_deref(), Some("#ebebeb"));
        assert_eq!(resolve(&theme, "--nothing"), None);
    }
}
