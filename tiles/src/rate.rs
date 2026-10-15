//! Rates a day (flights over a cell, through a box) as u16 floats: 0 none, else an 11-bit mantissa
//! and a 5-bit exponent, rate = (2048 + m) * 2^(e - 27), from 3.1e-5 to 65,520 a day within
//! 0.025 %. One flight in a year of data is 0.0027 a day; r064's busiest cell sees 477 a day
//! above 50 dB (measured 2026-10-15).

const MANTISSA_BITS: u32 = 11;
const EXPONENT_BIAS: i32 = 27;

/// A rate as its u16 float, rounded to the nearest.
pub(crate) fn rate_code(rate: f64) -> u16 {
    assert!(rate.is_finite() && rate >= 0.0, "a rate of {rate} a day");
    if rate == 0.0 {
        return 0;
    }
    let mut exponent = rate.log2().floor() as i32 - MANTISSA_BITS as i32 + EXPONENT_BIAS;
    let mut mantissa = (rate / 2f64.powi(exponent - EXPONENT_BIAS)).round() as i32;
    if mantissa == 2 << MANTISSA_BITS {
        (exponent, mantissa) = (exponent + 1, 1 << MANTISSA_BITS);
    }
    assert!(
        (1..32).contains(&exponent),
        "a rate of {rate} a day outside the u16 floats"
    );
    (exponent as u16) << MANTISSA_BITS | (mantissa - (1 << MANTISSA_BITS)) as u16
}

pub(crate) fn rate(code: u16) -> f64 {
    if code == 0 {
        return 0.0;
    }
    let (exponent, mantissa) = (
        i32::from(code >> MANTISSA_BITS),
        i32::from(code & ((1 << MANTISSA_BITS) - 1)),
    );
    f64::from((1 << MANTISSA_BITS) + mantissa) * 2f64.powi(exponent - EXPONENT_BIAS)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rates_keep_eleven_bits_from_a_flight_a_century_to_sixty_thousand_a_day() {
        for written in [3.1e-5, 1.0 / 354.0, 0.25, 1.0, 412.5, 2_047.9, 65_000.0] {
            let read = rate(rate_code(written));
            assert!(
                (read - written).abs() <= written * 2.5e-4,
                "{written} reads {read}"
            );
        }
        assert_eq!(rate(rate_code(0.0)), 0.0);
        assert_eq!(rate(rate_code(412.5)), 412.5, "exact in 11 bits");
    }
}
