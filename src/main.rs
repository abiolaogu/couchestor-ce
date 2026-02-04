//! Smart Storage Operator
//!
//! A Kubernetes operator for intelligent storage tiering with OpenEBS Mayastor.
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────┐
//! │                     Smart Storage Operator                       │
//! ├─────────────────────────────────────────────────────────────────┤
//! │  ┌──────────────┐    ┌──────────────┐    ┌──────────────┐       │
//! │  │   Metrics    │───▶│  Controller  │───▶│   Migrator   │       │
//! │  │   Watcher    │    │    (Brain)   │    │   (Hands)    │       │
//! │  │   (Eyes)     │    │              │    │              │       │
//! │  └──────────────┘    └──────────────┘    └──────────────┘       │
//! └─────────────────────────────────────────────────────────────────┘
//! ```

use clap::Parser;
use kube::Client;
use std::time::Duration;
use tracing::{error, info, Level};
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

mod adapters;
mod controller;
mod crd;
mod domain;
mod ec;
mod error;
mod metrics;
mod migrator;
#[cfg(any(feature = "spdk", feature = "mock-spdk"))]
mod spdk;

use crate::controller::{ControllerContext, EcPolicyContext};
use crate::ec::{
    EcMetadataManager, ReconstructionConfig, ReconstructionEngine, StripeManager,
    StripeManagerConfig,
};
use crate::error::Result;
use crate::metrics::{MetricsConfig, MetricsWatcher};
use crate::migrator::{Migrator, MigratorConfig};

// =============================================================================
// CLI Arguments
// =============================================================================

/// Smart Storage Operator - Intelligent storage tiering for Mayastor
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Prometheus server URL
    #[arg(
        long,
        env = "PROMETHEUS_URL",
        default_value = "http://prometheus.monitoring.svc.cluster.local:9090"
    )]
    prometheus_url: String,

    /// Maximum concurrent migrations
    #[arg(long, env = "MAX_CONCURRENT_MIGRATIONS", default_value = "2")]
    max_concurrent_migrations: usize,

    /// Migration timeout in minutes
    #[arg(long, env = "MIGRATION_TIMEOUT_MINUTES", default_value = "30")]
    migration_timeout_minutes: u64,

    /// Sync poll interval in seconds
    #[arg(long, env = "SYNC_POLL_INTERVAL_SECONDS", default_value = "10")]
    sync_poll_interval_seconds: u64,

    /// Enable dry-run mode (log only, no migrations)
    #[arg(long, env = "DRY_RUN")]
    dry_run: bool,

    /// Enable preservation mode (never remove old replicas)
    #[arg(long, env = "PRESERVATION_MODE")]
    preservation_mode: bool,

    /// Mayastor namespace
    #[arg(long, env = "MAYASTOR_NAMESPACE", default_value = "mayastor")]
    mayastor_namespace: String,

    /// Metrics server bind address
    #[arg(long, env = "METRICS_ADDR", default_value = "0.0.0.0:8080")]
    metrics_addr: String,

    /// Health server bind address
    #[arg(long, env = "HEALTH_ADDR", default_value = "0.0.0.0:8081")]
    health_addr: String,

    /// Log level (trace, debug, info, warn, error)
    #[arg(long, env = "LOG_LEVEL", default_value = "info")]
    log_level: String,

    /// Output logs as JSON
    #[arg(long, env = "LOG_JSON")]
    log_json: bool,
}

// =============================================================================
// Main
// =============================================================================

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    // Initialize logging
    init_logging(&args);

    info!("Starting Smart Storage Operator");
    info!("  Prometheus URL: {}", args.prometheus_url);
    info!(
        "  Max concurrent migrations: {}",
        args.max_concurrent_migrations
    );
    info!(
        "  Migration timeout: {} minutes",
        args.migration_timeout_minutes
    );
    info!("  Dry-run mode: {}", args.dry_run);
    info!("  Preservation mode: {}", args.preservation_mode);

    // Create Kubernetes client
    let client = Client::try_default().await.map_err(|e| {
        error!("Failed to create Kubernetes client: {}", e);
        error::Error::Internal(format!("Kubernetes client creation failed: {}", e))
    })?;

    info!("Connected to Kubernetes cluster");

    // Initialize metrics watcher
    let metrics_config = MetricsConfig {
        prometheus_url: args.prometheus_url.clone(),
        query_timeout: Duration::from_secs(30),
        cache_enabled: true,
        cache_ttl: Duration::from_secs(30),
        metric_name: "openebs_volume_iops".to_string(),
        fallback_metrics: vec![
            "mayastor_volume_iops".to_string(),
            "mayastor_volume_read_ops".to_string(),
        ],
    };

    let metrics_watcher = MetricsWatcher::new(metrics_config)?;

    // Check Prometheus health
    if let Err(e) = metrics_watcher.health_check().await {
        error!("Prometheus health check failed: {}", e);
        error!("Continuing anyway - metrics may not be available");
    } else {
        info!("Prometheus connection healthy");
    }

    // Initialize migrator
    let migrator_config = MigratorConfig {
        sync_timeout: Duration::from_secs(args.migration_timeout_minutes * 60),
        sync_poll_interval: Duration::from_secs(args.sync_poll_interval_seconds),
        max_retries: 3,
        dry_run: args.dry_run,
        preservation_mode: args.preservation_mode,
    };

    let migrator = Migrator::new(migrator_config, client.clone());

    // Create controller context
    let ctx = ControllerContext::new(
        client.clone(),
        metrics_watcher,
        migrator,
        args.max_concurrent_migrations,
    );

    // Initialize EC components
    let ec_metadata_manager = EcMetadataManager::new(client.clone());

    let stripe_manager_config = StripeManagerConfig {
        dry_run: args.dry_run,
        ..Default::default()
    };
    let stripe_manager = StripeManager::new(stripe_manager_config, ec_metadata_manager.clone());

    let reconstruction_config = ReconstructionConfig::default();
    let reconstruction_engine =
        ReconstructionEngine::new(reconstruction_config, ec_metadata_manager.clone());

    // Create EC policy controller context
    let ec_policy_ctx = EcPolicyContext::new(client.clone());

    // Spawn EC background tasks
    let stripe_manager_handle = stripe_manager.clone();
    tokio::spawn(async move {
        stripe_manager_handle.run().await;
    });

    let reconstruction_handle = reconstruction_engine.clone();
    tokio::spawn(async move {
        reconstruction_handle.run().await;
    });

    // Spawn EC policy controller
    let ec_ctx = ec_policy_ctx.clone();
    tokio::spawn(async move {
        if let Err(e) = controller::run_ec_policy(ec_ctx).await {
            error!("EC policy controller error: {}", e);
        }
    });

    info!("EC components initialized");

    // Start health server
    let health_addr = args.health_addr.clone();
    tokio::spawn(async move {
        if let Err(e) = run_health_server(&health_addr).await {
            error!("Health server error: {}", e);
        }
    });

    // Start metrics server
    let metrics_addr = args.metrics_addr.clone();
    tokio::spawn(async move {
        if let Err(e) = run_metrics_server(&metrics_addr).await {
            error!("Metrics server error: {}", e);
        }
    });

    // Run the controller
    info!("Starting StoragePolicy controller");
    controller::run(ctx).await?;

    info!("Operator shutdown complete");
    Ok(())
}

// =============================================================================
// Logging Setup
// =============================================================================

fn init_logging(args: &Args) {
    let level = match args.log_level.to_lowercase().as_str() {
        "trace" => Level::TRACE,
        "debug" => Level::DEBUG,
        "info" => Level::INFO,
        "warn" => Level::WARN,
        "error" => Level::ERROR,
        _ => Level::INFO,
    };

    let filter = EnvFilter::from_default_env()
        .add_directive(level.into())
        .add_directive("hyper=warn".parse().unwrap())
        .add_directive("kube=info".parse().unwrap())
        .add_directive("tower=warn".parse().unwrap());

    if args.log_json {
        tracing_subscriber::registry()
            .with(filter)
            .with(fmt::layer().json())
            .init();
    } else {
        tracing_subscriber::registry()
            .with(filter)
            .with(fmt::layer().with_target(true))
            .init();
    }
}

// =============================================================================
// Health Server
// =============================================================================

async fn run_health_server(addr: &str) -> Result<()> {
    use http_body_util::Full;
    use hyper::body::Bytes;
    use hyper::server::conn::http1;
    use hyper::service::service_fn;
    use hyper::{Request, Response, StatusCode};
    use hyper_util::rt::TokioIo;
    use std::net::SocketAddr;
    use tokio::net::TcpListener;

    async fn health_handler(
        req: Request<hyper::body::Incoming>,
    ) -> std::result::Result<Response<Full<Bytes>>, std::convert::Infallible> {
        let response = match req.uri().path() {
            "/healthz" | "/livez" => Response::builder()
                .status(StatusCode::OK)
                .body(Full::new(Bytes::from("ok")))
                .unwrap(),
            "/readyz" => Response::builder()
                .status(StatusCode::OK)
                .body(Full::new(Bytes::from("ok")))
                .unwrap(),
            _ => Response::builder()
                .status(StatusCode::NOT_FOUND)
                .body(Full::new(Bytes::from("not found")))
                .unwrap(),
        };
        Ok(response)
    }

    let addr: SocketAddr = addr
        .parse()
        .map_err(|e| error::Error::Internal(format!("Invalid health server address: {}", e)))?;

    let listener = TcpListener::bind(addr)
        .await
        .map_err(|e| error::Error::Internal(format!("Failed to bind health server: {}", e)))?;

    info!("Health server listening on {}", addr);

    loop {
        let (stream, _) = listener
            .accept()
            .await
            .map_err(|e| error::Error::Internal(format!("Health server accept error: {}", e)))?;

        let io = TokioIo::new(stream);

        tokio::spawn(async move {
            if let Err(e) = http1::Builder::new()
                .serve_connection(io, service_fn(health_handler))
                .await
            {
                tracing::error!("Health server connection error: {}", e);
            }
        });
    }
}

// =============================================================================
// Metrics Server
// =============================================================================

async fn run_metrics_server(addr: &str) -> Result<()> {
    use http_body_util::Full;
    use hyper::body::Bytes;
    use hyper::server::conn::http1;
    use hyper::service::service_fn;
    use hyper::{Request, Response, StatusCode};
    use hyper_util::rt::TokioIo;
    use prometheus::{Encoder, TextEncoder};
    use std::net::SocketAddr;
    use tokio::net::TcpListener;

    // Register metrics
    let _reconcile_counter = prometheus::register_counter!(
        "storage_operator_reconcile_total",
        "Total number of reconciliations"
    );
    let _migration_counter = prometheus::register_counter_vec!(
        "storage_operator_migrations_total",
        "Total number of migrations",
        &["status"]
    );
    let _active_migrations = prometheus::register_gauge!(
        "storage_operator_active_migrations",
        "Number of currently active migrations"
    );

    async fn metrics_handler(
        req: Request<hyper::body::Incoming>,
    ) -> std::result::Result<Response<Full<Bytes>>, std::convert::Infallible> {
        let response = match req.uri().path() {
            "/metrics" => {
                let encoder = TextEncoder::new();
                let metric_families = prometheus::gather();
                let mut buffer = Vec::new();
                encoder.encode(&metric_families, &mut buffer).unwrap();

                Response::builder()
                    .status(StatusCode::OK)
                    .header("Content-Type", encoder.format_type())
                    .body(Full::new(Bytes::from(buffer)))
                    .unwrap()
            }
            _ => Response::builder()
                .status(StatusCode::NOT_FOUND)
                .body(Full::new(Bytes::from("not found")))
                .unwrap(),
        };
        Ok(response)
    }

    let addr: SocketAddr = addr
        .parse()
        .map_err(|e| error::Error::Internal(format!("Invalid metrics server address: {}", e)))?;

    let listener = TcpListener::bind(addr)
        .await
        .map_err(|e| error::Error::Internal(format!("Failed to bind metrics server: {}", e)))?;

    info!("Metrics server listening on {}", addr);

    loop {
        let (stream, _) = listener
            .accept()
            .await
            .map_err(|e| error::Error::Internal(format!("Metrics server accept error: {}", e)))?;

        let io = TokioIo::new(stream);

        tokio::spawn(async move {
            if let Err(e) = http1::Builder::new()
                .serve_connection(io, service_fn(metrics_handler))
                .await
            {
                tracing::error!("Metrics server connection error: {}", e);
            }
        });
    }
}
