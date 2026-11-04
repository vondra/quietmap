//! Cell-by-cell drift against a stored reference.

use super::*;

/// A painted map against a reference (the etalon, or the popup's sparse answers) over the z13
/// squares `squares` ("X,Y;X,Y"), cell by cell where the reference assesses one and either side
/// has a level, by the owner's z13 contract.
pub(super) fn compare_maps(
    painted: &std::path::Path,
    reference: &std::path::Path,
    squares: &str,
) -> Result<(), String> {
    let mut scores: Vec<Score> = (0..=LAYERS).map(|_| Score::default()).collect();
    let mut cells = 0usize;
    for pair in squares.split(';') {
        let (x, y) = pair.split_once(',').ok_or("--squares is X,Y;X,Y")?;
        let square = (
            x.parse::<u32>().map_err(|_| "--squares x")?,
            y.parse::<u32>().map_err(|_| "--squares y")?,
        );
        let (mine, theirs) = (
            paint::hm3::read_nden(painted, 13, square)?,
            paint::hm3::read_nden(reference, 13, square)?,
        );
        for (at, (mine, theirs)) in mine.iter().zip(&theirs).enumerate() {
            let (theirs_total, mine_total) = (theirs[LAYERS], mine[LAYERS]);
            if theirs_total == NO_LEVEL || (theirs_total >= SILENCE && mine_total >= SILENCE) {
                continue;
            }
            cells += 1;
            let place = (
                square.0 * 1024 + (at % 1024) as u32,
                square.1 * 1024 + (at / 1024) as u32,
            );
            // A layer the reference does not assess there is not scored.
            for (score, (&m, &t)) in scores.iter_mut().zip(mine.iter().zip(theirs)) {
                if t != NO_LEVEL {
                    score.add(m, t, place);
                }
            }
        }
    }
    eprintln!("compare: {cells} cells");
    for (score, name) in scores.iter().zip(paint::hm3::layer_names()) {
        println!("{}", score.line(name));
    }
    Ok(())
}

/// One layer's score by the owner's z13 contract (dev1's accuracy ladder, in decibels there) in the
/// stored bytes' loudness level (phon: a byte is half a phon, a phon 7.2 % of loudness), banded by
/// the reference: the rungs over the cells it paints (1 sone, 40 phon, and louder), the last joined
/// by paint-state flips over 6 phon; quieter an error of at most 10 phon; presence changes (across
/// 40 phon unless both lie inside (39.5, 40.5)) at most 0.25 % of the painted cells; the signed
/// mean over the painted cells at most 0.5 phon. Silence reads 0 phon.
#[derive(Default)]
struct Score {
    painted: usize,
    over: [usize; 4],
    presence: usize,
    signed: f64,
    quiet_worst: f64,
    /// The worst difference: phon, the global z13 pixel, the map's and the reference's bytes.
    worst: (f64, u32, u32, u8, u8),
}

/// The rungs: a difference over this many phon at most this percentage of the painted cells.
const RUNGS: [(f64, f64); 4] = [(0.5, 20.0), (1.0, 1.0), (3.0, 0.01), (6.0, 0.001)];

impl Score {
    fn add(&mut self, mine: u8, reference: u8, (x, y): (u32, u32)) {
        const PAINTED: u8 = 80;
        let level = |b: u8| if b >= SILENCE { 0 } else { b };
        let (reference, mine) = (level(reference), level(mine));
        let difference = (f64::from(mine) - f64::from(reference)) / 2.0;
        let edge_band = |b: u8| (79..=81).contains(&b);
        let flip = (reference >= PAINTED) != (mine >= PAINTED)
            && !(edge_band(reference) && edge_band(mine));
        self.presence += usize::from(flip);
        if reference >= PAINTED {
            self.painted += 1;
            self.signed += difference;
            for (k, &(limit, _)) in RUNGS[..3].iter().enumerate() {
                self.over[k] += usize::from(difference.abs() > limit);
            }
        } else {
            self.quiet_worst = self.quiet_worst.max(difference.abs());
            if !flip {
                return;
            }
        }
        self.over[3] += usize::from(difference.abs() > 6.0);
        if difference.abs() > self.worst.0 {
            self.worst = (difference.abs(), x, y, mine, reference);
        }
    }

    fn line(&self, name: &str) -> String {
        let share = |count: usize| 100.0 * count as f64 / self.painted.max(1) as f64;
        let bias = self.signed / self.painted.max(1) as f64;
        let mut line = format!("{name:<10} painted {:>5}:", self.painted);
        for (k, &(limit, allowed)) in RUNGS.iter().enumerate() {
            let mark = if share(self.over[k]) > allowed {
                "!"
            } else {
                ""
            };
            line += &format!(
                " >{limit} {:.2} %{mark} ({})",
                share(self.over[k]),
                self.over[k]
            );
        }
        format!(
            "{line}; presence {:.2} %{} ({}); bias {:+.2} phon{}; quiet worst {:.1} phon{}; worst {:.1} phon at {},{} (map {}, reference {})",
            share(self.presence),
            if share(self.presence) > 0.25 { "!" } else { "" },
            self.presence,
            bias,
            if bias.abs() > 0.5 { "!" } else { "" },
            self.quiet_worst,
            if self.quiet_worst > 10.0 { "!" } else { "" },
            self.worst.0,
            self.worst.1,
            self.worst.2,
            f64::from(self.worst.3) / 2.0,
            f64::from(self.worst.4) / 2.0
        )
    }
}
