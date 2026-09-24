//! Current aircraft prepared-file identities shared by writers and popup readers.

pub const SCHEMA_VERSION: &str = "v16";
/// v3: baseline/increment sampling window stamps; flag bit 6 marks secondary-only rows.
pub const AIRBORNE_CONTRACT: &str = "airborne_segments_z9_v3";
/// v3: baseline/increment sampling window stamps and a `secondary_only` column.
pub const CRUISE_CONTRACT: &str = "cruise_owner_z9_v3";
/// v2: per-airport global movement unions live in the file footer under
/// [`AIRPORT_SUMMARIES_KEY`] instead of a sibling `airport_summary.arrow`.
/// v3: baseline/increment sampling window stamps, a `secondary_only` row
/// column and secondary-only movement unions in place of the GA split.
pub const AIRPORT_TRAFFIC_CONTRACT: &str = "airport_traffic_z9_v3";
/// Schema-metadata key of `airport_traffic.arrow`: a JSON object
/// `airport_key -> AirportSummaryEntry` covering every airport with traffic
/// rows in the file; Stage 2C stamps it after its world reduce.
pub const AIRPORT_SUMMARIES_KEY: &str = "qm_airport_summaries";
pub const SYNTH_AIRPORT_LINES_CONTRACT: &str = "synth_airport_lines_z9_v1";
