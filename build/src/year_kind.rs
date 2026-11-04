//! Closed input kinds shared by manifest validation and every builder call site.
use serde::Deserialize;

macro_rules! kinds {
    ($($variant:ident => $name:literal $( [ $intermediate:ident ] )?),+ $(,)?) => {
        #[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
        pub enum Kind { $(#[serde(rename = $name)] $variant),+ }
        impl std::fmt::Display for Kind {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(match self { $(Self::$variant => $name),+ })
            }
        }
        impl Kind {
            pub fn typescript() -> String {
                let kinds = [$($name),+];
                let intermediate = [$(($name, kinds!(@intermediate $($intermediate)?))),+]
                    .into_iter().filter(|(_, intermediate)| *intermediate)
                    .map(|(name, _)| name).collect::<Vec<_>>();
                format!("// Generated from build/src/year_kind.rs by qm-build input-kinds.\nexport const INPUT_KINDS = {} as const\nexport type InputKind = typeof INPUT_KINDS[number]\nexport const INTERMEDIATE_KINDS: ReadonlySet<InputKind> = new Set({})\n",
                    serde_json::to_string_pretty(&kinds[..]).unwrap(),
                    serde_json::to_string_pretty(&intermediate).unwrap())
            }
        }
    };
    (@intermediate intermediate) => { true };
    (@intermediate) => { false };
}
kinds! {
    AirportTraffic => "airport-traffic" [intermediate],
    Barriers => "barriers" [intermediate],
    BuildingPlant => "building-plant",
    BuildingRoadModels => "building-road-models",
    Bus => "bus" [intermediate],
    BusModels => "bus-models",
    ChurchBells => "church-bells",
    Climate => "climate",
    CountedClassShares => "counted-class-shares",
    DwellingTrips => "dwelling-trips",
    ElectricCarShare => "electric-car-share",
    Facilities => "facilities" [intermediate],
    FacilityModel => "facility-model",
    FacilityObservations => "facility-observations" [intermediate],
    FineDem => "fine-dem",
    FreightPeriodShares => "freight-period-shares",
    Geoid => "geoid" [intermediate],
    HeavySpeeds => "heavy-speeds",
    HotelTripRate => "hotel-trip-rate",
    LegalSpeeds => "legal-speeds",
    ModelObservations => "model-observations",
    MotorcycleShare => "motorcycle-share",
    NationalDem => "national-dem",
    OsmGeography => "osm-geography" [intermediate],
    Structures => "structures" [intermediate],
    StructureObservations => "structure-observations" [intermediate],
    StructureHeightRaster => "structure-height-raster",
    StructureHeightModel => "structure-height-model",
    PrepareBoundaries => "prepare-boundaries",
    PrepareBuildingAttributes => "prepare-building-attributes",
    PrepareBuildingFootprints => "prepare-building-footprints",
    PrepareBuildingHeights => "prepare-building-heights",
    PrepareCrossings => "prepare-crossings",
    PrepareGeography => "prepare-geography",
    PrepareRegions => "prepare-regions" [intermediate],
    PrepareOsmSpeeds => "prepare-osm-speeds",
    PrepareGroundHeight => "prepare-ground-height",
    PrepareIndustrialSites => "prepare-industrial-sites",
    PrepareLandCover => "prepare-land-cover",
    PrepareNoiseBarriers => "prepare-noise-barriers",
    PrepareOsm => "prepare-osm",
    PrepareOsmBoundaries => "prepare-osm-boundaries" [intermediate],
    PrepareRailNetwork => "prepare-rail-network",
    PrepareRoadCounts => "prepare-road-counts",
    PrepareRoadHourly => "prepare-road-hourly",
    PrepareRoadNetwork => "prepare-road-network",
    PrepareSealing => "prepare-sealing",
    PrepareShips => "prepare-ships",
    PrepareTimetable => "prepare-timetable",
    PrepareTreeCover => "prepare-tree-cover",
    PrepareTreeHeight => "prepare-tree-height",
    PrepareTurbines => "prepare-turbines",
    PrepareWeather => "prepare-weather",
    PrepareWeatherStatistics => "prepare-weather-statistics" [intermediate],
    Prepared => "prepared" [intermediate],
    Primary => "primary",
    RailModel => "rail-model",
    RailFlows => "rail-flows",
    RailFleetRegions => "rail-fleet-regions",
    RasterMosaic => "raster-mosaic" [intermediate],
    Rasters => "rasters" [intermediate],
    RegionalVehicleShares => "regional-vehicle-shares",
    RoadObservations => "road-observations" [intermediate],
    RoadConversion => "road-conversion" [intermediate],
    RoadCalibration => "road-calibration" [intermediate],
    RoadPreparationModel => "road-preparation-model" [intermediate],
    RoadClassPriors => "road-class-priors",
    RoadClassShares => "road-class-shares",
    RoadPeriodShares => "road-period-shares",
    RoadReferenceCounts => "road-reference-counts",
    RoadTraffic => "road-traffic" [intermediate],
    RuralFreeFlow => "rural-free-flow",
    Secondary => "secondary",
    Segments => "segments" [intermediate],
    ServiceCooling => "service-cooling",
    Ships => "ships" [intermediate],
    Shuffled => "shuffled" [intermediate],
    SubdivisionGrids => "subdivision-grids",
    TaggedSpeeds => "tagged-speeds",
    Tent => "tent",
    Terrain => "terrain" [intermediate],
    Tiff => "tiff",
    TrainScales => "train-scales",
    Turbines => "turbines" [intermediate],
    VenueCustoms => "venue-customs",
    VenueSeating => "venue-seating",
    Venues => "venues" [intermediate],
    Water => "water" [intermediate],
    Weather => "weather" [intermediate],
    Worship => "worship" [intermediate],
    WorshipCalls => "worship-calls",
}
