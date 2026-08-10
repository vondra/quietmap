//! The process-wide admin-table singleton, exercised in its OWN process.
//!
//! `init_admin_table` fills a `OnceLock` that can never be cleared, so this
//! test CANNOT live in the lib test binary: there it would initialise the
//! table for every concurrently-running lib test, and any test computing with
//! an admin-dependent default (the road/rail kernels resolve the receiver
//! admin as a fallback) would flip between the WORLD and the country arm
//! depending on when this init landed — an observed flake on data-carrying
//! boxes (`none_channel_is_receiver_path_bit_identical` lost the race between
//! its two compute calls). An integration binary is a separate process, so
//! the lib tests keep their "table never initialised" assumption and this one
//! keeps the real wiring covered.
//!
//! Data-gated like the other admin fixtures: skips when the prepared
//! `h3r4-admin.bin` is absent (CI is data-free by design).

use noise_compute::admin::{admin_for_hex, admin_for_latlng, init_admin_table, is_initialised};
use std::path::Path;

#[test]
fn singleton_init_then_lookup_by_hex_or_latlng() {
    // Verify the OnceLock wiring path — init once, then the static
    // helpers resolve CZ for Dobříš and Unknown for untouched coords.
    let path = Path::new("../../data/prepared/h3r4-admin.bin");
    if !path.exists() {
        return;
    }
    // Idempotent SET ignores the result — either way the table is present.
    let _ = init_admin_table(path);
    assert!(is_initialised());

    // Dobris via hex id
    let dobris: u64 = 0x0841_e309_ffff_ffff;
    assert_eq!(admin_for_hex(dobris).country_code(), Some("CZ"));

    // Dobris via lat/lng (49.78, 14.17)
    assert_eq!(admin_for_latlng(49.78, 14.17).country_code(), Some("CZ"));

    // Ocean coord (mid-Pacific) — should be Unknown
    let mid_pacific = admin_for_latlng(-20.0, -140.0);
    assert_eq!(mid_pacific.country_code(), None);
}
