//! The loudest flights of a click (PLAN section 5, as dev4 lists them): the pieces the loudest
//! boxes kept, each computed exactly with the kernel at the receiver; a flight's Lmax is the
//! loudest LAmax of its pieces at their closest points, and the list is ranked by it as dev4
//! ranks it (a flight's SEL sums only the pieces kept, so it would rank flights by what happened
//! to be kept).

use super::boxes::AircraftReceiver;
use physics::doc29::corrections::speed_correction_db;
use physics::doc29::helicopters::helicopter_levels;
use physics::doc29::npd::{class_anchor, is_helicopter_class};
use physics::doc29::screening::ReceiverHorizons;
use physics::doc29::segment::{SegmentEmission, SegmentGeometry, segment_sel_at_receiver};
use physics::doc29::thrust::PowerBracket;
use std::collections::HashMap;
use tiles::aircraft::{Aircraft, FlightPiece};
use tiles::geo::{LocalFrame, TileId};

/// Flights listed per click (dev4 lists ten).
pub const FLIGHTS_SHOWN: usize = 10;

/// One listed flight.
#[derive(Debug, Clone, PartialEq)]
pub struct LoudFlight {
    pub icao: u32,
    pub callsign: String,
    pub type_designator: String,
    pub start_unix: u32,
    pub period: u8,
    /// The flight's SEL at the receiver (dB), summed over its computed pieces only.
    pub sel_db: f64,
    /// Its loudest LAmax (dB) and where: horizontal distance and altitude above the receiver.
    pub lmax_db: f64,
    pub closest_m: f64,
    pub altitude_m: f64,
    /// Its computed pieces, the line on the map: each piece's ends as latitude, longitude (deg)
    /// and altitude above sea level (m).
    pub track: Vec<[[f64; 3]; 2]>,
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).trim().to_string()
}

/// The kernel's emission of a stored piece of a flight with `designator`.
fn emission_of(piece: &FlightPiece, designator: &str) -> SegmentEmission {
    let class = usize::from(piece.class);
    let anchor = class_anchor(class);
    let departure = piece.flags & 1 != 0;
    let descent = piece.flags & 2 != 0;
    SegmentEmission {
        class,
        departure,
        power: PowerBracket::from_code(piece.power_code),
        installation: anchor.installation,
        speed_correction_db: speed_correction_db(anchor.v_ref_kt, piece.speed_kt),
        helicopter_correction_db: if is_helicopter_class(class) {
            helicopter_levels(designator).correction_db(departure, descent)
        } else {
            0.0
        },
    }
}

/// Per flight (keyed by address and start) its summed SEL energy and its loudest piece.
pub struct FlightTotals {
    flights: HashMap<(u32, u32), LoudFlight>,
    /// Pieces read per box: all it keeps, or fewer (the benchmark's trials).
    pieces_per_box: usize,
}

impl Default for FlightTotals {
    fn default() -> Self {
        FlightTotals::reading(usize::MAX)
    }
}

impl FlightTotals {
    /// Totals reading at most `pieces_per_box` kept pieces (the loudest) of a box.
    pub fn reading(pieces_per_box: usize) -> Self {
        FlightTotals {
            flights: HashMap::new(),
            pieces_per_box,
        }
    }

    /// The Lmax a flight must exceed to enter the list: its last one's, or -inf while it is
    /// short.
    pub fn entry_lmax_db(&self) -> f64 {
        if self.flights.len() < FLIGHTS_SHOWN {
            return f64::NEG_INFINITY;
        }
        let mut levels: Vec<f64> = self.flights.values().map(|flight| flight.lmax_db).collect();
        *levels
            .select_nth_unstable_by(FLIGHTS_SHOWN - 1, |a, b| b.total_cmp(a))
            .1
    }

    /// Computes the pieces of boxes `boxes` (indices into `aircraft`) of `tile`.
    #[allow(clippy::too_many_arguments)]
    pub fn add_boxes(
        &mut self,
        aircraft: &Aircraft<'_>,
        tile: TileId,
        boxes: &[usize],
        frame: &LocalFrame,
        receiver: AircraftReceiver,
        horizons: &impl ReceiverHorizons,
    ) {
        for &index in boxes {
            let record = aircraft.aircraft_box(index);
            let first = record.first_piece as usize;
            let count = usize::from(record.piece_count).min(self.pieces_per_box);
            for piece_index in first..first + count {
                let piece = aircraft.piece(piece_index);
                let flight = aircraft.flight(piece.flight as usize);
                let designator = text(&flight.type_designator);
                let emission = emission_of(&piece, &designator);
                let point = |end: usize| {
                    let global = tile.global(piece.ends[end]);
                    let [east, north] = frame.metres_of_steps([global.x as f64, global.y as f64]);
                    [
                        east - receiver.position[0],
                        north - receiver.position[1],
                        piece.altitudes_m[end] - receiver.altitude_m,
                    ]
                };
                let geometry = SegmentGeometry {
                    start_m: point(0),
                    end_m: point(1),
                    ground_under_start_m: record.ground_m - receiver.altitude_m,
                    ground_under_end_m: record.ground_m - receiver.altitude_m,
                };
                let Some(sel) = segment_sel_at_receiver(&emission, &geometry, horizons) else {
                    continue;
                };
                let closest = sel.closest.on_segment_m;
                let lateral = closest[0].hypot(closest[1]);
                let lmax_db = emission.read_npd(lateral.hypot(closest[2])).lamax_db;
                let end_on_map = |end: usize| {
                    let (lat, lon) = tile.to_mercator(piece.ends[end]).to_degrees();
                    [lat, lon, piece.altitudes_m[end]]
                };
                let entry = self
                    .flights
                    .entry((flight.icao, flight.start_unix))
                    .or_insert_with(|| LoudFlight {
                        icao: flight.icao,
                        callsign: text(&flight.callsign),
                        type_designator: designator.clone(),
                        start_unix: flight.start_unix,
                        period: piece.period,
                        sel_db: f64::NEG_INFINITY,
                        lmax_db: f64::NEG_INFINITY,
                        closest_m: lateral,
                        altitude_m: closest[2],
                        track: Vec::new(),
                    });
                entry.track.push([end_on_map(0), end_on_map(1)]);
                let energy = 10f64.powf(entry.sel_db / 10.0) + 10f64.powf(sel.sel_db / 10.0);
                entry.sel_db = 10.0 * energy.log10();
                if lmax_db > entry.lmax_db {
                    (entry.lmax_db, entry.closest_m, entry.altitude_m) =
                        (lmax_db, lateral, closest[2]);
                }
            }
        }
    }

    /// The loudest flights by Lmax.
    pub fn loudest(&self) -> Vec<LoudFlight> {
        let mut flights: Vec<LoudFlight> = self.flights.values().cloned().collect();
        flights.sort_by(|a, b| {
            b.lmax_db
                .total_cmp(&a.lmax_db)
                .then(a.start_unix.cmp(&b.start_unix))
                .then(a.icao.cmp(&b.icao))
        });
        flights.truncate(FLIGHTS_SHOWN);
        flights
    }
}
