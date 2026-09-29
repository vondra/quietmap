//! The popup: the ring loop over whole tile files, the selection by upper bounds and the streamed
//! answer.
//!
//! Map: [`release`] (the opened release and ring reads), [`scene`] (the ground of read tiles),
//! [`obstacles`] (walls and buildings of read tiles), [`candidates`] (sources with their bounds),
//! [`evaluate`] (the full physics of one source), [`answer`] (the ring loop), [`json`] (an
//! update as a line of JSON).

pub mod answer;
pub mod candidates;
pub mod evaluate;
pub mod json;
pub mod obstacles;
pub mod release;
pub mod scene;
