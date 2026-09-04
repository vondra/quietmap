//! Daily flight source interface; provenance is carried into every output flight.

use crate::flight::Flight;
use anyhow::Result;

pub trait FlightSource: Send + Sync {
    fn source_id(&self) -> u8;
    fn read_day(&self, day_str: &str) -> Result<Vec<Flight>>;
}
