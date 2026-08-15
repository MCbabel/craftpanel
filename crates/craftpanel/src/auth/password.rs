use std::sync::OnceLock;

use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, Salt, SaltString};
use argon2::{Algorithm, Argon2, Params, Version};

use super::error::{Failure, Result};

const MEMORY_KIB: u32 = 64 * 1024;
const PASSES: u32 = 3;
const LANES: u32 = 1;

pub const MIN_LENGTH: usize = 10;

#[cfg(test)]
thread_local! {
    static ARGON2_RUNS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

fn cost() -> Params {
    #[cfg(test)]
    let (memory, passes) = (Params::MIN_M_COST, 1);
    #[cfg(not(test))]
    let (memory, passes) = (MEMORY_KIB, PASSES);

    Params::new(memory, passes, LANES, None).expect("the cost parameters are in range")
}

fn argon2() -> Argon2<'static> {
    Argon2::new(Algorithm::Argon2id, Version::V0x13, cost())
}

pub fn hash(password: &str) -> Result<String> {
    check_strength(password)?;
    Ok(encode(password))
}

fn encode(password: &str) -> String {
    #[cfg(test)]
    ARGON2_RUNS.with(|count| count.set(count.get() + 1));

    argon2()
        .hash_password(password.as_bytes(), &fresh_salt())
        .expect("argon2 accepts every password we let through")
        .to_string()
}

fn fresh_salt() -> SaltString {
    let bytes: [u8; Salt::RECOMMENDED_LENGTH] = rand::random();
    SaltString::encode_b64(&bytes).expect("sixteen bytes are a valid salt")
}

pub fn check_strength(password: &str) -> Result<()> {
    if password.chars().count() < MIN_LENGTH {
        return Err(Failure::bad_request(
            "weak_password",
            format!("a password needs at least {MIN_LENGTH} characters"),
        ));
    }
    Ok(())
}

pub fn verify(password: &str, stored: &str) -> bool {
    #[cfg(test)]
    ARGON2_RUNS.with(|count| count.set(count.get() + 1));

    let Ok(parsed) = PasswordHash::new(stored) else {
        tracing::error!("a stored password hash is unreadable");
        return false;
    };
    argon2().verify_password(password.as_bytes(), &parsed).is_ok()
}

pub fn verify_against_nobody(password: &str) {
    static DECOY: OnceLock<String> = OnceLock::new();
    let decoy = DECOY.get_or_init(|| encode("no one has this password"));
    verify(password, decoy);
}

#[cfg(test)]
pub fn argon2_runs() -> u64 {
    ARGON2_RUNS.with(std::cell::Cell::get)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cost_we_ship_is_the_one_we_chose() {
        let shipped = Params::new(MEMORY_KIB, PASSES, LANES, None).unwrap();
        assert_eq!(shipped.m_cost(), 65536, "64 MiB");
        assert_eq!(shipped.t_cost(), 3);
        assert_eq!(shipped.p_cost(), 1);
        assert!(shipped.m_cost() > Params::DEFAULT_M_COST, "above the library default");
    }

    #[test]
    fn a_hash_names_argon2id_and_carries_its_own_cost() {
        let stored = hash("korrekthorsebatterystaple").unwrap();
        assert!(stored.starts_with("$argon2id$v=19$"), "{stored}");
        assert!(verify("korrekthorsebatterystaple", &stored));
        assert!(!verify("korrekthorsebatterystapl", &stored));
    }

    #[test]
    fn two_hashes_of_one_password_differ() {
        let first = hash("korrekthorsebatterystaple").unwrap();
        let second = hash("korrekthorsebatterystaple").unwrap();
        assert_ne!(first, second, "each hash carries its own salt");
    }

    #[test]
    fn nine_characters_are_too_few() {
        assert_eq!(hash("123456789").unwrap_err().code(), "weak_password");
        assert!(hash("1234567890").is_ok());
        assert!(check_strength("tencharacters").is_ok());
    }

    #[test]
    fn a_password_is_counted_in_characters_not_bytes() {
        assert!(check_strength("äöüäöüäöüä").is_ok(), "ten characters, twenty bytes");
        assert_eq!(check_strength("äöüäöüäöü").unwrap_err().code(), "weak_password");
    }

    #[test]
    fn an_unreadable_stored_hash_refuses_rather_than_admits() {
        assert!(!verify("korrekthorsebatterystaple", ""));
        assert!(!verify("korrekthorsebatterystaple", "korrekthorsebatterystaple"));
    }

    #[test]
    fn the_decoy_costs_one_verification_like_a_real_one() {
        verify_against_nobody("korrekthorsebatterystaple");

        let before = argon2_runs();
        verify_against_nobody("korrekthorsebatterystaple");
        assert_eq!(argon2_runs(), before + 1);
    }

    #[test]
    fn making_a_hash_counts_as_much_as_checking_one() {
        let before = argon2_runs();
        hash("korrekthorsebatterystaple").unwrap();
        assert_eq!(argon2_runs(), before + 1);

        let before = argon2_runs();
        assert_eq!(hash("short").unwrap_err().code(), "weak_password");
        assert_eq!(argon2_runs(), before, "a refused password is never hashed");
    }

    #[test]
    #[ignore = "a measurement, not an assertion"]
    fn measure_the_shipped_cost() {
        let argon = Argon2::new(
            Algorithm::Argon2id,
            Version::V0x13,
            Params::new(MEMORY_KIB, PASSES, LANES, None).unwrap(),
        );

        let salt = fresh_salt();
        let started = std::time::Instant::now();
        let stored = argon.hash_password(b"korrekthorsebatterystaple", &salt).unwrap().to_string();
        let hashed = started.elapsed();

        let parsed = PasswordHash::new(&stored).unwrap();
        let started = std::time::Instant::now();
        argon.verify_password(b"korrekthorsebatterystaple", &parsed).unwrap();
        println!("argon2id m={MEMORY_KIB}KiB t={PASSES} p={LANES}");
        println!("  hash   {hashed:?}");
        println!("  verify {:?}", started.elapsed());
    }
}
