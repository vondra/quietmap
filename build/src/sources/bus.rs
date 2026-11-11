//! Buses on uncounted roads: mapped route directions and tagged intervals, with the admitted
//! timetable fit applied to untagged service. A counted road keeps its count, buses included.

use crate::source_tables::BusModel;
use std::path::Path;

pub struct BusRoutes {
    ways: Vec<i64>,
    /// Line directions of the bus, trolleybus and coach lines without an interval, and the
    /// departures a day of the bus and trolleybus routes and of the coach routes with one.
    service: Vec<([u16; 3], [f32; 2])>,
}

impl BusRoutes {
    /// Reads `bus-ways.txt`: `way bus trolleybus coach bus_departures coach_departures` per line,
    /// sorted by way.
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
            let [way, bus, trolleybus, coach, departures, coach_departures] = fields[..] else {
                return Err(format!("line {}: expected six fields", number + 1));
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
                [
                    departures.parse().map_err(|e| bad(&e))?,
                    coach_departures.parse().map_err(|e| bad(&e))?,
                ],
            ));
        }
        Ok(BusRoutes { ways, service })
    }

    /// Whether an OSM way carries a bus, trolleybus or coach line (every way listed carries one).
    pub fn serves(&self, way: i64) -> bool {
        self.lines(way).is_some()
    }

    /// An OSM way's bus, trolleybus and coach line directions without an interval and the
    /// departures a day of its bus and trolleybus and of its coach routes with one; `None` where
    /// no line runs.
    pub fn lines(&self, way: i64) -> Option<([u16; 3], [f32; 2])> {
        let index = self.ways.binary_search(&way).ok()?;
        Some(self.service[index])
    }

    /// Buses and coaches a day on an OSM way, its lines without an interval running `service`
    /// of the buses where 100,000 trips a day begin or end within 2 km.
    pub fn daily(&self, model: &BusModel, way: i64, service: f64) -> (f64, f64) {
        let Some(([bus, trolleybus, coach], [departures, coach_departures])) = self.lines(way)
        else {
            return (0.0, 0.0);
        };
        let line_directions = f64::from(bus) + model.trolleybus_line_weight * f64::from(trolleybus);
        let buses = model.one_line_direction
            * line_directions.powf(model.line_directions_exponent)
            * service
            + f64::from(departures);
        (
            buses,
            f64::from(coach) * model.coach_departures + f64::from(coach_departures),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_street_carries_its_lines_buses() {
        let model = &crate::source_tables::SourceTables::test_fixture().bus_model;
        let routes = BusRoutes::parse(
            "7 6 2 0 0.0 0.0\n9 0 0 1 108.0 0.0\n10 0 0 0 0.0 18.0\n11 2 0 0 0.0 0.0\n12 400 0 0 0.0 0.0\n",
        )
        .unwrap();
        let near = |(buses, coaches): (f64, f64), (expected, expected_coaches): (f64, f64)| {
            (buses - expected).abs() < 0.1 && coaches == expected_coaches
        };
        // A line both ways where the service share is 1: 47.8 buses; a quarter of them at a quarter.
        assert!(near(routes.daily(model, 11, 1.0), (47.8, 0.0)));
        assert!(near(routes.daily(model, 11, 0.25), (11.95, 0.0)));
        // Three bus lines both ways and a trolleybus line: 6 + 2 x 2.75 = 11.5 line directions.
        assert!(near(routes.daily(model, 7, 1.0), (153.5, 0.0)));
        // A route every ten minutes runs its 108; a coach line 4 a day each way.
        assert_eq!(routes.daily(model, 9, 1.0), (108.0, 4.0));
        assert_eq!(routes.daily(model, 8, 1.0), (0.0, 0.0));
        assert!(routes.serves(10) && !routes.serves(8));
        // A coach route every hour stays coaches, not buses.
        assert_eq!(routes.daily(model, 10, 1.0), (0.0, 18.0));
        // 400 line directions, as São Paulo's corridors list: 1,637 a day where the share is 1.
        assert!(near(routes.daily(model, 12, 1.0), (1_637.3, 0.0)));
        assert!(BusRoutes::parse("9 1 0 0 0 0\n7 1 0 0 0 0\n").is_err());
    }
}
