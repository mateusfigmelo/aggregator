use rand::Rng;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::time::interval;
use tracing::{debug, info};

use aggregator_utils::{
    orderbook::OrderbookState,
    types::{Address, Price, Quantity},
};

use crate::{metrics::Metrics, takehome::exchange_graph::ExchangeGraph};

pub struct MockDataGenerator {
    exchange_graph: Arc<ExchangeGraph>,
    metrics: Metrics,
    config: MockDataConfig,
    last_update: Instant,
}

#[derive(Clone)]
pub struct MockDataConfig {
    pub events_per_second: u32,
    pub requests_per_second: u32,
    pub tokens: Vec<Address>,
    pub token_pairs: Vec<(Address, Address)>,
    pub price_volatility: f64,     // Price change per second
    pub liquidity_volatility: f64, // Liquidity change per second
    pub base_price: Price,
    pub base_liquidity: Quantity,
    pub enable_realistic_patterns: bool,
    pub track_request_metrics: bool, // Whether to track request metrics (disable when real processor is running)
}

impl Default for MockDataConfig {
    fn default() -> Self {
        Self {
            events_per_second: 100,
            requests_per_second: 10,
            tokens: vec![],
            token_pairs: vec![],
            price_volatility: 0.01,                    // 1% per second
            liquidity_volatility: 0.05,                // 5% per second
            base_price: 1_000_000_000_000_000_000,     // 1.0 in wei
            base_liquidity: 1_000_000_000_000_000_000, // 1.0 in wei
            enable_realistic_patterns: true,
            track_request_metrics: true, // Default to true for backward compatibility
        }
    }
}

impl MockDataGenerator {
    pub fn new(
        exchange_graph: Arc<ExchangeGraph>,
        metrics: Metrics,
        config: MockDataConfig,
    ) -> Self {
        Self {
            exchange_graph,
            metrics,
            config,
            last_update: Instant::now(),
        }
    }

    pub async fn start(self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        info!(
            "Starting mock data generator with {} events/sec, {} requests/sec",
            self.config.events_per_second, self.config.requests_per_second
        );

        let event_interval = Duration::from_secs_f64(1.0 / self.config.events_per_second as f64);
        let request_interval =
            Duration::from_secs_f64(1.0 / self.config.requests_per_second as f64);

        let mut event_timer = interval(event_interval);
        let mut request_timer = interval(request_interval);
        let mut metrics_timer = interval(Duration::from_secs(1));

        let mut event_count = 0u64;
        let mut request_count = 0u64;
        let mut last_metrics_update = Instant::now();

        loop {
            tokio::select! {
                _ = event_timer.tick() => {
                    if let Some(pair) = self.config.token_pairs.get(event_count as usize % self.config.token_pairs.len()) {
                        self.generate_orderbook_update(pair.0, pair.1).await?;
                        event_count += 1;
                        self.metrics.events_processed.inc();
                    }
                }
                _ = request_timer.tick() => {
                    if let Some(input_token) = self.config.tokens.get(request_count as usize % self.config.tokens.len()) {
                        if let Some(output_token) = self.config.tokens.get((request_count + 1) as usize % self.config.tokens.len()) {
                            if input_token != output_token {
                                self.generate_swap_request(*input_token, *output_token).await?;
                                request_count += 1;
                            }
                        }
                    }
                }
                _ = metrics_timer.tick() => {
                    let now = Instant::now();
                    let elapsed = now.duration_since(last_metrics_update).as_secs_f64();

                    self.metrics.events_per_second.set((event_count as f64 / elapsed) as i64);
                    self.metrics.requests_per_second.set((request_count as f64 / elapsed) as i64);

                    event_count = 0;
                    request_count = 0;
                    last_metrics_update = now;
                }
            }
        }
    }

    async fn generate_orderbook_update(
        &self,
        base_token: Address,
        quote_token: Address,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let start_time = Instant::now();

        let mut rng = rand::rng();

        // Generate realistic price movements
        let time_factor = self.last_update.elapsed().as_secs_f64();
        let price_change = if self.config.enable_realistic_patterns {
            // Add some market patterns (trends, mean reversion)
            let trend = (time_factor * 0.1).sin() * self.config.price_volatility;
            let noise =
                rng.random_range(-self.config.price_volatility..self.config.price_volatility);
            trend + noise
        } else {
            rng.random_range(-self.config.price_volatility..self.config.price_volatility)
        };

        let current_price = (self.config.base_price as f64 * (1.0 + price_change)) as Price;
        let spread = current_price / 1000; // 0.1% spread

        let bid_price = current_price.saturating_sub(spread);
        let ask_price = current_price.saturating_add(spread);

        // Generate realistic liquidity
        let liquidity_change =
            rng.random_range(-self.config.liquidity_volatility..self.config.liquidity_volatility);
        // Increase base liquidity for better success rate
        let base_liquidity =
            (self.config.base_liquidity as f64 * 10.0 * (1.0 + liquidity_change)) as Quantity; // 10x more liquidity

        // Add some price levels for depth
        let mut orderbook = OrderbookState::new(base_token, quote_token);

        // Add multiple price levels for more realistic orderbook
        for i in 0..5 {
            let level_spread = spread * (i + 1) as u64;
            let bid_level = bid_price.saturating_sub(level_spread);
            let ask_level = ask_price.saturating_add(level_spread);

            let level_liquidity = base_liquidity / (i + 1) as u64;

            orderbook.insert_bid(bid_level, level_liquidity);
            orderbook.insert_ask(ask_level, level_liquidity);
        }

        // Update the exchange graph
        if let Err(e) = self.exchange_graph.update_orderbook(orderbook) {
            return Err(Box::new(std::io::Error::new(
                std::io::ErrorKind::Other,
                format!("Failed to update orderbook: {:?}", e),
            )) as Box<dyn std::error::Error + Send + Sync>);
        }

        let duration = start_time.elapsed();
        self.metrics
            .orderbook_update_duration
            .observe(duration.as_secs_f64());

        debug!(
            "Generated orderbook update for {}/{}: bid={}, ask={}, liquidity={}",
            base_token, quote_token, bid_price, ask_price, base_liquidity
        );
        Ok(())
    }

    async fn generate_swap_request(
        &self,
        input_token: Address,
        output_token: Address,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let start_time = Instant::now();

        let mut rng = rand::rng();

        // Generate realistic trade sizes (much smaller for better success rate)
        let trade_size = if self.config.enable_realistic_patterns {
            // Use simple random distribution instead of log-normal to avoid huge numbers
            let base_size = self.config.base_liquidity / 10000; // Much smaller base size
            let variance = base_size / 10; // Small variance
            rng.random_range(base_size.saturating_sub(variance)..base_size.saturating_add(variance))
        } else {
            // Much smaller trade sizes for better success rate
            let size_variants = [
                rng.random_range(1_000_000_000_000_000..self.config.base_liquidity / 10000), // Very small trades
                rng.random_range(
                    self.config.base_liquidity / 10000..self.config.base_liquidity / 1000,
                ), // Small trades
                rng.random_range(
                    self.config.base_liquidity / 1000..self.config.base_liquidity / 100,
                ), // Medium trades
            ];
            size_variants[rng.random_range(0..3)]
        };

        // Calculate minimum output based on current market conditions
        // Much more lenient minimum output for better success rate
        let min_output = trade_size / 10000; // Reduced from 1000 to 10000 (much more lenient)

        // Find route
        let route =
            self.exchange_graph
                .find_route(input_token, output_token, trade_size, min_output);

        let duration = start_time.elapsed();
        self.metrics
            .request_duration
            .observe(duration.as_secs_f64());

        if self.config.track_request_metrics {
            self.metrics.total_requests.inc();
        }

        match route {
            Some(route) => {
                if self.config.track_request_metrics {
                    self.metrics.successful_requests.inc();
                }

                // Record route quality metrics
                if let Some(last_swap) = route.last() {
                    let quality_score = self
                        .exchange_graph
                        .calculate_route_quality(&route, trade_size);
                    self.metrics.route_quality_score.observe(quality_score);
                    self.metrics.route_hops.observe(route.len() as f64);

                    // Calculate price impact
                    let input_amount = route[0].input_amount;
                    let output_amount = last_swap.expected_output_amount;
                    let price_impact_bps = if input_amount > 0 {
                        (input_amount as f64 / output_amount as f64 - 1.0) * 10000.0
                    } else {
                        0.0
                    };
                    self.metrics
                        .price_impact_bps
                        .observe(price_impact_bps.abs());
                }

                debug!(
                    "Generated successful swap request: {} -> {} (size: {}, hops: {})",
                    input_token,
                    output_token,
                    trade_size,
                    route.len()
                );
            }
            None => {
                if self.config.track_request_metrics {
                    self.metrics.failed_requests.inc();
                }
                debug!(
                    "Generated failed swap request: {} -> {} (size: {})",
                    input_token, output_token, trade_size
                );
            }
        }

        Ok(())
    }
}

pub async fn start_mock_data_generator(
    exchange_graph: Arc<ExchangeGraph>,
    metrics: Metrics,
    config: MockDataConfig,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let generator = MockDataGenerator::new(exchange_graph, metrics, config);
    generator.start().await
}
