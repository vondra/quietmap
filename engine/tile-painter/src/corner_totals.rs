//! Permanent per-surface-layer day/evening/night mean powers; source contributions remain temporary.
use crate::corner_store::CornerEnergy;
use crate::hm3::SURFACE_LAYERS;
use anyhow::{ensure, Result};

/// Surface planes a corner totals: the painter's GPU layers, in `SURFACE_LAYERS` order.
pub const SURFACE_LAYER_COUNT: usize = SURFACE_LAYERS.len();
/// Bytes of one encoded totals payload: three f32 periods per surface layer.
pub const TOTALS_BYTES: usize = SURFACE_LAYER_COUNT * 3 * 4;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SurfacePeriodTotals(pub [[f32; 3]; SURFACE_LAYER_COUNT]);
impl SurfacePeriodTotals {
    pub(crate) fn from_sources(energy: &CornerEnergy) -> Result<Self> {
        let mut totals = [[0.0_f64; 3]; SURFACE_LAYER_COUNT];
        for source in &energy.0 {
            ensure!(
                usize::from(source.layer) < SURFACE_LAYER_COUNT,
                "invalid surface layer"
            );
            for (total, value) in totals[source.layer as usize].iter_mut().zip(source.periods) {
                ensure!(value.is_finite() && value >= 0.0, "invalid period power");
                *total += f64::from(value);
            }
        }
        let totals = Self(totals.map(|periods| periods.map(|value| value as f32)));
        totals.encode()?;
        Ok(totals)
    }
    pub(crate) fn encode(self) -> Result<Vec<u8>> {
        ensure!(
            self.0
                .iter()
                .flatten()
                .all(|value| value.is_finite() && *value >= 0.0),
            "invalid surface total"
        );
        Ok(self
            .0
            .into_iter()
            .flatten()
            .flat_map(f32::to_le_bytes)
            .collect())
    }
    pub(crate) fn decode(bytes: &[u8]) -> Result<Self> {
        ensure!(bytes.len() == TOTALS_BYTES, "invalid surface totals payload");
        let totals = Self(std::array::from_fn(|layer| {
            std::array::from_fn(|period| {
                let offset = (layer * 3 + period) * 4;
                f32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
            })
        }));
        totals.encode()?;
        Ok(totals)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::corner_store::{SourceEnergy, SourceIdentity};

    /// Every painter surface layer, ships included, totals; the first layer past them is refused.
    #[test]
    fn totals_cover_every_surface_layer_and_refuse_the_next() {
        let source = |layer: u8| SourceEnergy {
            layer,
            source: SourceIdentity([layer; 32]),
            periods: [1.0, 2.0, 3.0],
        };
        let energy = CornerEnergy((0..SURFACE_LAYER_COUNT as u8).map(source).collect());
        let totals = SurfacePeriodTotals::from_sources(&energy).unwrap();
        assert_eq!(totals.0[SURFACE_LAYER_COUNT - 1], [1.0, 2.0, 3.0]);
        let bytes = totals.encode().unwrap();
        assert_eq!(bytes.len(), TOTALS_BYTES);
        assert_eq!(SurfacePeriodTotals::decode(&bytes).unwrap(), totals);
        let beyond = CornerEnergy(vec![source(SURFACE_LAYER_COUNT as u8)]);
        assert!(SurfacePeriodTotals::from_sources(&beyond).is_err());
    }
}
