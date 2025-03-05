use aggregator_utils::orderbook::OrderbookState;
use tracing::trace;

use crate::traits::EventProcessor;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    // Add any thiserror errors here
}

#[derive(Debug, Clone, Default)]
pub struct TakehomeEventProcessor {
    // Add any fields here
}

impl TakehomeEventProcessor {
    /// This constructor is called in cli/entry.rs::run()
    /// Any added configurations can be added there.
    pub fn new() -> Self {
        Self {}
    }
}

impl EventProcessor for TakehomeEventProcessor {
    type Error = Error;

    fn process_orderbook(&self, new_orderbook: OrderbookState) -> Result<(), Self::Error> {
        trace!(?new_orderbook, "Processing orderbook");
        //
        // TODO: Do something with the orderbook data!
        //

        Ok(())
    }
}
