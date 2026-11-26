//! A painted pixel's loudness: Nden per layer and in total, the popup's model at the map's
//! resolution (`popup::percentiles::Resolution::MAP`). The sources evaluated exactly there, each
//! group a line (a road, a railway) by its display fields; what the block left out of a group
//! sounds with its passes too, joined to its line, or as a line of its own from its loudest
//! source left out; the quiet hum held steady with its own bands; the flights a line a stream of
//! their passes (`crate::flight_streams`).

use crate::blocks::{LeftBin, LeftGroup, Strata};
use crate::corner_sources::{Ranked, SECTORS};
use crate::exact::{LAYERS, PairEnergy, Point, PointBands};
use crate::flights::FlightSums;
pub(crate) use crate::sone_fold::FoldScratch;
pub use crate::sone_fold::{MAP_PROFILES, MapHourGroups, map_hour_groups};
use crate::sone_fold::{MAP_WEATHER_SPREAD, fold_placed_for_map};
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
use popup::percentiles::{
    FLUCTUATING_SHARE_MIN, Group, Line, Passes, Resolution, WEATHER_STATES, Weather,
    merge_close_states,
};
use rayon::prelude::*;
use std::collections::HashMap;
use tiles::sources::Layer;

/// Floats a packed line takes ([`PixelSound::pack`], the card's `LoudLine`): its energy per period,
/// its mean in each weather state per period, its lambda per period, its hourly profile, its duty
/// and its passes in progress per period.
pub const PACKED_LINE_FLOATS: usize =
    PERIODS + PERIODS * WEATHER_STATES + PERIODS + 1 + 2 * PERIODS;

/// A line as the card reads it: [`PACKED_LINE_FLOATS`] floats, no profile -1, no duty or no passes
/// NaN.
pub fn pack_line(line: &Line, out: &mut Vec<f32>) {
    let parts = line.parts();
    out.extend(parts.energy.map(|value| value as f32));
    for period in &parts.means {
        out.extend(period.iter().map(|&value| value as f32));
    }
    out.extend(parts.lambda.map(|value| value as f32));
    out.push(parts.profile.map_or(-1.0, |profile| profile as f32));
    for shares in [parts.duty, parts.passing] {
        out.extend(shares.map_or([f32::NAN; PERIODS], |share| share.map(|value| value as f32)));
    }
}

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
/// groups its block left out; the flights.
pub(crate) struct SoundBesideExactPairs<'a> {
    pub steady: [[f64; PERIODS]; LAYERS],
    pub bands: PointBands,
    pub left_out: LeftOut<'a>,
    pub flights: FlightSums,
}

/// The groups a block left out and their weather bins at its corners, with how much of each
/// corner's layer and direction reaches the pixel: the direction's transfer times the pixel's
/// weight of the corner (`crate::transfers::Reaching`).
#[derive(Clone, Copy)]
pub(crate) struct LeftOut<'a> {
    pub groups: &'a [LeftGroup],
    pub bins: &'a [LeftBin],
    pub transfers: &'a [Strata<f64>; 4],
}

/// No group left out: a point that evaluates every loud source.
pub(crate) const NOTHING_LEFT_OUT: LeftOut = LeftOut {
    groups: &[],
    bins: &[],
    transfers: &[[[0.0; SECTORS]; LAYERS]; 4],
};

/// One source group's exact pairs at a pixel: its energy and binned weather, its nearest emitters'
/// distance from the pixel's receiver, its loudest pair's display record (as a click's contributor)
/// and that pair's Lden energy.
struct GroupSound {
    layer: Layer,
    energy: [f64; PERIODS],
    weather: Weather,
    slant_distance_m: f64,
    display: (DisplayRef, f64),
}

/// Source-group storage reused by one parallel pixel job: a flat pool avoids re-zeroing every
/// 1.4 KiB `Weather` table on each pixel (only touched bins are cleared), hashed by foldhash.
#[derive(Default)]
pub(crate) struct PixelSoundScratch {
    pool: Vec<GroupSound>,
    map: HashMap<u64, usize, FixedState>,
    used: usize,
    pub(crate) fold: FoldScratch,
}

impl PixelSoundScratch {
    fn reset(&mut self) {
        self.map.clear();
        for group in &mut self.pool[..self.used] {
            group.weather.clear();
        }
        self.used = 0;
    }

    fn entry_mut(
        &mut self,
        key: u64,
        init: impl FnOnce() -> (Layer, f64, (DisplayRef, f64)),
    ) -> &mut GroupSound {
        if let Some(&slot) = self.map.get(&key) {
            return &mut self.pool[slot];
        }
        let (layer, slant_distance_m, display) = init();
        let slot = self.used;
        self.used += 1;
        if slot == self.pool.len() {
            self.pool.push(GroupSound {
                layer,
                energy: [0.0; PERIODS],
                weather: Weather::default(),
                slant_distance_m,
                display,
            });
        } else {
            let group = &mut self.pool[slot];
            group.layer = layer;
            group.energy = [0.0; PERIODS];
            group.slant_distance_m = slant_distance_m;
            group.display = display;
        }
        self.map.insert(key, slot);
        &mut self.pool[slot]
    }
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
    fn fold_for_map(&self) -> Vec<PlacedLayer> {
        let mut placed = self.placed.clone();
        fold_placed_for_map(&mut placed, &mut FoldScratch::default());
        placed
    }

    fn ndens_from_placed(
        &self,
        placed: &[PlacedLayer],
        resolution: Resolution,
    ) -> [f64; LAYERS + 1] {
        let place = layers_loudness(
            placed,
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

    fn write_packed(&self, placed: &[PlacedLayer], out: &mut Vec<f32>) {
        for (lines, steady) in placed {
            out.extend(steady.map(|value| value as f32));
            out.push(lines.len() as f32);
            for line in lines {
                pack_line(line, out);
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

    /// Nden per layer and in total (sone, 0 for silence) at `resolution` on the cores.
    pub fn ndens(&self, resolution: Resolution) -> [f64; LAYERS + 1] {
        let placed = if resolution == Resolution::MAP {
            self.fold_for_map()
        } else {
            let mut placed = self.placed.clone();
            merge_close_states(&mut placed, MAP_WEATHER_SPREAD);
            placed
        };
        self.ndens_from_placed(&placed, resolution)
    }

    /// Consumes `self` to fold in place using `scratch` before evaluating Nden on the cores.
    pub(crate) fn ndens_with_scratch(
        mut self,
        scratch: &mut FoldScratch,
        resolution: Resolution,
    ) -> [f64; LAYERS + 1] {
        if resolution == Resolution::MAP {
            fold_placed_for_map(&mut self.placed, scratch);
        } else {
            merge_close_states(&mut self.placed, MAP_WEATHER_SPREAD);
        }
        self.ndens_from_placed(&self.placed, resolution)
    }

    /// The card's input in the click's weather states (paint-gpu/kernels/loudness.cuh): per
    /// layer its steady energy, its line count and its lines; per curve (each layer's, then the
    /// place's) and period its spectrum's shape, NaN for none. Shares [`fold_placed_for_map`] with
    /// [`Self::ndens`] so cores and card follow one folding and weather-merging rule.
    pub fn pack(&self, out: &mut Vec<f32>) {
        let placed = self.fold_for_map();
        self.write_packed(&placed, out);
    }

    /// Consumes `self` to fold in place using `scratch` before packing into `out` for the card.
    pub(crate) fn pack_with_scratch(mut self, scratch: &mut FoldScratch, out: &mut Vec<f32>) {
        fold_placed_for_map(&mut self.placed, scratch);
        self.write_packed(&self.placed, out);
    }
}

/// Steady layers (`Industry`, `Building`, `Ship`) never read `slant_distance_m` (`Passes::Steady`).
#[inline]
pub(crate) fn needs_slant(layer: Layer) -> bool {
    !matches!(layer, Layer::Industry | Layer::Building | Layer::Ship)
}

/// Like [`pixel_sound`], reading a corner's pre-ranked exact sources directly to reuse their
/// precomputed `lden` and `slant_distance_m` without copying into flat slices.
pub(crate) fn corner_pixel_sound(
    square: &Square,
    displays: &Displays,
    scratch: &mut PixelSoundScratch,
    reflection: f64,
    (ranked, bands): (&[Vec<Ranked>; LAYERS], &PointBands),
    (rest, flight_spectrum_db): (&SoundBesideExactPairs, &[f64; CLASS_THIRDS]),
) -> Result<PixelSound, String> {
    scratch.reset();
    let steady = rest
        .steady
        .map(|layer| layer.map(|value| reflection * value));
    let order = |d: DisplayRef| (d.ring, d.tile, d.attribute);
    for sources in ranked {
        for source in sources {
            let candidate = &square.candidates[source.index as usize];
            let group_sound = scratch.entry_mut(source.group, || {
                (
                    candidate.layer,
                    f64::INFINITY,
                    (candidate.display, f64::NEG_INFINITY),
                )
            });
            let lden = source.lden;
            if lden > group_sound.display.1
                || (lden == group_sound.display.1
                    && order(candidate.display) < order(group_sound.display.0))
            {
                group_sound.display = (candidate.display, lden);
            }
            let mixed = source.energy.mixed.map(|value| reflection * value);
            let states = source
                .energy
                .states
                .map(|state| state.map(|value| reflection * value));
            for (total, value) in group_sound.energy.iter_mut().zip(mixed) {
                *total += value;
            }
            group_sound.weather.add(&mixed, &states);
            if needs_slant(candidate.layer) {
                group_sound.slant_distance_m =
                    group_sound.slant_distance_m.min(source.slant_distance_m);
            }
        }
    }
    finish_pixel_sound(displays, scratch, steady, bands, rest, flight_spectrum_db)
}

/// Finishes [`pixel_sound`] / [`corner_pixel_sound`] from the populated scratch groups and steady
/// energy into a [`PixelSound`].
fn finish_pixel_sound(
    displays: &Displays,
    scratch: &PixelSoundScratch,
    steady: [[f64; PERIODS]; LAYERS],
    bands: &PointBands,
    rest: &SoundBesideExactPairs,
    flight_spectrum_db: &[f64; CLASS_THIRDS],
) -> Result<PixelSound, String> {
    // Each layer: its steady rest and its groups in key order, so a point's totals and
    // fluctuating-line cutoff depend only on that point, never on prior scratch table capacity.
    let mut views: Vec<Group> = (scratch.map.iter())
        .map(|(&key, &slot)| {
            let group_sound = &scratch.pool[slot];
            Group {
                key,
                layer: group_sound.layer,
                energy: group_sound.energy,
                weather: &group_sound.weather,
                slant_distance_m: group_sound.slant_distance_m,
                display: group_sound.display.0,
            }
        })
        .collect();
    views.sort_unstable_by_key(|group| group.key);
    let flights = rest.flights.energy;
    let mut energies = steady;
    for group in &views {
        for (total, value) in energies[group.layer as usize].iter_mut().zip(group.energy) {
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
    let passes = |group: &Group| displays.of_group(group);
    let flight_sound = FlightSound {
        energy: flights,
        spectrum_db: Some(*flight_spectrum_db),
    };
    let flight_lines = Line::flight_passes(&rest.flights.passes);
    let placed = place_layers(&views, &sounds, &flight_lines, &passes)?;
    Ok(PixelSound {
        placed,
        curves: curves(&sounds, &flight_sound),
        layer_curves: layer_curves(&sounds, &flight_sound),
        sounds,
    })
}

/// What the pixel's `point` hears: its exact pairs (candidates and their energies) and their
/// `bands`, the `rest`, all times its `reflection`; the flights' spectrum `flight_spectrum_db`
/// (unweighted third octaves from 50 Hz); as each layer's lines over its steady energy and the
/// curves.
pub(crate) fn pixel_sound(
    square: &Square,
    displays: &Displays,
    scratch: &mut PixelSoundScratch,
    (point, reflection): (&Point, f64),
    (candidates, energies, bands): (&[u32], &[PairEnergy], &PointBands),
    (rest, flight_spectrum_db): (&SoundBesideExactPairs, &[f64; CLASS_THIRDS]),
) -> Result<PixelSound, String> {
    scratch.reset();
    let mut steady = rest
        .steady
        .map(|layer| layer.map(|value| reflection * value));
    let mut layer_total = steady;
    for (total, value) in layer_total[Layer::Aircraft as usize]
        .iter_mut()
        .zip(rest.flights.energy)
    {
        *total += value;
    }
    for (&index, energy) in candidates.iter().zip(energies) {
        let candidate = &square.candidates[index as usize];
        let mixed = energy.mixed.map(|value| reflection * value);
        for (total, value) in layer_total[candidate.layer as usize].iter_mut().zip(mixed) {
            *total += value;
        }
        let group_sound = scratch.entry_mut(candidate.group_key, || {
            (
                candidate.layer,
                f64::INFINITY,
                (candidate.display, f64::NEG_INFINITY),
            )
        });
        let lden = lden_energy(&energy.mixed);
        let order = |d: DisplayRef| (d.ring, d.tile, d.attribute);
        if lden > group_sound.display.1
            || (lden == group_sound.display.1
                && order(candidate.display) < order(group_sound.display.0))
        {
            group_sound.display = (candidate.display, lden);
        }
        let states = energy
            .states
            .map(|state| state.map(|value| reflection * value));
        for (total, value) in group_sound.energy.iter_mut().zip(mixed) {
            *total += value;
        }
        group_sound.weather.add(&mixed, &states);
        if needs_slant(candidate.layer) {
            group_sound.slant_distance_m = group_sound
                .slant_distance_m
                .min(square.slant_distance_m(index, point));
        }
    }
    // What the block left out of a group sounds with its passes, and leaves the steady rest: its
    // energy, in either weather state, joins the group's line, or is a line of its own taken from
    // its loudest source left out.
    let LeftOut {
        groups: left_groups,
        bins,
        transfers,
    } = rest.left_out;
    let max_transfer_sum: [f64; LAYERS] = if left_groups.is_empty() {
        [0.0; LAYERS]
    } else {
        std::array::from_fn(|layer| {
            reflection
                * (0..4)
                    .map(|c| transfers[c][layer].iter().copied().fold(0.0f64, f64::max))
                    .sum::<f64>()
        })
    };
    for group in left_groups {
        let layer = group.layer as usize;
        let max_reach = max_transfer_sum[layer];
        if (0..PERIODS).all(|p| {
            max_reach * group.max_corner_energy[p]
                <= 0.9999 * FLUCTUATING_SHARE_MIN * layer_total[layer][p]
        }) && !scratch.map.contains_key(&group.key)
        {
            continue;
        }
        let loudest = &square.candidates[group.loudest as usize];
        let reach =
            |bin: &LeftBin| reflection * transfers[bin.corner as usize][layer][bin.sector as usize];
        let bins = &bins[group.bins.start as usize..group.bins.end as usize];
        let mut left = [0.0; PERIODS];
        for bin in bins {
            left[bin.period as usize] += reach(bin) * bin.values[2];
        }
        if left.iter().all(|&value| value <= 0.0) {
            continue;
        }
        // A left-out group absent from exact pairs and well below `FLUCTUATING_SHARE_MIN` would be
        // discarded by `place_lines` and returned to `steady`; leave it in `steady` directly.
        if !scratch.map.contains_key(&group.key)
            && (0..PERIODS)
                .all(|p| left[p] <= 0.9999 * FLUCTUATING_SHARE_MIN * layer_total[layer][p])
        {
            continue;
        }
        let nearest_m = if needs_slant(loudest.layer) {
            (group.nearest.iter())
                .filter(|&&index| index != u32::MAX)
                .map(|&index| square.slant_distance_m(index, point))
                .fold(f64::INFINITY, f64::min)
        } else {
            f64::INFINITY
        };
        let group_sound = scratch.entry_mut(group.key, || {
            (
                loudest.layer,
                nearest_m,
                (loudest.display, lden_energy(&left)),
            )
        });
        if needs_slant(loudest.layer) {
            group_sound.slant_distance_m = group_sound.slant_distance_m.min(nearest_m);
        }
        for bin in bins {
            let reach = reach(bin);
            if reach > 0.0 {
                let values = bin.values.map(|value| reach * value);
                (group_sound.weather).add_binned(bin.period as usize, bin.bin as usize, values);
            }
        }
        for p in 0..PERIODS {
            group_sound.energy[p] += left[p];
            steady[layer][p] = (steady[layer][p] - left[p]).max(0.0);
        }
    }
    finish_pixel_sound(displays, scratch, steady, bands, rest, flight_spectrum_db)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::levels::cell;
    use crate::square::Files;
    use physics::bands::BANDS;
    use physics::weather::{COLUMNS, ROWS, WeatherNode, WeatherTable, encode as encode_weather};
    use popup::release::Release;
    use tiles::Kind;
    use tiles::geo::TileId;
    use tiles::sources::{Attribute, Piece, encode as encode_sources};
    use tiles::square::write_tiles;

    /// A point's Nden must depend only on that point, never on how many groups an earlier pixel
    /// put into the reused scratch table (different hash bucket counts previously changed the
    /// rounding of the per-layer total at the fluctuating-line cutoff, cells 53 vs 79).
    #[test]
    fn pixel_sound_does_not_depend_on_prior_scratch_groups() {
        let root = tempfile::tempdir().unwrap();
        let year = root.path().join("2026");
        std::fs::create_dir(&year).unwrap();
        let weather_bytes = encode_weather(&vec![WeatherNode::default(); ROWS * COLUMNS]);
        std::fs::write(year.join("weather"), &weather_bytes).unwrap();
        tiles::mark_year_complete(&year, br#"{"year":"2026","as_of":"2027-01-01"}"#, "test")
            .unwrap();
        let weather = WeatherTable::parse(&weather_bytes).unwrap();
        let tile = TileId { x: 2212, y: 1387 };
        let keys: [u64; 10] = [0, 1, 2, 3, 4, 5, 6, 7, 13, 14];
        let attributes: Vec<_> = keys
            .iter()
            .map(|&group_key| Attribute {
                layer: Layer::Industry,
                height_m: 1.0,
                ground_percent: 0,
                platform_half_width_m: 0.0,
                exclusion_radius_m: 0.0,
                footprint_id: 0,
                group_key,
                emission: [[100.0; BANDS]; PERIODS],
                display: Vec::new(),
            })
            .collect();
        let pieces: Vec<_> = (0..keys.len() as u32)
            .map(|attribute| Piece {
                ends: [[0, 0]; 2],
                attribute,
            })
            .collect();
        write_tiles(
            &year,
            Kind::Sources,
            &[(tile, encode_sources(&pieces, &attributes, &[]))],
        )
        .unwrap();
        let release = Release::open(root.path(), "2026").unwrap();
        let files = Files::read(&release, tile).unwrap();
        let square = Square::new(&weather, tile, &files).unwrap();
        let displays = Displays(
            square
                .candidates
                .iter()
                .map(|c| {
                    (
                        (c.display.tile, c.display.attribute),
                        Passes::Steady(Some([1.0 / 1024.0; PERIODS])),
                    )
                })
                .collect(),
        );
        let candidate_of = |key: u64| -> u32 {
            square
                .candidates
                .iter()
                .position(|c| c.group_key == key)
                .unwrap() as u32
        };
        let point = Point::at(&square, [0.0, 0.0]).unwrap();
        let target_groups: [(u64, f64); 4] = [(1, 0.05), (14, 9998.9), (4, 0.05), (13, 1.0)];
        let target_ids: Vec<u32> = target_groups
            .iter()
            .map(|&(k, _)| candidate_of(k))
            .collect();
        let target_energies: Vec<PairEnergy> = target_groups
            .iter()
            .map(|&(_, relative)| {
                let energy = relative * 1024.0;
                PairEnergy {
                    mixed: [energy; PERIODS],
                    states: [[energy; PERIODS]; 2],
                }
            })
            .collect();
        let mut bands = [[[0.0; BANDS]; PERIODS]; LAYERS];
        for period in &mut bands[Layer::Industry as usize] {
            period[4] = 1024.0 * 10_000.0;
        }
        let rest = SoundBesideExactPairs {
            steady: [[0.0; PERIODS]; LAYERS],
            bands: [[[0.0; BANDS]; PERIODS]; LAYERS],
            left_out: NOTHING_LEFT_OUT,
            flights: FlightSums::default(),
        };
        let sound_after = |previous_groups: u64| -> (f64, u8) {
            let mut scratch = PixelSoundScratch::default();
            if previous_groups > 0 {
                let prior_ids: Vec<u32> = (0..previous_groups).map(candidate_of).collect();
                let prior_energies = vec![PairEnergy::default(); prior_ids.len()];
                pixel_sound(
                    &square,
                    &displays,
                    &mut scratch,
                    (&point, 1.0),
                    (&prior_ids, &prior_energies, &bands),
                    (&rest, &[0.0; CLASS_THIRDS]),
                )
                .unwrap();
            }
            let nden = pixel_sound(
                &square,
                &displays,
                &mut scratch,
                (&point, 1.0),
                (&target_ids, &target_energies, &bands),
                (&rest, &[0.0; CLASS_THIRDS]),
            )
            .unwrap()
            .ndens(Resolution::MAP)[LAYERS];
            (nden, cell(nden))
        };
        let (nden_zero, cell_zero) = sound_after(0);
        let (nden_eight, cell_eight) = sound_after(8);
        assert_eq!(cell_zero, cell_eight, "Nden {nden_zero} vs {nden_eight}");
        assert_eq!(cell_zero, 79);
        assert_eq!(nden_zero, nden_eight);
    }

    /// Cores (`ndens`) and card (`pack`) must apply the same weak-duty folding rule so a weak
    /// on/off source folds into steady energy on both paths.
    #[test]
    fn cores_and_card_share_weak_duty_folding_at_map_resolution() {
        let root = tempfile::tempdir().unwrap();
        let year = root.path().join("2026");
        std::fs::create_dir(&year).unwrap();
        let weather_bytes = encode_weather(&vec![WeatherNode::default(); ROWS * COLUMNS]);
        std::fs::write(year.join("weather"), &weather_bytes).unwrap();
        tiles::mark_year_complete(&year, br#"{"year":"2026","as_of":"2027-01-01"}"#, "test")
            .unwrap();
        let weather = WeatherTable::parse(&weather_bytes).unwrap();
        let tile = TileId { x: 2212, y: 1387 };
        let attr = Attribute {
            layer: Layer::Industry,
            height_m: 1.0,
            ground_percent: 0,
            platform_half_width_m: 0.0,
            exclusion_radius_m: 0.0,
            footprint_id: 0,
            group_key: 1,
            emission: [[80.0; BANDS]; PERIODS],
            display: Vec::new(),
        };
        write_tiles(
            &year,
            Kind::Sources,
            &[(
                tile,
                encode_sources(
                    &[Piece {
                        ends: [[0, 0]; 2],
                        attribute: 0,
                    }],
                    &[attr],
                    &[],
                ),
            )],
        )
        .unwrap();
        let release = Release::open(root.path(), "2026").unwrap();
        let files = Files::read(&release, tile).unwrap();
        let square = Square::new(&weather, tile, &files).unwrap();
        let c = &square.candidates[0];
        let point = Point::at(&square, [0.0, 0.0]).unwrap();
        let mut bands = [[[0.0; BANDS]; PERIODS]; LAYERS];
        for period in &mut bands[Layer::Industry as usize] {
            period[4] = 1_260_000.0;
        }
        // 4% duty-0.5 source beside 96% steady background at mean 1,260,000: off (60.83 dB) and
        // on (61.17 dB) straddle the 61 dB map bin boundary, so unfolded `ndens` differs from
        // pure steady unless cores and card share weak-duty folding (on-peak 8% <= 12%, std 4% <= 8%).
        let mut rest_steady = [[0.0; PERIODS]; LAYERS];
        rest_steady[Layer::Industry as usize] = [1_209_600.0; PERIODS];
        let rest = SoundBesideExactPairs {
            steady: rest_steady,
            bands,
            left_out: NOTHING_LEFT_OUT,
            flights: FlightSums::default(),
        };
        let displays_duty = Displays(
            [(
                (c.display.tile, c.display.attribute),
                Passes::Steady(Some([0.5; PERIODS])),
            )]
            .into_iter()
            .collect(),
        );
        let displays_steady = Displays(
            [((c.display.tile, c.display.attribute), Passes::Steady(None))]
                .into_iter()
                .collect(),
        );
        let pair = PairEnergy {
            mixed: [50_400.0; PERIODS],
            states: [[50_400.0; PERIODS]; 2],
        };
        let mut scratch = PixelSoundScratch::default();
        let with_duty = pixel_sound(
            &square,
            &displays_duty,
            &mut scratch,
            (&point, 1.0),
            (&[0], &[pair], &bands),
            (&rest, &[0.0; CLASS_THIRDS]),
        )
        .unwrap();
        let pure_steady = pixel_sound(
            &square,
            &displays_steady,
            &mut scratch,
            (&point, 1.0),
            (&[0], &[pair], &bands),
            (&rest, &[0.0; CLASS_THIRDS]),
        )
        .unwrap();
        let mut packed_duty = Vec::new();
        let mut packed_steady = Vec::new();
        with_duty.pack(&mut packed_duty);
        pure_steady.pack(&mut packed_steady);
        assert_eq!(
            packed_duty.iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
            packed_steady
                .iter()
                .map(|v| v.to_bits())
                .collect::<Vec<_>>()
        );
        assert_eq!(
            with_duty.ndens(Resolution::MAP),
            pure_steady.ndens(Resolution::MAP)
        );
    }

    /// Collapsing an unprofiled steady line whose weather states compare equal as `f32` must add
    /// the exact `f64` mean across states rather than only state 0.
    #[test]
    fn collapsed_steady_line_keeps_mean_across_weather_states() {
        let mut means = [[1_000_000.0; WEATHER_STATES]; PERIODS];
        let mut guard_means = [[100.0; WEATHER_STATES]; PERIODS];
        for period in 0..PERIODS {
            for state in WEATHER_STATES / 2..WEATHER_STATES {
                means[period][state] = 1_000_000.02;
                // Spread > 1.5 dB prevents `merge_close_states` from averaging the two halves first.
                guard_means[period][state] = 10_000.0;
            }
        }
        let sound = PixelSound {
            placed: vec![(
                vec![
                    Line::steady(means, None),
                    Line::steady(guard_means, Some(0)),
                ],
                [0.0; PERIODS],
            )],
            sounds: Vec::new(),
            curves: curves(
                &[],
                &FlightSound {
                    energy: [0.0; PERIODS],
                    spectrum_db: None,
                },
            ),
            layer_curves: Vec::new(),
        };
        let folded = sound.fold_for_map();
        assert_eq!(folded[0].0.len(), 1);
        for period in folded[0].1 {
            assert!((period - 1_000_000.01).abs() < 1e-8, "got {period}");
        }
    }
}
