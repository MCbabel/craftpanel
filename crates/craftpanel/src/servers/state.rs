use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunState {
    Stopped,
    Installing,
    Starting,
    Running,
    Stopping,
    Crashed,
    OutOfMemory,
}

impl RunState {
    pub fn is_live(self) -> bool {
        matches!(self, Self::Installing | Self::Starting | Self::Running | Self::Stopping)
    }

    pub fn can_become(self, next: Self) -> bool {
        use RunState::*;
        match (self, next) {
            (a, b) if a == b => false,
            (Stopped | Crashed | OutOfMemory, Installing | Starting) => true,
            (Installing, Starting | Stopped | Crashed) => true,
            (Starting, Running | Stopping | Crashed | OutOfMemory | Stopped) => true,
            (Running, Stopping | Crashed | OutOfMemory | Stopped) => true,
            (Stopping, Stopped | Crashed | OutOfMemory) => true,
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
    fn a_state_never_becomes_itself() {
        for state in [Stopped, Installing, Starting, Running, Stopping, Crashed, OutOfMemory] {
            assert!(!state.can_become(state));
        }
    }

    #[test]
    fn a_dead_server_can_only_start_again() {
        for dead in [Stopped, Crashed, OutOfMemory] {
            assert!(dead.can_become(Starting));
            assert!(!dead.can_become(Running));
            assert!(!dead.can_become(Stopping));
        }
    }
}
