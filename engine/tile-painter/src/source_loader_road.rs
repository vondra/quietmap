//! Read `roads.arrow` for a set of H3 R4 hex cells into per-segment rows
//! with pre-computed per-period emission. Each on-disk row is one road
//! microsegment (geometry + traffic columns); there is no per-vehicle-class
//! row explosion, so no group-by is needed (unlike airport_traffic).
//!
//! Normalisation + emission run ONCE here at load (not per pixel): the raw
//! row → [`RawRoadInput`] → [`normalize_road_with_cache`] (drops tunnels /
//! no-access, applies one-way × access × lane factors, junction speed cap,
//! surface correction) → [`NormalizedRoad::period_emissions`] (CNOSSOS
//! Annex II `L_W'/m` per octave band, per period). We store the per-period
//! emission as LINEAR band energy (`10^(dB/10)`) so the scatter hot loop
//! multiplies by a shared per-pixel path factor without a per-pixel `exp`.
//!
//! Column reads mirror the popup road reader (`source-reader::hex_store`):
//! only the geometry is required; every attribute column defaults when
//! absent, so base (un-enriched) extracts — which lack `aadt_*` — still load
//! with class-default traffic. roads.arrow has no aircraft `schema_version`,
//! so the surface read path skips that gate (per-column reads are the gate).
//!
//! `admin` drives the class-default AADT cascade for unenriched rows; the
//! popup resolves it per receiver, the heatmap once per region. Rows carrying
//! the M3 baked triplet (`country_iso`/`city_id`/`continent`) override it PER
//! SEGMENT — the row's own country drives the cascade (plan M4); only a batch
//! WITHOUT the `country_iso` column falls back to the region admin (pre-bake
//! arrows, byte-identical to before). Enriched roads (real AADT) ignore admin
//! entirely, so the motorway parity tile is exact.

use std::collections::HashMap;
use std::path::Path;

use anyhow::Result;
use arrow::array::{BooleanArray, Float32Array, Float64Array, Int32Array, UInt16Array, UInt8Array};
use arrow::record_batch::RecordBatch;
use noise_compute::admin::Admin;
use noise_compute::defaults::{baked_admin, build_traffic_default_cache, Aadt, WORLD_DEFAULT};
use noise_compute::normalize::{normalize_road_with_cache, RawRoadInput};
use noise_compute::propagation::geo::flat_dist;
use noise_compute::sources::provenance_of;

use crate::source_line::{db_bands_to_lin, opt, LineRow};

/// Cache key for the per-admin default-AADT cascade: the baked
/// (`country_iso`, `city_id`, `continent`) tuple — already `Hash`, unlike
/// `Admin` itself (kept `Hash`-less so `types/inputs.rs` stays untouched).
type AdminKey = ([u8; 2], u16, u8);

pub struct RoadData {
    rows: Vec<LineRow>,
}

impl RoadData {
    /// Load + normalise every `roads.arrow` row across `r4_hexes`. `admin`
    /// is the region's admin — the default-AADT cascade fallback for rows
    /// whose batch carries no baked triplet. Missing files are skipped
    /// (rural R4s have no roads).
    pub fn load_for_r4s(h3r4_dir: &Path, r4_hexes: &[u64], admin: Admin) -> Result<Self> {
        // Build the 13-class default-AADT cascade ONCE per load for the
        // region admin (the pre-bake fallback path — identical to before),
        // not per row — `normalize_road` would rebuild it each call. Baked
        // per-row admins re-key the lazily-filled map; a region carries a
        // handful of distinct tuples even on a border.
        let region_cache = build_traffic_default_cache(admin);
        let mut caches: HashMap<AdminKey, [Aadt; WORLD_DEFAULT.len()]> = HashMap::new();
        let mut rows = Vec::new();
        for &r4 in r4_hexes {
            crate::schema_check::read_surface_arrow_for_r4(h3r4_dir, r4, "roads.arrow", |batch| {
                absorb_batch(batch, admin, &region_cache, &mut caches, &mut rows)
            })?;
        }
        Ok(Self { rows })
    }

    pub fn into_rows(self) -> Vec<LineRow> {
        self.rows
    }
}

fn absorb_batch(
    batch: &RecordBatch,
    region_admin: Admin,
    region_cache: &[Aadt; WORLD_DEFAULT.len()],
    caches: &mut HashMap<AdminKey, [Aadt; WORLD_DEFAULT.len()]>,
    out: &mut Vec<LineRow>,
) -> Result<()> {
    let n = batch.num_rows();
    if n == 0 {
        return Ok(());
    }
    // Geometry is required; a batch lacking it has nothing to scatter.
    let (Some(slat), Some(slon), Some(elat), Some(elon)) = (
        opt::<Float64Array>(batch, "start_lat"),
        opt::<Float64Array>(batch, "start_lon"),
        opt::<Float64Array>(batch, "end_lat"),
        opt::<Float64Array>(batch, "end_lon"),
    ) else {
        return Ok(());
    };
    // Every attribute column defaults when absent — the popup's lenient
    // defaults for the acoustic fields (osm_id / length_m differ harmlessly:
    // we don't read identity, and length falls back to the endpoint distance).
    let length = opt::<Float32Array>(batch, "length_m");
    let rclass = opt::<UInt8Array>(batch, "road_class");
    let speed = opt::<UInt8Array>(batch, "speed_limit");
    // Absent on pre-taper arrows → 0 = none (R7 taper writes it).
    let speed_taper = opt::<UInt8Array>(batch, "speed_taper");
    let surface = opt::<UInt8Array>(batch, "surface_type");
    let oneway = opt::<BooleanArray>(batch, "oneway");
    let lanes = opt::<UInt8Array>(batch, "lanes");
    let access = opt::<UInt8Array>(batch, "access");
    let junction = opt::<UInt8Array>(batch, "junction");
    let aadt_l = opt::<Int32Array>(batch, "aadt_light");
    let aadt_m = opt::<Int32Array>(batch, "aadt_medium");
    let aadt_h = opt::<Int32Array>(batch, "aadt_heavy");
    let aadt_mo = opt::<Int32Array>(batch, "aadt_moto");
    let source_id = opt::<UInt16Array>(batch, "source_id");
    let bridge = opt::<BooleanArray>(batch, "bridge");
    let tunnel = opt::<BooleanArray>(batch, "tunnel");
    // Absent on pre-migration arrows → 0 = unknown → the legacy speed table.
    let built_up = opt::<UInt8Array>(batch, "built_up");
    // M3 baked admin triplet (all-or-none at bake time). The `country_iso`
    // column's PRESENCE is the fallback switch: a present 0 bakes
    // `Admin::UNKNOWN` (WORLD defaults, NO region fallback); only an ABSENT
    // column takes the region admin. `opt` treats a wrong-typed column as
    // absent — the tolerant reader never hard-fails (the bake does).
    let country_iso = opt::<UInt16Array>(batch, "country_iso");
    let city_id = opt::<UInt16Array>(batch, "city_id");
    let continent = opt::<UInt8Array>(batch, "continent");

    for i in 0..n {
        // The row's own baked admin when the column is present, else the
        // region admin (pre-bake arrows). The defaults cache follows: the
        // shared region cache for the fallback, a per-tuple entry otherwise.
        let (admin, cache) = match country_iso {
            Some(iso_col) => {
                let a = baked_admin(
                    iso_col.value(i),
                    city_id.map(|c| c.value(i)).unwrap_or(0),
                    continent.map(|c| c.value(i)).unwrap_or(0),
                );
                let key: AdminKey = (a.country_iso, a.city_id, a.continent as u8);
                let cache = caches
                    .entry(key)
                    .or_insert_with(|| build_traffic_default_cache(a));
                (a, &*cache)
            }
            None => (region_admin, region_cache),
        };
        let raw = RawRoadInput {
            road_class: rclass.map(|a| a.value(i)).unwrap_or(0),
            speed_limit: speed.map(|a| a.value(i)).unwrap_or(0),
            speed_taper: speed_taper.map(|a| a.value(i)).unwrap_or(0),
            surface_type: surface.map(|a| a.value(i)).unwrap_or(0),
            oneway: oneway.map(|a| a.value(i)).unwrap_or(false),
            lanes: lanes.map(|a| a.value(i)).unwrap_or(0),
            aadt_light: aadt_l.map(|a| a.value(i)).unwrap_or(0),
            aadt_medium: aadt_m.map(|a| a.value(i)).unwrap_or(0),
            aadt_heavy: aadt_h.map(|a| a.value(i)).unwrap_or(0),
            aadt_moto: aadt_mo.map(|a| a.value(i)).unwrap_or(0),
            provenance: provenance_of(source_id.map(|a| a.value(i)).unwrap_or(0)),
            tunnel: tunnel.map(|a| a.value(i)).unwrap_or(false),
            access: access.map(|a| a.value(i)).unwrap_or(0),
            junction: junction.map(|a| a.value(i)).unwrap_or(0),
            built_up: built_up.map(|a| a.value(i)).unwrap_or(0),
        };
        // Drops tunnels / access=no — the same gate the popup applies.
        let Some(norm) = normalize_road_with_cache(raw, admin, cache) else {
            continue;
        };
        let s_lat = slat.value(i);
        let s_lon = slon.value(i);
        let e_lat = elat.value(i);
        let e_lon = elon.value(i);
        // length_m is always written by osm-extract; fall back to the
        // endpoint distance if a stray batch omits/zeroes it.
        let len_m = length
            .map(|a| a.value(i))
            .filter(|l| *l > 0.0)
            .unwrap_or_else(|| flat_dist(s_lat, s_lon, e_lat, e_lon) as f32);
        let (day, eve, night) = norm.period_emissions();
        out.push(LineRow {
            start_lat: s_lat,
            start_lon: s_lon,
            end_lat: e_lat,
            end_lon: e_lon,
            length_m: len_m,
            max_distance_m: norm.max_distance_m,
            source_height_m: norm.source_height_m,
            bridge: bridge.map(|a| a.value(i)).unwrap_or(false),
            emission_lin: [
                db_bands_to_lin(day),
                db_bands_to_lin(eve),
                db_bands_to_lin(night),
            ],
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow::array::{ArrayRef, Int64Array};
    use arrow::datatypes::{DataType, Field, Schema};
    use noise_compute::admin::Continent;
    use noise_compute::types::NUM_BANDS;
    use std::sync::Arc;

    /// TH admin for the baked-vs-region fixtures (explicit country arm on the
    /// unenriched secondary test row — CZ would GDP-scale only classes 0-2
    /// and links, leaving class 3 at WORLD and unable to prove the override).
    const TH: Admin = Admin {
        continent: Continent::Asia,
        country_iso: *b"TH",
        city_id: 0,
    };

    /// Absorb one batch with the same wiring `load_for_r4s` uses: one region
    /// cache + a lazily-filled per-tuple map.
    fn absorb(batch: &RecordBatch, region: Admin, out: &mut Vec<LineRow>) {
        let region_cache = build_traffic_default_cache(region);
        let mut caches = HashMap::new();
        absorb_batch(batch, region, &region_cache, &mut caches, out).unwrap();
    }

    /// Build a one-row roads batch; `drop` removes a named column to test
    /// the geometry gate / attribute leniency. A motorway (class 0) with
    /// enriched AADT so the admin default cascade is bypassed (exact emission).
    fn road_batch(tunnel: bool, access: u8, drop: Option<&str>) -> RecordBatch {
        let cols: Vec<(Field, ArrayRef)> = vec![
            (
                Field::new("osm_id", DataType::Int64, false),
                Arc::new(Int64Array::from(vec![1i64])),
            ),
            (
                Field::new("start_lat", DataType::Float64, false),
                Arc::new(Float64Array::from(vec![50.10])),
            ),
            (
                Field::new("start_lon", DataType::Float64, false),
                Arc::new(Float64Array::from(vec![14.30])),
            ),
            (
                Field::new("end_lat", DataType::Float64, false),
                Arc::new(Float64Array::from(vec![50.11])),
            ),
            (
                Field::new("end_lon", DataType::Float64, false),
                Arc::new(Float64Array::from(vec![14.31])),
            ),
            (
                Field::new("length_m", DataType::Float32, false),
                Arc::new(Float32Array::from(vec![120.0f32])),
            ),
            (
                Field::new("road_class", DataType::UInt8, false),
                Arc::new(UInt8Array::from(vec![0u8])),
            ),
            (
                Field::new("speed_limit", DataType::UInt8, false),
                Arc::new(UInt8Array::from(vec![130u8])),
            ),
            (
                Field::new("surface_type", DataType::UInt8, false),
                Arc::new(UInt8Array::from(vec![0u8])),
            ),
            (
                Field::new("oneway", DataType::Boolean, false),
                Arc::new(BooleanArray::from(vec![false])),
            ),
            (
                Field::new("lanes", DataType::UInt8, false),
                Arc::new(UInt8Array::from(vec![4u8])),
            ),
            (
                Field::new("access", DataType::UInt8, false),
                Arc::new(UInt8Array::from(vec![access])),
            ),
            (
                Field::new("junction", DataType::UInt8, false),
                Arc::new(UInt8Array::from(vec![0u8])),
            ),
            (
                Field::new("aadt_light", DataType::Int32, false),
                Arc::new(Int32Array::from(vec![50_000i32])),
            ),
            (
                Field::new("aadt_medium", DataType::Int32, false),
                Arc::new(Int32Array::from(vec![5_000i32])),
            ),
            (
                Field::new("aadt_heavy", DataType::Int32, false),
                Arc::new(Int32Array::from(vec![8_000i32])),
            ),
            (
                Field::new("aadt_moto", DataType::Int32, false),
                Arc::new(Int32Array::from(vec![500i32])),
            ),
            (
                Field::new("source_id", DataType::UInt16, false),
                Arc::new(UInt16Array::from(vec![1u16])),
            ),
            (
                Field::new("tunnel", DataType::Boolean, false),
                Arc::new(BooleanArray::from(vec![tunnel])),
            ),
        ];
        let cols: Vec<_> = cols
            .into_iter()
            .filter(|(f, _)| Some(f.name().as_str()) != drop)
            .collect();
        let fields: Vec<Field> = cols.iter().map(|(f, _)| f.clone()).collect();
        let arrs: Vec<ArrayRef> = cols.into_iter().map(|(_, a)| a).collect();
        RecordBatch::try_new(Arc::new(Schema::new(fields)), arrs).unwrap()
    }

    #[test]
    fn loads_and_precomputes_positive_emission() {
        let mut rows = Vec::new();
        absorb(&road_batch(false, 0, None), Admin::UNKNOWN, &mut rows);
        assert_eq!(rows.len(), 1);
        let r = &rows[0];
        assert!((r.length_m - 120.0).abs() < 1e-3);
        assert_eq!(r.max_distance_m, 10_000.0, "motorway reach");
        assert!((r.source_height_m - 0.05).abs() < 1e-9);
        for p in 0..3 {
            assert!(
                r.emission_lin[p].iter().all(|&e| e > 0.0),
                "period {p} band energy"
            );
        }
        let sum = |b: &[f32; NUM_BANDS]| b.iter().sum::<f32>();
        assert!(
            sum(&r.emission_lin[0]) > sum(&r.emission_lin[2]),
            "day > night energy"
        );
    }

    #[test]
    fn tunnel_and_no_access_rows_are_dropped() {
        let mut rows = Vec::new();
        absorb(&road_batch(true, 0, None), Admin::UNKNOWN, &mut rows);
        assert!(rows.is_empty(), "tunnel dropped");
        // access=no with MEASURED AADT (fixture: source_id=1 GlobalMeasured,
        // aadt_light 50k) passes the gate — measured reality outranks the tag
        // (normalize/road.rs, Neratovice case 2026-07-10).
        absorb(&road_batch(false, 2, None), Admin::UNKNOWN, &mut rows);
        assert_eq!(rows.len(), 1, "access=no + measured AADT emits");
        rows.clear();
        // Same row WITHOUT the aadt_light column (reads 0): fail-closed drop —
        // a measured stamp with zero light traffic must not fall through to
        // class defaults on a closed road.
        absorb(
            &road_batch(false, 2, Some("aadt_light")),
            Admin::UNKNOWN,
            &mut rows,
        );
        assert!(
            rows.is_empty(),
            "access=no without light traffic stays dropped"
        );
    }

    #[test]
    fn missing_geometry_skips_batch_missing_attribute_defaults() {
        // Dropping a geometry column → whole batch skipped (popup-identical).
        let mut rows = Vec::new();
        absorb(
            &road_batch(false, 0, Some("start_lat")),
            Admin::UNKNOWN,
            &mut rows,
        );
        assert!(rows.is_empty(), "no geometry → skip");
        // Dropping an attribute column (no aadt_* → base extract) still
        // loads with class-default traffic, NOT an error.
        absorb(
            &road_batch(false, 0, Some("aadt_light")),
            Admin::UNKNOWN,
            &mut rows,
        );
        assert_eq!(
            rows.len(),
            1,
            "missing aadt_light defaults, row still loads"
        );
        assert!(rows[0].emission_lin[0].iter().all(|&e| e > 0.0));
    }

    // ── M4 per-segment admin gates ──────────────────────────────────────────

    /// One-row UNENRICHED secondary (class 3, tagged speed 50 — the admin
    /// affects ONLY the AADT cascade) with no aadt/source_id columns, so the
    /// class-default cascade decides the emission. `triplet` appends the M3
    /// baked columns as `(packed_iso, city_id, continent)`.
    fn unenriched_batch(triplet: Option<(u16, u16, u8)>) -> RecordBatch {
        let mut cols: Vec<(Field, ArrayRef)> = vec![
            (
                Field::new("start_lat", DataType::Float64, false),
                Arc::new(Float64Array::from(vec![50.10])),
            ),
            (
                Field::new("start_lon", DataType::Float64, false),
                Arc::new(Float64Array::from(vec![14.30])),
            ),
            (
                Field::new("end_lat", DataType::Float64, false),
                Arc::new(Float64Array::from(vec![50.11])),
            ),
            (
                Field::new("end_lon", DataType::Float64, false),
                Arc::new(Float64Array::from(vec![14.31])),
            ),
            (
                Field::new("length_m", DataType::Float32, false),
                Arc::new(Float32Array::from(vec![120.0f32])),
            ),
            (
                Field::new("road_class", DataType::UInt8, false),
                Arc::new(UInt8Array::from(vec![3u8])),
            ),
            (
                Field::new("speed_limit", DataType::UInt8, false),
                Arc::new(UInt8Array::from(vec![50u8])),
            ),
        ];
        if let Some((iso, city, cont)) = triplet {
            cols.push((
                Field::new("country_iso", DataType::UInt16, false),
                Arc::new(UInt16Array::from(vec![iso])),
            ));
            cols.push((
                Field::new("city_id", DataType::UInt16, false),
                Arc::new(UInt16Array::from(vec![city])),
            ));
            cols.push((
                Field::new("continent", DataType::UInt8, false),
                Arc::new(UInt8Array::from(vec![cont])),
            ));
        }
        let fields: Vec<Field> = cols.iter().map(|(f, _)| f.clone()).collect();
        let arrs: Vec<ArrayRef> = cols.into_iter().map(|(_, a)| a).collect();
        RecordBatch::try_new(Arc::new(Schema::new(fields)), arrs).unwrap()
    }

    const TH_TRIPLET: (u16, u16, u8) = (u16::from_le_bytes(*b"TH"), 0, Continent::Asia as u8);

    /// Gate (a): a row baked TH at an UNKNOWN region admin gets TH defaults —
    /// the segment's own country wins over the region.
    #[test]
    fn baked_country_wins_over_region_admin() {
        let mut baked = Vec::new();
        absorb(
            &unenriched_batch(Some(TH_TRIPLET)),
            Admin::UNKNOWN,
            &mut baked,
        );
        let mut region_th = Vec::new();
        absorb(&unenriched_batch(None), TH, &mut region_th);
        let mut region_world = Vec::new();
        absorb(&unenriched_batch(None), Admin::UNKNOWN, &mut region_world);
        assert_eq!(baked.len(), 1);
        assert_eq!(
            baked[0].emission_lin, region_th[0].emission_lin,
            "baked TH ≡ region TH (the TH arm drove the row)"
        );
        assert_ne!(
            baked[0].emission_lin, region_world[0].emission_lin,
            "TH arm ≠ WORLD arm — the region's UNKNOWN did NOT win"
        );
    }

    /// Gate (b): with the triplet ABSENT the region admin still drives —
    /// byte-identical to the pre-bake loader (the region cache path).
    #[test]
    fn absent_triplet_keeps_region_path() {
        let mut fallback = Vec::new();
        absorb(&unenriched_batch(None), TH, &mut fallback);
        let mut baked = Vec::new();
        absorb(
            &unenriched_batch(Some(TH_TRIPLET)),
            Admin::UNKNOWN,
            &mut baked,
        );
        assert_eq!(
            fallback[0].emission_lin, baked[0].emission_lin,
            "region-admin fallback ≡ baked admin for the same country"
        );
    }

    /// Gate (c): a PRESENT 0 (`\0\0`) bakes `Admin::UNKNOWN` → WORLD defaults
    /// — the row must NOT inherit the region's TH arm.
    #[test]
    fn present_zero_bakes_world_not_region() {
        let mut baked0 = Vec::new();
        absorb(&unenriched_batch(Some((0, 0, 0))), TH, &mut baked0);
        let mut region_world = Vec::new();
        absorb(&unenriched_batch(None), Admin::UNKNOWN, &mut region_world);
        let mut region_th = Vec::new();
        absorb(&unenriched_batch(None), TH, &mut region_th);
        assert_eq!(
            baked0[0].emission_lin, region_world[0].emission_lin,
            "present 0 ≡ WORLD arm"
        );
        assert_ne!(
            baked0[0].emission_lin, region_th[0].emission_lin,
            "present 0 ≠ region TH — no region fallback for baked rows"
        );
    }

    /// Tolerant reader: a wrong-typed `country_iso` (contract violation — the
    /// bake hard-fails on it) reads as ABSENT → region fallback, never a
    /// hard fail (mirrors the popup's lenient column reads).
    #[test]
    fn wrong_typed_country_iso_reads_as_absent() {
        let cols: Vec<(Field, ArrayRef)> = vec![
            (
                Field::new("start_lat", DataType::Float64, false),
                Arc::new(Float64Array::from(vec![50.10])),
            ),
            (
                Field::new("start_lon", DataType::Float64, false),
                Arc::new(Float64Array::from(vec![14.30])),
            ),
            (
                Field::new("end_lat", DataType::Float64, false),
                Arc::new(Float64Array::from(vec![50.11])),
            ),
            (
                Field::new("end_lon", DataType::Float64, false),
                Arc::new(Float64Array::from(vec![14.31])),
            ),
            (
                Field::new("road_class", DataType::UInt8, false),
                Arc::new(UInt8Array::from(vec![3u8])),
            ),
            (
                Field::new("speed_limit", DataType::UInt8, false),
                Arc::new(UInt8Array::from(vec![50u8])),
            ),
            (
                Field::new("country_iso", DataType::UInt8, false), // wrong width
                Arc::new(UInt8Array::from(vec![1u8])),
            ),
        ];
        let fields: Vec<Field> = cols.iter().map(|(f, _)| f.clone()).collect();
        let arrs: Vec<ArrayRef> = cols.into_iter().map(|(_, a)| a).collect();
        let batch = RecordBatch::try_new(Arc::new(Schema::new(fields)), arrs).unwrap();
        let mut wrong_typed = Vec::new();
        absorb(&batch, TH, &mut wrong_typed);
        let mut region_th = Vec::new();
        absorb(&unenriched_batch(None), TH, &mut region_th);
        assert_eq!(
            wrong_typed[0].emission_lin, region_th[0].emission_lin,
            "wrong-typed column ≡ absent → region fallback"
        );
    }

    /// End-to-end through the real file path: a baked `roads.arrow` in a temp
    /// h3r4 tree loads with the ROW's admin even when the region is UNKNOWN.
    #[test]
    fn load_for_r4s_reads_baked_columns_from_disk() {
        use arrow::ipc::writer::FileWriter;
        let hex: u64 = 0x0841_e309_ffff_ffff;
        let write_hex = |root: &std::path::Path, batch: &RecordBatch| {
            let hex_dir = root.join(format!("{hex:015x}"));
            std::fs::create_dir_all(&hex_dir).unwrap();
            let f = std::fs::File::create(hex_dir.join("roads.arrow")).unwrap();
            let mut w = FileWriter::try_new(f, &batch.schema()).unwrap();
            w.write(batch).unwrap();
            w.finish().unwrap();
        };
        let baked_root = tempfile::tempdir().unwrap();
        write_hex(baked_root.path(), &unenriched_batch(Some(TH_TRIPLET)));
        let plain_root = tempfile::tempdir().unwrap();
        write_hex(plain_root.path(), &unenriched_batch(None));

        let baked = RoadData::load_for_r4s(baked_root.path(), &[hex], Admin::UNKNOWN).unwrap();
        let plain = RoadData::load_for_r4s(plain_root.path(), &[hex], TH).unwrap();
        let baked = baked.into_rows();
        let plain = plain.into_rows();
        assert_eq!(baked.len(), 1);
        assert_eq!(plain.len(), 1);
        assert_eq!(
            baked[0].emission_lin, plain[0].emission_lin,
            "on-disk baked TH ≡ region TH through the real read path"
        );
    }

    /// Perf sanity (M4 gate): time `load_for_r4s` on a CZ/PL border ring —
    /// the unbaked fallback path (today's live data) vs a synthetically
    /// baked copy exercising the per-row path with two distinct tuples.
    /// Run: `cargo test --release perf_border_r4_load -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn perf_border_r4_load() {
        let dir = std::path::Path::new("../../data/prepared/2026/h3r4");
        if !dir.exists() {
            eprintln!("skipping: {} not found", dir.display());
            return;
        }
        noise_compute::admin::set_admin_h3r4_directory(dir);
        // Hlučínsko — the CZ wedge into PL (plan anchor for border cells).
        let (lat, lon) = (50.02, 18.08);
        let cell = h3o::LatLng::new(lat, lon)
            .unwrap()
            .to_cell(h3o::Resolution::Four);
        let ring: Vec<u64> = cell
            .grid_disk::<Vec<_>>(1)
            .into_iter()
            .map(u64::from)
            .collect();
        let admin = noise_compute::admin::admin_for_latlng(lat, lon);

        let time_load = |d: &std::path::Path, iters: u32| -> (usize, f64) {
            let warm = RoadData::load_for_r4s(d, &ring, admin).unwrap();
            let rows = warm.into_rows().len();
            let t = std::time::Instant::now();
            for _ in 0..iters {
                std::hint::black_box(RoadData::load_for_r4s(d, &ring, admin).unwrap());
            }
            (rows, t.elapsed().as_secs_f64())
        };

        const ITERS: u32 = 10;
        let (rows, dt_plain) = time_load(dir, ITERS);
        eprintln!(
            "unbaked : {rows} rows × {ITERS} iters in {dt_plain:.3}s → {:.0} rows/s",
            rows as f64 * ITERS as f64 / dt_plain
        );

        // Synthetically baked copy of the same ring: alternating CZ/PL
        // triplets per row (two distinct tuples — the border case).
        let baked_root = tempfile::tempdir().unwrap();
        bake_ring_copy(dir, baked_root.path(), &ring);
        let (baked_rows, dt_baked) = time_load(baked_root.path(), ITERS);
        eprintln!(
            "baked   : {baked_rows} rows × {ITERS} iters in {dt_baked:.3}s → {:.0} rows/s",
            baked_rows as f64 * ITERS as f64 / dt_baked
        );
        eprintln!(
            "baked/unbaked time ratio: {:.3}",
            dt_baked / dt_plain.max(1e-9)
        );
    }

    /// Copy the ring's `roads.arrow` files into `dst`, appending the M3
    /// triplet (CZ on even rows, PL on odd — border-realistic re-keying).
    fn bake_ring_copy(src: &std::path::Path, dst: &std::path::Path, ring: &[u64]) {
        use arrow::ipc::reader::FileReader;
        use arrow::ipc::writer::FileWriter;
        for &r4 in ring {
            let path = src.join(format!("{r4:015x}")).join("roads.arrow");
            if !path.exists() {
                continue;
            }
            let reader = FileReader::try_new(std::fs::File::open(&path).unwrap(), None).unwrap();
            let hex_dir = dst.join(format!("{r4:015x}"));
            std::fs::create_dir_all(&hex_dir).unwrap();
            let mut writer: Option<FileWriter<std::fs::File>> = None;
            for batch in reader {
                let batch = batch.unwrap();
                let n = batch.num_rows();
                let iso = UInt16Array::from_iter_values((0..n).map(|i| {
                    if i % 2 == 0 {
                        u16::from_le_bytes(*b"CZ")
                    } else {
                        u16::from_le_bytes(*b"PL")
                    }
                }));
                let city = UInt16Array::from(vec![0u16; n]);
                let cont = UInt8Array::from(vec![Continent::Europe as u8; n]);
                let mut fields: Vec<Field> = batch
                    .schema()
                    .fields()
                    .iter()
                    .map(|f| f.as_ref().clone())
                    .collect();
                let mut cols = batch.columns().to_vec();
                fields.push(Field::new("country_iso", DataType::UInt16, false));
                cols.push(Arc::new(iso));
                fields.push(Field::new("city_id", DataType::UInt16, false));
                cols.push(Arc::new(city));
                fields.push(Field::new("continent", DataType::UInt8, false));
                cols.push(Arc::new(cont));
                let schema = Arc::new(Schema::new(fields));
                let out = RecordBatch::try_new(schema.clone(), cols).unwrap();
                if writer.is_none() {
                    let f = std::fs::File::create(hex_dir.join("roads.arrow")).unwrap();
                    writer = Some(FileWriter::try_new(f, &schema).unwrap());
                }
                writer.as_mut().unwrap().write(&out).unwrap();
            }
            if let Some(mut w) = writer {
                w.finish().unwrap();
            }
        }
    }
}
