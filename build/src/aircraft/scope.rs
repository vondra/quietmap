//! The boxes a scoped run keeps (`south,west,north,east;..`): a trace stays when the extent of its
//! sane points meets a box, so every segment reaching a box belongs to a kept trace.

use super::trace::TracePoint;

#[derive(Clone, Debug, PartialEq)]
pub struct Scope {
    boxes: Vec<[f64; 4]>,
}

impl Scope {
    pub fn parse(text: &str) -> Result<Self, String> {
        let boxes = text
            .split(';')
            .map(|part| {
                let values = part
                    .split(',')
                    .map(|v| {
                        v.trim()
                            .parse::<f64>()
                            .map_err(|_| format!("box {part:?}: not a number"))
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                let [south, west, north, east] = values[..] else {
                    return Err(format!("box {part:?} is not south,west,north,east"));
                };
                let valid = values.iter().all(|v| v.is_finite())
                    && (-90.0..=90.0).contains(&south)
                    && (-90.0..=90.0).contains(&north)
                    && (-180.0..=180.0).contains(&west)
                    && (-180.0..=180.0).contains(&east)
                    && south <= north
                    && west <= east;
                if valid {
                    Ok([south, west, north, east])
                } else {
                    Err(format!("box {part:?} out of range or unordered"))
                }
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Scope { boxes })
    }

    /// The canonical text recorded in receipts and file metadata.
    pub fn key(&self) -> String {
        self.boxes
            .iter()
            .map(|b| format!("{},{},{},{}", b[0], b[1], b[2], b[3]))
            .collect::<Vec<_>>()
            .join(";")
    }

    /// Whether the extent of `points` meets a box; an extent spanning 180 degrees of longitude or
    /// more (the antimeridian) is kept.
    pub fn touches<'a>(&self, points: impl Iterator<Item = &'a TracePoint>) -> bool {
        let mut extent: Option<[f64; 4]> = None;
        for point in points {
            let (lat, lon) = (f64::from(point.lat), f64::from(point.lon));
            let e = extent.get_or_insert([lat, lon, lat, lon]);
            *e = [e[0].min(lat), e[1].min(lon), e[2].max(lat), e[3].max(lon)];
        }
        let Some([south, west, north, east]) = extent else {
            return false;
        };
        self.boxes.iter().any(|b| {
            south <= b[2]
                && north >= b[0]
                && (east - west >= 180.0 || (east >= b[1] && west <= b[3]))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::aircraft::altitude::tests::sample;

    #[test]
    fn a_trace_is_kept_when_its_extent_meets_a_box() {
        let scope = Scope::parse("49.5,13.82,50.7,15.02;33.45,-112.67,34.05,-111.47").unwrap();
        assert_eq!(
            scope.key(),
            "49.5,13.82,50.7,15.02;33.45,-112.67,34.05,-111.47"
        );
        let track = |points: &[(f32, f32)]| {
            points
                .iter()
                .map(|&(lat, lon)| sample(0.0, lat, lon, 1000.0, 100.0).point)
                .collect::<Vec<_>>()
        };
        assert!(scope.touches(track(&[(49.0, 13.0), (51.0, 16.0)]).iter()));
        assert!(!scope.touches(track(&[(48.0, 13.0), (49.0, 16.0)]).iter()));
        assert!(scope.touches(track(&[(33.0, -112.0), (35.0, -112.0)]).iter()));
        assert!(scope.touches(track(&[(50.0, 179.0), (50.0, -179.0)]).iter()));
        assert!(!scope.touches(std::iter::empty()));
        for bad in [
            "1,2,3",
            "30,18,27,20",
            "NaN,0,1,1",
            "-91,0,1,1",
            "0,0,1,181",
        ] {
            assert!(Scope::parse(bad).is_err(), "{bad}");
        }
    }
}
