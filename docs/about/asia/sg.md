---
title: Singapore
intro: Noise mapping data sources for Singapore.
map: { center: [103.85, 1.35], zoom: 11 }
---

## Overview

Singapore is a tiny city-state (~735 km², 2 H3R4 hexes). Despite high open-data maturity in aggregate, **per-segment noise enrichment data is limited** because most LTA data requires API key registration and government portals are Cloudflare-protected.

## Road traffic

Singapore publishes **no per-segment AADT in open form**:

- [data.gov.sg](https://data.gov.sg/) only has the "Average Daily Traffic Volume Entering the City" dataset — 1 CSV row per year (2004-2023 aggregate). Not per-segment.
- **LTA DataMall** Traffic Speed Bands API requires a free API key + is Cloudflare-protected (curl returns 403).
- LTA Land Transport Statistics is published as annual PDF (national vehicle totals only).
- LTA GIS datasets at `datamall.lta.gov.sg/content/dam/datamall/datasets/Geospatial/*.zip` are Cloudflare-protected.

Singapore roads currently use OSM `maxspeed` + class defaults.

## Railway

### Critical pipeline limitation

The pipeline's OSM extractor (`engine/osm-extract/src/classify/ways.rs`) only accepts railway tags `rail | tram | light_rail | narrow_gauge | funicular`. **Singapore's MRT lines tagged as `railway=subway` in OSM are NOT extracted into railways.arrow** — the same bug affects Korea, Japan, Hong Kong, and many other Asian countries.

Singapore's central hex has only 195 rail segments (mostly LRT loops at Bukit Panjang, Sengkang, Punggol), MISSING the entire MRT 6-line network:

- **NSL (North-South Line)** — 27 stations, 45 km
- **EWL (East-West Line)** — 35 stations, 57 km
- **NEL (North East Line)** — 16 stations, 20 km
- **CCL (Circle Line)** — 30 stations, 35 km
- **DTL (Downtown Line)** — 34 stations, 42 km
- **TEL (Thomson-East Coast Line)** — 32 stations, 43 km

Adding `subway` to the railway accept list and re-extracting OSM would unlock the entire Singapore MRT network.

### GTFS

LTA publishes static + realtime GTFS via DataMall but **requires free API key registration**. Direct downloads are Cloudflare-protected. The unofficial 2020 fallback at [github.com/yinshanyang/singapore-gtfs](https://github.com/yinshanyang/singapore-gtfs) is too stale (predates TEL line).

## Buildings

GHSL Built-H R2023A 100 m global raster + Overture Maps Foundation building footprints (already in `/enrich-global`). Singapore has the **best per-building open data of any Asian country**:

- **HDB 3D Buildings** at [github.com/ualsg/hdb3d-data](https://github.com/ualsg/hdb3d-data) — 91 MB CityJSON, ~12,000 HDB residential blocks with per-building heights, CC BY-SA 4.0
- **URA Master Plan 2019 Building Layer** (data.gov.sg `d_e8e3249d`) — 52 MB GeoJSON, ~130,000 building footprints (no heights), Singapore Open Data Licence v1.0

Combining HDB 3D heights with URA MP2019 footprints would dramatically improve Singapore building screening, but the CityJSON LOD1 parsing + dual-source merging is non-trivial. Not implemented in this enrichment pass — would need a dedicated `enrich-buildings-sg.ts` script.

## Industrial

### Power plants — GPPD

WRI Global Power Plant Database via `/enrich-global` covers Singapore's gas-fired electricity generation:

- **Senoko Energy** — 3,300 MW gas (Singapore's largest)
- **Tuas Power** — 2,670 MW gas
- **YTL PowerSeraya** — gas
- **Keppel Merlimau** — gas + cogeneration
- **Tembusu Multi-Utilities** — gas + waste-to-energy

### NEA (gap)

**NEA (National Environment Agency)** publishes only aggregate air quality station readings (PM2.5, SO2, NO2). No facility-level PRTR-style emissions inventory. Singapore is not in E-PRTR (EU-only).

### Wind turbines

Singapore has **no commercial wind farms** due to low wind speeds and dense urban land use. Skipped.

## Validation

Singapore's NEA publishes:

- **National Environment Agency Noise Standards** — under the Environmental Protection and Management Act
- **Building noise insulation standards** via BCA (Building and Construction Authority)
- **Aircraft noise contours** at Changi Airport published by CAAS

The Singapore MRT operates underground for most lines and the LRT runs at street level — both contribute to urban rail noise. Without GTFS frequencies, the noise model uses OSM rail tags with default operator-class assumptions.
