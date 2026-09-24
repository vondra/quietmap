//! Strict prepared road traffic decoding shared by popup and surface loaders.
use arrow::array::{Array, Float64Array, UInt16Array, UInt8Array};
use arrow::record_batch::RecordBatch;
use noise_compute::normalize::{RoadTimeProfile, RoadTimeProfileAttribution, RoadTraffic};

/// Schema-metadata key carrying the sparse profile dictionary: a JSON object
/// `{source, entries:[{station, window, days, status, profile}]}` written by
/// the producer into exactly the squares it enriched. `traffic_profile_id` is
/// a 1-based index into `entries`; 0 (or an absent column) means no observed
/// profile — genuinely unknown, the class default applies.
pub const ROAD_PROFILES_METADATA_KEY: &str = "roads_time_profiles";

pub struct RoadTrafficColumns<'a> {
    counts: [&'a Float64Array; 4],
    cross_section: &'a Float64Array,
    estimated: &'a UInt8Array,
    profile_ids: Option<&'a UInt16Array>,
    entries: Vec<RoadProfileEntry>,
}

/// One decoded dictionary entry: the emission shares plus the display
/// attribution validated alongside them — one parse, one truth per entry.
pub struct RoadProfileEntry {
    pub profile: RoadTimeProfile,
    pub attribution: RoadTimeProfileAttribution,
}

fn required<'a, T: Array + 'static>(batch: &'a RecordBatch, name: &str) -> Result<&'a T, String> {
    let value = batch
        .column_by_name(name)
        .and_then(|value| value.as_any().downcast_ref::<T>())
        .ok_or_else(|| format!("road traffic column {name} missing or wrong Arrow type"))?;
    if value.null_count() != 0 {
        return Err(format!("road traffic column {name} contains nulls"));
    }
    Ok(value)
}

impl<'a> RoadTrafficColumns<'a> {
    pub fn read(batch: &'a RecordBatch) -> Result<Self, String> {
        if batch
            .schema()
            .metadata()
            .get("road_traffic_contract")
            .map(String::as_str)
            != Some("1")
        {
            return Err("roads.arrow requires road_traffic_contract=1".to_owned());
        }
        let profile_ids = batch
            .column_by_name("traffic_profile_id")
            .map(|column| {
                column
                    .as_any()
                    .downcast_ref::<UInt16Array>()
                    .filter(|column| column.null_count() == 0)
                    .ok_or_else(|| "traffic_profile_id present but wrong Arrow type or nullable".to_owned())
            })
            .transpose()?;
        let profiles = decode_profile_dictionary(
            batch.schema().metadata().get(ROAD_PROFILES_METADATA_KEY),
            profile_ids.map(|column| column.len()),
        )?;
        let result = Self {
            counts: [
                required(batch, "aadt_light")?,
                required(batch, "aadt_medium")?,
                required(batch, "aadt_heavy")?,
                required(batch, "aadt_moto")?,
            ],
            estimated: required(batch, "traffic_estimated")?,
            cross_section: required(batch, "cross_section_aadt")?,
            profile_ids,
            entries: profiles,
        };
        for row in 0..batch.num_rows() {
            if result
                .counts
                .iter()
                .chain([&result.cross_section])
                .any(|column| !column.value(row).is_finite() || column.value(row) < 0.0)
                || result.estimated.value(row) > 15
                || result.profile_ids.is_some_and(|ids| ids.value(row) as usize > result.entries.len())
            {
                return Err(format!("invalid prepared road traffic at row {row}"));
            }
        }
        Ok(result)
    }

    /// 1-based dictionary index of the row's entry; 0 and an absent column
    /// both mean "no observed profile". `read` proved id ≤ entry count.
    fn entry_index(&self, row: usize) -> Option<usize> {
        match self.profile_ids?.value(row) {
            0 => None,
            id => Some(id as usize - 1),
        }
    }

    pub fn row(&self, row: usize) -> RoadTraffic {
        RoadTraffic {
            light: self.counts[0].value(row),
            medium: self.counts[1].value(row),
            heavy: self.counts[2].value(row),
            moto: self.counts[3].value(row),
            estimated: self.estimated.value(row),
            cross_section_aadt: self.cross_section.value(row),
            time_profile: self
                .entry_index(row)
                .and_then(|i| self.entries.get(i).map(|e| e.profile)),
        }
    }

    /// Display attribution paired with `row()`'s `time_profile` by
    /// construction (same id, same entry); `None` = no observed profile.
    pub fn attribution(&self, row: usize) -> Option<&RoadTimeProfileAttribution> {
        self.entry_index(row)
            .and_then(|i| self.entries.get(i).map(|e| &e.attribution))
    }
}

/// Decode and validate the dictionary once per batch. A profile column
/// without a dictionary (or a dictionary without any profile column rows)
/// is a producer bug and fails loudly; every entry's shares defer to the
/// one canonical `RoadTimeProfile::validate`.
fn decode_profile_dictionary(
    metadata: Option<&String>,
    column_rows: Option<usize>,
) -> Result<Vec<RoadProfileEntry>, String> {
    let Some(raw) = metadata else {
        if column_rows.is_some() {
            return Err("traffic_profile_id column without roads_time_profiles metadata".to_owned());
        }
        return Ok(Vec::new());
    };
    if column_rows.is_none() {
        return Err("roads_time_profiles metadata without traffic_profile_id column".to_owned());
    }
    let value: serde_json::Value =
        serde_json::from_str(raw).map_err(|e| format!("invalid {ROAD_PROFILES_METADATA_KEY}: {e}"))?;
    let entries = value
        .get("entries")
        .and_then(|v| v.as_array())
        .ok_or_else(|| format!("invalid {ROAD_PROFILES_METADATA_KEY}: entries missing"))?;
    let source = value
        .get("source")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if source.is_empty() {
        return Err(format!("invalid {ROAD_PROFILES_METADATA_KEY}: source missing"));
    }
    let mut decoded = Vec::with_capacity(entries.len());
    for entry in entries {
        let text_field = |name: &str| {
            entry
                .get(name)
                .and_then(|v| v.as_str())
                .filter(|v| !v.is_empty())
                .ok_or_else(|| format!("invalid {ROAD_PROFILES_METADATA_KEY} entry: {name} missing"))
        };
        text_field("station")?;
        let window = text_field("window")?;
        text_field("status")?;
        entry
            .get("days")
            .and_then(|v| v.as_u64())
            .filter(|v| *v > 0)
            .ok_or_else(|| {
                format!("invalid {ROAD_PROFILES_METADATA_KEY} entry: days missing or not a positive integer")
            })?;
        let profile_object = entry
            .get("profile")
            .and_then(|v| v.as_object())
            .ok_or_else(|| format!("invalid {ROAD_PROFILES_METADATA_KEY} entry: profile missing"))?;
        let known = ["light", "medium", "heavy", "moto", "total"];
        if profile_object.keys().any(|key| !known.contains(&key.as_str())) {
            return Err(format!(
                "invalid {ROAD_PROFILES_METADATA_KEY} entry: unknown class key (known: {known:?})"
            ));
        }
        if profile_object.is_empty() {
            return Err(format!("invalid {ROAD_PROFILES_METADATA_KEY} entry: no observed class or total"));
        }
        let shares = |class: &str| -> Result<Option<[f64; 3]>, String> {
            profile_object
                .get(class)
                .map(|value| {
                    let values = value.as_array().ok_or_else(||
                        format!("invalid {ROAD_PROFILES_METADATA_KEY} {class} shares"))?;
                    if values.len() != 3 {
                        return Err(format!("invalid {ROAD_PROFILES_METADATA_KEY} {class} shares"));
                    }
                    Ok(std::array::from_fn(|index| values[index].as_f64().unwrap_or(f64::NAN)))
                })
                .transpose()
        };
        let profile = RoadTimeProfile {
            light: shares("light")?,
            medium: shares("medium")?,
            heavy: shares("heavy")?,
            moto: shares("moto")?,
            total: shares("total")?,
        };
        profile.validate().map_err(|e| format!("{ROAD_PROFILES_METADATA_KEY}: {e}"))?;
        // Total-transfer caveat: a total share only backs classes without
        // their own observation, so the caveat fires iff at least one class
        // falls back to it (all-four-observed makes `total` inert).
        let total_transfer = profile.total.is_some()
            && [profile.light, profile.medium, profile.heavy, profile.moto]
                .into_iter()
                .any(|class| class.is_none());
        decoded.push(RoadProfileEntry {
            profile,
            attribution: RoadTimeProfileAttribution {
                source: source.to_owned(),
                window: window.to_owned(),
                total_transfer,
            },
        });
    }
    Ok(decoded)
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow::datatypes::{DataType, Field, Schema};
    use std::sync::Arc;

    fn valid_batch() -> (Schema, Vec<Arc<dyn Array>>) {
        let names = ["aadt_light", "aadt_medium", "aadt_heavy", "aadt_moto"];
        let mut fields: Vec<_> = names
            .iter()
            .map(|name| Field::new(*name, DataType::Float64, false))
            .collect();
        fields.push(Field::new("traffic_estimated", DataType::UInt8, false));
        fields.push(Field::new("cross_section_aadt", DataType::Float64, false));
        let columns: Vec<Arc<dyn Array>> = vec![
            Arc::new(Float64Array::from(vec![10_000.0])),
            Arc::new(Float64Array::from(vec![0.0])),
            Arc::new(Float64Array::from(vec![500.0])),
            Arc::new(Float64Array::from(vec![12.5])),
            Arc::new(UInt8Array::from(vec![0b0101])),
            Arc::new(Float64Array::from(vec![21_025.0])),
        ];
        (
            Schema::new(fields).with_metadata(std::collections::HashMap::from([(
                "road_traffic_contract".to_owned(),
                "1".to_owned(),
            )])),
            columns,
        )
    }

    #[test]
    fn road_traffic_contract_rejects_invalid_final_arrow() {
        let (schema, columns) = valid_batch();
        let batch = RecordBatch::try_new(Arc::new(schema.clone()), columns.clone()).unwrap();
        let traffic = RoadTrafficColumns::read(&batch).unwrap().row(0);
        assert_eq!(traffic.light, 10_000.0);
        assert_eq!(traffic.heavy, 500.0);
        assert_eq!(traffic.moto, 12.5);
        assert_eq!(traffic.estimated, 0b0101);
        assert_eq!(traffic.cross_section_aadt, 21_025.0);
        assert!(RoadTrafficColumns::read(&batch.project(&[0, 1, 2, 3, 4]).unwrap()).is_err());
        // Missing contract metadata on an otherwise valid schema.
        assert!(RoadTrafficColumns::read(
            &RecordBatch::try_new(
                Arc::new(Schema::new(schema.fields().clone())),
                columns.clone()
            )
            .unwrap()
        )
        .is_err());
        // Dropped column.
        assert!(
            RoadTrafficColumns::read(&batch.project(&[0, 1, 2, 4]).unwrap()).is_err()
        );
        for (index, name) in [(0, "aadt_light"), (5, "cross_section_aadt")] {
        for value in [Some(f64::NAN), Some(f64::INFINITY), Some(-0.25), None] {
            let mut invalid = columns.clone();
            invalid[index] = Arc::new(Float64Array::from(vec![value]));
            let mut fields = schema.fields().to_vec();
            fields[index] = Arc::new(Field::new(name, DataType::Float64, true));
            let invalid_schema = Schema::new(fields).with_metadata(schema.metadata().clone());
            let invalid = RecordBatch::try_new(Arc::new(invalid_schema), invalid).unwrap();
            assert!(RoadTrafficColumns::read(&invalid).is_err());
        }
        }
        // Out-of-domain bitmask.
        let mut invalid = columns.clone();
        invalid[4] = Arc::new(UInt8Array::from(vec![16]));
        let invalid = RecordBatch::try_new(Arc::new(schema.clone()), invalid).unwrap();
        assert!(RoadTrafficColumns::read(&invalid).is_err());
        // Wrong column type (the legacy Int32 traffic column).
        let mut fields = schema.fields().to_vec();
        fields[0] = Arc::new(Field::new("aadt_light", DataType::Int32, false));
        let mut typed = columns.clone();
        typed[0] = Arc::new(arrow::array::Int32Array::from(vec![1000]));
        let invalid = RecordBatch::try_new(
            Arc::new(Schema::new(fields).with_metadata(schema.metadata().clone())),
            typed,
        )
        .unwrap();
        assert!(RoadTrafficColumns::read(&invalid).is_err());
    }

    #[test]
    fn total_share_is_a_transferred_estimate_for_unmeasured_classes() {
        // Producer contract: a TOTAL-only entry (US TMAS shape — no class
        // breakdown in the source). Every class without its own observation
        // consumes the total share; a class-specific observation overrides it.
        let dictionary = r#"{"source":"https://www.fhwa.dot.gov/policyinformation/tables/tmasdata/","entries":[{"station":"6-021560","window":"2025-01..2025-12","days":360,"status":"total hourly volumes; no vehicle classes","profile":{"total":[0.80,0.10,0.10],"heavy":[0.55,0.18,0.27]}}]}"#;
        let mut fields: Vec<_> = ["aadt_light", "aadt_medium", "aadt_heavy", "aadt_moto"]
            .iter()
            .map(|name| Field::new(*name, DataType::Float64, false))
            .collect();
        fields.push(Field::new("traffic_estimated", DataType::UInt8, false));
        fields.push(Field::new("traffic_profile_id", DataType::UInt16, false));
        fields.push(Field::new("cross_section_aadt", DataType::Float64, false));
        let columns: Vec<Arc<dyn Array>> = vec![
            Arc::new(Float64Array::from(vec![20_000.0])),
            Arc::new(Float64Array::from(vec![0.0])),
            Arc::new(Float64Array::from(vec![2_000.0])),
            Arc::new(Float64Array::from(vec![0.0])),
            Arc::new(UInt8Array::from(vec![0])),
            Arc::new(UInt16Array::from(vec![1])),
            Arc::new(Float64Array::from(vec![22_000.0])),
        ];
        let metadata = std::collections::HashMap::from([
            ("road_traffic_contract".to_owned(), "1".to_owned()),
            (ROAD_PROFILES_METADATA_KEY.to_owned(), dictionary.to_owned()),
        ]);
        let batch = RecordBatch::try_new(Arc::new(Schema::new(fields).with_metadata(metadata)), columns).unwrap();
        let columns = RoadTrafficColumns::read(&batch).unwrap();
        let traffic = columns.row(0);
        let profile = traffic.time_profile.expect("total-only entry resolves");
        assert_eq!(profile.total.unwrap(), [0.80, 0.10, 0.10]);
        assert_eq!(profile.class_shares(2).unwrap(), [0.55, 0.18, 0.27], "heavy keeps its own observation");
        assert_eq!(profile.class_shares(0).unwrap(), [0.80, 0.10, 0.10], "light inherits the total estimate");
        assert_eq!(profile.class_shares(1).unwrap(), [0.80, 0.10, 0.10], "medium inherits the total estimate");
        // Attribution survives the same decode: stored source + window, and
        // the transfer caveat marked because light/medium/moto have no
        // class-specific observation backing them.
        let attribution = columns.attribution(0).expect("profiled row carries attribution");
        assert_eq!(attribution.source, "https://www.fhwa.dot.gov/policyinformation/tables/tmasdata/");
        assert_eq!(attribution.window, "2025-01..2025-12");
        assert!(attribution.total_transfer, "unmeasured classes inherit the total share");
        // A wrong-typed or null total share must fail, never degrade to absence.
        for invalid in ["null", "17", "[0.3,0.3]", "\"0.8,0.1,0.1\""] {
            let corrupt = dictionary.replacen("[0.80,0.10,0.10]", invalid, 1);
            assert!(decode_profile_dictionary(Some(&corrupt), Some(1)).is_err(), "corrupt total rejected: {invalid}");
        }
        // A class-only entry (legacy BW shape) is untouched by the total field
        // and carries no transfer caveat — unmeasured classes keep the
        // class-default split, not a transferred share.
        let legacy = dictionary.replace("\"total\":[0.80,0.10,0.10],", "");
        let entries = decode_profile_dictionary(Some(&legacy), Some(1)).unwrap();
        assert_eq!(entries[0].profile.total, None);
        assert_eq!(entries[0].profile.class_shares(0), None, "no total means light stays unknown");
        assert!(!entries[0].attribution.total_transfer, "no total share, no transfer caveat");
    }

    #[test]
    fn observed_profile_splits_classes_and_survives_to_emission() {
        // Producer contract: a dictionary entry as written by the enricher,
        // with truck night far above car night (observed BW motorway shape).
        let dictionary = r#"{"source":"https://mobidata-bw.de/de/dataset/stundenwerte_dauerzaehlstellen","entries":[{"station":"81191040","window":"2025-01..2025-12","days":345,"status":"flags -/u only","profile":{"light":[0.75,0.18,0.07],"heavy":[0.55,0.18,0.27]}}]}"#;
        let mut fields: Vec<_> = ["aadt_light", "aadt_medium", "aadt_heavy", "aadt_moto"]
            .iter()
            .map(|name| Field::new(*name, DataType::Float64, false))
            .collect();
        fields.push(Field::new("traffic_estimated", DataType::UInt8, false));
        fields.push(Field::new("traffic_profile_id", DataType::UInt16, false));
        fields.push(Field::new("cross_section_aadt", DataType::Float64, false));
        // Row 0 carries the profile reference; row 1 is the mixed case: id 0
        // (no observed profile) inside a file that HAS a dictionary.
        let columns: Vec<Arc<dyn Array>> = vec![
            Arc::new(Float64Array::from(vec![20_000.0, 20_000.0])),
            Arc::new(Float64Array::from(vec![0.0, 0.0])),
            Arc::new(Float64Array::from(vec![2_000.0, 2_000.0])),
            Arc::new(Float64Array::from(vec![0.0, 0.0])),
            Arc::new(UInt8Array::from(vec![0, 0])),
            Arc::new(UInt16Array::from(vec![1, 0])),
            Arc::new(Float64Array::from(vec![22_000.0, 22_000.0])),
        ];
        let metadata = std::collections::HashMap::from([
            ("road_traffic_contract".to_owned(), "1".to_owned()),
            (ROAD_PROFILES_METADATA_KEY.to_owned(), dictionary.to_owned()),
        ]);
        let batch = RecordBatch::try_new(
            Arc::new(Schema::new(fields).with_metadata(metadata)),
            columns,
        )
        .unwrap();
        for invalid_shares in ["null", "17", "{}", "[0.2,0.2,0.2]"] {
            let corrupt = dictionary.replace("[0.55,0.18,0.27]", invalid_shares);
            assert!(decode_profile_dictionary(Some(&corrupt), Some(2)).is_err());
        }
        let columns = RoadTrafficColumns::read(&batch).unwrap();
        let traffic = columns.row(0);
        assert!(columns.row(1).time_profile.is_none(), "id 0 stays no-profile without underflow");
        assert!(columns.attribution(1).is_none(), "unprofiled row omits attribution");
        let profile = traffic.time_profile.expect("dictionary row resolves a profile");
        assert_eq!(profile.heavy.unwrap()[2], 0.27);
        assert_eq!(profile.light.unwrap()[2], 0.07);
        assert_eq!(profile.medium, None);

        // Reader → emission: the night share must differ per class (heavy
        // ~4× the light share), while unmeasured classes keep the default.
        let input = |traffic| noise_compute::normalize::RawRoadInput {
            road_class: 0,
            speed_limit: 0,
            speed_taper: 0,
            surface_type: 0,
            traffic,
            tunnel: false,
            junction: 0,
            built_up: 0,
        };
        let road = noise_compute::normalize::normalize_road(
            input(traffic),
            noise_compute::square_country_city::SquareCountryCity::UNKNOWN,
        )
        .unwrap();
        let pcts = road.period_pcts();
        assert_eq!(pcts[2][0], 0.07); // light night share (observed)
        assert_eq!(pcts[2][2], 0.27); // heavy night share (observed)
        assert_eq!(pcts[2][1], 0.15); // medium keeps the motorway default (unmeasured)
        let (_, _, night) = road.period_emissions();
        // The same row without the profile must emit a different night.
        let mut unprofiled = traffic;
        unprofiled.time_profile = None;
        let plain = noise_compute::normalize::normalize_road(
            input(unprofiled),
            noise_compute::square_country_city::SquareCountryCity::UNKNOWN,
        )
        .unwrap();
        let (_, _, default_night) = plain.period_emissions();
        assert_ne!(night.to_vec(), default_night.to_vec());

        // An id past the dictionary is a producer bug.
        let mut override_columns = batch.columns().to_vec();
        override_columns[5] = Arc::new(UInt16Array::from(vec![2, 0]));
        let bad = RecordBatch::try_new(batch.schema().clone(), override_columns).unwrap();
        assert!(RoadTrafficColumns::read(&bad).is_err());

        // A column without a dictionary is a producer bug; absence of both is
        // a legal legacy arrow (no observed profile anywhere).
        let orphan = batch.project(&[0, 1, 2, 3, 4, 6]).unwrap();
        let orphan_schema = Arc::new(
            Schema::new(orphan.schema().fields().to_vec()).with_metadata(
                std::collections::HashMap::from([("road_traffic_contract".to_owned(), "1".to_owned())]),
            ),
        );
        let orphan = RecordBatch::try_new(orphan_schema, orphan.columns().to_vec()).unwrap();
        assert!(RoadTrafficColumns::read(&orphan).unwrap().row(0).time_profile.is_none());

        // Malformed data must never degrade into absence: an unknown class key,
        // non-numeric days, and a wrong-typed/null profile column all fail.
        for corrupt in [
            dictionary.replace("\"heavy\"", "\"truck\""),
            dictionary.replace("\"days\":345", "\"days\":\"many\""),
            dictionary.replace(
                "\"source\":\"https://mobidata-bw.de/de/dataset/stundenwerte_dauerzaehlstellen\"",
                "\"source\":\"\""),
        ] {
            let metadata = std::collections::HashMap::from([
                ("road_traffic_contract".to_owned(), "1".to_owned()),
                (ROAD_PROFILES_METADATA_KEY.to_owned(), corrupt.to_owned()),
            ]);
            let no_column = RecordBatch::try_new(
                Arc::new(Schema::new(
                    ["aadt_light", "aadt_medium", "aadt_heavy", "aadt_moto", "traffic_estimated"]
                        .iter()
                        .map(|name| Field::new(*name, if *name == "traffic_estimated" { DataType::UInt8 } else { DataType::Float64 }, false))
                        .collect::<Vec<_>>(),
                ).with_metadata(metadata)),
                vec![
                    Arc::new(Float64Array::from(vec![1.0])) as Arc<dyn Array>,
                    Arc::new(Float64Array::from(vec![1.0])),
                    Arc::new(Float64Array::from(vec![1.0])),
                    Arc::new(Float64Array::from(vec![1.0])),
                    Arc::new(UInt8Array::from(vec![0])),
                ],
            ).unwrap();
            assert!(RoadTrafficColumns::read(&no_column).is_err(), "corrupt dictionary rejected: {corrupt}");
        }
        let fields: Vec<_> = batch.schema().fields().iter().map(|field| {
            if field.name() == "traffic_profile_id" { Arc::new(Field::new("traffic_profile_id", DataType::Float64, false)) } else { field.clone() }
        }).collect();
        let schema = Arc::new(Schema::new(fields).with_metadata(batch.schema().metadata().clone()));
        let replaced = RecordBatch::try_new(schema, vec![
            Arc::new(Float64Array::from(vec![20_000.0, 20_000.0])) as Arc<dyn Array>,
            Arc::new(Float64Array::from(vec![0.0, 0.0])),
            Arc::new(Float64Array::from(vec![2_000.0, 2_000.0])),
            Arc::new(Float64Array::from(vec![0.0, 0.0])),
            Arc::new(UInt8Array::from(vec![0, 0])),
            Arc::new(Float64Array::from(vec![1.0, 0.0])),
            Arc::new(Float64Array::from(vec![22_000.0, 22_000.0])),
        ]).unwrap();
        assert!(RoadTrafficColumns::read(&replaced).is_err(), "wrong-typed profile column rejected");
    }
}
