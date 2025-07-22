use super::exchange_graph::ExchangeGraph;
use aggregator_utils::types::{SwapRequest, SwapResponse, SwapResponseSuccess};
use async_trait::async_trait;
use std::sync::Arc;
use tracing::{debug, trace};

use crate::metrics::Metrics;
use crate::traits::RequestProcessor;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("No route found for swap")]
    NoRouteFound,
}

#[derive(Debug, Clone)]
pub struct TakehomeRequestProcessor {
    pub exchange_graph: Arc<ExchangeGraph>,
    pub metrics: Metrics,
}

impl TakehomeRequestProcessor {
    /// This constructor is called in cli/entry.rs::run()
    /// Any added configurations can be added there.
    pub fn new(exchange_graph: Arc<ExchangeGraph>, metrics: Metrics) -> Self {
        Self {
            exchange_graph,
            metrics,
        }
    }
}

#[async_trait]
impl RequestProcessor for TakehomeRequestProcessor {
    type Error = Error;

    async fn process_request(&self, request: SwapRequest) -> Result<SwapResponse, Self::Error> {
        trace!(?request, "Processing request");

        // Increment total requests counter
        self.metrics.total_requests.inc();

        let route = self.exchange_graph.find_route(
            request.input_token,
            request.output_token,
            request.input_amount,
            request.min_output_amount,
        );
        match route {
            Some(route_vec) => {
                // Increment successful requests counter
                self.metrics.successful_requests.inc();
                Ok(SwapResponse::Success(SwapResponseSuccess {
                    route: route_vec,
                }))
            }
            None => {
                // Increment failed requests counter
                self.metrics.failed_requests.inc();
                debug!(
                    "No route found for swap from {} to {} (amount: {})",
                    request.input_token, request.output_token, request.input_amount
                );
                // Instead of returning an error, return a success response with empty route
                // This allows the simulation to continue running
                Ok(SwapResponse::Success(SwapResponseSuccess { route: vec![] }))
            }
        }
    }
}
