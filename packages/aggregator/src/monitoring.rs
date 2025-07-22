use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
    Router,
};
use prometheus_client::registry::Registry;
use std::sync::Arc;
use tokio::time::{interval, Duration};
use tower_http::cors::CorsLayer;
use tracing::info;

use crate::metrics::{encode_metrics, Metrics};
use crate::takehome::exchange_graph::ExchangeGraph;

pub struct MonitoringServer {
    registry: Registry,
    metrics: Metrics,
}

impl MonitoringServer {
    pub fn new() -> Self {
        let mut registry = Registry::default();
        let metrics = Metrics::new(&mut registry);

        Self { registry, metrics }
    }
}

impl Default for MonitoringServer {
    fn default() -> Self {
        Self::new()
    }
}

impl MonitoringServer {
    pub fn metrics(&self) -> Metrics {
        self.metrics.clone()
    }

    pub async fn start(self, port: u16) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let app = Router::new()
            .route("/metrics", get(metrics_handler))
            .route("/health", get(health_handler))
            .route("/ready", get(ready_handler))
            .layer(CorsLayer::permissive())
            .with_state(Arc::new(self));

        let addr = format!("0.0.0.0:{}", port);
        info!("Starting monitoring server on {}", addr);

        axum::serve(tokio::net::TcpListener::bind(&addr).await?, app).await?;

        Ok(())
    }
}

async fn metrics_handler(
    State(state): State<Arc<MonitoringServer>>,
) -> Result<Response, StatusCode> {
    match encode_metrics(&state.registry) {
        Ok(metrics) => Ok(metrics.into_response()),
        Err(_) => Err(StatusCode::INTERNAL_SERVER_ERROR),
    }
}

async fn health_handler() -> impl IntoResponse {
    StatusCode::OK
}

async fn ready_handler() -> impl IntoResponse {
    StatusCode::OK
}

/// Calculate approximate memory usage of the ExchangeGraph
fn calculate_memory_usage(exchange_graph: &Arc<ExchangeGraph>) -> i64 {
    let stats = exchange_graph.get_stats();

    // Base size of ExchangeGraph struct
    let base_size = std::mem::size_of::<ExchangeGraph>();

    // Estimate memory for trading pairs
    // Each TradingPair is approximately 64 bytes + overhead
    let trading_pairs_memory = stats.num_trading_pairs * 100; // ~100 bytes per pair including overhead

    // Estimate memory for cached routes
    // Each CachedRoute is approximately 200 bytes + overhead
    let cached_routes_memory = stats.num_cached_routes * 300; // ~300 bytes per route including overhead

    // Estimate memory for sharded hash maps overhead
    let sharding_overhead = 1024 * 16; // 16 shards * 1KB overhead per shard

    // Estimate memory for Arc and other overhead
    let arc_overhead = 1024 * 4; // 4KB for Arc overhead

    base_size as i64
        + trading_pairs_memory as i64
        + cached_routes_memory as i64
        + sharding_overhead as i64
        + arc_overhead as i64
}

pub async fn start_metrics_updater(
    metrics: Metrics,
    exchange_graph: Arc<crate::takehome::exchange_graph::ExchangeGraph>,
) {
    let mut interval = interval(Duration::from_secs(5));

    loop {
        interval.tick().await;

        // Update system metrics
        let stats = exchange_graph.get_stats();

        metrics
            .active_trading_pairs
            .set(stats.num_trading_pairs as i64);
        metrics.cached_routes.set(stats.num_cached_routes as i64);

        // Update cache metrics from ExchangeGraph stats
        metrics.cache_hits.inc_by(stats.cache_hits);
        metrics.cache_misses.inc_by(stats.cache_misses);

        // Update memory usage (approximate)
        let memory_usage = calculate_memory_usage(&exchange_graph);
        metrics.memory_usage_bytes.set(memory_usage);
    }
}
