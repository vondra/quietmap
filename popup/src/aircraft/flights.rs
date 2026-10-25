//! The loudest flights and their received spectrum from kept aircraft-box pieces, using Doc 29.
//!
//! The loudest flights of a click (PLAN section 5, as dev4 lists them): the pieces the loudest
//! boxes kept, each computed exactly with the kernel at the receiver; a flight's Lmax is the
//! loudest maximum level of its pieces (Doc 29 Eq. 4-8a), and the list is ranked by it as dev4
//! ranks it (a flight's SEL sums only the pieces kept, so it would rank flights by what happened
//! to be kept).

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
    ClosestPoints, SegmentEmission, SegmentGeometry, closest_points, receiver_impedance_for_db,
    receiver_lmax_db, screened_segment_sel, segment_screening_db,
};
use physics::doc29::thrust::PowerBracket;
use physics::loudness::{A_WEIGHTING_DB, OCTAVE_START};
use std::cmp::Ordering;
use std::collections::HashMap;
use std::collections::hash_map::Entry;
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

/// The list's order: the most Lmax first, then the earliest, then by address.
fn listed_first(a: &LoudFlight, b: &LoudFlight) -> Ordering {
    b.lmax_db
        .total_cmp(&a.lmax_db)
        .then(a.start_unix.cmp(&b.start_unix))
        .then(a.icao.cmp(&b.icao))
}

/// `listed` kept the first ten flights by [`listed_first`] as the flight `key` is read or its Lmax
/// rises: a listed flight only moves up within the ten, another enters in place of the last once
/// it comes before it.
fn keep_listed(
    listed: &mut Vec<(u32, u32)>,
    flights: &HashMap<(u32, u32), LoudFlight>,
    key: (u32, u32),
) {
    if listed.contains(&key) {
        return;
    }
    if listed.len() < FLIGHTS_SHOWN {
        listed.push(key);
        return;
    }
    let last = (0..listed.len())
        .max_by(|&a, &b| listed_first(&flights[&listed[a]], &flights[&listed[b]]))
        .expect("ten listed");
    if listed_first(&flights[&key], &flights[&listed[last]]).is_lt() {
        listed[last] = key;
    }
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

/// Per flight (keyed by address and start) its loudest piece and, listed, its summed SEL.
pub struct FlightTotals {
    flights: HashMap<(u32, u32), LoudFlight>,
    /// The listed flights, the first ten by [`listed_first`], kept as pieces are read: the list's
    /// entry level at each chunk of the search without sorting every flight read.
    listed: Vec<(u32, u32)>,
    /// Every piece read, in the order read, its screening found with the horizons it was read
    /// with, for the listed flights' SEL ([`Self::sum_listed`]): nine in ten pieces read are of
    /// flights never listed (Lleida's light squares, 2026-10-25).
    pieces: Vec<PieceRead>,
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
            listed: Vec::with_capacity(FLIGHTS_SHOWN),
            pieces: Vec::new(),
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
        if self.listed.len() < FLIGHTS_SHOWN {
            return f64::NEG_INFINITY;
        }
        (self.listed.iter())
            .map(|key| self.flights[key].lmax_db)
            .min_by(f64::total_cmp)
            .expect("ten listed")
    }

    /// Reads the pieces of boxes `boxes` (indices into `aircraft`) of `tile`: their Lmax and their
    /// screening by `horizons`, the rest of their SEL left for [`Self::sum_listed`].
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
            let record = aircraft.box_place(index);
            let count = usize::from(record.piece_count).min(self.pieces_per_box);
            for k in 0..count {
                let piece = aircraft.piece(&record, k);
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
                let points = closest_points(geometry.start_m, geometry.end_m);
                let closest = points.on_segment_m;
                let lateral = closest[0].hypot(closest[1]);
                let key = (flight.icao, flight.start_unix);
                self.read_piece(
                    || LoudFlight {
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
                    },
                    receiver_lmax_db(&emission, &points, receiver.altitude_m),
                    (lateral, closest[2]),
                    PieceRead {
                        flight: key,
                        emission,
                        screening: segment_screening_db(&geometry, &points, horizons),
                        closest: points,
                    },
                );
            }
        }
    }

    /// One piece read of its flight (made by `new` at the flight's first), its Lmax `lmax_db`
    /// at `closest` (horizontal distance and height above the receiver).
    fn read_piece(
        &mut self,
        new: impl FnOnce() -> LoudFlight,
        lmax_db: f64,
        closest: (f64, f64),
        piece: PieceRead,
    ) {
        let entry = self.flights.entry(piece.flight);
        let first = matches!(entry, Entry::Vacant(_));
        let flight = entry.or_insert_with(new);
        let rises = lmax_db > flight.lmax_db;
        if rises {
            (flight.lmax_db, flight.closest_m, flight.altitude_m) = (lmax_db, closest.0, closest.1);
        }
        if first || rises {
            keep_listed(&mut self.listed, &self.flights, piece.flight);
        }
        self.pieces.push(piece);
    }

    /// The listed flights' SEL: each sums every piece of it read, in the order read, at a receiver
    /// at `altitude_m`, each screened as it was read (a click's ring by the terrain read so far).
    pub fn sum_listed(&mut self, altitude_m: f64) {
        let mut sums = [f64::NEG_INFINITY; FLIGHTS_SHOWN];
        for piece in &self.pieces {
            let Some(at) = self.listed.iter().position(|&key| key == piece.flight) else {
                continue;
            };
            let sel = screened_segment_sel(&piece.emission, piece.closest, piece.screening);
            let impedance_db = receiver_impedance_for_db(&piece.emission, altitude_m);
            sums[at] = 10.0 * (energy(sums[at]) + energy(sel.sel_db + impedance_db)).log10();
        }
        for (key, sel_db) in self.listed.iter().zip(sums) {
            self.flights
                .get_mut(key)
                .expect("a listed flight was read")
                .sel_db = sel_db;
        }
    }

    /// The listed flights, first to last, their SEL summed by [`Self::sum_listed`].
    pub fn loudest(&self) -> Vec<LoudFlight> {
        let mut listed: Vec<&LoudFlight> =
            self.listed.iter().map(|key| &self.flights[key]).collect();
        listed.sort_by(|a, b| listed_first(a, b));
        listed.into_iter().cloned().collect()
    }
}

/// A piece read, for its flight's SEL once listed ([`FlightTotals::sum_listed`]).
struct PieceRead {
    flight: (u32, u32),
    emission: SegmentEmission,
    closest: ClosestPoints,
    /// Its terrain and building losses ([`segment_screening_db`]).
    screening: [f64; 2],
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

    /// A stored piece of the fallback class at 300 m, east of the receiver, screened by `horizons`.
    fn piece_east(
        totals: &FlightTotals,
        key: (u32, u32),
        east_m: f64,
        horizons: &impl ReceiverHorizons,
    ) -> PieceRead {
        let piece = FlightPiece {
            flight: 0,
            ends: [[0, 0]; 2],
            altitudes_m: [300.0; 2],
            speed_kt: 160.0,
            class: u16::from(FALLBACK_NOISE_CLASS),
            power_code: 0,
            flags: 1,
            period: 0,
        };
        let geometry = SegmentGeometry {
            start_m: [east_m, -500.0, 300.0],
            end_m: [east_m, 500.0, 300.0],
            ground_under_start_m: 0.0,
            ground_under_end_m: 0.0,
        };
        let closest = closest_points(geometry.start_m, geometry.end_m);
        PieceRead {
            flight: key,
            emission: emission_of(&piece, "", &totals.place),
            screening: segment_screening_db(&geometry, &closest, horizons),
            closest,
        }
    }

    fn read(
        totals: &mut FlightTotals,
        (icao, start_unix): (u32, u32),
        (lmax_db, east_m): (f64, f64),
        horizons: &impl ReceiverHorizons,
    ) {
        let piece = piece_east(totals, (icao, start_unix), east_m, horizons);
        let new = || LoudFlight {
            icao,
            callsign: String::new(),
            type_designator: String::new(),
            class: usize::from(FALLBACK_NOISE_CLASS),
            start_unix,
            period: 0,
            sel_db: f64::NEG_INFINITY,
            lmax_db: f64::NEG_INFINITY,
            closest_m: east_m,
            altitude_m: 300.0,
            track: Vec::new(),
        };
        totals.read_piece(new, lmax_db, (east_m, 300.0), piece);
    }

    /// The ten kept as flights are read are the first ten of all of them sorted, and the entry
    /// level the tenth's Lmax, however the flights rise, tie or stay without a level.
    #[test]
    fn the_listed_ten_and_their_entry_level_follow_every_flight_read() {
        let mut totals = FlightTotals::default();
        let mut state = 1u64;
        for _ in 0..3_000 {
            state = (state.wrapping_mul(6_364_136_223_846_793_005))
                .wrapping_add(1_442_695_040_888_963_407);
            let icao = (state >> 40) as u32 % 30;
            let lmax_db = if (state >> 16).is_multiple_of(50) {
                f64::NEG_INFINITY
            } else {
                ((state >> 20) % 100) as f64 / 2.0
            };
            read(&mut totals, (icao, icao % 3), (lmax_db, 100.0), &Hill(0.0));
            let mut all: Vec<&LoudFlight> = totals.flights.values().collect();
            all.sort_by(|a, b| listed_first(a, b));
            all.truncate(FLIGHTS_SHOWN);
            let keys = |flights: &[&LoudFlight]| -> Vec<(u32, u32)> {
                flights
                    .iter()
                    .map(|flight| (flight.icao, flight.start_unix))
                    .collect()
            };
            let loudest = totals.loudest();
            assert_eq!(keys(&loudest.iter().collect::<Vec<_>>()), keys(&all));
            let tenth = all
                .get(FLIGHTS_SHOWN - 1)
                .map_or(f64::NEG_INFINITY, |flight| flight.lmax_db);
            assert_eq!(totals.entry_lmax_db().to_bits(), tenth.to_bits());
        }
    }

    /// Terrain screening by a fixed loss toward every point.
    struct Hill(f64);

    impl ReceiverHorizons for Hill {
        fn terrain_loss_db(&self, _point_m: [f64; 3]) -> f64 {
            self.0
        }

        fn building_loss_db(&self, _point_m: [f64; 3]) -> f64 {
            0.0
        }
    }

    /// A flight listed only in a click's later ring sums every piece it had, in the order read,
    /// each screened by the horizons of the ring that read it, as the click summed each piece as
    /// it was read.
    #[test]
    fn a_flight_listed_late_sums_every_piece_screened_as_it_was_read() {
        let mut totals = FlightTotals::default();
        // The first ring, its terrain screening 3 dB: eleven flights, flight k at Lmax k, so the
        // quietest, flight 0, is not listed.
        for icao in 0..=FLIGHTS_SHOWN as u32 {
            let level = (f64::from(icao), 100.0 * f64::from(icao + 1));
            read(&mut totals, (icao, 0), level, &Hill(3.0));
        }
        totals.sum_listed(0.0);
        assert_eq!(totals.flights[&(0, 0)].sel_db, f64::NEG_INFINITY);
        assert_eq!(totals.entry_lmax_db(), 1.0);
        // The next ring, more terrain read, 9 dB: flight 0 read again, nearer, passes flight 1.
        read(&mut totals, (0, 0), (1.5, 50.0), &Hill(9.0));
        assert_eq!(totals.entry_lmax_db(), 1.5);
        totals.sum_listed(0.0);
        let sel = |east_m: f64, loss_db: f64| {
            let piece = piece_east(&totals, (0, 0), east_m, &Hill(loss_db));
            screened_segment_sel(&piece.emission, piece.closest, piece.screening).sel_db
                + receiver_impedance_for_db(&piece.emission, 0.0)
        };
        let summed = |sels: &[f64]| {
            (sels.iter()).fold(f64::NEG_INFINITY, |sum, &sel| {
                10.0 * (energy(sum) + energy(sel)).log10()
            })
        };
        assert_eq!(
            totals.flights[&(0, 0)].sel_db,
            summed(&[sel(100.0, 3.0), sel(50.0, 9.0)])
        );
        assert_eq!(totals.flights[&(2, 0)].sel_db, summed(&[sel(300.0, 3.0)]));
        let order: Vec<u32> = totals.loudest().iter().map(|flight| flight.icao).collect();
        assert_eq!(order, [10, 9, 8, 7, 6, 5, 4, 3, 2, 0]);
    }
}
