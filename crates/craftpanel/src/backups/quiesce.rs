use std::sync::Arc;
use std::time::Duration;

use sqlx::SqlitePool;
use tokio::sync::broadcast::error::RecvError;

use crate::model::{Id, LoaderId};
use crate::servers::{Hub, Link};

const CONFIRMATION: Duration = Duration::from_secs(30);

const REATTACH: Duration = Duration::from_secs(120);
const REATTACH_POLL: Duration = Duration::from_secs(1);

const SAVE_OFF: &str = "save-off";
const SAVE_FLUSH: &str = "save-all flush";
const SAVE_ON: &str = "save-on";

const CONFIRMATIONS: [&str; 2] = ["Saved the game", "Saved the world"];

pub struct Held {
    link: Arc<Link>,
    pub confirmed: bool,
}

impl Held {
    pub async fn take(hub: &Hub, server: Id, loader: Option<LoaderId>) -> Option<Self> {
        Self::take_within(hub, server, loader, CONFIRMATION).await
    }

    pub async fn take_within(
        hub: &Hub,
        server: Id,
        loader: Option<LoaderId>,
        patience: Duration,
    ) -> Option<Self> {
        if loader == Some(LoaderId::Velocity) {
            return None;
        }
        let link = running_link(hub, server).await?;

        let mut console = link.subscribe();

        if let Err(err) = say(&link, SAVE_OFF, patience).await {
            tracing::warn!(%server, "save-off did not reach the server: {err:#}");
            return None;
        }
        let mut held = Self { link, confirmed: false };
        if let Err(err) = say(&held.link, SAVE_FLUSH, patience).await {
            tracing::warn!(%server, "save-all flush did not reach the server: {err:#}");
            return Some(held);
        }

        held.confirmed = tokio::time::timeout(patience, async {
            loop {
                match console.recv().await {
                    Ok(line) if is_confirmation(&line.line) => return true,
                    Ok(_) => continue,
                    Err(RecvError::Lagged(_)) => continue,
                    Err(RecvError::Closed) => return false,
                }
            }
        })
        .await
        .unwrap_or(false);

        Some(held)
    }

    pub fn warning(server: Id) -> String {
        format!(
            "[craftpanel] {server} did not confirm 'save-all flush' within {} seconds; \
             the backup was taken anyway and may hold a half written region",
            CONFIRMATION.as_secs()
        )
    }
}

impl Drop for Held {
    fn drop(&mut self) {
        let link = Arc::clone(&self.link);
        match tokio::runtime::Handle::try_current() {
            Ok(handle) => {
                handle.spawn(async move {
                    if let Err(err) = say(&link, SAVE_ON, CONFIRMATION).await {
                        tracing::error!(
                            server = %link.server_id,
                            "save-on did not reach the server: {err:#}; \
                             it is running without automatic saving"
                        );
                    }
                });
            }
            Err(_) => tracing::error!(
                server = %link.server_id,
                "no runtime left to send save-on; the next panel start sweeps it up"
            ),
        }
    }
}

pub async fn sweep_after_restart(pool: &SqlitePool, hub: Arc<Hub>) -> sqlx::Result<Vec<Id>> {
    let interrupted: Vec<Id> = sqlx::query_scalar(
        "SELECT DISTINCT server_id FROM operations
          WHERE kind = 'backup_create' AND state IN ('queued', 'ongoing')",
    )
    .fetch_all(pool)
    .await?;

    for server in interrupted.iter().copied() {
        let hub = Arc::clone(&hub);
        tokio::spawn(async move {
            let deadline = tokio::time::Instant::now() + REATTACH;
            while tokio::time::Instant::now() < deadline {
                if let Some(link) = running_link(&hub, server).await {
                    match say(&link, SAVE_ON, CONFIRMATION).await {
                        Ok(()) => tracing::info!(%server, "saving switched back on after a restart"),
                        Err(err) => tracing::error!(%server, "save-on after a restart: {err:#}"),
                    }
                    return;
                }
                tokio::time::sleep(REATTACH_POLL).await;
            }
            tracing::info!(%server, "no supervisor came back; nothing left to switch on");
        });
    }
    Ok(interrupted)
}

async fn say(link: &Link, command: &str, patience: Duration) -> anyhow::Result<()> {
    match tokio::time::timeout(patience, link.send_command(command)).await {
        Ok(sent) => sent,
        Err(_) => anyhow::bail!("{command} was not taken within {patience:?}"),
    }
}

pub async fn running_link(hub: &Hub, server: Id) -> Option<Arc<Link>> {
    let link = hub.link(&server.to_string()).await?;
    link.state().await.is_live().then_some(link)
}

fn is_confirmation(line: &str) -> bool {
    CONFIRMATIONS.iter().any(|wanted| line.contains(wanted))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backups::testing::FakeServer;

    #[test]
    fn both_spellings_of_the_confirmation_are_recognised() {
        assert!(is_confirmation("[15:04:22] [Server thread/INFO]: Saved the game"));
        assert!(is_confirmation("[15:04:22] [Server thread/INFO]: Saved the world"));
        assert!(!is_confirmation("[15:04:22] [Server thread/INFO]: Saving the game"));
    }

    #[tokio::test]
    async fn the_three_commands_go_out_in_order_and_save_on_always_follows() {
        let game = FakeServer::start().await;
        let held = Held::take(game.hub(), game.server, Some(LoaderId::Paper))
            .await
            .expect("a running server is held");
        assert!(held.confirmed, "the fake answers the flush at once");
        assert_eq!(game.commands().await, vec![SAVE_OFF, SAVE_FLUSH]);

        drop(held);
        game.settle().await;
        assert_eq!(game.commands().await, vec![SAVE_OFF, SAVE_FLUSH, SAVE_ON]);
    }

    #[tokio::test]
    async fn a_panic_while_the_hold_is_up_still_switches_saving_on() {
        let game = FakeServer::start().await;
        let hub = Arc::clone(game.hub_arc());
        let server = game.server;

        let panicked = tokio::spawn(async move {
            let _held = Held::take(&hub, server, None).await.expect("a hold");
            panic!("the packing thread fell over");
        })
        .await;
        assert!(panicked.is_err(), "the task really did panic");

        game.settle().await;
        assert_eq!(game.commands().await.last().map(String::as_str), Some(SAVE_ON));
    }

    #[tokio::test]
    async fn a_server_that_never_confirms_is_backed_up_anyway_after_the_wait() {
        let game = FakeServer::silent().await;
        let held = Held::take_within(game.hub(), game.server, None, Duration::from_millis(50))
            .await
            .expect("a hold");
        assert!(!held.confirmed, "10.2: pack anyway, and say so in the console");
        assert!(Held::warning(game.server).contains("30 seconds"));
    }

    #[tokio::test]
    async fn a_proxy_and_a_stopped_server_are_never_held() {
        let game = FakeServer::start().await;
        assert!(
            Held::take(game.hub(), game.server, Some(LoaderId::Velocity)).await.is_none(),
            "a proxy has no world to flush"
        );

        let stopped = FakeServer::stopped().await;
        assert!(Held::take(stopped.hub(), stopped.server, None).await.is_none());
        assert!(game.commands().await.is_empty());
    }

    #[tokio::test]
    async fn a_restart_between_save_off_and_save_on_is_swept_up_when_the_server_comes_back() {
        let game = FakeServer::start().await;
        let run = crate::ops::NewOperation::new(
            game.server,
            crate::model::OperationKind::BackupCreate,
            None,
        );
        game.operations.create(run).await.expect("an interrupted run");

        let swept = sweep_after_restart(game.pool(), Arc::clone(game.hub_arc()))
            .await
            .expect("a sweep");
        assert_eq!(swept, vec![game.server]);

        game.settle().await;
        assert_eq!(
            game.commands().await,
            vec![SAVE_ON],
            "the one command that keeps a server from silently never saving again"
        );
    }
}
