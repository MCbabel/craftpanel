use std::collections::HashMap;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use craftpanel_proto::{OutputStream, PanelMessage, SupervisorMessage};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{broadcast, mpsc, RwLock};

use super::state::{ConsoleLine, RunState};

const CONSOLE_BACKLOG: usize = 512;
const OUTBOX_DEPTH: usize = 256;

pub struct Hub {
    socket: PathBuf,
    tokens: RwLock<HashMap<String, String>>,
    links: RwLock<HashMap<String, Arc<Link>>>,
}

pub struct Link {
    pub server_id: String,
    pub pid: u32,
    outbox: mpsc::Sender<PanelMessage>,
    console: broadcast::Sender<ConsoleLine>,
    state: RwLock<RunState>,
}

impl Link {
    pub async fn state(&self) -> RunState {
        *self.state.read().await
    }

    pub fn subscribe(&self) -> broadcast::Receiver<ConsoleLine> {
        self.console.subscribe()
    }

    pub async fn send_command(&self, line: impl Into<String>) -> Result<()> {
        self.outbox
            .send(PanelMessage::Stdin { line: line.into() })
            .await
            .context("the supervisor is no longer listening")
    }

    pub async fn request_stop(&self, command: Option<String>, grace_seconds: u32) -> Result<()> {
        self.outbox
            .send(PanelMessage::Stop { command, grace_seconds })
            .await
            .context("the supervisor is no longer listening")
    }

    pub async fn kill(&self) -> Result<()> {
        self.outbox.send(PanelMessage::Kill).await.context("the supervisor is already gone")
    }
}

impl Hub {
    pub fn new(socket: impl Into<PathBuf>) -> Self {
        Self {
            socket: socket.into(),
            tokens: RwLock::new(HashMap::new()),
            links: RwLock::new(HashMap::new()),
        }
    }

    pub fn socket(&self) -> &Path {
        &self.socket
    }

    pub async fn load_tokens(&self, known: impl IntoIterator<Item = (String, String)>) {
        let mut tokens = self.tokens.write().await;
        for (server_id, token) in known {
            tokens.insert(server_id, token);
        }
    }

    pub async fn set_token(&self, server_id: impl Into<String>, token: impl Into<String>) {
        self.tokens.write().await.insert(server_id.into(), token.into());
    }

    pub async fn forget_token(&self, server_id: &str) {
        self.tokens.write().await.remove(server_id);
    }

    pub async fn link(&self, server_id: &str) -> Option<Arc<Link>> {
        self.links.read().await.get(server_id).cloned()
    }

    pub async fn attached(&self) -> Vec<String> {
        self.links.read().await.keys().cloned().collect()
    }

    pub async fn listen(self: Arc<Self>) -> Result<()> {
        if let Some(parent) = self.socket.parent() {
            tokio::fs::create_dir_all(parent).await.ok();
        }
        let _ = tokio::fs::remove_file(&self.socket).await;

        let listener = UnixListener::bind(&self.socket)
            .with_context(|| format!("binding {}", self.socket.display()))?;

        tokio::fs::set_permissions(&self.socket, std::fs::Permissions::from_mode(0o666))
            .await
            .with_context(|| format!("opening {} to supervisors", self.socket.display()))?;

        tracing::info!(socket = %self.socket.display(), "waiting for supervisors");

        loop {
            let (stream, _) = listener.accept().await?;
            let hub = Arc::clone(&self);
            tokio::spawn(async move {
                if let Err(err) = hub.greet(stream).await {
                    tracing::warn!("supervisor connection ended: {err:#}");
                }
            });
        }
    }

    async fn greet(self: Arc<Self>, stream: UnixStream) -> Result<()> {
        let (reader, mut writer) = stream.into_split();
        let mut reader = BufReader::new(reader).lines();

        let Some(line) = reader.next_line().await? else {
            return Ok(());
        };

        let (server_id, token, pid) = match serde_json::from_str::<SupervisorMessage>(&line)? {
            SupervisorMessage::Hello { server_id, token, pid, .. } => (server_id, token, pid),
            other => {
                reject(&mut writer, "expected a greeting").await?;
                anyhow::bail!("supervisor opened with {other:?}");
            }
        };

        let expected = self.tokens.read().await.get(&server_id).cloned();
        if expected.as_deref() != Some(token.as_str()) {
            reject(&mut writer, "unknown server or token").await?;
            anyhow::bail!("rejected a supervisor claiming to be {server_id}");
        }

        write_line(&mut writer, &PanelMessage::Accepted).await?;

        let (outbox_tx, mut outbox_rx) = mpsc::channel::<PanelMessage>(OUTBOX_DEPTH);
        let (console_tx, _) = broadcast::channel::<ConsoleLine>(CONSOLE_BACKLOG);

        let link = Arc::new(Link {
            server_id: server_id.clone(),
            pid,
            outbox: outbox_tx,
            console: console_tx.clone(),
            state: RwLock::new(RunState::Running),
        });
        self.links.write().await.insert(server_id.clone(), Arc::clone(&link));
        tracing::info!(server = %server_id, pid, "supervisor attached");

        let writer_task = tokio::spawn(async move {
            while let Some(message) = outbox_rx.recv().await {
                if write_line(&mut writer, &message).await.is_err() {
                    break;
                }
            }
        });

        while let Some(line) = reader.next_line().await? {
            if line.trim().is_empty() {
                continue;
            }
            match serde_json::from_str::<SupervisorMessage>(&line) {
                Ok(message) => self.absorb(&link, message, &console_tx).await,
                Err(err) => tracing::warn!(server = %server_id, "unreadable message: {err}"),
            }
        }

        writer_task.abort();

        {
            let mut links = self.links.write().await;
            if links.get(&server_id).is_some_and(|held| Arc::ptr_eq(held, &link)) {
                links.remove(&server_id);
                tracing::info!(server = %server_id, "supervisor detached");
            } else {
                tracing::info!(server = %server_id, "an older supervisor gave up its place");
            }
        }
        Ok(())
    }

    async fn absorb(
        &self,
        link: &Arc<Link>,
        message: SupervisorMessage,
        console: &broadcast::Sender<ConsoleLine>,
    ) {
        match message {
            SupervisorMessage::Output { seq, line, stream } => {
                let _ = console.send(ConsoleLine {
                    seq,
                    line,
                    stderr: stream == OutputStream::Stderr,
                });
            }
            SupervisorMessage::Started { .. } => {
                *link.state.write().await = RunState::Running;
            }
            SupervisorMessage::Exited { code, signal, oom_killed } => {
                let next = if oom_killed {
                    RunState::OutOfMemory
                } else if code == Some(0) {
                    RunState::Stopped
                } else {
                    RunState::Crashed
                };
                tracing::info!(server = %link.server_id, ?code, ?signal, ?next, "server ended");
                *link.state.write().await = next;
            }
            SupervisorMessage::Hello { .. } => {}
        }
    }
}

async fn reject<W: AsyncWriteExt + Unpin>(writer: &mut W, reason: &str) -> Result<()> {
    write_line(writer, &PanelMessage::Rejected { reason: reason.to_owned() }).await
}

async fn write_line<W: AsyncWriteExt + Unpin>(writer: &mut W, message: &PanelMessage) -> Result<()> {
    let mut encoded = serde_json::to_vec(message)?;
    encoded.push(b'\n');
    writer.write_all(&encoded).await?;
    writer.flush().await?;
    Ok(())
}
