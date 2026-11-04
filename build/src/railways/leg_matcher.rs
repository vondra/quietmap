//! Reuse exact directed station-leg searches within one immutable network, including failed paths.
use super::{
    network::Network,
    route::{self, Visit},
    search::Search,
};
use std::collections::HashMap;

#[derive(Hash, PartialEq, Eq)]
struct Leg {
    ends: [u64; 4],
    shape: Vec<[u64; 2]>,
}

pub struct LegMatcher<'a> {
    pub network: &'a Network,
    search: Search,
    paths: HashMap<Leg, Result<Vec<Visit>, &'static str>>,
    hits: usize,
}
impl<'a> LegMatcher<'a> {
    pub fn new(network: &'a Network) -> Self {
        Self {
            network,
            search: Search::new(network.nodes.len()),
            paths: HashMap::new(),
            hits: 0,
        }
    }
    pub fn walk(
        &mut self,
        from: [f64; 2],
        to: [f64; 2],
        shape: &[[f64; 2]],
    ) -> Result<Vec<Visit>, &'static str> {
        let key = Leg {
            ends: [from[0], from[1], to[0], to[1]].map(f64::to_bits),
            shape: shape.iter().map(|point| point.map(f64::to_bits)).collect(),
        };
        if let Some(result) = self.paths.get(&key) {
            self.hits += 1;
            return result.clone();
        }
        let result = route::walk(self.network, &mut self.search, from, to, shape);
        self.paths.insert(key, result.clone());
        result
    }
    pub fn report(&self) {
        eprintln!(
            "railways: {} distinct directed legs, {} exact search reuses",
            self.paths.len(),
            self.hits
        );
    }
}
