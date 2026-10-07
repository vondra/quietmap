//! One streamed update of a click: every layer's totals and account, the loudest contributor
//! groups, what was read, and the building a click stands in.

use crate::aircraft::flights::LoudFlight;
use crate::answer::RECEIVER_HEIGHT_M;
use crate::building::BuildingClick;
use crate::candidates::DisplayRef;
use crate::listing::EvaluatedPiece;
use crate::scene::Ground;
use crate::selection::LayerSelection;
use physics::bands::PERIODS;
use physics::bands::lden_energy;
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
    /// Its energy per meteorological state, kept apart by its pieces' favourable shares.
    pub weather: crate::percentiles::Weather,
    pub distance_m: f64,
    pub display: DisplayRef,
    /// Its loudest evaluated pieces (ends in the click's frame, equal for a point) with their
    /// Lden-weighted energies: what the map shows of it while the click is computed.
    pub pieces: Vec<([[f64; 2]; 2], f64)>,
    /// All of it within the reach as lines (the final update's): what the map shows then.
    pub lines: crate::lines::Lines,
    /// How it is heard: its passes per hour and whether they run together (the final update's;
    /// `None` for a steady source).
    pub heard: Option<crate::percentiles::Heard>,
    /// Its energy exceeded 5 % of the time by itself, per period (the final update's).
    pub loud: Option<[f64; PERIODS]>,
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
    /// The levels exceeded 10, 50 and 90 % of the time (the final update's; none for a building
    /// without a façade).
    pub percentiles: Option<crate::percentiles::Percentiles>,
    /// How loud the click sounds, N5 (the final update's).
    pub loudness: Option<crate::loudness::Loudness>,
    /// The aircraft layer's energy exceeded 5 % of the time: its flights' L5 and its airport
    /// movements steady (the final update's).
    pub aircraft_loud: Option<[f64; PERIODS]>,
    /// What the aircraft layer is made of, Lden energies (final update): airliners, regional and
    /// business jets, propeller aircraft, helicopters, airport ground operations.
    pub aircraft_kinds: Option<[f64; 5]>,
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
    /// The display record of a contributor: its display fields by name.
    pub display_record: &'u dyn Fn(DisplayRef, Layer) -> Result<serde_json::Value, String>,
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

/// Every contributor, the loudest first by `key` (its Lden, or in the final update its loud
/// moments, as the visitor's list ranks them).
pub fn ranked_contributors(
    mut contributors: Vec<Contributor>,
    key: impl Fn(&Contributor) -> [f64; PERIODS],
) -> Vec<Contributor> {
    contributors.sort_by(|a, b| {
        lden_energy(&key(b))
            .total_cmp(&lden_energy(&key(a)))
            .then(a.group_key.cmp(&b.group_key))
    });
    contributors
}

pub fn all_contributors(selections: &[LayerSelection]) -> Vec<Contributor> {
    selections
        .iter()
        .flat_map(|selection| selection.contributors.values().cloned())
        .collect()
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
    let no_display = |_: DisplayRef, _: Layer| -> Result<serde_json::Value, String> {
        Err("a building without an exposed façade shows no contributors".into())
    };
    emit(&Update {
        partial: false,
        percentiles: None,
        loudness: None,
        aircraft_loud: None,
        aircraft_kinds: None,
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
        display_record: &no_display,
    })
}
