use aggregator_utils::types::{SwapRequest, SwapResponse};
use async_trait::async_trait;
use tracing::trace;

use crate::traits::RequestProcessor;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("No route found for swap")]
    NoRouteFound,
    // Add any thiserror errors here
}

#[derive(Debug, Clone, Default)]
pub struct TakehomeRequestProcessor {
    // Add any fields here
}

impl TakehomeRequestProcessor {
    /// This constructor is called in cli/entry.rs::run()
    /// Any added configurations can be added there.
    pub fn new() -> Self {
        Self {}
    }
}

#[async_trait]
impl RequestProcessor for TakehomeRequestProcessor {
    type Error = Error;

    async fn process_request(&self, request: SwapRequest) -> Result<SwapResponse, Self::Error> {
        trace!(?request, "Processing request");

        //
        // TODO: Do something with the request data!
        //
        // Err(Error::NoRouteFound)
        Ok(SwapResponse::Failure("No implementation".to_string()))
    }
}
