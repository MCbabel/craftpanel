use sha2::{Digest, Sha256};

pub fn fresh() -> String {
    base64url(&rand::random::<[u8; 32]>())
}

pub fn digest(secret: &str) -> String {
    hex::encode(Sha256::digest(secret.as_bytes()))
}

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

pub fn base64url(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let padded =
            [chunk[0], chunk.get(1).copied().unwrap_or(0), chunk.get(2).copied().unwrap_or(0)];
        let packed = u32::from_be_bytes([0, padded[0], padded[1], padded[2]]);
        for shift in [18, 12, 6, 0].into_iter().take(chunk.len() + 1) {
            out.push(ALPHABET[(packed >> shift) as usize & 63] as char);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64url_matches_rfc_4648() {
        assert_eq!(base64url(b""), "");
        assert_eq!(base64url(b"f"), "Zg");
        assert_eq!(base64url(b"fo"), "Zm8");
        assert_eq!(base64url(b"foo"), "Zm9v");
        assert_eq!(base64url(b"foob"), "Zm9vYg");
        assert_eq!(base64url(b"fooba"), "Zm9vYmE");
        assert_eq!(base64url(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn base64url_spells_the_last_two_digits_url_safe() {
        assert_eq!(base64url(&[0xfb, 0xff, 0xbf]), "-_-_", "never '+' or '/'");
    }

    #[test]
    fn a_secret_carries_256_bits_and_no_two_are_alike() {
        let first = fresh();
        assert_eq!(first.len(), 43, "32 bytes in unpadded base64url");
        let mut seen = std::collections::HashSet::new();
        for _ in 0..1000 {
            assert!(seen.insert(fresh()), "a secret repeated");
        }
    }

    #[test]
    fn a_digest_is_sha_256_in_hex_and_not_the_secret() {
        let secret = fresh();
        let stored = digest(&secret);
        assert_eq!(stored.len(), 64);
        assert!(stored.chars().all(|ch| ch.is_ascii_hexdigit()));
        assert_ne!(stored, secret);
        assert_eq!(stored, digest(&secret), "the same secret always digests the same");

        assert_eq!(
            digest("abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
