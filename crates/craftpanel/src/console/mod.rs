pub mod logs;
pub mod mclogs;

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::http::StatusCode;

use crate::auth::error::{Failure, Result};
use crate::model::Id;

pub const MAX_COMMAND_BYTES: usize = 8 * 1024;
const BURST: usize = 20;
const WINDOW: Duration = Duration::from_secs(10);

pub const PANEL_TAG: &str = "Panel";

pub struct Console {
    pub analyst: mclogs::Mclogs,
    commands: Mutex<HashMap<(Id, Id), VecDeque<Instant>>>,
    turns: Mutex<HashMap<Id, Arc<tokio::sync::Mutex<()>>>>,
}

impl Console {
    pub fn new() -> Self {
        Self::with_analyst(mclogs::Mclogs::new())
    }

    pub fn with_analyst(analyst: mclogs::Mclogs) -> Self {
        Self {
            analyst,
            commands: Mutex::new(HashMap::new()),
            turns: Mutex::new(HashMap::new()),
        }
    }

    pub fn accept(&self, user: Id, server: Id, now: Instant) -> Option<u64> {
        let mut commands = self.commands.lock().expect("the console brake");
        commands.retain(|_, times| {
            times.back().is_some_and(|last| now.duration_since(*last) < WINDOW)
        });

        let times = commands.entry((user, server)).or_default();
        while times.front().is_some_and(|first| now.duration_since(*first) >= WINDOW) {
            times.pop_front();
        }
        if let Some(oldest) = (times.len() >= BURST).then(|| times.front().copied()).flatten() {
            let left = WINDOW - now.duration_since(oldest);
            return Some(left.as_secs() + u64::from(left.subsec_nanos() > 0));
        }

        times.push_back(now);
        None
    }

    pub fn turn(&self, server: Id) -> Arc<tokio::sync::Mutex<()>> {
        Arc::clone(
            self.turns.lock().expect("the console lock").entry(server).or_default(),
        )
    }
}

impl Default for Console {
    fn default() -> Self {
        Self::new()
    }
}

pub fn check_command(command: &str) -> Result<()> {
    if command.trim().is_empty() {
        return Err(refuse("command_empty", "there is no command here"));
    }
    if command.len() > MAX_COMMAND_BYTES {
        return Err(refuse("command_too_long", "this command is longer than 8192 bytes"));
    }
    if command.chars().any(char::is_control) {
        return Err(refuse("command_invalid", "a command is one line without control characters"));
    }
    Ok(())
}

pub fn echo(command: &str) -> String {
    format!("{} [{PANEL_TAG}/INFO]: > {command}", clock())
}

fn clock() -> String {
    let seconds = crate::model::Timestamp::now().unix_seconds() as libc::time_t;
    let mut parts: libc::tm = unsafe { std::mem::zeroed() };
    unsafe { libc::localtime_r(&seconds, &mut parts) };
    format!("[{:02}:{:02}:{:02}]", parts.tm_hour, parts.tm_min, parts.tm_sec)
}

fn refuse(code: &'static str, message: &'static str) -> Failure {
    Failure::new(StatusCode::UNPROCESSABLE_ENTITY, code, message)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_command_is_one_line_of_at_most_eight_kibibytes() {
        assert!(check_command("say hello").is_ok());
        assert!(check_command("/give @a diamond 64").is_ok());

        assert_eq!(check_command("").unwrap_err().code(), "command_empty");
        assert_eq!(check_command("   \t ").unwrap_err().code(), "command_empty");
        assert_eq!(check_command(&"a".repeat(8193)).unwrap_err().code(), "command_too_long");
        assert!(check_command(&"a".repeat(8192)).is_ok());

        for smuggled in ["say hi\nstop", "say hi\rstop", "say \u{0} hi", "say\u{1b}[0m hi"] {
            let refused = check_command(smuggled).unwrap_err();
            assert_eq!(refused.code(), "command_invalid", "{smuggled}");
        }
    }

    #[test]
    fn the_echo_starts_with_the_clock_and_nothing_else() {
        let line = echo("say hello");
        assert!(line.ends_with(" [Panel/INFO]: > say hello"), "{line}");

        let front: Vec<char> = line.chars().take(10).collect();
        assert_eq!(front[0], '[');
        assert_eq!(front[3], ':');
        assert_eq!(front[6], ':');
        assert_eq!(front[9], ']');
        assert!(front[1..3].iter().all(char::is_ascii_digit), "{line}");
    }

    #[test]
    fn twenty_commands_pass_and_the_twenty_first_waits() {
        let console = Console::new();
        let user = Id::new();
        let server = Id::new();
        let start = Instant::now();

        for number in 0..BURST {
            assert_eq!(console.accept(user, server, start), None, "command {number}");
        }
        let wait = console.accept(user, server, start).expect("the twenty-first waits");
        assert!((1..=10).contains(&wait), "Retry-After was {wait}");

        assert_eq!(console.accept(user, Id::new(), start), None);
        assert_eq!(console.accept(Id::new(), server, start), None);

        let later = start + WINDOW + Duration::from_millis(1);
        assert_eq!(console.accept(user, server, later), None, "the window moved on");
        assert_eq!(console.commands.lock().unwrap().len(), 1, "and the stale keys went with it");
    }
}
