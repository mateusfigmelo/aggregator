use clap::{Args, Parser, Subcommand};

#[derive(Debug, Parser)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Run the aggregator with monitoring
    Run(RunCommandArgs),
    /// Show metrics from the aggregator
    Metrics(MetricsCommandArgs),
    /// Health check for the aggregator
    Health(HealthCommandArgs),
    /// Show system status and configuration
    Status(StatusCommandArgs),
}

#[derive(Debug, Args)]
pub struct RunCommandArgs {
    #[arg(
        long,
        global = true,
        default_value = "32",
        help = "The number of tokens to simulate"
    )]
    pub num_tokens: u64,

    #[arg(
        long,
        global = true,
        default_value = "10",
        help = "The number of orderbooks to simulate"
    )]
    pub num_orderbooks: u64,

    #[arg(
        long,
        global = true,
        default_value = "100",
        help = "The number of events to simulate per second"
    )]
    pub events_per_second: u32,

    #[arg(
        long,
        global = true,
        default_value = "10",
        help = "The number of swap requests to simulate per second"
    )]
    pub requests_per_second: u32,

    #[arg(long, global = true, default_value = "50")]
    pub slippage_tolerance_bps: u16,

    #[arg(long, global = true, default_value = "10")]
    pub num_price_levels: u16,

    // Monitoring options
    #[arg(
        long,
        global = true,
        default_value = "9090",
        help = "Port for Prometheus metrics endpoint"
    )]
    pub metrics_port: u16,

    #[arg(
        long,
        global = true,
        help = "Enable mock data generation for realistic metrics"
    )]
    pub enable_mock_data: bool,

    #[arg(
        long,
        global = true,
        default_value = "0.01",
        help = "Price volatility for mock data (0.01 = 1% per second)"
    )]
    pub price_volatility: f64,

    #[arg(
        long,
        global = true,
        default_value = "0.05",
        help = "Liquidity volatility for mock data (0.05 = 5% per second)"
    )]
    pub liquidity_volatility: f64,
}

#[derive(Debug, Args)]
pub struct MetricsCommandArgs {
    #[arg(
        long,
        default_value = "http://localhost:9000",
        help = "Aggregator metrics endpoint URL"
    )]
    pub endpoint: String,

    #[arg(long, help = "Show metrics in JSON format")]
    pub json: bool,

    #[arg(long, help = "Filter metrics by name (supports regex)")]
    pub filter: Option<String>,
}

#[derive(Debug, Args)]
pub struct HealthCommandArgs {
    #[arg(
        long,
        default_value = "http://localhost:9000",
        help = "Aggregator health endpoint URL"
    )]
    pub endpoint: String,

    #[arg(long, help = "Show detailed health information")]
    pub verbose: bool,
}

#[derive(Debug, Args)]
pub struct StatusCommandArgs {
    #[arg(long, help = "Show configuration details")]
    pub config: bool,

    #[arg(long, help = "Show system resources usage")]
    pub resources: bool,
}
