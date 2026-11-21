//! Runs of rows that meet end to end where no third row meets them: nothing enters or leaves
//! between them, so a run carries one traffic, its length-weighted median row's. Railways join the
//! rows of one kind where no third row of that kind meets (no switch); roads join counted rows of
//! one direction where no other road of any class meets (no junction).

use std::collections::HashMap;

/// A row as a run sees it: the rows it counts at its ends (`meets`: rows meet where they share
/// this group and an end), whether and as what it joins a run (`joins`: rows of one value join;
/// `None` meets but never joins), its ends (z30), its length (m) and its traffic.
#[derive(Clone, Copy, Debug)]
pub struct RunRow {
    pub meets: u8,
    pub joins: Option<u8>,
    pub ends: [(i32, i32); 2],
    pub length_m: f64,
    pub value: f64,
}

/// For every row the index of its run's length-weighted median row by value (a row that joins no
/// run is its own). Rows join where exactly two rows of their `meets` group share an end and both
/// join as the same value. Runs stop at the square's edge.
pub fn run_medians(rows: &[RunRow]) -> Vec<usize> {
    fn root(parent: &mut [usize], mut index: usize) -> usize {
        while parent[index] != index {
            parent[index] = parent[parent[index]];
            index = parent[index];
        }
        index
    }
    let mut at: HashMap<((i32, i32), u8), Vec<usize>> = HashMap::new();
    for (index, row) in rows.iter().enumerate() {
        for end in row.ends {
            at.entry((end, row.meets)).or_default().push(index);
        }
    }
    let mut parent: Vec<usize> = (0..rows.len()).collect();
    for meeting in at.values() {
        if let [a, b] = meeting[..]
            && a != b
            && rows[a].joins.is_some()
            && rows[a].joins == rows[b].joins
        {
            let (a, b) = (root(&mut parent, a), root(&mut parent, b));
            parent[a] = b;
        }
    }
    let mut runs: HashMap<usize, Vec<usize>> = HashMap::new();
    for index in 0..rows.len() {
        runs.entry(root(&mut parent, index))
            .or_default()
            .push(index);
    }
    let mut median_of = vec![0; rows.len()];
    for members in runs.values_mut() {
        members.sort_by(|&a, &b| rows[a].value.total_cmp(&rows[b].value).then(a.cmp(&b)));
        let half = members
            .iter()
            .map(|&index| rows[index].length_m)
            .sum::<f64>()
            / 2.0;
        let mut length = 0.0;
        let median = members
            .iter()
            .copied()
            .find(|&index| {
                length += rows[index].length_m;
                length >= half
            })
            .unwrap_or(members[0]);
        for &index in members.iter() {
            median_of[index] = median;
        }
    }
    median_of
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(
        meets: u8,
        joins: Option<u8>,
        from: (i32, i32),
        to: (i32, i32),
        length_m: f64,
        value: f64,
    ) -> RunRow {
        RunRow {
            meets,
            joins,
            ends: [from, to],
            length_m,
            value,
        }
    }

    /// Rows meeting end to end without a switch carry their run's trains, its length-weighted
    /// median: a 13 m bridge between two plain pieces of 123 takes 123 (dev4 left the whole line,
    /// 369, on it at Uvaly); a third row meeting at a node (a switch) ends the run; a tram on the
    /// same node is another kind.
    #[test]
    fn a_bridge_carries_its_runs_trains() {
        let rail =
            |kind: u8, from, to, length, trains| row(kind, Some(kind), from, to, length, trains);
        let rows = [
            rail(0, (0, 0), (100, 0), 100.0, 123.0),
            rail(0, (100, 0), (113, 0), 13.0, 369.0),
            rail(0, (113, 0), (213, 0), 100.0, 123.0),
            rail(0, (213, 0), (300, 0), 87.0, 80.0),
            rail(0, (213, 0), (213, 50), 50.0, 10.0),
            rail(1, (0, 0), (100, 0), 100.0, 5.0),
        ];
        let medians = run_medians(&rows);
        assert_eq!(rows[medians[1]].value, 123.0, "the bridge");
        assert_eq!(medians[0], medians[1]);
        assert_eq!((medians[3], medians[4], medians[5]), (3, 4, 5));
    }

    /// Ruta Nacional 40 in La Rioja: consecutive rows of one way where no road joins carry 17,400,
    /// 58,222 and 200 from three sections' counts; the run carries its median, 17,400. A side road
    /// meeting a node ends the run even when it carries no count, and a one-way row does not join
    /// a two-way one.
    #[test]
    fn a_road_where_nothing_joins_carries_one_count() {
        let (counted, oneway, side) = (Some(0), Some(1), None);
        let rows = [
            row(0, counted, (0, 0), (100, 0), 100.0, 17_400.0),
            row(0, counted, (100, 0), (200, 0), 100.0, 58_222.0),
            row(0, counted, (200, 0), (300, 0), 100.0, 17_400.0),
            row(0, counted, (300, 0), (350, 0), 50.0, 200.0),
            row(0, counted, (350, 0), (450, 0), 100.0, 17_400.0),
            row(0, counted, (450, 0), (550, 0), 100.0, 9_000.0),
            row(0, side, (450, 0), (450, 80), 80.0, 0.0),
            row(0, oneway, (550, 0), (650, 0), 100.0, 4_000.0),
        ];
        let medians = run_medians(&rows);
        for index in 0..5 {
            assert_eq!(rows[medians[index]].value, 17_400.0, "row {index}");
        }
        assert_eq!((medians[5], medians[6], medians[7]), (5, 6, 7));
    }
}
