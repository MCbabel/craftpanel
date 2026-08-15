use crate::auth::error::{Failure, Result};

const LOCAL_LIMIT: usize = 64;
const WHOLE_LIMIT: usize = 254;

pub fn normalise(input: &str) -> Result<String> {
    let address = input.trim().to_lowercase();

    if address.chars().any(char::is_control) {
        return Err(refuse("an address holds no control characters"));
    }
    if address.chars().any(char::is_whitespace) {
        return Err(refuse("an address holds no spaces"));
    }
    if address.chars().count() > WHOLE_LIMIT {
        return Err(refuse(format!("an address is at most {WHOLE_LIMIT} characters")));
    }

    let mut halves = address.split('@');
    let (Some(local), Some(domain), None) = (halves.next(), halves.next(), halves.next()) else {
        return Err(refuse("an address holds exactly one '@'"));
    };

    if local.is_empty() || local.chars().count() > LOCAL_LIMIT {
        return Err(refuse(format!("the part before '@' is 1 to {LOCAL_LIMIT} characters")));
    }
    if !domain.contains('.') || domain.starts_with('.') || domain.ends_with('.') {
        return Err(refuse("the part after '@' is a domain name with a dot"));
    }
    if domain.contains("..") {
        return Err(refuse("the domain holds two dots in a row"));
    }

    Ok(address)
}

fn refuse(message: impl Into<String>) -> Failure {
    Failure::bad_request("invalid_email", message)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_address_arrives_trimmed_and_folded() {
        assert_eq!(normalise("  Max@Example.TEST \n").unwrap(), "max@example.test");
        assert_eq!(normalise("max.morgan+panel@example.co.uk").unwrap(), "max.morgan+panel@example.co.uk");
    }

    #[test]
    fn two_spellings_of_one_mailbox_become_one_address() {
        assert_eq!(normalise("MAX@example.test").unwrap(), normalise("max@EXAMPLE.test").unwrap());
    }

    #[test]
    fn what_cannot_be_an_address_is_refused_with_the_contract_code() {
        for wrong in [
            "",
            "max",
            "max@",
            "@example.test",
            "max@example",
            "max@@example.test",
            "max@a@b.test",
            "max @example.test",
            "max@exa mple.test",
            "max@.example.test",
            "max@example.test.",
            "max@example..test",
        ] {
            let refusal = normalise(wrong).unwrap_err();
            assert_eq!(refusal.code(), "invalid_email", "{wrong:?} was let through");
        }
    }

    #[test]
    fn a_newline_cannot_be_smuggled_into_a_mail_header() {
        assert_eq!(
            normalise("max@example.test\r\nbcc: victim@example.test").unwrap_err().code(),
            "invalid_email"
        );
        assert_eq!(normalise("max\u{0}@example.test").unwrap_err().code(), "invalid_email");
    }

    #[test]
    fn the_two_lengths_of_rfc_5321_are_the_limits() {
        let local = "a".repeat(LOCAL_LIMIT);
        assert!(normalise(&format!("{local}@example.test")).is_ok());
        assert_eq!(
            normalise(&format!("{}@example.test", "a".repeat(LOCAL_LIMIT + 1))).unwrap_err().code(),
            "invalid_email"
        );

        let domain = format!("{}.test", "b".repeat(WHOLE_LIMIT - LOCAL_LIMIT - 1 - ".test".len()));
        let whole = format!("{local}@{domain}");
        assert_eq!(whole.chars().count(), WHOLE_LIMIT);
        assert!(normalise(&whole).is_ok());
        assert_eq!(normalise(&format!("b{whole}")).unwrap_err().code(), "invalid_email");
    }

    #[test]
    fn a_plus_tag_stays_its_own_address() {
        assert_ne!(
            normalise("max+one@example.test").unwrap(),
            normalise("max@example.test").unwrap(),
            "provider-specific folding would refuse real addresses elsewhere (20.10)"
        );
        assert_ne!(
            normalise("m.ax@example.test").unwrap(),
            normalise("max@example.test").unwrap()
        );
    }
}
