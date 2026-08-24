use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunState {
    Stopped,
    Installing,
    Starting,
    Running,
    Stopping,
    Terminated,
    Crashed,
    OutOfMemory,
}

impl RunState {
    pub fn after_exit(code: Option<i32>, signal: Option<i32>, oom_killed: bool) -> Self {
        if oom_killed {
            Self::OutOfMemory
        } else if code == Some(0) {
            Self::Stopped
        } else if signal == Some(libc::SIGTERM) || code == Some(128 + libc::SIGTERM) {
            Self::Terminated
        } else {
            Self::Crashed
        }
    }

    pub fn is_live(self) -> bool {
        matches!(self, Self::Installing | Self::Starting | Self::Running | Self::Stopping)
    }

    pub fn can_become(self, next: Self) -> bool {
        use RunState::*;
        match (self, next) {
            (a, b) if a == b => false,
            (Stopped | Terminated | Crashed | OutOfMemory, Installing | Starting) => true,
            (Installing, Starting | Stopped | Crashed) => true,
            (Starting, Running | Stopping | Terminated | Crashed | OutOfMemory | Stopped) => true,
            (Running, Stopping | Terminated | Crashed | OutOfMemory | Stopped) => true,
            (Stopping, Stopped | Terminated | Crashed | OutOfMemory) => true,
            _ => false,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ConsoleLine {
    pub seq: u64,
    pub line: String,
    pub stderr: bool,
}

#[cfg(test)]
mod tests {
    use super::RunState;
    use super::RunState::*;

    #[test]
    fn a_stopped_server_cannot_report_itself_stopping() {
        assert!(!Stopped.can_become(Stopping));
        assert!(!Stopped.can_become(Running));
    }

    #[test]
    fn the_ordinary_run_is_allowed_end_to_end() {
        assert!(Stopped.can_become(Installing));
        assert!(Installing.can_become(Starting));
        assert!(Starting.can_become(Running));
        assert!(Running.can_become(Stopping));
        assert!(Stopping.can_become(Stopped));
    }

    #[test]
    fn a_crash_may_interrupt_any_live_state() {
        for from in [Starting, Running, Stopping] {
            assert!(from.can_become(Crashed), "{from:?} should be able to crash");
            assert!(from.can_become(OutOfMemory), "{from:?} should be able to hit the ceiling");
        }
    }

    #[test]
    fn an_ending_the_process_ran_itself_on_a_sigterm_is_no_crash() {
        assert_eq!(RunState::after_exit(Some(0), None, false), Stopped);
        assert_eq!(
            RunState::after_exit(Some(143), None, false),
            Terminated,
            "the shutdown hook ran and the jvm left with 128 + SIGTERM"
        );
        assert_eq!(
            RunState::after_exit(None, Some(15), false),
            Terminated,
            "and one that carries no handler is ended by the same order"
        );
    }

    #[test]
    fn everything_else_that_ends_badly_is_still_a_crash() {
        assert_eq!(RunState::after_exit(Some(1), None, false), Crashed);
        assert_eq!(RunState::after_exit(Some(127), None, false), Crashed, "no java on the path");
        assert_eq!(RunState::after_exit(None, Some(6), false), Crashed, "the jvm aborted");
        assert_eq!(RunState::after_exit(None, Some(9), false), Crashed, "a SIGKILL from outside");
        assert_eq!(RunState::after_exit(Some(137), None, false), Crashed, "and one a wrapper saw");
        assert_eq!(RunState::after_exit(None, Some(9), true), OutOfMemory, "the memory ceiling");
    }

    #[test]
    fn a_state_never_becomes_itself() {
        for state in
            [Stopped, Installing, Starting, Running, Stopping, Terminated, Crashed, OutOfMemory]
        {
            assert!(!state.can_become(state));
        }
    }

    #[test]
    fn a_dead_server_can_only_start_again() {
        for dead in [Stopped, Terminated, Crashed, OutOfMemory] {
            assert!(dead.can_become(Starting));
            assert!(!dead.can_become(Running));
            assert!(!dead.can_become(Stopping));
        }
    }
}
