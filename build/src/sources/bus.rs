//! Buses on the roads nobody counted: the bus, trolleybus and coach lines of OpenStreetMap
//! (`fetch/bus.sh`), per way the line directions they serve (a line at most two, however many of
//! its variants are mapped). A street's buses grow with its lines less than in proportion, as where
//! many lines meet each runs less often there, and with the trips its surroundings make (the
//! caller's `service`, 1 where 100,000 trips a day begin or end within 2 km): 30.1 a day for one
//! line direction, times the line directions to the power 0.667, a trolleybus line weighing 2.75
//! bus lines. Fitted by least squares on the timetables (GTFS) of 35 cities and rural areas in 14
//! countries whose routes match the streets, each place alike (private evidence 2026-10-26, buses):
//! the 60 departures a route and direction this replaces read 3.4 dB over the timetables in the
//! median place, this law 0.4 dB; in 8 German cities left out of the fit 5.0 and 2.2 dB. A line
//! that tags its interval runs its shortest for 18 hours each direction (`fetch/bus-ways.py`); a
//! coach line 4 a day each direction. The buses run on their timetables' day, evening and night
//! shares. A counted road keeps its count, buses included.

use physics::bands::PERIODS;
use std::path::Path;

/// Buses a day on a street of one line direction where the service share is 1.
const BUSES_ONE_LINE_DIRECTION: f64 = 30.1;
/// How a street's buses grow with its line directions.
const LINE_DIRECTIONS_EXPONENT: f64 = 0.667;
/// The bus line directions a trolleybus line direction weighs (Sofia's and Prague's run more often).
const TROLLEYBUS_LINE_WEIGHT: f64 = 2.75;
const COACH_DEPARTURES: f64 = 4.0;
/// The buses' day, evening and night shares (07-19, 19-23, 23-07): the median place's bus stop
/// events in 43 of these timetables (Dublin 0.69/0.20/0.11, Prague 0.68/0.15/0.17).
pub const BUS_PERIOD_SHARES: [f64; PERIODS] = [0.731, 0.157, 0.112];

/// The routes per way, sorted by way id.
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
        self.ways.binary_search(&way).is_ok()
    }

    /// Buses and coaches a day on an OSM way, its lines without an interval running `service`
    /// of the buses where 100,000 trips a day begin or end within 2 km.
    pub fn daily(&self, way: i64, service: f64) -> (f64, f64) {
        let Ok(index) = self.ways.binary_search(&way) else {
            return (0.0, 0.0);
        };
        let ([bus, trolleybus, coach], [departures, coach_departures]) = self.service[index];
        let line_directions = f64::from(bus) + TROLLEYBUS_LINE_WEIGHT * f64::from(trolleybus);
        let buses =
            BUSES_ONE_LINE_DIRECTION * line_directions.powf(LINE_DIRECTIONS_EXPONENT) * service
                + f64::from(departures);
        (
            buses,
            f64::from(coach) * COACH_DEPARTURES + f64::from(coach_departures),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_street_carries_its_lines_buses() {
        let routes = BusRoutes::parse(
            "7 6 2 0 0.0 0.0\n9 0 0 1 108.0 0.0\n10 0 0 0 0.0 18.0\n11 2 0 0 0.0 0.0\n12 400 0 0 0.0 0.0\n",
        )
        .unwrap();
        let near = |(buses, coaches): (f64, f64), (expected, expected_coaches): (f64, f64)| {
            (buses - expected).abs() < 0.1 && coaches == expected_coaches
        };
        // A line both ways where the service share is 1: 47.8 buses; a quarter of them at a quarter.
        assert!(near(routes.daily(11, 1.0), (47.8, 0.0)));
        assert!(near(routes.daily(11, 0.25), (11.95, 0.0)));
        // Three bus lines both ways and a trolleybus line: 6 + 2 x 2.75 = 11.5 line directions.
        assert!(near(routes.daily(7, 1.0), (153.5, 0.0)));
        // A route every ten minutes runs its 108; a coach line 4 a day each way.
        assert_eq!(routes.daily(9, 1.0), (108.0, 4.0));
        assert_eq!(routes.daily(8, 1.0), (0.0, 0.0));
        assert!(routes.serves(10) && !routes.serves(8));
        // A coach route every hour stays coaches, not buses.
        assert_eq!(routes.daily(10, 1.0), (0.0, 18.0));
        // 400 line directions, as São Paulo's corridors list: 1,637 a day where the share is 1.
        assert!(near(routes.daily(12, 1.0), (1_637.3, 0.0)));
        assert!(BusRoutes::parse("9 1 0 0 0 0\n7 1 0 0 0 0\n").is_err());
    }
}
