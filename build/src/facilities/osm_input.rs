//! Convert the shared OSM classifier's sites and leisure rows without reclassifying their tags.

use super::{
    geometry,
    records::{Facility, INDUSTRY, LEISURE},
};
use crate::{
    dev4::{Square, column, read_table, require_stamp},
    year::{Kind, YearInputs},
};
use anyhow::{Result, anyhow, ensure};
use arrow_array::{
    Array, BinaryArray, Float32Array, Int32Array, Int64Array, StringArray, UInt8Array, UInt16Array,
};
use std::path::Path;

pub fn read(inputs: &YearInputs, squares: &[Square]) -> Result<Vec<(Square, Facility)>> {
    let root = inputs
        .required(Kind::OsmGeography)
        .map_err(|e| anyhow!(e))?;
    let observation = &inputs
        .manifest
        .inputs
        .iter()
        .find(|i| i.kind == Kind::PrepareOsm)
        .ok_or_else(|| anyhow!("missing OSM observation"))?
        .observed
        .end;
    let mut rows = Vec::new();
    for square in squares {
        let directory = root
            .join("z9")
            .join(square.x.to_string())
            .join(square.y.to_string());
        ensure!(
            directory.join("square-country-city.bin").is_file(),
            "OSM square not prepared: {square:?}"
        );
        for (family, role) in [("industrial", INDUSTRY), ("leisure", LEISURE)] {
            let path = root
                .join("z9")
                .join(square.x.to_string())
                .join(square.y.to_string())
                .join(format!("{family}.arrow"));
            read_family(&path, role, observation, &mut rows, *square)?;
        }
    }
    Ok(rows)
}

fn read_family(
    path: &Path,
    role: u8,
    observation: &str,
    rows: &mut Vec<(Square, Facility)>,
    square: Square,
) -> Result<()> {
    let Some(table) = read_table(path).map_err(|e| anyhow!(e))? else {
        return Ok(());
    };
    require_stamp(&table, "grid", "z30").map_err(|e| anyhow!(e))?;
    let stamp = if role == INDUSTRY {
        ("osm_industrial_contract", "2")
    } else {
        ("leisure_contract", "leisure_v5")
    };
    require_stamp(&table, stamp.0, stamp.1).map_err(|e| anyhow!(e))?;
    for batch in table.batches {
        macro_rules! values {
            ($type:ty, $name:literal) => {
                column::<$type>(&batch, $name).map_err(|e| anyhow!(e))?
            };
        }
        let xs = values!(Int32Array, "centroid_gx");
        let ys = values!(Int32Array, "centroid_gy");
        let ids = values!(Int64Array, "osm_id");
        let kinds = values!(StringArray, "osm_kind");
        let names = values!(StringArray, "name");
        let tags = values!(StringArray, "osm_tags");
        let geometries = values!(BinaryArray, "geom");
        let areas = values!(Float32Array, "area_m2");
        for i in 0..batch.num_rows() {
            let (latitude, longitude) = crate::dev4::z30_corner_degrees(xs.value(i), ys.value(i));
            let (source_type, subtype, activity, geometry_kind, capacity, country) =
                if role == INDUSTRY {
                    ensure!(
                        values!(UInt16Array, "source_id").value(i) == 0,
                        "facility preparation requires original OSM, not already enriched sites"
                    );
                    let powers = values!(Float32Array, "rated_power_kw");
                    (
                        values!(UInt8Array, "source_type").value(i),
                        values!(UInt8Array, "site_subtype").value(i),
                        0,
                        u8::from(geometries.is_valid(i)),
                        powers.is_valid(i).then(|| f64::from(powers.value(i))),
                        values!(UInt16Array, "country_iso").value(i),
                    )
                } else {
                    (
                        0,
                        0,
                        u16::from(values!(UInt8Array, "sport").value(i)),
                        values!(UInt8Array, "geometry_kind").value(i),
                        None,
                        0,
                    )
                };
            rows.push((
                square,
                Facility {
                    identity: format!("osm/{}/{}", kinds.value(i), ids.value(i)),
                    role,
                    longitude,
                    latitude,
                    geometry: geometries
                        .is_valid(i)
                        .then(|| geometry::mapped_geometry(geometries.value(i)))
                        .transpose()?,
                    geometry_kind,
                    area_m2: areas.is_valid(i).then(|| areas.value(i)),
                    osm_id: ids.value(i),
                    osm_kind: kinds.value(i).into(),
                    name: names.is_valid(i).then(|| names.value(i).into()),
                    osm_tags: tags.value(i).into(),
                    source_type,
                    site_subtype: subtype,
                    activity,
                    capacity,
                    capacity_unit: "kW".into(),
                    operating_from: None,
                    operating_until: None,
                    observed_on: observation.to_owned(),
                    operating_state: if source_type
                        == physics::emission::industrial::SOURCE_INACTIVE
                    {
                        2
                    } else {
                        0
                    },
                    source: "osm".into(),
                    country,
                    ..Facility::default()
                },
            ));
        }
    }
    Ok(())
}
