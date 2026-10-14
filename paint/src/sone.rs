//! A painted pixel's loudness: Nden per layer and in total, the popup's model at the map's
//! resolution (`popup::percentiles::Resolution::MAP`). The sources evaluated exactly there, each
//! group a line (a road, a railway) by its display fields; what the block left out of a group
//! sounds with its passes too, joined to its line, or as a line of its own from its loudest
//! source left out; the quiet hum held steady with its own bands; the flights one line.

use crate::blocks::LeftGroup;
use crate::exact::{LAYERS, PairEnergy, PointBands};
use crate::flights::FlightSums;
use crate::square::Square;
use physics::bands::{PERIODS, lden_energy};
use physics::doc29::atmosphere::THIRD_OCTAVES as CLASS_THIRDS;
use physics::loudness::THIRD_OCTAVES;
use popup::candidates::DisplayRef;
use popup::loudness::{
    Curve, FlightSound, LayerSound, PlacedLayer, curves, layer_curves, layers_loudness,
    place_layers,
};
use popup::percentiles::{
    FLUCTUATING_SHARE_MIN, Group, Passes, Resolution, Weather, flight_weather,
};
use rayon::prelude::*;
use std::collections::HashMap;
use std::ops::Range;
use tiles::sources::Layer;

/// How every source group a square's candidates point at sounds in time, read once from its
/// display record: where a line's traffic and events come from.
pub(crate) struct Displays {
    by_record: HashMap<(u16, u32), Passes>,
    by_candidate: Vec<Passes>,
}

impl Displays {
    pub fn of(square: &Square) -> Result<Self, String> {
        let mut keys: Vec<(u16, u32, Layer)> = (square.candidates.iter())
            .map(|candidate| {
                (
                    candidate.display.tile,
                    candidate.display.attribute,
                    candidate.layer,
                )
            })
            .collect();
        keys.par_sort_unstable_by_key(|&(tile, attribute, _)| (tile, attribute));
        keys.dedup_by_key(|&mut (tile, attribute, _)| (tile, attribute));
        let by_record: HashMap<(u16, u32), Passes> = keys
            .par_iter()
            .map(|&(tile, attribute, layer)| {
                let record = popup::answer::display_fields_of(
                    &square.sources[usize::from(tile)],
                    attribute,
                    layer,
                )?;
                Ok(((tile, attribute), Passes::read(layer, &record)))
            })
            .collect::<Result<_, String>>()?;
        let by_candidate = (square.candidates.iter())
            .map(|candidate| {
                by_record
                    .get(&(candidate.display.tile, candidate.display.attribute))
                    .copied()
                    .ok_or_else(|| {
                        format!(
                            "the display {}/{} was not read",
                            candidate.display.tile, candidate.display.attribute
                        )
                    })
            })
            .collect::<Result<Vec<_>, String>>()?;
        Ok(Displays {
            by_record,
            by_candidate,
        })
    }

    fn of_group(&self, group: &Group) -> Result<Passes, String> {
        let DisplayRef {
            tile, attribute, ..
        } = group.display;
        (self.by_record.get(&(tile, attribute)).copied())
            .ok_or_else(|| format!("the display {tile}/{attribute} was not read"))
    }
}

/// What a pixel hears beside its exact pairs, its reflection not yet taken: per layer the energy
/// per period not evaluated there (left out at the corners, humming) and its bands; of it the
/// groups its block left out with the pixel's weights of the block's corners, and the share of
/// what the block left out that reaches the pixel; the flights.
pub(crate) struct Rest<'a> {
    pub steady: [[f64; PERIODS]; LAYERS],
    pub bands: PointBands,
    pub left_groups: (&'a [LeftGroup], [f64; 4]),
    pub left_reach: f64,
    pub flights: FlightSums,
}

/// One source group's summed energy per period at a pixel before building its weather or distance,
/// referencing its slice of sorted candidate indices and optional left-out group.
struct GroupSum {
    key: u64,
    layer: Layer,
    energy: [f64; PERIODS],
    run: Range<usize>,
    left: Option<(usize, [f64; PERIODS])>,
}

/// One active group's weather, nearest piece distance and display record for [`place_layers`].
struct ActiveGroup {
    key: u64,
    layer: Layer,
    energy: [f64; PERIODS],
    weather: Weather,
    distance_m: f64,
    display: DisplayRef,
}

/// Blends four corners' per-period values with `weights` and `scale`.
fn blend_corners(
    at_corners: &[PairEnergy; 4],
    weights: &[f64; 4],
    scale: f64,
    of: impl Fn(&PairEnergy) -> [f64; PERIODS],
) -> [f64; PERIODS] {
    std::array::from_fn(|p| {
        scale
            * (0..4)
                .map(|c| weights[c] * of(&at_corners[c])[p])
                .sum::<f64>()
    })
}

/// The pixel's Nden per layer and in total (sone, 0 for silence) at `resolution` on the cores: its
/// [`pixel_sound`]'s.
pub(crate) fn pixel_ndens(
    square: &Square,
    displays: &Displays,
    position_reflection: ([f64; 2], f64),
    exact: (&[u32], &[PairEnergy], &PointBands),
    rest: (&Rest, &[f64; CLASS_THIRDS]),
    resolution: Resolution,
) -> Result<[f64; LAYERS + 1], String> {
    Ok(pixel_sound(square, displays, position_reflection, exact, rest)?.ndens(resolution))
}

/// What a pixel's loudness takes: each layer's lines over its steady energy, the layers' sounds and
/// the curves (the place's and each layer's).
pub struct PixelSound {
    placed: Vec<PlacedLayer>,
    sounds: Vec<LayerSound>,
    curves: [Curve; PERIODS],
    layer_curves: Vec<(Layer, [Curve; PERIODS])>,
}

impl PixelSound {
    /// Nden per layer and in total (sone, 0 for silence) at `resolution` on the cores.
    pub fn ndens(&self, resolution: Resolution) -> [f64; LAYERS + 1] {
        let place = layers_loudness(
            &self.placed,
            &self.sounds,
            (&self.curves, &self.layer_curves),
            resolution,
        );
        let mut ndens = [0.0; LAYERS + 1];
        for (layer, nden) in place.layers {
            ndens[layer as usize] = nden.unwrap_or(0.0);
        }
        ndens[LAYERS] = place.place.nden_sone;
        ndens
    }

    /// The card's input at `states` merged weather states (paint-gpu/kernels/loudness.cuh): per
    /// layer its steady energy, its line count and its lines; per curve (each layer's, then the
    /// place's) and period its spectrum's shape, NaN for none.
    pub fn pack(&self, states: usize, out: &mut Vec<f32>) {
        for (lines, steady) in &self.placed {
            out.extend(steady.map(|value| value as f32));
            out.push(lines.len() as f32);
            for line in lines {
                line.pack(states, out);
            }
        }
        let place = std::iter::once(&self.curves);
        for curves in (self.layer_curves.iter().map(|(_, curves)| curves)).chain(place) {
            for curve in curves {
                match curve.shape_db() {
                    Some(shape) => out.extend(shape.iter().map(|&value| value as f32)),
                    None => out.extend([f32::NAN; THIRD_OCTAVES]),
                }
            }
        }
    }
}

/// What the pixel hears: its exact pairs (candidates and their energies) and their `bands`, the
/// `rest`, all times its `reflection`; the flights' spectrum `flight_spectrum_db` (unweighted third
/// octaves from 50 Hz); as each layer's lines over its steady energy and the curves.
pub(crate) fn pixel_sound(
    square: &Square,
    displays: &Displays,
    (position, reflection): ([f64; 2], f64),
    (candidates, energies, bands): (&[u32], &[PairEnergy], &PointBands),
    (rest, flight_spectrum_db): (&Rest, &[f64; CLASS_THIRDS]),
) -> Result<PixelSound, String> {
    let mut order: Vec<u32> = (0..candidates.len() as u32).collect();
    order.sort_by_key(|&i| square.candidates[candidates[i as usize] as usize].group_key);

    let mut steady = rest
        .steady
        .map(|layer| layer.map(|value| reflection * value));
    let (left_groups, weights) = rest.left_groups;
    let scale = reflection * rest.left_reach;

    let mut groups: Vec<GroupSum> =
        Vec::with_capacity((candidates.len() / 8).max(left_groups.len()));
    let (mut pos, mut left_at) = (0usize, 0usize);
    while pos < order.len() || left_at < left_groups.len() {
        let next_left = loop {
            if left_at >= left_groups.len() {
                break None;
            }
            let left = blend_corners(&left_groups[left_at].at_corners, &weights, scale, |e| {
                e.mixed
            });
            if left.iter().any(|&value| value > 0.0) {
                break Some((left_at, left_groups[left_at].key, left));
            }
            left_at += 1;
        };
        let next_cand = (pos < order.len()).then(|| {
            let first = &square.candidates[candidates[order[pos] as usize] as usize];
            let key = first.group_key;
            let mut end = pos + 1;
            while end < order.len()
                && square.candidates[candidates[order[end] as usize] as usize].group_key == key
            {
                end += 1;
            }
            (key, first.layer, pos..end)
        });
        match (next_cand, next_left) {
            (Some((key, layer, run)), Some((l_idx, l_key, left))) if key <= l_key => {
                let mut energy = [0.0; PERIODS];
                for &i in &order[run.clone()] {
                    let mixed = energies[i as usize].mixed;
                    for p in 0..PERIODS {
                        energy[p] += reflection * mixed[p];
                    }
                }
                let matched_left = if key == l_key {
                    for p in 0..PERIODS {
                        energy[p] += left[p];
                        steady[layer as usize][p] = (steady[layer as usize][p] - left[p]).max(0.0);
                    }
                    left_at = l_idx + 1;
                    Some((l_idx, left))
                } else {
                    None
                };
                pos = run.end;
                groups.push(GroupSum {
                    key,
                    layer,
                    energy,
                    run,
                    left: matched_left,
                });
            }
            (_, Some((l_idx, l_key, left))) => {
                let layer = square.candidates[left_groups[l_idx].loudest as usize].layer;
                for p in 0..PERIODS {
                    steady[layer as usize][p] = (steady[layer as usize][p] - left[p]).max(0.0);
                }
                left_at = l_idx + 1;
                groups.push(GroupSum {
                    key: l_key,
                    layer,
                    energy: left,
                    run: pos..pos,
                    left: Some((l_idx, left)),
                });
            }
            (Some((key, layer, run)), None) => {
                let mut energy = [0.0; PERIODS];
                for &i in &order[run.clone()] {
                    let mixed = energies[i as usize].mixed;
                    for p in 0..PERIODS {
                        energy[p] += reflection * mixed[p];
                    }
                }
                pos = run.end;
                groups.push(GroupSum {
                    key,
                    layer,
                    energy,
                    run,
                    left: None,
                });
            }
            (None, None) => break,
        }
    }

    // Each layer: its groups and its steady rest, its spectrum the exact bands and the rest's.
    let [flights, flights_lambda] = rest.flights;
    let sounds: Vec<LayerSound> = (0..LAYERS)
        .map(|layer| {
            let mut energy = steady[layer];
            for group in groups.iter().filter(|group| group.layer as usize == layer) {
                for (total, value) in energy.iter_mut().zip(group.energy) {
                    *total += value;
                }
            }
            if layer == Layer::Aircraft as usize {
                for (total, value) in energy.iter_mut().zip(flights) {
                    *total += value;
                }
            }
            LayerSound {
                layer: Layer::ALL[layer],
                energy,
                spectrum: std::array::from_fn(|p| {
                    std::array::from_fn(|b| bands[layer][p][b] + rest.bands[layer][p][b])
                }),
            }
        })
        .collect();

    // Only groups exceeding FLUCTUATING_SHARE_MIN of their layer's energy with known passes can
    // become lines: build Weather and compute distance_from only for them.
    let order_ref = |d: DisplayRef| (d.ring, d.tile, d.attribute);
    let mut active = Vec::new();
    for group in groups {
        let total = &sounds[group.layer as usize].energy;
        if !(0..PERIODS).any(|p| group.energy[p] > FLUCTUATING_SHARE_MIN * total[p]) {
            continue;
        }
        let (winner_index, display) = if !group.run.is_empty() {
            let mut best_index = 0u32;
            let mut best_display = DisplayRef {
                ring: 0,
                tile: 0,
                attribute: 0,
            };
            let mut best_lden = f64::NEG_INFINITY;
            for &i in &order[group.run.clone()] {
                let index = candidates[i as usize];
                let candidate = &square.candidates[index as usize];
                let lden = lden_energy(&energies[i as usize].mixed);
                if lden > best_lden
                    || (lden == best_lden && order_ref(candidate.display) < order_ref(best_display))
                {
                    best_index = index;
                    best_display = candidate.display;
                    best_lden = lden;
                }
            }
            (best_index, best_display)
        } else {
            let loudest = left_groups[group.left.unwrap().0].loudest;
            (loudest, square.candidates[loudest as usize].display)
        };
        let passes = displays.by_candidate[winner_index as usize];
        if matches!(passes, Passes::Unknown) {
            continue;
        }
        let mut weather = Weather::default();
        for &i in &order[group.run.clone()] {
            let energy = &energies[i as usize];
            let mixed = energy.mixed.map(|value| reflection * value);
            let states = energy
                .states
                .map(|state| state.map(|value| reflection * value));
            weather.add(&mixed, &states);
        }
        if let Some((l_idx, left)) = group.left {
            let left_group = &left_groups[l_idx];
            let states = [0, 1].map(|state| {
                blend_corners(&left_group.at_corners, &weights, scale, |e| e.states[state])
            });
            weather.add(&left, &states);
        }
        let distance_m = if matches!(passes, Passes::Steady(_)) {
            f64::INFINITY
        } else {
            let mut near_m = f64::INFINITY;
            for &i in &order[group.run] {
                let index = candidates[i as usize];
                near_m = near_m.min(square.candidates[index as usize].distance_from(position));
            }
            if let Some((l_idx, _)) = group.left {
                for &index in &left_groups[l_idx].nearest {
                    if index != u32::MAX {
                        near_m =
                            near_m.min(square.candidates[index as usize].distance_from(position));
                    }
                }
            }
            near_m
        };
        active.push(ActiveGroup {
            key: group.key,
            layer: group.layer,
            energy: group.energy,
            weather,
            distance_m,
            display,
        });
    }

    let views: Vec<Group> = (active.iter())
        .map(|group| Group {
            key: group.key,
            layer: group.layer,
            energy: group.energy,
            weather: &group.weather,
            distance_m: group.distance_m,
            display: group.display,
        })
        .collect();
    let weather = flight_weather(&(flights, flights_lambda));
    let passes = |group: &Group| displays.of_group(group);
    let flight_sound = FlightSound {
        energy: flights,
        spectrum_db: Some(*flight_spectrum_db),
    };
    let placed = place_layers(
        &views,
        &sounds,
        (&weather, (flights, flights_lambda)),
        &passes,
    )?;
    Ok(PixelSound {
        placed,
        curves: curves(&sounds, &flight_sound),
        layer_curves: layer_curves(&sounds, &flight_sound),
        sounds,
    })
}
