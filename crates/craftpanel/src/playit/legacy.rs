use std::path::Path;

use sqlx::SqlitePool;

use crate::model::{Id, Timestamp};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Adopted {
    pub admin: Id,
    pub tunnels: u32,
}

pub async fn adopt(pool: &SqlitePool, dir: &Path) -> anyhow::Result<Option<Adopted>> {
    let old = dir.join("secret");
    if !tokio::fs::metadata(&old).await.is_ok_and(|meta| meta.is_file()) {
        return Ok(None);
    }

    let Some((admin, measured)) = heir(pool).await? else {
        tracing::warn!(
            "there is a panel-wide playit.gg key but no administrator to hand it to; \
             it stays where it is"
        );
        return Ok(None);
    };

    let mine = dir.join(admin.to_string());
    let moved = mine.join("secret");
    if tokio::fs::metadata(&moved).await.is_ok() {
        tracing::warn!(
            user = %admin,
            "the user the panel-wide playit.gg key would go to already has one of his own, \
             so it is left where it is"
        );
        return Ok(None);
    }

    sqlx::query(
        "INSERT INTO playit_accounts \
             (user_id, agent_id, account_status, is_self_managed, has_premium, claim_code, \
              claim_state, claim_started_at, checked_at, last_error, updated_at) \
         SELECT ?, agent_id, account_status, is_self_managed, has_premium, claim_code, \
                claim_state, claim_started_at, checked_at, last_error, ? \
           FROM playit_account WHERE id = 1 \
         ON CONFLICT(user_id) DO UPDATE \
            SET agent_id = excluded.agent_id, account_status = excluded.account_status, \
                is_self_managed = excluded.is_self_managed, \
                has_premium = excluded.has_premium, claim_code = excluded.claim_code, \
                claim_state = excluded.claim_state, \
                claim_started_at = excluded.claim_started_at, \
                checked_at = excluded.checked_at, last_error = excluded.last_error, \
                updated_at = excluded.updated_at",
    )
    .bind(admin)
    .bind(Timestamp::now())
    .execute(pool)
    .await?;

    let tunnels = sqlx::query("UPDATE playit_tunnels SET user_id = ?")
        .bind(admin)
        .execute(pool)
        .await?
        .rows_affected() as u32;
    sqlx::query("UPDATE playit_released SET user_id = ?").bind(admin).execute(pool).await?;

    tokio::fs::create_dir_all(&mine).await?;
    set_mode(&mine, 0o700).await?;
    tokio::fs::rename(&old, &moved).await?;

    let _ = tokio::fs::remove_file(dir.join("playitd.sock")).await;

    let rule = if measured { "every tunnel of it is his" } else { "oldest administrator" };
    tracing::info!(
        user = %admin,
        tunnels,
        rule,
        "the panel-wide playit.gg account now belongs to one user"
    );
    Ok(Some(Adopted { admin, tunnels }))
}

async fn heir(pool: &SqlitePool) -> sqlx::Result<Option<(Id, bool)>> {
    let owner: Option<Id> = sqlx::query_scalar(
        "SELECT min(s.owner_id) FROM playit_tunnels t JOIN servers s ON s.id = t.server_id \
          HAVING count(DISTINCT s.owner_id) = 1",
    )
    .fetch_optional(pool)
    .await?;

    if let Some(owner) = owner {
        let admin: Option<i64> =
            sqlx::query_scalar("SELECT 1 FROM users WHERE id = ? AND role = 'admin'")
                .bind(owner)
                .fetch_optional(pool)
                .await?;
        if admin.is_some() {
            return Ok(Some((owner, true)));
        }
    }

    let oldest: Option<Id> = sqlx::query_scalar(
        "SELECT id FROM users WHERE role = 'admin' ORDER BY created_at, id LIMIT 1",
    )
    .fetch_optional(pool)
    .await?;

    Ok(oldest.map(|admin| (admin, false)))
}

async fn set_mode(path: &Path, mode: u32) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    tokio::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::harness::{a_server, a_user, an_admin, test_pool};
    use crate::playit::store;
    use std::os::unix::fs::PermissionsExt;

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("craftpanel-playit-{name}-{}", Id::new()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    async fn a_panel_wide_key(dir: &Path) {
        std::fs::write(dir.join("secret"), "deadbeefcafe\n").unwrap();
        std::fs::set_permissions(dir.join("secret"), std::fs::Permissions::from_mode(0o600))
            .unwrap();
        std::fs::write(dir.join("playitd.sock"), "").unwrap();
    }

    async fn the_old_row(pool: &SqlitePool) {
        sqlx::query(
            "UPDATE playit_account \
                SET agent_id = '11112222', account_status = 'verified', is_self_managed = 1, \
                    has_premium = 0, updated_at = ? \
              WHERE id = 1",
        )
        .bind(Timestamp::now())
        .execute(pool)
        .await
        .unwrap();
    }

    async fn made_first(pool: &SqlitePool, user: Id) {
        sqlx::query("UPDATE users SET created_at = '2020-01-01T00:00:00Z' WHERE id = ?")
            .bind(user)
            .execute(pool)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn the_oldest_administrator_gets_the_key_and_the_strangers_tunnel_stays_up() {
        let pool = test_pool().await;
        let dir = scratch("legacy");
        a_panel_wide_key(&dir).await;
        the_old_row(&pool).await;

        let first = an_admin(&pool, "root").await;
        let second = an_admin(&pool, "second").await;
        let stranger = a_user(&pool, "anna").await;
        sqlx::query("UPDATE users SET created_at = '2026-01-01T00:00:00Z' WHERE role = 'admin'")
            .execute(&pool)
            .await
            .unwrap();
        let older = if first < second { first } else { second };

        let hers = a_server(&pool, stranger, "survival", 1024).await;
        store::claim_slot(&pool, stranger, hers, 25565, 4).await.unwrap();
        store::attach(&pool, hers, "mauritania").await.unwrap();

        let done = adopt(&pool, &dir).await.unwrap().expect("there was something to adopt");
        assert_eq!(done, Adopted { admin: older, tunnels: 1 });

        let key = dir.join(older.to_string()).join("secret");
        assert_eq!(std::fs::read_to_string(&key).unwrap().trim(), "deadbeefcafe");
        assert_eq!(std::fs::metadata(&key).unwrap().permissions().mode() & 0o777, 0o600);
        assert_eq!(
            std::fs::metadata(dir.join(older.to_string())).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert!(!dir.join("secret").exists(), "the panel-wide key is still lying there");
        assert!(!dir.join("playitd.sock").exists());

        let row = store::account(&pool, older).await.unwrap().expect("his row");
        assert_eq!(row.agent_id.as_deref(), Some("11112222"));
        assert!(row.is_self_managed);
        assert!(store::account(&pool, stranger).await.unwrap().is_none());

        let tunnel = store::tunnel(&pool, hers).await.unwrap().expect("still there");
        assert_eq!(tunnel.user_id, older);
        assert_eq!(tunnel.tunnel_id.as_deref(), Some("mauritania"));
        assert!(store::released(&pool, older).await.unwrap().is_empty());
        assert_eq!(store::used(&pool, older).await.unwrap(), 1);
        assert_eq!(store::for_others(&pool, older).await.unwrap(), 1);

        let left: i64 = sqlx::query_scalar("SELECT count(*) FROM playit_account WHERE id = 1")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(left, 1);

        assert!(adopt(&pool, &dir).await.unwrap().is_none());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn the_administrator_whose_servers_carry_every_tunnel_gets_the_key() {
        let pool = test_pool().await;
        let dir = scratch("measured");
        a_panel_wide_key(&dir).await;
        the_old_row(&pool).await;

        let root = an_admin(&pool, "root").await;
        let anna = an_admin(&pool, "anna").await;
        made_first(&pool, root).await;

        let survival = a_server(&pool, anna, "survival", 1024).await;
        let creative = a_server(&pool, anna, "creative", 1024).await;
        store::claim_slot(&pool, anna, survival, 25565, 4).await.unwrap();
        store::attach(&pool, survival, "mauritania").await.unwrap();
        store::claim_slot(&pool, anna, creative, 25566, 4).await.unwrap();

        let done = adopt(&pool, &dir).await.unwrap().expect("there was something to adopt");
        assert_eq!(done, Adopted { admin: anna, tunnels: 2 });

        let key = dir.join(anna.to_string()).join("secret");
        assert_eq!(std::fs::read_to_string(&key).unwrap().trim(), "deadbeefcafe");
        assert!(
            !dir.join(root.to_string()).exists(),
            "the oldest administrator was handed a key he never used"
        );

        let row = store::account(&pool, anna).await.unwrap().expect("her row");
        assert_eq!(row.agent_id.as_deref(), Some("11112222"));
        assert!(row.is_self_managed);
        assert!(store::account(&pool, root).await.unwrap().is_none());

        assert_eq!(store::used(&pool, anna).await.unwrap(), 2);
        assert_eq!(store::for_others(&pool, anna).await.unwrap(), 0, "these are her own servers");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn tunnels_on_the_servers_of_two_users_measure_nothing() {
        let pool = test_pool().await;
        let dir = scratch("two-owners");
        a_panel_wide_key(&dir).await;
        the_old_row(&pool).await;

        let root = an_admin(&pool, "root").await;
        let anna = an_admin(&pool, "anna").await;
        let ben = a_user(&pool, "ben").await;
        made_first(&pool, root).await;

        let hers = a_server(&pool, anna, "survival", 1024).await;
        let his = a_server(&pool, ben, "creative", 1024).await;
        store::claim_slot(&pool, anna, hers, 25565, 4).await.unwrap();
        store::claim_slot(&pool, ben, his, 25566, 4).await.unwrap();

        let done = adopt(&pool, &dir).await.unwrap().expect("there was something to adopt");
        assert_eq!(done, Adopted { admin: root, tunnels: 2 });

        assert!(dir.join(root.to_string()).join("secret").exists());
        assert!(
            !dir.join(anna.to_string()).exists(),
            "one of two owners was read as a measurement"
        );
        assert!(store::account(&pool, anna).await.unwrap().is_none());
        assert_eq!(store::used(&pool, root).await.unwrap(), 2);
        assert_eq!(store::for_others(&pool, root).await.unwrap(), 2);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn a_key_without_a_single_tunnel_leaves_nothing_to_measure() {
        let pool = test_pool().await;
        let dir = scratch("notunnel");
        a_panel_wide_key(&dir).await;
        the_old_row(&pool).await;

        let root = an_admin(&pool, "root").await;
        let anna = an_admin(&pool, "anna").await;
        made_first(&pool, root).await;

        let done = adopt(&pool, &dir).await.unwrap().expect("there was something to adopt");
        assert_eq!(done, Adopted { admin: root, tunnels: 0 });

        assert!(dir.join(root.to_string()).join("secret").exists());
        assert!(!dir.join(anna.to_string()).exists());
        assert!(store::account(&pool, root).await.unwrap().is_some());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn the_debt_of_a_deleted_server_measures_nothing_and_goes_with_the_key() {
        let pool = test_pool().await;
        let dir = scratch("deleted");
        a_panel_wide_key(&dir).await;
        the_old_row(&pool).await;

        let root = an_admin(&pool, "root").await;
        let anna = an_admin(&pool, "anna").await;
        let ben = a_user(&pool, "ben").await;
        made_first(&pool, root).await;

        let hers = a_server(&pool, anna, "survival", 1024).await;
        store::claim_slot(&pool, anna, hers, 25565, 4).await.unwrap();
        store::attach(&pool, hers, "mauritania").await.unwrap();

        let gone = a_server(&pool, ben, "creative", 1024).await;
        store::claim_slot(&pool, ben, gone, 25566, 4).await.unwrap();
        store::attach(&pool, gone, "bens-tunnel").await.unwrap();
        sqlx::query("DELETE FROM servers WHERE id = ?").bind(gone).execute(&pool).await.unwrap();

        let done = adopt(&pool, &dir).await.unwrap().expect("there was something to adopt");
        assert_eq!(done, Adopted { admin: anna, tunnels: 1 });

        assert!(dir.join(anna.to_string()).join("secret").exists());
        assert_eq!(store::released(&pool, anna).await.unwrap(), vec!["bens-tunnel".to_owned()]);
        assert!(
            store::released(&pool, ben).await.unwrap().is_empty(),
            "a debt stayed on an account whose key cannot pay it"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn a_second_run_measures_the_servers_and_not_what_the_first_run_wrote() {
        let pool = test_pool().await;
        let dir = scratch("second-run");
        a_panel_wide_key(&dir).await;
        the_old_row(&pool).await;

        let root = an_admin(&pool, "root").await;
        let anna = an_admin(&pool, "anna").await;
        made_first(&pool, root).await;

        let hers = a_server(&pool, anna, "survival", 1024).await;
        store::claim_slot(&pool, anna, hers, 25565, 4).await.unwrap();
        sqlx::query("UPDATE playit_tunnels SET user_id = ?")
            .bind(root)
            .execute(&pool)
            .await
            .unwrap();

        let done = adopt(&pool, &dir).await.unwrap().expect("there was something to adopt");
        assert_eq!(done, Adopted { admin: anna, tunnels: 1 });
        assert_eq!(store::tunnel(&pool, hers).await.unwrap().expect("her row").user_id, anna);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn a_panel_that_never_connected_playit_is_left_alone_and_says_nothing() {
        let pool = test_pool().await;
        let dir = scratch("nolegacy");
        let root = an_admin(&pool, "root").await;

        assert!(adopt(&pool, &dir).await.unwrap().is_none());
        assert!(store::account(&pool, root).await.unwrap().is_none());
        assert!(!dir.join(root.to_string()).exists());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn a_key_with_nobody_to_hand_it_to_stays_where_it_is() {
        let pool = test_pool().await;
        let dir = scratch("noadmin");
        a_panel_wide_key(&dir).await;
        a_user(&pool, "anna").await;

        assert!(adopt(&pool, &dir).await.unwrap().is_none());
        assert!(dir.join("secret").exists());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn an_administrator_who_already_connected_his_own_account_keeps_it() {
        let pool = test_pool().await;
        let dir = scratch("mine");
        a_panel_wide_key(&dir).await;
        let root = an_admin(&pool, "root").await;

        std::fs::create_dir_all(dir.join(root.to_string())).unwrap();
        std::fs::write(dir.join(root.to_string()).join("secret"), "aaaaaaaa\n").unwrap();

        assert!(adopt(&pool, &dir).await.unwrap().is_none());
        assert_eq!(
            std::fs::read_to_string(dir.join(root.to_string()).join("secret")).unwrap().trim(),
            "aaaaaaaa"
        );
        assert!(dir.join("secret").exists(), "the old key was thrown away");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
