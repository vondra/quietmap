//! Nearest national building attributes preserve mapped floors and explicit activity classes.

use super::{observations::Observation, prepare_rows::OsmBuilding};
use crate::osm::geometry::grid_cell_lonlat;
use std::collections::BTreeMap;

pub struct NationalAttributes<'a>(BTreeMap<(i32, i32), Vec<&'a Observation>>);

impl<'a> NationalAttributes<'a> {
    pub fn new(rows: &'a [Observation]) -> Self {
        let mut cells = BTreeMap::<_, Vec<_>>::new();
        for row in rows {
            cells
                .entry((
                    (row.latitude * 100.0).floor() as i32,
                    (row.longitude * 100.0).floor() as i32,
                ))
                .or_default()
                .push(row);
        }
        Self(cells)
    }

    pub fn apply(&self, building: &mut OsmBuilding) {
        if building.ground_activity {
            return;
        }
        let (longitude, latitude) =
            grid_cell_lonlat(building.row.centroid.0, building.row.centroid.1);
        let latitude_reach = 30.0 / 110_540.0;
        let longitude_reach = (latitude_reach
            / (latitude.abs() + latitude_reach)
                .min(90.0)
                .to_radians()
                .cos())
        .min(180.0);
        let mut nearest = None;
        let mut distance = 30.0;
        for y in ((latitude - latitude_reach) * 100.0).floor() as i32
            ..=((latitude + latitude_reach) * 100.0).floor() as i32
        {
            for x in ((longitude - longitude_reach) * 100.0).floor() as i32
                ..=((longitude + longitude_reach) * 100.0).floor() as i32
            {
                for point in self.0.get(&(y, x)).into_iter().flatten() {
                    if point.latitude < latitude - latitude_reach
                        || point.latitude > latitude + latitude_reach
                        || point.longitude < longitude - longitude_reach
                        || point.longitude > longitude + longitude_reach
                    {
                        continue;
                    }
                    let dy = (point.latitude - latitude) * 110_540.0;
                    let dx =
                        crate::osm::geometry::wrapped_longitude_delta(longitude, point.longitude)
                            * 111_320.0
                            * ((latitude + point.latitude) / 2.0).to_radians().cos();
                    let separation = dx.hypot(dy);
                    if separation < distance {
                        distance = separation;
                        nearest = Some(*point);
                    }
                }
            }
        }
        if let Some(point) = nearest {
            if building.row.floors == Some(0) && point.floors > 0 {
                building.row.floors = Some(point.floors);
            }
            if let Some(class) = point.building_type
                && !(building.row.building_type.is_some_and(|v| v >= 10) && class < 10)
            {
                building.row.building_type = Some(class);
            }
        }
    }
}
