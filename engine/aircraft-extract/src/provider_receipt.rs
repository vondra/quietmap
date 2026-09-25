//! Provider-day content receipts and the admission of baseline and increment days: a day failing its receipts is missing, never an observed-empty day.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use sha2::{Digest, Sha256};

use crate::filters::point_is_sane;
use crate::flight::FlightSegment;
use crate::provider_merge::MergeCounts;
use crate::trace::{AircraftTrace, CorruptMember};

/// Directory under the work dir holding one `<day>.json` [`DayReceipt`].
pub const RECEIPTS_DIR: &str = "provider-receipts";
/// Admission record beside the sealed shuffle manifests.
pub const ADMISSION_FILE: &str = "admission.json";

/// A provider-day is complete only when every UTC hour shows at least this
/// share of the provider's median aircraft count for that hour over the
/// requested days. W5 hourly receipts 2026-09-24 (`evidence/hourly-receipts.json`,
/// adsb.lol): a broken export falls to 0 (2026-05-07 00–17 UTC) or to
/// 0.067–0.075 of the previous day (2026-08-01 16–23 UTC), while the most
/// distant complete days, 2026-02-01 against 2026-07-31, stay at 0.52–0.90
/// hour by hour; against a whole-year median the seasonal spread is smaller.
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
    pub fn from_traces(
        source_id: u8,
        day: &str,
        traces: &[AircraftTrace],
        corrupt_members: Vec<CorruptMember>,
    ) -> Result<Self> {
        let day_start = day_start_epoch(day)?;
        let mut aircraft_per_utc_hour = vec![0u32; 24];
        let mut hours = HashSet::with_capacity(24);
        for trace in traces {
            hours.clear();
            for point in trace.points.iter().filter(|p| point_is_sane(p)) {
                let hour = ((point.timestamp - day_start) / 3600.0).floor();
                if (0.0..24.0).contains(&hour) {
                    hours.insert(hour as usize);
                }
            }
            for &hour in &hours {
                aircraft_per_utc_hour[hour] += 1;
            }
        }
        Ok(Self {
            source_id,
            traces: traces.len() as u64,
            corrupt_members,
            aircraft_per_utc_hour,
        })
    }

    fn unrecovered_members(&self) -> usize {
        self.corrupt_members.iter().filter(|m| !m.recovered).count()
    }
}

/// Everything Stage 0 learned about one UTC day's inputs; `None` is a
/// provider without an archive that day (missing, never observed-empty).
/// Every requested day carries one, so a day without a receipt is
/// incomplete work, not missing data.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DayReceipt {
    pub day: String,
    pub primary: Option<ProviderDayReceipt>,
    pub secondary: Option<ProviderDayReceipt>,
    pub merge: MergeCounts,
}

impl DayReceipt {
    /// The receipt of a requested day without a primary archive.
    pub fn primary_missing(day: &str) -> Self {
        Self {
            day: day.to_owned(),
            primary: None,
            secondary: None,
            merge: MergeCounts::default(),
        }
    }
}

fn day_start_epoch(day: &str) -> Result<f64> {
    crate::period::parse_date_id(day)?;
    let date = chrono::NaiveDate::parse_from_str(day, "%Y-%m-%d")?;
    Ok(date
        .and_hms_opt(0, 0, 0)
        .context("midnight")?
        .and_utc()
        .timestamp() as f64)
}

pub fn write_day_receipt(work_dir: &Path, receipt: &DayReceipt) -> Result<()> {
    let dir = work_dir.join(RECEIPTS_DIR);
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{}.json", receipt.day));
    let temporary = dir.join(format!(".{}.json.tmp", receipt.day));
    std::fs::write(&temporary, serde_json::to_vec_pretty(receipt)?)?;
    std::fs::rename(&temporary, &path)?;
    Ok(())
}

pub fn read_day_receipt(work_dir: &Path, day: &str) -> Result<Option<DayReceipt>> {
    let path = work_dir.join(RECEIPTS_DIR).join(format!("{day}.json"));
    match std::fs::read(&path) {
        Ok(bytes) => {
            let receipt: DayReceipt = serde_json::from_slice(&bytes)
                .with_context(|| format!("parse {}", path.display()))?;
            anyhow::ensure!(receipt.day == day, "{} names another day", path.display());
            Ok(Some(receipt))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("read {}", path.display())),
    }
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ProviderDayStatus {
    Complete,
    Partial { reasons: Vec<String> },
    Missing,
}

/// Admitted sampling days of one run. Baseline days are complete primary
/// days; increment days are the increment candidates where both providers
/// are complete. The estimator divides primary energy by the baseline count
/// and secondary-only energy by the increment count.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Admission {
    pub baseline_days: BTreeSet<String>,
    pub increment_days: BTreeSet<String>,
    pub primary: BTreeMap<String, ProviderDayStatus>,
    pub secondary: BTreeMap<String, ProviderDayStatus>,
}

/// Classify every requested provider-day from its receipt; every requested
/// day needs one.
pub fn admit(
    requested_days: &BTreeSet<String>,
    increment_candidates: &BTreeSet<String>,
    receipts: &BTreeMap<String, DayReceipt>,
) -> Result<Admission> {
    anyhow::ensure!(
        increment_candidates.is_subset(requested_days),
        "increment candidates must be requested days"
    );
    let unreceipted: Vec<&String> = requested_days
        .iter()
        .filter(|day| !receipts.contains_key(*day))
        .collect();
    anyhow::ensure!(
        unreceipted.is_empty(),
        "no provider receipt for requested day(s) {unreceipted:?}: their Stage 0 work is incomplete"
    );
    let primary_days: BTreeMap<&str, &ProviderDayReceipt> = requested_days
        .iter()
        .filter_map(|day| {
            receipts
                .get(day)
                .and_then(|r| r.primary.as_ref())
                .map(|p| (day.as_str(), p))
        })
        .collect();
    let secondary_days: BTreeMap<&str, &ProviderDayReceipt> = increment_candidates
        .iter()
        .filter_map(|day| {
            receipts
                .get(day)
                .and_then(|r| r.secondary.as_ref())
                .map(|s| (day.as_str(), s))
        })
        .collect();
    let primary = classify(requested_days, &primary_days);
    let secondary = classify(increment_candidates, &secondary_days);
    let complete = |statuses: &BTreeMap<String, ProviderDayStatus>, day: &String| {
        statuses.get(day) == Some(&ProviderDayStatus::Complete)
    };
    let baseline_days: BTreeSet<String> = requested_days
        .iter()
        .filter(|day| complete(&primary, day))
        .cloned()
        .collect();
    let increment_days = increment_candidates
        .iter()
        .filter(|day| complete(&primary, day) && complete(&secondary, day))
        .cloned()
        .collect();
    anyhow::ensure!(
        !baseline_days.is_empty(),
        "no complete primary day among {} requested",
        requested_days.len()
    );
    Ok(Admission {
        baseline_days,
        increment_days,
        primary,
        secondary,
    })
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
            let weak_hours: Vec<String> = (0..24)
                .filter(|&hour| {
                    f64::from(receipt.aircraft_per_utc_hour[hour])
                        < HOURLY_AIRCRAFT_MIN_SHARE_OF_MEDIAN * medians[hour]
                })
                .map(|hour| format!("{hour:02}"))
                .collect();
            if !weak_hours.is_empty() {
                reasons.push(format!(
                    "aircraft below {HOURLY_AIRCRAFT_MIN_SHARE_OF_MEDIAN} of the median in UTC hours {}",
                    weak_hours.join(",")
                ));
            }
            let unrecovered = receipt.unrecovered_members();
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

/// Read every receipt of `days` from the work dir.
pub fn read_receipts(work_dir: &Path, days: &BTreeSet<String>) -> Result<BTreeMap<String, DayReceipt>> {
    let mut receipts = BTreeMap::new();
    for day in days {
        if let Some(receipt) = read_day_receipt(work_dir, day)? {
            receipts.insert(day.clone(), receipt);
        }
    }
    Ok(receipts)
}

pub fn write_admission(directory: &Path, admission: &Admission) -> Result<()> {
    let temporary = directory.join(format!(".{ADMISSION_FILE}.tmp"));
    std::fs::write(&temporary, serde_json::to_vec_pretty(admission)?)?;
    std::fs::rename(&temporary, directory.join(ADMISSION_FILE))?;
    Ok(())
}

/// One admitted day of Stage 1 segments: every primary row counts; a
/// secondary-only row counts only on an increment day (its secondary
/// provider passed the receipts).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AdmittedDay {
    pub segments: PathBuf,
    pub increment: bool,
}

impl AdmittedDay {
    pub fn keeps(&self, segment: &FlightSegment) -> bool {
        self.increment || !segment.is_secondary_only()
    }
}

impl Admission {
    /// The admitted day files under `segments_dir` and the sampling window
    /// their prepared outputs are stamped with.
    pub fn admitted_days(&self, segments_dir: &Path) -> Vec<AdmittedDay> {
        self.baseline_days
            .iter()
            .map(|day| AdmittedDay {
                segments: segments_dir.join(format!("{day}.arrow")),
                increment: self.increment_days.contains(day),
            })
            .collect()
    }

    pub fn sampling_window(&self) -> Result<noise_compute::emission::aircraft::SamplingWindow> {
        Ok(noise_compute::emission::aircraft::SamplingWindow {
            baseline_days: u16::try_from(self.baseline_days.len())?,
            increment_days: u16::try_from(self.increment_days.len())?,
            baseline_days_sha256: day_list_sha256(&self.baseline_days),
            increment_days_sha256: day_list_sha256(&self.increment_days),
        })
    }
}

/// The canonical identity of a day list: SHA-256 of the sorted days joined by `\n`.
pub fn day_list_sha256(days: &BTreeSet<String>) -> String {
    let joined = days.iter().cloned().collect::<Vec<_>>().join("\n");
    format!("{:x}", Sha256::digest(joined.as_bytes()))
}

/// A window of `baseline_days` and `increment_days` with fixed day-list hashes.
#[cfg(test)]
pub(crate) fn window_of(
    baseline_days: u16,
    increment_days: u16,
) -> noise_compute::emission::aircraft::SamplingWindow {
    noise_compute::emission::aircraft::SamplingWindow {
        baseline_days,
        increment_days,
        baseline_days_sha256: "baseline".into(),
        increment_days_sha256: "increment".into(),
    }
}

#[cfg(test)]
#[path = "provider_receipt_tests.rs"]
mod tests;
