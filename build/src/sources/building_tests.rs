//! Building rows: the height ladder, ground activities, footprint ids and each class's plant.

use super::*;
use crate::sources::building_rows::read_batch;
use crate::structures::footprint_id;
use arrow_array::{
    ArrayRef, BinaryArray, Float32Array, Int32Array, Int64Array, RecordBatch, StringArray,
    UInt8Array, UInt32Array,
};
use physics::emission::plant::kitchen_hours;
use physics::emission::settlement::{area_law_lw_dba, building_profile};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

const CZ: u16 = u16::from_le_bytes(*b"CZ");

fn steady(emission: &BuildingEmission) -> &SoundPower {
    match &emission.emitted {
        Emitted::Steady(sound) => sound,
        Emitted::InUse(plants) => panic!("heard in use: {plants:?}"),
    }
}

fn in_use(emission: Option<BuildingEmission>) -> Vec<(&'static str, Running)> {
    match emission.map(|emission| emission.emitted) {
        Some(Emitted::InUse(plants)) => plants,
        other => panic!("not heard in use: {other:?}"),
    }
}

/// A climate of Prague's temperatures (a mean of 9.3 C, 18.2 in the warmest quarter, 0.6 in the
/// coldest: 2,640 heating degree days, 40 cooling) in the cells around `(lat, lon)`, the rest of
/// the world sea (no degree days).
fn climate_around(lat: f64, lon: f64) -> Climate {
    static CALLS: AtomicUsize = AtomicUsize::new(0);
    let (cell_row, cell_column) = (
        ((90.0 - lat) * 6.0) as usize,
        ((lon + 180.0) * 6.0) as usize,
    );
    let grid = |value: f32| {
        let mut cells = vec![-3.4e38f32; 2_160 * 1_080];
        for r in cell_row - 1..=cell_row + 1 {
            cells[r * 2_160 + cell_column - 1..=r * 2_160 + cell_column + 1].fill(value);
        }
        cells
            .iter()
            .flat_map(|cell| cell.to_le_bytes())
            .collect::<Vec<u8>>()
    };
    let directory = std::env::temp_dir().join(format!(
        "qm-climate-{}-{}",
        std::process::id(),
        CALLS.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&directory).unwrap();
    for (name, value) in [("bio1.f32", 9.3), ("bio10.f32", 18.2), ("bio11.f32", 0.6)] {
        std::fs::write(directory.join(name), grid(value)).unwrap();
    }
    let climate = Climate::load(&directory).unwrap();
    std::fs::remove_dir_all(&directory).unwrap();
    climate
}

fn row(class: u8, height_tag_m: f64, floors: u8, ground_activity: bool) -> BuildingRow {
    BuildingRow {
        osm_id: 1,
        centroid: (1 << 29, 1 << 29),
        ring: Vec::new(),
        height_tag_m,
        floors,
        storeys: floors,
        area_m2: Some(1_000.0),
        class,
        ground_activity,
        name: String::new(),
        address: String::new(),
        footprint_id: 0,
    }
}

/// A ground activity has no floors and no invented facade: it emits its footprint at 1.5 m
/// whatever its tags; an untagged building stands 8 m (3 floors) with its source at 4 m.
#[test]
fn ground_activities_emit_their_footprint_at_one_and_a_half_metres() {
    // The rows stand at sea, beyond the climate's cells.
    let climate = climate_around(50.0, 14.5);
    let emission = |row: &BuildingRow| building_emission(row, (CZ, &climate), None);
    for class in [3, 7, 9] {
        let ground = emission(&row(class, 0.0, 0, true)).unwrap();
        assert_eq!(ground, emission(&row(class, 24.0, 8, true)).unwrap());
        assert_eq!(
            (ground.source_height_m, ground.height_m, ground.floors),
            (1.5, 0.0, 0)
        );
        let profile = building_profile(class).unwrap();
        let expected = area_law_lw_dba(profile.lw_fixed_dba, profile.lw_per_m2_dba, 1_000.0);
        assert!((steady(&ground).day_dba - expected).abs() < 1e-9);
        let building = emission(&row(class, 0.0, 0, false)).unwrap();
        assert_eq!((building.floors, building.source_height_m), (3, 4.0));
    }
    let shop = emission(&row(COMMERCIAL, 0.0, 0, true)).unwrap();
    assert_eq!(shop, emission(&row(COMMERCIAL, 24.0, 8, true)).unwrap());
    assert_eq!(
        emission(&row(10, 30.0, 10, false)),
        None,
        "sheds are silent"
    );
}

/// The ceiling applies after the whole ladder: tag errors stop at 828 m, 255 floors stay 765 m.
#[test]
fn source_height_is_half_the_laddered_height_under_the_tallest_building() {
    let climate = climate_around(50.0, 14.5);
    let source = |height_tag_m: f64, floors: u8| {
        building_emission(&row(1, height_tag_m, floors, false), (CZ, &climate), None)
            .unwrap()
            .source_height_m
    };
    assert_eq!((source(31_231.0, 2), source(0.0, u8::MAX)), (414.0, 382.5));
    assert!((source(827.8, 0) - 413.9).abs() < 1e-9);
    assert_eq!((source(300.0, 0), source(12.0, 4)), (150.0, 6.0));
}

fn batch() -> RecordBatch {
    let ring = |x: i32| -> Vec<u8> {
        let words = [4, x, 0, x + 800, 0, x + 800, 800, x, 800];
        words
            .iter()
            .flat_map(|word: &i32| word.to_le_bytes())
            .collect()
    };
    let screening = [1u32, 1, 4, 0, 0, 800, 0, 800, 800, 0, 800];
    let screening: Vec<u8> = screening
        .iter()
        .flat_map(|word| word.to_le_bytes())
        .collect();
    let (a, b) = (ring(1 << 29), ring((1 << 29) + 5_000));
    let columns: Vec<(&str, ArrayRef)> = vec![
        ("kind", Arc::new(UInt8Array::from(vec![0, 0, 0, 1]))),
        (
            "osm_id",
            Arc::new(Int64Array::from(vec![Some(7), Some(8), None, None])),
        ),
        (
            "centroid_gx",
            Arc::new(Int32Array::from(vec![1 << 29, 1 << 29, 0, 0])),
        ),
        (
            "centroid_gy",
            Arc::new(Int32Array::from(vec![400, 400, 0, 0])),
        ),
        (
            "emission_centroid_gx",
            Arc::new(Int32Array::from(vec![
                Some((1 << 29) + 400),
                None,
                None,
                None,
            ])),
        ),
        (
            "emission_centroid_gy",
            Arc::new(Int32Array::from(vec![Some(400), None, None, None])),
        ),
        (
            "height",
            Arc::new(Float32Array::from(vec![Some(12.0), None, None, None])),
        ),
        (
            "floors",
            Arc::new(UInt8Array::from(vec![Some(4), Some(0), None, None])),
        ),
        (
            "storeys",
            Arc::new(UInt8Array::from(vec![Some(0), Some(2), None, None])),
        ),
        (
            "area_m2",
            Arc::new(Float32Array::from(vec![
                Some(900.0),
                Some(900.0),
                None,
                None,
            ])),
        ),
        (
            "building_type",
            Arc::new(UInt8Array::from(vec![Some(0), Some(1), None, None])),
        ),
        (
            "height_source",
            Arc::new(UInt8Array::from(vec![1, 7, 1, 0])),
        ),
        (
            "emission_geom",
            Arc::new(BinaryArray::from(vec![
                Some(a.as_slice()),
                Some(b.as_slice()),
                None,
                None,
            ])),
        ),
        (
            "geom",
            Arc::new(BinaryArray::from(vec![
                Some(screening.as_slice()),
                None,
                Some(screening.as_slice()),
                None,
            ])),
        ),
        (
            "screening_ordinal",
            Arc::new(UInt32Array::from(vec![Some(5), None, Some(6), None])),
        ),
        (
            "name",
            Arc::new(StringArray::from(vec![Some("Dum"), None, None, None])),
        ),
        (
            "addr_street",
            Arc::new(StringArray::from(vec![
                Some("Gower Street"),
                Some("Store Street"),
                None,
                None,
            ])),
        ),
        (
            "addr_housenumber",
            Arc::new(StringArray::from(vec![Some("12"), None, None, None])),
        ),
    ];
    RecordBatch::try_from_iter(columns).unwrap()
}

/// OSM buildings only; a screening footprint's id is the obstacles builder's, a ground activity
/// has none; the emission centroid is taken when present.
#[test]
fn rows_carry_the_footprint_id_of_their_screening_outline() {
    let square = Square { x: 276, y: 173 };
    let rows = read_batch(&batch(), square).unwrap();
    assert_eq!(
        rows.iter().map(|row| row.osm_id).collect::<Vec<_>>(),
        vec![7, 8]
    );
    assert_eq!(rows[0].footprint_id, footprint_id(square, 5));
    assert_eq!((rows[1].footprint_id, rows[1].ground_activity), (0, true));
    assert_eq!(rows[0].centroid, ((1 << 29) + 400, 400));
    assert_eq!(rows[1].centroid, (1 << 29, 400));
    assert_eq!(
        (rows[0].address.as_str(), rows[1].address.as_str()),
        ("Gower Street 12", "Store Street")
    );
    assert_eq!(
        (rows[0].height_tag_m, rows[0].floors, rows[0].ring.len()),
        (12.0, 4, 4)
    );
    assert_eq!(
        (rows[0].storeys, rows[1].storeys),
        (4, 2),
        "dev4's storeys, else the floors"
    );
}

/// A home emits its dwellings' outdoor units: a 120 m2 one-storey house in Prague's climate is one
/// dwelling (45.0 dB(A)), a 1,000 m2 four-storey block of flats fifty; a shed of 20 m2 nothing.
#[test]
fn homes_emit_their_dwellings_units_by_country_and_climate() {
    let (lat, lon) = (50.0, 14.5);
    let climate = climate_around(lat, lon);
    let at = crate::dev4::degrees_to_z30(lat, lon);
    let home = |class: u8, storeys: u8, area: f64| {
        let mut home = row(class, 0.0, storeys, false);
        (home.storeys, home.area_m2, home.centroid) = (storeys, Some(area), at);
        building_emission(&home, (CZ, &climate), None).map(|emission| steady(&emission).day_dba)
    };
    let house = home(0, 1, 120.0).unwrap();
    assert!((house - 45.0).abs() < 0.2, "{house}");
    let flats = home(0, 4, 1_000.0).unwrap();
    assert!(
        (flats - house - 10.0 * 50f64.log10()).abs() < 1e-9,
        "{flats}"
    );
    assert_eq!(home(0, 1, 20.0), None, "a shed");
}

/// A shop's plant cools its gross floor area in business hours, in Prague's climate as 9.9 % of
/// Czechia's service floor is cooled, less in the evening, silent at night. The building of a
/// restaurant runs its extract over its footprint in the kitchen's hours and its cooling while its
/// places are open; a building of cafés only its cooling; one with no place mapped in it a
/// restaurant's.
#[test]
fn shops_and_the_buildings_of_bars_and_restaurants_run_their_plant_in_its_hours() {
    let (lat, lon) = (50.0, 14.5);
    let climate = climate_around(lat, lon);
    let at = crate::dev4::degrees_to_z30(lat, lon);
    let building = |class: u8, floors: u8| BuildingRow {
        centroid: at,
        area_m2: Some(200.0),
        ..row(class, 0.0, floors, false)
    };
    let emission = |class: u8, floors: u8, hosted: Option<&PlaceHours>| {
        in_use(building_emission(
            &building(class, floors),
            (CZ, &climate),
            hosted,
        ))
    };
    let shop: Vec<Running> = emission(COMMERCIAL, 2, None)
        .into_iter()
        .map(|(name, plant)| {
            assert_eq!(name, "commercial");
            plant
        })
        .collect();
    assert_eq!(shop.len(), 1);
    assert!(shop[0].duty[0] > 0.0 && shop[0].duty[0] < 0.01, "{shop:?}");
    assert!(shop[0].duty[1] > 0.0 && shop[0].duty[1] < shop[0].duty[0]);
    assert_eq!(shop[0].duty[2], 0.0);
    let one_floor = emission(COMMERCIAL, 1, None);
    assert!(
        shop[0].running_dba > one_floor[0].1.running_dba + 4.0,
        "two floors cool twice the area"
    );
    let mut lunch_and_dinner = [[false; 24]; 7];
    for day in &mut lunch_and_dinner {
        day[12..15].fill(true);
        day[18..22].fill(true);
    }
    let restaurant = PlaceHours {
        open: lunch_and_dinner,
        kitchen: Some(kitchen_hours(&lunch_and_dinner)),
    };
    let [("kitchen_extract", extract), ("restaurant_bar", cooling)] =
        &emission(HOSPITALITY, 3, Some(&restaurant))[..]
    else {
        panic!("an extract and a cooling plant");
    };
    assert_eq!(extract.duty, [7.0 / 12.0, 1.0, 0.0]);
    assert!(
        (extract.running_dba - (47.0 + 10.0 * 200f64.log10())).abs() < 1e-9,
        "its footprint"
    );
    assert!(cooling.duty[1] > 0.0 && cooling.duty[0] < extract.duty[0]);
    let cafes = PlaceHours {
        open: lunch_and_dinner,
        kitchen: None,
    };
    assert_eq!(
        emission(HOSPITALITY, 3, Some(&cafes)),
        [("restaurant_bar", *cooling)]
    );
    let unmapped = unmapped_place_hours(&crate::sources::venues::customs(CZ));
    assert_eq!(emission(HOSPITALITY, 3, Some(&unmapped)).len(), 2);
}
