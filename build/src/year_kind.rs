//! Closed input kinds shared by manifest validation and every builder call site.
use serde::Deserialize;

/// Uniform admission bucket for every declared input kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Admission {
    /// Point-in-time world geometry snapshots (OSM planet, Overture footprints): strictly < as_of.
    Snapshot,
    /// Physical structures, static rasters, geography/networks, station lookups and inventories:
    /// later-observed inventories stand for the map date, with per-item date filtering where present.
    Structure,
    /// Flow, activity, practice and model inputs: < as_of by default, or validity / fallback_vintage.
    Activity,
}

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
    AirportOperations => "airport-operations",
    AirportTraffic => "airport-traffic" [intermediate],
    Barriers => "barriers",
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
    Lodgings => "lodgings" [intermediate],
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

impl Kind {
    /// Only standing practices and legal speed rules admit a manifest `validity` span.
    pub fn allows_validity(self) -> bool {
        matches!(
            self,
            Self::ChurchBells
                | Self::HeavySpeeds
                | Self::LegalSpeeds
                | Self::PrepareOsmSpeeds
                | Self::VenueCustoms
                | Self::WorshipCalls
        )
    }

    pub fn admission(self) -> Admission {
        use Kind::*;
        match self {
            PrepareOsm | PrepareBuildingFootprints => Admission::Snapshot,
            Barriers
            | Climate
            | FineDem
            | NationalDem
            | PrepareBoundaries
            | PrepareBuildingAttributes
            | PrepareBuildingHeights
            | PrepareGeography
            | PrepareGroundHeight
            | PrepareIndustrialSites
            | PrepareLandCover
            | PrepareNoiseBarriers
            | PrepareOsmBoundaries
            | PrepareRailNetwork
            | PrepareRoadNetwork
            | PrepareSealing
            | PrepareTreeCover
            | PrepareTreeHeight
            | PrepareTurbines
            | PrepareWeather
            | RasterMosaic
            | StructureHeightRaster
            | Tent
            | Tiff
            | Water => Admission::Structure,
            AirportOperations
            | AirportTraffic
            | BuildingPlant
            | BuildingRoadModels
            | Bus
            | BusModels
            | ChurchBells
            | CountedClassShares
            | DwellingTrips
            | ElectricCarShare
            | Facilities
            | FacilityModel
            | FacilityObservations
            | FreightPeriodShares
            | Geoid
            | HeavySpeeds
            | HotelTripRate
            | LegalSpeeds
            | Lodgings
            | ModelObservations
            | MotorcycleShare
            | OsmGeography
            | PrepareCrossings
            | PrepareOsmSpeeds
            | PrepareRegions
            | PrepareRoadCounts
            | PrepareRoadHourly
            | PrepareShips
            | PrepareTimetable
            | PrepareWeatherStatistics
            | Prepared
            | Primary
            | RailFleetRegions
            | RailFlows
            | RailModel
            | Rasters
            | RegionalVehicleShares
            | RoadCalibration
            | RoadClassPriors
            | RoadClassShares
            | RoadConversion
            | RoadObservations
            | RoadPeriodShares
            | RoadPreparationModel
            | RoadReferenceCounts
            | RoadTraffic
            | RuralFreeFlow
            | Secondary
            | Segments
            | ServiceCooling
            | Ships
            | Shuffled
            | StructureHeightModel
            | StructureObservations
            | Structures
            | SubdivisionGrids
            | TaggedSpeeds
            | Terrain
            | TrainScales
            | Turbines
            | VenueCustoms
            | VenueSeating
            | Venues
            | Weather
            | Worship
            | WorshipCalls => Admission::Activity,
        }
    }
}
