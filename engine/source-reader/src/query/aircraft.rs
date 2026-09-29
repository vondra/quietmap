//! Select aircraft batches and validate their sampling windows and airport summaries.

use square_store::store::SquareData;

pub(super) struct AircraftPointQueryData {
    pub(super) airborne_batches: Vec<arrow::record_batch::RecordBatch>,
    pub(super) cruise_batches: Vec<arrow::record_batch::RecordBatch>,
    pub(super) airport_traffic_batches: Vec<arrow::record_batch::RecordBatch>,
    pub(super) airport_summary: crate::aircraft_v6::airport_summary_view::AirportSummaryAccum,
    pub(super) airport_lines_batches: Vec<arrow::record_batch::RecordBatch>,
    /// The one sampling window every opened aircraft file carries; `None`
    /// when no aircraft file lies within reach.
    pub(super) sampling_window: Option<noise_compute::emission::aircraft::SamplingWindow>,
}

impl AircraftPointQueryData {
    pub(super) fn none() -> Self {
        Self {
            airborne_batches: Vec::new(),
            cruise_batches: Vec::new(),
            airport_traffic_batches: Vec::new(),
            airport_summary: Default::default(),
            airport_lines_batches: Vec::new(),
            sampling_window: None,
        }
    }
}

/// Unreadable bytes are an error of the whole query, as for every other layer.
pub(super) fn read_aircraft_batches_of_square_data(
    square_data: &[(grid::Square, &SquareData)],
    lat: f64,
    lng: f64,
) -> Result<AircraftPointQueryData, String> {
    let mut aircraft = AircraftPointQueryData::none();
    // Airborne blocks carry their rows' full-geometry envelope, so the kernel's axis envelope
    // gates them in every owner square. Cruise batches carry synthetic-line envelopes, so the
    // horizontal reach gates them. RecordBatch clones are refcount bumps, not data copies.
    let airborne_gate = airborne_envelope_gate(lat, lng);
    for (_, data) in square_data {
        aircraft
            .airborne_batches
            .extend(data.aircraft_airborne.batches_where(&airborne_gate)?);
        aircraft
            .cruise_batches
            .extend(data.aircraft_cruise.batches_within(
                lat,
                lng,
                noise_compute::emission::aircraft::AIRCRAFT_MAX_HORIZONTAL_REACH_M,
            )?);
        aircraft
            .airport_traffic_batches
            .extend(airport_traffic_batches_near(data, lat, lng)?);
    }
    // The label lookup needs every selected airport-line row, but only when nearby airport
    // traffic exists. Keep the files footer-only for all other clicks.
    if !aircraft.airport_traffic_batches.is_empty() {
        for (_, data) in square_data {
            aircraft
                .airport_lines_batches
                .extend(data.airport_lines.batches_all()?);
        }
    }
    Ok(aircraft)
}

/// Airport traffic's row accept is a planar circle.
fn airport_traffic_batches_near(
    data: &SquareData,
    lat: f64,
    lng: f64,
) -> Result<Vec<arrow::record_batch::RecordBatch>, String> {
    data.aircraft_airport_traffic.batches_within(
        lat,
        lng,
        noise_compute::constants::GROUND_OPS_RUNWAY_MAX_RADIUS,
    )
}

/// Sampling windows and airport summaries. An `Err` is a stamp the reader does not know, so the
/// caller serves the other layers without aircraft.
pub(super) fn read_aircraft_stamps_of_square_data(
    square_data: &[(grid::Square, &SquareData)],
    lat: f64,
    lng: f64,
    aircraft: &mut AircraftPointQueryData,
) -> Result<(), String> {
    for (_, data) in square_data {
        // Footer-only: every opened traffic file must carry current summaries,
        // whether or not its rows are near the click.
        if let Some(schema) = data.aircraft_airport_traffic.schema() {
            aircraft
                .airport_summary
                .merge_square(schema, &airport_traffic_batches_near(data, lat, lng)?)?;
        }
        // Every aircraft file is read from its owner cell. The file stamp is the
        // sampling window even when no row is near; a release mixing windows
        // would divide rows by the wrong day counts.
        for arrow in [
            &data.aircraft_airborne,
            &data.aircraft_cruise,
            &data.aircraft_airport_traffic,
        ] {
            if let Some(schema) = arrow.schema() {
                let window =
                    noise_compute::emission::aircraft::SamplingWindow::from_metadata(
                        schema.metadata(),
                    )
                    .map_err(|error| format!("{}: {error}", arrow.path().display()))?;
                match &aircraft.sampling_window {
                    None => aircraft.sampling_window = Some(window),
                    Some(seen) if *seen != window => {
                        return Err(format!(
                            "{}: sampling window {window:?} differs from {seen:?} of another \
                             aircraft file; mixed aircraft releases",
                            arrow.path().display()
                        ))
                    }
                    Some(_) => {}
                }
            }
        }
    }
    Ok(())
}

/// Share the kernel's f32 envelope rounding and conservative wide-bounds handling.
fn airborne_envelope_gate(lat: f64, lng: f64) -> impl Fn(&arrow_batching::RowBbox) -> bool {
    let envelope = noise_compute::emission::aircraft::AirborneEnvelope::new(lat, lng);
    move |bb| envelope.intersects_bbox(*bb)
}

#[cfg(test)]
mod airborne_gate_tests {
    #[test]
    fn corner_batch_passes_axis_envelope_but_not_a_circle() {
        let keep = super::airborne_envelope_gate(0.0, 0.0);
        // A bbox ~20.4 km away point-to-point has both
        // axis distances ~14.4 km < 16 km — the kernel's row filter keeps
        // such rows, so the batch gate must too.
        let bb = [0.13, 0.13, 0.14, 0.14];
        assert!(keep(&bb));
        assert!(arrow_batching::point_to_bbox_distance_m(0.0, 0.0, &bb) > 16_000.0);
        // Far on both axes → pruned.
        assert!(!keep(&[1.0, 1.0, 1.1, 1.1]));
    }

    #[test]
    fn antimeridian_receiver_prunes_distant_longitudes_but_keeps_crossing_batches() {
        let keep = super::airborne_envelope_gate(0.0, 179.95);
        assert!(keep(&[0.0, -179.99, 0.1, -179.98]));
        assert!(!keep(&[0.0, -81.0, 0.1, -80.0]));
        assert!(!keep(&[5.0, -179.99, 5.1, -179.98]));
        assert!(super::airborne_envelope_gate(0.001, 179.5)(&[
            0.0, -179.0, 0.0, 179.0
        ]));
    }
}
