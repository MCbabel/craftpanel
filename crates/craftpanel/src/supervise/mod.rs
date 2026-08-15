mod ring;

use std::collections::VecDeque;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use craftpanel_proto::{
    OutputStream, PanelMessage, SupervisorMessage, HELPER_PROTOCOL_VERSION,
};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::{mpsc, watch, Mutex};

pub use ring::Ring;

const RECONNECT_DELAY: Duration = Duration::from_secs(2);
const RING_CAPACITY: usize = 2000;
const SHUTDOWN_GRACE: Duration = Duration::from_secs(10);

pub struct Options {
    pub server_id: String,
    pub socket: PathBuf,
    pub working_dir: PathBuf,
    pub program: PathBuf,
    pub args: Vec<String>,
}

pub async fn run(options: Options) -> Result<()> {
    let token = std::env::var("CRAFTPANEL_SUPERVISOR_TOKEN")
        .context("CRAFTPANEL_SUPERVISOR_TOKEN is not set")?;

    let mut child = Command::new(&options.program)
        .args(&options.args)
        .current_dir(&options.working_dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .kill_on_drop(false)
        .spawn()
        .with_context(|| format!("starting {}", options.program.display()))?;

    let pid = child.id().context("child vanished before it reported a pid")?;
    tracing::info!(pid, server = %options.server_id, "game process started");

    let stdout = child.stdout.take().context("stdout was not piped")?;
    let stderr = child.stderr.take().context("stderr was not piped")?;
    let stdin = child.stdin.take().context("stdin was not piped")?;

    let ring = Arc::new(Mutex::new(Ring::new(RING_CAPACITY)));
    let (lines_tx, lines_rx) = mpsc::channel::<SupervisorMessage>(256);

    spawn_reader(stdout, OutputStream::Stdout, ring.clone(), lines_tx.clone());
    spawn_reader(stderr, OutputStream::Stderr, ring.clone(), lines_tx.clone());

    let (gone_tx, gone_rx) = watch::channel(false);
    let forced = Arc::new(AtomicBool::new(false));
    let controls = Arc::new(Controls {
        stdin: Mutex::new(Some(stdin)),
        ending: Ending {
            group: pid as libc::pid_t,
            grace: SHUTDOWN_GRACE,
            gone: gone_rx,
            forced: Arc::clone(&forced),
        },
    });
    let exit = watch_exit(child, lines_tx, gone_tx, forced, was_oom_killed);

    connect_loop(&options, &token, pid, ring, lines_rx, controls, exit).await
}

struct Controls {
    stdin: Mutex<Option<ChildStdin>>,
    ending: Ending,
}

#[derive(Clone)]
struct Ending {
    group: libc::pid_t,
    grace: Duration,
    gone: watch::Receiver<bool>,
    forced: Arc<AtomicBool>,
}

impl Ending {
    async fn now(&self) {
        self.forced.store(true, Ordering::SeqCst);
        self.signal(libc::SIGTERM);
        if self.wait_out(self.grace).await {
            return;
        }
        tracing::warn!(
            group = self.group,
            "no answer to SIGTERM in {}s, sending SIGKILL",
            self.grace.as_secs()
        );
        self.signal(libc::SIGKILL);
    }

    async fn after(&self, grace: Duration) {
        if self.wait_out(grace).await {
            return;
        }
        tracing::warn!(group = self.group, "`stop` went unanswered for {}s", grace.as_secs());
        self.now().await;
    }

    fn signal(&self, signal: i32) {
        if *self.gone.borrow() {
            return;
        }
        if unsafe { libc::killpg(self.group, signal) } == 0 {
            return;
        }
        let err = std::io::Error::last_os_error();
        if err.raw_os_error() != Some(libc::ESRCH) {
            tracing::warn!(group = self.group, "signal {signal} did not go through: {err}");
        }
    }

    async fn wait_out(&self, grace: Duration) -> bool {
        let mut gone = self.gone.clone();
        let seen = tokio::time::timeout(grace, gone.wait_for(|gone| *gone)).await;
        matches!(seen, Ok(Ok(_)))
    }
}

fn spawn_reader<R>(
    source: R,
    stream: OutputStream,
    ring: Arc<Mutex<Ring>>,
    tx: mpsc::Sender<SupervisorMessage>,
) where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        let mut lines = BufReader::new(source).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            let seq = ring.lock().await.push(&line, stream);
            let _ = tx.send(SupervisorMessage::Output { seq, line, stream }).await;
        }
    });
}

fn watch_exit(
    mut child: Child,
    tx: mpsc::Sender<SupervisorMessage>,
    gone: watch::Sender<bool>,
    forced: Arc<AtomicBool>,
    ceiling: fn() -> bool,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let ended = child.wait().await;
        let _ = gone.send(true);

        let status = match ended {
            Ok(status) => status,
            Err(err) => {
                tracing::error!("waiting for the game process failed: {err}");
                return;
            }
        };

        use std::os::unix::process::ExitStatusExt;
        let signal = status.signal();
        let message = SupervisorMessage::Exited {
            code: status.code(),
            signal,
            oom_killed: signal == Some(9) && !forced.load(Ordering::SeqCst) && ceiling(),
        };
        tracing::info!(?message, "game process ended");
        let _ = tx.send(message).await;
    })
}

fn was_oom_killed() -> bool {
    let Ok(events) = std::fs::read_to_string("/proc/self/cgroup") else {
        return false;
    };
    let Some(path) = events.lines().find_map(|l| l.rsplit(':').next()) else {
        return false;
    };
    let file = format!("/sys/fs/cgroup{path}/memory.events");
    std::fs::read_to_string(file)
        .map(|content| {
            content
                .lines()
                .find_map(|line| line.strip_prefix("oom_kill "))
                .and_then(|count| count.trim().parse::<u64>().ok())
                .is_some_and(|count| count > 0)
        })
        .unwrap_or(false)
}

async fn connect_loop(
    options: &Options,
    token: &str,
    pid: u32,
    ring: Arc<Mutex<Ring>>,
    mut lines_rx: mpsc::Receiver<SupervisorMessage>,
    controls: Arc<Controls>,
    exit: tokio::task::JoinHandle<()>,
) -> Result<()> {
    let mut pending: VecDeque<SupervisorMessage> = VecDeque::new();
    let mut finished = false;
    tokio::pin!(exit);

    loop {
        match UnixStream::connect(&options.socket).await {
            Ok(stream) => {
                tracing::info!("attached to the panel");
                match serve_panel(
                    stream, options, token, pid, &ring, &mut lines_rx, &controls, &mut pending,
                )
                .await
                {
                    Ok(()) => tracing::info!("panel disconnected"),
                    Err(err) => tracing::warn!("panel link failed: {err:#}"),
                }
            }
            Err(err) => {
                tracing::debug!("panel not reachable ({err}); the server keeps running");
                drain_into(&mut lines_rx, &mut pending);
            }
        }

        if finished && pending.is_empty() {
            return Ok(());
        }
        if exit.is_finished() {
            finished = true;
        }

        tokio::time::sleep(RECONNECT_DELAY).await;
    }
}

fn drain_into(rx: &mut mpsc::Receiver<SupervisorMessage>, sink: &mut VecDeque<SupervisorMessage>) {
    while let Ok(message) = rx.try_recv() {
        if sink.len() >= RING_CAPACITY {
            sink.pop_front();
        }
        sink.push_back(message);
    }
}

async fn serve_panel(
    stream: UnixStream,
    options: &Options,
    token: &str,
    pid: u32,
    ring: &Arc<Mutex<Ring>>,
    lines_rx: &mut mpsc::Receiver<SupervisorMessage>,
    controls: &Arc<Controls>,
    pending: &mut VecDeque<SupervisorMessage>,
) -> Result<()> {
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader).lines();

    send(
        &mut writer,
        &SupervisorMessage::Hello {
            server_id: options.server_id.clone(),
            token: token.to_owned(),
            pid,
            protocol: HELPER_PROTOCOL_VERSION,
        },
    )
    .await?;

    match reader.next_line().await?.as_deref() {
        Some(line) => match serde_json::from_str::<PanelMessage>(line)? {
            PanelMessage::Accepted => {}
            PanelMessage::Rejected { reason } => bail!("panel rejected us: {reason}"),
            other => bail!("expected a greeting, got {other:?}"),
        },
        None => bail!("panel closed the connection during the greeting"),
    }

    for line in ring.lock().await.replay() {
        send(&mut writer, &line).await?;
    }
    while let Some(message) = pending.pop_front() {
        send(&mut writer, &message).await?;
    }

    loop {
        tokio::select! {
            outgoing = lines_rx.recv() => {
                let Some(message) = outgoing else { return Ok(()) };
                if send(&mut writer, &message).await.is_err() {
                    pending.push_back(message);
                    return Ok(());
                }
            }
            incoming = reader.next_line() => {
                let Some(line) = incoming? else { return Ok(()) };
                if line.trim().is_empty() {
                    continue;
                }
                handle_panel_message(serde_json::from_str(&line)?, controls).await?;
            }
        }
    }
}

async fn handle_panel_message(message: PanelMessage, controls: &Arc<Controls>) -> Result<()> {
    match message {
        PanelMessage::Stdin { line } => write_stdin(&controls.stdin, &line).await,
        PanelMessage::Stop { command, grace_seconds } => {
            let asked = write_stdin(&controls.stdin, command.as_deref().unwrap_or("stop")).await;
            let ending = controls.ending.clone();
            let grace = Duration::from_secs(u64::from(grace_seconds));
            tokio::spawn(async move { ending.after(grace).await });
            asked
        }
        PanelMessage::Kill => {
            let ending = controls.ending.clone();
            tokio::spawn(async move { ending.now().await });
            Ok(())
        }
        PanelMessage::Accepted | PanelMessage::Rejected { .. } => Ok(()),
    }
}

async fn write_stdin(stdin: &Mutex<Option<ChildStdin>>, line: &str) -> Result<()> {
    let mut guard = stdin.lock().await;
    let Some(pipe) = guard.as_mut() else {
        return Ok(());
    };
    pipe.write_all(line.as_bytes()).await?;
    pipe.write_all(b"\n").await?;
    pipe.flush().await?;
    Ok(())
}

async fn send<W>(writer: &mut W, message: &SupervisorMessage) -> Result<()>
where
    W: AsyncWriteExt + Unpin,
{
    let mut encoded = serde_json::to_vec(message)?;
    encoded.push(b'\n');
    writer.write_all(&encoded).await?;
    writer.flush().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Doomed {
        controls: Arc<Controls>,
        ends: mpsc::Receiver<SupervisorMessage>,
    }

    impl Doomed {
        async fn new(script: &str, grace: Duration) -> Self {
            Self::under_ceiling(script, grace, was_oom_killed).await
        }

        async fn under_ceiling(script: &str, grace: Duration, ceiling: fn() -> bool) -> Self {
            let mut child = Command::new("/bin/sh")
                .arg("-c")
                .arg(script)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .process_group(0)
                .kill_on_drop(false)
                .spawn()
                .expect("a shell");

            let stdin = child.stdin.take();
            let mut said = BufReader::new(child.stdout.take().expect("stdout")).lines();
            let ready = said.next_line().await.expect("the shell ran");
            assert_eq!(ready.as_deref(), Some("ready"));

            let group = child.id().expect("a pid") as libc::pid_t;
            let (lines_tx, ends) = mpsc::channel(8);
            let (gone_tx, gone) = watch::channel(false);
            let forced = Arc::new(AtomicBool::new(false));
            watch_exit(child, lines_tx, gone_tx, Arc::clone(&forced), ceiling);

            Self {
                controls: Arc::new(Controls {
                    stdin: Mutex::new(stdin),
                    ending: Ending { group, grace, gone, forced },
                }),
                ends,
            }
        }

        fn alive(&self) -> bool {
            !*self.controls.ending.gone.borrow()
        }

        async fn ended(&mut self) -> (Option<i32>, bool) {
            let message = tokio::time::timeout(Duration::from_secs(20), self.ends.recv())
                .await
                .expect("the child is still running")
                .expect("an exit message");
            match message {
                SupervisorMessage::Exited { signal, oom_killed, .. } => (signal, oom_killed),
                other => panic!("expected an exit, got {other:?}"),
            }
        }
    }

    impl Drop for Doomed {
        fn drop(&mut self) {
            self.controls.ending.signal(libc::SIGKILL);
        }
    }

    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir()
                .join(format!("craftpanel-ending-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }

        fn file(&self, name: &str) -> PathBuf {
            self.0.join(name)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn size_of(path: &PathBuf) -> u64 {
        std::fs::metadata(path).map(|meta| meta.len()).unwrap_or(0)
    }

    #[tokio::test]
    async fn a_child_that_ignores_sigterm_outlives_the_grace_and_not_the_kill() {
        let grace = Duration::from_millis(600);
        let mut doomed =
            Doomed::new("trap '' TERM; echo ready; while :; do sleep 1; done", grace).await;

        handle_panel_message(PanelMessage::Kill, &doomed.controls).await.unwrap();

        tokio::time::sleep(grace / 2).await;
        assert!(doomed.alive(), "the grace was cut short");

        let (signal, _) = doomed.ended().await;
        assert_eq!(signal, Some(9), "the grace passed, so SIGKILL had to finish it");
    }

    #[tokio::test]
    async fn a_kill_of_our_own_is_not_the_memory_ceiling() {
        let mut doomed = Doomed::under_ceiling(
            "trap '' TERM; echo ready; while :; do sleep 1; done",
            Duration::from_millis(300),
            || true,
        )
        .await;

        handle_panel_message(PanelMessage::Kill, &doomed.controls).await.unwrap();

        let (signal, oom) = doomed.ended().await;
        assert_eq!(signal, Some(9), "it never heard SIGTERM, so ours was the SIGKILL");
        assert!(!oom, "the account had been at its ceiling before, but this end was ours");
    }

    #[tokio::test]
    async fn a_sigkill_that_was_not_ours_is_still_the_memory_ceiling() {
        let mut doomed = Doomed::under_ceiling(
            "echo ready; while :; do sleep 1; done",
            Duration::from_secs(300),
            || true,
        )
        .await;

        assert_eq!(unsafe { libc::kill(doomed.controls.ending.group, libc::SIGKILL) }, 0);

        let (signal, oom) = doomed.ended().await;
        assert_eq!(signal, Some(9));
        assert!(oom, "no ending of ours explains it, so the ceiling does");
    }

    #[tokio::test]
    async fn a_child_that_takes_sigterm_is_never_killed() {
        let grace = Duration::from_secs(300);
        let mut doomed = Doomed::new("echo ready; while :; do sleep 1; done", grace).await;

        handle_panel_message(PanelMessage::Kill, &doomed.controls).await.unwrap();

        let (signal, _) = doomed.ended().await;
        assert_eq!(signal, Some(15), "SIGTERM ended it, and nothing harder was needed");
    }

    #[tokio::test]
    async fn the_kill_reaches_what_the_game_started_too() {
        let scratch = Scratch::new("tree");
        let ticks = scratch.file("ticks");
        let mut doomed = Doomed::new(
            &format!(
                "trap '' TERM; ( while :; do echo tick >> {}; sleep 0.1; done ) & \
                 echo ready; wait",
                ticks.display()
            ),
            Duration::from_millis(600),
        )
        .await;

        let mut running = 0;
        for _ in 0..100 {
            tokio::time::sleep(Duration::from_millis(50)).await;
            running = size_of(&ticks);
            if running > 0 {
                break;
            }
        }
        assert!(running > 0, "the child of the child never got going");

        handle_panel_message(PanelMessage::Kill, &doomed.controls).await.unwrap();
        assert_eq!(doomed.ended().await.0, Some(9));

        let last = size_of(&ticks);
        tokio::time::sleep(Duration::from_millis(400)).await;
        assert_eq!(size_of(&ticks), last, "what the game started is still writing");
    }

    #[tokio::test]
    async fn a_stop_the_game_never_carries_out_still_ends_it() {
        let mut doomed = Doomed::new(
            "trap '' TERM; echo ready; while :; do sleep 1; done",
            Duration::from_millis(400),
        )
        .await;

        handle_panel_message(
            PanelMessage::Stop { command: Some("stop".to_owned()), grace_seconds: 1 },
            &doomed.controls,
        )
        .await
        .unwrap();

        assert!(doomed.alive(), "the console command is asked first, not forced");
        assert_eq!(doomed.ended().await.0, Some(9), "the grace ran out and nothing came of it");
    }
}
