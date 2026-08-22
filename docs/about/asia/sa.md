---
title: Saudi Arabia
intro: Noise mapping data sources for the Kingdom of Saudi Arabia.
map: { center: [45.0, 24.0], zoom: 6 }
---

## Road traffic

### MoT count stations + Riyadh PMS + SA national atlas (3-tier enrichment)

Three open-data sources combine to give per-corridor AADT with vehicle-class split:

**Tier 1 — [Ministry of Transport Open Data (MoTLS)](https://mot.gov.sa/en/open-data)** publishes a [Traffic density on roads XLSX](https://mot.gov.sa/documents/7507107/7507109/Traffic+density+on+roads.xlsx/a8c0fb92-e478-5eba-bc50-3435a86e077a) with 32 count stations (full year 2024) across 16 Saudi highways: Road 10, 15, 30, 40, 65, 175, 177, 180, 205, 246, 513, 517, 1144, 3409, 3410, 5816. Each station has `Latitude, Longitude, 24 Hour Total, Total`. Averaged per Road No. and matched to OSM by `ref` tag.

| Road ref | AADT (rural average, 2024) | Use |
|---|---:|---|
| 65 | 14,692 | Riyadh ↔ Qassim |
| 517 | 14,422 | Riyadh urban arterial |
| 40 | 11,345 | **Highway 40** Jeddah ↔ Riyadh ↔ Dammam (1500 km national spine) |
| 513 | 7,618 | Riyadh metropolitan |
| 246 | 6,407 | Al Baha area |
| 205 | 3,719 | Al Baha ↔ Taif |
| 15 | 3,619 | Jazan ↔ Najran |
| 30 | 3,033 | Central SA |
| 10 | 2,378 | Riyadh ↔ Dammam |
| 180 | 1,654 | Southern SA |

Caveat: MoT stations are placed on rural corridor sections, so urban Highway 40 near Jeddah/Riyadh is under-estimated — peak urban AADT is typically 40-80k.

**Tier 2 — Riyadh PMS (Pavement Management System)** — the Royal Commission for Riyadh City / ArRiyadh Development Authority hosts a public ArcGIS FeatureServer with 1,604 lane-level polylines inside central Riyadh, with CLASS (A/B/C/D), NO_OF_LANE, LANE_WIDTH, STREET_NAM (Arabic), and DIRECTION. Class + lane count maps to AADT (Class A 5-lane = 50k, Class D 2-lane = 1.8k). Spatially matched within 200 m inside the Riyadh bbox [24.4-25.1°N, 46.4-47.0°E]. Endpoint: `services9.arcgis.com/7cs4rq15YlksXBMf/arcgis/rest/services/RiyadhPMS/FeatureServer`.

**Tier 3 — [Interactive Atlas of SA Transportation Networks](https://www.arcgis.com/home/item.html?id=a69a52e770ba4f91950cfd208c556dcb)** — a publicly hosted ArcGIS Online feature service with 3,555 national road polylines classified as Primary Route (1,294), Secondary Route (2,146), or Unknown (107). Spatially matched within 500 m as national fallback for segments not in Tier 1 or 2. Endpoint: `services6.arcgis.com/UBlzpwddcwD1J1A0/arcgis/rest/services/Interactive_atlas_of_spatial_features_and_transportation_networks_in_Saudi_Arabia_WFL1/FeatureServer/6`.

**Vehicle-class split** (CNOSSOS-tuned for Gulf oil-freight traffic): 78% light / 10% medium / 11% heavy / 1% moto — SA has an elevated heavy-vehicle share due to Aramco/SABIC/petrochem trucking.

**Coverage**: the major road network (OSM motorway..tertiary + `_link`) across the SA hexes is matched in priority order — MoT `ref` first, then Riyadh PMS inside the Riyadh bbox, then the SAU Atlas Primary/Secondary spatial fallback. The bulk of matches come from the SAU Atlas national fallback (most segments lack a MoT `ref` and sit outside Riyadh); the class gate keeps residential/service streets from inheriting a nearby highway's AADT.

### Blocked government portals (egress-level TCP block, not WAF)

The following canonical portals were investigated but are TCP-blocked from non-SA IPs at the network egress layer (not WAF fingerprinting — a real browser also fails):

- **[data.gov.sa](https://data.gov.sa/)** (Saudi Open Data Portal, SDAIA) — 15,286-dataset catalog, 874 Transport entries, but unreachable
- **[Transport General Authority (TGA) Open Data](https://www.tga.gov.sa/en/KnowledgeCenter/OpenData)**
- **[GASTAT Road Transport Statistics 2024](https://stats.gov.sa/)** — PDF only, national totals
- **[RCRC Open Data Portal](https://opendata.rcrc.gov.sa/)** — reachable but catalog (35 datasets) confirmed to have no AADT layer; only `traffic-intersections-by-main-street-and-cross-street-2024` (633 intersection points, no volumes)
- **mapservice.alriyadh.gov.sa** (Riyadh Municipality GIS viewer) — blocked, but the underlying ArcGIS tiles are mirrored to ArcGIS Online services9, which is how we reached Riyadh PMS
- **[General Authority for Roads (RGA)](https://rga.gov.sa/)**, **[GEOSA (Survey and Geospatial Authority)](https://gasgi.gov.sa/)** — no open-data download endpoints

## Railway

### No open GTFS for any Saudi operator

Saudi Arabia publishes no GTFS feeds — for any operator, and there is no bespoke SA rail enricher. Saudi rail therefore uses the engine's generic CNOSSOS class defaults, differentiated only by OSM `rail_type`, `usage`, and `highspeed` (no SA-specific traffic counts are applied).

### SAR (Saudi Arabia Railways)

[SAR](https://www.sar.com.sa/) operates the national network, formed in 2021 by merging the old Saudi Railway Organization (SRO) eastern line with the newer SAR North line:

- **East Line** (former SRO) — Dammam ↔ Abqaiq ↔ Hofuf ↔ Harad ↔ Riyadh (733 km, double track). Rolling stock: 22 trainsets (passenger) + mixed freight.
- **North Line** — Riyadh ↔ Al-Majma'ah ↔ Qassim ↔ Hail ↔ Al-Jalamid (bauxite/phosphate freight terminal) ↔ Al Jawf ↔ Qurayyat (1,250 km). 6 passenger trainsets (4 day + 2 night).
- These mixed passenger/freight mainlines (`rail_type=0`, `usage=main`) are scored at the engine's generic heavy-rail class default — no SA-specific train counts are applied.

### Haramain High Speed Railway (HHR)

[HHR](https://sar.hhr.sa/) connects the two holy mosques at 300 km/h:

- **Mecca ↔ Jeddah Airport ↔ Jeddah ↔ KAEC ↔ Rabigh ↔ Medinah** (450 km, 10 stations)
- Fleet: 35 Talgo 350 trainsets
- Baseline service: ~6 trains per direction per day, surging during Umrah/Hajj
- **Tagged `highspeed=yes` in OSM** — when `maxspeed` is missing, the normalizer resolves 300 km/h. Emission still uses the standard passenger spectrum scaled by speed.

### Riyadh Metro (King Abdulaziz Public Transport Project)

Operated by [Royal Commission for Riyadh City](https://www.rcrc.gov.sa/) / [SAPTCO](https://www.saptco.com.sa/). 6 lines, 85–94 stations, 176 km, fully automated driverless Bombardier Innovia Metro 300 trains. Opened in waves December 2024 → early 2025:

| Line | Color | Terminals | Stations |
|------|-------|-----------|----------|
| Line 1 | Blue | SAB Bank ↔ Ad Dar Al Baida | 25 |
| Line 2 | Red | King Saud University ↔ King Fahad Sport City | 15 |
| Line 3 | Orange | Jeddah Road ↔ Khashm Al An | 22 |
| Line 4 | Yellow | Airport T1-2 ↔ KAFD | 9 |
| Line 5 | Green | Ministry of Education ↔ National Museum | 12 |
| Line 6 | Purple | KAFD ↔ An Naseem | 11 |

- **No GTFS published** — `rpt.sa` exposes an interactive route planner only
- Line + station geometry available as GeoJSON from [opendata.rcrc.gov.sa](https://opendata.rcrc.gov.sa/) (6 lines, 94 stations, WGS84, KSA Open Data License)
- **Tagged `railway=light_rail` in OSM** — so it is extracted into `railways.arrow` (unlike Dubai Metro, which uses `railway=subway` and is dropped by the OSM extractor) and scored at the engine's generic light-rail class default. No SA-specific Riyadh Metro frequency is applied.

### Al Mashaaer Al Mugaddassah Metro Southern Line (Mashair Metro)

Hajj-pilgrimage-only automated metro (18 km, Arafat ↔ Muzdalifah ↔ Mina), built and operated by China Railway Construction Corporation (CRCC). Runs only during the few days of Hajj each year (17 twelve-car trainsets at peak). Where the line carries OSM rail tags it is scored at the engine's generic class default; its real-world service is a handful of Hajj days, so any annualised noise contribution is negligible.

## Buildings

GHSL Built-H R2023A 100 m raster + Overture Maps Foundation (Microsoft contributed 590k Saudi buildings in Dec 2024; Bing Maps ~2.5M buildings in Nov 2022). Saudi cadastres — [GEOSA](https://gasgi.gov.sa/), [REGA (Real Estate General Authority)](https://rega.gov.sa/), [Saudi Address](https://splonline.com.sa/) — are all auth-gated or address-only (no building heights). No SA-specific building enhancement applied.

## Industrial

### Power plants — WRI GPPD (global)

WRI Global Power Plant Database via `/enrich-global` covers **90 Saudi plants, ~84 GW total**. Largest:

- **Shaiba (Rabigh)** — 6.8 GW oil
- **Ghazlan II** — 4.3 GW gas/oil (Eastern Province)
- **Rabigh IPP** — 4.3 GW
- **Hajr / Waad al-Shamal IPP** — 4.1 GW gas CCGT
- **Qurayyah CC** — 3.8 GW gas CCGT
- **Riyadh 9** — 3.6 GW
- **Jubail, Shuqaiq, Yanbu, Shuaiba, Mirfa, Duba** — large steam/CCGT plants
- **Dumat Al Jandal** — 400 MW wind (first commercial wind farm in the Arabian peninsula, operational 2021, Al Jouf region)
- **Sakaka** — 405 MW PV (first utility-scale solar, 2021, Al Jouf)

Missing from WRI (stopped updating in early 2022):
- **Sudair** — 1.5 GW PV (operational 2024, largest single solar farm in the Gulf)
- **NEOM** — 4 GW green hydrogen solar + wind under construction (Tabuk region)
- Various Vision 2030 PV/BESS additions

### Industrial facility registry (gap)

No Saudi industrial facility registry is open. The following are visible only via OSM `man_made=refinery` / `landuse=industrial` / `industrial=*`:

- **Saudi Aramco refineries** — Ras Tanura (550k bpd, the largest in the Middle East), Yanbu 400k bpd, Rabigh 400k bpd, Riyadh 120k bpd, Jeddah 78k bpd (closed 2017); JV refineries Jubail SATORP 440k bpd, SASREF Jubail 305k bpd, Yanbu YASREF 400k bpd
- **SABIC petrochemical complexes** — Jubail Industrial City (Ibn Sina, Petrokemya, ibn Zahr, Kemya, Hadeed, SADAF) and Yanbu Industrial City (YANSAB)
- **Ma'aden** — Ras Al Khair aluminum smelter + phosphate, Waad al-Shamal phosphate mine, Mahd ad Dhahab gold mine
- **MODON Industrial Cities** — 36 industrial cities, 4,000+ factories registered but not published as open shapefile

### Wind turbines — minimal

Dumat Al Jandal (400 MW) is the only commercial wind farm in Saudi Arabia as of early 2026. NEOM wind projects are under construction. Global `/enrich-global-windturbines` via OSM `power=generator` covers Dumat Al Jandal turbine positions; no national wind registry exists.

## Validation

Saudi Arabia implements noise regulation through:

- **General Authority for Meteorology and Environmental Protection (GAMEP)** — *before 2019*
- **Ministry of Environment, Water and Agriculture (MEWA)** at [mewa.gov.sa](https://www.mewa.gov.sa/) — *current*
- **National Center for Environmental Compliance (NCEC)** at [ncec.gov.sa](https://ncec.gov.sa/) — operational compliance monitoring
- **General Environmental Standards** under the Environmental Law (2020 Royal Decree)
- **Saudi Standards, Metrology and Quality Organization (SASO)** noise standards
- **Riyadh Municipality**, **Jeddah Municipality**, **Makkah Municipality** — urban noise enforcement

Notable noise zones include:

- **King Fahd Road** and **Northern Ring Road** in Riyadh — ~150,000 vehicles/day in peak sections
- **Highway 40 (Riyadh ↔ Jeddah)** — the main east-west corridor, especially Riyadh↔Ta'if and Jeddah↔Makkah urban segments
- **Dammam ↔ Riyadh ↔ Jeddah SAR freight corridor** — mixed passenger/freight
- **Haramain HHR** — elevated viaducts through Jeddah, Medinah, KAEC
- **Riyadh Metro** elevated sections (Blue Line 1 southern section, Red Line, Yellow Line airport branch)
- **King Khalid International Airport (Riyadh)**, **King Abdulaziz International Airport (Jeddah)**, **King Fahd International Airport (Dammam)** — covered by the global aircraft layer
- **Jubail Industrial City**, **Yanbu Industrial City**, **Ras Al Khair**, **Ras Tanura** refinery/petrochemical complexes — locally dominant near heavy industry
- **Mashair (Arafat/Mina)** — pilgrimage peak noise during Hajj only
