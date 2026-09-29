//! Quantized Lden HM3 tile bytes in one Brotli stream for the browser's native decoder.
//!
//! Format version 4 (one writer, this module): a 6-byte header `HM3 `, the
//! version, the layer's source id, then 512 × 512 cells. A cell is 2·Lden
//! (0–253, i.e. 0–126.5 dB), [`COMPUTED_SILENCE`] or [`NOT_ASSESSED`]; coverage
//! is never energy (#38): a place no modelled source reaches is quiet, a place
//! nothing was computed for is neither quiet nor loud.
use anyhow::{ensure, Result};
use grid::surface_corner::TILE_PIXEL_SIDE;
use std::io::Cursor;

/// The HM3 format version this crate writes and reads.
pub const HM3_VERSION: u8 = 4;
/// Computed, and no modelled energy reaches 0 dB here: quiet.
pub const COMPUTED_SILENCE: u8 = 254;
/// Not assessed: outside painted coverage, missing input, or a building
/// without an exposed façade.
pub const NOT_ASSESSED: u8 = 255;
/// The loudest level byte: 126.5 dB.
const MAXIMUM_LEVEL_BYTE: f64 = 253.0;

fn header(layer: Hm3Layer) -> [u8; 6] {
    [b'H', b'M', b'3', b' ', HM3_VERSION, layer.source_id()]
}

/// The cell of one pixel's period powers (mean power, day/evening/night).
pub fn lden_cell(periods: [f64; 3]) -> u8 {
    let db: [f64; 3] = std::array::from_fn(|i| 10.0 * periods[i].log10());
    let lden = noise_compute::periods::compute_lden(db[0], db[1], db[2]);
    if lden.is_finite() && lden >= 0.0 {
        (lden * 2.0).round().min(MAXIMUM_LEVEL_BYTE) as u8
    } else {
        COMPUTED_SILENCE
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Hm3Layer {
    Total,
    Road,
    Rail,
    Industrial,
    Building,
    AircraftGround,
    AircraftAirborne,
    AircraftCruise,
    Ship,
}

impl Hm3Layer {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Total => "total",
            Self::Road => "road",
            Self::Rail => "rail",
            Self::Industrial => "industrial",
            Self::Building => "building",
            Self::AircraftGround => "aircraft-ground",
            Self::AircraftAirborne => "aircraft-airborne",
            Self::AircraftCruise => "aircraft-cruise",
            Self::Ship => "ship",
        }
    }

    pub const fn source_id(self) -> u8 {
        match self {
            Self::Total => 0,
            Self::Road => 1,
            Self::Rail => 2,
            Self::Industrial => 4,
            Self::Building => 5,
            Self::AircraftGround | Self::AircraftAirborne | Self::AircraftCruise => 3,
            Self::Ship => 6,
        }
    }
}

/// Planes the surface painter evaluates, in its GPU layer order.
pub const SURFACE_LAYERS: [Hm3Layer; 6] = [
    Hm3Layer::Road,
    Hm3Layer::Rail,
    Hm3Layer::Industrial,
    Hm3Layer::Building,
    Hm3Layer::AircraftGround,
    Hm3Layer::Ship,
];
/// Published source planes: the aircraft field planes sit between ground ops and ships.
pub const SOURCE_LAYERS: [Hm3Layer; 8] = [
    SURFACE_LAYERS[0],
    SURFACE_LAYERS[1],
    SURFACE_LAYERS[2],
    SURFACE_LAYERS[3],
    SURFACE_LAYERS[4],
    Hm3Layer::AircraftAirborne,
    Hm3Layer::AircraftCruise,
    SURFACE_LAYERS[5],
];

pub const ALL_LAYERS: [Hm3Layer; 9] = [
    SOURCE_LAYERS[0],
    SOURCE_LAYERS[1],
    SOURCE_LAYERS[2],
    SOURCE_LAYERS[3],
    SOURCE_LAYERS[4],
    SOURCE_LAYERS[5],
    SOURCE_LAYERS[6],
    SOURCE_LAYERS[7],
    Hm3Layer::Total,
];

/// Complete validated HM3 payload; only the encoder can construct it.
#[derive(Clone)]
pub struct EncodedHm3 {
    pub(crate) layer: Hm3Layer,
    pub(crate) bytes: Vec<u8>,
}

impl EncodedHm3 {
    pub fn layer(&self) -> Hm3Layer {
        self.layer
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }
}

/// Encode the mean-power planes in SOURCE_LAYERS order, followed by their total.
/// Sampling-day and period-duration normalization belongs to the producers.
/// A pixel with `assessed[pixel] == false` is [`NOT_ASSESSED`] in every layer.
pub fn encode_all_period_powers(
    planes: [&[f32]; SOURCE_LAYERS.len()],
    assessed: &[bool],
) -> Result<Vec<EncodedHm3>> {
    let pixels = TILE_PIXEL_SIDE * TILE_PIXEL_SIDE;
    ensure!(
        assessed.len() == pixels && planes.iter().all(|plane| plane.len() == pixels * 3),
        "invalid output tile dimensions"
    );
    ensure!(
        planes
            .iter()
            .all(|plane| plane.iter().all(|power| power.is_finite() && *power >= 0.0)),
        "invalid painted power"
    );
    let cells = |power: &dyn Fn(usize, usize) -> f64| -> Vec<u8> {
        (0..pixels)
            .map(|pixel| {
                if assessed[pixel] {
                    lden_cell(std::array::from_fn(|period| power(pixel, period)))
                } else {
                    NOT_ASSESSED
                }
            })
            .collect()
    };
    let mut tiles = Vec::with_capacity(ALL_LAYERS.len());
    for (layer, plane) in SOURCE_LAYERS.into_iter().zip(planes) {
        let layer_cells = cells(&|pixel, period| f64::from(plane[pixel * 3 + period]));
        tiles.push(encode_cells(&layer_cells, layer)?);
    }
    // Sum before lossy quantization, as the popup does.
    let total_cells = cells(&|pixel, period| {
        planes
            .iter()
            .map(|plane| f64::from(plane[pixel * 3 + period]))
            .sum()
    });
    tiles.push(encode_cells(&total_cells, Hm3Layer::Total)?);
    Ok(tiles)
}

/// Encode already-quantized cells as one HM3 stream.
pub fn encode_cells(cells: &[u8], layer: Hm3Layer) -> Result<EncodedHm3> {
    ensure!(
        cells.len() == TILE_PIXEL_SIDE * TILE_PIXEL_SIDE,
        "invalid HM3 cell count"
    );
    let mut raw = Vec::with_capacity(cells.len() + 6);
    raw.extend_from_slice(&header(layer));
    raw.extend_from_slice(cells);
    compress_raw(raw, layer)
}

/// Decode HM3 bytes to quantized cells, rejecting truncation, trailing bytes
/// or a header that names another layer.
pub fn decode_cells(bytes: &[u8], layer: Hm3Layer) -> Result<Vec<u8>> {
    use std::io::Read;
    let expected = TILE_PIXEL_SIDE * TILE_PIXEL_SIDE + 6;
    let mut raw = Vec::with_capacity(expected);
    brotli::Decompressor::new(Cursor::new(bytes), 4096)
        .take((expected + 1) as u64)
        .read_to_end(&mut raw)?;
    ensure!(
        raw.len() == expected && raw[..6] == header(layer),
        "invalid HM3 header, layer or dimensions"
    );
    Ok(raw.split_off(6))
}

fn compress_raw(raw: Vec<u8>, layer: Hm3Layer) -> Result<EncodedHm3> {
    let mut compressed = Vec::new();
    brotli::BrotliCompress(
        &mut Cursor::new(raw),
        &mut compressed,
        &brotli::enc::BrotliEncoderParams {
            quality: 9,
            ..Default::default()
        },
    )?;
    Ok(EncodedHm3 {
        layer,
        bytes: compressed,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(tile: &EncodedHm3) -> Vec<u8> {
        let mut raw = Vec::new();
        brotli::BrotliDecompress(&mut Cursor::new(tile.as_bytes()), &mut raw).unwrap();
        raw
    }

    #[test]
    fn all_layers_keep_aircraft_identity_and_sum_before_display_losses() {
        let pixels = TILE_PIXEL_SIDE * TILE_PIXEL_SIDE;
        let mut road = vec![0.0; pixels * 3];
        road[0] = 1.1; // Individually below 0 dB, together audible.
        road[6] = (2.0 * 10.0_f64.powf(60.24 / 10.0)) as f32;
        let silent = vec![0.0; pixels * 3];
        let assessed = vec![true; pixels];
        let planes = [
            &road[..],
            &road[..],
            &silent,
            &silent,
            &silent,
            &silent,
            &silent,
            &silent,
        ];
        let tiles = encode_all_period_powers(planes, &assessed).unwrap();
        assert_eq!(tiles.len(), 9);
        let decoded: Vec<_> = tiles.iter().map(raw).collect();
        for (tile, raw) in tiles.iter().zip(&decoded) {
            // Same fixed header and dense body accepted by frontend decodeHM3.
            assert_eq!(raw.len(), 6 + pixels);
            assert_eq!(&raw[..5], b"HM3 \x04");
            assert_eq!(raw[5], tile.layer().source_id());
        }
        assert_eq!(&decoded[0][6..9], &[COMPUTED_SILENCE, COMPUTED_SILENCE, 120]);
        assert_eq!(decoded[2][6], COMPUTED_SILENCE, "zero energy is computed silence");
        assert_eq!(tiles[7].layer(), Hm3Layer::Ship);
        assert_eq!(decoded[7][5], 6);
        assert_eq!(tiles[8].layer(), Hm3Layer::Total);
        assert_eq!(&decoded[8][6..9], &[1, COMPUTED_SILENCE, 127]);
        assert_eq!(
            tiles[4..7]
                .iter()
                .map(|tile| tile.layer().name())
                .collect::<Vec<_>>(),
            ["aircraft-ground", "aircraft-airborne", "aircraft-cruise"]
        );
        assert!(decoded[4..7].iter().all(|raw| raw[5] == 3));
        let mut missing = planes;
        missing[7] = &[];
        assert!(encode_all_period_powers(missing, &assessed).is_err());
    }

    /// Coverage is not energy (#38): a pixel nothing was assessed for is 255 in
    /// every layer and the total, whatever power its planes carry, and the
    /// loudest level stays below the two codes.
    #[test]
    fn unassessed_pixels_are_not_assessed_in_every_layer_and_levels_stop_at_253() {
        let pixels = TILE_PIXEL_SIDE * TILE_PIXEL_SIDE;
        let loud = vec![1.0e14_f32; pixels * 3];
        let mut assessed = vec![true; pixels];
        assessed[1] = false;
        let tiles = encode_all_period_powers([&loud[..]; SOURCE_LAYERS.len()], &assessed).unwrap();
        for tile in &tiles {
            let raw = raw(tile);
            assert_eq!(raw[6], 253);
            assert_eq!(raw[7], NOT_ASSESSED);
        }
        assert_eq!(lden_cell([0.0; 3]), COMPUTED_SILENCE);
        assert_eq!(lden_cell([0.1; 3]), COMPUTED_SILENCE, "below 0 dB is quiet, not unassessed");
    }

    #[test]
    fn a_version_three_tile_is_refused() {
        let cells = vec![COMPUTED_SILENCE; TILE_PIXEL_SIDE * TILE_PIXEL_SIDE];
        let mut stream = vec![b'H', b'M', b'3', b' ', 3, Hm3Layer::Road.source_id()];
        stream.extend_from_slice(&cells);
        let mut compressed = Vec::new();
        brotli::BrotliCompress(&mut Cursor::new(stream), &mut compressed, &Default::default())
            .unwrap();
        assert!(decode_cells(&compressed, Hm3Layer::Road).is_err());
        let current = encode_cells(&cells, Hm3Layer::Road).unwrap();
        assert_eq!(decode_cells(current.as_bytes(), Hm3Layer::Road).unwrap(), cells);
    }
}
