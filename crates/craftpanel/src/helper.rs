use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use craftpanel_proto::{HelperOk, HelperRequest, HelperResponse, ResourceLimits, SpawnRequest};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

#[derive(Clone)]
pub struct Helper {
    socket: PathBuf,
}

impl Helper {
    pub fn new(socket: impl Into<PathBuf>) -> Self {
        Self { socket: socket.into() }
    }

    pub async fn ping(&self) -> Result<u32> {
        match self.call(HelperRequest::Ping).await? {
            HelperOk::Pong { version } => Ok(version),
            other => bail!("helper answered a ping with {other:?}"),
        }
    }

    pub async fn create_user(&self, user_id: &str) -> Result<u32> {
        let request = HelperRequest::CreateUser { user_id: user_id.to_owned() };
        match self.call(request).await? {
            HelperOk::UserCreated { uid, .. } => Ok(uid),
            other => bail!("unexpected answer to create_user: {other:?}"),
        }
    }

    pub async fn delete_user(&self, user_id: &str, remove_home: bool) -> Result<()> {
        let request = HelperRequest::DeleteUser { user_id: user_id.to_owned(), remove_home };
        match self.call(request).await? {
            HelperOk::UserDeleted => Ok(()),
            other => bail!("unexpected answer to delete_user: {other:?}"),
        }
    }

    pub async fn apply_limits(&self, user_id: &str, limits: ResourceLimits) -> Result<()> {
        let request = HelperRequest::ApplyLimits { user_id: user_id.to_owned(), limits };
        match self.call(request).await? {
            HelperOk::LimitsApplied => Ok(()),
            other => bail!("unexpected answer to apply_limits: {other:?}"),
        }
    }

    pub async fn chown_tree(&self, user_id: &str, steps: Vec<String>) -> Result<u64> {
        let request = HelperRequest::ChownTree { user_id: user_id.to_owned(), steps };
        match self.call(request).await? {
            HelperOk::TreeChowned { entries } => Ok(entries),
            other => bail!("unexpected answer to chown_tree: {other:?}"),
        }
    }

    pub async fn spawn(&self, request: SpawnRequest) -> Result<u32> {
        match self.call(HelperRequest::Spawn(request)).await? {
            HelperOk::Spawned { pid } => Ok(pid),
            other => bail!("unexpected answer to spawn: {other:?}"),
        }
    }

    async fn call(&self, request: HelperRequest) -> Result<HelperOk> {
        let stream = UnixStream::connect(&self.socket).await.with_context(|| {
            format!("the privileged helper is not answering on {}", self.socket.display())
        })?;
        let (reader, mut writer) = stream.into_split();

        let mut encoded = serde_json::to_vec(&request)?;
        encoded.push(b'\n');
        writer.write_all(&encoded).await?;
        writer.flush().await?;

        let mut line = String::new();
        BufReader::new(reader).read_line(&mut line).await?;
        if line.trim().is_empty() {
            bail!("the helper closed the connection without answering");
        }

        match serde_json::from_str::<HelperResponse>(&line)? {
            HelperResponse::Ok(ok) => Ok(ok),
            HelperResponse::Error { code, message } => {
                bail!("helper refused ({code:?}): {message}")
            }
        }
    }
}

pub fn all_servers() -> Vec<String> {
    vec![craftpanel_proto::SERVERS.to_owned()]
}

pub fn in_servers(name: impl std::fmt::Display) -> Vec<String> {
    let mut steps = all_servers();
    steps.push(name.to_string());
    steps
}

pub fn below_server(server: impl std::fmt::Display, rel: &[String]) -> Vec<String> {
    let mut steps = in_servers(server);
    steps.extend(rel.iter().cloned());
    steps
}
