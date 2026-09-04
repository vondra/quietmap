//! Reversible bit-packed flight identifier.
//!
//! `flight_id: u64` carries either:
//!
//! ```text
//! Real ADS-B observation (synthetic = 0):
//!   bits [63:40]  ICAO 24-bit hex   (0x000001..=0xFFFFFE — reserves "no addr" + "anonymous")
//!   bits [39:33]  reserved (zero)
//!   bit  [32]     synthetic = 0
//!   bits [31: 0]  Unix timestamp seconds (flight start; covers 1970-2106)
//!
//! Synthetic (no real ADS-B identity — anonymous-transponder traces in
//! the extract path get a deterministic synth id; the popup compute path
//! also hashes cruise R7-bucket aggregates into a synth id since one row
//! folds many flights):
//!   bit  [32]     synthetic = 1
//!   bits [63:33] | [31:0]  opaque seq, low 32 bits in [31:0], high 31 bits in [63:33]
//! ```
//!
//! `pack_synth` interleaves around the synthetic tag bit so it is a true
//! bijection on `seq ∈ [0, 2^63)`. A naive `seq | SYNTHETIC_BIT` would
//! collide `seq=0` with `seq=1<<32` (both produce just `SYNTHETIC_BIT`); the
//! split-and-shift layout below avoids that and round-trips through `unpack`.
//!
//! Why packed instead of a hash:
//! - **Reversible**: popup unpacks `icao_hex` for hexdb.io / globe.adsb.lol
//!   links, and exact `start_unix` for tooltips. The previous FNV-1a hash was
//!   one-way.
//! - **Same u64 footprint**: per-row cost in the popup arrows unchanged.
//! - **Synthetic bit**: marks IDs that don't map 1:1 to a single
//!   observed aircraft (anonymous-transponder traces and cruise R7
//!   aggregates) without a magic numeric range.

/// Position of the ICAO-24 field. Top 24 bits.
pub const ICAO_SHIFT: u32 = 40;
/// 24-bit mask for ICAO field after right-shift.
pub const ICAO_MASK: u64 = 0xFF_FFFF;
/// Position of the synthetic bit. Distinguishes real ADS-B from synthesised.
pub const SYNTHETIC_BIT: u64 = 1 << 32;
/// 32-bit mask for the timestamp field (low 32 bits in real layout).
pub const TS_MASK: u64 = 0xFFFF_FFFF;

/// `0xFFFFFF` is reserved by ICAO Annex 10 for "anonymous / fictitious";
/// pack rejects so anonymous traffic doesn't masquerade as a discoverable
/// aircraft. `0x000000` ("no address") is rejected by the same predicate.
pub const ICAO_RESERVED_ANON: u32 = 0xFFFFFF;

/// Decoded flight identity.
///
/// `Real` carries a hardware-level ICAO 24-bit transponder address and the
/// flight-start Unix timestamp. `Synth` carries an opaque sequence number for
/// IDs that don't map to a single observed aircraft — anonymous-transponder
/// traces (ICAO 0xFFFFFF / 0x000000) and cruise R7-bucket aggregates that
/// fold many flights into one row both go through `pack_synth`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlightIdKind {
    Real { icao24: u32, start_unix: u32 },
    Synth { seq: u64 },
}

/// Pack a real ADS-B identity. Returns `None` for `icao24 == 0`,
/// `icao24 >= 0xFFFFFF` (reserved + 24-bit overflow), or `ts_sec == 0`.
#[inline]
pub fn pack_real(icao24: u32, ts_sec: u32) -> Option<u64> {
    if icao24 == 0 || icao24 >= ICAO_RESERVED_ANON || ts_sec == 0 {
        return None;
    }
    Some(((icao24 as u64) << ICAO_SHIFT) | (ts_sec as u64))
}

/// Pack a synthetic identity. Bijection on `seq ∈ [0, 2^63)` — top bit of
/// `seq` is silently dropped to make room for the tag at position 32. Round-
/// trips losslessly through `unpack`.
#[inline]
pub fn pack_synth(seq: u64) -> u64 {
    let low = seq & 0xFFFF_FFFF;
    let high = (seq >> 32) & 0x7FFF_FFFF;
    SYNTHETIC_BIT | (high << 33) | low
}

/// Decode a packed flight ID into its constituent parts.
#[inline]
pub fn unpack(fid: u64) -> FlightIdKind {
    if (fid & SYNTHETIC_BIT) != 0 {
        let low = fid & 0xFFFF_FFFF;
        let high = (fid >> 33) & 0x7FFF_FFFF;
        FlightIdKind::Synth {
            seq: (high << 32) | low,
        }
    } else {
        let icao24 = ((fid >> ICAO_SHIFT) & ICAO_MASK) as u32;
        let start_unix = (fid & TS_MASK) as u32;
        FlightIdKind::Real { icao24, start_unix }
    }
}

/// Quick test — does this packed ID belong to a synthetic emitter? Cheap
/// alternative to `unpack` when the caller only cares about the bit.
#[inline]
pub fn is_synthetic(fid: u64) -> bool {
    (fid & SYNTHETIC_BIT) != 0
}

/// Format an `icao24` value as 6-character lowercase hex (no allocation
/// when the buffer is reused). Convenience for popup serialisation.
pub fn icao24_to_hex_lower(icao24: u32) -> String {
    format!("{:06x}", icao24 & 0xFF_FFFF)
}

/// Common destructure used at every popup top-flight emit site: real
/// fids resolve to (hex, Some(unix)); synth fids to (empty, None).
pub fn icao_hex_and_start_unix(fid: u64) -> (String, Option<u32>) {
    match unpack(fid) {
        FlightIdKind::Real { icao24, start_unix } => {
            (icao24_to_hex_lower(icao24), Some(start_unix))
        }
        FlightIdKind::Synth { .. } => (String::new(), None),
    }
}

/// Parse 1..=6 hex chars into a u32 (24-bit) value. Returns `None` for
/// empty input, length > 6, or non-hex characters. Hot path — no allocations.
#[inline]
pub fn parse_icao24_hex(s: &str) -> Option<u32> {
    let bytes = s.as_bytes();
    if bytes.is_empty() || bytes.len() > 6 {
        return None;
    }
    let mut acc: u32 = 0;
    for &b in bytes {
        let nibble = match b {
            b'0'..=b'9' => b - b'0',
            b'a'..=b'f' => b - b'a' + 10,
            b'A'..=b'F' => b - b'A' + 10,
            _ => return None,
        };
        acc = (acc << 4) | nibble as u32;
    }
    Some(acc)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pack_real_round_trip() {
        let fid = pack_real(0x4B1805, 1714559123).unwrap();
        match unpack(fid) {
            FlightIdKind::Real { icao24, start_unix } => {
                assert_eq!(icao24, 0x4B1805);
                assert_eq!(start_unix, 1714559123);
            }
            FlightIdKind::Synth { .. } => panic!("real packed ID decoded as synth"),
        }
    }

    /// Locks the bit layout against accidental shift/mask changes — the
    /// concrete u64 must stay stable since it's persisted in the popup arrows.
    #[test]
    fn pack_real_layout_is_stable() {
        // 0x4B1805 << 40 = 0x4B_1805_0000_0000_0000
        // | 0x6630_4953 (ts) = 0x4B_1805_0000_6630_4953
        assert_eq!(
            pack_real(0x4B1805, 0x6630_4953).unwrap(),
            0x4B18_0500_6630_4953
        );
    }

    #[test]
    fn pack_synth_sets_synthetic_bit() {
        for seq in [0, 1, 42, 0x7FFF_FFFF_FFFF_FFFF] {
            let fid = pack_synth(seq);
            assert!(is_synthetic(fid), "synth bit not set for seq={seq:#x}");
        }
    }

    /// Bijection on `seq ∈ [0, 2^63)`. `pack_synth(0)` and
    /// `pack_synth(SYNTHETIC_BIT)` must produce *distinct* IDs — the bug
    /// the previous `seq | SYNTHETIC_BIT` had (both collapsed to
    /// `SYNTHETIC_BIT`) is what this test guards against.
    #[test]
    fn pack_synth_round_trip_for_seq_under_2_to_63() {
        let cases = [
            0u64,
            1,
            SYNTHETIC_BIT,
            SYNTHETIC_BIT - 1,
            SYNTHETIC_BIT + 1,
            0x7FFF_FFFF_FFFF_FFFF,
            0xDEAD_BEEF_CAFE_BABE & 0x7FFF_FFFF_FFFF_FFFF,
        ];
        let mut seen = std::collections::HashSet::new();
        for &seq in &cases {
            let fid = pack_synth(seq);
            assert!(seen.insert(fid), "collision for seq={seq:#x}");
            match unpack(fid) {
                FlightIdKind::Synth { seq: got } => assert_eq!(got, seq, "seq round-trip {seq:#x}"),
                FlightIdKind::Real { .. } => panic!("synth packed ID decoded as real"),
            }
        }
    }

    #[test]
    fn unpack_zero_decodes_as_real_zero() {
        // Asymmetry worth locking: `pack_real(0, 0)` is rejected (returns
        // None), but `unpack(0)` produces `Real { icao24: 0, start_unix: 0 }`.
        // The layout is total — every u64 decodes to *something*. Zero is
        // not a sentinel; callers must check `pack_real`'s `Option` result
        // rather than relying on a magic decoded value.
        assert_eq!(
            unpack(0),
            FlightIdKind::Real {
                icao24: 0,
                start_unix: 0
            }
        );
    }

    #[test]
    fn pack_real_rejects_invalid_inputs() {
        assert!(pack_real(0, 1).is_none(), "zero ICAO");
        assert!(pack_real(0xFFFFFF, 1).is_none(), "anonymous ICAO");
        assert!(pack_real(0x100_0000, 1).is_none(), "24-bit overflow");
        assert!(pack_real(0x4B1805, 0).is_none(), "zero timestamp");
    }

    #[test]
    fn parse_icao24_hex_handles_case_and_short_input() {
        assert_eq!(parse_icao24_hex("4b1805"), Some(0x4B1805));
        assert_eq!(parse_icao24_hex("4B1805"), Some(0x4B1805));
        assert_eq!(parse_icao24_hex("a0"), Some(0xA0));
    }

    #[test]
    fn parse_icao24_hex_rejects_invalid() {
        assert_eq!(parse_icao24_hex(""), None);
        assert_eq!(parse_icao24_hex("XYZ"), None);
        assert_eq!(parse_icao24_hex("1234567"), None); // > 6 chars
    }

    #[test]
    fn parse_then_pack_rejects_anonymous_hex() {
        // Defence in depth: `"ffffff"` parses fine to 0xFFFFFF, but pack_real
        // rejects it. Callers that compose `parse_icao24_hex` with `pack_real`
        // get the right behaviour for free.
        let icao24 = parse_icao24_hex("ffffff").unwrap();
        assert!(pack_real(icao24, 1).is_none());
    }

    #[test]
    fn synth_does_not_alias_real() {
        // Even with `icao24 = 0xFF_FFFE` (highest valid real) and `ts = u32::MAX`
        // (every low bit set, bit 32 still 0), the synth bit can never be set
        // in a real packed ID.
        let real = pack_real(0xFFFFFE, u32::MAX).unwrap();
        assert!(!is_synthetic(real));
        let synth = pack_synth(0xDEAD_BEEF_CAFE_BABE);
        assert!(is_synthetic(synth));
        assert_ne!(real, synth);
    }

    #[test]
    fn icao24_to_hex_lower_pads_to_six() {
        assert_eq!(icao24_to_hex_lower(0x4B1805), "4b1805");
        assert_eq!(icao24_to_hex_lower(0xA0), "0000a0");
        assert_eq!(icao24_to_hex_lower(0), "000000");
    }
}
