use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

use crate::auth::error::{Failure, Result};
use crate::model::{Allocation, Id, PortRange, Timestamp};

pub const PER_SERVER: usize = 8;
const MIN_PORT: u16 = 1024;
const MAX_NAME: usize = 32;

#[derive(Debug, Clone, Deserialize)]
pub struct CreateAllocationRequest {
    pub name: String,
    #[serde(default)]
    pub port: Option<i64>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RenameAllocationRequest {
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SetPrimaryResponse {
    pub primary_port: u16,
    pub allocations: Vec<Allocation>,
    pub restart_required: bool,
}

pub async fn list(pool: &SqlitePool, server: Id) -> sqlx::Result<Vec<Allocation>> {
    let rows: Vec<(u16, String)> = sqlx::query_as(
        "SELECT port, name FROM allocations WHERE server_id = ? AND is_primary = 0 ORDER BY port",
    )
    .bind(server)
    .fetch_all(pool)
    .await?;

    Ok(rows.into_iter().map(|(port, name)| Allocation { port, name }).collect())
}

pub async fn primary(pool: &SqlitePool, server: Id) -> sqlx::Result<Option<u16>> {
    sqlx::query_scalar("SELECT port FROM allocations WHERE server_id = ? AND is_primary = 1")
        .bind(server)
        .fetch_optional(pool)
        .await
}

pub async fn create(
    pool: &SqlitePool,
    server: Id,
    pool_range: PortRange,
    request: &CreateAllocationRequest,
    caller_is_admin: bool,
) -> Result<Allocation> {
    let name = check_name(&request.name)?;

    let mut transaction = pool.begin().await?;

    let held: Vec<u16> = sqlx::query_scalar("SELECT port FROM allocations WHERE server_id = ?")
        .bind(server)
        .fetch_all(&mut *transaction)
        .await?;
    if held.len() >= PER_SERVER {
        return Err(Failure::conflict(
            "allocation_limit",
            format!("a server keeps at most {PER_SERVER} ports"),
        ));
    }

    let port = match request.port {
        Some(asked) => {
            let wanted = u16::try_from(asked)
                .ok()
                .filter(|port| *port >= MIN_PORT)
                .ok_or_else(|| {
                    Failure::bad_request(
                        "invalid_port",
                        format!("{asked} is outside 1024-65535"),
                    )
                })?;
            let inside = (pool_range.from..=pool_range.to).contains(&wanted);
            if !inside && !caller_is_admin {
                return Err(Failure::new(
                    axum::http::StatusCode::FORBIDDEN,
                    "port_out_of_pool",
                    format!("{wanted} is not one of the ports this panel hands out"),
                ));
            }
            if !free_on_the_machine(wanted) {
                return Err(Failure::conflict(
                    "port_unavailable",
                    format!("another process on this machine holds {wanted}"),
                ));
            }
            wanted
        }
        None => next_free(&mut transaction, pool_range).await?,
    };

    let inserted = sqlx::query(
        "INSERT INTO allocations (port, server_id, name, is_primary, created_at) \
         VALUES (?, ?, ?, 0, ?)",
    )
    .bind(port)
    .bind(server)
    .bind(&name)
    .bind(Timestamp::now())
    .execute(&mut *transaction)
    .await;

    match inserted {
        Ok(_) => {
            transaction.commit().await?;
            Ok(Allocation { port, name })
        }
        Err(err) if is_taken(&err) => {
            Err(Failure::conflict("port_in_use", format!("{port} belongs to another server here")))
        }
        Err(err) => Err(err.into()),
    }
}

pub async fn rename(pool: &SqlitePool, server: Id, port: u16, name: &str) -> Result<Allocation> {
    let name = check_name(name)?;
    let changed = sqlx::query("UPDATE allocations SET name = ? WHERE server_id = ? AND port = ?")
        .bind(&name)
        .bind(server)
        .bind(port)
        .execute(pool)
        .await?;

    if changed.rows_affected() == 0 {
        return Err(unknown(port));
    }
    Ok(Allocation { port, name })
}

pub async fn remove(pool: &SqlitePool, server: Id, port: u16) -> Result<()> {
    let is_primary: Option<bool> =
        sqlx::query_scalar("SELECT is_primary FROM allocations WHERE server_id = ? AND port = ?")
            .bind(server)
            .bind(port)
            .fetch_optional(pool)
            .await?;

    match is_primary {
        None => Err(unknown(port)),
        Some(true) => Err(Failure::conflict(
            "primary_allocation",
            "the primary port is swapped, not deleted",
        )),
        Some(false) => {
            sqlx::query("DELETE FROM allocations WHERE server_id = ? AND port = ?")
                .bind(server)
                .bind(port)
                .execute(pool)
                .await?;
            Ok(())
        }
    }
}

pub async fn set_primary(pool: &SqlitePool, server: Id, port: u16) -> Result<()> {
    let mut transaction = pool.begin().await?;

    let is_primary: Option<bool> =
        sqlx::query_scalar("SELECT is_primary FROM allocations WHERE server_id = ? AND port = ?")
            .bind(server)
            .bind(port)
            .fetch_optional(&mut *transaction)
            .await?;

    match is_primary {
        None => return Err(unknown(port)),
        Some(true) => {
            return Err(Failure::conflict("already_primary", format!("{port} is already primary")))
        }
        Some(false) => {}
    }

    let published: Option<u16> =
        sqlx::query_scalar("SELECT local_port FROM playit_tunnels WHERE server_id = ?")
            .bind(server)
            .fetch_optional(&mut *transaction)
            .await?;
    if published.is_some() {
        return Err(Failure::conflict(
            "playit_tunnel_exists",
            "this server has a public address through playit.gg; give that address back \
             before swapping the primary port",
        ));
    }

    sqlx::query("UPDATE allocations SET is_primary = 0 WHERE server_id = ? AND is_primary = 1")
        .bind(server)
        .execute(&mut *transaction)
        .await?;
    sqlx::query("UPDATE allocations SET is_primary = 1 WHERE server_id = ? AND port = ?")
        .bind(server)
        .bind(port)
        .execute(&mut *transaction)
        .await?;

    transaction.commit().await?;
    Ok(())
}

async fn next_free(
    transaction: &mut sqlx::SqliteConnection,
    range: PortRange,
) -> Result<u16> {
    let taken: Vec<u16> =
        sqlx::query_scalar("SELECT port FROM allocations WHERE port BETWEEN ? AND ? ORDER BY port")
            .bind(range.from)
            .bind(range.to)
            .fetch_all(&mut *transaction)
            .await?;

    (range.from..=range.to)
        .find(|port| !taken.contains(port) && free_on_the_machine(*port))
        .ok_or_else(|| {
            Failure::conflict("port_pool_exhausted", "no port of the pool is free")
        })
}

fn free_on_the_machine(port: u16) -> bool {
    match std::net::TcpListener::bind(("0.0.0.0", port)) {
        Ok(listener) => {
            drop(listener);
            true
        }
        Err(err) => err.kind() != std::io::ErrorKind::AddrInUse,
    }
}

fn check_name(name: &str) -> Result<String> {
    let trimmed = name.trim();
    if trimmed.is_empty() || trimmed.chars().count() > MAX_NAME {
        return Err(Failure::bad_request(
            "invalid_name",
            format!("a port name is 1 to {MAX_NAME} characters"),
        ));
    }
    if trimmed.contains(|letter: char| letter.is_control()) {
        return Err(Failure::bad_request("invalid_name", "a port name has no control characters"));
    }
    Ok(trimmed.to_owned())
}

fn unknown(port: u16) -> Failure {
    Failure::not_found("allocation_not_found", format!("this server has no port {port}"))
}

fn is_taken(err: &sqlx::Error) -> bool {
    err.as_database_error().is_some_and(|db| db.is_unique_violation())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::harness::{an_allocation, pool_with_server};
    use std::sync::atomic::{AtomicU16, Ordering};

    fn a_pool(width: u16) -> PortRange {
        static NEXT: AtomicU16 = AtomicU16::new(61000);
        let from = NEXT.fetch_add(width + 1, Ordering::Relaxed);
        PortRange { from, to: from + width }
    }

    fn range(from: u16, to: u16) -> PortRange {
        PortRange { from, to }
    }

    fn wanted(name: &str, port: Option<u16>) -> CreateAllocationRequest {
        CreateAllocationRequest { name: name.to_owned(), port: port.map(i64::from) }
    }

    fn asked_for(name: &str, port: i64) -> CreateAllocationRequest {
        CreateAllocationRequest { name: name.to_owned(), port: Some(port) }
    }

    #[tokio::test]
    async fn the_list_is_every_port_but_the_primary_one_in_order() {
        let (pool, server, _) = pool_with_server().await;
        an_allocation(&pool, server, 25570, "voice", false).await;
        an_allocation(&pool, server, 25565, "game", true).await;
        an_allocation(&pool, server, 25567, "map", false).await;

        let ports: Vec<u16> = list(&pool, server).await.unwrap().iter().map(|a| a.port).collect();
        assert_eq!(ports, [25567, 25570], "9.6: ascending, and the primary is not in it");
        assert_eq!(primary(&pool, server).await.unwrap(), Some(25565));
    }

    #[tokio::test]
    async fn the_pool_hands_out_the_next_free_number() {
        let (pool, server, _) = pool_with_server().await;
        let mine = a_pool(5);
        an_allocation(&pool, server, mine.from, "game", true).await;

        let first = create(&pool, server, mine, &wanted("map", None), false).await.unwrap();
        assert_eq!(first.port, mine.from + 1);

        let second = create(&pool, server, mine, &wanted("voice", None), false).await.unwrap();
        assert_eq!(second.port, mine.from + 2);
    }

    #[tokio::test]
    async fn a_pool_with_nothing_left_says_so_rather_than_reaching_past_it() {
        let (pool, server, _) = pool_with_server().await;
        let mine = a_pool(0);
        an_allocation(&pool, server, mine.from, "game", true).await;

        let refusal =
            create(&pool, server, mine, &wanted("map", None), false).await.unwrap_err();
        assert_eq!(refusal.code(), "port_pool_exhausted");
    }

    #[tokio::test]
    async fn a_port_of_another_server_is_in_use_and_not_merely_taken() {
        let (pool, mine, owner) = pool_with_server().await;
        let theirs = crate::auth::harness::a_server(&pool, owner, "Other", 2048).await;
        let ours = a_pool(4);
        an_allocation(&pool, theirs, ours.from, "game", true).await;

        let refusal = create(&pool, mine, ours, &wanted("map", Some(ours.from)), true)
            .await
            .unwrap_err();
        assert_eq!(refusal.code(), "port_in_use");
        assert_eq!(refusal.status(), axum::http::StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn only_an_admin_reaches_outside_the_pool() {
        let (pool, server, _) = pool_with_server().await;
        let mine = a_pool(5);
        let outside = a_pool(0).from;

        let refusal = create(&pool, server, mine, &wanted("far", Some(outside)), false)
            .await
            .unwrap_err();
        assert_eq!(refusal.code(), "port_out_of_pool");
        assert_eq!(refusal.status(), axum::http::StatusCode::FORBIDDEN);

        let allowed =
            create(&pool, server, mine, &wanted("far", Some(outside)), true).await.unwrap();
        assert_eq!(allowed.port, outside);

        let ordinary = create(&pool, server, mine, &wanted("near", Some(mine.from)), false)
            .await
            .unwrap();
        assert_eq!(ordinary.port, mine.from);
    }

    #[tokio::test]
    async fn the_bounds_of_9_7_each_refuse_on_their_own() {
        let (pool, server, _) = pool_with_server().await;
        let full = a_pool(35);

        for outside in [80, 0, -1, 70_000] {
            let refusal =
                create(&pool, server, full, &asked_for("far out", outside), true).await.unwrap_err();
            assert_eq!(refusal.code(), "invalid_port", "{outside}");
            assert_eq!(refusal.status(), axum::http::StatusCode::BAD_REQUEST);
        }

        for bad in ["", "   ", &"x".repeat(33)] {
            let refusal = create(&pool, server, full, &wanted(bad, None), true).await.unwrap_err();
            assert_eq!(refusal.code(), "invalid_name", "{bad:?}");
        }

        for index in 0..PER_SERVER {
            create(&pool, server, full, &wanted(&format!("p{index}"), None), true).await.unwrap();
        }
        let refusal = create(&pool, server, full, &wanted("one too many", None), true)
            .await
            .unwrap_err();
        assert_eq!(refusal.code(), "allocation_limit");
    }

    #[tokio::test]
    async fn renaming_touches_the_name_and_nothing_else() {
        let (pool, server, _) = pool_with_server().await;
        an_allocation(&pool, server, 25570, "voice", false).await;

        let renamed = rename(&pool, server, 25570, "  Language  ").await.unwrap();
        assert_eq!(renamed, Allocation { port: 25570, name: "Language".to_owned() });
        assert_eq!(rename(&pool, server, 25599, "x").await.unwrap_err().code(), "allocation_not_found");
        assert_eq!(rename(&pool, server, 25570, "").await.unwrap_err().code(), "invalid_name");
    }

    #[tokio::test]
    async fn the_primary_port_is_swapped_and_never_deleted() {
        let (pool, server, _) = pool_with_server().await;
        an_allocation(&pool, server, 25565, "game", true).await;
        an_allocation(&pool, server, 25570, "voice", false).await;

        assert_eq!(remove(&pool, server, 25565).await.unwrap_err().code(), "primary_allocation");
        assert_eq!(remove(&pool, server, 25599).await.unwrap_err().code(), "allocation_not_found");

        set_primary(&pool, server, 25570).await.unwrap();
        assert_eq!(primary(&pool, server).await.unwrap(), Some(25570));

        let left: Vec<u16> = list(&pool, server).await.unwrap().iter().map(|a| a.port).collect();
        assert_eq!(left, [25565], "the old primary stays with the server");

        assert_eq!(set_primary(&pool, server, 25570).await.unwrap_err().code(), "already_primary");
        assert_eq!(
            set_primary(&pool, server, 25599).await.unwrap_err().code(),
            "allocation_not_found"
        );
    }

    #[tokio::test]
    async fn a_deleted_port_goes_back_into_the_pool() {
        let (pool, server, _) = pool_with_server().await;
        let mine = a_pool(5);
        an_allocation(&pool, server, mine.from, "game", true).await;
        let handed = create(&pool, server, mine, &wanted("map", None), false).await.unwrap();

        remove(&pool, server, handed.port).await.unwrap();
        let again = create(&pool, server, mine, &wanted("map", None), false).await.unwrap();
        assert_eq!(again.port, handed.port, "9.9: the number comes back");
    }

    #[tokio::test]
    async fn a_port_a_foreign_process_holds_is_neither_handed_out_nor_accepted() {
        let (pool, server, _) = pool_with_server().await;
        let stranger = std::net::TcpListener::bind(("0.0.0.0", 0)).unwrap();
        let held = stranger.local_addr().unwrap().port();

        let refusal = create(&pool, server, range(held, held), &wanted("held", Some(held)), true)
            .await
            .unwrap_err();
        assert_eq!(refusal.code(), "port_unavailable");

        let empty = create(&pool, server, range(held, held), &wanted("held", None), true)
            .await
            .unwrap_err();
        assert_eq!(empty.code(), "port_pool_exhausted", "the pool skips it and then runs out");

        drop(stranger);
    }
}
