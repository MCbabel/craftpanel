use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::{Serialize, Serializer};
use tokio::sync::broadcast;

use crate::model::{
    Allocation, Id, JreVendor, PowerState, PowerTarget, Server, Timestamp,
};

use super::console::{Console, History};
use super::store::Snapshot;

pub const EVENT_BACKLOG: usize = 64;

pub const MAX_SOCKETS_PER_SESSION: usize = 4;

pub const SNAPSHOT_INTERVAL: Duration = Duration::from_secs(1);

pub const STATS_RETROSPECT: usize = 10;

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct StateReport {
    pub power_state: PowerState,
    pub target: Option<PowerTarget>,
    pub uptime_seconds: u64,
    pub exit_code: Option<i32>,
    pub oom_killed: bool,
}

impl Default for StateReport {
    fn default() -> Self {
        Self {
            power_state: PowerState::Stopped,
            target: None,
            uptime_seconds: 0,
            exit_code: None,
            oom_killed: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct StatsSample {
    pub cpu_percent: f64,
    pub ram_usage_bytes: u64,
    pub ram_total_bytes: u64,
    pub storage_usage_bytes: u64,
    pub storage_total_bytes: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ContentChangeReason {
    UpdatesChecked,
    ExternalChange,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct StartupReport {
    pub java_version: Option<u32>,
    pub jre_vendor: Option<JreVendor>,
    pub memory_mib: u32,
    pub startup_command: String,
    pub original_invocation: String,
    pub restart_required: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct NetworkReport {
    pub primary_port: u16,
    pub allocations: Vec<Allocation>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WsMessage {
    Server {
        server: Server,
    },
    State(StateReport),
    Stats(StatsSample),
    Operations(Snapshot),
    ConsoleHistoryStart {
        total_lines: usize,
        dropped_lines: u64,
    },
    Console {
        seq: u64,
        #[serde(serialize_with = "as_strings")]
        lines: Vec<Arc<str>>,
    },
    ConsoleHistoryEnd,
    ConsoleCleared,
    ContentChanged {
        reason: ContentChangeReason,
    },
    BackupListChanged,
    StartupChanged(StartupReport),
    NetworkChanged(NetworkReport),
}

fn as_strings<S: Serializer>(lines: &[Arc<str>], serializer: S) -> Result<S::Ok, S::Error> {
    serializer.collect_seq(lines.iter().map(|line| &**line))
}

#[derive(Debug, Clone)]
pub enum ServerEvent {
    Say(Arc<str>),
    Server(Arc<Server>),
    Close(u16),
}

pub struct Attachment {
    pub events: broadcast::Receiver<ServerEvent>,
    pub history: History,
    pub state: StateReport,
    pub stats: Vec<StatsSample>,
}

#[derive(Debug, Default)]
struct Throttle {
    last: Option<Instant>,
    pending: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Due {
    Now,
    After(Duration),
    Pending,
}

#[derive(Debug)]
pub struct Channel {
    events: broadcast::Sender<ServerEvent>,
    console: Mutex<Console>,
    primed: AtomicBool,
    state: Mutex<StateReport>,
    running_since: Mutex<Option<Instant>>,
    stats: Mutex<VecDeque<StatsSample>>,
    sockets: Mutex<HashMap<Id, usize>>,
    throttle: Mutex<Throttle>,
}

impl Channel {
    fn new() -> Self {
        Self {
            events: broadcast::channel(EVENT_BACKLOG).0,
            console: Mutex::new(Console::new(0)),
            primed: AtomicBool::new(false),
            state: Mutex::new(StateReport::default()),
            running_since: Mutex::new(None),
            stats: Mutex::new(VecDeque::new()),
            sockets: Mutex::new(HashMap::new()),
            throttle: Mutex::new(Throttle::default()),
        }
    }

    pub fn say(&self, message: &WsMessage) {
        let json = match serde_json::to_string(message) {
            Ok(json) => json,
            Err(err) => {
                tracing::error!("a websocket message would not serialise: {err}");
                return;
            }
        };
        let _ = self.events.send(ServerEvent::Say(Arc::from(json)));
    }

    pub fn send_server(&self, server: Arc<Server>) {
        let _ = self.events.send(ServerEvent::Server(server));
    }

    pub fn close(&self, code: u16) {
        let _ = self.events.send(ServerEvent::Close(code));
    }

    pub fn listeners(&self) -> usize {
        self.events.receiver_count()
    }

    pub fn needs_priming(&self) -> bool {
        !self.primed.load(Ordering::Acquire)
    }

    pub fn prime(&self, next_seq: u64, lines: Vec<String>) {
        let mut console = self.console.lock().expect("the console lock");
        if self.primed.swap(true, Ordering::AcqRel) {
            return;
        }
        *console = Console::new(next_seq);
        for line in lines {
            console.push(&line);
        }
    }

    pub fn console_seq(&self) -> u64 {
        self.console.lock().expect("the console lock").next_seq()
    }

    pub fn console_lines(&self, lines: &[String]) {
        if lines.is_empty() {
            return;
        }
        let mut console = self.console.lock().expect("the console lock");
        let seq = console.next_seq();
        let stored: Vec<Arc<str>> = lines.iter().map(|line| console.push(line)).collect();
        let message = WsMessage::Console { seq, lines: stored };
        self.say(&message);
    }

    pub fn clear_console(&self) {
        let mut console = self.console.lock().expect("the console lock");
        console.clear();
        self.say(&WsMessage::ConsoleCleared);
    }

    pub fn state(&self) -> StateReport {
        self.state_at(Instant::now())
    }

    fn state_at(&self, now: Instant) -> StateReport {
        let mut state = *self.state.lock().expect("the state lock");
        if let Some(since) = *self.running_since.lock().expect("the uptime lock") {
            state.uptime_seconds = now.saturating_duration_since(since).as_secs();
        }
        state
    }

    pub fn set_state(&self, state: StateReport) {
        *self.running_since.lock().expect("the uptime lock") =
            (state.power_state == PowerState::Running)
                .then(|| {
                    Instant::now().checked_sub(Duration::from_secs(state.uptime_seconds))
                })
                .flatten();

        *self.state.lock().expect("the state lock") = state;
        self.say(&WsMessage::State(state));
    }

    pub fn stats(&self, sample: StatsSample) {
        let mut stats = self.stats.lock().expect("the stats lock");
        if stats.len() == STATS_RETROSPECT {
            stats.pop_front();
        }
        stats.push_back(sample);
        drop(stats);
        self.say(&WsMessage::Stats(sample));
    }

    pub fn attach(&self) -> Attachment {
        let console = self.console.lock().expect("the console lock");
        let events = self.events.subscribe();
        let history = console.history();
        drop(console);

        Attachment {
            events,
            history,
            state: self.state(),
            stats: self.stats.lock().expect("the stats lock").iter().copied().collect(),
        }
    }

    pub fn open_socket(self: &Arc<Self>, session: Id) -> Option<SocketGuard> {
        let mut sockets = self.sockets.lock().expect("the socket lock");
        let count = sockets.entry(session).or_insert(0);
        if *count >= MAX_SOCKETS_PER_SESSION {
            return None;
        }
        *count += 1;
        Some(SocketGuard { channel: Arc::clone(self), session })
    }

    pub fn claim_snapshot(&self, urgent: bool) -> Due {
        let mut throttle = self.throttle.lock().expect("the throttle lock");
        let now = Instant::now();
        let waited = throttle.last.map(|last| now.duration_since(last));

        if urgent || waited.is_none_or(|waited| waited >= SNAPSHOT_INTERVAL) {
            throttle.last = Some(now);
            return Due::Now;
        }
        if throttle.pending {
            return Due::Pending;
        }
        throttle.pending = true;
        Due::After(SNAPSHOT_INTERVAL - waited.expect("a measured wait"))
    }

    pub fn snapshot_sent(&self) {
        let mut throttle = self.throttle.lock().expect("the throttle lock");
        throttle.pending = false;
        throttle.last = Some(Instant::now());
    }
}

pub struct SocketGuard {
    channel: Arc<Channel>,
    session: Id,
}

impl Drop for SocketGuard {
    fn drop(&mut self) {
        let mut sockets = self.channel.sockets.lock().expect("the socket lock");
        if let Some(count) = sockets.get_mut(&self.session) {
            *count -= 1;
            if *count == 0 {
                sockets.remove(&self.session);
            }
        }
    }
}

#[derive(Debug, Default)]
pub struct Bus {
    channels: Mutex<HashMap<Id, Arc<Channel>>>,
}

impl Bus {
    pub fn channel(&self, server: Id) -> Arc<Channel> {
        let mut channels = self.channels.lock().expect("the bus lock");
        Arc::clone(channels.entry(server).or_insert_with(|| Arc::new(Channel::new())))
    }

    pub fn existing(&self, server: Id) -> Option<Arc<Channel>> {
        self.channels.lock().expect("the bus lock").get(&server).cloned()
    }

    pub fn forget(&self, server: Id) {
        if let Some(channel) = self.channels.lock().expect("the bus lock").remove(&server) {
            channel.close(4404);
        }
    }

    pub fn servers(&self) -> Vec<Id> {
        self.channels.lock().expect("the bus lock").keys().copied().collect()
    }

    pub fn say(&self, server: Id, message: &WsMessage) {
        self.channel(server).say(message);
    }
}

pub fn power_state_of(state: crate::servers::RunState) -> (PowerState, bool) {
    use crate::servers::RunState;
    match state {
        RunState::Stopped | RunState::Installing | RunState::Terminated => {
            (PowerState::Stopped, false)
        }
        RunState::Starting => (PowerState::Starting, false),
        RunState::Running => (PowerState::Running, false),
        RunState::Stopping => (PowerState::Stopping, false),
        RunState::Crashed => (PowerState::Crashed, false),
        RunState::OutOfMemory => (PowerState::Crashed, true),
    }
}

pub fn seconds_since(then: Timestamp) -> u64 {
    (Timestamp::now().unix_seconds() - then.unix_seconds()).max(0) as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{BusyReasonCode, OperationKind};
    use crate::servers::RunState;

    fn json(message: &WsMessage) -> serde_json::Value {
        serde_json::to_value(message).expect("a message serialises")
    }

    #[test]
    fn all_twelve_messages_are_here_and_named_as_the_contract_names_them() {
        let messages = [
            WsMessage::Server { server: crate::ops::testing::a_server_object() },
            WsMessage::State(StateReport::default()),
            WsMessage::Stats(StatsSample {
                cpu_percent: 12.5,
                ram_usage_bytes: 1,
                ram_total_bytes: 2,
                storage_usage_bytes: 3,
                storage_total_bytes: 4,
            }),
            WsMessage::Operations(Snapshot {
                revision: 7,
                operations: Vec::new(),
                busy_reasons: vec![BusyReasonCode::Installing],
            }),
            WsMessage::ConsoleHistoryStart { total_lines: 3, dropped_lines: 0 },
            WsMessage::Console { seq: 41, lines: vec![Arc::from("[15:04:22] hello")] },
            WsMessage::ConsoleHistoryEnd,
            WsMessage::ConsoleCleared,
            WsMessage::ContentChanged { reason: ContentChangeReason::UpdatesChecked },
            WsMessage::BackupListChanged,
            WsMessage::StartupChanged(StartupReport {
                java_version: Some(21),
                jre_vendor: Some(JreVendor::Temurin),
                memory_mib: 4096,
                startup_command: "java -Xmx4096M -jar server.jar nogui".to_owned(),
                original_invocation: "java -jar server.jar".to_owned(),
                restart_required: true,
            }),
            WsMessage::NetworkChanged(NetworkReport {
                primary_port: 25565,
                allocations: vec![Allocation { port: 25566, name: "Voice".to_owned() }],
            }),
        ];

        let names: Vec<String> = messages
            .iter()
            .map(|message| json(message)["type"].as_str().expect("a type").to_owned())
            .collect();
        assert_eq!(
            names,
            [
                "server",
                "state",
                "stats",
                "operations",
                "console_history_start",
                "console",
                "console_history_end",
                "console_cleared",
                "content_changed",
                "backup_list_changed",
                "startup_changed",
                "network_changed",
            ]
        );
    }

    #[test]
    fn the_flat_messages_keep_their_fields_beside_the_type() {
        let state = json(&WsMessage::State(StateReport {
            power_state: PowerState::Crashed,
            target: None,
            uptime_seconds: 0,
            exit_code: Some(1),
            oom_killed: true,
        }));
        assert_eq!(state["power_state"], "crashed");
        assert_eq!(state["exit_code"], 1);
        assert_eq!(state["oom_killed"], true);
        assert!(state["target"].is_null());

        let console = json(&WsMessage::Console { seq: 9, lines: vec![Arc::from("a")] });
        assert_eq!(console["seq"], 9);
        assert_eq!(console["lines"], serde_json::json!(["a"]));

        let operations = json(&WsMessage::Operations(Snapshot {
            revision: 3,
            operations: Vec::new(),
            busy_reasons: vec![BusyReasonCode::BackupCreating],
        }));
        assert_eq!(operations["revision"], 3);
        assert_eq!(operations["busy_reasons"], serde_json::json!(["backup_creating"]));
        assert_eq!(operations["operations"], serde_json::json!([]));
    }

    #[test]
    fn out_of_memory_is_a_crash_that_says_how_it_died() {
        assert_eq!(power_state_of(RunState::OutOfMemory), (PowerState::Crashed, true));
        assert_eq!(power_state_of(RunState::Crashed), (PowerState::Crashed, false));
        assert_eq!(power_state_of(RunState::Installing), (PowerState::Stopped, false));
        assert_eq!(power_state_of(RunState::Running), (PowerState::Running, false));
    }

    #[test]
    fn a_reader_that_never_reads_is_dropped_lines_and_not_waited_for() {
        let bus = Bus::default();
        let server = Id::new();
        let channel = bus.channel(server);
        let mut reader = channel.attach().events;

        let started = Instant::now();
        for index in 0..EVENT_BACKLOG * 4 {
            channel.console_lines(&[format!("line {index}")]);
        }
        let spent = started.elapsed();
        assert!(spent < Duration::from_millis(200), "sending took {spent:?}");

        let missed = match reader.try_recv() {
            Err(broadcast::error::TryRecvError::Lagged(missed)) => missed,
            other => panic!("expected a lag, got {other:?}"),
        };
        assert_eq!(missed as usize, EVENT_BACKLOG * 4 - EVENT_BACKLOG);

        let mut held = 0;
        while reader.try_recv().is_ok() {
            held += 1;
        }
        assert_eq!(held, EVENT_BACKLOG, "the channel holds a fixed number of events and no more");
    }

    #[test]
    fn a_lagging_reader_sees_the_gap_in_the_line_numbers() {
        let bus = Bus::default();
        let channel = bus.channel(Id::new());
        let mut reader = channel.attach().events;

        for index in 0..EVENT_BACKLOG + 10 {
            channel.console_lines(&[format!("line {index}")]);
        }
        assert!(matches!(reader.try_recv(), Err(broadcast::error::TryRecvError::Lagged(_))));

        let first = match reader.try_recv().expect("a message after the lag") {
            ServerEvent::Say(json) => json,
            other => panic!("expected a message, got {other:?}"),
        };
        let value: serde_json::Value = serde_json::from_str(&first).expect("json");
        assert_eq!(value["type"], "console");
        assert_eq!(value["seq"], 10);
    }

    #[test]
    fn the_retrospect_and_the_live_stream_do_not_overlap() {
        let bus = Bus::default();
        let channel = bus.channel(Id::new());
        channel.console_lines(&["before".to_owned()]);

        let attachment = channel.attach();
        channel.console_lines(&["after".to_owned()]);
        let mut reader = attachment.events;

        assert_eq!(attachment.history.lines.iter().map(|l| &**l).collect::<Vec<_>>(), ["before"]);
        let live = match reader.try_recv().expect("the line written after attaching") {
            ServerEvent::Say(json) => json,
            other => panic!("expected a message, got {other:?}"),
        };
        assert!(live.contains("after"), "{live}");
        assert!(!live.contains("before"), "a line must not arrive twice: {live}");
        assert!(reader.try_recv().is_err(), "nothing else was written");
    }

    #[test]
    fn attaching_while_lines_arrive_loses_none_and_repeats_none() {
        let bus = Bus::default();
        let channel = bus.channel(Id::new());

        let writing = Arc::clone(&channel);
        let writer = std::thread::spawn(move || {
            for index in 0..50 {
                writing.console_lines(&[format!("line {index}")]);
                std::thread::sleep(Duration::from_micros(200));
            }
        });
        std::thread::sleep(Duration::from_millis(3));
        let attachment = channel.attach();
        writer.join().expect("the writer finishes");

        let mut seen: Vec<String> =
            attachment.history.lines.iter().map(|line| line.to_string()).collect();
        let mut events = attachment.events;
        while let Ok(ServerEvent::Say(json)) = events.try_recv() {
            let value: serde_json::Value = serde_json::from_str(&json).expect("json");
            if value["type"] == "console" {
                for line in value["lines"].as_array().expect("lines") {
                    seen.push(line.as_str().expect("a line").to_owned());
                }
            }
        }

        let expected: Vec<String> = (0..50).map(|index| format!("line {index}")).collect();
        assert_eq!(seen, expected);
    }

    #[test]
    fn the_fifth_socket_of_one_session_is_refused() {
        let bus = Bus::default();
        let channel = bus.channel(Id::new());
        let session = Id::new();
        let other = Id::new();

        let held: Vec<_> =
            (0..MAX_SOCKETS_PER_SESSION).map(|_| channel.open_socket(session).expect("a socket")).collect();
        assert!(channel.open_socket(session).is_none());
        assert!(channel.open_socket(other).is_some());

        drop(held);
        assert!(channel.open_socket(session).is_some(), "closing one makes room again");
    }

    #[test]
    fn progress_waits_a_second_and_a_state_change_does_not() {
        let bus = Bus::default();
        let channel = bus.channel(Id::new());

        assert_eq!(channel.claim_snapshot(false), Due::Now);
        assert!(matches!(channel.claim_snapshot(false), Due::After(_)));
        assert_eq!(channel.claim_snapshot(false), Due::Pending);
        assert_eq!(channel.claim_snapshot(true), Due::Now, "a state change never waits");
    }

    #[test]
    fn the_uptime_of_a_running_server_grows_between_two_readers() {
        let bus = Bus::default();
        let channel = bus.channel(Id::new());

        channel.set_state(StateReport {
            power_state: PowerState::Running,
            uptime_seconds: 100,
            ..StateReport::default()
        });

        let now = Instant::now();
        assert_eq!(channel.state_at(now).uptime_seconds, 100, "the first reader sees what came in");
        assert_eq!(
            channel.state_at(now + Duration::from_secs(60)).uptime_seconds,
            160,
            "a minute later the same report is worth a minute more"
        );
    }

    #[test]
    fn a_stopped_server_has_no_uptime_to_count() {
        let bus = Bus::default();
        let channel = bus.channel(Id::new());

        channel.set_state(StateReport {
            power_state: PowerState::Running,
            uptime_seconds: 100,
            ..StateReport::default()
        });
        channel.set_state(StateReport::default());

        let late = Instant::now() + Duration::from_secs(600);
        assert_eq!(channel.state_at(late).uptime_seconds, 0);
    }

    #[test]
    fn only_the_kinds_the_table_names_carry_a_lock() {
        assert_eq!(OperationKind::Unarchive.busy_reason(), None);
        assert_eq!(OperationKind::ServerCreate.busy_reason(), Some(BusyReasonCode::Installing));
    }
}
