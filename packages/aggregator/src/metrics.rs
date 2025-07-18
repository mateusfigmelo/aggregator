use prometheus_client::{
    encoding::text::encode,
    metrics::{counter::Counter, gauge::Gauge, histogram::Histogram},
    registry::Registry,
};

#[derive(Clone, Debug)]
pub struct Metrics {
    // Request metrics
    pub total_requests: Counter,
    pub successful_requests: Counter,
    pub failed_requests: Counter,
    pub cache_hits: Counter,
    pub cache_misses: Counter,

    // Performance metrics
    pub request_duration: Histogram,
    pub route_finding_duration: Histogram,
    pub orderbook_update_duration: Histogram,

    // System metrics
    pub active_trading_pairs: Gauge<i64>,
    pub cached_routes: Gauge<i64>,
    pub memory_usage_bytes: Gauge<i64>,

    // Route quality metrics
    pub route_quality_score: Histogram,
    pub route_hops: Histogram,
    pub price_impact_bps: Histogram,

    // Event processing metrics
    pub events_processed: Counter,
    pub events_per_second: Gauge<i64>,
    pub requests_per_second: Gauge<i64>,
}

impl Metrics {
    pub fn new(registry: &mut Registry) -> Self {
        let total_requests = Counter::default();
        let successful_requests = Counter::default();
        let failed_requests = Counter::default();
        let cache_hits = Counter::default();
        let cache_misses = Counter::default();

        let request_duration = Histogram::new(
            [
                0.001, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0,
            ]
            .into_iter(),
        );
        let route_finding_duration = Histogram::new(
            [
                0.001, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0,
            ]
            .into_iter(),
        );
        let orderbook_update_duration = Histogram::new(
            [
                0.0001, 0.0005, 0.001, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5,
            ]
            .into_iter(),
        );

        let active_trading_pairs = Gauge::default();
        let cached_routes = Gauge::default();
        let memory_usage_bytes = Gauge::default();

        let route_quality_score =
            Histogram::new([0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8, 0.9, 1.0].into_iter());
        let route_hops =
            Histogram::new([1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0].into_iter());
        let price_impact_bps = Histogram::new(
            [
                1.0, 5.0, 10.0, 25.0, 50.0, 100.0, 250.0, 500.0, 1000.0, 2500.0,
            ]
            .into_iter(),
        );

        let events_processed = Counter::default();
        let events_per_second = Gauge::default();
        let requests_per_second = Gauge::default();

        // Register metrics with the registry
        registry.register(
            "aggregator_total_requests",
            "Total number of swap requests processed",
            total_requests.clone(),
        );
        registry.register(
            "aggregator_successful_requests",
            "Number of successful swap requests",
            successful_requests.clone(),
        );
        registry.register(
            "aggregator_failed_requests",
            "Number of failed swap requests",
            failed_requests.clone(),
        );
        registry.register(
            "aggregator_cache_hits",
            "Number of cache hits",
            cache_hits.clone(),
        );
        registry.register(
            "aggregator_cache_misses",
            "Number of cache misses",
            cache_misses.clone(),
        );

        registry.register(
            "aggregator_request_duration_seconds",
            "Request processing duration in seconds",
            request_duration.clone(),
        );
        registry.register(
            "aggregator_route_finding_duration_seconds",
            "Route finding duration in seconds",
            route_finding_duration.clone(),
        );
        registry.register(
            "aggregator_orderbook_update_duration_seconds",
            "Orderbook update duration in seconds",
            orderbook_update_duration.clone(),
        );

        registry.register(
            "aggregator_active_trading_pairs",
            "Number of active trading pairs",
            active_trading_pairs.clone(),
        );
        registry.register(
            "aggregator_cached_routes",
            "Number of cached routes",
            cached_routes.clone(),
        );
        registry.register(
            "aggregator_memory_usage_bytes",
            "Memory usage in bytes",
            memory_usage_bytes.clone(),
        );

        registry.register(
            "aggregator_route_quality_score",
            "Route quality score (0-1)",
            route_quality_score.clone(),
        );
        registry.register(
            "aggregator_route_hops",
            "Number of hops in routes",
            route_hops.clone(),
        );
        registry.register(
            "aggregator_price_impact_bps",
            "Price impact in basis points",
            price_impact_bps.clone(),
        );

        registry.register(
            "aggregator_events_processed",
            "Total number of events processed",
            events_processed.clone(),
        );
        registry.register(
            "aggregator_events_per_second",
            "Events processed per second",
            events_per_second.clone(),
        );
        registry.register(
            "aggregator_requests_per_second",
            "Requests processed per second",
            requests_per_second.clone(),
        );

        Self {
            total_requests,
            successful_requests,
            failed_requests,
            cache_hits,
            cache_misses,
            request_duration,
            route_finding_duration,
            orderbook_update_duration,
            active_trading_pairs,
            cached_routes,
            memory_usage_bytes,
            route_quality_score,
            route_hops,
            price_impact_bps,
            events_processed,
            events_per_second,
            requests_per_second,
        }
    }
}

pub fn encode_metrics(registry: &Registry) -> Result<String, Box<dyn std::error::Error>> {
    let mut buffer = String::new();
    encode(&mut buffer, registry)?;
    Ok(buffer)
}
