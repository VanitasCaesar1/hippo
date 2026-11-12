//! The `hippo-feed` command line.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use anyhow::{bail, Context};
use clap::{Parser, Subcommand};
use hippo_feed::auth::mock::MockProvider;
use hippo_feed::clock::{Clock, SystemClock};
use hippo_feed::config::{Command, Config};
use hippo_feed::db::{self, Db};
use hippo_feed::http::Http;
use hippo_feed::web::{AppState, Quotas};
use hippo_feed::{activity, backup, demo, jobs, policy, registry, router, users};

#[derive(Parser)]
#[command(
    name = "hippo-feed",
    version,
    about = "hippo feed: packs, evidence and community"
)]
struct Cli {
    #[command(subcommand)]
    command: Sub,
}

#[derive(Subcommand)]
enum Sub {
    /// Serve the site.
    Serve,
    /// Apply database migrations.
    Migrate,
    /// Copy the database to a dated file and delete old copies.
    Backup {
        /// Where copies go.
        #[arg(long)]
        dir: PathBuf,
        /// Delete copies older than this many days (never the newest).
        #[arg(long, default_value_t = policy::BACKUP_KEEP_DAYS)]
        keep_days: u32,
    },
    /// Read the registry index now.
    SyncRegistry,
    /// Print weekly activity.
    Activity {
        #[arg(long, default_value_t = activity::WEEKS_DEFAULT,
              value_parser = clap::value_parser!(u32).range(1..=i64::from(activity::WEEKS_MAX)))]
        weeks: u32,
    },
    /// Export or delete a member's account.
    Account {
        #[command(subcommand)]
        action: AccountAction,
    },
    /// Run the mock sign-in provider (dev mode only).
    MockOauth {
        #[arg(long, default_value_t = 9100)]
        port: u16,
    },
    /// Fill the database with demo data (dev mode only).
    SeedDemo,
}

#[derive(Subcommand)]
enum AccountAction {
    Export { login: String },
    Delete { login: String },
}

fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .init();
    match run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> anyhow::Result<()> {
    match cli.command {
        Sub::Serve => serve(Config::from_env(Command::Serve)?),
        Sub::Migrate => {
            let config = Config::from_env(Command::Migrate)?;
            let (from, to) = db::migrate(&db_path(&config)?)?;
            println!("migrated {from} -> {to}");
            Ok(())
        }
        Sub::Backup { dir, keep_days } => {
            let config = Config::from_env(Command::Backup)?;
            let info = backup::backup(&db_path(&config)?, &dir, keep_days, SystemClock.now())?;
            println!(
                "backup {} {} bytes schema {}",
                info.path.display(),
                info.bytes,
                info.version
            );
            Ok(())
        }
        Sub::SyncRegistry => sync_registry(Config::from_env(Command::SyncRegistry)?),
        Sub::Activity { weeks } => {
            let config = Config::from_env(Command::Activity)?;
            print!(
                "{}",
                activity::table_at(&db_path(&config)?, SystemClock.now(), weeks)?
            );
            Ok(())
        }
        Sub::Account { action } => {
            let config = Config::from_env(Command::Account)?;
            let path = db_path(&config)?;
            match action {
                AccountAction::Export { login } => {
                    println!(
                        "{}",
                        users::export_by_login(&path, &login, SystemClock.now())?
                    );
                }
                AccountAction::Delete { login } => {
                    users::delete_by_login(&path, &login, SystemClock.now())?;
                    println!("deleted {login}");
                }
            }
            Ok(())
        }
        Sub::MockOauth { port } => {
            let config = Config::from_env(Command::MockOauth)?;
            require_dev(&config, "mock-oauth")?;
            mock_oauth(port)
        }
        Sub::SeedDemo => {
            let config = Config::from_env(Command::SeedDemo)?;
            require_dev(&config, "seed-demo")?;
            let seeded = demo::seed(&db_path(&config)?, SystemClock.now())?;
            println!(
                "seeded {} members, {} discussions, {} comments, {} votes and {} measurements; \
                 start the server with FEED_EVAL_SETS={}:{}:{} to open measuring",
                seeded.members,
                seeded.discussions,
                seeded.comments,
                seeded.votes,
                seeded.measurements,
                demo::EVAL_SET,
                demo::PLANTED,
                demo::KEPT
            );
            Ok(())
        }
    }
}

fn db_path(config: &Config) -> anyhow::Result<PathBuf> {
    config.db.clone().context("FEED_DB: is required")
}

fn require_dev(config: &Config, name: &str) -> anyhow::Result<()> {
    if !config.dev {
        bail!("{name} runs only in dev mode: set FEED_DEV=1");
    }
    Ok(())
}

/// Reads the registry index once and prints what changed.
fn sync_registry(config: Config) -> anyhow::Result<()> {
    let path = db_path(&config)?;
    let source = config
        .registry_index
        .clone()
        .context("FEED_REGISTRY_INDEX: is required")?;
    {
        let conn = db::open(&path)
            .with_context(|| format!("cannot open the database {}", path.display()))?;
        db::check_version(&conn)?;
    }
    let ctx = registry::SyncContext {
        source,
        raw_url: config.raw_url.clone(),
        agent: Http::new(),
    };
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("cannot start the async runtime")?;
    let report = runtime
        .block_on(registry::sync(
            &Db::new(&path),
            &ctx,
            SystemClock.now(),
            None,
        ))
        .map_err(|e| anyhow::anyhow!("{}", e.message))?;
    if let Some(why) = &report.error {
        bail!("{why}");
    }
    println!("{}", report.line());
    Ok(())
}

/// Runs the mock sign-in provider on loopback until Ctrl-C or SIGTERM.
fn mock_oauth(port: u16) -> anyhow::Result<()> {
    let (app, _handle) = MockProvider::demo(SystemClock.now());
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("cannot start the async runtime")?;
    runtime.block_on(async move {
        let address = SocketAddr::from(([127, 0, 0, 1], port));
        let listener = tokio::net::TcpListener::bind(address)
            .await
            .with_context(|| format!("cannot listen on {address}"))?;
        println!("mock-oauth listening on {}", listener.local_addr()?);
        axum::serve(listener, app)
            .with_graceful_shutdown(shutdown_signal())
            .await?;
        anyhow::Ok(())
    })
}

/// Loads the database, binds, and serves until SIGTERM or Ctrl-C; then waits
/// up to 10 seconds for requests in flight.
fn serve(config: Config) -> anyhow::Result<()> {
    let path = db_path(&config)?;
    {
        let conn = db::open(&path)
            .with_context(|| format!("cannot open the database {}", path.display()))?;
        db::check_version(&conn)?;
        registry::clear_refusals(&conn).context("cannot clear the recorded refusals")?;
    }
    let bind = config.bind;
    let state = AppState::new(
        config,
        Db::new(&path),
        Arc::new(SystemClock),
        Quotas::default(),
    );
    let app = router(state.clone());

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("cannot start the async runtime")?;
    runtime.block_on(async move {
        let listener = tokio::net::TcpListener::bind(bind)
            .await
            .with_context(|| format!("cannot listen on {bind}"))?;
        tracing::info!("listening on {bind}");
        jobs::spawn(state);

        let (stop_tx, stop_rx) = tokio::sync::oneshot::channel::<()>();
        let server = axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .with_graceful_shutdown(async move {
            shutdown_signal().await;
            let _ = stop_tx.send(());
        });
        let mut task = tokio::spawn(async move { server.await });

        tokio::select! {
            result = &mut task => {
                result.context("the server task failed")??;
            }
            _ = async {
                let _ = stop_rx.await;
                tokio::time::sleep(policy::SHUTDOWN_WAIT).await;
            } => {
                tracing::warn!("requests were still running after 10 seconds; exiting");
            }
        }
        anyhow::Ok(())
    })
}

async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut s) => {
                s.recv().await;
            }
            Err(_) => std::future::pending::<()>().await,
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
    tracing::info!("shutting down");
}
