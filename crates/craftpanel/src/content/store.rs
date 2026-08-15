use sqlx::SqlitePool;

use crate::model::{
    ContentProjectType, ContentSourceKind, Id, ModpackSourceKind, Timestamp,
};

pub type Result<T> = std::result::Result<T, sqlx::Error>;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ItemRow {
    pub id: Id,
    pub server_id: Id,
    pub file_name: String,
    pub file_path: String,
    pub size_bytes: i64,
    pub enabled: bool,
    pub locked: bool,
    pub project_type: ContentProjectType,
    pub source_kind: ContentSourceKind,
    pub environment: Option<String>,
    pub pack_client_depends: bool,
    pub external: bool,
    pub external_url: Option<String>,
    pub sha512: Option<String>,
    pub project_id: Option<String>,
    pub version_id: Option<String>,
    pub has_update: bool,
    pub update_version_id: Option<String>,
    pub date_added: Timestamp,
}

const COLUMNS: &str = "id, server_id, file_name, file_path, size_bytes, enabled, locked,
     project_type, source_kind, environment, pack_client_depends, external, external_url,
     sha512, project_id, version_id, has_update, update_version_id, date_added";

impl ItemRow {
    pub fn fresh(server_id: Id, file_path: &str, project_type: ContentProjectType) -> Self {
        Self {
            id: Id::new(),
            server_id,
            file_name: file_name_of(file_path),
            file_path: file_path.to_owned(),
            size_bytes: 0,
            enabled: !file_path.ends_with(DISABLED),
            locked: false,
            project_type,
            source_kind: ContentSourceKind::Local,
            environment: None,
            pack_client_depends: false,
            external: false,
            external_url: None,
            sha512: None,
            project_id: None,
            version_id: None,
            has_update: false,
            update_version_id: None,
            date_added: Timestamp::now(),
        }
    }
}

pub const DISABLED: &str = ".disabled";

pub fn file_name_of(path: &str) -> String {
    path.rsplit('/').next().unwrap_or(path).to_owned()
}

pub fn base_path(path: &str) -> &str {
    path.strip_suffix(DISABLED).unwrap_or(path)
}

pub fn toggled(path: &str, enabled: bool) -> String {
    let base = base_path(path);
    if enabled {
        base.to_owned()
    } else {
        format!("{base}{DISABLED}")
    }
}

pub async fn list(pool: &SqlitePool, server: Id) -> Result<Vec<ItemRow>> {
    sqlx::query_as(&format!(
        "SELECT {COLUMNS} FROM content_items WHERE server_id = ? ORDER BY file_path"
    ))
    .bind(server)
    .fetch_all(pool)
    .await
}

pub async fn of_kind(
    pool: &SqlitePool,
    server: Id,
    from_modpack: bool,
) -> Result<Vec<ItemRow>> {
    let comparison = if from_modpack { "=" } else { "<>" };
    sqlx::query_as(&format!(
        "SELECT {COLUMNS} FROM content_items
          WHERE server_id = ? AND source_kind {comparison} 'modrinth_modpack'
          ORDER BY file_path"
    ))
    .bind(server)
    .fetch_all(pool)
    .await
}

pub async fn by_path(pool: &SqlitePool, server: Id, path: &str) -> Result<Option<ItemRow>> {
    let base = base_path(path);
    sqlx::query_as(&format!(
        "SELECT {COLUMNS} FROM content_items
          WHERE server_id = ? AND file_path IN (?, ?) ORDER BY file_path LIMIT 1"
    ))
    .bind(server)
    .bind(base)
    .bind(format!("{base}{DISABLED}"))
    .fetch_optional(pool)
    .await
}

pub async fn one(pool: &SqlitePool, server: Id, id: Id) -> Result<Option<ItemRow>> {
    sqlx::query_as(&format!(
        "SELECT {COLUMNS} FROM content_items WHERE server_id = ? AND id = ?"
    ))
    .bind(server)
    .bind(id)
    .fetch_optional(pool)
    .await
}

pub async fn upsert(pool: &SqlitePool, row: &ItemRow) -> Result<()> {
    sqlx::query(
        "INSERT INTO content_items (id, server_id, file_name, file_path, size_bytes, enabled,
                                    locked, project_type, source_kind, environment,
                                    pack_client_depends, external, external_url, sha512,
                                    project_id, version_id, has_update, update_version_id,
                                    date_added)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT (id) DO UPDATE
            SET file_name = excluded.file_name, file_path = excluded.file_path,
                size_bytes = excluded.size_bytes, enabled = excluded.enabled,
                locked = excluded.locked, project_type = excluded.project_type,
                source_kind = excluded.source_kind, environment = excluded.environment,
                pack_client_depends = excluded.pack_client_depends,
                external = excluded.external, external_url = excluded.external_url,
                sha512 = excluded.sha512, project_id = excluded.project_id,
                version_id = excluded.version_id, has_update = excluded.has_update,
                update_version_id = excluded.update_version_id",
    )
    .bind(row.id)
    .bind(row.server_id)
    .bind(&row.file_name)
    .bind(&row.file_path)
    .bind(row.size_bytes)
    .bind(row.enabled)
    .bind(row.locked)
    .bind(row.project_type)
    .bind(row.source_kind)
    .bind(&row.environment)
    .bind(row.pack_client_depends)
    .bind(row.external)
    .bind(&row.external_url)
    .bind(&row.sha512)
    .bind(&row.project_id)
    .bind(&row.version_id)
    .bind(row.has_update)
    .bind(&row.update_version_id)
    .bind(row.date_added)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn move_to(pool: &SqlitePool, id: Id, path: &str, enabled: bool) -> Result<()> {
    sqlx::query(
        "UPDATE content_items SET file_path = ?, file_name = ?, enabled = ? WHERE id = ?",
    )
    .bind(path)
    .bind(file_name_of(path))
    .bind(enabled)
    .bind(id)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn remove(pool: &SqlitePool, id: Id) -> Result<()> {
    sqlx::query("DELETE FROM content_items WHERE id = ?").bind(id).execute(pool).await?;
    Ok(())
}

pub async fn set_update(
    pool: &SqlitePool,
    id: Id,
    update_version_id: Option<&str>,
) -> Result<()> {
    sqlx::query("UPDATE content_items SET has_update = ?, update_version_id = ? WHERE id = ?")
        .bind(update_version_id.is_some())
        .bind(update_version_id)
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn checked_at(pool: &SqlitePool, server: Id, when: Timestamp) -> Result<()> {
    sqlx::query("UPDATE servers SET updates_checked_at = ? WHERE id = ?")
        .bind(when)
        .bind(server)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn set_dependencies(
    pool: &SqlitePool,
    content: Id,
    dependencies: &[(String, String)],
) -> Result<()> {
    sqlx::query("DELETE FROM content_dependencies WHERE content_id = ?")
        .bind(content)
        .execute(pool)
        .await?;
    for (project_id, kind) in dependencies {
        sqlx::query(
            "INSERT OR IGNORE INTO content_dependencies (content_id, project_id, kind)
             VALUES (?, ?, ?)",
        )
        .bind(content)
        .bind(project_id)
        .bind(kind)
        .execute(pool)
        .await?;
    }
    Ok(())
}

pub async fn dependents_of(
    pool: &SqlitePool,
    server: Id,
    going: &[Id],
) -> Result<Vec<(Id, Vec<Id>)>> {
    if going.is_empty() {
        return Ok(Vec::new());
    }
    let rows: Vec<(Id, String)> = sqlx::query_as(
        "SELECT content_dependencies.content_id, content_dependencies.project_id
           FROM content_dependencies
           JOIN content_items ON content_items.id = content_dependencies.content_id
          WHERE content_items.server_id = ?
            AND content_dependencies.kind IN ('required', 'embedded')",
    )
    .bind(server)
    .fetch_all(pool)
    .await?;

    let leaving: Vec<(Id, Option<String>)> = sqlx::query_as(
        "SELECT id, project_id FROM content_items WHERE server_id = ?",
    )
    .bind(server)
    .fetch_all(pool)
    .await?;

    let projects_going: Vec<(Id, String)> = leaving
        .iter()
        .filter(|(id, _)| going.contains(id))
        .filter_map(|(id, project)| project.clone().map(|project| (*id, project)))
        .collect();

    let mut out: Vec<(Id, Vec<Id>)> = Vec::new();
    for (dependent, needed) in rows {
        if going.contains(&dependent) {
            continue;
        }
        let hit: Vec<Id> = projects_going
            .iter()
            .filter(|(_, project)| *project == needed)
            .map(|(id, _)| *id)
            .collect();
        if hit.is_empty() {
            continue;
        }
        match out.iter_mut().find(|(id, _)| *id == dependent) {
            Some((_, already)) => already.extend(hit),
            None => out.push((dependent, hit)),
        }
    }
    for (_, list) in &mut out {
        list.sort();
        list.dedup();
    }
    Ok(out)
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ModpackRow {
    pub server_id: Id,
    pub source_kind: ModpackSourceKind,
    pub project_id: Option<String>,
    pub version_id: Option<String>,
    pub title: String,
    pub filename: Option<String>,
    pub version_number: Option<String>,
    pub date_published: Option<Timestamp>,
    pub has_update: bool,
    pub update_version_id: Option<String>,
    pub linked_at: Timestamp,
}

pub async fn modpack(pool: &SqlitePool, server: Id) -> Result<Option<ModpackRow>> {
    sqlx::query_as(
        "SELECT server_id, source_kind, project_id, version_id, title, filename, version_number,
                date_published, has_update, update_version_id, linked_at
           FROM server_modpacks WHERE server_id = ?",
    )
    .bind(server)
    .fetch_optional(pool)
    .await
}

pub async fn link(pool: &SqlitePool, row: &ModpackRow) -> Result<()> {
    sqlx::query(
        "INSERT INTO server_modpacks (server_id, source_kind, project_id, version_id, title,
                                      filename, version_number, date_published, has_update,
                                      update_version_id, linked_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT (server_id) DO UPDATE
            SET source_kind = excluded.source_kind, project_id = excluded.project_id,
                version_id = excluded.version_id, title = excluded.title,
                filename = excluded.filename, version_number = excluded.version_number,
                date_published = excluded.date_published, has_update = excluded.has_update,
                update_version_id = excluded.update_version_id",
    )
    .bind(row.server_id)
    .bind(row.source_kind)
    .bind(&row.project_id)
    .bind(&row.version_id)
    .bind(&row.title)
    .bind(&row.filename)
    .bind(&row.version_number)
    .bind(row.date_published)
    .bind(row.has_update)
    .bind(&row.update_version_id)
    .bind(row.linked_at)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn unlink(pool: &SqlitePool, server: Id) -> Result<u64> {
    let adopted = sqlx::query(
        "UPDATE content_items SET source_kind = 'local'
          WHERE server_id = ? AND source_kind = 'modrinth_modpack'",
    )
    .bind(server)
    .execute(pool)
    .await?
    .rows_affected();

    sqlx::query("DELETE FROM server_modpacks WHERE server_id = ?")
        .bind(server)
        .execute(pool)
        .await?;
    Ok(adopted)
}

pub async fn set_modpack_update(
    pool: &SqlitePool,
    server: Id,
    update_version_id: Option<&str>,
) -> Result<()> {
    sqlx::query(
        "UPDATE server_modpacks SET has_update = ?, update_version_id = ? WHERE server_id = ?",
    )
    .bind(update_version_id.is_some())
    .bind(update_version_id)
    .bind(server)
    .execute(pool)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::harness::{a_server, a_user, schema};

    #[test]
    fn the_switch_is_a_suffix_and_the_base_name_survives_it() {
        assert_eq!(base_path("mods/foo.jar.disabled"), "mods/foo.jar");
        assert_eq!(base_path("mods/foo.jar"), "mods/foo.jar");
        assert_eq!(toggled("mods/foo.jar", false), "mods/foo.jar.disabled");
        assert_eq!(toggled("mods/foo.jar.disabled", true), "mods/foo.jar");
        assert_eq!(toggled("mods/foo.jar.disabled", false), "mods/foo.jar.disabled");
        assert_eq!(file_name_of("mods/foo.jar"), "foo.jar");
    }

    #[tokio::test]
    async fn disabling_moves_the_file_and_keeps_the_row() {
        let pool = schema().await;
        let owner = a_user(&pool).await;
        let server = a_server(&pool, owner, "fabric", "1.21.1").await;

        let row = ItemRow::fresh(server, "mods/foo.jar", ContentProjectType::Mod);
        upsert(&pool, &row).await.expect("a row");
        move_to(&pool, row.id, "mods/foo.jar.disabled", false).await.expect("a rename");

        let after = one(&pool, server, row.id).await.expect("a read").expect("the row");
        assert_eq!(after.id, row.id, "8.1: the selection hangs on this id");
        assert_eq!(after.file_path, "mods/foo.jar.disabled");
        assert_eq!(after.file_name, "foo.jar.disabled");
        assert!(!after.enabled);
    }

    #[tokio::test]
    async fn a_dependent_is_named_only_when_what_it_needs_is_the_thing_going() {
        let pool = schema().await;
        let owner = a_user(&pool).await;
        let server = a_server(&pool, owner, "fabric", "1.21.1").await;

        let mut api = ItemRow::fresh(server, "mods/fabric-api.jar", ContentProjectType::Mod);
        api.project_id = Some("P7dR8mSH".to_owned());
        upsert(&pool, &api).await.expect("a row");

        let dependent = ItemRow::fresh(server, "mods/needs-api.jar", ContentProjectType::Mod);
        upsert(&pool, &dependent).await.expect("a row");
        set_dependencies(&pool, dependent.id, &[("P7dR8mSH".to_owned(), "required".to_owned())])
            .await
            .expect("a dependency");

        let bystander = ItemRow::fresh(server, "mods/alone.jar", ContentProjectType::Mod);
        upsert(&pool, &bystander).await.expect("a row");
        set_dependencies(&pool, bystander.id, &[("other".to_owned(), "optional".to_owned())])
            .await
            .expect("a dependency");

        let warned = dependents_of(&pool, server, &[api.id]).await.expect("an answer");
        assert_eq!(warned, vec![(dependent.id, vec![api.id])]);

        let together =
            dependents_of(&pool, server, &[api.id, dependent.id]).await.expect("an answer");
        assert!(together.is_empty());
    }

    #[tokio::test]
    async fn unlinking_leaves_the_files_and_takes_the_pack() {
        let pool = schema().await;
        let owner = a_user(&pool).await;
        let server = a_server(&pool, owner, "fabric", "1.21.1").await;

        link(
            &pool,
            &ModpackRow {
                server_id: server,
                source_kind: ModpackSourceKind::ModrinthModpack,
                project_id: Some("PACK".to_owned()),
                version_id: Some("V1".to_owned()),
                title: "A pack".to_owned(),
                filename: None,
                version_number: Some("1.0".to_owned()),
                date_published: None,
                has_update: false,
                update_version_id: None,
                linked_at: Timestamp::now(),
            },
        )
        .await
        .expect("a link");

        let mut from_pack = ItemRow::fresh(server, "mods/packed.jar", ContentProjectType::Mod);
        from_pack.source_kind = ContentSourceKind::ModrinthModpack;
        upsert(&pool, &from_pack).await.expect("a row");
        let mine = ItemRow::fresh(server, "mods/mine.jar", ContentProjectType::Mod);
        upsert(&pool, &mine).await.expect("a row");

        assert_eq!(of_kind(&pool, server, true).await.expect("a list").len(), 1);
        assert_eq!(unlink(&pool, server).await.expect("an unlink"), 1);
        assert!(modpack(&pool, server).await.expect("a read").is_none());
        assert_eq!(of_kind(&pool, server, false).await.expect("a list").len(), 2);
        assert!(of_kind(&pool, server, true).await.expect("a list").is_empty());
    }
}
