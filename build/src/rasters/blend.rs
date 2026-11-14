//! Continuous national-model selection, preserving the repaired producer's spatial conformance.

fn box_mean(values: &mut [f64], rows: usize, columns: usize, size: usize) {
    for vertical in [true, false] {
        let (lines, length) = if vertical {
            (columns, rows)
        } else {
            (rows, columns)
        };
        let mut original = vec![0.0; length];
        for line in 0..lines {
            let index = |offset| {
                if vertical {
                    offset * columns + line
                } else {
                    line * columns + offset
                }
            };
            for (offset, value) in original.iter_mut().enumerate() {
                *value = values[index(offset)];
            }
            let at = |offset: isize| original[offset.clamp(0, length as isize - 1) as usize];
            let before = size as isize / 2;
            let after = size as isize - before;
            let mut sum: f64 = (-before..after).map(at).sum();
            values[index(0)] = sum / size as f64;
            for offset in 1..length {
                sum += at(offset as isize + after - 1) - at(offset as isize - before - 1);
                values[index(offset)] = sum / size as f64;
            }
        }
    }
}

pub fn apply(
    base: &mut [f64],
    national: &[f64],
    distance: &[f64],
    rows: usize,
    columns: usize,
    halo: u32,
) {
    // The repaired terrain producer ramps disagreement across halo/16..3*halo/16
    // metres, then averages that ramp over the halo. Its former hard switch made cliffs.
    let lower = halo as f64 / 16.0;
    let span = halo as f64 / 8.0;
    let mut conformance: Vec<_> = base
        .iter()
        .zip(national)
        .map(|(a, b)| {
            if b.is_finite() {
                ((b - a).abs() - lower).clamp(0.0, span) / span
            } else {
                0.0
            }
        })
        .collect();
    box_mean(&mut conformance, rows, columns, halo as usize);
    for index in 0..base.len() {
        let sampled = national[index];
        if !super::dem::is_ground(sampled) {
            continue;
        }
        let weight = ((distance[index] - 1.0) / halo as f64).clamp(0.0, 1.0);
        let share = conformance[index] * weight;
        let conformed = if share > 0.0 {
            (1.0 - share) * base[index] + share * sampled
        } else {
            base[index]
        };
        base[index] = conformed + weight * (sampled - conformed);
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn even_box_matches_nearest_edges_and_selection_stays_continuous() {
        let mut values = vec![0.0, 0.0, 1.0, 1.0];
        super::box_mean(&mut values, 1, 4, 2);
        assert_eq!(values, [0.0, 0.0, 0.5, 1.0]);
        let values: Vec<_> = [31.99, 32.0, 32.01]
            .into_iter()
            .map(|height| {
                let mut base = [0.0];
                super::apply(&mut base, &[height], &[129.0], 1, 1, 256);
                base[0]
            })
            .collect();
        assert!(values.windows(2).all(|v| v[1] > v[0] && v[1] - v[0] < 0.02));
    }
}
