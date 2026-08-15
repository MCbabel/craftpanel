#![allow(dead_code)]

mod event;
mod page;

#[allow(unused_imports)]
pub use event::{AddonRef, Event, ModpackSpec};
#[allow(unused_imports)]
pub use page::{page, AddonSummary, Order, Page, Query, UserSummary, VersionSummary};

use sqlx::SqlitePool;

use crate::auth::access::Access;
use crate::auth::Caller;
use crate::model::{Id, Timestamp};

pub const RETENTION: time::Duration = time::Duration::days(180);

pub async fn record(pool: &SqlitePool, access: Access, caller: &Caller, event: Event) {
    record_by(pool, access.server_id, caller.id(), event).await;
}

pub async fn record_by(pool: &SqlitePool, server: Id, actor: Id, event: Event) {
    let action = event.action();
    let metadata = event.metadata().map(|value| value.to_string());

    let written = sqlx::query(
        "INSERT INTO audit_log (id, server_id, actor_user_id, action, metadata, created_at) \
         VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(Id::new())
    .bind(server)
    .bind(actor)
    .bind(action)
    .bind(metadata)
    .bind(Timestamp::now())
    .execute(pool)
    .await;

    if let Err(err) = written {
        tracing::error!(%server, %actor, %action, "the audit log lost an entry: {err}");
    }
}

pub async fn purge(pool: &SqlitePool) -> sqlx::Result<u64> {
    let cutoff = Timestamp::at(Timestamp::now().as_datetime() - RETENTION);
    let gone = sqlx::query("DELETE FROM audit_log WHERE created_at < ?")
        .bind(cutoff)
        .execute(pool)
        .await?;
    Ok(gone.rows_affected())
}

pub fn spawn_purge(pool: SqlitePool) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(24 * 60 * 60));
        loop {
            tick.tick().await;
            match purge(&pool).await {
                Ok(0) => {}
                Ok(gone) => tracing::info!(gone, "audit entries older than 180 days removed"),
                Err(err) => tracing::error!("the audit log could not be swept: {err}"),
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::harness::{a_server, a_user, test_pool};
    use crate::model::{AuditAction, Permissions, ServerRole};

    #[tokio::test]
    async fn an_entry_comes_back_the_way_the_renderer_reads_it() {
        let pool = test_pool().await;
        let max = a_user(&pool, "max").await;
        let anna = a_user(&pool, "anna").await;
        let server = a_server(&pool, max, "one", 2048).await;

        record_by(
            &pool,
            server,
            max,
            Event::UserInvited {
                user: anna,
                permissions: Permissions::from_role(ServerRole::Editor),
            },
        )
        .await;

        let page = page(&pool, server, &Query::default()).await.unwrap();
        assert_eq!(page.data.len(), 1);
        assert_eq!(page.data[0].action.action, AuditAction::UserInvited);
        assert!(page.data[0].world_id.is_none(), "1.9: one world per server");

        let metadata = page.data[0].action.metadata.as_ref().unwrap();
        assert_eq!(metadata["user_id"], anna.to_string());
        assert_eq!(
            page.users[&max.to_string()].username,
            "max",
            "the actor has to be in the lookup"
        );
        assert_eq!(
            page.users[&anna.to_string()].username,
            "anna",
            "and so has everyone named in metadata"
        );
    }

    #[tokio::test]
    async fn addon_names_come_out_of_the_store_of_eight_sixteen() {
        let pool = test_pool().await;
        let max = a_user(&pool, "max").await;
        let server = a_server(&pool, max, "one", 2048).await;
        let now = Timestamp::now();

        sqlx::query(
            "INSERT INTO modrinth_project (project_id, slug, title, icon_url, fetched_at, \
             expires_at) VALUES ('AABBCCDD', 'sodium', 'Sodium', 'https://x/i.png', ?, ?)",
        )
        .bind(now)
        .bind(now)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO modrinth_version (version_id, project_id, payload, fetched_at, \
             expires_at) VALUES ('11223344', 'AABBCCDD', \
             '{\"name\":\"Sodium 0.6\",\"version_number\":\"0.6.0\"}', ?, ?)",
        )
        .bind(now)
        .bind(now)
        .execute(&pool)
        .await
        .unwrap();

        record_by(
            &pool,
            server,
            max,
            Event::AddonAdded { addons: vec![AddonRef::new("AABBCCDD", "11223344")] },
        )
        .await;

        let page = page(&pool, server, &Query::default()).await.unwrap();
        assert_eq!(page.addons["AABBCCDD"].title, "Sodium");
        assert_eq!(page.addons["AABBCCDD"].slug.as_deref(), Some("sodium"));
        assert_eq!(page.versions["11223344"].version_number.as_deref(), Some("0.6.0"));
        assert!(!page.addons.contains_key("11223344"), "a version is no project");
    }

    #[tokio::test]
    async fn one_entry_may_name_more_addons_than_a_statement_can_bind() {
        let pool = test_pool().await;
        let max = a_user(&pool, "max").await;
        let server = a_server(&pool, max, "one", 2048).await;
        let now = Timestamp::now();

        sqlx::query(
            "INSERT INTO modrinth_project (project_id, slug, title, icon_url, fetched_at, \
             expires_at) VALUES ('p39999', 'sodium', 'Sodium', NULL, ?, ?)",
        )
        .bind(now)
        .bind(now)
        .execute(&pool)
        .await
        .unwrap();

        let addons = (0..40_000).map(|n| AddonRef::new(format!("p{n}"), format!("v{n}"))).collect();
        record_by(&pool, server, max, Event::AddonAdded { addons }).await;

        let page = page(&pool, server, &Query::default()).await.expect("a page, not a 500");
        assert_eq!(page.data.len(), 1);
        assert_eq!(page.addons["p39999"].title, "Sodium", "the last bite is looked up too");
    }

    #[tokio::test]
    async fn a_deleted_server_takes_its_log_with_it() {
        let pool = test_pool().await;
        let max = a_user(&pool, "max").await;
        let server = a_server(&pool, max, "one", 2048).await;
        record_by(&pool, server, max, Event::ServerStarted).await;

        sqlx::query("DELETE FROM servers WHERE id = ?").bind(server).execute(&pool).await.unwrap();

        let left: i64 = sqlx::query_scalar("SELECT count(*) FROM audit_log")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(left, 0);
    }

    #[tokio::test]
    async fn a_user_who_is_deleted_does_not_rewrite_who_did_what() {
        let pool = test_pool().await;
        let max = a_user(&pool, "max").await;
        let anna = a_user(&pool, "anna").await;
        let server = a_server(&pool, max, "one", 2048).await;
        record_by(&pool, server, anna, Event::ServerStopped).await;

        sqlx::query("DELETE FROM users WHERE id = ?").bind(anna).execute(&pool).await.unwrap();

        let page = page(&pool, server, &Query::default()).await.unwrap();
        assert_eq!(page.data.len(), 1, "the entry stays");
        assert!(page.users.is_empty(), "the name is gone, the renderer shows the id");
    }

    #[tokio::test]
    async fn entries_older_than_a_hundred_and_eighty_days_are_swept() {
        let pool = test_pool().await;
        let max = a_user(&pool, "max").await;
        let server = a_server(&pool, max, "one", 2048).await;

        record_by(&pool, server, max, Event::ServerStarted).await;
        record_by(&pool, server, max, Event::ServerStopped).await;

        let long_ago = Timestamp::at(Timestamp::now().as_datetime() - time::Duration::days(181));
        sqlx::query("UPDATE audit_log SET created_at = ? WHERE action = 'server_started'")
            .bind(long_ago)
            .execute(&pool)
            .await
            .unwrap();

        assert_eq!(purge(&pool).await.unwrap(), 1);
        let page = page(&pool, server, &Query::default()).await.unwrap();
        assert_eq!(page.data.len(), 1);
        assert_eq!(page.data[0].action.action, AuditAction::ServerStopped);
    }
}
