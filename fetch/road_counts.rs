//! Convert pinned publisher formats into the single dated road-observation input selected by the manifest.

#[path = "road_counts/anas.rs"]
mod anas;
#[path = "road_counts/bast.rs"]
mod bast;
#[path = "road_counts/bast_daily.rs"]
mod bast_daily;
#[path = "road_counts/bw_hourly.rs"]
mod bw_hourly;
#[path = "road_counts/cerema.rs"]
mod cerema;
#[path = "road_counts/city.rs"]
mod city;
#[path = "road_counts/dft.rs"]
mod dft;
#[path = "road_counts/djma.rs"]
mod djma;
#[path = "road_counts/fields.rs"]
mod fields;
#[path = "road_counts/gpr.rs"]
mod gpr;
#[path = "road_counts/hpms.rs"]
mod hpms;
#[path = "road_counts/hpms_gdb.rs"]
mod hpms_gdb;
#[path = "road_counts/hpms_sections.rs"]
mod hpms_sections;
#[path = "road_counts/inweva.rs"]
mod inweva;
#[path = "road_counts/kvl.rs"]
mod kvl;
#[path = "road_counts/mastra.rs"]
mod mastra;
#[path = "road_counts/mitma.rs"]
mod mitma;
#[path = "road_counts/mlit.rs"]
mod mlit;
#[path = "road_counts/mop.rs"]
mod mop;
#[path = "road_counts/mot.rs"]
mod mot;
#[path = "road_counts/nvdb.rs"]
mod nvdb;
#[path = "road_counts/nzta.rs"]
mod nzta;
#[path = "road_counts/pldp.rs"]
mod pldp;
#[path = "road_counts/records.rs"]
mod records;
#[path = "road_counts/sasvz.rs"]
mod sasvz;
#[path = "road_counts/sict.rs"]
mod sict;
#[cfg(test)]
#[path = "road_counts/tests.rs"]
mod tests;
#[path = "road_counts/thai_drr.rs"]
mod thai_drr;
#[path = "road_counts/tii.rs"]
mod tii;
#[path = "road_counts/tmas.rs"]
mod tmas;
#[path = "road_counts/trafik.rs"]
mod trafik;
#[path = "road_counts/tsk.rs"]
mod tsk;
#[path = "road_counts/wien.rs"]
mod wien;

#[path = "road_counts/source.rs"]
mod source;
use source::Source;
#[path = "road_counts/exclusions.rs"]
mod exclusions;
#[path = "road_counts/fields_recipe.rs"]
mod fields_recipe;
#[cfg(test)]
#[path = "road_counts/format_tests.rs"]
mod format_tests;
#[path = "road_counts/geometry.rs"]
mod geometry;

use crate::{
    roads::observation_io,
    year::{Kind, YearInputs},
};
use anyhow::{Result, anyhow, ensure};
use serde::Deserialize;
use std::path::Path;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Recipe {
    objects: Vec<Converter>,
}

#[derive(Deserialize)]
#[serde(tag = "format", rename_all = "kebab-case", deny_unknown_fields)]
enum Converter {
    Fields(Box<fields::Publisher>),
    Inweva(inweva::Recipe),
    ThaiDrr(thai_drr::Recipe),
    Mlit(mlit::Recipe),
    Tsk(tsk::Recipe),
    Wien(wien::Recipe),
    Bast(bast::Recipe),
    BastDaily(bast_daily::Recipe),
    Mot(mot::Recipe),
    Mop(mop::Recipe),
    Hpms(hpms::Recipe),
    HpmsGdb(hpms_gdb::Recipe),
    Sict(sict::Recipe),
    Dft(dft::Recipe),
    Nvdb(nvdb::Recipe),
    Kvl(kvl::Recipe),
    Mastra(mastra::Recipe),
    Trafik(trafik::Recipe),
    Djma(djma::Recipe),
    Cerema(cerema::Recipe),
    Gpr(gpr::Recipe),
    Mitma(mitma::Recipe),
    Pldp(pldp::Recipe),
    Sasvz(sasvz::Recipe),
    Anas(anas::Recipe),
    Nzta(nzta::Recipe),
    Tii(tii::Recipe),
    City(city::Recipe),
    Tmas(tmas::Recipe),
    BwHourly(bw_hourly::Recipe),
}

pub fn convert(inputs: &YearInputs, output: &Path) -> Result<()> {
    let path = inputs
        .required(Kind::RoadConversion)
        .map_err(anyhow::Error::msg)?;
    let recipe: Recipe = serde_json::from_slice(&std::fs::read(path)?)?;
    let conversion = inputs
        .manifest
        .inputs
        .iter()
        .find(|r| r.kind == Kind::RoadConversion)
        .ok_or_else(|| anyhow!("missing converter input"))?;
    ensure!(!recipe.objects.is_empty(), "empty road converter recipe");
    let mut rows = Vec::new();
    for object in recipe.objects {
        let input_id = match &object {
            Converter::Fields(r) => &r.input_id,
            Converter::Inweva(r) => &r.source.input_id,
            Converter::ThaiDrr(r) => &r.source.input_id,
            Converter::Mlit(r) => &r.source.input_id,
            Converter::Tsk(r) => &r.source.input_id,
            Converter::Wien(r) => &r.source.input_id,
            Converter::Bast(r) => &r.source.input_id,
            Converter::BastDaily(r) => &r.source.input_id,
            Converter::Mot(r) => &r.source.input_id,
            Converter::Mop(r) => &r.source.input_id,
            Converter::Hpms(r) => &r.source.input_id,
            Converter::HpmsGdb(r) => &r.source.input_id,
            Converter::Sict(r) => &r.source.input_id,
            Converter::Dft(r) => &r.source.input_id,
            Converter::Nvdb(r) => &r.source.input_id,
            Converter::Kvl(r) => &r.source.input_id,
            Converter::Mastra(r) => &r.source.input_id,
            Converter::Trafik(r) => &r.source.input_id,
            Converter::Djma(r) => &r.source.input_id,
            Converter::Cerema(r) => &r.source.input_id,
            Converter::Gpr(r) => &r.source.input_id,
            Converter::Mitma(r) => &r.source.input_id,
            Converter::Pldp(r) => &r.source.input_id,
            Converter::Sasvz(r) => &r.source.input_id,
            Converter::Anas(r) => &r.source.input_id,
            Converter::Nzta(r) => &r.source.input_id,
            Converter::Tii(r) => &r.source.input_id,
            Converter::City(r) => &r.source.input_id,
            Converter::Tmas(r) => &r.source.input_id,
            Converter::BwHourly(r) => &r.source.input_id,
        };
        ensure!(
            conversion.depends_on.contains(input_id),
            "converter omits raw dependency {input_id}"
        );
        if let Converter::Tsk(recipe) = &object {
            ensure!(
                conversion.depends_on.contains(&recipe.coverage_input),
                "converter omits municipal boundary dependency"
            );
        }
        if let Converter::Fields(recipe) = &object {
            ensure!(
                recipe
                    .exclusion
                    .as_ref()
                    .is_none_or(|r| conversion.depends_on.contains(&r.input_id)),
                "converter omits excluded-network dependency"
            );
            ensure!(
                recipe
                    .coverage
                    .as_ref()
                    .is_none_or(|r| conversion.depends_on.contains(&r.input_id)),
                "converter omits municipal boundary dependency"
            );
        }
        if let Converter::Wien(recipe) = &object {
            ensure!(
                conversion.depends_on.contains(&recipe.coverage.input_id),
                "converter omits municipal boundary dependency"
            );
        }
        if let Converter::Sict(recipe) = &object {
            ensure!(
                conversion.depends_on.contains(&recipe.composition_input),
                "converter omits composition dependency"
            );
        }
        if let Converter::Cerema(recipe) = &object {
            ensure!(
                recipe
                    .older_editions
                    .iter()
                    .all(|edition| conversion.depends_on.contains(&edition.source.input_id)),
                "converter omits older census dependency"
            );
        }
        let sites = match &object {
            Converter::Pldp(r) => Some(&r.sites_input),
            Converter::Sasvz(r) => Some(&r.sites_input),
            Converter::Tii(r) => Some(&r.sites_input),
            Converter::Wien(r) => Some(&r.sites_input),
            _ => None,
        };
        ensure!(
            sites.is_none_or(|id| conversion.depends_on.contains(id)),
            "converter omits station-location dependency"
        );
        let path = inputs.by_id(input_id).map_err(anyhow::Error::msg)?;
        let converted = match &object {
            Converter::Fields(r) => fields::convert(r, &path, inputs)?,
            Converter::Inweva(r) => {
                inweva::convert(r, inputs, &serde_json::from_slice(&std::fs::read(path)?)?)?
            }
            Converter::ThaiDrr(r) => thai_drr::convert(r, inputs, &std::fs::read(path)?)?,
            Converter::Mlit(r) => mlit::convert(r, inputs, &path)?,
            Converter::Tsk(r) => tsk::convert(r, inputs, &path)?,
            Converter::Wien(r) => wien::convert(r, inputs, &path)?,
            Converter::Bast(r) => bast::convert(r, inputs, &path)?,
            Converter::BastDaily(r) => bast_daily::convert(r, inputs, &path)?,
            Converter::Mot(r) => mot::convert(r, inputs, &path)?,
            Converter::Mop(r) => {
                mop::convert(r, inputs, &serde_json::from_slice(&std::fs::read(path)?)?)?
            }
            Converter::Hpms(r) => hpms::convert(r, inputs, &path)?,
            Converter::HpmsGdb(r) => {
                hpms_gdb::convert(r, inputs, &path, output.parent().unwrap_or(Path::new(".")))?
            }
            Converter::Sict(r) => sict::convert(r, inputs, &path)?,
            Converter::Dft(r) => dft::convert(r, inputs, &path)?,
            Converter::Nvdb(r) => {
                nvdb::convert(r, inputs, &serde_json::from_slice(&std::fs::read(path)?)?)?
            }
            Converter::Kvl(r) => kvl::convert(r, inputs, &path)?,
            Converter::Mastra(r) => mastra::convert(r, inputs, &path)?,
            Converter::Trafik(r) => trafik::convert(r, inputs, &path)?,
            Converter::Djma(r) => {
                djma::convert(r, inputs, &serde_json::from_slice(&std::fs::read(path)?)?)?
            }
            Converter::Cerema(r) => cerema::convert(r, inputs, &path)?,
            Converter::Gpr(r) => {
                gpr::convert(r, inputs, &serde_json::from_slice(&std::fs::read(path)?)?)?
            }
            Converter::Mitma(r) => mitma::convert(r, inputs, &std::fs::read_to_string(path)?)?,
            Converter::Pldp(r) => pldp::convert(r, inputs, &path)?,
            Converter::Sasvz(r) => sasvz::convert(r, inputs, &path)?,
            Converter::Tii(r) => tii::convert(r, inputs, &path)?,
            Converter::Tmas(r) => tmas::convert(r, inputs, &path)?,
            Converter::BwHourly(r) => bw_hourly::convert(r, inputs, &path)?,
            Converter::City(r) => {
                city::convert(r, inputs, &serde_json::from_slice(&std::fs::read(path)?)?)?
            }
            Converter::Nzta(r) => nzta::convert(r, inputs, &path)?,
            Converter::Anas(r) => {
                anas::convert(r, inputs, &serde_json::from_slice(&std::fs::read(path)?)?)?
            }
        };
        eprintln!("road-counts: {input_id}: {} observations", converted.len());
        rows.extend(converted);
    }
    observation_io::write(output, &rows, inputs)
}

#[cfg(test)]
#[path = "road_counts/trafik_tests.rs"]
mod trafik_tests;
