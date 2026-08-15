use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

use super::known::{self, Key};
use super::properties::Properties;
use crate::auth::error::{Failure, Result};
use crate::model::{Id, KnownProperties, Timestamp};

pub const FILE: &str = "server.properties";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ServerProperties {
    pub known: KnownProperties,
    pub custom: BTreeMap<String, String>,
    pub restart_required: bool,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ServerPropertiesPatch {
    pub known: BTreeMap<String, Option<String>>,
    pub custom: BTreeMap<String, Option<String>>,
}

impl ServerPropertiesPatch {
    fn entries(&self) -> impl Iterator<Item = (&String, &Option<String>)> {
        self.known.iter().chain(self.custom.iter())
    }
}

pub fn path(dir: &Path) -> PathBuf {
    dir.join(FILE)
}

pub fn read(dir: &Path) -> Result<Properties> {
    match super::disk::read(dir, FILE) {
        Ok(Some(bytes)) => Ok(Properties::parse(&decode(bytes))),
        Ok(None) => Ok(Properties::default()),
        Err(err) => {
            Err(Failure::internal(anyhow::Error::from(err).context("reading server.properties")))
        }
    }
}

fn decode(bytes: Vec<u8>) -> String {
    match String::from_utf8(bytes) {
        Ok(text) => text,
        Err(err) => err.into_bytes().into_iter().map(|byte| byte as char).collect(),
    }
}

pub fn write(dir: &Path, properties: &Properties) -> Result<()> {
    super::disk::write(dir, FILE, properties.render().as_bytes()).map_err(|err| {
        Failure::internal(anyhow::Error::from(err).context("writing server.properties"))
    })
}

pub fn view(properties: &Properties, restart_required: bool) -> ServerProperties {
    let mut known = KnownProperties::default();
    let mut custom = BTreeMap::new();

    for (file_key, value) in properties.entries() {
        let (name, is_known) = known::from_file(file_key);
        if is_known {
            known.set(&name, Some(value.to_owned()));
        } else {
            custom.insert(name, value.to_owned());
        }
    }

    ServerProperties { known, custom, restart_required }
}

pub fn plan(patch: &ServerPropertiesPatch) -> Result<Vec<(Key, Option<String>)>> {
    let mut wanted = Vec::new();
    for (name, value) in patch.entries() {
        let key = known::resolve(name)?;
        if key.is_panel_owned() {
            return Err(Failure::conflict(
                "property_is_panel_owned",
                format!("{} is written by the panel; change the port instead", key.file),
            ));
        }
        if let Some(value) = value {
            known::check_value(&key, value)?;
        }
        wanted.push((key, value.clone()));
    }
    Ok(wanted)
}

pub fn apply(properties: &mut Properties, wanted: &[(Key, Option<String>)]) {
    for (key, value) in wanted {
        match value {
            Some(value) => properties.set(&key.file, value),
            None => {
                properties.remove(&key.file);
            }
        }
    }
}

pub async fn queue(
    pool: &SqlitePool,
    server: Id,
    wanted: &[(Key, Option<String>)],
) -> sqlx::Result<()> {
    let now = Timestamp::now();
    for (key, value) in wanted {
        sqlx::query(
            "INSERT INTO server_property_overrides (server_id, key, value, queued_at) \
             VALUES (?, ?, ?, ?) \
             ON CONFLICT (server_id, key) DO UPDATE SET value = excluded.value, \
             queued_at = excluded.queued_at",
        )
        .bind(server)
        .bind(&key.file)
        .bind(value.as_deref())
        .bind(now)
        .execute(pool)
        .await?;
    }
    Ok(())
}

pub async fn pending(pool: &SqlitePool, server: Id) -> sqlx::Result<Vec<(String, Option<String>)>> {
    sqlx::query_as("SELECT key, value FROM server_property_overrides WHERE server_id = ? ORDER BY key")
        .bind(server)
        .fetch_all(pool)
        .await
}

pub async fn has_pending(pool: &SqlitePool, server: Id) -> sqlx::Result<bool> {
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM server_property_overrides WHERE server_id = ?")
            .bind(server)
            .fetch_one(pool)
            .await?;
    Ok(count > 0)
}

pub async fn replay(pool: &SqlitePool, server: Id, dir: &Path) -> Result<usize> {
    let queued = pending(pool, server).await?;
    if queued.is_empty() {
        return Ok(0);
    }

    let mut properties = read(dir)?;
    for (key, value) in &queued {
        match value {
            Some(value) => properties.set(key, value),
            None => {
                properties.remove(key);
            }
        }
    }
    write(dir, &properties)?;

    sqlx::query("DELETE FROM server_property_overrides WHERE server_id = ?")
        .bind(server)
        .execute(pool)
        .await?;
    Ok(queued.len())
}

pub fn set_ports(properties: &mut Properties, port: u16) {
    properties.set("server-port", &port.to_string());
    properties.set("query.port", &port.to_string());
}

pub fn port_overrides(port: u16) -> Vec<(Key, Option<String>)> {
    known::PANEL_OWNED
        .iter()
        .map(|file| {
            (
                Key { file: (*file).to_owned(), wire: (*file).to_owned(), known: false },
                Some(port.to_string()),
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::harness::{a_dir, pool_with_server};

    fn patch(json: serde_json::Value) -> ServerPropertiesPatch {
        serde_json::from_value(json).expect("a patch body")
    }

    #[test]
    fn a_missing_file_reads_as_two_empty_buckets_and_not_as_an_error() {
        let dir = a_dir();
        let view = view(&read(dir.path()).unwrap(), false);

        assert_eq!(view.known, KnownProperties::default());
        assert!(view.custom.is_empty());
    }

    #[test]
    fn the_twenty_five_land_in_known_and_the_rest_in_custom_with_their_raw_names() {
        let dir = a_dir();
        std::fs::write(
            path(dir.path()),
            "#header\nmotd=Hello\nspawn-protection=16\nenable-command-block=false\nquery.port=25565\n",
        )
        .unwrap();

        let view = view(&read(dir.path()).unwrap(), false);
        assert_eq!(view.known.motd.as_deref(), Some("Hello"));
        assert_eq!(view.known.spawn_protection.as_deref(), Some("16"));
        assert_eq!(view.custom.get("enable-command-block").map(String::as_str), Some("false"));
        assert_eq!(view.custom.get("query.port").map(String::as_str), Some("25565"));
        assert!(!view.custom.contains_key("spawn-protection"), "a known key is in one bucket only");
    }

    #[test]
    fn a_patch_writes_only_what_it_names_and_leaves_the_rest_standing() {
        let dir = a_dir();
        std::fs::write(
            path(dir.path()),
            "#header\nmotd=Alt\nview-distance=10\nenable-command-block=false\n",
        )
        .unwrap();

        let wanted = plan(&patch(serde_json::json!({
            "known": { "motd": "New", "difficulty": "hard", "view_distance": null },
            "custom": { "enable-rcon": "true" }
        })))
        .unwrap();

        let mut properties = read(dir.path()).unwrap();
        apply(&mut properties, &wanted);
        write(dir.path(), &properties).unwrap();

        let written = std::fs::read_to_string(path(dir.path())).unwrap();
        assert!(written.starts_with("#header\n"), "the comment survives: {written}");
        assert!(written.contains("motd=New\n"));
        assert!(written.contains("difficulty=hard\n"), "9.2 lets a patch make a key");
        assert!(written.contains("enable-rcon=true\n"));
        assert!(written.contains("enable-command-block=false\n"), "unnamed lines stay");
        assert!(!written.contains("view-distance"), "null takes the line out");
    }

    #[test]
    fn the_two_panel_owned_keys_are_refused_from_either_bucket() {
        for body in [
            serde_json::json!({ "custom": { "server-port": "25577" } }),
            serde_json::json!({ "custom": { "query.port": "25577" } }),
            serde_json::json!({ "known": { "server-port": "25577" } }),
        ] {
            let refusal = plan(&patch(body.clone())).unwrap_err();
            assert_eq!(refusal.code(), "property_is_panel_owned", "{body}");
            assert_eq!(refusal.status(), axum::http::StatusCode::CONFLICT);
        }
    }

    #[test]
    fn nothing_is_written_when_one_key_of_the_patch_is_wrong() {
        let dir = a_dir();
        std::fs::write(path(dir.path()), "motd=Alt\n").unwrap();

        let refused = plan(&patch(serde_json::json!({
            "known": { "motd": "New", "max_players": "many" }
        })));
        assert_eq!(refused.unwrap_err().code(), "invalid_property_value");
        assert_eq!(std::fs::read_to_string(path(dir.path())).unwrap(), "motd=Alt\n");
    }

    #[tokio::test]
    async fn what_was_changed_while_the_server_ran_is_played_in_again_afterwards() {
        let dir = a_dir();
        let (pool, server, _owner) = pool_with_server().await;
        std::fs::write(path(dir.path()), "motd=Alt\nview-distance=10\n").unwrap();

        let wanted = plan(&patch(serde_json::json!({
            "known": { "motd": "New", "view_distance": null }
        })))
        .unwrap();
        queue(&pool, server, &wanted).await.unwrap();
        assert!(has_pending(&pool, server).await.unwrap());

        std::fs::write(path(dir.path()), "motd=Alt\nview-distance=10\n").unwrap();

        assert_eq!(replay(&pool, server, dir.path()).await.unwrap(), 2);
        let written = std::fs::read_to_string(path(dir.path())).unwrap();
        assert!(written.contains("motd=New\n"), "{written}");
        assert!(!written.contains("view-distance"), "{written}");

        assert!(!has_pending(&pool, server).await.unwrap(), "the queue empties behind itself");
        assert_eq!(replay(&pool, server, dir.path()).await.unwrap(), 0);
    }

    #[tokio::test]
    async fn the_newest_value_of_a_key_is_the_one_that_gets_replayed() {
        let dir = a_dir();
        let (pool, server, _owner) = pool_with_server().await;
        std::fs::write(path(dir.path()), "motd=Alt\n").unwrap();

        for value in ["One", "Two", "Three"] {
            let wanted = plan(&patch(serde_json::json!({ "known": { "motd": value } }))).unwrap();
            queue(&pool, server, &wanted).await.unwrap();
        }

        assert_eq!(pending(&pool, server).await.unwrap().len(), 1, "one row per key");
        replay(&pool, server, dir.path()).await.unwrap();
        assert!(std::fs::read_to_string(path(dir.path())).unwrap().contains("motd=Three\n"));
    }

    #[test]
    fn the_scratch_file_never_stays_behind() {
        let dir = a_dir();
        let mut properties = Properties::default();
        properties.set("motd", "x");
        write(dir.path(), &properties).unwrap();

        let left: Vec<String> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(left, [FILE]);
    }
}
