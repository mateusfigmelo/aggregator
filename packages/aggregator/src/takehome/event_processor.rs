use super::exchange_graph::ExchangeGraph;
use aggregator_utils::orderbook::OrderbookState;
use std::sync::Arc;
use tracing::trace;

use crate::traits::EventProcessor;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Failed to acquire lock on exchange graph")]
    LockError,
    #[error("Failed to update exchange graph: {0}")]
    ExchangeGraphError(String),
}

#[derive(Debug, Clone)]
pub struct TakehomeEventProcessor {
    pub exchange_graph: Arc<ExchangeGraph>,
}

impl TakehomeEventProcessor {
    /// This constructor is called in cli/entry.rs::run()
    /// Any added configurations can be added there.
    pub fn new(exchange_graph: Arc<ExchangeGraph>) -> Self {
        Self { exchange_graph }
    }
}

impl EventProcessor for TakehomeEventProcessor {
    type Error = Error;

    fn process_orderbook(&self, new_orderbook: OrderbookState) -> Result<(), Self::Error> {
        trace!(?new_orderbook, "Processing orderbook");
        self.exchange_graph
            .update_orderbook(new_orderbook)
            .map_err(|e| Error::ExchangeGraphError(format!("{:?}", e)))
    }
}
