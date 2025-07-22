use std::sync::Arc;
use std::time::Duration;

use aggregator::{
    backend::{
        event_thread::{EventThread, EventThreadConfig},
        request_thread::{RequestThread, RequestThreadConfig},
    },
    mock_data::{start_mock_data_generator, MockDataConfig},
    monitoring::{start_metrics_updater, MonitoringServer},
    takehome::{
        event_processor::TakehomeEventProcessor,
        exchange_graph::{ExchangeGraph, ExchangeGraphConfig},
        request_processor::TakehomeRequestProcessor,
    },
};
use aggregator_utils::types::Address;
use async_trait::async_trait;
use tokio_graceful_shutdown::{IntoSubsystem, SubsystemBuilder, SubsystemHandle, Toplevel};

use crate::commands::{Cli, Command, RunCommandArgs};

// Custom error type that implements Send + Sync
#[derive(Debug)]
struct SubsystemError(Box<dyn std::error::Error + Send + Sync>);

impl std::fmt::Display for SubsystemError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for SubsystemError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&*self.0)
    }
}

// Wrapper structs for subsystems
struct MonitoringServerSubsystem {
    server: MonitoringServer,
    port: u16,
}

struct MetricsUpdaterSubsystem {
    metrics: aggregator::metrics::Metrics,
    exchange_graph: Arc<ExchangeGraph>,
}

struct MockDataGeneratorSubsystem {
    exchange_graph: Arc<ExchangeGraph>,
    metrics: aggregator::metrics::Metrics,
    config: MockDataConfig,
}

#[async_trait]
impl IntoSubsystem<SubsystemError> for MonitoringServerSubsystem {
    async fn run(self, _subsys: SubsystemHandle) -> Result<(), SubsystemError> {
        self.server.start(self.port).await.map_err(SubsystemError)
    }
}

#[async_trait]
impl IntoSubsystem<SubsystemError> for MetricsUpdaterSubsystem {
    async fn run(self, _subsys: SubsystemHandle) -> Result<(), SubsystemError> {
        start_metrics_updater(self.metrics, self.exchange_graph).await;
        Ok(())
    }
}

#[async_trait]
impl IntoSubsystem<SubsystemError> for MockDataGeneratorSubsystem {
    async fn run(self, _subsys: SubsystemHandle) -> Result<(), SubsystemError> {
        start_mock_data_generator(self.exchange_graph, self.metrics, self.config)
            .await
            .map_err(SubsystemError)
    }
}

pub async fn entry(cli: Cli) -> Result<(), Box<dyn std::error::Error>> {
    match cli.command {
        Command::Run(args) => run(args).await,
        Command::Metrics(args) => metrics(args).await,
        Command::Health(args) => health(args).await,
        Command::Status(args) => status(args).await,
    }
}

/// Constructs all subcomponents and starts the event loops.
///
/// Should you want to add another thread, it should be defined here as a
/// subsystem similar to backend/event_thread.rs or backend/request_thread.rs
async fn run(args: RunCommandArgs) -> Result<(), Box<dyn std::error::Error>> {
    // Validate arguments to prevent index out of bounds
    if u64::from(args.num_price_levels) < args.num_orderbooks {
        return Err(format!(
            "num_price_levels ({}) must be at least as large as num_orderbooks ({})",
            args.num_price_levels, args.num_orderbooks
        )
        .into());
    }

    let mut tokens = Vec::with_capacity(args.num_tokens as usize);
    for _ in 0..args.num_tokens {
        tokens.push(Address::new_random());
    }

    // Create unique token pairs
    let mut token_pairs = Vec::with_capacity(args.num_tokens as usize);
    'outer: for i in 0..args.num_tokens {
        for j in i + 1..args.num_tokens {
            token_pairs.push((tokens[i as usize], tokens[j as usize]));

            if token_pairs.len() == args.num_orderbooks as usize {
                break 'outer;
            }
        }
    }

    //
    // Don't change the above code!! ^^^^
    //

    // Create a shared ExchangeGraph
    let exchange_graph = Arc::new(ExchangeGraph::new(ExchangeGraphConfig::default()));

    // Initialize monitoring
    let monitoring_server = MonitoringServer::new();
    let metrics = monitoring_server.metrics();

    let takehome_request_processor =
        TakehomeRequestProcessor::new(exchange_graph.clone(), metrics.clone());
    let takehome_event_processor = TakehomeEventProcessor::new(exchange_graph.clone());

    //
    // Dont change the below code!!
    //

    // RequestThread is a thread that simulates incoming swap requests to the aggregator
    let request_thread = RequestThread::new(
        RequestThreadConfig {
            requests_per_second: args.requests_per_second,
            slippage_tolerance_bps: args.slippage_tolerance_bps,
            tokens: tokens.clone(),
        },
        takehome_request_processor,
    );

    // EventThread is a thread that simulates incoming orderbook events to the aggregator
    let event_thread = EventThread::new(
        EventThreadConfig {
            events_per_second: args.events_per_second,
            num_prices: args.num_price_levels as usize,
            pairs: token_pairs.clone(),
        },
        takehome_event_processor,
    );

    // Extract values needed for the closure
    let metrics_port = args.metrics_port;
    let enable_mock_data = args.enable_mock_data;
    let events_per_second = args.events_per_second;
    let requests_per_second = args.requests_per_second;
    let price_volatility = args.price_volatility;
    let liquidity_volatility = args.liquidity_volatility;

    // The main event loop.
    //
    // This loop will run until the user presses SIGINT
    // Any additional threads should be added here as subsystems
    Toplevel::new(move |s| async move {
        s.start(SubsystemBuilder::new(
            "event_thread",
            event_thread.into_subsystem(),
        ));

        s.start(SubsystemBuilder::new(
            "request_thread",
            request_thread.into_subsystem(),
        ));

        // Start monitoring server
        s.start(SubsystemBuilder::new(
            "monitoring_server",
            MonitoringServerSubsystem {
                server: monitoring_server,
                port: metrics_port,
            }
            .into_subsystem(),
        ));

        // Start metrics updater
        s.start(SubsystemBuilder::new(
            "metrics_updater",
            MetricsUpdaterSubsystem {
                metrics: metrics.clone(),
                exchange_graph: exchange_graph.clone(),
            }
            .into_subsystem(),
        ));

        // Start mock data generator if enabled
        if enable_mock_data {
            let mock_config = MockDataConfig {
                events_per_second,
                requests_per_second,
                tokens: tokens.clone(),
                token_pairs: token_pairs.clone(),
                price_volatility,
                liquidity_volatility,
                track_request_metrics: false, // Disable request metrics tracking since real processor handles it
                ..Default::default()
            };

            s.start(SubsystemBuilder::new(
                "mock_data_generator",
                MockDataGeneratorSubsystem {
                    exchange_graph,
                    metrics,
                    config: mock_config,
                }
                .into_subsystem(),
            ));
        }
    })
    .catch_signals()
    .handle_shutdown_requests(Duration::from_secs(10))
    .await
    .map_err(|e| Box::new(e) as Box<dyn std::error::Error>)
}

/// Show metrics from the aggregator
async fn metrics(
    args: crate::commands::MetricsCommandArgs,
) -> Result<(), Box<dyn std::error::Error>> {
    let url = format!("{}/metrics", args.endpoint);
    let response = reqwest::get(&url).await?;

    if response.status().is_success() {
        let metrics_text = response.text().await?;

        if args.json {
            // Convert Prometheus format to JSON (simplified)
            let mut metrics_json = serde_json::Map::new();
            for line in metrics_text.lines() {
                if !line.starts_with('#') && !line.is_empty() {
                    if let Some((name, value)) = line.split_once(' ') {
                        if let Ok(num_value) = value.parse::<f64>() {
                            if let Some(number) = serde_json::Number::from_f64(num_value) {
                                metrics_json
                                    .insert(name.to_string(), serde_json::Value::Number(number));
                            }
                        }
                    }
                }
            }
            println!("{}", serde_json::to_string_pretty(&metrics_json)?);
        } else if let Some(filter) = args.filter {
            for line in metrics_text.lines() {
                if line.contains(&filter) {
                    println!("{}", line);
                }
            }
        } else {
            println!("{}", metrics_text);
        }
    } else {
        eprintln!("Failed to fetch metrics: {}", response.status());
        std::process::exit(1);
    }

    Ok(())
}

/// Health check for the aggregator
async fn health(
    args: crate::commands::HealthCommandArgs,
) -> Result<(), Box<dyn std::error::Error>> {
    let url = format!("{}/health", args.endpoint);
    let response = reqwest::get(&url).await?;

    if response.status().is_success() {
        if args.verbose {
            println!("✅ Aggregator is healthy");
            println!("Status: {}", response.status());
        } else {
            println!("✅ Healthy");
        }
    } else {
        if args.verbose {
            println!("❌ Aggregator is unhealthy");
            println!("Status: {}", response.status());
        } else {
            println!("❌ Unhealthy");
        }
        std::process::exit(1);
    }

    Ok(())
}

/// Show system status and configuration
async fn status(
    args: crate::commands::StatusCommandArgs,
) -> Result<(), Box<dyn std::error::Error>> {
    println!("📊 Aggregator Status");
    println!("==================");

    if args.config {
        println!("\n🔧 Configuration:");
        println!("  - Default metrics port: 9090");
        println!("  - Default events per second: 100");
        println!("  - Default requests per second: 10");
        println!("  - Default tokens: 32");
        println!("  - Default orderbooks: 10");
    }

    if args.resources {
        println!("\n💾 System Resources:");
        // This would typically show memory usage, CPU, etc.
        // For now, just show basic info
        println!("  - Memory usage: Available via metrics endpoint");
        println!("  - Active trading pairs: Available via metrics endpoint");
        println!("  - Cached routes: Available via metrics endpoint");
    }

    if !args.config && !args.resources {
        println!("\nUse --config to show configuration details");
        println!("Use --resources to show system resources");
    }

    Ok(())
}
