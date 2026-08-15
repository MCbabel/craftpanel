use axum::http::HeaderMap;
use axum_extra::extract::CookieJar;
use sha2::{Digest, Sha256};
use sqlx::SqlitePool;

use crate::model::{Id, PanelRole, Permission, Permissions, ServerRole, Timestamp};

use super::fault::{Answer, Fault};

pub const SESSION_COOKIE: &str = "craft_session";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Caller {
    pub user_id: Id,
    pub session_id: Id,
    pub panel_role: PanelRole,
}

impl Caller {
    pub fn is_panel_admin(&self) -> bool {
        self.panel_role == PanelRole::Admin
    }
}

pub fn token_hash(token: &str) -> String {
    hex::encode(Sha256::digest(token.as_bytes()))
}

pub async fn caller(pool: &SqlitePool, headers: &HeaderMap) -> Answer<Caller> {
    let jar = CookieJar::from_headers(headers);
    let cookie = jar.get(SESSION_COOKIE).ok_or_else(Fault::unauthenticated)?;

    let row: Option<(Id, Id, PanelRole, Timestamp)> = sqlx::query_as(
        "SELECT sessions.id, users.id, users.role, sessions.expires_at
           FROM sessions JOIN users ON users.id = sessions.user_id
          WHERE sessions.token_hash = ?",
    )
    .bind(token_hash(cookie.value()))
    .fetch_optional(pool)
    .await?;

    let (session_id, user_id, panel_role, expires_at) = row.ok_or_else(Fault::unauthenticated)?;
    if expires_at <= Timestamp::now() {
        return Err(Fault::unauthenticated());
    }
    Ok(Caller { user_id, session_id, panel_role })
}

pub async fn session_alive(pool: &SqlitePool, session: Id) -> bool {
    let row: sqlx::Result<Option<(Timestamp,)>> =
        sqlx::query_as("SELECT expires_at FROM sessions WHERE id = ?")
            .bind(session)
            .fetch_optional(pool)
            .await;
    match row {
        Ok(found) => found.is_some_and(|(expires_at,)| expires_at > Timestamp::now()),
        Err(err) => {
            tracing::error!("a session could not be looked at again: {err}");
            true
        }
    }
}

pub async fn permissions(pool: &SqlitePool, server: Id, caller: &Caller) -> Answer<Permissions> {
    let owner: Option<(Id,)> = sqlx::query_as("SELECT owner_id FROM servers WHERE id = ?")
        .bind(server)
        .fetch_optional(pool)
        .await?;
    let Some((owner,)) = owner else {
        return Err(Fault::server_not_found());
    };

    if caller.is_panel_admin() || owner == caller.user_id {
        return Ok(Permissions::of(Permission::ServerAdmin));
    }

    let role: Option<(ServerRole,)> = sqlx::query_as(
        "SELECT role FROM server_members
          WHERE server_id = ? AND user_id = ? AND joined_at IS NOT NULL",
    )
    .bind(server)
    .bind(caller.user_id)
    .fetch_optional(pool)
    .await?;

    role.map(|(role,)| Permissions::from_role(role)).ok_or_else(Fault::server_not_found)
}

pub fn require(mask: Permissions, permission: Permission) -> Answer<()> {
    if mask.allows(permission) {
        Ok(())
    } else {
        Err(Fault::forbidden())
    }
}

pub async fn visible_servers(pool: &SqlitePool, caller: &Caller) -> Answer<Vec<Id>> {
    let rows: Vec<(Id,)> = if caller.is_panel_admin() {
        sqlx::query_as("SELECT id FROM servers ORDER BY id").fetch_all(pool).await?
    } else {
        sqlx::query_as(
            "SELECT id FROM servers WHERE owner_id = ?
              UNION
             SELECT server_id FROM server_members
              WHERE user_id = ? AND joined_at IS NOT NULL
              ORDER BY 1",
        )
        .bind(caller.user_id)
        .bind(caller.user_id)
        .fetch_all(pool)
        .await?
    };
    Ok(rows.into_iter().map(|(id,)| id).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ops::testing::{a_server, a_session, a_user, schema};
    use axum::http::header::COOKIE;

    fn headers(cookie: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(COOKIE, format!("craft_session={cookie}").parse().expect("a header value"));
        headers
    }

    #[tokio::test]
    async fn a_session_is_found_by_the_hash_of_its_cookie_and_never_by_the_cookie() {
        let pool = schema().await;
        let user = a_user(&pool, PanelRole::User).await;
        let token = a_session(&pool, user).await;

        let found = caller(&pool, &headers(&token)).await.expect("the session is known");
        assert_eq!(found.user_id, user);

        let stored: (String,) = sqlx::query_as("SELECT token_hash FROM sessions")
            .fetch_one(&pool)
            .await
            .expect("the row");
        assert_ne!(stored.0, token);
        assert_eq!(stored.0, token_hash(&token));
    }

    #[tokio::test]
    async fn an_expired_or_unknown_cookie_is_no_session() {
        let pool = schema().await;
        let user = a_user(&pool, PanelRole::User).await;
        let token = a_session(&pool, user).await;

        assert_eq!(
            caller(&pool, &headers("something else")).await.unwrap_err().code(),
            "unauthenticated"
        );
        assert_eq!(caller(&pool, &HeaderMap::new()).await.unwrap_err().code(), "unauthenticated");

        sqlx::query("UPDATE sessions SET expires_at = '2020-01-01T00:00:00Z'")
            .execute(&pool)
            .await
            .expect("expiring the session");
        assert_eq!(caller(&pool, &headers(&token)).await.unwrap_err().code(), "unauthenticated");
    }

    #[tokio::test]
    async fn the_owner_an_admin_a_member_and_a_stranger_get_four_different_answers() {
        let pool = schema().await;
        let owner = a_user(&pool, PanelRole::User).await;
        let admin = a_user(&pool, PanelRole::Admin).await;
        let editor = a_user(&pool, PanelRole::User).await;
        let viewer = a_user(&pool, PanelRole::User).await;
        let invited = a_user(&pool, PanelRole::User).await;
        let stranger = a_user(&pool, PanelRole::User).await;
        let server = a_server(&pool, owner).await;

        for (user, role, joined) in [
            (editor, ServerRole::Editor, true),
            (viewer, ServerRole::Viewer, true),
            (invited, ServerRole::Editor, false),
        ] {
            sqlx::query(
                "INSERT INTO server_members (id, server_id, user_id, role, invited_at, joined_at)
                 VALUES (?, ?, ?, ?, ?, ?)",
            )
            .bind(Id::new())
            .bind(server)
            .bind(user)
            .bind(role)
            .bind(Timestamp::now())
            .bind(joined.then(Timestamp::now))
            .execute(&pool)
            .await
            .expect("a membership");
        }

        let mask = |user: Id, panel_role: PanelRole| {
            let pool = pool.clone();
            async move {
                let caller = Caller { user_id: user, session_id: Id::new(), panel_role };
                permissions(&pool, server, &caller).await
            }
        };

        assert_eq!(
            mask(owner, PanelRole::User).await.expect("the owner reads his own server").role(),
            ServerRole::Owner
        );
        assert_eq!(
            mask(admin, PanelRole::Admin).await.expect("a panel admin reads every server").role(),
            ServerRole::Owner
        );
        assert_eq!(mask(editor, PanelRole::User).await.expect("a member").role(), ServerRole::Editor);

        let viewer_mask = mask(viewer, PanelRole::User).await.expect("a member");
        assert_eq!(viewer_mask.role(), ServerRole::Viewer);
        assert!(viewer_mask.allows(Permission::BaseRead));
        assert!(!viewer_mask.allows(Permission::Backups));

        assert_eq!(mask(invited, PanelRole::User).await.unwrap_err().code(), "server_not_found");
        assert_eq!(mask(stranger, PanelRole::User).await.unwrap_err().code(), "server_not_found");
    }

    #[tokio::test]
    async fn the_visible_list_holds_owned_and_shared_servers_and_an_admin_sees_all() {
        let pool = schema().await;
        let owner = a_user(&pool, PanelRole::User).await;
        let other = a_user(&pool, PanelRole::User).await;
        let admin = a_user(&pool, PanelRole::Admin).await;
        let mine = a_server(&pool, owner).await;
        let theirs = a_server(&pool, other).await;

        sqlx::query(
            "INSERT INTO server_members (id, server_id, user_id, role, invited_at, joined_at)
             VALUES (?, ?, ?, 'viewer', ?, ?)",
        )
        .bind(Id::new())
        .bind(theirs)
        .bind(owner)
        .bind(Timestamp::now())
        .bind(Timestamp::now())
        .execute(&pool)
        .await
        .expect("a membership");

        let seen = |user, panel_role| {
            let pool = pool.clone();
            async move {
                let caller = Caller { user_id: user, session_id: Id::new(), panel_role };
                visible_servers(&pool, &caller).await.expect("a list")
            }
        };

        let mut ours = seen(owner, PanelRole::User).await;
        ours.sort();
        let mut both = vec![mine, theirs];
        both.sort();
        assert_eq!(ours, both);

        assert_eq!(seen(other, PanelRole::User).await, vec![theirs]);
        assert_eq!(seen(admin, PanelRole::Admin).await.len(), 2);
    }
}
