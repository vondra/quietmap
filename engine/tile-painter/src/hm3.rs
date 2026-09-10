//! Quantized Lden HM3 tile bytes in one Brotli stream for the browser's native decoder.
use anyhow::{ensure, Result};
use grid::surface_corner::TILE_PIXEL_SIDE;
use std::io::Cursor;

pub const SURFACE_SOURCE_IDS: [u8; 5] = [1, 2, 4, 5, 3];

/// Complete validated HM3 payload; only the encoder can construct it.
#[derive(Clone)]
pub struct EncodedHm3 {
    pub(crate) source_id: u8,
    pub(crate) bytes: Vec<u8>,
}

/// The five tiles a paint of nothing writes: every pixel NO_DATA (255), because zero
/// energy in every period is -inf dB and stays -inf behind any facade. An owner or
/// tile no source reaches gets these bytes without the card.
pub fn silent_tiles() -> Result<Vec<EncodedHm3>> {
    let pixels = TILE_PIXEL_SIDE * TILE_PIXEL_SIDE;
    SURFACE_SOURCE_IDS
        .into_iter()
        .map(|source_id| encode_period_power(&vec![0.0; pixels * 3], source_id, &vec![0.0; pixels]))
        .collect()
}

pub fn encode_period_power(
    energy: &[f32],
    source_id: u8,
    indoor_attenuation: &[f32],
) -> Result<EncodedHm3> {
    ensure!(
        SURFACE_SOURCE_IDS.contains(&source_id),
        "invalid surface source ID"
    );
    let pixels = TILE_PIXEL_SIDE * TILE_PIXEL_SIDE;
    ensure!(
        energy.len() == pixels * 3 && indoor_attenuation.len() == pixels,
        "invalid output tile dimensions"
    );
    let mut raw = Vec::with_capacity(pixels + 6);
    raw.extend_from_slice(b"HM3 \x03");
    raw.push(source_id);
    for (periods, delta) in energy.chunks_exact(3).zip(indoor_attenuation) {
        ensure!(
            periods.iter().all(|p| p.is_finite() && *p >= 0.0)
                && delta.is_finite()
                && *delta >= 0.0,
            "invalid painted power or indoor attenuation"
        );
        let db: [f64; 3] = std::array::from_fn(|i| 10.0 * f64::from(periods[i]).log10());
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
        source_id,
        bytes: compressed,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A silent tile carries the bytes the encoder writes for zero energy, with or
    /// without enclosed pixels, and every one of its pixels is NO_DATA.
    #[test]
    fn a_silent_tile_is_the_paint_of_zero_energy_behind_any_facade() {
        let pixels = TILE_PIXEL_SIDE * TILE_PIXEL_SIDE;
        let tiles = silent_tiles().unwrap();
        assert_eq!(
            tiles.iter().map(|tile| tile.source_id).collect::<Vec<_>>(),
            SURFACE_SOURCE_IDS
        );
        for tile in &tiles {
            let enclosed =
                encode_period_power(&vec![0.0; pixels * 3], tile.source_id, &vec![30.0; pixels])
                    .unwrap();
            assert_eq!(enclosed.bytes, tile.bytes);
            let mut raw = Vec::new();
            brotli::BrotliDecompress(&mut Cursor::new(&tile.bytes), &mut raw).unwrap();
            assert_eq!(
                &raw[..6],
                [b"HM3 \x03".as_slice(), &[tile.source_id]].concat()
            );
            assert_eq!(raw.len(), pixels + 6);
            assert!(raw[6..].iter().all(|byte| *byte == 255));
        }
    }
}
