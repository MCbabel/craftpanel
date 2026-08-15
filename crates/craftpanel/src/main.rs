mod api;
mod audit;
mod auth;
mod backups;
mod config;
mod console;
mod content;
mod db;
mod drive;
mod files;
mod helper;
mod loaders;
mod mail;
mod model;
mod ops;
mod playit;
mod registration;
mod servers;
mod settings;
mod supervise;
mod web;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use axum::routing::get;
use axum::{Json, Router};
use clap::{Parser, Subcommand};
use sqlx::SqlitePool;
use tower_http::trace::TraceLayer;
use tracing_subscriber::EnvFilter;

use crate::config::Config;

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    pub pool: SqlitePool,
}

#[derive(Parser)]
#[command(name = "craftpanel", version, about = "Minecraft server panel")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Run the panel (the default when no subcommand is given).
    Serve,
    /// Look after panel accounts from the command line. The installer uses this
    /// to make the first administrator before anything is listening.
    #[command(subcommand)]
    Admin(auth::cli::AdminCommand),
    /// Look at the mails the panel sends, without a Resend key and without a
    /// database: `craftpanel mail preview --out /tmp/craftpanel-mail`.
    #[command(subcommand)]
    Mail(mail::cli::MailCommand),
    /// Attend one game process. Started by the privileged helper, never by hand.
    Supervise {
        #[arg(long)]
        server_id: String,
        #[arg(long)]
        socket: PathBuf,
        #[arg(long)]
        working_dir: PathBuf,
        #[arg(long)]
        program: PathBuf,
        #[arg(trailing_var_arg = true)]
        args: Vec<String>,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_env("CRAFTPANEL_LOG").unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    match Cli::parse().command {
        Some(Command::Supervise { server_id, socket, working_dir, program, args }) => {
            supervise::run(supervise::Options { server_id, socket, working_dir, program, args })
                .await
        }
        Some(Command::Admin(command)) => auth::cli::run(command).await,
        Some(Command::Mail(command)) => mail::cli::run(command).await,
        Some(Command::Serve) | None => serve().await,
    }
}

async fn serve() -> Result<()> {
    let config_path = std::env::var_os("CRAFTPANEL_CONFIG")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/etc/craftpanel/config.toml"));

    let config = Config::load(&config_path)?;
    tracing::info!(path = %config_path.display(), data = %config.data_dir.display(), "starting");

    let pool = db::connect(&config.database_path()).await?;
    let state = AppState { config: Arc::new(config.clone()), pool: pool.clone() };

    let operations = ops::Operations::new(pool.clone(), config.data_dir.clone());
    let hub = Arc::new(servers::Hub::new(
        config.helper_socket.with_file_name("supervise.sock"),
    ));

    tokio::spawn({
        let hub = Arc::clone(&hub);
        async move {
            if let Err(err) = hub.listen().await {
                tracing::error!("the supervisor socket stopped: {err:#}");
            }
        }
    });

    let live = {
        let hub = Arc::clone(&hub);
        auth::LiveServers::from_fn(move || {
            let hub = Arc::clone(&hub);
            async move { hub.attached().await.into_iter().filter_map(|id| id.parse().ok()).collect() }
        })
    };

    let helper = helper::Helper::new(config.helper_socket.clone());

    let disks = auth::Disks::over(
        pool.clone(),
        config.data_dir.clone(),
        auth::disk::WINDOW,
        helper.clone(),
    );

    let sources = Arc::new(loaders::Sources::new().context("setting up the loader sources")?);

    let playit = playit::Playit::new(pool.clone(), Arc::clone(&state.config))
        .context("setting up the playit.gg service")?;
    playit.start();

    let mail = mail::Mail::new(pool.clone(), Arc::clone(&state.config))
        .context("setting up the mail service")?;
    mail.start();

    let sign_ups =
        registration::Registrations::new(pool.clone(), Arc::clone(&mail), helper.clone());
    let recovery = auth::reset::Recovery::new(pool.clone(), Arc::clone(&mail));

    let drive = drive::Drive::new(pool.clone(), Arc::clone(&state.config))
        .context("setting up the Google Drive service")?;
    drive.start();

    let manager = servers::manager::Manager::new(
        pool.clone(),
        Arc::clone(&state.config),
        Arc::clone(&operations),
        Arc::clone(&hub),
        helper.clone(),
        sources,
        disks.clone(),
    );
    let content = content::Content::new(
        pool.clone(),
        config.data_dir.clone(),
        helper.clone(),
        Arc::clone(&operations),
        disks.clone(),
    )
    .context("setting up the content service")?;
    let backups = backups::Backups::new(
        pool.clone(),
        config.data_dir.clone(),
        Arc::clone(&operations),
        Arc::clone(&hub),
        helper.clone(),
        disks.clone(),
        Arc::clone(&drive),
    );

    manager.spawn_recovery();

    await_helper(&helper).await;

    match auth::users::reconcile(&pool, &helper).await {
        Ok(ready) if ready > 0 => tracing::info!(accounts = ready, "system accounts made ready"),
        Ok(_) => {}
        Err(err) => tracing::warn!("could not settle pending system accounts: {err}"),
    }

    tokio::spawn(ops::follow(Arc::clone(&operations), Arc::clone(&hub)));

    content.sweep_updates(live.clone());
    manager.spawn_metrics();
    manager.spawn_dispatcher();
    operations.spawn_housekeeping();
    backups.spawn_scheduler();
    audit::spawn_purge(pool.clone());
    mail::spawn_purge(pool.clone());
    registration::spawn_sweep(Arc::clone(&sign_ups));
    auth::disk::spawn_sweep(pool.clone(), disks.clone());
    tokio::spawn(sweep_upload_parts(pool.clone(), Arc::clone(&state.config)));

    let api = Router::new()
        .route("/health", get(health))
        .merge(api::with_live(
            live.clone(),
            disks.clone(),
            Arc::clone(&playit),
            Arc::clone(&drive),
        ))
        .merge(ops::api::router(Arc::clone(&operations)))
        .merge(api::ws::router(Arc::clone(&operations)))
        .merge(api::servers::router(manager))
        .merge(api::files::router(Arc::clone(&operations), disks.clone()))
        .merge(api::content::router(Arc::clone(&content), live.clone()))
        .merge(api::registration::with_live(Arc::clone(&sign_ups), live.clone(), disks.clone()))
        .merge(api::recovery::router(Arc::clone(&recovery)))
        .merge(api::settings::router(Arc::clone(&operations), live))
        .merge(api::backups::router(Arc::clone(&backups)))
        .merge(api::access::router())
        .merge(api::playit::router(Arc::clone(&playit)))
        .merge(api::mail::router(Arc::clone(&mail)))
        .merge(api::drive::router(Arc::clone(&drive)))
        .merge(api::console::router(Arc::clone(&operations), Arc::clone(&hub)))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            auth::extract::slide_sessions,
        ))
        .with_state(state.clone());

    let app = Router::new()
        .nest("/api/v1", api)
        .merge(api::backups::compat_router(backups).with_state(state))
        .merge(web::router())
        .layer(TraceLayer::new_for_http());

    let listener = tokio::net::TcpListener::bind(config.bind)
        .await
        .with_context(|| format!("binding {}", config.bind))?;

    tracing::info!(addr = %config.bind, "listening");
    axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>())
        .await
        .context("serving")?;

    Ok(())
}

async fn sweep_upload_parts(pool: SqlitePool, config: Arc<Config>) {
    let mut tick = tokio::time::interval(std::time::Duration::from_secs(60 * 60));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    loop {
        tick.tick().await;
        match files::sweep_parts(&pool, &config).await {
            Ok(0) => {}
            Ok(swept) => tracing::info!(swept, "cleared abandoned upload parts"),
            Err(err) => tracing::warn!("could not sweep upload parts: {err}"),
        }
    }
}

async fn await_helper(helper: &helper::Helper) {
    for attempt in 1..=20 {
        match helper.ping().await {
            Ok(version) => {
                tracing::info!(version, "the privileged helper answered");
                if version != craftpanel_proto::HELPER_PROTOCOL_VERSION {
                    tracing::error!(
                        version,
                        wanted = craftpanel_proto::HELPER_PROTOCOL_VERSION,
                        "this helper speaks another version of the protocol; \
                         run install.sh again so that both binaries are the same build"
                    );
                }
                return;
            }
            Err(err) if attempt == 20 => {
                tracing::warn!("the privileged helper never answered: {err:#}");
            }
            Err(_) => tokio::time::sleep(std::time::Duration::from_millis(250)).await,
        }
    }
}

async fn health() -> Json<serde_json::Value> {
    Json(serde_json::json!({ "status": "ok", "version": env!("CARGO_PKG_VERSION") }))
}
