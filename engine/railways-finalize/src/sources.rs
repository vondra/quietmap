//! Per-category precedence using the generated source registry.

use noise_compute::sources::{get_source, Provenance, Source};

const NATIONALLY_OWNED_KEYS: &[&str] = &["cz-timetable-silent"];
const NONSTANDARD_ISO: &[(&str, [u8; 2])] = &[
    ("city-praha-tsk", *b"CZ"),
    ("city-wien-dauerzaehlstellen", *b"AT"),
    ("city-brno-detectors", *b"CZ"),
    ("global-uswtdb", *b"US"),
];

pub fn should_overwrite(existing_id: u16, new_id: u16) -> bool {
    if existing_id == 0 || existing_id == new_id {
        return true;
    }
    let Some(existing) = get_source(existing_id) else {
        return true;
    };
    let Some(new) = get_source(new_id) else {
        return true;
    };
    let rank_a = existing.provenance.rank();
    let rank_b = new.provenance.rank();
    if rank_b != rank_a {
        return rank_b > rank_a;
    }
    match (existing.year, new.year) {
        (Some(year_a), Some(year_b)) if year_a != year_b => year_b > year_a,
        _ => new.id > existing.id,
    }
}

/// A baseline-tier railway claim (the CZ timetable-silent residual) says a timetable runs no train
/// on a track; it stands only on a line whose tracks carry no ranked evidence in either category.
pub fn is_residual(source_id: u16) -> bool {
    get_source(source_id).is_some_and(|source| matches!(source.provenance, Provenance::Baseline))
}

/// Proxy, heuristic and residual railway claims repeat one whole-line value on every track they
/// stamp; measured sources observe the track itself (routed trips, platform stop counts).
pub fn stamps_whole_line(source_id: u16) -> bool {
    get_source(source_id)
        .is_some_and(|source| source.provenance.rank() <= Provenance::NationalProxy.rank())
}

pub fn nationally_owned(source: &Source) -> bool {
    matches!(
        source.provenance,
        Provenance::CityMeasured | Provenance::NationalMeasured | Provenance::NationalProxy
    ) || NATIONALLY_OWNED_KEYS.contains(&source.key)
}

pub fn national_iso(source: &Source) -> Option<[u8; 2]> {
    if !nationally_owned(source) {
        return None;
    }
    if let Some((_, iso)) = NONSTANDARD_ISO
        .iter()
        .copied()
        .find(|(key, _)| *key == source.key)
    {
        return Some(iso);
    }
    let prefix = source.key.as_bytes();
    if prefix.len() >= 3 && prefix[2] == b'-' {
        let iso = [
            prefix[0].to_ascii_uppercase(),
            prefix[1].to_ascii_uppercase(),
        ];
        if iso[0].is_ascii_uppercase() && iso[1].is_ascii_uppercase() {
            return Some(iso);
        }
    }
    None
}

pub fn ownership_isos(iso: [u8; 2]) -> Vec<[u8; 2]> {
    if iso == *b"MA" {
        vec![iso, *b"EH"]
    } else {
        vec![iso]
    }
}

pub fn source_applies_to_row(source_id: u16, row_iso: [u8; 2]) -> bool {
    let Some(source) = get_source(source_id) else {
        return true;
    };
    let Some(iso) = national_iso(source) else {
        return true;
    };
    ownership_isos(iso).contains(&row_iso)
}

/// Different national owners never replace each other, even when rank would allow it.
pub fn blocks_foreign_national(existing_id: u16, candidate_id: u16) -> bool {
    let Some(existing) = get_source(existing_id) else {
        return false;
    };
    let Some(candidate) = get_source(candidate_id) else {
        return false;
    };
    let Some(existing_iso) = national_iso(existing) else {
        return false;
    };
    let Some(candidate_iso) = national_iso(candidate) else {
        return false;
    };
    ownership_key(existing_iso) != ownership_key(candidate_iso)
}

fn ownership_key(iso: [u8; 2]) -> [u8; 2] {
    if iso == *b"EH" {
        *b"MA"
    } else {
        iso
    }
}
