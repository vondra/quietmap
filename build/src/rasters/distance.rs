//! Exact Euclidean distance to missing coverage, including the outside border.

fn squared_distance_line(values: &mut [f64]) {
    let input = values.to_vec();
    let mut sites = vec![0usize; values.len()];
    let mut edges = vec![f64::INFINITY; values.len() + 1];
    edges[0] = f64::NEG_INFINITY;
    let mut last = 0;
    for q in 1..input.len() {
        let intersect = |p: usize| {
            ((input[q] + (q * q) as f64) - (input[p] + (p * p) as f64)) / (2 * (q - p)) as f64
        };
        let mut edge = intersect(sites[last]);
        while edge <= edges[last] {
            last -= 1;
            edge = intersect(sites[last]);
        }
        last += 1;
        sites[last] = q;
        edges[last] = edge;
        edges[last + 1] = f64::INFINITY;
    }
    last = 0;
    for (q, value) in values.iter_mut().enumerate() {
        while edges[last + 1] < q as f64 {
            last += 1;
        }
        let distance = q as f64 - sites[last] as f64;
        *value = distance * distance + input[sites[last]];
    }
}

pub fn to_missing(valid: &[bool], rows: usize, columns: usize) -> Vec<f64> {
    distances(valid, rows, columns, true)
}

/// The same with the outside border unknown, not missing: the distance to the shore inside a work
/// window, where the window's edge is no shore. Without a missing node every distance exceeds the
/// window's width and height together.
pub fn to_missing_inside(valid: &[bool], rows: usize, columns: usize) -> Vec<f64> {
    distances(valid, rows, columns, false)
}

fn distances(valid: &[bool], rows: usize, columns: usize, outside_missing: bool) -> Vec<f64> {
    let width = columns + 2;
    let height = rows + 2;
    let infinity = ((width + height) * (width + height)) as f64;
    let mut data = vec![if outside_missing { 0.0 } else { infinity }; width * height];
    for row in 0..rows {
        for column in 0..columns {
            data[(row + 1) * width + column + 1] = if valid[row * columns + column] {
                infinity
            } else {
                0.0
            };
        }
    }
    for line in data.chunks_exact_mut(width) {
        squared_distance_line(line);
    }
    let mut vertical = vec![0.0; height];
    for column in 0..width {
        for row in 0..height {
            vertical[row] = data[row * width + column];
        }
        squared_distance_line(&mut vertical);
        for row in 0..height {
            data[row * width + column] = vertical[row];
        }
    }
    (0..rows)
        .flat_map(|row| {
            data[(row + 1) * width + 1..(row + 1) * width + 1 + columns]
                .iter()
                .map(|v| v.sqrt())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    #[test]
    fn diagonal_distance_is_euclidean_and_outside_counts_as_missing() {
        let mut valid = vec![true; 49];
        valid[2 * 7 + 2] = false;
        let distances = super::to_missing(&valid, 7, 7);
        assert_eq!(distances[3 * 7 + 3], 2.0_f64.sqrt());
        assert_eq!(distances[0], 1.0);
        assert_eq!(distances[2 * 7 + 2], 0.0);
        let inside = super::to_missing_inside(&valid, 7, 7);
        assert_eq!(inside[0], 8.0_f64.sqrt());
        assert!(super::to_missing_inside(&[true; 49], 7, 7)[24] >= 14.0);
    }
}
