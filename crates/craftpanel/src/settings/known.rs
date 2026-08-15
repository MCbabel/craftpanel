use crate::auth::error::{Failure, Result};
use crate::model::KNOWN_PROPERTY_KEYS;

const BOOLEAN: [&str; 9] = [
    "allow_cheats",
    "allow_flight",
    "enforce_whitelist",
    "force_gamemode",
    "generate_structures",
    "hardcore",
    "require_resource_pack",
    "sync_chunk_writes",
    "white_list",
];

const INTEGER: [&str; 7] = [
    "max_players",
    "max_tick_time",
    "pause_when_empty_seconds",
    "player_idle_timeout",
    "simulation_distance",
    "spawn_protection",
    "view_distance",
];

const DIFFICULTY: [&str; 4] = ["peaceful", "easy", "normal", "hard"];
const GAMEMODE: [&str; 4] = ["survival", "creative", "adventure", "spectator"];

pub const PANEL_OWNED: [&str; 2] = ["server-port", "query.port"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Key {
    pub file: String,
    pub wire: String,
    pub known: bool,
}

impl Key {
    pub fn is_panel_owned(&self) -> bool {
        PANEL_OWNED.contains(&self.file.as_str())
    }
}

pub fn resolve(name: &str) -> Result<Key> {
    let allowed =
        |byte: u8| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-');
    if name.is_empty() || !name.bytes().all(allowed) {
        return Err(Failure::bad_request(
            "invalid_property_key",
            format!("{name}: a key is [A-Za-z0-9._-]+"),
        ));
    }

    let underscored = name.replace('-', "_");
    if KNOWN_PROPERTY_KEYS.contains(&underscored.as_str()) {
        return Ok(Key {
            file: underscored.replace('_', "-"),
            wire: underscored,
            known: true,
        });
    }

    Ok(Key { file: name.to_owned(), wire: name.to_owned(), known: false })
}

pub fn from_file(file_key: &str) -> (String, bool) {
    let underscored = file_key.replace('-', "_");
    if KNOWN_PROPERTY_KEYS.contains(&underscored.as_str()) {
        (underscored, true)
    } else {
        (file_key.to_owned(), false)
    }
}

pub fn check_value(key: &Key, value: &str) -> Result<()> {
    if value.contains(['\n', '\r', '\0']) {
        return Err(refuse(key, "no line breaks and no null bytes"));
    }

    let name = key.wire.as_str();
    if INTEGER.contains(&name) && value.parse::<i64>().is_err() {
        return Err(refuse(key, "a whole number"));
    }
    if BOOLEAN.contains(&name) && !matches!(value, "true" | "false") {
        return Err(refuse(key, "true or false"));
    }
    if name == "difficulty" && !DIFFICULTY.contains(&value) {
        return Err(refuse(key, "one of peaceful, easy, normal, hard"));
    }
    if name == "gamemode" && !GAMEMODE.contains(&value) {
        return Err(refuse(key, "one of survival, creative, adventure, spectator"));
    }
    Ok(())
}

fn refuse(key: &Key, wanted: &str) -> Failure {
    Failure::bad_request(
        "invalid_property_value",
        format!("{}: {wanted}", key.wire),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_twenty_five_are_hyphenated_in_the_file_and_nothing_else_is() {
        for name in KNOWN_PROPERTY_KEYS {
            let key = resolve(name).unwrap();
            assert!(key.known, "{name} is one of the 25");
            assert_eq!(key.file, name.replace('_', "-"));
            assert_eq!(key.wire, name);
        }

        for raw in ["enable-command-block", "query.port", "rcon.password", "level-name"] {
            let key = resolve(raw).unwrap();
            assert!(!key.known, "{raw} is nobody's known key");
            assert_eq!(key.file, raw, "an unknown key keeps its raw spelling");
            assert_eq!(key.wire, raw);
        }
    }

    #[test]
    fn the_name_decides_and_not_the_bucket() {
        assert_eq!(resolve("spawn_protection").unwrap().file, "spawn-protection");
        assert_eq!(resolve("spawn-protection").unwrap().file, "spawn-protection");
        assert!(resolve("spawn-protection").unwrap().known);

        assert_eq!(resolve("level-name").unwrap().file, "level-name");
        assert_eq!(from_file("level-name"), ("level-name".to_owned(), false));
        assert_eq!(from_file("spawn-protection"), ("spawn_protection".to_owned(), true));
    }

    #[test]
    fn the_two_panel_owned_keys_are_recognised_under_both_spellings() {
        assert!(resolve("server-port").unwrap().is_panel_owned());
        assert!(resolve("query.port").unwrap().is_panel_owned());
        assert!(!resolve("rcon.port").unwrap().is_panel_owned());
    }

    #[test]
    fn a_key_outside_the_alphabet_is_refused_by_name() {
        for bad in ["", "with space", "semi;colon", "new\nline", "quote\"d", "sla/sh"] {
            let refusal = resolve(bad).unwrap_err();
            assert_eq!(refusal.code(), "invalid_property_key", "{bad:?} should be refused");
        }
        assert!(resolve("a.b_c-d0").is_ok());
    }

    #[test]
    fn every_typed_key_of_9_2_is_checked_and_names_itself() {
        let cases: Vec<(&str, &str, bool)> = vec![
            ("max_players", "20", true),
            ("max_players", "twenty", false),
            ("max_players", "-1", true),
            ("view_distance", "10", true),
            ("view_distance", "10.5", false),
            ("difficulty", "hard", true),
            ("difficulty", "Hard", false),
            ("gamemode", "creative", true),
            ("gamemode", "hard", false),
            ("white_list", "true", true),
            ("white_list", "TRUE", false),
            ("hardcore", "false", true),
            ("hardcore", "1", false),
            ("motd", "anything at all", true),
            ("motd", "two\nlines", false),
            ("level_seed", "a\0b", false),
        ];

        for (name, value, allowed) in cases {
            let key = resolve(name).unwrap();
            let outcome = check_value(&key, value);
            assert_eq!(outcome.is_ok(), allowed, "{name}={value:?}");
            if let Err(refusal) = outcome {
                assert_eq!(refusal.code(), "invalid_property_value");
                assert!(refusal.to_string().contains(name), "the message names the key: {refusal}");
            }
        }
    }

    #[test]
    fn the_typed_lists_add_up_to_the_twenty_five() {
        for name in BOOLEAN.iter().chain(INTEGER.iter()) {
            assert!(KNOWN_PROPERTY_KEYS.contains(name), "{name} is not one of the 25");
        }
        assert_eq!(BOOLEAN.len() + INTEGER.len() + 9, KNOWN_PROPERTY_KEYS.len());
    }
}
