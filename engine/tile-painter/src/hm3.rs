//! Quantized Lden HM3 tile bytes in one Brotli stream for the browser's native decoder.
use anyhow::{ensure, Result};
use grid::surface_corner::TILE_PIXEL_SIDE;
use std::io::Cursor;

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
        }
    }
}

pub const SURFACE_LAYERS: [Hm3Layer; 5] = [
    Hm3Layer::Road,
    Hm3Layer::Rail,
    Hm3Layer::Industrial,
    Hm3Layer::Building,
    Hm3Layer::AircraftGround,
];
pub const SOURCE_LAYERS: [Hm3Layer; 7] = [
    SURFACE_LAYERS[0],
    SURFACE_LAYERS[1],
    SURFACE_LAYERS[2],
    SURFACE_LAYERS[3],
    SURFACE_LAYERS[4],
    Hm3Layer::AircraftAirborne,
    Hm3Layer::AircraftCruise,
];

pub const ALL_LAYERS: [Hm3Layer; 8] = [
    SOURCE_LAYERS[0],
    SOURCE_LAYERS[1],
    SOURCE_LAYERS[2],
    SOURCE_LAYERS[3],
    SOURCE_LAYERS[4],
    SOURCE_LAYERS[5],
    SOURCE_LAYERS[6],
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

/// Encode seven outdoor mean-power planes in SOURCE_LAYERS order, followed by total.
/// Sampling-day and period-duration normalization belongs to the producers.
pub fn encode_all_period_powers(
    planes: [&[f32]; 7],
    indoor_attenuation: &[f32],
) -> Result<Vec<EncodedHm3>> {
    let mut tiles = Vec::with_capacity(ALL_LAYERS.len());
    for (layer, plane) in SOURCE_LAYERS.into_iter().zip(planes) {
        tiles.push(encode_period_power(plane, layer, indoor_attenuation)?);
    }
    // Sum before per-layer indoor flooring or lossy quantization, as the popup does.
    let total = (0..indoor_attenuation.len()).map(|pixel| {
        std::array::from_fn(|period| {
            planes
                .iter()
                .map(|plane| f64::from(plane[pixel * 3 + period]))
                .sum()
        })
    });
    tiles.push(encode_pixels(total, Hm3Layer::Total, indoor_attenuation)?);
    Ok(tiles)
}

/// The complete tiles a paint of nothing writes: every pixel NO_DATA (255), because zero
/// energy in every period is -inf dB and stays -inf behind any facade. An owner or
/// tile no source reaches gets these bytes without the card.
pub fn silent_tiles() -> Result<Vec<EncodedHm3>> {
    let pixels = TILE_PIXEL_SIDE * TILE_PIXEL_SIDE;
    ALL_LAYERS
        .into_iter()
        .map(|layer| encode_period_power(&vec![0.0; pixels * 3], layer, &vec![0.0; pixels]))
        .collect()
}

pub fn encode_period_power(
    energy: &[f32],
    layer: Hm3Layer,
    indoor_attenuation: &[f32],
) -> Result<EncodedHm3> {
    let pixels = TILE_PIXEL_SIDE * TILE_PIXEL_SIDE;
    ensure!(
        energy.len() == pixels * 3 && indoor_attenuation.len() == pixels,
        "invalid output tile dimensions"
    );
    encode_pixels(
        energy
            .chunks_exact(3)
            .map(|periods| std::array::from_fn(|i| f64::from(periods[i]))),
        layer,
        indoor_attenuation,
    )
}

fn encode_pixels(
    energy: impl Iterator<Item = [f64; 3]>,
    layer: Hm3Layer,
    indoor_attenuation: &[f32],
) -> Result<EncodedHm3> {
    let pixels = TILE_PIXEL_SIDE * TILE_PIXEL_SIDE;
    let mut raw = Vec::with_capacity(pixels + 6);
    raw.extend_from_slice(b"HM3 \x03");
    raw.push(layer.source_id());
    for (periods, delta) in energy.zip(indoor_attenuation) {
        ensure!(
            periods.iter().all(|p| p.is_finite() && *p >= 0.0)
                && delta.is_finite()
                && *delta >= 0.0,
            "invalid painted power or indoor attenuation"
        );
        let db: [f64; 3] = std::array::from_fn(|i| 10.0 * periods[i].log10());
        let facade = noise_compute::periods::compute_lden(db[0], db[1], db[2]);
        let lden = if *delta > 0.0 {
            noise_compute::envelope::indoor_level_db(facade, f64::from(*delta))
        } else {
            facade
        };
        raw.push(if lden.is_finite() && lden >= 0.0 {
            (lden * 2.0).round().min(254.0) as u8
        } else {
            255
        });
    }
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

    #[test]
    fn all_layers_keep_aircraft_identity_and_sum_before_display_losses() {
        let pixels = TILE_PIXEL_SIDE * TILE_PIXEL_SIDE;
        let mut road = vec![0.0; pixels * 3];
        road[0] = 1.1; // Individually below 0 dB, together visible.
        road[3] = 10.0; // Indoor floor applies independently to total.
        road[6] = (2.0 * 10.0_f64.powf(60.24 / 10.0)) as f32;
        let silent = vec![0.0; pixels * 3];
        let mut delta = vec![0.0; pixels];
        delta[1] = 20.0;
        let planes = [
            &road[..],
            &road[..],
            &silent,
            &silent,
            &silent,
            &silent,
            &silent,
        ];
        let tiles = encode_all_period_powers(planes, &delta).unwrap();
        assert_eq!(tiles.len(), 8);
        let mut decoded = Vec::new();
        for tile in &tiles {
            let mut raw = Vec::new();
            brotli::BrotliDecompress(&mut Cursor::new(tile.as_bytes()), &mut raw).unwrap();
            // Same fixed header and dense body accepted by frontend decodeHM3.
            assert_eq!(raw.len(), 6 + pixels);
            assert_eq!(&raw[..5], b"HM3 \x03");
            assert_eq!(raw[5], tile.layer().source_id());
            decoded.push(raw);
        }
        assert_eq!(&decoded[0][6..9], &[255, 0, 120]);
        assert_eq!(tiles[7].layer(), Hm3Layer::Total);
        assert_eq!(&decoded[7][6..9], &[1, 0, 127]);
        assert_eq!(
            tiles[4..7]
                .iter()
                .map(|tile| tile.layer().name())
                .collect::<Vec<_>>(),
            ["aircraft-ground", "aircraft-airborne", "aircraft-cruise"]
        );
        assert!(decoded[4..7].iter().all(|raw| raw[5] == 3));
        let mut missing = planes;
        missing[6] = &[];
        assert!(encode_all_period_powers(missing, &delta).is_err());
    }

    /// A silent tile carries the bytes the encoder writes for zero energy, with or
    /// without enclosed pixels, and every one of its pixels is NO_DATA.
    #[test]
    fn a_silent_tile_is_the_paint_of_zero_energy_behind_any_facade() {
        let pixels = TILE_PIXEL_SIDE * TILE_PIXEL_SIDE;
        let tiles = silent_tiles().unwrap();
        assert_eq!(
            tiles.iter().map(|tile| tile.layer).collect::<Vec<_>>(),
            ALL_LAYERS
        );
        for tile in &tiles {
            let enclosed =
                encode_period_power(&vec![0.0; pixels * 3], tile.layer, &vec![30.0; pixels])
                    .unwrap();
            assert_eq!(enclosed.bytes, tile.bytes);
            let mut raw = Vec::new();
            brotli::BrotliDecompress(&mut Cursor::new(&tile.bytes), &mut raw).unwrap();
            assert_eq!(
                &raw[..6],
                [b"HM3 \x03".as_slice(), &[tile.layer.source_id()]].concat()
            );
            assert_eq!(raw.len(), pixels + 6);
            assert!(raw[6..].iter().all(|byte| *byte == 255));
        }
    }
}
