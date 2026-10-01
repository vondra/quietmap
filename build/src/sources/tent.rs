//! The TEN-T rail network carrying freight (the GISCO layer of Regulation 1315/2013's maps, EU27,
//! `fetch/tent.sh`): a heavy rail row whose middle lies within [`MATCH_M`] of a TEN-T freight line
//! and runs within [`MATCH_ANGLE_DEG`] of it takes that line's tier, the highest of the lines it
//! matches. The rail converter weighs a country's guessed freight by the tiers.

use std::collections::HashMap;
use std::path::Path;

/// A row matches a line within this distance (m on the ground) and angle.
const MATCH_M: f64 = 100.0;
const MATCH_ANGLE_DEG: f64 = 30.0;
/// Index cells (Web Mercator m); a segment is listed in the cells its box meets and their
/// neighbours, so a row's middle finds every segment within reach in its own cell.
const CELL_M: f64 = 2_000.0;
/// The Web Mercator sphere's radius (m).
const RADIUS_M: f64 = 6_378_137.0;

/// Where a row runs: off the network, on a TEN-T freight line, on a core network corridor.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Tier {
    Off = 0,
    Network = 1,
    Corridor = 2,
}

/// The network's segments (Web Mercator m) with their tiers, indexed by cell.
#[derive(Default)]
pub struct FreightNetwork {
    segments: Vec<([f64; 4], Tier)>,
    cells: HashMap<(i64, i64), Vec<u32>>,
}

fn cell(value: f64) -> i64 {
    (value / CELL_M).floor() as i64
}

impl FreightNetwork {
    pub fn load(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|error| format!("{}: {error}", path.display()))?;
        Self::parse(&text).map_err(|error| format!("{}: {error}", path.display()))
    }

    /// One line string per line: its tier (1 network, 2 corridor) and its vertices `x y ...`.
    pub fn parse(text: &str) -> Result<Self, String> {
        let mut network = FreightNetwork::default();
        for (number, line) in text.lines().enumerate() {
            let values = line
                .split_whitespace()
                .map(|value| value.parse::<f64>())
                .collect::<Result<Vec<f64>, _>>()
                .map_err(|_| format!("line {}: not numbers", number + 1))?;
            let (tier, vertices) = match values.split_first() {
                Some((&tier, vertices)) if vertices.len() >= 4 && vertices.len() % 2 == 0 => {
                    (tier, vertices)
                }
                _ => return Err(format!("line {}: not a tier and a line", number + 1)),
            };
            let tier = match tier as u8 {
                1 => Tier::Network,
                2 => Tier::Corridor,
                _ => return Err(format!("line {}: tier {tier}", number + 1)),
            };
            for pair in vertices.windows(4).step_by(2) {
                network.add([pair[0], pair[1], pair[2], pair[3]], tier);
            }
        }
        Ok(network)
    }

    fn add(&mut self, [x0, y0, x1, y1]: [f64; 4], tier: Tier) {
        let index = self.segments.len() as u32;
        self.segments.push(([x0, y0, x1, y1], tier));
        for cx in cell(x0.min(x1)) - 1..=cell(x0.max(x1)) + 1 {
            for cy in cell(y0.min(y1)) - 1..=cell(y0.max(y1)) + 1 {
                self.cells.entry((cx, cy)).or_default().push(index);
            }
        }
    }

    /// The tier of a row from `a` to `b` (Web Mercator m).
    pub fn tier(&self, a: [f64; 2], b: [f64; 2]) -> Tier {
        let middle = [0.5 * (a[0] + b[0]), 0.5 * (a[1] + b[1])];
        let latitude = 2.0 * (middle[1] / RADIUS_M).exp().atan() - std::f64::consts::FRAC_PI_2;
        let ground = latitude.cos();
        let direction = (b[1] - a[1]).atan2(b[0] - a[0]);
        let mut best = Tier::Off;
        let Some(listed) = self.cells.get(&(cell(middle[0]), cell(middle[1]))) else {
            return best;
        };
        for &index in listed {
            let ([x0, y0, x1, y1], tier) = self.segments[index as usize];
            if tier <= best {
                continue;
            }
            let (dx, dy) = (x1 - x0, y1 - y0);
            let length_sq = dx * dx + dy * dy;
            let t = if length_sq > 0.0 {
                (((middle[0] - x0) * dx + (middle[1] - y0) * dy) / length_sq).clamp(0.0, 1.0)
            } else {
                0.0
            };
            let distance = (middle[0] - x0 - t * dx).hypot(middle[1] - y0 - t * dy) * ground;
            let pi = std::f64::consts::PI;
            let angle = ((dy.atan2(dx) - direction + pi / 2.0).rem_euclid(pi) - pi / 2.0).abs();
            if distance <= MATCH_M && angle <= MATCH_ANGLE_DEG.to_radians() {
                best = tier;
            }
        }
        best
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A row along a corridor takes it, also running the other way; one 150 m off, one across the
    /// line and one beyond its end do not; a network line and a corridor together read corridor.
    #[test]
    fn rows_take_the_tier_of_the_line_they_run_on() {
        // At 50 N one ground metre is 1/cos(50 deg) = 1.556 Mercator metres.
        let y = RADIUS_M
            * (std::f64::consts::FRAC_PI_4 + 25f64.to_radians())
                .tan()
                .ln();
        let k = 1.0 / 50f64.to_radians().cos();
        let text = format!(
            "2 0 {y} {} {y}\n1 0 {} {} {}\n",
            10_000.0 * k,
            y + 3_000.0 * k,
            10_000.0 * k,
            y + 3_000.0 * k
        );
        let network = FreightNetwork::parse(&text).unwrap();
        let row = |x0: f64, y0: f64, x1: f64, y1: f64| {
            network.tier([x0 * k, y + y0 * k], [x1 * k, y + y1 * k])
        };
        assert_eq!(row(1_000.0, 50.0, 1_200.0, 50.0), Tier::Corridor);
        assert_eq!(row(1_200.0, -80.0, 1_000.0, -80.0), Tier::Corridor);
        assert_eq!(row(1_000.0, 150.0, 1_200.0, 150.0), Tier::Off);
        assert_eq!(row(1_000.0, -50.0, 1_000.0, 50.0), Tier::Off, "across");
        assert_eq!(
            row(10_300.0, 0.0, 10_500.0, 0.0),
            Tier::Off,
            "beyond the end"
        );
        assert_eq!(row(5_000.0, 3_020.0, 5_200.0, 3_020.0), Tier::Network);
        let both = FreightNetwork::parse(&format!("1 0 {y} 100 {y}\n2 0 {y} 100 {y}\n")).unwrap();
        assert_eq!(both.tier([20.0, y], [60.0, y]), Tier::Corridor);
        assert!(FreightNetwork::parse("3 0 0 1 1\n").is_err());
        assert!(FreightNetwork::parse("1 0 0 1\n").is_err());
    }
}
