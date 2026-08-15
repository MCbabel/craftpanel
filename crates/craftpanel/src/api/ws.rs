use std::sync::Arc;

use axum::extract::ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::response::Response;
use axum::routing::get;
use axum::Router;
use futures::{Sink, SinkExt, Stream, StreamExt};

use crate::model::{
    Id, Minecraft, Permission, Permissions, Server, ServerFlows, ServerNet, ServerStatus,
    ServerUpstream, Timestamp, UpdateChannel,
};
use crate::ops::{self, Attachment, Caller, Operations, ServerEvent, WsMessage};

const PING_EVERY: std::time::Duration = std::time::Duration::from_secs(30);
const RECHECK_EVERY: std::time::Duration = std::time::Duration::from_secs(60);
const MOST_A_CLIENT_MAY_SAY: usize = 4 * 1024;

pub fn router(operations: Arc<Operations>) -> Router<crate::AppState> {
    routes(operations)
}

fn routes<S: Clone + Send + Sync + 'static>(operations: Arc<Operations>) -> Router<S> {
    Router::new().route("/servers/{server}/ws", get(connect)).with_state(operations)
}

async fn connect(
    State(operations): State<Arc<Operations>>,
    headers: HeaderMap,
    Path(server): Path<Id>,
    upgrade: WebSocketUpgrade,
) -> Response {
    let admitted = admit(&operations, &headers, server).await;
    upgrade.max_message_size(MOST_A_CLIENT_MAY_SAY).on_upgrade(move |socket| async move {
        match admitted {
            Ok(caller) => run(operations, server, caller, socket).await,
            Err(code) => shut(socket, code).await,
        }
    })
}

#[derive(Debug, Clone)]
struct Admitted {
    caller: Caller,
    mask: Permissions,
}

async fn admit(
    operations: &Operations,
    headers: &HeaderMap,
    server: Id,
) -> Result<Admitted, u16> {
    if !same_origin(headers) {
        return Err(4403);
    }
    let caller = ops::caller(operations.pool(), headers).await.map_err(|_| 4401u16)?;
    let mask = ops::permissions(operations.pool(), server, &caller).await.map_err(|_| 4403u16)?;
    ops::require(mask, Permission::BaseRead).map_err(|_| 4403u16)?;
    Ok(Admitted { caller, mask })
}

fn same_origin(headers: &HeaderMap) -> bool {
    let Some(origin) = headers.get(axum::http::header::ORIGIN).and_then(|v| v.to_str().ok())
    else {
        return false;
    };
    let Some(host) = headers.get(axum::http::header::HOST).and_then(|v| v.to_str().ok()) else {
        return false;
    };
    origin
        .split_once("://")
        .is_some_and(|(_, authority)| authority.eq_ignore_ascii_case(host))
}

async fn shut(mut socket: WebSocket, code: u16) {
    let frame = CloseFrame { code, reason: close_reason(code).into() };
    socket.send(Message::Close(Some(frame))).await.ok();
}

fn close_reason(code: u16) -> &'static str {
    match code {
        4401 => "no session",
        4403 => "no access to this server",
        4404 => "the server is gone",
        4429 => "too many sockets for this session",
        _ => "",
    }
}

async fn run(operations: Arc<Operations>, server: Id, admitted: Admitted, socket: WebSocket) {
    let Ok(channel) = operations.channel(server).await else {
        shut(socket, 4404).await;
        return;
    };
    let Some(_socket_count) = channel.open_socket(admitted.caller.session_id) else {
        shut(socket, 4429).await;
        return;
    };

    let attachment = channel.attach();
    let (writer, incoming) = socket.split();
    let code = serve(&operations, server, &admitted, attachment, writer, incoming).await;
    tracing::debug!(%server, code, "socket closed");
}

async fn serve<W, R>(
    operations: &Operations,
    server: Id,
    admitted: &Admitted,
    attachment: Attachment,
    mut writer: W,
    incoming: R,
) -> u16
where
    W: Sink<Message> + Unpin,
    R: Stream<Item = Result<Message, axum::Error>> + Unpin,
{
    let code = converse(operations, server, admitted, attachment, &mut writer, incoming).await;
    let frame = CloseFrame { code, reason: close_reason(code).into() };
    writer.send(Message::Close(Some(frame))).await.ok();
    code
}

async fn converse<W, R>(
    operations: &Operations,
    server: Id,
    admitted: &Admitted,
    attachment: Attachment,
    writer: &mut W,
    mut incoming: R,
) -> u16
where
    W: Sink<Message> + Unpin,
    R: Stream<Item = Result<Message, axum::Error>> + Unpin,
{
    let Attachment { mut events, history, state, stats } = attachment;
    let mask = admitted.mask;

    let opening = match server_object(operations, server, mask).await {
        Ok(object) => {
            let snapshot = operations.snapshot(server).await.ok();
            ops_opening(object, state, snapshot, &history, &stats)
        }
        Err(_) => return 4404,
    };
    for message in opening {
        if say(writer, &message).await.is_err() {
            return 1000;
        }
    }

    let mut ping = tokio::time::interval(PING_EVERY);
    ping.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    ping.tick().await;
    let mut recheck = tokio::time::interval(RECHECK_EVERY);
    recheck.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    recheck.tick().await;

    loop {
        tokio::select! {
            frame = incoming.next() => match frame {
                None | Some(Err(_)) | Some(Ok(Message::Close(_))) => return 1000,
                Some(Ok(_)) => {}
            },
            event = events.recv() => match event {
                Ok(ServerEvent::Say(json)) => {
                    if write(writer, Message::text(json.to_string())).await.is_err() {
                        return 1000;
                    }
                }
                Ok(ServerEvent::Server(object)) => {
                    let mut object = (*object).clone();
                    object.current_user_permissions = mask;
                    if say(writer, &WsMessage::Server { server: object }).await.is_err() {
                        return 1000;
                    }
                }
                Ok(ServerEvent::Close(code)) => return code,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(missed)) => {
                    tracing::warn!(%server, missed, "a socket fell behind");
                    if let Ok(snapshot) = operations.snapshot(server).await {
                        if say(writer, &WsMessage::Operations(snapshot)).await.is_err() {
                            return 1000;
                        }
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => return 1000,
            },
            _ = ping.tick() => {
                if write(writer, Message::Ping(Default::default())).await.is_err() {
                    return 1000;
                }
            }
            _ = recheck.tick() => {
                if let Some(code) = withdrawn(operations, server, &admitted.caller).await {
                    return code;
                }
            }
        }
    }
}

async fn withdrawn(operations: &Operations, server: Id, caller: &Caller) -> Option<u16> {
    if !ops::session_alive(operations.pool(), caller.session_id).await {
        return Some(4401);
    }
    match ops::permissions(operations.pool(), server, caller).await {
        Ok(mask) if mask.allows(Permission::BaseRead) => None,
        _ => Some(4403),
    }
}

fn ops_opening(
    server: Server,
    state: ops::StateReport,
    snapshot: Option<ops::Snapshot>,
    history: &ops::History,
    stats: &[ops::StatsSample],
) -> Vec<WsMessage> {
    let running = state.power_state == crate::model::PowerState::Running;
    let mut messages = vec![WsMessage::Server { server }, WsMessage::State(state)];
    if let Some(snapshot) = snapshot {
        messages.push(WsMessage::Operations(snapshot));
    }

    messages.push(WsMessage::ConsoleHistoryStart {
        total_lines: history.lines.len(),
        dropped_lines: history.dropped,
    });
    messages.extend(blocks(history).map(|(seq, lines)| WsMessage::Console { seq, lines }));
    messages.push(WsMessage::ConsoleHistoryEnd);

    if running {
        messages.extend(stats.iter().copied().map(WsMessage::Stats));
    } else if let Some(last) = stats.last() {
        messages.push(WsMessage::Stats(ops::StatsSample {
            cpu_percent: 0.0,
            ram_usage_bytes: 0,
            ..*last
        }));
    }
    messages
}

const BLOCK_LINES: usize = 500;
const BLOCK_BYTES: usize = 64 * 1024;

fn blocks(history: &ops::History) -> impl Iterator<Item = (u64, Vec<std::sync::Arc<str>>)> + '_ {
    let mut seq = history.first_seq;
    let mut rest = history.lines.as_slice();
    std::iter::from_fn(move || {
        if rest.is_empty() {
            return None;
        }
        let mut bytes = 0;
        let mut taken = 0;
        while taken < rest.len() && taken < BLOCK_LINES && (bytes < BLOCK_BYTES || taken == 0) {
            bytes += rest[taken].len();
            taken += 1;
        }
        let (block, remainder) = rest.split_at(taken);
        rest = remainder;
        let first = seq;
        seq += taken as u64;
        Some((first, block.to_vec()))
    })
}

async fn say<W: Sink<Message> + Unpin>(writer: &mut W, message: &WsMessage) -> Result<(), ()> {
    let json = serde_json::to_string(message).map_err(|_| ())?;
    write(writer, Message::text(json)).await
}

async fn write<W: Sink<Message> + Unpin>(writer: &mut W, message: Message) -> Result<(), ()> {
    writer.send(message).await.map_err(|_| ())
}

async fn server_object(
    operations: &Operations,
    server: Id,
    mask: Permissions,
) -> ops::Answer<Server> {
    let row: Option<(
        Id,
        String,
        Id,
        ServerStatus,
        Option<crate::model::LoaderId>,
        Option<String>,
        Option<String>,
        i64,
        UpdateChannel,
        bool,
        Timestamp,
    )> = sqlx::query_as(
        "SELECT id, name, owner_id, status, loader, loader_version, game_version,
                memory_mib, update_channel, flows_intro, created_at
           FROM servers WHERE id = ?",
    )
    .bind(server)
    .fetch_optional(operations.pool())
    .await?;

    let Some((
        id,
        name,
        owner_id,
        status,
        loader,
        loader_version,
        game_version,
        memory_mib,
        update_channel,
        flows_intro,
        created_at,
    )) = row
    else {
        return Err(ops::Fault::server_not_found());
    };

    let port: Option<(i64,)> =
        sqlx::query_as("SELECT port FROM allocations WHERE server_id = ? AND is_primary = 1")
            .bind(server)
            .fetch_optional(operations.pool())
            .await?;
    let (address, quota): (Option<String>, i64) = sqlx::query_as(
        "SELECT public_address, max_backups_per_server FROM panel_settings WHERE id = 1",
    )
    .fetch_one(operations.pool())
    .await?;
    let (used,): (i64,) = sqlx::query_as("SELECT count(*) FROM backups WHERE server_id = ?")
        .bind(server)
        .fetch_one(operations.pool())
        .await?;
    let modpack: Option<(String, Option<String>)> =
        sqlx::query_as("SELECT project_id, version_id FROM server_modpacks WHERE server_id = ?")
            .bind(server)
            .fetch_optional(operations.pool())
            .await
            .unwrap_or(None);

    Ok(Server {
        id,
        name,
        owner_id,
        status,
        game: Minecraft,
        loader,
        loader_version,
        game_version,
        net: ServerNet {
            ip: address,
            port: port.map(|(port,)| port as u16).unwrap_or_default(),
            domain: String::new(),
        },
        memory_mib: memory_mib.max(0) as u32,
        upstream: modpack.and_then(|(project_id, version_id)| {
            version_id.map(|version_id| ServerUpstream::Modpack { project_id, version_id })
        }),
        flows: ServerFlows { intro: flows_intro },
        backup_quota: quota.max(0) as u32,
        used_backup_quota: used.max(0) as u32,
        update_channel,
        current_user_permissions: mask,
        created_at,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{OperationKind, PanelRole, PowerState, ServerRole};
    use crate::ops::testing::{self, DataDir};
    use crate::ops::{NewOperation, StateReport, StatsSample};
    use axum::http::header::{COOKIE, HOST, ORIGIN};
    use sqlx::SqlitePool;

    fn headers(pairs: &[(axum::http::HeaderName, &str)]) -> HeaderMap {
        let mut headers = HeaderMap::new();
        for (name, value) in pairs {
            headers.insert(name.clone(), value.parse().expect("a header value"));
        }
        headers
    }

    struct Fixture {
        operations: Arc<Operations>,
        pool: SqlitePool,
        owner: Id,
        server: Id,
        cookie: String,
        _dir: DataDir,
    }

    async fn fixture() -> Fixture {
        let (operations, dir, pool) = testing::operations().await;
        let owner = testing::a_user(&pool, PanelRole::User).await;
        let server = testing::a_server(&pool, owner).await;
        let cookie = testing::a_session(&pool, owner).await;
        Fixture { operations, pool, owner, server, cookie, _dir: dir }
    }

    impl Fixture {
        fn headers(&self, cookie: Option<&str>) -> HeaderMap {
            let cookie = cookie.unwrap_or(&self.cookie);
            headers(&[
                (COOKIE, &format!("craft_session={cookie}")),
                (ORIGIN, "https://panel.example"),
                (HOST, "panel.example"),
            ])
        }

        fn as_owner(&self, mask: Permissions) -> Admitted {
            Admitted {
                caller: Caller {
                    user_id: self.owner,
                    session_id: Id::new(),
                    panel_role: PanelRole::User,
                },
                mask,
            }
        }
    }

    #[tokio::test]
    async fn a_viewer_is_let_in_and_a_stranger_is_not() {
        let fixture = fixture().await;
        let viewer = testing::a_user(&fixture.pool, PanelRole::User).await;
        let viewer_cookie = testing::a_session(&fixture.pool, viewer).await;
        sqlx::query(
            "INSERT INTO server_members (id, server_id, user_id, role, invited_at, joined_at)
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(Id::new())
        .bind(fixture.server)
        .bind(viewer)
        .bind(ServerRole::Viewer)
        .bind(Timestamp::now())
        .bind(Timestamp::now())
        .execute(&fixture.pool)
        .await
        .expect("a membership");

        let watching = admit(
            &fixture.operations,
            &fixture.headers(Some(&viewer_cookie)),
            fixture.server,
        )
        .await;
        assert!(watching.is_ok(), "a viewer may watch");
        assert_eq!(watching.map(|a| a.mask.role()).unwrap_or(ServerRole::Owner), ServerRole::Viewer);

        let stranger = testing::a_user(&fixture.pool, PanelRole::User).await;
        let stranger_cookie = testing::a_session(&fixture.pool, stranger).await;
        assert_eq!(
            admit(&fixture.operations, &fixture.headers(Some(&stranger_cookie)), fixture.server)
                .await
                .err(),
            Some(4403)
        );
    }

    #[tokio::test]
    async fn the_four_refusals_of_13_6_each_have_their_own_code() {
        let fixture = fixture().await;

        let bare = headers(&[(ORIGIN, "https://panel.example"), (HOST, "panel.example")]);
        assert_eq!(admit(&fixture.operations, &bare, fixture.server).await.err(), Some(4401));

        let foreign = headers(&[
            (COOKIE, &format!("craft_session={}", fixture.cookie)),
            (ORIGIN, "https://evil.example"),
            (HOST, "panel.example"),
        ]);
        assert_eq!(admit(&fixture.operations, &foreign, fixture.server).await.err(), Some(4403));

        let silent = headers(&[
            (COOKIE, &format!("craft_session={}", fixture.cookie)),
            (HOST, "panel.example"),
        ]);
        assert_eq!(admit(&fixture.operations, &silent, fixture.server).await.err(), Some(4403));

        assert_eq!(
            admit(&fixture.operations, &fixture.headers(None), Id::new()).await.err(),
            Some(4403)
        );

        assert!(admit(&fixture.operations, &fixture.headers(None), fixture.server).await.is_ok());
    }

    #[tokio::test]
    async fn the_fifth_socket_of_a_session_is_the_one_that_is_refused() {
        let fixture = fixture().await;
        let channel = fixture.operations.channel(fixture.server).await.expect("a channel");
        let session = Id::new();
        let held: Vec<_> = (0..4).map(|_| channel.open_socket(session).expect("a socket")).collect();
        assert!(channel.open_socket(session).is_none(), "the fifth is 4429");
        drop(held);
    }

    #[tokio::test]
    async fn the_opening_is_server_state_operations_console_stats() {
        let fixture = fixture().await;
        let channel = fixture.operations.channel(fixture.server).await.expect("a channel");
        channel.console_lines(&["[15:04:22] one".to_owned(), "[15:04:23] two".to_owned()]);
        let attachment = channel.attach();

        fixture
            .operations
            .create(NewOperation::new(
                fixture.server,
                OperationKind::InstallLoader,
                Some(fixture.owner),
            ))
            .await
            .expect("an operation");

        let object = server_object(
            &fixture.operations,
            fixture.server,
            Permissions::from_role(ServerRole::Viewer),
        )
        .await
        .expect("a server object");
        let snapshot = fixture.operations.snapshot(fixture.server).await.ok();
        let messages =
            ops_opening(object, attachment.state, snapshot, &attachment.history, &attachment.stats);

        let kinds: Vec<String> = messages
            .iter()
            .map(|message| {
                serde_json::to_value(message).expect("json")["type"]
                    .as_str()
                    .expect("a type")
                    .to_owned()
            })
            .collect();
        assert_eq!(
            kinds,
            [
                "server",
                "state",
                "operations",
                "console_history_start",
                "console",
                "console_history_end",
            ],
            "a stopped server sends no stats retrospect"
        );

        let start = serde_json::to_value(&messages[3]).expect("json");
        assert_eq!(start["total_lines"], 2);
        assert_eq!(start["dropped_lines"], 0);
        let console = serde_json::to_value(&messages[4]).expect("json");
        assert_eq!(console["seq"], 0);
        assert_eq!(console["lines"].as_array().expect("lines").len(), 2);

        let server = serde_json::to_value(&messages[0]).expect("json")["server"].clone();
        assert_eq!(server["current_user_permissions"], "BASE_READ | POWER_ACTIONS");
        assert_eq!(server["game"], "Minecraft");
        assert_eq!(server["net"]["domain"], "");
        assert_eq!(server["backup_quota"], 10);
    }

    #[tokio::test]
    async fn the_retrospect_of_a_running_server_is_ten_samples_and_of_a_stopped_one_is_one() {
        let fixture = fixture().await;
        let channel = fixture.operations.channel(fixture.server).await.expect("a channel");
        for index in 0..12 {
            channel.stats(StatsSample {
                cpu_percent: index as f64,
                ram_usage_bytes: 1,
                ram_total_bytes: 2,
                storage_usage_bytes: 3,
                storage_total_bytes: 4,
            });
        }
        let attachment = channel.attach();
        let object = server_object(&fixture.operations, fixture.server, Permissions::NONE)
            .await
            .expect("a server object");

        let stopped =
            ops_opening(object.clone(), attachment.state, None, &attachment.history, &attachment.stats);
        assert_eq!(count_of(&stopped, "stats"), 1);
        let sample = stopped.iter().find_map(|message| match message {
            WsMessage::Stats(sample) => Some(*sample),
            _ => None,
        });
        let sample = sample.expect("the one sample");
        assert_eq!(sample.cpu_percent, 0.0, "a dead process uses no processor");
        assert_eq!(sample.ram_usage_bytes, 0);
        assert_eq!(sample.storage_usage_bytes, 3, "the size on disk stays true");

        let running = ops_opening(
            object,
            StateReport { power_state: PowerState::Running, ..StateReport::default() },
            None,
            &attachment.history,
            &attachment.stats,
        );
        assert_eq!(count_of(&running, "stats"), 10, "ten points, oldest first");
        let first = running.iter().find_map(|message| match message {
            WsMessage::Stats(sample) => Some(sample.cpu_percent),
            _ => None,
        });
        assert_eq!(first, Some(2.0), "the two oldest of the twelve have fallen out");
    }

    #[tokio::test]
    async fn a_long_history_is_cut_into_blocks_whose_numbers_carry_on() {
        let fixture = fixture().await;
        let channel = fixture.operations.channel(fixture.server).await.expect("a channel");
        let lines: Vec<String> = (0..1200).map(|index| format!("line {index}")).collect();
        channel.console_lines(&lines);

        let attachment = channel.attach();
        let object = server_object(&fixture.operations, fixture.server, Permissions::NONE)
            .await
            .expect("a server object");
        let messages =
            ops_opening(object, attachment.state, None, &attachment.history, &attachment.stats);

        let blocks: Vec<(u64, usize)> = messages
            .iter()
            .filter_map(|message| match message {
                WsMessage::Console { seq, lines } => Some((*seq, lines.len())),
                _ => None,
            })
            .collect();
        assert_eq!(blocks, vec![(0, 500), (500, 500), (1000, 200)]);
    }

    #[test]
    fn a_block_is_cut_at_sixty_four_kibibytes_as_well_as_at_five_hundred_lines() {
        let fat: std::sync::Arc<str> = std::sync::Arc::from("x".repeat(8 * 1024));
        let history = ops::History {
            first_seq: 7,
            lines: (0..20).map(|_| std::sync::Arc::clone(&fat)).collect(),
            dropped: 0,
        };
        let cut: Vec<(u64, usize)> =
            blocks(&history).map(|(seq, lines)| (seq, lines.len())).collect();
        assert_eq!(cut, vec![(7, 8), (15, 8), (23, 4)]);
    }

    #[tokio::test]
    async fn a_client_that_stops_reading_costs_the_panel_nothing_and_is_told_of_the_gap() {
        let fixture = fixture().await;
        let channel = fixture.operations.channel(fixture.server).await.expect("a channel");
        let attachment = channel.attach();

        let (writer, mut reader) = futures::channel::mpsc::channel::<Message>(0);
        let operations = Arc::clone(&fixture.operations);
        let server = fixture.server;
        let admitted = fixture.as_owner(Permissions::from_role(ServerRole::Viewer));
        let connection = tokio::spawn(async move {
            serve(
                &operations,
                server,
                &admitted,
                attachment,
                writer,
                futures::stream::pending::<Result<Message, axum::Error>>(),
            )
            .await
        });

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let started = std::time::Instant::now();
        for index in 0..5_000 {
            channel.console_lines(&[format!("[15:04:22] line {index}")]);
        }
        let spent = started.elapsed();
        assert!(spent < std::time::Duration::from_secs(1), "the producer waited {spent:?}");

        assert!(
            channel.listeners() >= 1,
            "the connection is still attached and is simply behind"
        );

        let mut seqs = Vec::new();
        for _ in 0..40 {
            match tokio::time::timeout(std::time::Duration::from_millis(500), reader.next()).await {
                Ok(Some(Message::Text(text))) => {
                    let value: serde_json::Value =
                        serde_json::from_str(text.as_str()).expect("json");
                    if value["type"] == "console" {
                        seqs.push(value["seq"].as_u64().expect("a seq"));
                    }
                }
                Ok(Some(_)) => {}
                _ => break,
            }
        }
        assert!(!seqs.is_empty(), "the client gets going again");
        assert!(
            seqs.last().copied().unwrap_or_default() > 4_000,
            "it carries on at the front, not where it fell behind: {seqs:?}"
        );

        connection.abort();
    }

    #[tokio::test]
    async fn a_deleted_server_closes_its_sockets_with_4404() {
        let fixture = fixture().await;
        let channel = fixture.operations.channel(fixture.server).await.expect("a channel");
        let attachment = channel.attach();
        let (writer, mut reader) = futures::channel::mpsc::channel::<Message>(64);

        let operations = Arc::clone(&fixture.operations);
        let server = fixture.server;
        let admitted = fixture.as_owner(Permissions::from_role(ServerRole::Owner));
        let connection = tokio::spawn(async move {
            serve(
                &operations,
                server,
                &admitted,
                attachment,
                writer,
                futures::stream::pending::<Result<Message, axum::Error>>(),
            )
            .await
        });

        let heard = tokio::spawn(async move {
            let mut last = None;
            while let Some(message) = reader.next().await {
                last = Some(message);
            }
            last
        });

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        fixture.operations.bus().forget(fixture.server);

        let code = tokio::time::timeout(std::time::Duration::from_secs(2), connection)
            .await
            .expect("the connection ends")
            .expect("no panic");
        assert_eq!(code, 4404);

        let last = tokio::time::timeout(std::time::Duration::from_secs(2), heard)
            .await
            .expect("the reader ends")
            .expect("no panic");
        match last.expect("a last message") {
            Message::Close(Some(frame)) => assert_eq!(frame.code, 4404, "{}", frame.reason),
            other => panic!("expected a close frame, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_socket_outlives_neither_its_session_nor_its_access() {
        let fixture = fixture().await;
        let viewer = testing::a_user(&fixture.pool, PanelRole::User).await;
        testing::a_session(&fixture.pool, viewer).await;
        let (session,): (Id,) = sqlx::query_as("SELECT id FROM sessions WHERE user_id = ?")
            .bind(viewer)
            .fetch_one(&fixture.pool)
            .await
            .expect("the session");
        let member = Id::new();
        let join = |pool: SqlitePool| async move {
            sqlx::query(
                "INSERT INTO server_members (id, server_id, user_id, role, invited_at, joined_at)
                 VALUES (?, ?, ?, ?, ?, ?)",
            )
            .bind(member)
            .bind(fixture.server)
            .bind(viewer)
            .bind(ServerRole::Viewer)
            .bind(Timestamp::now())
            .bind(Timestamp::now())
            .execute(&pool)
            .await
            .expect("a membership");
        };
        join(fixture.pool.clone()).await;

        let caller =
            Caller { user_id: viewer, session_id: session, panel_role: PanelRole::User };
        assert_eq!(withdrawn(&fixture.operations, fixture.server, &caller).await, None);

        sqlx::query("DELETE FROM server_members WHERE id = ?")
            .bind(member)
            .execute(&fixture.pool)
            .await
            .expect("the access is taken away");
        assert_eq!(
            withdrawn(&fixture.operations, fixture.server, &caller).await,
            Some(4403),
            "back to the server list"
        );

        join(fixture.pool.clone()).await;
        sqlx::query("DELETE FROM sessions WHERE id = ?")
            .bind(session)
            .execute(&fixture.pool)
            .await
            .expect("signing out");
        assert_eq!(
            withdrawn(&fixture.operations, fixture.server, &caller).await,
            Some(4401),
            "signing out or changing the password sends the browser to the sign-in page"
        );
    }

    fn count_of(messages: &[WsMessage], kind: &str) -> usize {
        messages
            .iter()
            .filter(|message| {
                serde_json::to_value(message).expect("json")["type"] == kind
            })
            .count()
    }
}
