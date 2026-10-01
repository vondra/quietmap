//! Buses on the roads nobody counted: the bus, trolleybus and coach routes of OpenStreetMap
//! (`fetch/bus.sh`), per way the directions they serve. A route that tags its interval runs
//! that often for 18 hours; any other runs a typical service per direction: a city bus line 60
//! departures a day (every 15 minutes, less by evening and night), fewer where fewer people
//! live (the caller's `service` share: a village's line 12-20), a trolleybus 80, a coach 4.
//! A street's buses saturate towards a trunk corridor's 3,000 a day (both directions): São
//! Paulo's avenues list 300-400 route directions, variants of the same lines among them. A
//! counted road keeps its count, buses included.

use std::path::Path;

/// Departures a day per direction of a city bus route without an interval.
const BUS_DEPARTURES: f64 = 60.0;
const TROLLEYBUS_DEPARTURES: f64 = 80.0;
const COACH_DEPARTURES: f64 = 4.0;
/// The most buses a street carries a day (both directions), approached smoothly.
const MOST_BUSES: f64 = 3_000.0;

/// The routes per way, sorted by way id.
pub struct BusRoutes {
    ways: Vec<i64>,
    /// Directions served by bus, trolleybus and coach routes without an interval, and the
    /// departures a day of the routes with one.
    service: Vec<([u16; 3], f32)>,
}

impl BusRoutes {
    /// Reads `bus-ways.txt`: `way bus trolleybus coach departures` per line, sorted by way.
    pub fn load(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|error| format!("{}: {error}", path.display()))?;
        Self::parse(&text).map_err(|error| format!("{}: {error}", path.display()))
    }

    fn parse(text: &str) -> Result<Self, String> {
        let mut ways = Vec::new();
        let mut service = Vec::new();
        for (number, line) in text.lines().enumerate() {
            let fields: Vec<&str> = line.split_ascii_whitespace().collect();
            let [way, bus, trolleybus, coach, departures] = fields[..] else {
                return Err(format!("line {}: expected five fields", number + 1));
            };
            let bad = |error: &dyn std::fmt::Display| format!("line {}: {error}", number + 1);
            let way: i64 = way.parse().map_err(|e| bad(&e))?;
            if ways.last().is_some_and(|&last| last >= way) {
                return Err(format!("line {}: ways not sorted", number + 1));
            }
            ways.push(way);
            service.push((
                [
                    bus.parse().map_err(|e| bad(&e))?,
                    trolleybus.parse().map_err(|e| bad(&e))?,
                    coach.parse().map_err(|e| bad(&e))?,
                ],
                departures.parse().map_err(|e| bad(&e))?,
            ));
        }
        Ok(BusRoutes { ways, service })
    }

    /// Buses (medium) and coaches (heavy) a day on an OSM way, the bus routes without an
    /// interval running `service` (0-1) of a city line's departures.
    pub fn daily(&self, way: i64, service: f64) -> (f64, f64) {
        let Ok(index) = self.ways.binary_search(&way) else {
            return (0.0, 0.0);
        };
        let ([bus, trolleybus, coach], departures) = self.service[index];
        let listed = f64::from(bus) * BUS_DEPARTURES * service
            + f64::from(trolleybus) * TROLLEYBUS_DEPARTURES
            + f64::from(departures);
        let buses = MOST_BUSES * (1.0 - (-listed / MOST_BUSES).exp());
        (buses, f64::from(coach) * COACH_DEPARTURES)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_street_carries_its_routes_departures() {
        let routes = BusRoutes::parse("7 6 2 0 0.0\n9 0 0 1 108.0\n").unwrap();
        // Three city lines both ways and a trolleybus line: 6 x 60 + 2 x 80, saturating.
        let saturated = |listed: f64| MOST_BUSES * (1.0 - (-listed / MOST_BUSES).exp());
        assert_eq!(routes.daily(7, 1.0), (saturated(520.0), 0.0));
        assert!((saturated(520.0) - 477.0).abs() < 1.0);
        assert_eq!(routes.daily(7, 0.2), (saturated(232.0), 0.0));
        assert_eq!(routes.daily(9, 1.0), (saturated(108.0), 4.0));
        assert_eq!(routes.daily(8, 1.0), (0.0, 0.0));
        // 400 route directions: a corridor's 3,000, not 24,000.
        assert!(saturated(24_000.0) > 2_990.0 && saturated(24_000.0) <= 3_000.0);
        assert!(BusRoutes::parse("9 1 0 0 0\n7 1 0 0 0\n").is_err());
    }
}
