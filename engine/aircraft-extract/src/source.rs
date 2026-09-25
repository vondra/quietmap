//! Daily ADS-B provider interface: provider-tagged whole-aircraft traces with their content receipt.

use crate::provider_receipt::ProviderDayReceipt;
use crate::trace::AircraftTrace;
use anyhow::Result;

/// One provider's archive for one UTC day.
pub struct ProviderDay {
    pub source_id: u8,
    pub traces: Vec<AircraftTrace>,
    pub receipt: ProviderDayReceipt,
}

pub trait FlightSource: Send + Sync {
    fn source_id(&self) -> u8;
    /// Whether this provider has an archive for the day; a day without one is
    /// missing, never observed-empty.
    fn has_day(&self, day_str: &str) -> bool;
    fn read_provider_day(&self, day_str: &str) -> Result<ProviderDay>;
}
