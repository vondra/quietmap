//! The check's report of one point: the boxes around it, fine and as the popup reads them,
//! against the exact sums, and the popup's flight lists against the exact loudest flights.

use super::exact::{
    ALOFT, BEYOND, BY_DISTANCE, DISTANCE_BANDS_M, EXACT, NEAR_GROUND, Sums, distance_band,
};
use super::{
    FlightList, LIST_TOLERANCE_DB, PIECES, PointReport, REACH_M, REACH_RINGS, Receiver, leq_db,
};
use physics::bands::PERIODS;
use physics::doc29::boxes::{AircraftBoxAtReceiver, box_sel_at_receiver};
use physics::doc29::screening::Unscreened;
use popup::aircraft::boxes::{AircraftReceiver, tile_energy};
use popup::aircraft::flights::{FLIGHTS_SHOWN, FlightTotals};
use popup::aircraft::ring_aircraft;
use std::collections::HashMap;
use std::path::Path;
use tiles::aircraft::Aircraft;
use tiles::geo::TileId;

/// Whether two lists name the same flights with the same Lmax (their SELs sum what each
/// computed and may differ).
fn same_list(a: &FlightTotals, b: &FlightTotals) -> bool {
    let named = |totals: &FlightTotals| -> Vec<(u32, u32, f64)> {
        totals
            .loudest()
            .iter()
            .map(|flight| (flight.icao, flight.start_unix, flight.lmax_db))
            .collect()
    };
    named(a) == named(b)
}

/// The aircraft files of `kind` of each ring (0 ..= [`REACH_RINGS`]) around `centre`.
fn read_rings(
    aircraft_root: &Path,
    centre: TileId,
    kind: tiles::Kind,
) -> Vec<Vec<(TileId, Vec<u8>)>> {
    (0..=REACH_RINGS)
        .map(|ring| {
            centre
                .ring(ring)
                .into_iter()
                .filter_map(|tile| {
                    std::fs::read(tiles::tile_path(aircraft_root, tile, kind))
                        .ok()
                        .map(|bytes| (tile, bytes))
                })
                .collect()
        })
        .collect()
}

fn parse_rings(
    rings: &[Vec<(TileId, Vec<u8>)>],
) -> Result<Vec<Vec<(TileId, Aircraft<'_>)>>, String> {
    rings
        .iter()
        .map(|ring| {
            ring.iter()
                .map(|(tile, bytes)| Aircraft::parse(bytes).map(|aircraft| (*tile, aircraft)))
                .collect::<Result<_, _>>()
        })
        .collect::<Result<_, _>>()
        .map_err(|error| error.to_string())
}

/// The day SEL energy per period of the boxes within the reach: those of the first band and
/// those above it, and by the horizontal distance of their centroids.
struct BoxEnergy {
    bands: [[f64; PERIODS]; 2],
    distances: [[f64; PERIODS]; DISTANCE_BANDS_M.len()],
}

fn box_energy<'a>(
    tiles: impl Iterator<Item = &'a (TileId, Aircraft<'a>)>,
    receiver: &Receiver,
) -> BoxEnergy {
    let mut energy = BoxEnergy {
        bands: [[0.0; PERIODS]; 2],
        distances: [[0.0; PERIODS]; DISTANCE_BANDS_M.len()],
    };
    for (tile, aircraft) in tiles {
        for index in 0..aircraft.box_count() {
            let record = aircraft.aircraft_box(index);
            let global = tile.global(record.centroid);
            let [east, north] = receiver
                .frame
                .metres_of_steps([global.x as f64, global.y as f64]);
            if east.hypot(north) > REACH_M {
                continue;
            }
            let at_receiver = AircraftBoxAtReceiver {
                centroid_m: [
                    east,
                    north,
                    record.centroid_altitude_m - receiver.altitude_m,
                ],
                axis_rad: record.axis_rad,
                gradient: record.gradient,
                piece_length_m: record.piece_length_m,
                levels_db: &record.energy_db,
                tail_levels_db: &record.tail_energy_db,
                scaled_distance_m: &record.scaled_distance_m,
                installation_shares: record.installation_shares,
                ground_m: record.ground_m - receiver.altitude_m,
            };
            if let Some(sel) = box_sel_at_receiver(&at_receiver, &Unscreened) {
                let band = usize::from(record.clearance_m > 0.0);
                let distance = distance_band(east.hypot(north));
                for (period, level) in sel.sel_db.iter().enumerate() {
                    let value = 10f64.powf(level / 10.0);
                    energy.bands[band][period] += value;
                    energy.distances[distance][period] += value;
                }
            }
        }
    }
    energy
}

/// One point's report from its sums and the written boxes around it: the fine boxes of every
/// ring, and as the popup reads them (fine boxes in the tile and ring 1, far boxes beyond).
pub(super) fn report(
    aircraft_root: &Path,
    receiver: &Receiver,
    total: Sums,
) -> Result<PointReport, String> {
    let centre = TileId::containing(receiver.frame.origin);
    let fine_files = read_rings(aircraft_root, centre, tiles::Kind::Aircraft);
    let far_files = read_rings(aircraft_root, centre, tiles::Kind::AircraftFar);
    let (fine, far) = (parse_rings(&fine_files)?, parse_rings(&far_files)?);
    let megabytes = |rings: &[&Vec<(TileId, Vec<u8>)>]| {
        rings
            .iter()
            .flat_map(|ring| ring.iter())
            .map(|(_, bytes)| bytes.len())
            .sum::<usize>() as f64
            / 1e6
    };
    let popup_rings: Vec<&Vec<(TileId, Aircraft<'_>)>> = (0..fine.len())
        .map(|ring| {
            if ring >= 2 && !far[ring].is_empty() {
                &far[ring]
            } else {
                &fine[ring]
            }
        })
        .collect();
    let popup_bytes = megabytes(
        &(0..fine_files.len())
            .map(|ring| {
                if ring >= 2 && !far_files[ring].is_empty() {
                    &far_files[ring]
                } else {
                    &fine_files[ring]
                }
            })
            .collect::<Vec<_>>(),
    );
    let fine_bytes = megabytes(&fine_files.iter().collect::<Vec<_>>());
    let fine_energy = box_energy(fine.iter().flatten(), receiver);
    let [near_ground, aloft] = fine_energy.bands;
    let popup_energy = box_energy(popup_rings.iter().flat_map(|ring| ring.iter()), receiver);
    let popup: [f64; PERIODS] =
        std::array::from_fn(|p| popup_energy.bands[0][p] + popup_energy.bands[1][p]);
    let boxed: [f64; PERIODS] = std::array::from_fn(|p| near_ground[p] + aloft[p]);
    // The ten loudest flights by LAmax of the exact sum, and the popup's list reading K pieces
    // per box.
    let mut exact_top: Vec<(u64, f64)> = total
        .flights
        .iter()
        .map(|(&flight, &(_, lmax_db))| (flight, lmax_db))
        .collect();
    exact_top.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    exact_top.truncate(FLIGHTS_SHOWN);
    // The popup names a flight by its address and start (the id's bits 39-32 are dropped).
    let named = |flight: u64| (flight >> 40 & 0x00ff_ffff, flight & 0xffff_ffff);
    let exact_lmax: HashMap<(u64, u64), f64> = total
        .flights
        .iter()
        .map(|(&flight, &(_, lmax_db))| (named(flight), lmax_db))
        .collect();
    let aircraft_receiver = AircraftReceiver {
        position: [0.0, 0.0],
        altitude_m: receiver.altitude_m,
    };
    let mut lists = Vec::new();
    for pieces in PIECES {
        let (mut searched, mut exhaustive) =
            (FlightTotals::reading(pieces), FlightTotals::reading(pieces));
        for ring in &popup_rings {
            ring_aircraft(
                ring,
                &receiver.frame,
                aircraft_receiver,
                &Unscreened,
                &mut searched,
            );
            for (tile, aircraft) in ring.iter() {
                let answer = tile_energy(
                    aircraft,
                    *tile,
                    &receiver.frame,
                    aircraft_receiver,
                    &Unscreened,
                );
                let within: Vec<usize> = answer
                    .lamax_bounds
                    .iter()
                    .map(|&(index, _)| index)
                    .collect();
                exhaustive.add_boxes(
                    aircraft,
                    *tile,
                    &within,
                    &receiver.frame,
                    aircraft_receiver,
                    &Unscreened,
                );
            }
        }
        let listed: Vec<(u64, f64, f64)> = searched
            .loudest()
            .iter()
            .map(|flight| {
                let id = (u64::from(flight.icao), u64::from(flight.start_unix));
                let exact = exact_lmax.get(&id).copied().unwrap_or(f64::NAN);
                (id.0 << 40 | id.1, flight.lmax_db, flight.lmax_db - exact)
            })
            .collect();
        let found = exact_top
            .iter()
            .filter(|(flight, _)| {
                let (icao, start) = named(*flight);
                listed.iter().any(|(id, _, _)| *id == icao << 40 | start)
            })
            .count();
        let last = exact_top
            .last()
            .map_or(f64::NEG_INFINITY, |(_, lmax_db)| *lmax_db);
        let as_loud = listed
            .iter()
            .filter(|(id, _, _)| {
                exact_lmax
                    .get(&(id >> 40, id & 0xffff_ffff))
                    .is_some_and(|lmax_db| *lmax_db >= last - LIST_TOLERANCE_DB)
            })
            .count();
        lists.push(FlightList {
            pieces,
            recall: found as f64 / exact_top.len().max(1) as f64,
            tolerant_recall: as_loud as f64 / listed.len().max(1) as f64,
            listed,
            search_is_exhaustive: same_list(&searched, &exhaustive),
        });
    }
    Ok(PointReport {
        exact: leq_db(&total.energy[EXACT..]),
        boxed: leq_db(&boxed),
        boxed_as_read: leq_db(&popup),
        megabytes: [fine_bytes, popup_bytes],
        beyond: leq_db(&total.energy[BEYOND..]),
        by_distance: std::array::from_fn(|band| {
            [
                leq_db(&total.energy[BY_DISTANCE + PERIODS * band..]),
                leq_db(&fine_energy.distances[band]),
            ]
        }),
        near_ground: [leq_db(&total.energy[NEAR_GROUND..]), leq_db(&near_ground)],
        aloft: [leq_db(&total.energy[ALOFT..]), leq_db(&aloft)],
        exact_top,
        lists,
    })
}
