# Aggregator Monitoring Setup

This document explains how to set up real-time monitoring for the aggregator using Prometheus, Grafana, Loki, and Promtail.

## Overview

The monitoring setup includes:

- **Prometheus**: Metrics collection and storage
- **Loki**: Log aggregation and storage  
- **Promtail**: Log collection from containers and system
- **Grafana**: Metrics and logs visualization
- **Mock Data Generator**: Realistic trading simulation for testing

## Quick Start

### 1. Start the Monitoring Stack

```bash
# Stop all running services
docker compose down -v
# Start all monitoring services (Prometheus, Grafana, Loki, Promtail)
docker compose up --build -d

# Verify services are running
docker compose ps
```

### 2. Run the Aggregator with Monitoring

```bash
# Basic run with monitoring
cargo run -- run --enable-mock-data

# With custom configuration
cargo run -- run \
  --enable-mock-data \
  --metrics-port 9090 \
  --events-per-second 200 \
  --requests-per-second 50 \
  --price-volatility 0.02 \
  --liquidity-volatility 0.1


```

### 3. Additional Commands

```bash
# Show metrics from the aggregator
cargo run -- metrics
cargo run -- metrics --json
cargo run -- metrics --filter "requests"

# Health check
cargo run -- health
cargo run -- health --verbose

# Show system status
cargo run -- status
cargo run -- status --config
cargo run -- status --resources
```

### 4. Access the Dashboards

- **Grafana**: http://localhost:3000 (admin/admin)
- **Prometheus**: http://localhost:9090
- **Loki**: http://localhost:3100
- **Promtail**: http://localhost:9080 (metrics only)

## Configuration Options

### Command Line Arguments

| Argument | Default | Description |
|----------|---------|-------------|
| `--metrics-port` | 9090 | Port for Prometheus metrics endpoint |
| `--enable-mock-data` | false | Enable realistic mock data generation |
| `--price-volatility` | 0.01 | Price volatility for mock data (1% per second) |
| `--liquidity-volatility` | 0.05 | Liquidity volatility for mock data (5% per second) |

### Environment Variables

| Variable | Description |
|----------|-------------|
| `RUST_LOG` | Log level (e.g., `aggregator=debug`) |

### Logging Configuration

The ExchangeGraph now includes detailed structured logging:

#### Log Targets
- `exchange_graph`: All ExchangeGraph operations
- `route_finding`: Route finding process
- `cache_hit`: Cache operations
- `orderbook_update`: Orderbook updates
- `route_success`: Successful route discoveries
- `route_failure`: Failed route attempts
- `no_route`: When no route is found
- `stats`: Periodic statistics

#### Log Levels
- `info`: Important operations (route found, orderbook updated)
- `debug`: Detailed process information
- `warn`: Issues that don't prevent operation (insufficient output, stale cache)
- `error`: Critical errors

#### Structured Fields
Each log entry includes relevant context:
- `input_token`, `output_token`: Token addresses
- `input_amount`, `min_output_amount`: Trade amounts
- `route_hops`: Number of hops in route
- `total_output`: Expected output amount
- `quality_score`: Route quality (0-1)
- `latency_ms`: Processing time
- `hit_count`: Cache hit count

## Metrics Available

### Request Metrics
- `aggregator_total_requests`: Total requests processed
- `aggregator_successful_requests`: Successful requests
- `aggregator_failed_requests`: Failed requests
- `aggregator_cache_hits`: Cache hits
- `aggregator_cache_misses`: Cache misses

### Performance Metrics
- `aggregator_request_duration_seconds`: Request processing time
- `aggregator_route_finding_duration_seconds`: Route finding time
- `aggregator_orderbook_update_duration_seconds`: Orderbook update time

### System Metrics
- `aggregator_active_trading_pairs`: Number of active trading pairs
- `aggregator_cached_routes`: Number of cached routes
- `aggregator_memory_usage_bytes`: Memory usage

### Route Quality Metrics
- `aggregator_route_quality_score`: Route quality (0-1)
- `aggregator_route_hops`: Number of hops in routes
- `aggregator_price_impact_bps`: Price impact in basis points

### Throughput Metrics
- `aggregator_events_processed`: Total events processed
- `aggregator_events_per_second`: Events per second
- `aggregator_requests_per_second`: Requests per second

## Grafana Dashboards

### Aggregator Overview Dashboard
The main dashboard includes:

1. **Key Metrics**: Request rate, success rate, cache hit rate, active trading pairs
2. **Performance**: Request duration percentiles
3. **Route Quality**: Quality scores and hop distribution
4. **Price Impact**: Price impact analysis
5. **System Health**: Memory usage and throughput

### Aggregator Logs Dashboard
The logs dashboard provides comprehensive visibility into ExchangeGraph operations:

1. **Aggregator Logs**: Single comprehensive log panel showing all ExchangeGraph operations including:
   - Route finding processes
   - Orderbook updates
   - Cache hits and misses
   - Successful route discoveries
   - Warnings and errors
   - All with structured fields and timestamps

### Log Queries
You can use these Loki queries in Grafana:

- `{container = "aggregator-app"}` - All aggregator logs
- `{container = "aggregator-app"} |= "exchange_graph"` - ExchangeGraph operations
- `{container = "aggregator-app"} |= "route_finding"` - Route finding operations
- `{container = "aggregator-app"} |= "cache_hit"` - Cache operations
- `{container = "aggregator-app"} |= "orderbook_update"` - Orderbook updates
- `{container = "aggregator-app"} |= "warn"` - Warning messages
- `{container = "aggregator-app"} |= "error"` - Error messages

## Mock Data Generation

The mock data generator creates realistic trading scenarios:

### Features
- **Realistic Price Movements**: Trends and mean reversion patterns
- **Liquidity Variations**: Dynamic liquidity changes
- **Trade Size Distribution**: Log-normal distribution for realistic trade sizes
- **Market Depth**: Multiple price levels in orderbooks

### Configuration
```rust
MockDataConfig {
    events_per_second: 100,
    requests_per_second: 100,
    price_volatility: 0.01,      // 1% per second
    liquidity_volatility: 0.05,  // 5% per second
    enable_realistic_patterns: true,
}
```