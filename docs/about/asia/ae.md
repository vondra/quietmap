---
title: United Arab Emirates
intro: Noise mapping data sources for the United Arab Emirates.
map: { center: [54.5, 24.5], zoom: 7 }
---

## Road traffic

### No per-segment AADT in open form

The UAE publishes aggregate traffic statistics but **no per-segment AADT dataset**:

- **Dubai Pulse / data.dubai** at [dubaipulse.gov.ae](https://www.dubaipulse.gov.ae/) — 17 "Roads and Cars" datasets exist (taxi drivers, traffic incidents, parking inventory) but no traffic volume or road-network AADT. The new `data.dubai` portal is behind an F5 BIG-IP WAF that rejects programmatic requests.
- **Bayanat Abu Dhabi Open Data** at [bayanat.ae](https://data.bayanat.ae/) — geo-fenced / firewall-blocked from non-UAE IPs. Traffic records limited to accident counts by year/emirate.
- **Abu Dhabi Spatial Data Infrastructure** at [sdi.gov.abudhabi](https://sdi.gov.abudhabi/) — ArcGIS REST root is anonymous but all road-network FeatureServers return HTTP 404 (auth-gated).
- **RTA Dubai, DMT Abu Dhabi, SRTA, Ministry of Energy and Infrastructure** — publish annual statistics as PDFs only.

UAE roads currently use OSM `maxspeed` + CNOSSOS class defaults. Sheikh Zayed Road (E11) actual traffic is ~220,000 vehicles/day in peak sections (Dubai Municipality reports) vs the 21,600 motorway default — so Dubai↔Abu Dhabi corridor noise is systematically under-estimated.

## Railway

### Dubai RTA unified GTFS

The [Dubai Roads & Transport Authority (RTA)](https://www.rta.ae/) publishes a unified GTFS feed via [Dubai Pulse](https://www.dubaipulse.gov.ae/) containing Dubai Metro (3 routes), Dubai Tram (1 route), Water Bus (15 routes), and 163 bus routes — distributed as a 10.4 MB 7z archive.

- **Source**: [Dubai Pulse CKAN direct download](https://www.dubaipulse.gov.ae/dataset/73765e8f-e8c4-443c-9687-288072ed9d12/resource/11515bd3-bdba-466f-ab65-f057bd123ab5/download/gtfs.7z) (anonymous, browser user-agent required)
- **Operators**:
  - **Dubai Metro Red Line** (Rashidiya ↔ UAE Exchange, 52 km, 33 stations) — 383 Wed trips
  - **Dubai Metro Red Line extension "Route 2020"** (Nakheel Harbour ↔ Expo 2020, 15 km, 7 stations) — 276 Wed trips
  - **Dubai Metro Green Line** (Etisalat ↔ Creek, 22 km, 20 stations) — 407 Wed trips
  - **Dubai Tram T1** (Al Sufouh, Jumeirah Beach Residence ↔ Dubai Marina, 11 stops, 11 km) — 278 Wed trips
- **Result**: 372 Dubai Tram segments enriched from GTFS + 9,519 further segments via CNOSSOS class defaults (Etihad Rail, Yas Island APM, Palm Monorail, theme park rides, Ruwais sidings) = 9,891 / 11,366 (87%) across 27 hexes
- **License**: Dubai Pulse open data terms

### Critical pipeline limitation: Dubai Metro missing

Dubai Metro's Red Line (52 km) + Route 2020 extension (15 km) + Green Line (22 km) and all ~50 stations are tagged `railway=subway` in OpenStreetMap. The pipeline's OSM extractor only accepts `rail | tram | light_rail | narrow_gauge | funicular`, so **Dubai Metro is NOT in the railway enrichment**. The GTFS frequency data (654 trains/day at each busy station like Emirates Towers, Burj Khalifa/Dubai Mall, Mall of the Emirates) is parsed correctly but has no matching OSM geometry to write to.

This is the same pipeline bug affecting Taipei Metro, Kaohsiung Metro, Seoul Metro, Singapore MRT, Tokyo Metro, Hong Kong MTR, and Mexico City Metro. Adding `subway` to the extractor accept list would unlock all Asian metro systems at once.

### Etihad Rail

[Etihad Rail](https://www.etihadrail.ae/) operates the UAE federal freight railway. Stage 1 (Ghuweifat ↔ Habshan ↔ Ruwais ↔ Fujairah, ~1,200 km) has been operational since 2023. Stage 2 passenger service launched in phases in 2026. No open GTFS is published — CNOSSOS class defaults applied:

| rail_type | usage | trains/day | Use case |
|---|---|---|---|
| 0 (rail) | 0 (main) | 40 freight + 5 passenger | Etihad Rail Ghuweifat↔Fujairah mainline |
| 0 (rail) | 1 (branch) | 15 freight + 5 passenger | Jebel Ali spur, Mussafah spur, other branches |
| 0 (rail) | 2 (industrial) | 25 freight | Ruwais/Habshan refinery sidings (117 segments) |
| 1 (tram) | * | 200 (fallback) or 278 (GTFS) | Dubai Tram T1 |
| 2 (light_rail) | * | 200 | Abu Dhabi Yas Island People Mover, airport APMs |
| 3 (narrow_gauge) | * | 40 | Palm Jumeirah Monorail, Ferrari World / Warner Bros / Motiongate / IMG theme park rides |

## Buildings

GHSL Built-H R2023A 100 m global raster + Overture Maps Foundation building footprints (already in `/enrich-global`). Dubai's GeoDubai/Makani cadastre is municipality-gated and Abu Dhabi 3D City Model (on the Abu Dhabi SDI ArcGIS) is I3S mesh format — not worth custom extraction compared to Overture.

## Industrial

### Power plants — WRI GPPD

The WRI Global Power Plant Database via `/enrich-global` covers the UAE's major facilities:

- **Nuclear**: Barakah — 4 × 1400 MW APR-1400 units at Al Dhafra (UAE's only civilian nuclear plant; first commercial reactor in the Arab world; all 4 units fully operational since March 2024)
- **Gas CCGT + desalination**: Jebel Ali (Dubai), Taweelah A/B/C (Abu Dhabi), Shuweihat S1/S2/S3 (Western Region), Fujairah F1/F2, Mirfa
- **Solar**: Mohammed bin Rashid Al Maktoum Solar Park (Dubai, 5 GW target by 2030 — one of the world's largest single-site solar parks), Al Dhafra Solar PV (2 GW, operational 2023), Noor Abu Dhabi (1.2 GW PV, operational 2019)
- **Wind**: essentially zero (only Sir Bani Yas Island ~0.85 MW demonstration turbine)

### Industrial registry (gap)

- **Ministry of Climate Change and Environment (MoCCAE)** publishes a [National Air Emissions Inventory 2017 PDF](https://www.moccae.gov.ae/) but no machine-readable facility registry
- **ADNOC, Dubai Petroleum, Jebel Ali Free Zone** — no open asset maps. Ruwais refinery, Habshan, Das Island, Jebel Ali Free Zone facilities visible only in OSM tags (no NACE sector codes)
- **E-PRTR / UNEP PRTR** — UAE is not a party, no public pollutant-release register

UAE industrial facilities currently use the generic OSM industrial base profile with no NACE override. Major noise hotspots (Ruwais refinery complex, Jebel Ali tank farm, Taweelah power/desal) are tagged but not NACE-classified.

## Validation

The UAE implements noise regulation at the federal and emirate levels:

- **Federal**: MoCCAE under the Federal Law No. 24 of 1999 on Environmental Protection
- **Abu Dhabi**: Environment Agency — Abu Dhabi (EAD) — ambient noise standards in the Abu Dhabi Environment, Health & Safety Management System (AD-EHSMS)
- **Dubai**: Dubai Municipality Environment Department — Administrative Decision No. 154 of 2018 on Environmental Performance
- **Sharjah**: Sharjah City Municipality — Environmental Pollution Control Regulation

Notable noise zones include:

- **Sheikh Zayed Road (E11)** through Dubai — one of the heaviest-traffic roads in the Middle East (~220,000 vehicles/day in peak Dubai↔Jebel Ali sections)
- **Dubai Metro Red Line** — elevated viaduct for most of its length (Jebel Ali ↔ Ibn Battuta ↔ Dubai Marina ↔ Mall of the Emirates ↔ Burj Khalifa ↔ Union ↔ Rashidiya; Route 2020 extension to Expo 2020 site)
- **Al Maktoum International Airport (DWC)** in Dubai South and **Abu Dhabi International Airport (AUH)** — ADS-B covered by the global aircraft layer
- **Ruwais industrial complex** (ADNOC refinery, Borouge petrochemicals, Etihad Rail terminal) — Western Region
- **Jebel Ali Free Zone & Port** — the world's largest man-made port and a major industrial cluster in Dubai South
