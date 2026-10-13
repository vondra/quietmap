//! The loudest flights and their received spectrum from kept aircraft-box pieces, using Doc 29.

use super::boxes::AircraftReceiver;
use physics::bands::energy;
use physics::doc29::atmosphere::{
    PlaceAtmosphere, SHIFT_DISTANCES, SPECTRUM_DISTANCE_M, THIRD_OCTAVES, class_spectrum_energy_at,
};
use physics::doc29::corrections::speed_correction_db;
use physics::doc29::helicopters::helicopter_levels;
use physics::doc29::npd::{class_anchor, is_helicopter_class};
use physics::doc29::profiles_generated::FALLBACK_NOISE_CLASS;
use physics::doc29::screening::ReceiverHorizons;
use physics::doc29::segment::{
    SegmentEmission, SegmentGeometry, receiver_impedance_for_db, receiver_lmax_db,
    segment_sel_at_receiver,
};
use physics::doc29::thrust::PowerBracket;
use physics::loudness::{A_WEIGHTING_DB, OCTAVE_START};
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
    /// The noise class it flies (Stage 1 decides it once a flight; its spectrum in the Nden).
    pub class: usize,
    pub start_unix: u32,
    pub period: u8,
    /// The flight's SEL at the receiver (dB), summed over its computed pieces only.
    pub sel_db: f64,
    /// Its loudest maximum level (dB) and where: horizontal distance and altitude above the
    /// receiver.
    pub lmax_db: f64,
    pub closest_m: f64,
    pub altitude_m: f64,
    /// Its line on the map near the click ([`super::tracks`]): points as latitude and longitude
    /// (deg); the final answer's only.
    pub track: Vec<Vec<[f64; 2]>>,
}

/// The flights' received spectrum for the loudness (unweighted third-octave energies up to a
/// constant): the listed flights' spectral classes at their slants in the place's air
/// (`rates_db_per_m`), each by its sound exposure, as their energy adds (one loud light aircraft
/// among airliners no longer lends them its spectrum). A helicopter, which Doc 29 gives none, takes
/// the fallback class's, as do the flights where none is listed, so their energy always keeps a
/// spectrum.
pub fn heard_spectrum_energy(
    listed: &[LoudFlight],
    rates_db_per_m: &[f64; THIRD_OCTAVES],
) -> [f64; THIRD_OCTAVES] {
    let fallback = usize::from(FALLBACK_NOISE_CLASS);
    let spectrum = |class: usize, slant_m: f64| {
        class_spectrum_energy_at(class, slant_m, rates_db_per_m)
            .or_else(|| class_spectrum_energy_at(fallback, slant_m, rates_db_per_m))
            .expect("the fallback class has a spectrum")
    };
    if listed.is_empty() {
        return spectrum(fallback, SPECTRUM_DISTANCE_M);
    }
    let weighting = std::array::from_fn::<_, THIRD_OCTAVES, _>(|n| {
        energy(A_WEIGHTING_DB[n + OCTAVE_START - 1])
    });
    let mut spectrum_energy = [0.0; THIRD_OCTAVES];
    for flight in listed {
        let own = spectrum(flight.class, flight.closest_m.hypot(flight.altitude_m));
        // Its shape set to its A-weighted sound exposure.
        let a_weighted: f64 = own
            .iter()
            .zip(weighting)
            .map(|(value, weight)| value * weight)
            .sum();
        let scale = energy(flight.sel_db) / a_weighted;
        for (total, value) in spectrum_energy.iter_mut().zip(own) {
            *total += scale * value;
        }
    }
    spectrum_energy
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).trim().to_string()
}

/// The kernel's emission of a stored piece of a flight with `designator`, in the atmosphere of
/// `place`.
fn emission_of(piece: &FlightPiece, designator: &str, place: &PlaceAtmosphere) -> SegmentEmission {
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
        atmosphere_shift_db: [0.0; SHIFT_DISTANCES],
    }
    .in_atmosphere(place)
}

/// Per flight (keyed by address and start) its summed SEL energy and its loudest piece.
pub struct FlightTotals {
    flights: HashMap<(u32, u32), LoudFlight>,
    /// Pieces read per box: all it keeps, or fewer (the benchmark's trials).
    pieces_per_box: usize,
    /// The receiver's yearly atmosphere, as the boxes' pieces were summed in theirs.
    place: PlaceAtmosphere,
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
            place: PlaceAtmosphere::model(),
        }
    }

    /// The totals in the atmosphere of a place with octave-band absorption `alpha_db_per_km`.
    pub fn in_atmosphere(self, alpha_db_per_km: &[f64; physics::bands::BANDS]) -> Self {
        FlightTotals {
            place: PlaceAtmosphere::new(alpha_db_per_km),
            ..self
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
                let emission = emission_of(&piece, &designator, &self.place);
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
                let sel = segment_sel_at_receiver(&emission, &geometry, horizons);
                let impedance_db = receiver_impedance_for_db(&emission, receiver.altitude_m);
                let closest = sel.closest.on_segment_m;
                let lateral = closest[0].hypot(closest[1]);
                let lmax_db = receiver_lmax_db(&emission, &sel.closest, receiver.altitude_m);
                let entry = self
                    .flights
                    .entry((flight.icao, flight.start_unix))
                    .or_insert_with(|| LoudFlight {
                        icao: flight.icao,
                        callsign: text(&flight.callsign),
                        type_designator: designator.clone(),
                        class: usize::from(piece.class),
                        start_unix: flight.start_unix,
                        period: piece.period,
                        sel_db: f64::NEG_INFINITY,
                        lmax_db: f64::NEG_INFINITY,
                        closest_m: lateral,
                        altitude_m: closest[2],
                        track: Vec::new(),
                    });
                let summed_energy = energy(entry.sel_db) + energy(sel.sel_db + impedance_db);
                entry.sel_db = 10.0 * summed_energy.log10();
                if lmax_db > entry.lmax_db {
                    (entry.lmax_db, entry.closest_m, entry.altitude_m) =
                        (lmax_db, lateral, closest[2]);
                }
            }
        }
    }

    /// The loudest flights by Lmax.
    pub fn loudest(&self) -> Vec<LoudFlight> {
        let mut flights: Vec<&LoudFlight> = self.flights.values().collect();
        flights.sort_by(|a, b| {
            b.lmax_db
                .total_cmp(&a.lmax_db)
                .then(a.start_unix.cmp(&b.start_unix))
                .then(a.icao.cmp(&b.icao))
        });
        flights.truncate(FLIGHTS_SHOWN);
        flights.into_iter().cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use physics::atmosphere::ALPHA_DB_PER_KM;
    use physics::doc29::atmosphere::place_rates_db_per_m;
    use physics::doc29::profiles_generated::NUM_CLASSES;

    #[test]
    fn a_helicopter_or_no_listed_flight_keeps_the_fallback_spectrum() {
        let rates = place_rates_db_per_m(&ALPHA_DB_PER_KM);
        let flight = |class| LoudFlight {
            icao: 1,
            callsign: String::new(),
            type_designator: String::new(),
            class,
            start_unix: 0,
            period: 0,
            sel_db: 80.0,
            lmax_db: 70.0,
            closest_m: 300.0,
            altitude_m: 400.0,
            track: Vec::new(),
        };
        let fallback = usize::from(FALLBACK_NOISE_CLASS);
        let own = |class| class_spectrum_energy_at(class, 500.0, &rates);
        let helicopter = (0..NUM_CLASSES)
            .find(|&class| own(class).is_none())
            .unwrap();
        assert!(is_helicopter_class(helicopter));
        // A single flight keeps its spectral energy ratios.
        let same_shape = |a: [f64; THIRD_OCTAVES], b: [f64; THIRD_OCTAVES]| {
            (0..THIRD_OCTAVES).all(|n| ((a[n] / b[n]) / (a[0] / b[0]) - 1.0).abs() < 1e-9)
        };
        assert!(same_shape(
            heard_spectrum_energy(&[flight(helicopter)], &rates),
            own(fallback).unwrap()
        ));
        let jet = (0..NUM_CLASSES)
            .find(|&class| {
                own(class).is_some_and(|shape| !same_shape(shape, own(fallback).unwrap()))
            })
            .unwrap();
        assert!(same_shape(
            heard_spectrum_energy(&[flight(jet)], &rates),
            own(jet).unwrap()
        ));
        // Two by their exposure: the jet 10 dB louder in SEL weighs ten times the fallback's.
        let mut loud = flight(jet);
        loud.sel_db += 10.0;
        let mixed = heard_spectrum_energy(&[loud, flight(fallback)], &rates);
        assert!(!same_shape(mixed, own(jet).unwrap()));
        let unlisted = class_spectrum_energy_at(fallback, SPECTRUM_DISTANCE_M, &rates).unwrap();
        assert_eq!(heard_spectrum_energy(&[], &rates), unlisted);
    }
}
