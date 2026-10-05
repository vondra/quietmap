//! The people outside bars, pubs, nightclubs, beer gardens, restaurants, cafés and fast-food places
//! as sources (`physics::emission::people`, `venues`; research 2026-10-04, people): each place one
//! point 1.5 m up in front of the wall of the building it is mapped in (`outside`), with its seats
//! and door crowd by its hours and its country's customs, the terrace season of its climate, and
//! its share of the street crowd where the places along 40 m of street weigh three or more.

use super::Converted;
use super::cells::{Site, Z30Ring, push_site_points, site_points};
use super::venues::{VenueKind, VenueSite, cluster_weight, customs, hours, seats};
use super::{group_key, metres};
use crate::climate::Climate;
use crate::dev4::{degrees_to_z30, z30_corner_degrees};
use physics::emission::people::{
    EATING_DBA, LEAST_SEASON_SHARE, LIVELY_DBA, STANDING_DBA, TERRACE_SEASON_C, Venue,
    street_crowd_dba, venue_sound_power,
};
use serde_json::json;
use std::collections::HashMap;
use tiles::sources::{Attribute, GROUND_FROM_TERRAIN, Layer};

/// People sit at 1.1 m and stand at 1.6 m (Chauvineau 2025, Jacquesson 2017): one height between.
const PEOPLE_HEIGHT_M: f64 = 1.5;
/// A cluster's places are those within this distance (m) of a place: 40 m of street around it.
const CLUSTER_REACH_M: f64 = 20.0;

fn label(kind: VenueKind) -> &'static str {
    match kind {
        VenueKind::Bar => "people_bar",
        VenueKind::Pub => "people_pub",
        VenueKind::Nightclub => "people_nightclub",
        VenueKind::Biergarten => "people_biergarten",
        VenueKind::Restaurant => "people_restaurant",
        VenueKind::Cafe => "people_cafe",
        VenueKind::FastFood => "people_fast_food",
    }
}

/// Each place's share of its cluster's street crowd (dB(A) at the weekend's peak): the places
/// weighing within 20 m make the 40 m segment's crowd, shared among them.
fn crowd_shares(sites: &[VenueSite]) -> Vec<Option<f64>> {
    let cell = |site: &VenueSite| {
        let (gx, gy) = degrees_to_z30(site.lat, site.lon);
        (gx >> 10, gy >> 10)
    };
    let mut grid: HashMap<(i32, i32), Vec<usize>> = HashMap::new();
    for (index, site) in sites.iter().enumerate() {
        if cluster_weight(site.kind) > 0.0 {
            grid.entry(cell(site)).or_default().push(index);
        }
    }
    sites
        .iter()
        .map(|site| {
            if cluster_weight(site.kind) <= 0.0 {
                return None;
            }
            let (cx, cy) = cell(site);
            let near: Vec<&VenueSite> = (-1..=1)
                .flat_map(|dx| (-1..=1).map(move |dy| (cx + dx, cy + dy)))
                .filter_map(|key| grid.get(&key))
                .flatten()
                .map(|&other| &sites[other])
                .filter(|other| {
                    metres((site.lat, site.lon), (other.lat, other.lon)) <= CLUSTER_REACH_M
                })
                .collect();
            let weight: f64 = near.iter().map(|other| cluster_weight(other.kind)).sum();
            street_crowd_dba(weight).map(|crowd| crowd - 10.0 * (near.len() as f64).log10())
        })
        .collect()
}

/// The people of the square's places at `positions` (z30, outside their buildings). Returns how
/// many emit.
pub fn convert_people(
    sites: &[VenueSite],
    positions: &[(i32, i32)],
    (country_iso, climate): (u16, &Climate),
    out: &mut Vec<Converted>,
) -> usize {
    let customs = customs(country_iso);
    let crowds = crowd_shares(sites);
    let mut emitting = 0;
    for ((site, &position), crowd) in sites.iter().zip(positions).zip(crowds) {
        let (open, terrace) = hours(site, &customs);
        let drinking = matches!(
            site.kind,
            VenueKind::Bar | VenueKind::Pub | VenueKind::Nightclub | VenueKind::Biergarten
        );
        let (evening_dba, night_dba) = match site.kind {
            VenueKind::FastFood => (EATING_DBA, LIVELY_DBA),
            _ if drinking => (LIVELY_DBA, STANDING_DBA),
            _ => (LIVELY_DBA, LIVELY_DBA),
        };
        let venue = Venue {
            seats: seats(site),
            season_share: climate
                .share_of_year_above(site.lat, site.lon, TERRACE_SEASON_C)
                .max(LEAST_SEASON_SHARE),
            open: &open,
            terrace: &terrace,
            door_from_hour: match site.kind {
                VenueKind::Bar | VenueKind::Pub => Some(19),
                VenueKind::Nightclub => Some(0),
                _ => None,
            },
            evening_dba,
            night_dba,
            drinking,
            crowd_dba: crowd,
        };
        let Some(sound) = venue_sound_power(&venue) else {
            continue;
        };
        let loudest = [0.0, sound.evening_offset_db, sound.night_offset_db]
            .iter()
            .fold(f64::NEG_INFINITY, |a, &b| a.max(b))
            + sound.day_dba;
        let ring: Z30Ring = Vec::new();
        let point = Site {
            centroid: z30_corner_degrees(position.0, position.1),
            ring: &ring,
            area_m2: 1.0,
            single_point_up_to_m2: 1.0,
            cell_m: 1.0,
        };
        let attribute = Attribute {
            layer: Layer::Building,
            height_m: PEOPLE_HEIGHT_M,
            ground_percent: GROUND_FROM_TERRAIN,
            platform_half_width_m: 0.0,
            exclusion_radius_m: 0.0,
            footprint_id: 0,
            group_key: group_key(&[
                label(site.kind),
                &format!("{:.6},{:.6}", site.lat, site.lon),
            ]),
            emission: sound.band_levels_db(),
            display: json!([
                site.name,
                label(site.kind),
                PEOPLE_HEIGHT_M,
                0,
                0,
                "",
                (loudest * 10.0).round() / 10.0
            ])
            .to_string(),
        };
        push_site_points(&site_points(&point), 1.0, &attribute, out);
        emitting += 1;
    }
    emitting
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sources::venues::Seating;

    fn site(lat: f64, lon: f64, kind: VenueKind) -> VenueSite {
        VenueSite {
            lat,
            lon,
            kind,
            seating: Seating::Unknown,
            area_m2: 0.0,
            hours: None,
            name: String::new(),
        }
    }

    /// Four bars and a restaurant within 20 m weigh 3.8: each carries a fifth of the segment's
    /// crowd; a lone bar 200 m away and a café carry none.
    #[test]
    fn clustered_places_share_their_street_crowd() {
        let mut sites: Vec<VenueSite> = (0..4)
            .map(|i| site(41.38, 2.17 + f64::from(i) * 0.00005, VenueKind::Bar))
            .collect();
        sites.push(site(41.38005, 2.17, VenueKind::Restaurant));
        sites.push(site(41.382, 2.17, VenueKind::Bar));
        sites.push(site(41.38, 2.17001, VenueKind::Cafe));
        let shares = crowd_shares(&sites);
        let expected = street_crowd_dba(3.8).unwrap() - 10.0 * 5f64.log10();
        assert!((shares[0].unwrap() - expected).abs() < 1e-9);
        assert_eq!(shares[5], None);
        assert_eq!(shares[6], None);
    }
}
