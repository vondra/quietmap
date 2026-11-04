//! Operating intervals distinguish observed state from unknown historical operation.

use anyhow::{Result, ensure};
use chrono::NaiveDate;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Operation {
    Unknown,
    Operating,
    Inactive,
}

pub fn date(value: &str) -> Result<NaiveDate> {
    Ok(NaiveDate::parse_from_str(value, "%Y-%m-%d")?)
}

fn dated_bounds(value: &str) -> Result<(NaiveDate, NaiveDate)> {
    if value.len() == 4 && value.bytes().all(|b| b.is_ascii_digit()) {
        let year = value.parse()?;
        return Ok((
            NaiveDate::from_ymd_opt(year, 1, 1)
                .ok_or_else(|| anyhow::anyhow!("invalid opening year"))?,
            NaiveDate::from_ymd_opt(year, 12, 31)
                .ok_or_else(|| anyhow::anyhow!("invalid closing year"))?,
        ));
    }
    let day = date(value)?;
    Ok((day, day))
}

pub fn operation_at(
    start: Option<&str>,
    end: Option<&str>,
    observed: &str,
    state: u8,
    cutoff: &str,
) -> Result<Operation> {
    let cutoff = date(cutoff)?;
    let observed = date(observed)?;
    let start = start.map(dated_bounds).transpose()?;
    let end = end.map(dated_bounds).transpose()?;
    ensure!(state <= 2, "invalid facility operating state");
    ensure!(
        start.zip(end).is_none_or(|(a, b)| a.0 <= b.1),
        "reversed facility interval"
    );
    if start.is_some_and(|d| d.0 >= cutoff) || end.is_some_and(|d| d.1 < cutoff) {
        return Ok(Operation::Inactive);
    }
    // A year-only date cannot establish which side of a day inside that year the event falls on.
    if start.is_some_and(|d| d.0 < cutoff && d.1 >= cutoff)
        || end.is_some_and(|d| d.0 < cutoff && d.1 >= cutoff)
    {
        return Ok(Operation::Unknown);
    }
    if start.is_some_and(|d| d.1 < cutoff) && (end.is_some_and(|d| d.0 >= cutoff) || state == 1) {
        return Ok(Operation::Operating);
    }
    if observed >= cutoff {
        return Ok(Operation::Unknown);
    }
    Ok(match state {
        1 => Operation::Operating,
        2 => Operation::Inactive,
        _ => Operation::Unknown,
    })
}

#[derive(Clone, Copy)]
pub struct OperatingEvidence<'a> {
    facility: &'a super::records::Facility,
    evidence_operation: Operation,
    pub operation: Operation,
    states: u8,
    same_snapshot: bool,
}

impl<'a> OperatingEvidence<'a> {
    pub fn new(facility: &'a super::records::Facility, cutoff: &str) -> Result<Self> {
        let operation = operation_at(
            facility.operating_from.as_deref(),
            facility.operating_until.as_deref(),
            &facility.observed_on,
            facility.operating_state,
            cutoff,
        )?;
        Ok(Self {
            facility,
            evidence_operation: operation,
            operation,
            states: match operation {
                Operation::Unknown => 1,
                Operation::Operating => 2,
                Operation::Inactive => 4,
            },
            same_snapshot: true,
        })
    }
    pub fn merge(&mut self, other: Self) {
        self.same_snapshot &= self.facility.observed_on == other.facility.observed_on;
        self.states |= other.states;
        // Different registers can describe different tenants. Their dates cannot order a site's
        // closure; a working unit in one common snapshot does keep that site operating.
        self.operation = match self.states {
            2 | 3 => Operation::Operating,
            4 => Operation::Inactive,
            6 | 7 if self.same_snapshot => Operation::Operating,
            _ => Operation::Unknown,
        };
        // The selected observation must support the aggregate state, including its interval bounds.
        let current_supports_state = self.evidence_operation == self.operation;
        let other_supports_state = other.evidence_operation == self.operation;
        if other_supports_state && !current_supports_state
            || other_supports_state == current_supports_state
                && (
                    &other.facility.observed_on,
                    &other.facility.evidence_identity,
                ) > (&self.facility.observed_on, &self.facility.evidence_identity)
        {
            self.facility = other.facility;
            self.evidence_operation = other.evidence_operation;
        }
    }
    pub fn stamp_operation(&self, site: &mut super::records::Facility) {
        site.operation_identity = self.facility.evidence_identity.clone();
        site.operating_from = (self.operation != Operation::Unknown)
            .then(|| self.facility.operating_from.clone())
            .flatten();
        site.operating_until = (self.operation != Operation::Unknown)
            .then(|| self.facility.operating_until.clone())
            .flatten();
        site.observed_on = self.facility.observed_on.clone();
        site.operating_state = match self.operation {
            Operation::Unknown => 0,
            Operation::Operating => 1,
            Operation::Inactive => 2,
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_operating_site_cites_the_working_units_identity_and_interval_in_both_orders() {
        use super::super::records::Facility;
        let active = Facility {
            evidence_identity: "working".into(),
            operating_from: Some("1990".into()),
            observed_on: "2001-01-01".into(),
            operating_state: 1,
            ..Default::default()
        };
        for other in [
            Facility {
                evidence_identity: "retired".into(),
                operating_until: Some("2000".into()),
                observed_on: active.observed_on.clone(),
                operating_state: 2,
                ..Default::default()
            },
            Facility {
                evidence_identity: "unreported".into(),
                observed_on: "2001-12-31".into(),
                ..Default::default()
            },
        ] {
            for observations in [[&other, &active], [&active, &other]] {
                let mut evidence = OperatingEvidence::new(observations[0], "2002-01-01").unwrap();
                evidence.merge(OperatingEvidence::new(observations[1], "2002-01-01").unwrap());
                let mut site = Facility::default();
                evidence.stamp_operation(&mut site);
                assert_eq!(site.operating_state, 1);
                assert_eq!(site.operation_identity, "working");
                assert_eq!(site.operating_from.as_deref(), Some("1990"));
                assert_eq!(site.operating_until, None);
                assert_eq!(site.observed_on, active.observed_on);
            }
        }
    }

    #[test]
    fn snapshots_do_not_invent_history_and_intervals_cover_the_exclusive_state() {
        let at = |start, end, state, cutoff| {
            operation_at(start, end, "2006-06-01", state, cutoff).unwrap()
        };
        assert_eq!(at(None, None, 1, "2005-01-01"), Operation::Unknown);
        assert_eq!(at(None, None, 2, "2005-01-01"), Operation::Unknown);
        assert_eq!(
            at(Some("2001-01-01"), Some("2005-01-01"), 2, "2005-01-01"),
            Operation::Operating
        );
        assert_eq!(
            at(Some("2001-01-01"), Some("2004-12-31"), 2, "2005-01-01"),
            Operation::Inactive
        );
        assert_eq!(
            at(Some("2005-01-01"), None, 1, "2005-01-01"),
            Operation::Inactive
        );
        assert!(
            operation_at(
                Some("2005-01-01"),
                Some("2004-01-01"),
                "2006-01-01",
                1,
                "2007-01-01"
            )
            .is_err()
        );
    }
}

#[cfg(test)]
mod precision_tests {
    use super::*;
    #[test]
    fn year_precision_allows_opening_and_retirement_in_one_year_without_inventing_a_day() {
        assert_eq!(
            operation_at(Some("2000"), Some("2000"), "2001-01-01", 2, "2001-01-01").unwrap(),
            Operation::Inactive
        );
        assert_eq!(
            operation_at(Some("1999"), Some("2000"), "2001-01-01", 2, "2000-01-01").unwrap(),
            Operation::Operating
        );
        assert_eq!(
            operation_at(Some("1999"), Some("2000"), "2001-01-01", 2, "2000-07-01").unwrap(),
            Operation::Unknown
        );
        assert_eq!(
            operation_at(Some("2000"), None, "2001-01-01", 1, "2000-07-01").unwrap(),
            Operation::Unknown
        );
    }
}
