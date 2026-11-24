//! Shared freight-eligibility rule for railway preparation, train-scale fitting and source emission.

use crate::dev4::z30_corner_mercator_m;
use arrow_array::{
    Array, BooleanArray, Int32Array, RecordBatch, StringArray, UInt8Array, UInt16Array,
};
use std::collections::HashMap;

/// A line run this fast or faster is built for high speed (the EU's category I, specially built
/// lines "equipped for speeds generally equal to or greater than 250 km/h", Directive 96/48/EC):
/// such lines carry passengers only unless mapped otherwise. dev4 gave its freight prior to every
/// line not mapped passenger-only: the Shinkansen took 10 freight trains a day a track, 6.67 of them
/// at night, on standard gauge no freight wagon of Japan's runs on.
pub(crate) const HIGH_SPEED_LINE_KMH: f64 = 250.0;

/// One railway piece's geometry and line identity as high-speed line speed calculation sees it.
pub(crate) struct HighSpeedChord<'a> {
    pub highspeed: bool,
    pub maxspeed: u16,
    pub name: &'a str,
    pub reference: &'a str,
    pub ends: [(i32, i32); 2],
}

/// Per-row guessed-freight eligibility and high-speed line speed across a square's railway batches.
pub(crate) struct SquareFreightEligibility {
    pub takes_freight: Vec<bool>,
    pub line_speed: Vec<Option<f64>>,
}

/// Read a UTF-8 Arrow cell, treating SQL nulls as empty strings.
pub(crate) fn utf8_value(array: &StringArray, row: usize) -> &str {
    if array.is_null(row) {
        ""
    } else {
        array.value(row)
    }
}

/// Whether a line built for `speed_kmh` and mapped with `traffic_mode` (`railway:traffic_mode`:
/// 0 unknown, 1 passenger, 2 freight, 3 mixed) takes guessed freight.
pub(crate) fn takes_guessed_freight(speed_kmh: f64, traffic_mode: u8) -> bool {
    match traffic_mode {
        1 => false,
        2 | 3 => true,
        _ => speed_kmh < HIGH_SPEED_LINE_KMH,
    }
}

/// The posted speed of each named high-speed line in a square, `(name, ref)` to the km-weighted
/// median of its high-speed pieces that post one, measured on chord lengths in Mercator metres.
pub(crate) fn high_speed_line_speeds_from_chords<'a>(
    rows: impl IntoIterator<Item = HighSpeedChord<'a>>,
) -> HashMap<(String, String), f64> {
    let mut posted: HashMap<(String, String), Vec<(f64, f64)>> = HashMap::new();
    for row in rows {
        if !row.highspeed || row.maxspeed == 0 || (row.name.is_empty() && row.reference.is_empty())
        {
            continue;
        }
        let [a, b] = row.ends.map(|(x, y)| z30_corner_mercator_m(x, y));
        posted
            .entry((row.name.to_owned(), row.reference.to_owned()))
            .or_default()
            .push((f64::from(row.maxspeed), (a[0] - b[0]).hypot(a[1] - b[1])));
    }
    posted
        .into_iter()
        .map(|(line, mut speeds)| {
            speeds.sort_by(|a, b| a.0.total_cmp(&b.0));
            let half = speeds.iter().map(|(_, length)| length).sum::<f64>() / 2.0;
            let mut length = 0.0;
            let median = speeds
                .iter()
                .find(|(_, row_length)| {
                    length += row_length;
                    length >= half
                })
                .map_or(speeds[0].0, |(speed, _)| *speed);
            (line, median)
        })
        .collect()
}

/// Guessed-freight eligibility and high-speed line speed for one railway piece: `built_for` is the
/// higher of its posted speed and its named high-speed line's median speed (never the default).
pub(crate) fn row_takes_guessed_freight(
    highspeed: bool,
    maxspeed: u16,
    traffic_mode: u8,
    name: &str,
    reference: &str,
    line_speeds: &HashMap<(String, String), f64>,
) -> (bool, Option<f64>) {
    let line_speed =
        if highspeed && !line_speeds.is_empty() && (!name.is_empty() || !reference.is_empty()) {
            line_speeds
                .get(&(name.to_owned(), reference.to_owned()))
                .copied()
        } else {
            None
        };
    let built_for = line_speed.unwrap_or(0.0).max(f64::from(maxspeed));
    (takes_guessed_freight(built_for, traffic_mode), line_speed)
}

/// Compute guessed-freight eligibility and high-speed line speeds for every row of a square's
/// Arrow batches, in table order.
pub(crate) fn table_freight_eligibility(
    batches: &[RecordBatch],
) -> Result<SquareFreightEligibility, String> {
    let mut chords = Vec::new();
    for batch in batches {
        let start_gx = downcast::<Int32Array>(batch, "start_gx")?;
        let start_gy = downcast::<Int32Array>(batch, "start_gy")?;
        let end_gx = downcast::<Int32Array>(batch, "end_gx")?;
        let end_gy = downcast::<Int32Array>(batch, "end_gy")?;
        let maxspeed = downcast::<UInt16Array>(batch, "maxspeed")?;
        let highspeed = downcast::<BooleanArray>(batch, "highspeed")?;
        let names = downcast::<StringArray>(batch, "name")?;
        let refs = downcast::<StringArray>(batch, "ref")?;
        for row in 0..batch.num_rows() {
            chords.push(HighSpeedChord {
                highspeed: highspeed.value(row),
                maxspeed: maxspeed.value(row),
                name: utf8_value(names, row),
                reference: utf8_value(refs, row),
                ends: [
                    (start_gx.value(row), start_gy.value(row)),
                    (end_gx.value(row), end_gy.value(row)),
                ],
            });
        }
    }
    let line_speeds = high_speed_line_speeds_from_chords(chords);
    let total_rows: usize = batches.iter().map(RecordBatch::num_rows).sum();
    let mut takes_freight = Vec::with_capacity(total_rows);
    let mut line_speed = Vec::with_capacity(total_rows);
    for batch in batches {
        let maxspeed = downcast::<UInt16Array>(batch, "maxspeed")?;
        let traffic_mode = downcast::<UInt8Array>(batch, "traffic_mode")?;
        let highspeed = downcast::<BooleanArray>(batch, "highspeed")?;
        let names = downcast::<StringArray>(batch, "name")?;
        let refs = downcast::<StringArray>(batch, "ref")?;
        for row in 0..batch.num_rows() {
            let (takes, speed) = row_takes_guessed_freight(
                highspeed.value(row),
                maxspeed.value(row),
                traffic_mode.value(row),
                utf8_value(names, row),
                utf8_value(refs, row),
                &line_speeds,
            );
            takes_freight.push(takes);
            line_speed.push(speed);
        }
    }
    Ok(SquareFreightEligibility {
        takes_freight,
        line_speed,
    })
}

fn downcast<'a, T: Array + 'static>(batch: &'a RecordBatch, name: &str) -> Result<&'a T, String> {
    batch
        .column_by_name(name)
        .and_then(|column| column.as_any().downcast_ref::<T>())
        .ok_or_else(|| format!("railways.arrow: no column {name}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    /// Passenger-only tracks take no guessed freight at any speed; freight and mixed tracks always
    /// do; untagged tracks take guessed freight only below 250 km/h.
    #[test]
    fn takes_guessed_freight_handles_every_traffic_mode_and_speed() {
        assert!(!takes_guessed_freight(160.0, 1));
        assert!(!takes_guessed_freight(0.0, 1));
        assert!(!takes_guessed_freight(250.0, 1));
        assert!(!takes_guessed_freight(250.0, 0));
        assert!(!takes_guessed_freight(285.0, 0));
        assert!(takes_guessed_freight(249.0, 0));
        assert!(takes_guessed_freight(160.0, 0));
        assert!(takes_guessed_freight(280.0, 2));
        assert!(takes_guessed_freight(280.0, 3));
        assert!(takes_guessed_freight(160.0, 2));
        assert!(takes_guessed_freight(160.0, 3));
    }

    /// Preparation, national fit and source emission share the exact same high-speed line speed
    /// and eligibility on nullable `name`/`ref` columns and on curved splits: a curved piece split
    /// into two shorter chords with longer total chord length shifts the median across 250 km/h
    /// identically when preparation measures expanded child chords.
    #[test]
    fn nullable_strings_and_split_curved_chords_agree_across_stages() {
        // One straight 200 km/h piece (length 1100 in z30) vs a 300 km/h curve whose unsplit
        // chord is 1000 (< 1100 -> median 200) while its two split chords sum to 2 * 707 = 1414
        // (> 1100 -> median 300).
        let unsplit = high_speed_line_speeds_from_chords([
            HighSpeedChord {
                highspeed: true,
                maxspeed: 200,
                name: "CurveLine",
                reference: "",
                ends: [(0, 0), (1100, 0)],
            },
            HighSpeedChord {
                highspeed: true,
                maxspeed: 300,
                name: "CurveLine",
                reference: "",
                ends: [(1100, 0), (2100, 0)],
            },
        ]);
        assert_eq!(unsplit[&("CurveLine".into(), String::new())], 200.0);

        let batch = RecordBatch::try_from_iter([
            (
                "start_gx",
                Arc::new(Int32Array::from(vec![0, 1100, 1600, 3000])) as _,
            ),
            (
                "start_gy",
                Arc::new(Int32Array::from(vec![0, 0, 500, 0])) as _,
            ),
            (
                "end_gx",
                Arc::new(Int32Array::from(vec![1100, 1600, 2100, 4000])) as _,
            ),
            (
                "end_gy",
                Arc::new(Int32Array::from(vec![0, 500, 0, 0])) as _,
            ),
            (
                "maxspeed",
                Arc::new(UInt16Array::from(vec![200, 300, 300, 160])) as _,
            ),
            (
                "highspeed",
                Arc::new(BooleanArray::from(vec![true, true, true, false])) as _,
            ),
            (
                "name",
                Arc::new(StringArray::from(vec![
                    Some("CurveLine"),
                    Some("CurveLine"),
                    Some("CurveLine"),
                    None,
                ])) as _,
            ),
            (
                "ref",
                Arc::new(StringArray::from(vec![None::<&str>, None, None, None])) as _,
            ),
            (
                "traffic_mode",
                Arc::new(UInt8Array::from(vec![0, 0, 0, 1])) as _,
            ),
        ])
        .unwrap();
        let eligibility = table_freight_eligibility(&[batch]).unwrap();
        assert_eq!(
            eligibility.line_speed,
            [Some(300.0), Some(300.0), Some(300.0), None]
        );
        assert_eq!(eligibility.takes_freight, [false, false, false, false]);
    }

    /// Ordinary lines, named high-speed lines (including slower station approaches and unposted
    /// pieces), mixed high-speed lines, unnamed high-speed lines and passenger-only lines.
    #[test]
    fn the_fit_and_the_emitter_share_which_rows_take_guessed_freight() {
        let rows = [
            (160, false, "", 0, 900),
            (300, true, "LGV", 0, 900),
            (120, true, "LGV", 0, 100),
            (0, true, "LGV", 0, 900),
            (280, true, "Mixed", 3, 900),
            (0, true, "", 0, 900),
            (160, false, "Suburban", 1, 900),
        ];
        let x: Vec<i32> = (0..rows.len() as i32).map(|i| i * 1000).collect();
        let batch = RecordBatch::try_from_iter([
            ("start_gx", Arc::new(Int32Array::from(x.clone())) as _),
            (
                "start_gy",
                Arc::new(Int32Array::from(vec![0; rows.len()])) as _,
            ),
            (
                "end_gx",
                Arc::new(Int32Array::from(
                    x.iter().zip(rows).map(|(x, r)| x + r.4).collect::<Vec<_>>(),
                )) as _,
            ),
            (
                "end_gy",
                Arc::new(Int32Array::from(vec![0; rows.len()])) as _,
            ),
            (
                "maxspeed",
                Arc::new(UInt16Array::from_iter_values(rows.map(|r| r.0))) as _,
            ),
            (
                "highspeed",
                Arc::new(BooleanArray::from(rows.map(|r| r.1).to_vec())) as _,
            ),
            (
                "name",
                Arc::new(StringArray::from(rows.map(|r| r.2).to_vec())) as _,
            ),
            (
                "ref",
                Arc::new(StringArray::from(vec![""; rows.len()])) as _,
            ),
            (
                "traffic_mode",
                Arc::new(UInt8Array::from_iter_values(rows.map(|r| r.3))) as _,
            ),
        ])
        .unwrap();
        let eligibility = table_freight_eligibility(&[batch]).unwrap();
        assert_eq!(
            eligibility.takes_freight,
            [true, false, false, false, true, true, false]
        );
    }
}
