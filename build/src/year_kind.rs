//! Closed input kinds shared by manifest validation and every builder call site.
use serde::Deserialize;

macro_rules! kinds {
    ($($variant:ident => $name:literal),+ $(,)?) => {
        #[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
        pub enum Kind { $(#[serde(rename = $name)] $variant),+ }
        impl std::fmt::Display for Kind {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(match self { $(Self::$variant => $name),+ })
            }
        }
        #[cfg(test)]
        pub const ALL: &[Kind] = &[$(Kind::$variant),+];
    };
}
kinds! {
    AirportTraffic => "airport-traffic",
    Barriers => "barriers",
    BuildingPlant => "building-plant",
    BuildingRoadModels => "building-road-models",
    Bus => "bus",
    BusModels => "bus-models",
    ChurchBells => "church-bells",
    Climate => "climate",
    CountedClassShares => "counted-class-shares",
    DwellingTrips => "dwelling-trips",
    ElectricCarShare => "electric-car-share",
    FineDem => "fine-dem",
    FreightPeriodShares => "freight-period-shares",
    Geoid => "geoid",
    HeavySpeeds => "heavy-speeds",
    HotelTripRate => "hotel-trip-rate",
    LegalSpeeds => "legal-speeds",
    ModelObservations => "model-observations",
    MotorcycleShare => "motorcycle-share",
    NationalDem => "national-dem",
    PrepareBoundaries => "prepare-boundaries",
    PrepareBuildingAttributes => "prepare-building-attributes",
    PrepareBuildingFootprints => "prepare-building-footprints",
    PrepareBuildingHeights => "prepare-building-heights",
    PrepareCrossings => "prepare-crossings",
    PrepareGroundHeight => "prepare-ground-height",
    PrepareIndustrialSites => "prepare-industrial-sites",
    PrepareLandCover => "prepare-land-cover",
    PrepareNoiseBarriers => "prepare-noise-barriers",
    PrepareOsm => "prepare-osm",
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
    Prepared => "prepared",
    Primary => "primary",
    RailFleetRegions => "rail-fleet-regions",
    RasterMosaic => "raster-mosaic",
    Rasters => "rasters",
    RegionalVehicleShares => "regional-vehicle-shares",
    RoadClassPriors => "road-class-priors",
    RoadClassShares => "road-class-shares",
    RoadPeriodShares => "road-period-shares",
    RoadReferenceCounts => "road-reference-counts",
    RoadTraffic => "road-traffic",
    RuralFreeFlow => "rural-free-flow",
    Secondary => "secondary",
    Segments => "segments",
    ServiceCooling => "service-cooling",
    Shuffled => "shuffled",
    SubdivisionGrids => "subdivision-grids",
    TaggedSpeeds => "tagged-speeds",
    Tent => "tent",
    Terrain => "terrain",
    Tiff => "tiff",
    TrainScales => "train-scales",
    Turbines => "turbines",
    VenueCustoms => "venue-customs",
    VenueSeating => "venue-seating",
    Venues => "venues",
    Water => "water",
    Weather => "weather",
    Worship => "worship",
    WorshipCalls => "worship-calls",
}
