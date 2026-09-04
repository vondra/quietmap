//! Enum-code → display-name lookups (surface, rail type, rail usage) shared by
//! the compute kernels and trace builders for popup labelling.
pub(crate) fn surface_name(surface_type: u8) -> &'static str {
    match surface_type {
        0 => "asphalt",
        1 => "paving",
        2 => "concrete",
        3 => "unpaved",
        4 => "gravel",
        _ => "asphalt",
    }
}

pub(crate) fn rail_type_name(rt: u8) -> &'static str {
    match rt {
        0 => "rail",
        1 => "tram",
        2 => "light_rail",
        3 => "narrow_gauge",
        4 => "funicular",
        _ => "rail",
    }
}

pub(crate) fn rail_usage_name(u: u8) -> &'static str {
    match u {
        0 => "main",
        1 => "branch",
        2 => "industrial",
        _ => "main",
    }
}

pub(crate) fn building_type_name(bt: u8) -> &'static str {
    // Leisure-area sources fold into the building/settlement layer (settlement
    // v2 phase 2); their PointSource carries `source_type = LEISURE_TYPE_BASE +
    // sport`, decoded here so the popup labels a padel court, not "default".
    if bt >= crate::types::LEISURE_TYPE_BASE {
        return leisure_type_name(bt - crate::types::LEISURE_TYPE_BASE);
    }
    match bt {
        0 => "residential_multi",
        1 => "commercial",
        2 => "warehouse",
        3 => "education",
        4 => "healthcare",
        5 => "worship",
        6 => "hotel",
        7 => "garage",
        8 => "farm",
        9 => "public",
        // Phase-2 additions (settlement::SILENT/HOUSE/FOOD_RETAIL/HOSPITALITY).
        // SILENT emits nothing so it never reaches a contributor; named for
        // completeness.
        10 => "silent",
        11 => "residential_house",
        12 => "food_retail",
        13 => "restaurant_bar",
        _ => "default",
    }
}

/// Display label for a leisure `sport` class (`emission::leisure` ids).
pub(crate) fn leisure_type_name(sport: u8) -> &'static str {
    use crate::emission::leisure::*;
    match sport {
        PADEL => "padel_court",
        TENNIS => "tennis_court",
        BASKETBALL => "ball_court",
        PLAYGROUND => "playground",
        POOL => "swimming_pool",
        OUTDOOR_SEATING => "outdoor_seating",
        STADIUM => "stadium",
        _ => "sports_pitch",
    }
}

pub(crate) fn industrial_type_name(st: u8) -> &'static str {
    match st {
        0 => "industrial_area",
        1 => "quarry",
        2 => "farm",
        3 => "factory",
        4 => "wastewater",
        10 => "wind_turbine",
        _ => "industrial_area",
    }
}
