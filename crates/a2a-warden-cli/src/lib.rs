//! A2A Warden CLI library.
//!
//! Wiring shared by the `a2a-warden` executable and by the end to end
//! acceptance suite, so the tests exercise exactly the code that ships.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::task::JoinHandle;

use clap::Parser;
use tracing_subscriber::EnvFilter;

use a2a_warden_core::config::Config;
use a2a_warden_core::protocol::AgentCard;
use a2a_warden_core::push::PushPolicy;
use a2a_warden_core::service::WardenService;
use a2a_warden_core::stores::Store;
use a2a_warden_core::verification::VerificationPolicy;
use a2a_warden_crypto::{AgentCardVerifier, AllowedAlgorithm, JwksResolver, JwksSource};
use a2a_warden_server::agent_client::ReqwestAgentClient;
use a2a_warden_server::push_client::HttpPushNotifier;
use a2a_warden_server::{build_router, build_state};

/// Command line interface of the warden.
#[derive(Debug, Parser)]
#[command(
    name = "a2a-warden",
    version,
    about = "A2A Warden: see every message, verify every agent, approve every task"
)]
pub struct Cli {
    /// Path to the YAML configuration file.
    #[arg(short, long, value_name = "FILE")]
    pub config: Option<PathBuf>,

    /// Override the listen address (`host:port`).
    #[arg(long, value_name = "ADDR")]
    pub listen: Option<String>,

    /// Print the effective configuration and exit.
    #[arg(long)]
    pub check: bool,

    /// Increase log verbosity (repeatable).
    #[arg(short, long, action = clap::ArgAction::Count)]
    pub verbose: u8,
}

/// A running warden, ready to be served.
pub struct Warden {
    /// The application router.
    pub router: axum::Router,
    /// The address the server was bound to.
    pub listen_addr: SocketAddr,
    /// Background tasks spawned during setup (e.g. task expiry sweeper).
    ///
    /// These are aborted during shutdown so the process exits cleanly.
    pub background_tasks: Vec<JoinHandle<()>>,
}

/// Builds the warden from a validated configuration, without serving it.
///
/// The database is connected, the migrations are applied, and the configured
/// agents are registered in the agent registry.
///
/// # Errors
/// Returns an error when the database cannot be reached or the agents cannot
/// be registered.
pub async fn build(
    config: &Config,
    background_tasks: &mut Vec<JoinHandle<()>>,
) -> Result<Warden, Box<dyn std::error::Error + Send + Sync>> {
    // Persistence: the backend is selected exclusively by configuration and no
    // domain code depends on which one is active (AD-2).
    let store = a2a_warden_store::connect(&config.database).await?;
    let store: Arc<dyn Store> = store;

    // The warden acts as an A2A client toward the registered remote agents.
    let client =
        Arc::new(ReqwestAgentClient::new(Duration::from_secs(config.server.request_timeout_secs))?);

    let mut service = WardenService::new(store, client);

    // Push notifications: the service enforces the SSRF policy on
    // registration and again at delivery time; the notifier only posts.
    if config.push_notifications.enabled {
        let notifier = Arc::new(HttpPushNotifier::new(Duration::from_secs(
            config.push_notifications.timeout_secs,
        ))?);
        let policy = PushPolicy::new(
            true,
            config.push_notifications.allowed_hosts.clone(),
            config.push_notifications.allow_private_addresses,
        );
        service = service.with_push(policy, notifier);
    }

    // Agent Card verification: configured JWKS sources are pinned to the
    // algorithms admitted by the crate. Verification becomes mandatory as soon
    // as a key set is pinned (see `VerificationPolicy::from_config`).
    let verification_policy = VerificationPolicy::from_config(&config.agent_card_verification);
    if verification_policy.enabled {
        let sources = config
            .agent_card_verification
            .jwks
            .iter()
            .map(|source| JwksSource {
                issuer: source.issuer.clone(),
                jwks_json: source.jwks_json.clone(),
                url: source.url.clone(),
                allowed_algorithms: source
                    .allowed_algorithms
                    .iter()
                    .filter_map(|algorithm| AllowedAlgorithm::parse(algorithm))
                    .collect(),
            })
            .collect::<Vec<_>>();
        let verifier = Arc::new(AgentCardVerifier::new(Arc::new(JwksResolver::new(sources))));
        service = service.with_verification(verifier, verification_policy);
    }

    // Cedar policy engine: pass the configured policy rules to the service.
    service = service.with_policy_config(config.policy.clone());

    service.register_agents(&config.agents).await?;

    // Task expiry: a background sweeper cancels tasks that never reached a
    // terminal state. Disabled by default, so legacy keeps every task unless
    // the operator opts in.
    if config.task_expiry.enabled {
        let sweeper = service.clone();
        let interval = Duration::from_secs(config.task_expiry.sweep_interval_secs.max(1));
        let ttl = Duration::from_secs(config.task_expiry.ttl_secs.max(1));
        let handle = tokio::spawn(async move {
            let mut ticker = tokio::time::interval(interval);
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                ticker.tick().await;
                match sweeper.expire_stale_tasks(ttl).await {
                    Ok(0) => {}
                    Ok(expired) => {
                        tracing::info!(expired, "expired inactive tasks");
                    }
                    Err(err) => {
                        tracing::warn!(error = %err, "task expiry sweep failed");
                    }
                }
            }
        });
        background_tasks.push(handle);
    }

    let listen_addr: SocketAddr =
        config.server.listen_addr.parse().map_err(|err| {
            format!("invalid listen address '{}': {err}", config.server.listen_addr)
        })?;

    let public_url = config
        .server
        .public_url
        .clone()
        .unwrap_or_else(|| format!("http://127.0.0.1:{}", listen_addr.port()));
    let agent_card = AgentCard::warden_card("A2A Warden", &public_url, env!("CARGO_PKG_VERSION"));

    let state = build_state(config, service, agent_card, config.database.backend_type());
    let router = build_router(state, config)?;

    Ok(Warden {
        router,
        listen_addr,
        background_tasks: std::mem::take(background_tasks),
    })
}

/// Runs the warden: loads the configuration, builds it, and serves it.
///
/// # Errors
/// Returns an error when the configuration is invalid, the database cannot be
/// reached, or the listener cannot be bound.
pub async fn run(cli: Cli) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut config = Config::load(cli.config.as_ref())?;
    if let Some(listen) = cli.listen.clone() {
        config.server.listen_addr = listen;
    }
    config.validate()?;

    tracing::info!(
        listen = %config.server.listen_addr,
        backend = config.database.backend_type(),
        agents = config.agents.len(),
        "starting a2a-warden"
    );

    if cli.check {
        println!("configuration is valid");
        println!("  listen address : {}", config.server.listen_addr);
        println!("  database       : {}", config.database.backend_type());
        for agent in &config.agents {
            println!(
                "  agent          : {} ({} skills)",
                agent.name,
                agent.skills.len()
            );
        }
        return Ok(());
    }

    let mut background_tasks: Vec<JoinHandle<()>> = Vec::new();
    let warden = build(&config, &mut background_tasks).await?;
    let listener = tokio::net::TcpListener::bind(warden.listen_addr)
        .await
        .map_err(|err| format!("failed to bind {}: {err}", warden.listen_addr))?;

    tracing::info!(
        "listening on http://{} (console: http://{}/ui, admin: http://{}{})",
        warden.listen_addr,
        warden.listen_addr,
        warden.listen_addr,
        config.admin.path
    );

    let serve_result = axum::serve(listener, warden.router)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .map_err(|err| format!("server error: {err}"));

    // Abort all background tasks so the process exits cleanly, even on error.
    for handle in warden.background_tasks {
        handle.abort();
    }

    serve_result?;

    // Force process exit: other tokio::spawn tasks (push delivery, streaming)
    // may keep the Tokio runtime alive even after the HTTP server stops.
    tracing::info!("warden stopped");
    std::process::exit(0);
}

/// Initializes the tracing subscriber.
pub fn init_tracing(verbosity: u8) {
    let default = match verbosity {
        0 => "info",
        1 => "debug",
        _ => "trace",
    };
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(format!("a2a_warden={default},{default}")));
    tracing_subscriber::fmt().with_env_filter(filter).with_target(true).init();
}

/// Waits for Ctrl+C or SIGTERM.
pub async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c().await.expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await
            .expect("failed to receive SIGTERM");
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }

    tracing::info!("shutdown signal received, exiting");
    std::process::exit(0);
}
