use sqlx::SqlitePool;

use super::error::{Failure, Result};
use super::extract::Caller;
use crate::model::{Id, Permission, Permissions, ServerRole};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Access {
    pub server_id: Id,
    pub owner_id: Id,
    pub permissions: Permissions,
}

impl Access {
    pub fn role(self) -> ServerRole {
        self.permissions.role()
    }

    pub fn is_owner(self, caller: &Caller) -> bool {
        self.owner_id == caller.id()
    }

    pub fn allows(self, permission: Permission) -> bool {
        self.permissions.allows(permission)
    }

    pub fn require(self, permission: Permission) -> Result<Self> {
        if !self.allows(Permission::BaseRead) {
            return Err(unknown_server());
        }
        if !self.allows(permission) {
            return Err(Failure::forbidden());
        }
        Ok(self)
    }

    pub fn require_ownership(self, caller: &Caller) -> Result<Self> {
        if !self.allows(Permission::BaseRead) {
            return Err(unknown_server());
        }
        if self.is_owner(caller) || caller.is_admin() {
            return Ok(self);
        }
        Err(Failure::forbidden())
    }
}

pub async fn of(pool: &SqlitePool, caller: &Caller, server_id: Id) -> Result<Access> {
    let owner_id: Option<Id> = sqlx::query_scalar("SELECT owner_id FROM servers WHERE id = ?")
        .bind(server_id)
        .fetch_optional(pool)
        .await?;
    let owner_id = owner_id.ok_or_else(unknown_server)?;

    if caller.is_admin() || owner_id == caller.id() {
        return Ok(Access {
            server_id,
            owner_id,
            permissions: Permissions::of(Permission::ServerAdmin),
        });
    }

    let role: Option<ServerRole> = sqlx::query_scalar(
        "SELECT role FROM server_members \
         WHERE server_id = ? AND user_id = ? AND joined_at IS NOT NULL",
    )
    .bind(server_id)
    .bind(caller.id())
    .fetch_optional(pool)
    .await?;

    Ok(Access {
        server_id,
        owner_id,
        permissions: role.map_or(Permissions::NONE, Permissions::from_role),
    })
}

pub async fn require(
    pool: &SqlitePool,
    caller: &Caller,
    server_id: Id,
    permission: Permission,
) -> Result<Access> {
    of(pool, caller, server_id).await?.require(permission)
}

pub async fn visible_servers(pool: &SqlitePool, caller: &Caller) -> Result<Vec<Id>> {
    let sql = if caller.is_admin() {
        "SELECT id FROM servers ORDER BY id"
    } else {
        "SELECT s.id FROM servers s \
         LEFT JOIN server_members m \
           ON m.server_id = s.id AND m.user_id = ? AND m.joined_at IS NOT NULL \
         WHERE s.owner_id = ? OR m.id IS NOT NULL \
         ORDER BY s.id"
    };

    let mut query = sqlx::query_scalar::<_, Id>(sql);
    if !caller.is_admin() {
        query = query.bind(caller.id()).bind(caller.id());
    }
    Ok(query.fetch_all(pool).await?)
}

fn unknown_server() -> Failure {
    Failure::not_found("server_not_found", "no such server")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::harness::{a_server, a_user, an_admin, test_pool};
    use crate::auth::session;
    use crate::auth::users;
    use crate::model::Timestamp;

    async fn caller(pool: &SqlitePool, id: Id) -> Caller {
        let user = users::load(pool, id).await.unwrap();
        let (session, _) = session::open(pool, id, None, Timestamp::now()).await.unwrap();
        Caller { user, session, secure: false }
    }

    async fn make_member(pool: &SqlitePool, server: Id, user: Id, role: ServerRole, joined: bool) {
        let now = Timestamp::now();
        sqlx::query(
            "INSERT INTO server_members (id, server_id, user_id, role, invited_at, joined_at) \
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(Id::new())
        .bind(server)
        .bind(user)
        .bind(role)
        .bind(now)
        .bind(joined.then_some(now))
        .execute(pool)
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn the_owner_holds_every_bit() {
        let pool = test_pool().await;
        let max = a_user(&pool, "max").await;
        let server = a_server(&pool, max, "one", 2048).await;

        let access = of(&pool, &caller(&pool, max).await, server).await.unwrap();
        assert_eq!(access.role(), ServerRole::Owner);
        for permission in Permission::ALL {
            assert!(access.allows(*permission), "the owner lacks {permission}");
        }
    }

    #[tokio::test]
    async fn a_panel_admin_holds_every_bit_without_a_membership_row() {
        let pool = test_pool().await;
        let max = a_user(&pool, "max").await;
        let anna = an_admin(&pool, "anna").await;
        let server = a_server(&pool, max, "one", 2048).await;

        let access = of(&pool, &caller(&pool, anna).await, server).await.unwrap();
        assert_eq!(access.role(), ServerRole::Owner);
        assert!(access.require(Permission::ResetServer).is_ok());

        let members: i64 = sqlx::query_scalar("SELECT count(*) FROM server_members")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(members, 0, "1.10: he does not appear in the member list");
    }

    #[tokio::test]
    async fn an_editor_may_write_files_but_not_manage_users() {
        let pool = test_pool().await;
        let max = a_user(&pool, "max").await;
        let anna = a_user(&pool, "anna").await;
        let server = a_server(&pool, max, "one", 2048).await;
        make_member(&pool, server, anna, ServerRole::Editor, true).await;

        let access = of(&pool, &caller(&pool, anna).await, server).await.unwrap();
        assert_eq!(access.role(), ServerRole::Editor);

        for allowed in [
            Permission::BaseRead,
            Permission::PowerActions,
            Permission::ExecCommands,
            Permission::FilesWrite,
            Permission::Setup,
            Permission::Backups,
            Permission::Advanced,
        ] {
            assert!(access.require(allowed).is_ok(), "an editor should hold {allowed}");
        }
        for refused in
            [Permission::ManageUsers, Permission::ResetServer, Permission::ServerAdmin]
        {
            assert_eq!(access.require(refused).unwrap_err().code(), "forbidden", "{refused}");
        }
    }

    #[tokio::test]
    async fn a_viewer_may_restart_but_not_delete_files() {
        let pool = test_pool().await;
        let max = a_user(&pool, "max").await;
        let anna = a_user(&pool, "anna").await;
        let server = a_server(&pool, max, "one", 2048).await;
        make_member(&pool, server, anna, ServerRole::Viewer, true).await;

        let access = of(&pool, &caller(&pool, anna).await, server).await.unwrap();
        assert_eq!(access.role(), ServerRole::Viewer);
        assert!(access.require(Permission::PowerActions).is_ok(), "P6: a viewer may restart");
        assert_eq!(access.require(Permission::FilesWrite).unwrap_err().code(), "forbidden");
        assert_eq!(access.require(Permission::Backups).unwrap_err().code(), "forbidden");
    }

    #[tokio::test]
    async fn a_stranger_is_told_the_server_does_not_exist() {
        let pool = test_pool().await;
        let max = a_user(&pool, "max").await;
        let anna = a_user(&pool, "anna").await;
        let server = a_server(&pool, max, "one", 2048).await;

        let access = of(&pool, &caller(&pool, anna).await, server).await.unwrap();
        let refusal = access.require(Permission::BaseRead).unwrap_err();
        assert_eq!(refusal.code(), "server_not_found", "1.7: 403 here would leak the id");
        assert_eq!(refusal.status(), axum::http::StatusCode::NOT_FOUND);

        let missing = of(&pool, &caller(&pool, anna).await, Id::new()).await.unwrap_err();
        assert_eq!(missing.code(), "server_not_found");
    }

    #[tokio::test]
    async fn an_invitation_grants_nothing_until_it_is_accepted() {
        let pool = test_pool().await;
        let max = a_user(&pool, "max").await;
        let anna = a_user(&pool, "anna").await;
        let server = a_server(&pool, max, "one", 2048).await;
        make_member(&pool, server, anna, ServerRole::Editor, false).await;

        let access = of(&pool, &caller(&pool, anna).await, server).await.unwrap();
        assert_eq!(access.permissions, Permissions::NONE);
        assert_eq!(access.require(Permission::BaseRead).unwrap_err().code(), "server_not_found");
    }

    #[tokio::test]
    async fn ownership_is_the_owner_and_the_panel_admin_and_no_one_else() {
        let pool = test_pool().await;
        let max = a_user(&pool, "max").await;
        let anna = a_user(&pool, "anna").await;
        let bea = an_admin(&pool, "bea").await;
        let server = a_server(&pool, max, "one", 2048).await;
        make_member(&pool, server, anna, ServerRole::Editor, true).await;

        let owner = caller(&pool, max).await;
        let editor = caller(&pool, anna).await;
        let admin = caller(&pool, bea).await;

        assert!(of(&pool, &owner, server).await.unwrap().require_ownership(&owner).is_ok());
        assert!(of(&pool, &admin, server).await.unwrap().require_ownership(&admin).is_ok());
        assert_eq!(
            of(&pool, &editor, server).await.unwrap().require_ownership(&editor).unwrap_err().code(),
            "forbidden"
        );
    }

    #[tokio::test]
    async fn a_list_shows_what_the_caller_may_read() {
        let pool = test_pool().await;
        let max = a_user(&pool, "max").await;
        let anna = a_user(&pool, "anna").await;
        let bea = an_admin(&pool, "bea").await;

        let his = a_server(&pool, max, "his", 2048).await;
        let _hers = a_server(&pool, anna, "hers", 2048).await;
        let shared = a_server(&pool, anna, "shared", 2048).await;
        let invited = a_server(&pool, anna, "invited", 2048).await;
        make_member(&pool, shared, max, ServerRole::Viewer, true).await;
        make_member(&pool, invited, max, ServerRole::Editor, false).await;

        let mut visible = visible_servers(&pool, &caller(&pool, max).await).await.unwrap();
        visible.sort();
        let mut expected = vec![his, shared];
        expected.sort();
        assert_eq!(visible, expected, "his own and the one he joined, not the invitation");

        assert_eq!(visible_servers(&pool, &caller(&pool, bea).await).await.unwrap().len(), 4);
        assert_eq!(visible_servers(&pool, &caller(&pool, anna).await).await.unwrap().len(), 3);
    }
}
