//! OSM extraction library: `classify` tags, `relations`/`junctions` Pass 0, `node_cache` Pass 1,
//! `pass2` features into the `spill`, `transport` provenance text, `microsegment` acoustic pieces,
//! `implicit_speed` rule resolution, `model_nodes` power and transport controls,
//! `poi_join` building functions, `ids` identities and `finalize` Arrow writers per z9 square.

pub mod classify;
pub mod finalize;
pub mod ids;
pub mod implicit_speed;
pub mod junctions;
pub mod microsegment;
pub mod model_nodes;
pub mod node_cache;
pub mod pass2;
pub mod poi_join;
pub mod relations;
pub mod spill;
pub mod transport;
