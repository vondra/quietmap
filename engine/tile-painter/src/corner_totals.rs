//! Permanent five-layer day/evening/night mean powers; source contributions remain temporary.
use crate::corner_store::CornerEnergy;
use anyhow::{ensure, Result};
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SurfacePeriodTotals(pub [[f32; 3]; 5]);
impl SurfacePeriodTotals {
    pub(crate) fn from_sources(energy: &CornerEnergy) -> Result<Self> {
        let mut totals = [[0.0_f64; 3]; 5];
        for source in &energy.0 {
            ensure!(source.layer < 5, "invalid surface layer");
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
        ensure!(bytes.len() == 60, "invalid surface totals payload");
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
