//! Joined runway ownership uses its complete geometry and keeps a stable direction.
use super::*;
use crate::airport::lines::{AerowayLine, Airport};
use crate::airport::ownership::Aerodrome;
use crate::dev4::{Square, degrees_to_z30};
use crate::sources::cells::PreparedRing;
use std::sync::Arc;

#[test]
fn joined_ways_use_the_complete_geometry_and_keep_the_first_direction() {
    let mut original = Aeroways::default();
    for (index, key) in ["NEAR", "OWNER"].iter().enumerate() {
        let airport = Airport {
            key: (*key).into(),
            name: String::new(),
        };
        original.intern(airport.clone());
        original.aerodromes.push(Aerodrome {
            osm_id: index as i64,
            airport,
            centre: [50.0, if index == 0 { 14.0005 } else { 14.015 }],
            area_m2: if index == 0 { 0.0 } else { 1_000_000.0 },
            radius_m: 6_000.0,
            footprint: (index == 1).then(|| {
                Arc::new(
                    PreparedRing::new(
                        &[
                            [49.999, 14.001],
                            [50.001, 14.001],
                            [50.001, 14.031],
                            [49.999, 14.031],
                        ]
                        .map(|p| degrees_to_z30(p[0], p[1])),
                    )
                    .unwrap(),
                )
            }),
        });
    }
    // The lowest-id short end lies outside the airport; the two longer ways lie inside.
    // The highest-id middle way joins both ends in either hash iteration order.
    for (index, (start, end)) in [(14.0, 14.001), (14.03, 14.02), (14.001, 14.02)]
        .into_iter()
        .enumerate()
    {
        let pieces = 10;
        let at = |i: usize| [50.0, start + (end - start) * i as f64 / pieces as f64];
        for piece in 0..pieces {
            original.lines.push(AerowayLine {
                osm_id: index as i64,
                segment: piece as u16,
                ends: [at(piece), at(piece + 1)],
                aeroway: Aeroway::Runway,
                airport: u32::from(index != 0),
                square: Square { x: 275, y: 173 },
            });
        }
    }
    for _ in 0..64 {
        let mut aeroways = original.clone();
        let runways = Runways::new(&mut aeroways);
        assert_eq!(runways.runways.len(), 1);
        assert_eq!(runways.airport(0), 1);
        assert!(aeroways.lines.iter().all(|line| line.airport == 1));
        let path = runways
            .path(RollKey {
                runway: 0,
                forward: true,
                landing: false,
                class: 0,
            })
            .unwrap();
        assert!(path.end[1] > path.start[1]);
        assert_eq!(path.start, [50.0, 14.0]);
    }
}
