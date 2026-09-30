//! One streamed update of a click: every layer's totals and account, the loudest contributor
//! groups, what was read, and the building a click stands in.

use crate::aircraft::flights::LoudFlight;
use crate::answer::RECEIVER_HEIGHT_M;
use crate::building::BuildingClick;
use crate::candidates::{DisplayRef, lden_weighted};
use crate::listing::EvaluatedPiece;
use crate::scene::Ground;
use crate::selection::LayerSelection;
use physics::bands::PERIODS;
use tiles::geo::LocalFrame;
use tiles::sources::Layer;

/// Contributors listed per answer.
pub const CONTRIBUTORS_SHOWN: usize = 30;

/// A layer's state after a ring.
pub struct LayerAnswer {
    pub layer: Layer,
    pub energy: [f64; PERIODS],
    /// Sum of the bounds of the candidates left out.
    pub omitted_bound: [f64; PERIODS],
    pub evaluated: usize,
    pub candidates: usize,
}

/// One contributor group (sources sharing a display group key).
#[derive(Clone)]
pub struct Contributor {
    pub group_key: u64,
    pub layer: Layer,
    pub energy: [f64; PERIODS],
    pub distance_m: f64,
    pub display: DisplayRef,
    /// Its loudest evaluated pieces (ends in the click's frame, equal for a point) with their
    /// Lden-weighted energies: what the map shows of it.
    pub pieces: Vec<([[f64; 2]; 2], f64)>,
}

/// Pieces a contributor keeps for the map.
pub const CONTRIBUTOR_PIECES: usize = 24;

pub struct Statistics {
    pub rings_read: u32,
    pub files: usize,
    pub bytes: u64,
    pub read_seconds: f64,
    /// Parsing tiles and bounding candidates.
    pub candidate_seconds: f64,
    /// The full physics of the selected candidates (and the façade choice).
    pub evaluate_seconds: f64,
    pub elapsed_seconds: f64,
}

/// One streamed update.
pub struct Update<'u> {
    pub partial: bool,
    pub lat: f64,
    pub lon: f64,
    pub frame: LocalFrame,
    pub receiver_altitude_m: f64,
    /// The receiver reflection bonus of the surroundings (dB, 0, 1.5 or 3).
    pub reflection_db: f64,
    /// The building the click stands in, and its chosen façade.
    pub building: Option<BuildingClick>,
    pub layers: Vec<LayerAnswer>,
    pub contributors: Vec<Contributor>,
    /// The loudest flights so far.
    pub flights: Vec<LoudFlight>,
    /// The loudest evaluated pieces per layer (final update, when asked for).
    pub pieces: Vec<EvaluatedPiece>,
    pub statistics: Statistics,
    /// The display JSON of a contributor.
    pub display_json: &'u dyn Fn(DisplayRef, Layer) -> Result<String, String>,
}

pub fn layer_answers(selections: &[LayerSelection]) -> Vec<LayerAnswer> {
    selections
        .iter()
        .map(|selection| LayerAnswer {
            layer: selection.layer,
            energy: selection.answer_energy(),
            omitted_bound: selection.uncertainty(),
            evaluated: selection.evaluated,
            candidates: selection.covered + selection.pending.len(),
        })
        .collect()
}

pub fn loudest_contributors(selections: &[LayerSelection]) -> Vec<Contributor> {
    let mut contributors: Vec<Contributor> = selections
        .iter()
        .flat_map(|selection| selection.contributors.values().cloned())
        .collect();
    contributors.sort_by(|a, b| {
        lden_weighted(&b.energy)
            .total_cmp(&lden_weighted(&a.energy))
            .then(a.group_key.cmp(&b.group_key))
    });
    contributors.truncate(CONTRIBUTORS_SHOWN);
    contributors
}

/// The one and final update of a building without an exposed façade: no levels.
#[allow(clippy::too_many_arguments)]
pub fn empty_answer(
    lat: f64,
    lon: f64,
    frame: LocalFrame,
    ground: &Ground<'_>,
    click: BuildingClick,
    selections: &[LayerSelection],
    (files, bytes, read_seconds, started): (usize, u64, f64, std::time::Instant),
    emit: &mut dyn FnMut(&Update) -> Result<(), String>,
) -> Result<(), String> {
    let no_display = |_: DisplayRef, _: Layer| -> Result<String, String> {
        Err("a building without an exposed façade shows no contributors".into())
    };
    emit(&Update {
        partial: false,
        lat,
        lon,
        frame,
        receiver_altitude_m: ground.at([0.0, 0.0])?.height_m + RECEIVER_HEIGHT_M,
        reflection_db: 0.0,
        building: Some(click),
        layers: layer_answers(selections),
        contributors: Vec::new(),
        flights: Vec::new(),
        pieces: Vec::new(),
        statistics: Statistics {
            rings_read: 1,
            files,
            bytes,
            read_seconds,
            candidate_seconds: 0.0,
            evaluate_seconds: 0.0,
            elapsed_seconds: started.elapsed().as_secs_f64(),
        },
        display_json: &no_display,
    })
}
