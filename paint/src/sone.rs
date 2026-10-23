//! A painted pixel's loudness: Nden per layer and in total, the popup's model at the map's
//! resolution (`popup::percentiles::Resolution::MAP`). The sources evaluated exactly there, each
//! group a line (a road, a railway) by its display fields; what the block left out of a group
//! sounds with its passes too, joined to its line, or as a line of its own from its loudest
//! source left out; the quiet hum held steady with its own bands; the flights one line.

use crate::blocks::LeftGroup;
use crate::exact::{LAYERS, PairEnergy, PointBands};
use crate::flights::FlightSums;
use crate::square::Square;
use foldhash::fast::FixedState;
use physics::bands::{PERIODS, lden_energy};
use physics::doc29::atmosphere::THIRD_OCTAVES as CLASS_THIRDS;
use physics::loudness::THIRD_OCTAVES;
use popup::candidates::DisplayRef;
use popup::loudness::{
    Curve, FlightSound, LayerSound, PlacedLayer, curves, layer_curves, layers_loudness,
    place_layers,
};
use popup::percentiles::{Group, Passes, Resolution, Weather, flight_weather};
use rayon::prelude::*;
use std::collections::HashMap;
use tiles::sources::Layer;

/// How every source group a square's candidates point at sounds in time, read once from its
/// display record: where a line's traffic and events come from.
pub(crate) struct Displays(HashMap<(u16, u32), Passes, FixedState>);

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
        let records = keys
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
        Ok(Displays(records))
    }

    fn of_group(&self, group: &Group) -> Result<Passes, String> {
        let DisplayRef {
            tile, attribute, ..
        } = group.display;
        (self.0.get(&(tile, attribute)).copied())
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

/// One source group's exact pairs at a pixel: its energy and binned weather, its loudest pair's
/// display record (as a click's contributor) and that pair's Lden energy.
struct Heard {
    layer: Layer,
    energy: [f64; PERIODS],
    weather: Weather,
    distance_m: f64,
    display: (DisplayRef, f64),
}

/// Source-group storage reused by one parallel pixel job, hashed by foldhash: SipHash's resistance
/// to chosen keys buys nothing for a square's own group keys, at every group of every pixel.
#[derive(Default)]
pub(crate) struct PixelSoundScratch {
    groups: HashMap<u64, Heard, FixedState>,
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
    Ok(pixel_sound(
        square,
        displays,
        &mut PixelSoundScratch::default(),
        position_reflection,
        exact,
        rest,
    )?
    .ndens(resolution))
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
                let parts = line.parts(states);
                out.extend(parts.energy.map(|value| value as f32));
                for means in &parts.means {
                    out.extend(means.iter().map(|&value| value as f32));
                }
                out.extend(parts.lambda.map(|value| value as f32));
                out.push(parts.profile.map_or(-1.0, |profile| profile as f32));
                out.extend(
                    parts
                        .duty
                        .map_or([f32::NAN; PERIODS], |duty| duty.map(|value| value as f32)),
                );
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
    scratch: &mut PixelSoundScratch,
    (position, reflection): ([f64; 2], f64),
    (candidates, energies, bands): (&[u32], &[PairEnergy], &PointBands),
    (rest, flight_spectrum_db): (&Rest, &[f64; CLASS_THIRDS]),
) -> Result<PixelSound, String> {
    let groups = &mut scratch.groups;
    groups.clear();
    for (&index, energy) in candidates.iter().zip(energies) {
        let candidate = &square.candidates[index as usize];
        let heard = groups.entry(candidate.group_key).or_insert_with(|| Heard {
            layer: candidate.layer,
            energy: [0.0; PERIODS],
            weather: Weather::default(),
            distance_m: f64::INFINITY,
            display: (candidate.display, f64::NEG_INFINITY),
        });
        let lden = lden_energy(&energy.mixed);
        let order = |d: DisplayRef| (d.ring, d.tile, d.attribute);
        if lden > heard.display.1
            || (lden == heard.display.1 && order(candidate.display) < order(heard.display.0))
        {
            heard.display = (candidate.display, lden);
        }
        let mixed = energy.mixed.map(|value| reflection * value);
        let states = energy
            .states
            .map(|state| state.map(|value| reflection * value));
        for (total, value) in heard.energy.iter_mut().zip(mixed) {
            *total += value;
        }
        heard.weather.add(&mixed, &states);
        heard.distance_m = heard.distance_m.min(candidate.distance_from(position));
    }
    // What the block left out of a group sounds with its passes, and leaves the steady rest: its
    // energy, in either weather state, joins the group's line, or is a line of its own taken from
    // its loudest source left out.
    let mut steady = rest
        .steady
        .map(|layer| layer.map(|value| reflection * value));
    let (left_groups, weights) = rest.left_groups;
    for group in left_groups {
        let blend = |of: &dyn Fn(&PairEnergy) -> [f64; PERIODS]| -> [f64; PERIODS] {
            std::array::from_fn(|p| {
                reflection
                    * rest.left_reach
                    * (0..4)
                        .map(|c| weights[c] * of(&group.at_corners[c])[p])
                        .sum::<f64>()
            })
        };
        let left = blend(&|energy| energy.mixed);
        let states = [0, 1].map(|state| blend(&|energy| energy.states[state]));
        if left.iter().all(|&value| value <= 0.0) {
            continue;
        }
        let nearest_m = (group.nearest.iter())
            .filter(|&&index| index != u32::MAX)
            .map(|&index| square.candidates[index as usize].distance_from(position))
            .fold(f64::INFINITY, f64::min);
        let loudest = &square.candidates[group.loudest as usize];
        let heard = groups.entry(group.key).or_insert_with(|| Heard {
            layer: loudest.layer,
            energy: [0.0; PERIODS],
            weather: Weather::default(),
            distance_m: nearest_m,
            display: (loudest.display, lden_energy(&left)),
        });
        heard.distance_m = heard.distance_m.min(nearest_m);
        heard.weather.add(&left, &states);
        let layer = heard.layer as usize;
        for p in 0..PERIODS {
            heard.energy[p] += left[p];
            steady[layer][p] = (steady[layer][p] - left[p]).max(0.0);
        }
    }
    // Each layer: its steady rest and its groups, its spectrum the exact bands and the rest's.
    let [flights, flights_lambda] = rest.flights;
    let mut energies = steady;
    for heard in groups.values() {
        for (total, value) in energies[heard.layer as usize].iter_mut().zip(heard.energy) {
            *total += value;
        }
    }
    for (total, value) in energies[Layer::Aircraft as usize].iter_mut().zip(flights) {
        *total += value;
    }
    let sounds: Vec<LayerSound> = (0..LAYERS)
        .map(|layer| LayerSound {
            layer: Layer::ALL[layer],
            energy: energies[layer],
            spectrum: std::array::from_fn(|p| {
                std::array::from_fn(|b| bands[layer][p][b] + rest.bands[layer][p][b])
            }),
        })
        .collect();
    let views: Vec<Group> = (groups.iter())
        .map(|(&key, heard)| Group {
            key,
            layer: heard.layer,
            energy: heard.energy,
            weather: &heard.weather,
            distance_m: heard.distance_m,
            display: heard.display.0,
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
