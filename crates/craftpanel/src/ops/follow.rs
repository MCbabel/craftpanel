use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::broadcast::error::RecvError;
use tokio::task::JoinHandle;

use crate::model::{Id, Timestamp};
use crate::servers::{Hub, Link};

use super::events::{power_state_of, StateReport};
use super::Operations;

const LOOK_FOR_SUPERVISORS: Duration = Duration::from_millis(50);
const BUNDLE_EVERY: Duration = Duration::from_millis(100);
const BUNDLE_LINES: usize = 500;
const BUNDLE_BYTES: usize = 64 * 1024;

pub async fn follow(operations: Arc<Operations>, hub: Arc<Hub>) {
    let mut attending: HashMap<Id, JoinHandle<()>> = HashMap::new();
    let mut last_pid: HashMap<Id, u32> = HashMap::new();

    loop {
        attending.retain(|_, task| !task.is_finished());

        for name in hub.attached().await {
            let Ok(server) = name.parse::<Id>() else {
                tracing::warn!("a supervisor calls itself {name:?}, which is no server id");
                continue;
            };
            if attending.contains_key(&server) {
                continue;
            }
            let Some(link) = hub.link(&name).await else { continue };

            let fresh = last_pid.insert(server, link.pid).is_some_and(|pid| pid != link.pid);
            attending.insert(
                server,
                tokio::spawn(attend(Arc::clone(&operations), Arc::clone(&hub), server, link, fresh)),
            );
        }

        tokio::time::sleep(LOOK_FOR_SUPERVISORS).await;
    }
}

async fn attend(
    operations: Arc<Operations>,
    hub: Arc<Hub>,
    server: Id,
    link: Arc<Link>,
    fresh_process: bool,
) {
    let Ok(channel) = operations.channel(server).await else {
        tracing::warn!(%server, "a supervisor attached for a server we do not know");
        return;
    };
    if fresh_process {
        channel.clear_console();
    }

    let mut console = link.subscribe();
    let mut state = link.state().await;
    let mut running_since = anchor(&operations, server, state).await;
    channel.set_state(report(state, running_since));

    let mut bundle = Bundle::default();
    let mut tick = tokio::time::interval(BUNDLE_EVERY);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    loop {
        tokio::select! {
            line = console.recv() => match line {
                Ok(line) => {
                    if bundle.take(line.line) {
                        channel.console_lines(&bundle.flush());
                    }
                }
                Err(RecvError::Lagged(missed)) => {
                    tracing::warn!(%server, missed, "console lines were dropped on the way in");
                }
                Err(RecvError::Closed) => break,
            },
            _ = tick.tick() => {
                channel.console_lines(&bundle.flush());

                let now = link.state().await;
                if now != state {
                    if now == crate::servers::RunState::Running {
                        running_since = Instant::now();
                    }
                    state = now;
                    channel.set_state(report(state, running_since));
                }

                match hub.link(&server.to_string()).await {
                    Some(current) if Arc::ptr_eq(&current, &link) => {}
                    _ => break,
                }
            }
        }
    }

    channel.console_lines(&bundle.flush());
    if state.is_live() {
        channel.console_lines(&["[Panel/ERROR]: lost contact with the supervisor".to_owned()]);
        channel.set_state(StateReport::default());
    }
}

async fn anchor(operations: &Operations, server: Id, state: crate::servers::RunState) -> Instant {
    let now = Instant::now();
    if state != crate::servers::RunState::Running {
        return now;
    }

    let since: Option<Option<Timestamp>> =
        sqlx::query_scalar("SELECT running_since FROM servers WHERE id = ?")
            .bind(server)
            .fetch_optional(operations.pool())
            .await
            .ok()
            .flatten();
    let Some(since) = since.flatten() else { return now };

    let seconds = (Timestamp::now().unix_seconds() - since.unix_seconds()).max(0) as u64;
    now.checked_sub(Duration::from_secs(seconds)).unwrap_or(now)
}

fn report(state: crate::servers::RunState, running_since: Instant) -> StateReport {
    let (power_state, oom_killed) = power_state_of(state);
    StateReport {
        power_state,
        target: None,
        uptime_seconds: match power_state {
            crate::model::PowerState::Running => running_since.elapsed().as_secs(),
            _ => 0,
        },
        exit_code: None,
        oom_killed,
    }
}

#[derive(Debug, Default)]
struct Bundle {
    lines: Vec<String>,
    bytes: usize,
}

impl Bundle {
    fn take(&mut self, line: String) -> bool {
        self.bytes += line.len();
        self.lines.push(line);
        self.lines.len() >= BUNDLE_LINES || self.bytes >= BUNDLE_BYTES
    }

    fn flush(&mut self) -> Vec<String> {
        self.bytes = 0;
        std::mem::take(&mut self.lines)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{PanelRole, PowerState};
    use crate::ops::events::ServerEvent;
    use crate::ops::testing;
    use craftpanel_proto::{OutputStream, SupervisorMessage};
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    #[test]
    fn a_bundle_is_sent_when_it_is_full_and_kept_when_it_is_not() {
        let mut bundle = Bundle::default();
        assert!(!bundle.take("short".to_owned()));
        assert!(bundle.take("x".repeat(BUNDLE_BYTES)));
        assert_eq!(bundle.flush().len(), 2);
        assert!(bundle.flush().is_empty());

        let mut counted = Bundle::default();
        for index in 0..BUNDLE_LINES - 1 {
            assert!(!counted.take(format!("line {index}")));
        }
        assert!(counted.take("the five hundredth".to_owned()));
    }

    #[tokio::test]
    async fn lines_from_the_hub_leave_as_console_and_the_end_of_a_process_as_state() {
        let (operations, dir, pool) = testing::operations().await;
        let owner = testing::a_user(&pool, PanelRole::User).await;
        let server = testing::a_server(&pool, owner).await;

        let hub = Arc::new(crate::servers::Hub::new(dir.path().join("supervisors.sock")));
        hub.set_token(server.to_string(), "a-token").await;
        let listening = tokio::spawn(Arc::clone(&hub).listen());
        let following = tokio::spawn(follow(Arc::clone(&operations), Arc::clone(&hub)));

        let mut events = operations.channel(server).await.expect("a channel").attach().events;

        let stream = connect(hub.socket()).await;
        let (reader, mut writer) = stream.into_split();
        let mut reader = BufReader::new(reader).lines();

        say(
            &mut writer,
            &SupervisorMessage::Hello {
                server_id: server.to_string(),
                token: "a-token".to_owned(),
                pid: 4242,
                protocol: craftpanel_proto::HELPER_PROTOCOL_VERSION,
            },
        )
        .await;
        let greeting = reader.next_line().await.expect("a line").expect("the hub answers");
        assert!(greeting.contains("accepted"), "{greeting}");

        let running = next_message(&mut events, "state").await;
        assert_eq!(running["power_state"], "running");

        say(
            &mut writer,
            &SupervisorMessage::Output {
                seq: 1,
                line: "\u{1b}[32m[15:04:22] [Server thread/INFO]: Done\u{1b}[0m".to_owned(),
                stream: OutputStream::Stdout,
            },
        )
        .await;

        let console = next_message(&mut events, "console").await;
        assert_eq!(console["lines"], serde_json::json!(["[15:04:22] [Server thread/INFO]: Done"]));
        assert_eq!(console["seq"], 0);

        say(&mut writer, &SupervisorMessage::Exited { code: Some(1), signal: None, oom_killed: false })
            .await;

        let state = next_message(&mut events, "state").await;
        assert_eq!(state["power_state"], "crashed");
        assert_eq!(state["oom_killed"], false);

        following.abort();
        listening.abort();
    }

    #[tokio::test]
    async fn a_game_that_saved_and_left_on_a_sigterm_is_not_reported_as_a_crash() {
        let (operations, dir, pool) = testing::operations().await;
        let owner = testing::a_user(&pool, PanelRole::User).await;
        let server = testing::a_server(&pool, owner).await;

        let hub = Arc::new(crate::servers::Hub::new(dir.path().join("supervisors.sock")));
        hub.set_token(server.to_string(), "a-token").await;
        let listening = tokio::spawn(Arc::clone(&hub).listen());
        let following = tokio::spawn(follow(Arc::clone(&operations), Arc::clone(&hub)));

        let mut events = operations.channel(server).await.expect("a channel").attach().events;

        let stream = connect(hub.socket()).await;
        let (reader, mut writer) = stream.into_split();
        let mut reader = BufReader::new(reader).lines();

        say(
            &mut writer,
            &SupervisorMessage::Hello {
                server_id: server.to_string(),
                token: "a-token".to_owned(),
                pid: 4242,
                protocol: craftpanel_proto::HELPER_PROTOCOL_VERSION,
            },
        )
        .await;
        let greeting = reader.next_line().await.expect("a line").expect("the hub answers");
        assert!(greeting.contains("accepted"), "{greeting}");
        assert_eq!(next_message(&mut events, "state").await["power_state"], "running");

        say(
            &mut writer,
            &SupervisorMessage::Exited { code: Some(143), signal: None, oom_killed: false },
        )
        .await;

        let state = next_message(&mut events, "state").await;
        assert_eq!(state["power_state"], "stopped", "the installer asked for this ending");
        assert_eq!(state["oom_killed"], false);

        following.abort();
        listening.abort();
    }

    #[tokio::test]
    async fn the_first_state_of_a_server_nobody_supervises_is_stopped() {
        let (operations, _dir, pool) = testing::operations().await;
        let owner = testing::a_user(&pool, PanelRole::User).await;
        let server = testing::a_server(&pool, owner).await;

        let channel = operations.channel(server).await.expect("a channel");
        assert_eq!(channel.state().power_state, PowerState::Stopped);
        assert_eq!(channel.state().uptime_seconds, 0);
    }

    async fn connect(socket: &std::path::Path) -> tokio::net::UnixStream {
        for _ in 0..100 {
            if let Ok(stream) = tokio::net::UnixStream::connect(socket).await {
                return stream;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("the hub never came up on {}", socket.display());
    }

    async fn say(writer: &mut tokio::net::unix::OwnedWriteHalf, message: &SupervisorMessage) {
        let mut line = serde_json::to_vec(message).expect("a message");
        line.push(b'\n');
        writer.write_all(&line).await.expect("the hub takes it");
        writer.flush().await.expect("flushed");
    }

    async fn next_message(
        events: &mut tokio::sync::broadcast::Receiver<ServerEvent>,
        kind: &str,
    ) -> serde_json::Value {
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            let Ok(Ok(event)) =
                tokio::time::timeout(Duration::from_secs(1), events.recv()).await
            else {
                continue;
            };
            if let ServerEvent::Say(json) = event {
                let value: serde_json::Value = serde_json::from_str(&json).expect("json");
                if value["type"] == kind {
                    return value;
                }
            }
        }
        panic!("no {kind} message arrived");
    }
}
