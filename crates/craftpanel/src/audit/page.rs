use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;
use serde_json::Value;
use sqlx::{QueryBuilder, Sqlite, SqlitePool};

use crate::auth::error::{Failure, Result};
use crate::model::{AuditAction, AuditActor, AuditEntry, AuditEvent, Id, Timestamp};

const PAGE: u32 = 200;
const PAGE_CEILING: u32 = 500;

const MOST_ACTORS: usize = 500;
const LOOKUP_BITE: usize = 500;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Order {
    Asc,
    Desc,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Query {
    pub limit: u32,
    pub offset: u32,
    pub order: Order,
    pub min_datetime: Option<Timestamp>,
    pub max_datetime: Option<Timestamp>,
    pub actors: Vec<Id>,
    pub actions: Vec<AuditAction>,
}

impl Default for Query {
    fn default() -> Self {
        Self {
            limit: PAGE,
            offset: 0,
            order: Order::Desc,
            min_datetime: None,
            max_datetime: None,
            actors: Vec::new(),
            actions: Vec::new(),
        }
    }
}

impl Query {
    pub fn read(pairs: &[(String, String)]) -> Result<Self> {
        let mut query = Self::default();
        for (key, value) in pairs {
            match key.as_str() {
                "limit" => query.limit = number(value, "limit")?.clamp(1, PAGE_CEILING),
                "offset" => query.offset = number(value, "offset")?,
                "order" => {
                    query.order = match value.as_str() {
                        "asc" => Order::Asc,
                        "desc" => Order::Desc,
                        other => {
                            return Err(Failure::invalid_request(format!(
                                "order is asc or desc, not {other:?}"
                            )))
                        }
                    }
                }
                "min_datetime" => query.min_datetime = Some(moment(value, "min_datetime")?),
                "max_datetime" => query.max_datetime = Some(moment(value, "max_datetime")?),
                "actor" => query.actors.push(
                    value
                        .parse()
                        .map_err(|_| Failure::invalid_request("actor is not a ULID"))?,
                ),
                "action" => query.actions.push(value.parse().map_err(|_| {
                    Failure::invalid_request(format!("{value:?} is not an action of 11.9"))
                })?),
                _ => {}
            }
        }

        query.actors.sort_unstable();
        query.actors.dedup();
        query.actions.sort_unstable();
        query.actions.dedup();

        if query.actors.len() > MOST_ACTORS {
            return Err(Failure::invalid_request(format!(
                "at most {MOST_ACTORS} actors can be filtered on, not {}",
                query.actors.len()
            )));
        }
        Ok(query)
    }
}

#[derive(Debug, Serialize)]
pub struct Page {
    pub next_offset: Option<u32>,
    pub data: Vec<AuditEntry>,
    pub users: BTreeMap<String, UserSummary>,
    pub addons: BTreeMap<String, AddonSummary>,
    pub versions: BTreeMap<String, VersionSummary>,
}

#[derive(Debug, Serialize)]
pub struct UserSummary {
    pub username: String,
    pub avatar_url: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct AddonSummary {
    pub title: String,
    pub slug: Option<String>,
    pub icon_url: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct VersionSummary {
    pub name: String,
    pub version_number: Option<String>,
}

pub async fn page(pool: &SqlitePool, server: Id, query: &Query) -> Result<Page> {
    let mut data = rows(pool, server, query).await?;
    let more = data.len() > query.limit as usize;
    data.truncate(query.limit as usize);

    let mut users: BTreeSet<Id> = BTreeSet::new();
    let mut addons: BTreeSet<String> = BTreeSet::new();
    let mut versions: BTreeSet<String> = BTreeSet::new();
    for entry in &data {
        let AuditActor::User { user_id } = entry.actor;
        users.insert(user_id);
        if let Some(metadata) = &entry.action.metadata {
            harvest(metadata, &mut users, &mut addons, &mut versions);
        }
    }

    Ok(Page {
        next_offset: more.then(|| query.offset + query.limit),
        data,
        users: named_users(pool, &users).await?,
        addons: cached_addons(pool, &addons).await?,
        versions: cached_versions(pool, &versions).await?,
    })
}

async fn rows(pool: &SqlitePool, server: Id, query: &Query) -> Result<Vec<AuditEntry>> {
    let mut sql = QueryBuilder::<Sqlite>::new(
        "SELECT id, actor_user_id, action, metadata, created_at FROM audit_log WHERE server_id = ",
    );
    sql.push_bind(server);

    if let Some(from) = query.min_datetime {
        sql.push(" AND created_at >= ").push_bind(from);
    }
    if let Some(until) = query.max_datetime {
        sql.push(" AND created_at <= ").push_bind(until);
    }
    if !query.actors.is_empty() {
        sql.push(" AND actor_user_id IN (");
        let mut list = sql.separated(", ");
        for actor in &query.actors {
            list.push_bind(*actor);
        }
        sql.push(")");
    }
    if !query.actions.is_empty() {
        sql.push(" AND action IN (");
        let mut list = sql.separated(", ");
        for action in &query.actions {
            list.push_bind(*action);
        }
        sql.push(")");
    }

    sql.push(match query.order {
        Order::Asc => " ORDER BY created_at ASC, rowid ASC LIMIT ",
        Order::Desc => " ORDER BY created_at DESC, rowid DESC LIMIT ",
    });
    sql.push_bind(query.limit + 1);
    sql.push(" OFFSET ").push_bind(query.offset);

    let found = sql
        .build_query_as::<(Id, Id, AuditAction, Option<String>, Timestamp)>()
        .fetch_all(pool)
        .await?;

    Ok(found
        .into_iter()
        .map(|(id, actor, action, metadata, created_at)| AuditEntry {
            id,
            actor: AuditActor::User { user_id: actor },
            action: AuditEvent {
                action,
                metadata: metadata.as_deref().and_then(|json| serde_json::from_str(json).ok()),
            },
            server_id: server,
            world_id: None,
            timestamp: created_at,
        })
        .collect())
}

fn harvest(
    metadata: &Value,
    users: &mut BTreeSet<Id>,
    addons: &mut BTreeSet<String>,
    versions: &mut BTreeSet<String>,
) {
    if let Some(user) = metadata.get("user_id").and_then(Value::as_str) {
        if let Ok(id) = user.parse() {
            users.insert(id);
        }
    }
    if let Some(list) = metadata.get("addons").and_then(Value::as_array) {
        for addon in list {
            if let Some(project) = addon.get("addon_id").and_then(Value::as_str) {
                addons.insert(project.to_owned());
            }
            if let Some(version) = addon.get("version_id").and_then(Value::as_str) {
                versions.insert(version.to_owned());
            }
        }
    }
    if let Some(spec) = metadata.get("spec") {
        if let Some(project) = spec.get("project_id").and_then(Value::as_str) {
            addons.insert(project.to_owned());
        }
        if let Some(version) = spec.get("version_id").and_then(Value::as_str) {
            versions.insert(version.to_owned());
        }
    }
}

async fn named_users(pool: &SqlitePool, wanted: &BTreeSet<Id>) -> Result<BTreeMap<String, UserSummary>> {
    let mut found = BTreeMap::new();
    for bite in bites(wanted) {
        let mut sql = QueryBuilder::<Sqlite>::new("SELECT id, username FROM users WHERE id IN (");
        let mut list = sql.separated(", ");
        for id in bite {
            list.push_bind(*id);
        }
        sql.push(")");

        for (id, username) in sql.build_query_as::<(Id, String)>().fetch_all(pool).await? {
            found.insert(id.to_string(), UserSummary { username, avatar_url: None });
        }
    }
    Ok(found)
}

async fn cached_addons(
    pool: &SqlitePool,
    wanted: &BTreeSet<String>,
) -> Result<BTreeMap<String, AddonSummary>> {
    let mut found = BTreeMap::new();
    for bite in bites(wanted) {
        let mut sql = QueryBuilder::<Sqlite>::new(
            "SELECT project_id, title, slug, icon_url FROM modrinth_project WHERE project_id IN (",
        );
        let mut list = sql.separated(", ");
        for id in bite {
            list.push_bind(id.as_str());
        }
        sql.push(")");

        let rows = sql
            .build_query_as::<(String, String, Option<String>, Option<String>)>()
            .fetch_all(pool)
            .await?;
        for (project_id, title, slug, icon_url) in rows {
            found.insert(project_id, AddonSummary { title, slug, icon_url });
        }
    }
    Ok(found)
}

async fn cached_versions(
    pool: &SqlitePool,
    wanted: &BTreeSet<String>,
) -> Result<BTreeMap<String, VersionSummary>> {
    let mut found = BTreeMap::new();
    for bite in bites(wanted) {
        let mut sql = QueryBuilder::<Sqlite>::new(
            "SELECT version_id, json_extract(payload, '$.name'), \
             json_extract(payload, '$.version_number') FROM modrinth_version WHERE version_id IN (",
        );
        let mut list = sql.separated(", ");
        for id in bite {
            list.push_bind(id.as_str());
        }
        sql.push(")");

        let rows = sql
            .build_query_as::<(String, Option<String>, Option<String>)>()
            .fetch_all(pool)
            .await?;
        for (version_id, name, version_number) in rows {
            let Some(name) = name.or_else(|| version_number.clone()) else { continue };
            found.insert(version_id, VersionSummary { name, version_number });
        }
    }
    Ok(found)
}

fn bites<T>(wanted: &BTreeSet<T>) -> Vec<Vec<&T>> {
    wanted.iter().collect::<Vec<_>>().chunks(LOOKUP_BITE).map(<[&T]>::to_vec).collect()
}

fn number(value: &str, field: &'static str) -> Result<u32> {
    value.parse().map_err(|_| Failure::invalid_request(format!("{field} is not a number")))
}

fn moment(value: &str, field: &'static str) -> Result<Timestamp> {
    value.parse().map_err(|_| Failure::invalid_request(format!("{field} is not an RFC 3339 time")))
}
