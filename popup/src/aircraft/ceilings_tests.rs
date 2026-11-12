//! A box's ceiling over an area against what `tile_flight_sums` sums of it at the area's receivers.

use super::*;
use crate::aircraft::boxes::{AircraftReceiver, box_at};
use physics::doc29::npd::{METRES_PER_FOOT, NPD_DISTANCES, NPD_DISTANCES_FT};
use physics::doc29::screening::Unscreened;
use tiles::aircraft::BoxPlace;
use tiles::geo::Mercator;

/// SplitMix64: the same boxes and receivers every run.
struct Random(u64);

impl Random {
    fn between(&mut self, low: f64, high: f64) -> f64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        low + (high - low) * ((z ^ (z >> 31)) as f64 / u64::MAX as f64)
    }
}

/// A box with curves of a jet's shape (a level falling 18-24 dB a decade, a bump of up to 2 dB
/// at times), a period silent at times, fixed wing or a helicopter.
fn random_box(random: &mut Random) -> AircraftBox {
    let period_box = |random: &mut Random| -> ([f64; NPD_DISTANCES], f64) {
        if random.between(0.0, 1.0) < 0.2 {
            return ([f64::NEG_INFINITY; NPD_DISTANCES], f64::NEG_INFINITY);
        }
        let (start, slope) = (random.between(70.0, 105.0), random.between(18.0, 24.0));
        let bump = random.between(0.0, NPD_DISTANCES as f64) as usize;
        let levels = std::array::from_fn(|k| {
            start - slope * (NPD_DISTANCES_FT[k] / 200.0).log10()
                + if k == bump {
                    random.between(0.0, 2.0)
                } else {
                    0.0
                }
        });
        (
            levels,
            levels[NPD_DISTANCES - 1] - random.between(7.0, 20.0),
        )
    };
    let periods: [([f64; NPD_DISTANCES], f64); PERIODS] =
        std::array::from_fn(|_| period_box(random));
    let shares = |random: &mut Random| {
        let [wing, fuselage] = [random.between(0.0, 1.0), random.between(0.0, 1.0)];
        let total = wing + fuselage + random.between(0.0, 1.0);
        [
            wing / total,
            fuselage / total,
            1.0 - (wing + fuselage) / total,
        ]
    };
    let ground_m = random.between(0.0, 600.0);
    AircraftBox {
        place: BoxPlace {
            zoom: 14,
            cell: [0, 0],
            group: if random.between(0.0, 1.0) < 0.15 {
                Group::Helicopter
            } else {
                Group::FixedWing
            },
            ground_m,
            clearance_m: 0.0,
            height_m: 150.0,
            centroid: [
                random.between(-30_000.0, 30_000.0) as i16,
                random.between(-30_000.0, 30_000.0) as i16,
            ],
            first_piece: 0,
            piece_count: 0,
            loudest_lamax_db: f64::NEG_INFINITY,
        },
        centroid_altitude_m: ground_m + random.between(30.0, 9_000.0),
        axis_rad: random.between(0.0, std::f64::consts::PI),
        gradient: random.between(-0.1, 0.1),
        gradient_spread: if random.between(0.0, 1.0) < 0.5 {
            0.0
        } else {
            random.between(0.0, 0.06)
        },
        piece_length_m: std::array::from_fn(|_| random.between(0.0, 2_000.0)),
        flights_per_day: std::array::from_fn(|_| random.between(0.0, 300.0)),
        energy_db: periods.map(|(levels, _)| levels),
        tail_energy_db: periods.map(|(_, tail)| tail),
        lg_scaled_distance: std::array::from_fn(|k| {
            (NPD_DISTANCES_FT[k] * METRES_PER_FOOT * random.between(0.5, 3.0)).log10()
        }),
        installation_shares: std::array::from_fn(|_| [shares(random), shares(random)]),
    }
}

/// Areas from a point to a city square, at one elevation or across a valley, near a box or out
/// of its reach: no receiver of one hears more of a box than its ceiling, nor more of its lambda.
#[test]
fn no_receiver_of_an_area_sums_more_of_a_box_than_its_ceiling() {
    let tile = TileId::containing(Mercator::from_degrees(50.08, 14.42));
    let frame = LocalFrame::at(tile.centre());
    let origin = AircraftReceiver {
        position: [0.0, 0.0],
        altitude_m: 0.0,
    };
    let mut random = Random(5);
    let mut heard = 0;
    for _ in 0..5_000 {
        let record = random_box(&mut random);
        let box_m = centroid_offset_m(record.place.centroid, tile, &frame, origin);
        let distance = random.between(0.0, 20_000.0);
        let bearing = random.between(0.0, std::f64::consts::TAU);
        let middle = [
            box_m[0] + distance * bearing.cos(),
            box_m[1] + distance * bearing.sin(),
        ];
        let extent = [random.between(0.0, 2_500.0), random.between(0.0, 2_500.0)];
        let low_altitude = random.between(-30.0, 2_300.0);
        let area = ReceiverArea {
            low: [middle[0] - extent[0], middle[1] - extent[1]],
            high: [middle[0] + extent[0], middle[1] + extent[1]],
            altitudes_m: [low_altitude, low_altitude + random.between(0.0, 300.0)],
        };
        let ceiling = box_flight_ceiling(&record, tile, &frame, &CeilingArea::of(area));
        for _ in 0..8 {
            let receiver = AircraftReceiver {
                position: std::array::from_fn(|axis| {
                    random.between(area.low[axis], area.high[axis])
                }),
                altitude_m: random.between(area.altitudes_m[0], area.altitudes_m[1]),
            };
            let offset = centroid_offset_m(record.place.centroid, tile, &frame, receiver);
            if offset[0].hypot(offset[1]) > AIRCRAFT_REACH_M {
                continue;
            }
            let impedance_gain = energy(receiver_impedance_db(receiver.altitude_m));
            let (sums, sel) = box_at(
                &record,
                offset,
                receiver.altitude_m,
                &Unscreened,
                impedance_gain,
            );
            let lambda = box_lambda(&record, sel.closest_slant_m);
            for period in 0..PERIODS {
                assert!(
                    sums[period] <= ceiling.energy[period] * (1.0 + 1e-9),
                    "{record:?} at {receiver:?}"
                );
                let weighted = sums[period] * lambda[period];
                assert!(
                    weighted <= ceiling.energy_lambda[period] * (1.0 + 1e-9),
                    "{record:?} at {receiver:?}"
                );
                heard += usize::from(sums[period] > 0.0);
            }
        }
    }
    assert!(heard > 20_000, "{heard} energies checked");
}
