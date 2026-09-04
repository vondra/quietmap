//! Cross-source flight deduplication.
//!
//! No-op today (single source) but wired up so a future schedule-synth
//! driver can be plugged in without restructuring Stage 0. The intent
//! is: when the same physical flight arrives via two channels (e.g.
//! adsb.lol observation + OAG schedule), keep the channel with the
//! richer trace and tag the synth row accordingly.

use std::collections::HashMap;

use crate::flight::Flight;

/// Drop duplicate flights keyed by `flight_id`. Within a single source
/// duplicates are unlikely (each ICAO24 + timestamp is unique); cross-
/// source dedup will live here when the second source lands.
pub fn dedup(flights: Vec<Flight>) -> Vec<Flight> {
    let mut seen: HashMap<u64, usize> = HashMap::new();
    let mut out: Vec<Flight> = Vec::with_capacity(flights.len());
    for f in flights {
        match seen.get(&f.flight_id) {
            Some(&i) => {
                // Keep the one with more points — it carries more signal.
                if f.points.len() > out[i].points.len() {
                    out[i] = f;
                }
            }
            None => {
                seen.insert(f.flight_id, out.len());
                out.push(f);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::flight::origin;
    use crate::trace::TracePoint;

    fn pt() -> TracePoint {
        TracePoint {
            timestamp: 0.0,
            lat: 50.0,
            lon: 14.0,
            alt_ft: 1000.0,
            speed_kt: 250.0,
            track_deg: 90.0,
            baro_rate_fpm: 0.0,
            flags: 0,
        }
    }

    fn flight(id: u64, n: usize) -> Flight {
        Flight {
            flight_id: id,
            callsign: String::new(),
            aircraft_type: "B738".into(),
            profile_idx: 0,
            source_id: 0,
            origin: origin::OBSERVED,
            veh_kind: 0,
            gse_class: 0,
            points: vec![pt(); n],
        }
    }

    #[test]
    fn dedup_keeps_richer_trace() {
        let dup = vec![flight(1, 5), flight(1, 20), flight(2, 3)];
        let unique = dedup(dup);
        assert_eq!(unique.len(), 2);
        let f1 = unique.iter().find(|f| f.flight_id == 1).unwrap();
        assert_eq!(f1.points.len(), 20);
    }
}
