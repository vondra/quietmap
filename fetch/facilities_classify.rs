//! Publisher activity codes become NACE classes; unknown codes remain unknown.

use serde_json::Value;

pub fn text(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

pub fn field(properties: &Value, names: &[impl AsRef<str>]) -> String {
    names
        .iter()
        .map(|n| text(&properties[n.as_ref()]))
        .find(|s| !s.is_empty())
        .unwrap_or_default()
}

pub fn number(value: &str) -> Option<f64> {
    // Published numeric fields can include unit suffixes; no missing value becomes zero.
    let end = value
        .trim()
        .char_indices()
        .find(|(_, c)| !c.is_ascii_digit() && !matches!(c, '.' | '+' | '-' | 'e' | 'E'))
        .map_or(value.trim().len(), |(i, _)| i);
    value.trim()[..end]
        .parse::<f64>()
        .ok()
        .filter(|v| v.is_finite())
}

fn gem_fuel(value: &str, explicit: bool) -> u16 {
    let value = value.to_lowercase();
    let has = |needles: &[&str]| needles.iter().any(|n| value.contains(n));
    if value.is_empty() || value == "unknown" || has(&["wind"]) {
        0
    } else if !explicit && has(&["solar"]) {
        3599
    } else if has(&["hydro"]) || explicit && has(&["pump"]) {
        3512
    } else if has(&["solar"]) || explicit && has(&["csp", "photovolt", "pv"]) {
        3599
    } else if !explicit
        || has(&[
            "coal",
            "nuclear",
            "gas",
            "oil",
            "biomass",
            "bioenergy",
            "thermal",
            "fossil",
            "diesel",
            "peat",
        ])
    {
        3511
    } else {
        0
    }
}

fn annex(value: &str) -> u16 {
    let compact: String = value.chars().filter(|c| !c.is_whitespace()).collect();
    let entries = [
        ("1(a)", 1920),
        ("1(b)", 1920),
        ("1(c)", 3511),
        ("1(d)", 1910),
        ("1(e)", 1920),
        ("1(f)", 1920),
        ("2(a)", 2410),
        ("2(b)", 2410),
        ("2(c)(i)", 2410),
        ("2(c)(ii)", 2550),
        ("2(c)(iii)", 2561),
        ("2(c)", 2410),
        ("2(d)", 2451),
        ("2(e)(i)", 2442),
        ("2(e)(ii)", 2453),
        ("2(e)", 2453),
        ("2(f)", 2561),
        ("3(a)", 810),
        ("3(b)", 812),
        ("3(c)(i)", 2351),
        ("3(c)(ii)", 2352),
        ("3(c)(iii)", 2352),
        ("3(c)", 2351),
        ("3(d)", 2399),
        ("3(e)", 2311),
        ("3(f)", 2399),
        ("3(g)", 2332),
        ("4(a)", 2011),
        ("4(b)", 2011),
        ("4(c)", 2015),
        ("4(d)", 2020),
        ("4(e)", 2120),
        ("4(f)", 2051),
        ("5(a)", 3822),
        ("5(b)", 3821),
        ("5(c)", 3821),
        ("5(d)", 3821),
        ("5(e)", 1011),
        ("5(f)", 3700),
        ("5(g)", 3700),
        ("6(a)", 1711),
        ("6(b)", 1712),
        ("6(c)", 1610),
        ("7(a)", 146),
        ("7(b)", 321),
        ("8(a)", 1011),
        ("8(b)", 1089),
        ("8(c)", 1051),
        ("9(a)", 1330),
        ("9(b)", 1511),
        ("9(c)", 2561),
        ("9(d)", 2399),
        ("9(e)", 3011),
    ];
    entries
        .into_iter()
        .filter(|(key, _)| compact.starts_with(key))
        .max_by_key(|(key, _)| key.len())
        .map_or(0, |(_, code)| code)
}

pub fn activity(parser: &str, p: &Value, constant: u16) -> anyhow::Result<u16> {
    let fuel = field(p, &["Type", "FuelType", "Fuel", "primary_fuel"]);
    Ok(match parser {
        "constant" => constant,
        "gppd" => match field(p, &["primary_fuel"]).trim() {
            "Hydro" | "Geothermal" => 3512,
            "Solar" => 3599,
            "Nuclear" | "Gas" | "Oil" | "Coal" | "Petcoke" | "Biomass" | "Waste" => 3511,
            _ => 0,
        },
        "annex" => annex(&field(p, &["EPRTRAnnexIMainActivity"])),
        "gem-fuel" => gem_fuel(&fuel, false),
        "explicit-fuel" => gem_fuel(&fuel, true),
        "generation-type" => match field(p, &["Tipo"]).to_uppercase().as_str() {
            "HE" | "MICRO CENTRAL HIDROELECTRICA" => 3512,
            "SL" | "SOLAR FOTOVOTAICA" => 3599,
            "TG"
            | "BM"
            | "DO"
            | "TERMOELECTRICA DIESEL O GAS OIL"
            | "TERMOELECTRICA GAS NATURAL" => 3511,
            _ => 0,
        },
        "generation-fuel" => {
            let f = fuel.to_lowercase();
            if ["wind", "eolic", "eólic"].iter().any(|s| f.contains(s))
                || f.is_empty()
                || f.contains("unknown")
            {
                0
            } else if f.contains("hydro") {
                3512
            } else if ["solar", "photovolt", "fotovolt"]
                .iter()
                .any(|s| f.contains(s))
            {
                3599
            } else {
                3511
            }
        }
        "plant-category" => {
            let f = field(p, &["CATEGORY"]).to_lowercase();
            if f.contains("wind") {
                0
            } else if f.contains("hydro") || f.contains("pump") {
                3512
            } else if f.contains("csp") || f.contains("solar") {
                3599
            } else {
                3511
            }
        }
        "mineral-title" => {
            let f = field(p, &["MINERALES"]).to_uppercase();
            if ["CARBÓN", "CARBON", "ANTRACITA", "HULLA", "LIGNITO", "TURBA"]
                .iter()
                .any(|s| f.contains(s))
            {
                500
            } else if [
                "ORO",
                "COBRE",
                "HIERRO",
                "PLATA",
                "ZINC",
                "PLOMO",
                "NIQUEL",
                "PLATIN",
                "MOLIBDENO",
                "MANGANESO",
                "COBALTO",
                "ESTAÑO",
                "TUNGSTENO",
                "MERCURIO",
                "EMERALDA",
                "ESMERALDA",
            ]
            .iter()
            .any(|s| f.contains(s))
            {
                700
            } else if [
                "ARENA",
                "GRAVA",
                "ARCILLA",
                "CALIZA",
                "MARMOL",
                "GRANITO",
                "YESO",
                "FELDESPATO",
                "CAOLIN",
                "RECEBO",
                "ROCA",
                "CANTERA",
                "BENTONITA",
                "DOLOMITA",
                "ANHIDRITA",
                "FOSFORICA",
            ]
            .iter()
            .any(|s| f.contains(s))
            {
                800
            } else {
                700
            }
        }
        other => anyhow::bail!("unknown facility classifier {other}"),
    })
}
