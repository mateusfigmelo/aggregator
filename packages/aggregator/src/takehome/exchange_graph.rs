use std::{
    cmp::Ordering,
    collections::{BinaryHeap, HashMap},
    hash::{Hash, Hasher},
    sync::{Arc, RwLock},
    time::{Duration, Instant},
};

use aggregator_utils::{
    orderbook::OrderbookState,
    types::{Address, Price, Quantity, Side, Swap},
};

use super::event_processor::Error as EventError;

const NUM_SHARDS: usize = 16;

/// Sharded hash map for better concurrency - reduces lock contention
#[derive(Debug)]
struct ShardedHashMap<K, V> {
    shards: Vec<RwLock<HashMap<K, V>>>,
}

impl<K, V> ShardedHashMap<K, V>
where
    K: std::hash::Hash + Eq + Clone,
    V: Clone,
{
    fn new() -> Self {
        let mut shards = Vec::with_capacity(NUM_SHARDS);
        for _ in 0..NUM_SHARDS {
            shards.push(RwLock::new(HashMap::new()));
        }
        Self { shards }
    }

    fn get_shard(&self, key: &K) -> &RwLock<HashMap<K, V>> {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        key.hash(&mut hasher);
        let shard_idx = hasher.finish() as usize % NUM_SHARDS;
        &self.shards[shard_idx]
    }

    fn len(&self) -> Result<usize, EventError> {
        let mut total = 0;
        for shard in &self.shards {
            let guard = shard.read().map_err(|_| EventError::LockError)?;
            total += guard.len();
        }
        Ok(total)
    }

    /// Returns a Vec of all key-value pairs (read-only snapshot)
    /// Uses non-blocking reads to avoid deadlocks
    fn all_items(&self) -> Result<Vec<(K, V)>, EventError> {
        let mut items = Vec::new();

        // Try to acquire all read locks with timeout to avoid deadlocks
        let mut guards = Vec::new();

        for shard in &self.shards {
            let guard = shard.read().map_err(|_| EventError::LockError)?;
            guards.push(guard);
        }

        for guard in guards {
            for (k, v) in guard.iter() {
                items.push((k.clone(), v.clone()));
            }
        }

        Ok(items)
    }

    /// Retain only the elements specified by the predicate
    /// Uses non-blocking writes to avoid deadlocks
    fn retain<F>(&self, mut f: F) -> Result<(), EventError>
    where
        F: FnMut(&K, &mut V) -> bool,
    {
        for shard in &self.shards {
            let mut guard = shard.write().map_err(|_| EventError::LockError)?;
            guard.retain(|k, v| f(k, v));
        }
        Ok(())
    }

    /// Update a value atomically if it exists, or insert a new one
    fn update_or_insert<F, G>(&self, key: K, updater: F, inserter: G) -> Result<(), EventError>
    where
        F: FnOnce(&mut V),
        G: FnOnce() -> V,
    {
        let shard = self.get_shard(&key);
        let mut guard = shard.write().map_err(|_| EventError::LockError)?;
        let value = guard.entry(key).or_insert_with(inserter);
        updater(value);
        Ok(())
    }
}

/// Represents a trading pair with liquidity information
#[derive(Debug, Clone)]
pub struct TradingPair {
    pub base_token: Address,
    pub quote_token: Address,
    pub best_bid: Price,
    pub best_ask: Price,
    pub bid_liquidity: Quantity,
    pub ask_liquidity: Quantity,
    pub last_updated: Instant,
    pub volume_24h: Quantity, // Track volume for route quality scoring
}

impl TradingPair {
    pub fn new(base_token: Address, quote_token: Address) -> Self {
        Self {
            base_token,
            quote_token,
            best_bid: 0,
            best_ask: u64::MAX,
            bid_liquidity: 0,
            ask_liquidity: 0,
            last_updated: Instant::now(),
            volume_24h: 0,
        }
    }

    pub fn update_from_orderbook(&mut self, orderbook: &OrderbookState) {
        self.last_updated = Instant::now();

        // Update bid side
        if let Some(best_bid) = orderbook.bids().first() {
            self.best_bid = best_bid.px;
            self.bid_liquidity = best_bid.sz;
        }

        // Update ask side
        if let Some(best_ask) = orderbook.asks().first() {
            self.best_ask = best_ask.px;
            self.ask_liquidity = best_ask.sz;
        }
    }

    pub fn get_price_for_side(&self, side: Side) -> Option<Price> {
        match side {
            Side::Bid => Some(self.best_bid),
            Side::Ask => {
                if self.best_ask != u64::MAX {
                    Some(self.best_ask)
                } else {
                    None
                }
            }
        }
    }

    pub fn get_liquidity_for_side(&self, side: Side) -> Quantity {
        match side {
            Side::Bid => self.bid_liquidity,
            Side::Ask => self.ask_liquidity,
        }
    }

    pub fn is_stale(&self, max_age: Duration) -> bool {
        self.last_updated.elapsed() > max_age
    }

    /// Calculate price impact for a given trade size
    pub fn calculate_price_impact(&self, trade_size: Quantity, side: Side) -> f64 {
        let liquidity = self.get_liquidity_for_side(side);
        if liquidity == 0 {
            return f64::INFINITY;
        }
        trade_size as f64 / liquidity as f64
    }
}

/// Efficient cache key using a custom struct instead of string
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RouteKey {
    pub input_token: Address,
    pub output_token: Address,
    pub input_amount_bucket: u64, // Quantized amount for better cache hit rates
}

impl RouteKey {
    pub fn new(input_token: Address, output_token: Address, input_amount: Quantity) -> Self {
        // Quantize input amount to reduce cache fragmentation
        // Reduced quantization for more cache entries
        let bucket_size = std::cmp::max(input_amount / 100, 1); // Changed from 1000 to 100
        let quantized_amount = (input_amount / bucket_size) * bucket_size;
        Self {
            input_token,
            output_token,
            input_amount_bucket: quantized_amount,
        }
    }
}

/// A cached route with performance metrics and quality scoring
#[derive(Debug, Clone)]
pub struct CachedRoute {
    pub route: Vec<Swap>,
    pub total_output: Quantity,
    pub last_updated: Instant,
    pub hit_count: u64,
    pub quality_score: f64, // Higher is better
    pub avg_latency_ms: f64,
}

impl CachedRoute {
    pub fn new(route: Vec<Swap>, total_output: Quantity, quality_score: f64) -> Self {
        Self {
            route,
            total_output,
            last_updated: Instant::now(),
            hit_count: 0,
            quality_score,
            avg_latency_ms: 0.0,
        }
    }

    pub fn is_stale(&self, max_age: Duration) -> bool {
        self.last_updated.elapsed() > max_age
    }

    pub fn update_latency(&mut self, latency_ms: f64) {
        // Exponential moving average
        const ALPHA: f64 = 0.1;
        self.avg_latency_ms = ALPHA * latency_ms + (1.0 - ALPHA) * self.avg_latency_ms;
    }
}

/// Node for the A* pathfinding algorithm
#[derive(Debug, Clone)]
struct PathNode {
    token: Address,
    amount: Quantity,
    route: Vec<Swap>,
    cost: f64,     // g-score: actual cost from start to current node
    priority: f64, // f-score: g-score + heuristic (for A* prioritization)
    hops: usize,
}

impl PartialEq for PathNode {
    fn eq(&self, other: &Self) -> bool {
        self.priority == other.priority
    }
}

impl Eq for PathNode {}

impl PartialOrd for PathNode {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for PathNode {
    fn cmp(&self, other: &Self) -> Ordering {
        // Reverse ordering for min-heap (we want lowest priority first)
        other
            .priority
            .partial_cmp(&self.priority)
            .unwrap_or(Ordering::Equal)
    }
}

/// The main exchange graph that manages trading pairs and cached routes
#[derive(Debug)]
pub struct ExchangeGraph {
    /// Trading pairs with sharded access for better concurrency
    pairs: Arc<ShardedHashMap<(Address, Address), TradingPair>>,

    /// Cached routes with sharded access for better concurrency
    cached_routes: Arc<ShardedHashMap<RouteKey, CachedRoute>>,

    /// Performance metrics
    metrics: Arc<RwLock<PerformanceMetrics>>,

    /// Configuration parameters
    config: ExchangeGraphConfig,
}

#[derive(Debug, Clone)]
pub struct ExchangeGraphConfig {
    /// Maximum number of hops to consider in a route
    pub max_hops: usize,

    /// Maximum age for cached routes before they're considered stale
    pub route_cache_ttl: Duration,

    /// Maximum age for trading pair data before it's considered stale
    pub pair_cache_ttl: Duration,

    /// Maximum number of cached routes to keep in memory
    pub max_cached_routes: usize,

    /// Minimum liquidity threshold for considering a trading pair
    pub min_liquidity_threshold: Quantity,

    /// Maximum price impact allowed for a route (in basis points)
    pub max_price_impact_bps: u64,
    /// Weight for liquidity in route quality scoring (0-1.0)
    pub liquidity_weight: f64,
    /// Weight for hop count in route quality scoring (0-1.0)
    pub hop_weight: f64,
    /// Weight for price impact in route quality scoring (0-1.0)
    pub price_impact_weight: f64,
}

impl Default for ExchangeGraphConfig {
    fn default() -> Self {
        Self {
            max_hops: 3,
            route_cache_ttl: Duration::from_secs(120), // Increased from 30s to 2 minutes
            pair_cache_ttl: Duration::from_secs(5),
            max_cached_routes: 50000,     // Increased from 10000 to 50000
            min_liquidity_threshold: 100, // Reduced from 1000 to 100 for more pairs
            max_price_impact_bps: 1000,   // Increased from 500 to 1000 (10%) for more routes
            liquidity_weight: 0.4,
            hop_weight: 0.3,
            price_impact_weight: 0.3,
        }
    }
}

#[derive(Debug, Clone)]
pub struct PerformanceMetrics {
    pub total_requests: u64,
    pub cache_hits: u64,
    pub cache_misses: u64,
    pub avg_response_time_ms: f64,
    pub successful_routes: u64,
    pub failed_routes: u64,
    pub last_reset: Instant,
}

impl Default for PerformanceMetrics {
    fn default() -> Self {
        Self {
            total_requests: 0,
            cache_hits: 0,
            cache_misses: 0,
            avg_response_time_ms: 0.0,
            successful_routes: 0,
            failed_routes: 0,
            last_reset: Instant::now(),
        }
    }
}

impl ExchangeGraph {
    pub fn new(config: ExchangeGraphConfig) -> Self {
        Self {
            pairs: Arc::new(ShardedHashMap::new()),
            cached_routes: Arc::new(ShardedHashMap::new()),
            metrics: Arc::new(RwLock::new(PerformanceMetrics::default())),
            config,
        }
    }

    /// Calculate heuristic estimate for A* pathfinding
    /// This estimates the minimum cost to reach the target token
    fn calculate_heuristic(
        &self,
        current_token: Address,
        target_token: Address,
        current_amount: Quantity,
        pairs: &[(Address, Address, TradingPair)],
    ) -> f64 {
        if current_token == target_token {
            return 0.0; // Already at target
        }

        // Look for direct path to target
        for (base, quote, pair) in pairs.iter() {
            if ((*base == current_token && *quote == target_token)
                || (*quote == current_token && *base == target_token))
                && !pair.is_stale(self.config.pair_cache_ttl)
            {
                // Found direct path, estimate cost based on price impact
                let side = if *base == current_token {
                    Side::Ask
                } else {
                    Side::Bid
                };
                let price_impact = pair.calculate_price_impact(current_amount, side);
                // Cap price impact to reasonable range and add small hop penalty
                let capped_impact = price_impact.min(0.5);
                return (capped_impact + 0.1).min(1.0);
            }
        }

        // No direct path found, use conservative estimate
        // Estimate based on remaining hops and average price impact
        let remaining_hops = self.config.max_hops.saturating_sub(1);
        let estimated_price_impact = 0.01; // Conservative estimate
        let total_estimate =
            (remaining_hops as f64 * estimated_price_impact) + (remaining_hops as f64 * 0.1);
        total_estimate.min(10.0) // Cap at 1.0
    }

    /// Update the exchange graph with new orderbook data
    pub fn update_orderbook(&self, orderbook: OrderbookState) -> Result<(), EventError> {
        let key = (orderbook.base_token, orderbook.quote_token);
        let reverse_key = (orderbook.quote_token, orderbook.base_token);

        tracing::debug!(
            target: "exchange_graph",
            component = "orderbook_update",
            base_token = ?orderbook.base_token,
            quote_token = ?orderbook.quote_token,
            num_bids = orderbook.bids().len(),
            num_asks = orderbook.asks().len(),
            "Updating orderbook"
        );

        // Update forward pair atomically (insert if not exists)
        self.pairs.update_or_insert(
            key,
            |pair| {
                pair.update_from_orderbook(&orderbook);
            },
            || TradingPair::new(orderbook.base_token, orderbook.quote_token),
        )?;

        // Update reverse pair atomically (insert if not exists, inverted prices)
        self.pairs.update_or_insert(
            reverse_key,
            |pair| {
                pair.last_updated = Instant::now();
                if let Some(best_bid) = orderbook.bids().first() {
                    pair.best_ask = best_bid.px;
                    pair.ask_liquidity = best_bid.sz;
                }
                if let Some(best_ask) = orderbook.asks().first() {
                    pair.best_bid = best_ask.px;
                    pair.bid_liquidity = best_ask.sz;
                }
            },
            || TradingPair::new(orderbook.quote_token, orderbook.base_token),
        )?;

        // Invalidate related cached routes
        self.invalidate_related_routes(orderbook.base_token, orderbook.quote_token)?;

        tracing::info!(
            target: "exchange_graph",
            component = "orderbook_updated",
            base_token = ?orderbook.base_token,
            quote_token = ?orderbook.quote_token,
            "Orderbook updated successfully"
        );

        Ok(())
    }

    /// Find the best route for a swap request with performance tracking
    pub fn find_route(
        &self,
        input_token: Address,
        output_token: Address,
        input_amount: Quantity,
        min_output_amount: Quantity,
    ) -> Option<Vec<Swap>> {
        let start_time = Instant::now();
        let mut metrics = self.metrics.write().unwrap();
        metrics.total_requests += 1;
        drop(metrics);

        tracing::info!(
            target: "exchange_graph",
            component = "route_finding",
            input_token = ?input_token,
            output_token = ?output_token,
            input_amount = input_amount,
            min_output_amount = min_output_amount,
            "Starting route finding"
        );

        // First, try to find a cached route
        if let Some(cached_route) = self.get_cached_route(input_token, output_token, input_amount) {
            if cached_route.total_output >= min_output_amount {
                let mut metrics = self.metrics.write().unwrap();
                metrics.cache_hits += 1;
                metrics.successful_routes += 1;
                let latency = start_time.elapsed().as_micros() as f64 / 1000.0;
                metrics.avg_response_time_ms =
                    (metrics.avg_response_time_ms * (metrics.total_requests - 1) as f64 + latency)
                        / metrics.total_requests as f64;

                tracing::info!(
                    target: "exchange_graph",
                    component = "cache_hit",
                    route_hops = cached_route.route.len(),
                    total_output = cached_route.total_output,
                    quality_score = cached_route.quality_score,
                    latency_ms = latency,
                    hit_count = cached_route.hit_count,
                    "Cache hit - route found"
                );

                return Some(cached_route.route.clone());
            } else {
                tracing::warn!(
                    target: "exchange_graph",
                    component = "cache_invalid",
                    cached_output = cached_route.total_output,
                    min_required = min_output_amount,
                    "Cached route output insufficient"
                );
            }
        }

        // If no cached route or it doesn't meet minimum output, find a new route
        tracing::debug!(
            target: "exchange_graph",
            component = "route_search",
            "Searching for new route"
        );

        let route = match self.find_best_route(input_token, output_token, input_amount) {
            Some(route) => route,
            None => {
                let mut metrics = self.metrics.write().unwrap();
                metrics.cache_misses += 1;
                metrics.failed_routes += 1;

                tracing::warn!(
                    target: "exchange_graph",
                    component = "no_route",
                    input_token = ?input_token,
                    output_token = ?output_token,
                    input_amount = input_amount,
                    "No route found between tokens"
                );

                return None;
            }
        };

        // Calculate total output
        let total_output = route.last()?.expected_output_amount;

        if total_output >= min_output_amount {
            // Calculate quality score for the route
            let quality_score = self.calculate_route_quality(&route, input_amount);

            // Cache the successful route
            self.cache_route(
                input_token,
                output_token,
                input_amount,
                route.clone(),
                total_output,
                quality_score,
            );

            let mut metrics = self.metrics.write().unwrap();
            metrics.cache_misses += 1;
            metrics.successful_routes += 1;
            let latency = start_time.elapsed().as_micros() as f64 / 1000.0;
            metrics.avg_response_time_ms =
                (metrics.avg_response_time_ms * (metrics.total_requests - 1) as f64 + latency)
                    / metrics.total_requests as f64;

            tracing::info!(
                target: "exchange_graph",
                component = "route_success",
                route_hops = route.len(),
                total_output = total_output,
                quality_score = quality_score,
                latency_ms = latency,
                "New route found and cached"
            );

            Some(route)
        } else {
            let mut metrics = self.metrics.write().unwrap();
            metrics.cache_misses += 1;
            metrics.failed_routes += 1;

            tracing::warn!(
                target: "exchange_graph",
                component = "route_failure",
                found_output = total_output,
                min_required = min_output_amount,
                route_hops = route.len(),
                "Route found but output insufficient"
            );

            None
        }
    }

    /// Get a cached route if it exists and is not stale
    fn get_cached_route(
        &self,
        input_token: Address,
        output_token: Address,
        input_amount: Quantity,
    ) -> Option<CachedRoute> {
        let key = RouteKey::new(input_token, output_token, input_amount);
        let shard = self.cached_routes.get_shard(&key);
        let mut cached_routes = shard.write().unwrap();

        if let Some(cached_route) = cached_routes.get_mut(&key) {
            if !cached_route.is_stale(self.config.route_cache_ttl) {
                cached_route.hit_count += 1;
                return Some(cached_route.clone());
            } else {
                // Remove stale route
                cached_routes.remove(&key);
            }
        }

        None
    }

    /// Cache a route for future use
    fn cache_route(
        &self,
        input_token: Address,
        output_token: Address,
        input_amount: Quantity,
        route: Vec<Swap>,
        total_output: Quantity,
        quality_score: f64,
    ) {
        let key = RouteKey::new(input_token, output_token, input_amount);
        let shard = self.cached_routes.get_shard(&key);
        let mut cached_routes = shard.write().unwrap();

        // Implement LRU eviction if we exceed max cached routes per shard
        if cached_routes.len() >= self.config.max_cached_routes / NUM_SHARDS {
            // Simple eviction: remove oldest entries
            let oldest_key = cached_routes
                .iter()
                .min_by_key(|(_, route)| route.last_updated)
                .map(|(k, _)| k.clone());

            if let Some(oldest_key) = oldest_key {
                cached_routes.remove(&oldest_key);
            }
        }

        cached_routes.insert(key, CachedRoute::new(route, total_output, quality_score));
    }

    /// Find the best route using A* pathfinding algorithm
    fn find_best_route(
        &self,
        input_token: Address,
        output_token: Address,
        input_amount: Quantity,
    ) -> Option<Vec<Swap>> {
        let pairs_raw = self.pairs.all_items().unwrap();
        let pairs: Vec<(Address, Address, TradingPair)> =
            pairs_raw.into_iter().map(|((a, b), p)| (a, b, p)).collect();

        // Use a priority queue for A* pathfinding
        let mut queue = BinaryHeap::new();
        let mut visited = HashMap::new();

        // Calculate initial heuristic
        let initial_heuristic =
            self.calculate_heuristic(input_token, output_token, input_amount, &pairs);

        // Start with input token
        queue.push(PathNode {
            token: input_token,
            amount: input_amount,
            route: vec![],
            cost: 0.0,
            priority: 0.0 + initial_heuristic, // f = g + h
            hops: 0,
        });

        while let Some(node) = queue.pop() {
            if node.token == output_token {
                return Some(node.route);
            }

            if node.hops >= self.config.max_hops {
                continue;
            }

            // Check if we've already found a better path to this token
            if let Some(&best_amount) = visited.get(&node.token) {
                if best_amount >= node.amount {
                    continue;
                }
            }
            visited.insert(node.token, node.amount);

            // Try all possible next tokens
            for (base, quote, pair) in pairs.iter() {
                if *base == node.token
                    && !pair.is_stale(self.config.pair_cache_ttl)
                    && pair.ask_liquidity >= node.amount
                {
                    let next_amount =
                        self.calculate_output_amount(node.amount, pair.best_ask, Side::Ask);

                    if next_amount > 0 {
                        let price_impact = pair.calculate_price_impact(node.amount, Side::Ask);
                        let hop_penalty = (node.hops as f64 + 1.0) * 0.1;
                        let new_cost = node.cost + price_impact + hop_penalty;

                        // Calculate heuristic for next node
                        let heuristic =
                            self.calculate_heuristic(*quote, output_token, next_amount, &pairs);
                        let priority = new_cost + heuristic; // f = g + h

                        let mut new_route = node.route.clone();
                        new_route.push(Swap {
                            input_token: *base,
                            output_token: *quote,
                            direction: Side::Ask,
                            input_amount: node.amount,
                            expected_output_amount: next_amount,
                        });

                        queue.push(PathNode {
                            token: *quote,
                            amount: next_amount,
                            route: new_route,
                            cost: new_cost,
                            priority,
                            hops: node.hops + 1,
                        });
                    }
                }

                if *quote == node.token
                    && !pair.is_stale(self.config.pair_cache_ttl)
                    && pair.bid_liquidity >= node.amount
                {
                    let next_amount =
                        self.calculate_output_amount(node.amount, pair.best_bid, Side::Bid);

                    if next_amount > 0 {
                        let price_impact = pair.calculate_price_impact(node.amount, Side::Bid);
                        let hop_penalty = (node.hops as f64 + 1.0) * 0.1;
                        let new_cost = node.cost + price_impact + hop_penalty;

                        // Calculate heuristic for next node
                        let heuristic =
                            self.calculate_heuristic(*base, output_token, next_amount, &pairs);
                        let priority = new_cost + heuristic; // f = g + h

                        let mut new_route = node.route.clone();
                        new_route.push(Swap {
                            input_token: *quote,
                            output_token: *base,
                            direction: Side::Bid,
                            input_amount: node.amount,
                            expected_output_amount: next_amount,
                        });

                        queue.push(PathNode {
                            token: *base,
                            amount: next_amount,
                            route: new_route,
                            cost: new_cost,
                            priority,
                            hops: node.hops + 1,
                        });
                    }
                }
            }
        }

        None
    }

    /// Calculate output amount based on input amount and price
    fn calculate_output_amount(
        &self,
        input_amount: Quantity,
        price: Price,
        side: Side,
    ) -> Quantity {
        match side {
            Side::Bid => input_amount.saturating_mul(price) / 1_000_000_000_000_000_000, // Assuming 6 decimal precision
            Side::Ask => input_amount.saturating_mul(1_000_000_000_000_000_000) / price, // Convert to quote token units
        }
    }

    /// Calculate quality score for a route based on multiple factors
    pub fn calculate_route_quality(&self, route: &[Swap], input_amount: Quantity) -> f64 {
        let pairs_raw = self.pairs.all_items().unwrap();
        let pairs: Vec<(Address, Address, TradingPair)> =
            pairs_raw.into_iter().map(|((a, b), p)| (a, b, p)).collect();

        let mut total_liquidity_score = 0.0;
        let mut total_price_impact = 0.0;
        let hop_count = route.len() as f64;

        for swap in route {
            let key = (swap.input_token, swap.output_token);
            if let Some((_, _, pair)) = pairs.iter().find(|(b, q, _)| *b == key.0 && *q == key.1) {
                let liquidity = pair.get_liquidity_for_side(swap.direction);
                // Normalize liquidity score to 0-1 range
                let liquidity_score = (liquidity as f64 / (input_amount as f64)).min(1.0);
                total_liquidity_score += liquidity_score;

                let price_impact = pair.calculate_price_impact(swap.input_amount, swap.direction);
                total_price_impact += price_impact;
            }
        }

        // Normalize scores
        let avg_liquidity_score = total_liquidity_score / hop_count;
        let avg_price_impact = total_price_impact / hop_count;

        // Calculate individual components with proper normalization
        let liquidity_component = self.config.liquidity_weight * avg_liquidity_score.min(1.0);
        let hop_component = self.config.hop_weight * (1.0 / hop_count.max(1.0)); // Fewer hops is better
        let price_impact_component =
            self.config.price_impact_weight * (1.0 / (1.0 + avg_price_impact.min(1.0))); // Lower price impact is better

        // Ensure the sum doesn't exceed 1.0
        (liquidity_component + hop_component + price_impact_component).min(1.0)
    }

    /// Invalidate cached routes that involve the given tokens
    fn invalidate_related_routes(
        &self,
        token1: Address,
        token2: Address,
    ) -> Result<(), EventError> {
        self.cached_routes.retain(|key, _| {
            key.input_token != token1
                && key.input_token != token2
                && key.output_token != token1
                && key.output_token != token2
        })?;
        Ok(())
    }

    /// Clean up stale data periodically
    pub fn cleanup_stale_data(&self) {
        // Clean up stale routes
        if let Err(e) = self
            .cached_routes
            .retain(|_, route| !route.is_stale(self.config.route_cache_ttl))
        {
            tracing::warn!("Failed to cleanup stale routes: {:?}", e);
        }
        // Clean up stale pairs (but keep them for a bit longer to avoid thrashing)
        if let Err(e) = self
            .pairs
            .retain(|_, pair| !pair.is_stale(self.config.pair_cache_ttl.saturating_mul(2)))
        {
            tracing::warn!("Failed to cleanup stale pairs: {:?}", e);
        }
    }

    /// Get statistics about the exchange graph
    pub fn get_stats(&self) -> ExchangeGraphStats {
        let metrics = self.metrics.read().unwrap();
        let stats = ExchangeGraphStats {
            num_trading_pairs: self.pairs.len().unwrap_or(0),
            num_cached_routes: self.cached_routes.len().unwrap_or(0),
            total_route_hits: self
                .cached_routes
                .all_items()
                .unwrap()
                .iter()
                .map(|(_, r)| r.hit_count)
                .sum(),
            cache_hit_rate: if metrics.total_requests > 0 {
                metrics.cache_hits as f64 / metrics.total_requests as f64
            } else {
                0.0
            },
            avg_response_time_ms: metrics.avg_response_time_ms,
            success_rate: if metrics.total_requests > 0 {
                metrics.successful_routes as f64 / metrics.total_requests as f64
            } else {
                0.0
            },
            cache_hits: metrics.cache_hits,
            cache_misses: metrics.cache_misses,
        };

        tracing::debug!(
            target: "exchange_graph",
            component = "stats",
            trading_pairs = stats.num_trading_pairs,
            cached_routes = stats.num_cached_routes,
            total_requests = metrics.total_requests,
            cache_hits = metrics.cache_hits,
            cache_misses = metrics.cache_misses,
            cache_hit_rate = stats.cache_hit_rate,
            success_rate = stats.success_rate,
            avg_response_time_ms = stats.avg_response_time_ms,
            "ExchangeGraph statistics"
        );

        stats
    }

    /// Reset performance metrics
    pub fn reset_metrics(&self) {
        let mut metrics = self.metrics.write().unwrap();
        *metrics = PerformanceMetrics::default();
    }
}

#[derive(Debug, Clone)]
pub struct ExchangeGraphStats {
    pub num_trading_pairs: usize,
    pub num_cached_routes: usize,
    pub total_route_hits: u64,
    pub cache_hit_rate: f64,
    pub avg_response_time_ms: f64,
    pub success_rate: f64,
    pub cache_hits: u64,
    pub cache_misses: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use aggregator_utils::orderbook::OrderbookState;
    use std::time::Duration;

    fn create_test_orderbook(
        base: Address,
        quote: Address,
        bid_price: Price,
        ask_price: Price,
        liquidity: Quantity,
    ) -> OrderbookState {
        let mut orderbook = OrderbookState::new(base, quote);
        orderbook.insert_bid(bid_price, liquidity);
        orderbook.insert_ask(ask_price, liquidity);
        orderbook
    }

    // ============================================================================
    // CORE FUNCTIONALITY TESTS
    // ============================================================================

    mod core_functionality {
        use super::*;

        #[test]
        fn test_heuristic_direct_path() {
            let config = ExchangeGraphConfig::default();
            let graph = ExchangeGraph::new(config);

            let token_a = Address::from_bytes([1u8; 20]);
            let token_b = Address::from_bytes([2u8; 20]);

            // Add a direct trading pair
            let orderbook = create_test_orderbook(
                token_a,
                token_b,
                1_000_000_000_000_000_000,
                1_000_000_000_000_000_000,
                1_000_000,
            );
            graph.update_orderbook(orderbook).unwrap();

            let pairs_raw = graph.pairs.all_items().unwrap();
            let pairs: Vec<(Address, Address, TradingPair)> =
                pairs_raw.into_iter().map(|((a, b), p)| (a, b, p)).collect();
            let heuristic = graph.calculate_heuristic(token_a, token_b, 1000, &pairs);

            // Should be low (good) for direct path
            assert!(heuristic < 0.5, "Direct path should have low heuristic");
        }

        #[test]
        fn test_heuristic_no_direct_path() {
            let config = ExchangeGraphConfig::default();
            let graph = ExchangeGraph::new(config);

            let token_a = Address::from_bytes([1u8; 20]);
            let token_b = Address::from_bytes([2u8; 20]);
            let token_c = Address::from_bytes([3u8; 20]);

            // Add indirect path: A -> C -> B
            let orderbook1 = create_test_orderbook(
                token_a,
                token_c,
                1_000_000_000_000_000_000,
                1_000_000_000_000_000_000,
                1_000_000,
            );
            let orderbook2 = create_test_orderbook(
                token_c,
                token_b,
                1_000_000_000_000_000_000,
                1_000_000_000_000_000_000,
                1_000_000,
            );
            graph.update_orderbook(orderbook1).unwrap();
            graph.update_orderbook(orderbook2).unwrap();

            let pairs_raw = graph.pairs.all_items().unwrap();
            let pairs: Vec<(Address, Address, TradingPair)> =
                pairs_raw.into_iter().map(|((a, b), p)| (a, b, p)).collect();
            let heuristic = graph.calculate_heuristic(token_a, token_b, 1000, &pairs);

            // Should be higher than direct path but still reasonable
            assert!(
                heuristic > 0.0,
                "Direct path should have positive heuristic"
            );
            assert!(heuristic <= 1.0, "Heuristic should be capped at 1");
        }

        #[test]
        fn test_heuristic_same_token() {
            let config = ExchangeGraphConfig::default();
            let graph = ExchangeGraph::new(config);

            let token_a = Address::from_bytes([1u8; 20]);

            let pairs_raw = graph.pairs.all_items().unwrap();
            let pairs: Vec<(Address, Address, TradingPair)> =
                pairs_raw.into_iter().map(|((a, b), p)| (a, b, p)).collect();
            let heuristic = graph.calculate_heuristic(token_a, token_a, 1000, &pairs);

            // Should be 0 token
            assert_eq!(heuristic, 0.0, "Same token should have zero heuristic");
        }

        #[test]
        fn test_find_best_route_direct() {
            let config = ExchangeGraphConfig::default();
            let graph = ExchangeGraph::new(config);

            let token_a = Address::from_bytes([1u8; 20]);
            let token_b = Address::from_bytes([2u8; 20]);

            // Add a direct trading pair
            let orderbook = create_test_orderbook(
                token_a,
                token_b,
                1_000_000_000_000_000_000,
                1_000_000_000_000_000_000,
                1_000_000,
            );
            graph.update_orderbook(orderbook).unwrap();

            let route = graph.find_best_route(token_a, token_b, 1000);
            assert!(route.is_some(), "Should find direct route");

            let route = route.unwrap();
            assert_eq!(route.len(), 1, "Direct route should have 1 hop");
            assert_eq!(route[0].input_token, token_a);
            assert_eq!(route[0].output_token, token_b);
        }

        #[test]
        fn test_find_best_route_indirect() {
            let config = ExchangeGraphConfig::default();
            let graph = ExchangeGraph::new(config);

            let token_a = Address::from_bytes([1u8; 20]);
            let token_b = Address::from_bytes([2u8; 20]);
            let token_c = Address::from_bytes([3u8; 20]);

            // Add indirect path: A -> C -> B with reasonable prices
            let orderbook1 = create_test_orderbook(
                token_a,
                token_c,
                1_000_000_000_000_000_000,
                1_000_000_000_000_000_000,
                1_000_000,
            );
            let orderbook2 = create_test_orderbook(
                token_c,
                token_b,
                1_000_000_000_000_000_000,
                1_000_000_000_000_000_000,
                1_000_000,
            );
            graph.update_orderbook(orderbook1).unwrap();
            graph.update_orderbook(orderbook2).unwrap();

            let route = graph.find_best_route(token_a, token_b, 1000);
            assert!(route.is_some(), "Should find indirect route");

            let route = route.unwrap();
            assert_eq!(route.len(), 2, "Indirect route should have 2 hops");
            assert_eq!(route[0].input_token, token_a);
            assert_eq!(route[0].output_token, token_c);
            assert_eq!(route[1].input_token, token_c);
            assert_eq!(route[1].output_token, token_b);
        }

        #[test]
        fn test_find_best_route_no_path() {
            let config = ExchangeGraphConfig::default();
            let graph = ExchangeGraph::new(config);

            let token_a = Address::from_bytes([1u8; 20]);
            let token_b = Address::from_bytes([2u8; 20]);

            // No trading pairs added
            let route = graph.find_best_route(token_a, token_b, 1000);
            assert!(route.is_none(), "Should not find route when no path exists");
        }

        #[test]
        fn test_find_best_route_max_hops() {
            let config = ExchangeGraphConfig {
                max_hops: 1,
                ..Default::default()
            };
            let graph = ExchangeGraph::new(config);

            let token_a = Address::from_bytes([1u8; 20]);
            let token_b = Address::from_bytes([2u8; 20]);
            let token_c = Address::from_bytes([3u8; 20]);

            // Add path that requires 2 hops: A -> C -> B
            let orderbook1 = create_test_orderbook(
                token_a,
                token_c,
                1_000_000_000_000_000_000,
                1_000_000_000_000_000_000,
                1_000_000,
            );
            let orderbook2 = create_test_orderbook(
                token_c,
                token_b,
                1_000_000_000_000_000_000,
                1_000_000_000_000_000_000,
                1_000_000,
            );
            graph.update_orderbook(orderbook1).unwrap();
            graph.update_orderbook(orderbook2).unwrap();

            let route = graph.find_best_route(token_a, token_b, 1000);
            assert!(route.is_none(), "Should not find route exceeding max hops");
        }
    }

    // ============================================================================
    // CACHING AND PERFORMANCE TESTS
    // ============================================================================

    mod caching_and_performance {
        use super::*;

        #[test]
        fn test_find_route_with_caching() {
            let config = ExchangeGraphConfig::default();
            let graph = ExchangeGraph::new(config);

            let token_a = Address::from_bytes([1u8; 20]);
            let token_b = Address::from_bytes([2u8; 20]);

            // Add a trading pair
            let orderbook = create_test_orderbook(
                token_a,
                token_b,
                1_000_000_000_000_000_000,
                1_000_000_000_000_000_000,
                1_000_000,
            );
            graph.update_orderbook(orderbook).unwrap();

            // First call should miss cache
            let route1 = graph.find_route(token_a, token_b, 1000, 1);
            assert!(route1.is_some(), "Should find route");

            // Second call should hit cache
            let route2 = graph.find_route(token_a, token_b, 1000, 1);
            assert!(route2.is_some(), "Should find cached route");

            let stats = graph.get_stats();
            assert!(stats.cache_hit_rate > 0.0, "Should have cache hits");
        }

        #[test]
        fn test_find_route_min_output_amount() {
            let config = ExchangeGraphConfig::default();
            let graph = ExchangeGraph::new(config);

            let token_a = Address::from_bytes([1u8; 20]);
            let token_b = Address::from_bytes([2u8; 20]);

            // Add a trading pair with specific price
            let orderbook = create_test_orderbook(
                token_a,
                token_b,
                1_000_000_000_000_000_000,
                1_000_000_000_000_000_000,
                1_000_000,
            );
            graph.update_orderbook(orderbook).unwrap();

            // Try with minimum output that's too high (much higher than what we'd get)
            let route = graph.find_route(token_a, token_b, 100, 1_000_000_000_000_000_000);
            assert!(
                route.is_none(),
                "Should not find route when min output is too high"
            );
        }

        #[test]
        fn test_route_quality_scoring() {
            let config = ExchangeGraphConfig::default();
            let graph = ExchangeGraph::new(config);

            let token_a = Address::from_bytes([1u8; 20]);
            let token_b = Address::from_bytes([2u8; 20]);

            // Add a trading pair
            let orderbook = create_test_orderbook(
                token_a,
                token_b,
                1_000_000_000_000_000_000,
                1_000_000_000_000_000_000,
                1_000_000,
            );
            graph.update_orderbook(orderbook).unwrap();

            let route = graph.find_best_route(token_a, token_b, 100).unwrap();
            let quality = graph.calculate_route_quality(&route, 100);

            // Quality should be between 0 and 1
            assert!(quality >= 0.0, "Quality should be non-negative");
            assert!(quality <= 1.0, "Quality should be capped at 1");
        }

        #[test]
        fn test_concurrent_access() {
            use std::sync::Arc;
            use std::thread;

            let config = ExchangeGraphConfig::default();
            let graph = Arc::new(ExchangeGraph::new(config));

            let token_a = Address::from_bytes([1u8; 20]);
            let token_b = Address::from_bytes([2u8; 20]);

            // Add a trading pair
            let orderbook = create_test_orderbook(
                token_a,
                token_b,
                1_000_000_000_000_000_000,
                1_000_000_000_000_000_000,
                1_000_000,
            );
            graph.update_orderbook(orderbook).unwrap();

            // Spawn multiple threads accessing the graph
            let mut handles = vec![];
            for _ in 0..10 {
                let graph_clone = Arc::clone(&graph);

                handles.push(thread::spawn(move || {
                    let route = graph_clone.find_route(token_a, token_b, 100, 1);
                    assert!(route.is_some(), "Should find route in concurrent access");
                }));
            }

            // Wait for all threads
            for handle in handles {
                handle.join().unwrap();
            }
        }

        #[test]
        fn test_stale_data_cleanup() {
            let config = ExchangeGraphConfig {
                route_cache_ttl: Duration::from_millis(1),
                pair_cache_ttl: Duration::from_millis(1),
                ..Default::default()
            };
            let graph = ExchangeGraph::new(config);

            let token_a = Address::from_bytes([1u8; 20]);
            let token_b = Address::from_bytes([2u8; 20]);

            // Add data and cache routes
            let orderbook = create_test_orderbook(
                token_a,
                token_b,
                1_000_000_000_000_000_000,
                1_000_000_000_000_000_000,
                1_000_000,
            );
            graph.update_orderbook(orderbook).unwrap();
            let _route = graph.find_route(token_a, token_b, 1000, 1);

            // Wait for data to become stale
            std::thread::sleep(Duration::from_millis(10));

            // Clean up stale data
            graph.cleanup_stale_data();

            // Refresh the data after cleanup
            let orderbook = create_test_orderbook(
                token_a,
                token_b,
                1_000_000_000_000_000_000,
                1_000_000_000_000_000_000,
                1_000_000,
            );
            graph.update_orderbook(orderbook).unwrap();

            // Should still be able to find routes (data gets refreshed)
            let route = graph.find_route(token_a, token_b, 1000, 1);
            assert!(route.is_some(), "Should find route after cleanup");
        }

        #[test]
        fn test_performance_metrics() {
            let config = ExchangeGraphConfig::default();
            let graph = ExchangeGraph::new(config);

            let token_a = Address::from_bytes([1u8; 20]);
            let token_b = Address::from_bytes([2u8; 20]);

            // Add a trading pair
            let orderbook = create_test_orderbook(
                token_a,
                token_b,
                1_000_000_000_000_000_000,
                1_000_000_000_000_000_000,
                1_000_000,
            );
            graph.update_orderbook(orderbook).unwrap();

            // Make some requests
            for _ in 0..5 {
                let _route = graph.find_route(token_a, token_b, 100, 1);
            }

            let stats = graph.get_stats();
            assert!(stats.total_route_hits > 0, "Should track total route hits");
            assert!(stats.success_rate > 0.0, "Old have successful requests");
        }
    }

    // ============================================================================
    // LIQUIDITY AND PRICE IMPACT TESTS
    // ============================================================================

    mod liquidity_and_price_impact {
        use super::*;

        #[test]
        fn test_route_finding_with_insufficient_liquidity() {
            let config = ExchangeGraphConfig::default();
            let graph = ExchangeGraph::new(config);

            let token_a = Address::from_bytes([1u8; 20]);
            let token_b = Address::from_bytes([2u8; 20]);

            // Add a trading pair with limited liquidity
            let orderbook = create_test_orderbook(token_a, token_b, 1, 1, 10);
            graph.update_orderbook(orderbook).unwrap();

            // Try to trade more than available liquidity
            let route = graph.find_route(token_a, token_b, 20, 1);
            assert!(
                route.is_none(),
                "Should not find route when liquidity is insufficient"
            );
        }

        #[test]
        fn test_route_finding_with_exact_liquidity() {
            let config = ExchangeGraphConfig::default();
            let graph = ExchangeGraph::new(config);

            let token_a = Address::from_bytes([1u8; 20]);
            let token_b = Address::from_bytes([2u8; 20]);

            // Add a trading pair with exact liquidity needed
            let liquidity = 1_000_000;
            let orderbook = create_test_orderbook(token_a, token_b, 1, 1, liquidity);
            graph.update_orderbook(orderbook).unwrap();

            // Try to trade exactly the available liquidity
            let route = graph.find_route(token_a, token_b, liquidity, 1);
            assert!(
                route.is_some(),
                "Should find route when liquidity exactly matches"
            );
        }

        #[test]
        fn test_orderbook_update_changes_route() {
            let config = ExchangeGraphConfig::default();
            let graph = ExchangeGraph::new(config);

            let token_a = Address::from_bytes([1u8; 20]);
            let token_b = Address::from_bytes([2u8; 20]);

            // Add initial orderbook with higher ask price (worse for buyer)
            let orderbook1 =
                create_test_orderbook(token_a, token_b, 1_000_000, 2_000_000, 1_000_000);
            graph.update_orderbook(orderbook1).unwrap();

            let route1 = graph.find_route(token_a, token_b, 1000, 1).unwrap();
            let output1 = route1.last().unwrap().expected_output_amount;

            // Update with lower ask price (better for buyer)
            let orderbook2 =
                create_test_orderbook(token_a, token_b, 1_000_000, 1_000_000, 1_000_000);
            graph.update_orderbook(orderbook2).unwrap();

            let route2 = graph.find_route(token_a, token_b, 1000, 1).unwrap();
            let output2 = route2.last().unwrap().expected_output_amount;

            // Lower ask price should give more output for the same input amount
            assert!(
                output2 > output1,
                "Better price should result in more output"
            );
        }

        #[test]
        fn test_zero_amount_handling() {
            let config = ExchangeGraphConfig::default();
            let graph = ExchangeGraph::new(config);

            let token_a = Address::from_bytes([1u8; 20]);
            let token_b = Address::from_bytes([2u8; 20]);

            // Add a trading pair
            let orderbook = create_test_orderbook(
                token_a,
                token_b,
                1_000_000_000_000_000_000,
                1_000_000_000_000_000_000,
                1_000_000,
            );
            graph.update_orderbook(orderbook).unwrap();

            // Try with zero amount
            let route = graph.find_route(token_a, token_b, 0, 1);
            assert!(
                route.is_none(),
                "Should not find route with zero input amount"
            );
        }
    }

    // ============================================================================
    // CACHE INVALIDATION AND UPDATES TESTS
    // ============================================================================

    mod cache_invalidation_and_updates {
        use super::*;

        #[test]
        fn test_route_invalidation_on_orderbook_update() {
            let config = ExchangeGraphConfig::default();
            let graph = ExchangeGraph::new(config);

            let token_a = Address::from_bytes([1u8; 20]);
            let token_b = Address::from_bytes([2u8; 20]);

            // Add initial orderbook with reasonable price
            let orderbook1 =
                create_test_orderbook(token_a, token_b, 1_000_000, 1_000_000, 1_000_000);
            graph.update_orderbook(orderbook1).unwrap();

            // Cache a route
            let route1 = graph.find_route(token_a, token_b, 1000, 1);
            assert!(route1.is_some(), "Should find route before update");
            let output1 = route1.unwrap().last().unwrap().expected_output_amount;

            // Update orderbook with different price (price 2)
            let orderbook2 =
                create_test_orderbook(token_a, token_b, 2_000_000, 2_000_000, 1_000_000);
            graph.update_orderbook(orderbook2).unwrap();

            // Should get new route with different output
            let route2 = graph.find_route(token_a, token_b, 1000, 1);
            assert!(route2.is_some(), "Should find route after update");
            let output2 = route2.unwrap().last().unwrap().expected_output_amount;

            assert_ne!(
                output1, output2,
                "Route should be invalidated and recalculated"
            );
        }

        #[test]
        fn test_concurrent_orderbook_updates() {
            use std::sync::Arc;
            use std::thread;

            let config = ExchangeGraphConfig::default();
            let graph = Arc::new(ExchangeGraph::new(config));

            let token_a = Address::from_bytes([1u8; 20]);
            let token_b = Address::from_bytes([2u8; 20]);

            // Spawn multiple threads updating orderbooks
            let mut handles = vec![];
            for i in 0..10 {
                let graph_clone = Arc::clone(&graph);
                let price = 10 + i as u64;

                handles.push(thread::spawn(move || {
                    let orderbook =
                        create_test_orderbook(token_a, token_b, price, price, 1_000_000);
                    graph_clone.update_orderbook(orderbook).unwrap();
                }));
            }

            // Wait for all updates
            for handle in handles {
                handle.join().unwrap();
            }

            // Should still be able to find routes
            let route = graph.find_route(token_a, token_b, 1000, 1);
            assert!(
                route.is_some(),
                "Should find route after concurrent updates"
            );
        }
    }

    // ============================================================================
    // CACHE BEHAVIOR AND QUANTIZATION TESTS
    // ============================================================================

    mod cache_behavior_and_quantization {
        use super::*;

        #[test]
        fn test_route_key_quantization() {
            let config = ExchangeGraphConfig::default();
            let graph = ExchangeGraph::new(config);

            let token_a = Address::from_bytes([1u8; 20]);
            let token_b = Address::from_bytes([2u8; 20]);

            // Add a trading pair with good liquidity
            let orderbook = create_test_orderbook(
                token_a,
                token_b,
                1_000_000_000_000_000_000,
                1_000_000_000_000_000_000,
                1_000_000,
            );
            graph.update_orderbook(orderbook).unwrap();

            // Test that similar amounts use the same cache key
            let route1 = graph.find_route(token_a, token_b, 1000, 1);
            assert!(route1.is_some(), "First route should be found");

            let route2 = graph.find_route(token_a, token_b, 11111, 1); // Should use same cache key
            assert!(route2.is_some(), "Second route should be found");
        }

        #[test]
        fn test_metrics_increment_correctly() {
            let config = ExchangeGraphConfig::default();
            let graph = ExchangeGraph::new(config);

            let token_a = Address::from_bytes([1u8; 20]);
            let token_b = Address::from_bytes([2u8; 20]);

            // Add a trading pair with good liquidity
            let orderbook = create_test_orderbook(
                token_a,
                token_b,
                1_000_000_000_000_000_000,
                1_000_000_000_000_000_000,
                1_000_000,
            );
            graph.update_orderbook(orderbook).unwrap();

            // Make successful requests
            let _route1 = graph.find_route(token_a, token_b, 1000, 1);
            let _route2 = graph.find_route(token_a, token_b, 1000, 1); // Should be cache hit

            // Make a failed request (insufficient liquidity)
            let _route3 = graph.find_route(token_a, token_b, 100000000000000, 1);

            let stats = graph.get_stats();
            assert!(stats.num_trading_pairs > 0, "Should have trading pairs");
            assert!(stats.num_cached_routes > 0, "Should have cached routes");
        }

        #[test]
        fn test_cleanup_removes_stale_data() {
            let config = ExchangeGraphConfig {
                route_cache_ttl: Duration::from_millis(1),
                pair_cache_ttl: Duration::from_millis(1),
                ..Default::default()
            };
            let graph = ExchangeGraph::new(config);

            let token_a = Address::from_bytes([1u8; 20]);
            let token_b = Address::from_bytes([2u8; 20]);

            // Add data and cache routes
            let orderbook = create_test_orderbook(
                token_a,
                token_b,
                1_000_000_000_000_000_000,
                1_000_000_000_000_000_000,
                1_000_000,
            );
            graph.update_orderbook(orderbook).unwrap();
            let _route = graph.find_route(token_a, token_b, 1000, 1);

            // Wait for data to become stale
            std::thread::sleep(Duration::from_millis(10));

            let stats_before = graph.get_stats();
            assert!(
                stats_before.num_cached_routes > 0,
                "Should have cached routes before cleanup"
            );

            // Clean up stale data
            graph.cleanup_stale_data();

            let stats_after = graph.get_stats();
            assert_eq!(
                stats_after.num_cached_routes, 0,
                "Stale routes should be cleaned up"
            );
        }
    }
}
