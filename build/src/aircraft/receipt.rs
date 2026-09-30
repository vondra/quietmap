//! Day receipts and the admission of baseline and increment days (dev4 `provider_receipt.rs`): a
//! provider-day failing its receipt is missing, never an observed-empty day. Baseline days are the
//! complete primary days; increment days the candidates where both providers are complete. The
//! box builder divides primary energy by the baseline count, secondary-only energy by the
//! increment count.

use super::archive::CorruptMember;
use super::filters::point_is_sane;
use super::merge::MergeCounts;
use super::trace::AircraftTrace;
use std::collections::{BTreeMap, BTreeSet, HashSet};

/// A provider-day is complete when every UTC hour has at least this share of the provider's median
/// aircraft count for that hour over the requested days. Hourly receipts (adsb.lol, 2026-09-24): a
/// broken export falls to 0 or 0.07 of the day before, while complete days six months apart stay
/// at 0.52-0.90 hour by hour.
pub const HOURLY_AIRCRAFT_MIN_SHARE_OF_MEDIAN: f64 = 0.5;

/// Content of one provider's archive for one UTC day.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ProviderDayReceipt {
    pub source_id: u8,
    pub traces: u64,
    pub corrupt_members: Vec<CorruptMember>,
    /// Distinct aircraft with at least one sane position in each UTC hour.
    pub aircraft_per_utc_hour: Vec<u32>,
}

impl ProviderDayReceipt {
    pub fn of(
        source_id: u8,
        day_start: i64,
        traces: &[AircraftTrace],
        corrupt_members: Vec<CorruptMember>,
    ) -> Self {
        let mut aircraft_per_utc_hour = vec![0u32; 24];
        let mut hours = HashSet::with_capacity(24);
        for trace in traces {
            hours.clear();
            for point in trace.points.iter().filter(|p| point_is_sane(p)) {
                let hour = ((point.timestamp - day_start as f64) / 3600.0).floor();
                if (0.0..24.0).contains(&hour) {
                    hours.insert(hour as usize);
                }
            }
            for &hour in &hours {
                aircraft_per_utc_hour[hour] += 1;
            }
        }
        ProviderDayReceipt {
            source_id,
            traces: traces.len() as u64,
            corrupt_members,
            aircraft_per_utc_hour,
        }
    }
}

/// Everything Stage 0 learned about one day's inputs. `primary: None` is a day without a complete
/// primary export (missing); `scope` the boxes a scoped run kept (`None`: the world).
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DayReceipt {
    pub day: String,
    pub primary: Option<ProviderDayReceipt>,
    pub secondary: Option<ProviderDayReceipt>,
    pub merge: MergeCounts,
    pub scope: Option<String>,
}

impl DayReceipt {
    pub fn primary_missing(day: &str, scope: Option<String>) -> Self {
        DayReceipt {
            day: day.into(),
            primary: None,
            secondary: None,
            merge: MergeCounts::default(),
            scope,
        }
    }

    /// A merged day whose secondary provider was not admitted must be rewritten from the primary
    /// alone: its interleaved secondary samples split primary pairs and its `~` echoes are gone.
    /// A merge that kept no secondary content already equals the primary-only day.
    pub fn needs_primary_only_rewrite(&self, increment_days: &BTreeSet<String>) -> bool {
        !increment_days.contains(&self.day)
            && self.secondary.is_some()
            && (self.merge.secondary_points_kept > 0 || self.merge.secondary_only_addresses > 0)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ProviderDayStatus {
    Complete,
    Partial { reasons: Vec<String> },
    Missing,
}

/// The admitted sampling days of one run.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Admission {
    pub baseline_days: BTreeSet<String>,
    pub increment_days: BTreeSet<String>,
    pub primary: BTreeMap<String, ProviderDayStatus>,
    pub secondary: BTreeMap<String, ProviderDayStatus>,
}

/// Classify every requested provider-day by its receipt; every requested day needs one.
pub fn admit(
    requested: &BTreeSet<String>,
    increment_candidates: &BTreeSet<String>,
    receipts: &BTreeMap<String, DayReceipt>,
) -> Result<Admission, String> {
    if !increment_candidates.is_subset(requested) {
        return Err("increment candidates must be requested days".into());
    }
    let unreceipted: Vec<&String> = requested
        .iter()
        .filter(|day| !receipts.contains_key(*day))
        .collect();
    if !unreceipted.is_empty() {
        return Err(format!(
            "no receipt for requested day(s) {unreceipted:?}: their extraction is incomplete"
        ));
    }
    let primary = classify(
        requested,
        &provider_receipts(requested, receipts, |r| r.primary.as_ref()),
    );
    let secondary = classify(
        increment_candidates,
        &provider_receipts(increment_candidates, receipts, |r| r.secondary.as_ref()),
    );
    let complete = |statuses: &BTreeMap<String, ProviderDayStatus>, day: &String| {
        statuses.get(day) == Some(&ProviderDayStatus::Complete)
    };
    let baseline_days: BTreeSet<String> = requested
        .iter()
        .filter(|d| complete(&primary, d))
        .cloned()
        .collect();
    let increment_days = increment_candidates
        .iter()
        .filter(|d| complete(&primary, d) && complete(&secondary, d))
        .cloned()
        .collect();
    if baseline_days.is_empty() {
        return Err(format!(
            "no complete primary day among {} requested",
            requested.len()
        ));
    }
    Ok(Admission {
        baseline_days,
        increment_days,
        primary,
        secondary,
    })
}

fn provider_receipts<'a>(
    days: &'a BTreeSet<String>,
    receipts: &'a BTreeMap<String, DayReceipt>,
    pick: fn(&DayReceipt) -> Option<&ProviderDayReceipt>,
) -> BTreeMap<&'a str, &'a ProviderDayReceipt> {
    days.iter()
        .filter_map(|day| pick(&receipts[day]).map(|r| (day.as_str(), r)))
        .collect()
}

fn classify(
    days: &BTreeSet<String>,
    receipts: &BTreeMap<&str, &ProviderDayReceipt>,
) -> BTreeMap<String, ProviderDayStatus> {
    let medians: Vec<f64> = (0..24)
        .map(|hour| {
            let mut counts: Vec<u32> = receipts
                .values()
                .map(|r| r.aircraft_per_utc_hour[hour])
                .collect();
            counts.sort_unstable();
            match counts.len() {
                0 => 0.0,
                n if n % 2 == 1 => f64::from(counts[n / 2]),
                n => (f64::from(counts[n / 2 - 1]) + f64::from(counts[n / 2])) / 2.0,
            }
        })
        .collect();
    days.iter()
        .map(|day| {
            let Some(receipt) = receipts.get(day.as_str()) else {
                return (day.clone(), ProviderDayStatus::Missing);
            };
            let mut reasons = Vec::new();
            let weak: Vec<String> = (0..24)
                .filter(|&h| f64::from(receipt.aircraft_per_utc_hour[h]) < HOURLY_AIRCRAFT_MIN_SHARE_OF_MEDIAN * medians[h])
                .map(|h| format!("{h:02}"))
                .collect();
            if !weak.is_empty() {
                reasons.push(format!(
                    "aircraft below {HOURLY_AIRCRAFT_MIN_SHARE_OF_MEDIAN} of the median in UTC hours {}",
                    weak.join(",")
                ));
            }
            let unrecovered = receipt.corrupt_members.iter().filter(|m| !m.recovered).count();
            if unrecovered > 0 {
                reasons.push(format!("{unrecovered} corrupt trace members without an intact copy"));
            }
            let status = if reasons.is_empty() {
                ProviderDayStatus::Complete
            } else {
                ProviderDayStatus::Partial { reasons }
            };
            (day.clone(), status)
        })
        .collect()
}

#[cfg(test)]
#[path = "receipt_tests.rs"]
mod tests;
