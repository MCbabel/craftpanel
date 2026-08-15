use std::path::Path;

use sqlx::SqlitePool;

use crate::model::{ContentProjectType, Id, Timestamp};

use super::store::{self, ItemRow};

pub const DIRECTORIES: [(&str, ContentProjectType); 3] = [
    ("mods", ContentProjectType::Mod),
    ("plugins", ContentProjectType::Plugin),
    ("world/datapacks", ContentProjectType::Datapack),
];

pub const CEILING: usize = 2_000;

pub fn capped(found: usize) -> bool {
    found >= CEILING
}

const EXTENSIONS: [&str; 2] = ["jar", "zip"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    pub path: String,
    pub size: u64,
    pub modified: Timestamp,
    pub project_type: ContentProjectType,
}

pub fn walk(root: &Path) -> Vec<Found> {
    let mut found = Vec::new();
    for (directory, project_type) in DIRECTORIES {
        let Ok(here) = super::paths::resolve(root, directory) else { continue };
        let entries = match std::fs::read_dir(&here) {
            Ok(entries) => entries,
            Err(err) => {
                if err.kind() != std::io::ErrorKind::NotFound {
                    tracing::warn!(
                        "{} could not be read, so what is in it is missing from the content \
                         list: {err}",
                        here.display()
                    );
                }
                continue;
            }
        };

        for entry in entries.flatten() {
            let Ok(meta) = entry.metadata() else { continue };
            if !meta.is_file() {
                continue;
            }
            let Ok(name) = entry.file_name().into_string() else { continue };
            if !is_content(&name) {
                continue;
            }
            found.push(Found {
                path: format!("{directory}/{name}"),
                size: meta.len(),
                modified: meta
                    .modified()
                    .map(|when| Timestamp::at(when.into()))
                    .unwrap_or_else(|_| Timestamp::now()),
                project_type,
            });
        }
    }

    found.sort_by(|left, right| left.path.cmp(&right.path));
    found.truncate(CEILING);
    found
}

fn is_content(name: &str) -> bool {
    let base = store::base_path(name);
    base.rsplit_once('.')
        .is_some_and(|(stem, extension)| {
            !stem.is_empty() && EXTENSIONS.contains(&extension.to_ascii_lowercase().as_str())
        })
}

pub async fn reconcile(
    pool: &SqlitePool,
    server: Id,
    root: &Path,
) -> Result<Vec<ItemRow>, sqlx::Error> {
    let root = root.to_path_buf();
    let found = tokio::task::spawn_blocking(move || walk(&root)).await.unwrap_or_default();
    let known = store::list(pool, server).await?;

    let mut rows = Vec::with_capacity(found.len());
    let mut matched: Vec<Id> = Vec::with_capacity(found.len());

    for file in found {
        let base = store::base_path(&file.path);
        let enabled = !file.path.ends_with(store::DISABLED);
        let existing = known
            .iter()
            .find(|row| store::base_path(&row.file_path) == base && !matched.contains(&row.id));

        let mut row = match existing {
            Some(row) => row.clone(),
            None => {
                let mut fresh = ItemRow::fresh(server, &file.path, file.project_type);
                fresh.date_added = file.modified;
                fresh
            }
        };

        let changed = row.file_path != file.path
            || row.size_bytes != file.size as i64
            || row.enabled != enabled;
        row.file_path = file.path.clone();
        row.file_name = store::file_name_of(&file.path);
        row.size_bytes = file.size as i64;
        row.enabled = enabled;
        if row.project_type != file.project_type && row.source_kind == crate::model::ContentSourceKind::Local {
            row.project_type = file.project_type;
        }

        if changed || existing.is_none() {
            store::upsert(pool, &row).await?;
        }
        matched.push(row.id);
        rows.push(row);
    }

    for gone in known.iter().filter(|row| !matched.contains(&row.id)) {
        store::remove(pool, gone.id).await?;
    }

    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::harness::{a_server, a_user, schema, DataDir};
    use crate::model::ContentSourceKind;

    fn write(root: &Path, path: &str, body: &[u8]) {
        let full = root.join(path);
        std::fs::create_dir_all(full.parent().expect("a parent")).expect("a directory");
        std::fs::write(full, body).expect("a file");
    }

    #[test]
    fn only_jars_and_zips_count_and_a_disabled_one_still_does() {
        assert!(is_content("foo.jar"));
        assert!(is_content("foo.jar.disabled"));
        assert!(is_content("pack.zip"));
        assert!(!is_content("readme.txt"));
        assert!(!is_content(".jar"));
        assert!(!is_content("foo.jar.bak"));
    }

    #[test]
    fn the_ceiling_is_measured_against_what_was_found_and_not_against_what_is_shown() {
        assert!(!capped(CEILING - 1));
        assert!(capped(CEILING));
        assert!(capped(CEILING + 1));
    }

    #[tokio::test]
    async fn a_file_dropped_in_by_hand_gets_a_row_and_keeps_its_own_date() {
        let pool = schema().await;
        let owner = a_user(&pool).await;
        let server = a_server(&pool, owner, "fabric", "1.21.1").await;
        let dir = DataDir::new();
        write(dir.path(), "mods/dropped.jar", b"jar");

        let rows = reconcile(&pool, server, dir.path()).await.expect("a scan");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].file_path, "mods/dropped.jar");
        assert_eq!(rows[0].source_kind, ContentSourceKind::Local);
        assert!(rows[0].enabled);
        assert!(rows[0].project_id.is_none(), "a file we never installed has no origin");

        let again = reconcile(&pool, server, dir.path()).await.expect("a second scan");
        assert_eq!(again[0].id, rows[0].id, "a second look must not renumber it");
    }

    #[tokio::test]
    async fn a_file_disabled_from_a_shell_keeps_the_row_it_had() {
        let pool = schema().await;
        let owner = a_user(&pool).await;
        let server = a_server(&pool, owner, "fabric", "1.21.1").await;
        let dir = DataDir::new();
        write(dir.path(), "mods/foo.jar", b"jar");

        let first = reconcile(&pool, server, dir.path()).await.expect("a scan");
        std::fs::rename(dir.path().join("mods/foo.jar"), dir.path().join("mods/foo.jar.disabled"))
            .expect("a rename outside the panel");

        let second = reconcile(&pool, server, dir.path()).await.expect("a scan");
        assert_eq!(second[0].id, first[0].id);
        assert!(!second[0].enabled);
        assert_eq!(second[0].file_path, "mods/foo.jar.disabled");
    }

    #[tokio::test]
    async fn a_file_deleted_outside_the_panel_takes_its_row_with_it() {
        let pool = schema().await;
        let owner = a_user(&pool).await;
        let server = a_server(&pool, owner, "paper", "1.21.1").await;
        let dir = DataDir::new();
        write(dir.path(), "plugins/gone.jar", b"jar");
        write(dir.path(), "plugins/stays.jar", b"jar");

        reconcile(&pool, server, dir.path()).await.expect("a scan");
        std::fs::remove_file(dir.path().join("plugins/gone.jar")).expect("a delete");

        let rows = reconcile(&pool, server, dir.path()).await.expect("a scan");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].file_path, "plugins/stays.jar");
        assert_eq!(store::list(&pool, server).await.expect("the rows").len(), 1);
    }

    #[test]
    fn the_kind_follows_the_directory_the_file_sits_in() {
        let dir = DataDir::new();
        write(dir.path(), "mods/a.jar", b"x");
        write(dir.path(), "plugins/b.jar", b"x");
        write(dir.path(), "world/datapacks/c.zip", b"x");
        write(dir.path(), "logs/latest.log", b"x");

        let found = walk(dir.path());
        let kinds: Vec<_> = found.iter().map(|file| (file.path.as_str(), file.project_type)).collect();
        assert_eq!(
            kinds,
            [
                ("mods/a.jar", ContentProjectType::Mod),
                ("plugins/b.jar", ContentProjectType::Plugin),
                ("world/datapacks/c.zip", ContentProjectType::Datapack),
            ]
        );
    }

    #[test]
    fn two_thousand_and_one_files_stop_at_the_ceiling_and_the_answer_says_so() {
        let dir = DataDir::new();
        std::fs::create_dir_all(dir.path().join("mods")).expect("a directory");
        std::fs::create_dir_all(dir.path().join("plugins")).expect("a directory");
        for index in 0..CEILING {
            std::fs::write(dir.path().join("mods").join(format!("m{index:05}.jar")), b"x")
                .expect("a mod");
        }
        std::fs::write(dir.path().join("plugins").join("late.jar"), b"x").expect("a plugin");

        let found = walk(dir.path());
        assert_eq!(found.len(), CEILING);
        assert!(capped(found.len()));
        assert!(
            found.iter().all(|file| file.path.starts_with("mods/")),
            "the ceiling bites in path order, so the plugin is the one left out"
        );

        std::fs::remove_file(dir.path().join("mods").join("m00000.jar")).expect("one fewer");
        let under = walk(dir.path());
        assert_eq!(under.len(), CEILING);
        std::fs::remove_file(dir.path().join("mods").join("m00001.jar")).expect("one fewer");
        assert!(!capped(walk(dir.path()).len()));
    }

    #[test]
    fn a_link_in_a_content_directory_is_not_content() {
        let dir = DataDir::new();
        std::fs::create_dir_all(dir.path().join("mods")).expect("a directory");
        let outside = dir.path().parent().expect("a parent").join("secret.jar");
        std::fs::write(&outside, b"panel.db").expect("a file");
        std::os::unix::fs::symlink(&outside, dir.path().join("mods").join("link.jar"))
            .expect("a link");
        write(dir.path(), "mods/real.jar", b"jar");

        let found = walk(dir.path());
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].path, "mods/real.jar");
        assert!(super::super::paths::resolve(dir.path(), "mods/link.jar").is_err());
        let _ = std::fs::remove_file(&outside);
    }
}
