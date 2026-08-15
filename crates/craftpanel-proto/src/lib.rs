use std::path::PathBuf;

use serde::{Deserialize, Serialize};

pub const HELPER_PROTOCOL_VERSION: u32 = 2;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum HelperRequest {
    Ping,
    CreateUser { user_id: String },
    DeleteUser { user_id: String, remove_home: bool },
    ApplyLimits { user_id: String, limits: ResourceLimits },
    Spawn(SpawnRequest),
    ChownTree { user_id: String, steps: Vec<String> },
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct ResourceLimits {
    pub memory_high_bytes: Option<u64>,
    pub memory_max_bytes: Option<u64>,
    pub cpu_quota_percent: Option<u32>,
    pub pids_max: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpawnRequest {
    pub user_id: String,
    pub server_id: String,
    pub working_dir: Vec<String>,
    pub program: PathBuf,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    pub supervisor_socket: PathBuf,
    pub token: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum HelperResponse {
    Ok(HelperOk),
    Error { code: HelperErrorCode, message: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum HelperOk {
    Pong { version: u32 },
    UserCreated { uid: u32, gid: u32, home: PathBuf },
    UserDeleted,
    LimitsApplied,
    Spawned { pid: u32 },
    TreeChowned { entries: u64 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HelperErrorCode {
    MalformedRequest,
    InvalidUserId,
    UnknownUser,
    UserExists,
    PathOutsideRoot,
    SpawnFailed,
    CgroupFailed,
    Internal,
}

pub fn is_valid_user_id(id: &str) -> bool {
    id.len() == 26 && id.bytes().all(|b| b.is_ascii_digit() || b.is_ascii_uppercase())
}

pub fn system_username(user_id: &str) -> String {
    format!("craft-{}", user_id.to_ascii_lowercase())
}

pub const SERVERS: &str = "servers";

pub fn is_valid_step(step: &str) -> bool {
    !step.is_empty()
        && step != "."
        && step != ".."
        && step.len() <= 255
        && !step.contains('/')
        && !step.contains('\0')
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SupervisorMessage {
    Hello { server_id: String, token: String, pid: u32, protocol: u32 },
    Started { pid: u32 },
    Output { seq: u64, line: String, stream: OutputStream },
    Exited { code: Option<i32>, signal: Option<i32>, oom_killed: bool },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutputStream {
    Stdout,
    Stderr,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PanelMessage {
    Accepted,
    Rejected { reason: String },
    Stdin { line: String },
    Stop { command: Option<String>, grace_seconds: u32 },
    Kill,
}

#[cfg(test)]
mod tests {
    use super::*;

    const LINUX_USERNAME_MAX: usize = 32;

    #[test]
    fn rejects_user_ids_that_are_not_ulids() {
        assert!(is_valid_user_id("01ARZ3NDEKTSV4RRFFQ69G5FAV"));
        assert!(!is_valid_user_id("../../etc/passwd"));
        assert!(!is_valid_user_id("01arz3ndektsv4rrffq69g5fav"));
        assert!(!is_valid_user_id(""));
        assert!(!is_valid_user_id("01ARZ3NDEKTSV4RRFFQ69G5FA"));
        assert!(!is_valid_user_id("01ARZ3NDEKTSV4RRFFQ69G5FAV "));
        assert!(!is_valid_user_id("01ARZ3NDEKTSV4RRFFQ69G5FA;"));
    }

    #[test]
    fn a_step_is_one_name_and_never_a_path() {
        assert!(is_valid_step("servers"));
        assert!(is_valid_step("01ARZ3NDEKTSV4RRFFQ69G5FAV.restoring-01J"));
        assert!(is_valid_step("..hidden"));
        assert!(!is_valid_step(""));
        assert!(!is_valid_step("."));
        assert!(!is_valid_step(".."));
        assert!(!is_valid_step("/"));
        assert!(!is_valid_step("/etc"));
        assert!(!is_valid_step("servers/one"));
        assert!(!is_valid_step("..\0"));
        assert!(!is_valid_step(&"a".repeat(256)));
    }

    #[test]
    fn system_usernames_fit_linux_limits() {
        let name = system_username("01ARZ3NDEKTSV4RRFFQ69G5FAV");
        assert_eq!(name, "craft-01arz3ndektsv4rrffq69g5fav");
        assert!(name.len() <= LINUX_USERNAME_MAX);
    }

    #[test]
    fn the_prefix_uses_up_the_last_free_character() {
        let longest = "Z".repeat(26);
        assert!(is_valid_user_id(&longest));
        assert_eq!(system_username(&longest).len(), LINUX_USERNAME_MAX);
    }
}
