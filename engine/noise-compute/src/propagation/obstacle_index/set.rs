//! Query obstacle indexes together under unique footprint ownership.

use super::{
    CellGate, CellPrune, CrossingCandidate, CrossingScratch, ObstacleIndex, SeenEdges, SkylineArc,
};

/// A query-scoped set of per-cell [`ObstacleIndex`]es (Arc-shared from a
/// process cache). The ingest's half-open centroid ownership guarantees a
/// footprint lives in exactly ONE cell index, so per-index results concat
/// without cross-index dedupe; one final sort restores chainage order.
pub struct ObstacleSet {
    pub indexes: Vec<std::sync::Arc<ObstacleIndex>>,
}

impl ObstacleSet {
    /// Known-empty obstacle coverage; missing data must fail in the loader.
    pub fn empty() -> Self {
        Self {
            indexes: Vec::new(),
        }
    }

    /// Total indexed edges across the set (telemetry / emptiness check).
    /// Every edge of the set re-framed into one index centred at `(origin_lat, origin_lon)`,
    /// footprint ids made unique across the set (each index's ids follow those of the indexes
    /// before it): one grid that a single ray walk crosses in chainage order, which the
    /// painter's streaming CNOSSOS scan needs to pair a footprint's walls into roofs.
    pub fn merged(&self, origin_lat: f64, origin_lon: f64) -> ObstacleIndex {
        let mut builder = ObstacleIndex::builder(origin_lat, origin_lon);
        let mut id_base = 0_u32;
        for index in &self.indexes {
            let Some(max_id) = index.edges.iter().map(|edge| edge.id).max() else {
                continue;
            };
            let to_merged = |x: f32, y: f32| {
                let lat = index.origin_lat + f64::from(y) / grid::geo::M_PER_DEG_LAT;
                let lon = index.origin_lon + f64::from(x) / index.m_per_deg_lon;
                let (x, y) = builder.local(lat, lon);
                (x as f32, y as f32)
            };
            let mut edges = Vec::with_capacity(index.edges.len());
            for edge in index.edges.iter() {
                let (x0, y0) = to_merged(edge.x0, edge.y0);
                let (x1, y1) = to_merged(edge.x1, edge.y1);
                edges.push(super::ObstacleEdge { x0, y0, x1, y1, id: id_base + edge.id, ..*edge });
            }
            builder.edges.extend(edges);
            let classes = &index.footprint_class;
            builder.footprint_class.resize((id_base + max_id + 1) as usize, crate::envelope::EnvelopeClass::Default as u8);
            builder.footprint_class[id_base as usize..id_base as usize + classes.len()].copy_from_slice(classes);
            id_base += max_id + 1;
        }
        builder.build()
    }

    pub fn edge_count(&self) -> usize {
        self.indexes.iter().map(|i| i.edge_count()).sum()
    }

    /// Tallest building crossed, excluding barriers; used only for popup metadata.
    pub fn max_height_crossed(
        &self,
        src_lat: f64,
        src_lon: f64,
        rcv_lat: f64,
        rcv_lon: f64,
    ) -> f64 {
        let mut scratch = CrossingScratch::default();
        self.walk_ray(
            src_lat,
            src_lon,
            rcv_lat,
            rcv_lon,
            CellGate::TallestBuilding,
            &mut scratch,
            &mut Vec::new(),
        );
        f64::from(scratch.tallest_building_m)
    }

    /// Exact crossings of the ray across every cell index, t-sorted.
    pub fn crossings(
        &self,
        src_lat: f64,
        src_lon: f64,
        rcv_lat: f64,
        rcv_lon: f64,
        out: &mut Vec<CrossingCandidate>,
    ) {
        self.walk_ray(
            src_lat,
            src_lon,
            rcv_lat,
            rcv_lon,
            CellGate::All,
            &mut CrossingScratch::default(),
            out,
        );
    }

    /// The ray across every cell index under `gate`, t-sorted into `out`; a
    /// [`CellGate::TallestBuilding`] bound carries across the indexes.
    pub(super) fn walk_ray(
        &self,
        src_lat: f64,
        src_lon: f64,
        rcv_lat: f64,
        rcv_lon: f64,
        gate: CellGate<'_>,
        scratch: &mut CrossingScratch,
        out: &mut Vec<CrossingCandidate>,
    ) {
        out.clear();
        scratch.tallest_building_m = 0.0;
        for (ordinal, idx) in self.indexes.iter().enumerate() {
            if !idx.segment_may_hit(src_lat, src_lon, rcv_lat, rcv_lon) {
                continue;
            }
            let start = out.len();
            idx.append_crossings(src_lat, src_lon, rcv_lat, rcv_lon, gate, scratch, out);
            let ordinal = u16::try_from(ordinal).expect("an obstacle set holds at most 65,536 indexes");
            out[start..].iter_mut().for_each(|crossing| crossing.index = ordinal);
        }
        out.sort_unstable_by(|a, b| a.t.partial_cmp(&b.t).unwrap());
    }

    /// [`Self::crossings`] with the per-cell branch-and-bound prune — see
    /// [`ObstacleIndex::crossings_pruned`] for what `floor_m` must be.
    pub fn crossings_pruned(
        &self,
        src_lat: f64,
        src_lon: f64,
        rcv_lat: f64,
        rcv_lon: f64,
        prune: &CellPrune<'_>,
        out: &mut Vec<CrossingCandidate>,
    ) {
        self.walk_ray(
            src_lat,
            src_lon,
            rcv_lat,
            rcv_lon,
            CellGate::Delta(prune),
            &mut CrossingScratch::default(),
            out,
        );
    }

    /// Exact building intersections for every ray in one fixed direction
    /// lattice, gathered with one neighbourhood scan per member index.
    pub fn visit_building_sector_crossings<const SECTORS: usize>(
        &self,
        receiver_lat: f64,
        receiver_lon: f64,
        query_m_per_deg_lat: f64,
        query_m_per_deg_lon: f64,
        radius_m: f64,
        directions: &[(f64, f64); SECTORS],
        scratch: &mut CrossingScratch,
        visit: &mut impl FnMut(usize, f64, f32),
    ) {
        for index in &self.indexes {
            index.visit_building_sector_crossings(
                receiver_lat,
                receiver_lon,
                query_m_per_deg_lat,
                query_m_per_deg_lon,
                radius_m,
                directions,
                scratch,
                visit,
            );
        }
    }

    /// [`ObstacleIndex::skyline_arcs_within`] over every member index. Arcs are
    /// origin-relative angles, so per-index results simply concatenate — the
    /// consumer's merge is what turns them into one skyline.
    #[allow(clippy::too_many_arguments)]
    pub fn skyline_arcs_within(
        &self,
        lat: f64,
        lon: f64,
        min_radius_m: f64,
        radius_m: f64,
        los_floor_m: f64,
        delta_min_m: f64,
        wedge: Option<(f64, f64)>,
        mut seen: Option<&mut SeenEdges>,
        visit: &mut impl FnMut(SkylineArc),
    ) {
        let mut edge_ordinal_base = 0_u64;
        for idx in &self.indexes {
            idx.skyline_arcs_within(
                edge_ordinal_base,
                lat,
                lon,
                min_radius_m,
                radius_m,
                los_floor_m,
                delta_min_m,
                wedge,
                seen.as_deref_mut(),
                visit,
            );
            edge_ordinal_base = edge_ordinal_base
                .checked_add(idx.edges.len() as u64)
                .expect("flattened obstacle edge count overflow");
        }
    }
}
