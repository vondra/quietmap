//! Lden period computation (END 2002/49/EC).

use crate::types::NoisePeriods;

/// Compute Lden from separate day/evening/night A-weighted levels.
///
/// Lden = 10 × log₁₀((12×10^(Ld/10) + 4×10^((Le+5)/10) + 8×10^((Ln+10)/10)) / 24)
///
/// Penalty: +5 dB evening, +10 dB night.
pub fn compute_lden(ld: f64, le: f64, ln: f64) -> f64 {
    let c = std::f64::consts::LN_10 * 0.1;
    let energy =
        12.0 * (ld * c).exp() + 4.0 * ((le + 5.0) * c).exp() + 8.0 * ((ln + 10.0) * c).exp();
    10.0 * (energy / 24.0).log10()
}

/// Build NoisePeriods from separate day/evening/night levels.
pub fn periods(ld: f64, le: f64, ln: f64) -> NoisePeriods {
    NoisePeriods {
        ld_db: ld,
        le_db: le,
        ln_db: ln,
        lden_db: compute_lden(ld, le, ln),
    }
}

/// Sum energy from multiple NoisePeriods (e.g., combining sources).
pub fn sum_periods(items: &[NoisePeriods]) -> NoisePeriods {
    if items.is_empty() {
        return NoisePeriods::silence();
    }

    let ld = energy_sum(items.iter().map(|p| p.ld_db));
    let le = energy_sum(items.iter().map(|p| p.le_db));
    let ln = energy_sum(items.iter().map(|p| p.ln_db));

    periods(ld, le, ln)
}

/// Energy sum of dB values: 10×log₁₀(Σ 10^(Li/10))
fn energy_sum(values: impl Iterator<Item = f64>) -> f64 {
    let c = std::f64::consts::LN_10 * 0.1;
    let sum: f64 = values
        .filter(|v| v.is_finite())
        .map(|v| (v * c).exp())
        .sum();
    if sum > 0.0 {
        10.0 * sum.log10()
    } else {
        f64::NEG_INFINITY
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lden_k8() {
        // K8: Ld=60, Le=55, Ln=50 → Lden=60.00
        let lden = compute_lden(60.0, 55.0, 50.0);
        assert!(
            (lden - 60.00).abs() < 0.01,
            "K8: expected 60.00, got {:.2}",
            lden
        );
    }

    #[test]
    fn test_energy_sum() {
        // Two equal sources: +3 dB
        let sum = energy_sum([60.0, 60.0].into_iter());
        assert!((sum - 63.01).abs() < 0.1, "expected ~63, got {:.2}", sum);
    }

    #[test]
    fn test_sum_periods() {
        let p1 = periods(60.0, 55.0, 50.0);
        let p2 = periods(60.0, 55.0, 50.0);
        let total = sum_periods(&[p1, p2]);
        // Each period: two equal sources = +3 dB
        assert!((total.ld_db - 63.01).abs() < 0.1);
    }
}
