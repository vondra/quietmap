---
title: Thailand
intro: Noise mapping data sources for Thailand.
map: { center: [100.5, 14.0], zoom: 6 }
---

## Road traffic

### DRR Rural Roads AADT 2024 + DOH motorway/trunk defaults + Thai-tuned CNOSSOS

Thailand publishes high-quality road traffic data but most authoritative portals are TCP-blocked from non-Thai networks. The **Ministry of Transport CKAN mirror** at `datagov.mot.go.th` keeps Department of Rural Roads (DRR) data pre-loaded in its CKAN datastore, which bypasses the blocked upstream portals.

### Tier 1 — DRR Rural Roads AADT 2024

The **Department of Rural Roads (กรมทางหลวงชนบท, DRR)** publishes per-segment AADT for the rural road network with full CNOSSOS vehicle-class breakdown:

- **Source**: DRR annual traffic census via MOT CKAN datastore
- **URL (datastore dump)**: `https://datagov.mot.go.th/datastore/dump/d0675c68-510b-45e1-b865-1ce261814948?format=csv`
- **Records**: 3,415 rural segments, 3,215 with non-zero AADT
- **Columns**: `road_code, road_name, MC, SV, SVT, TB2, TB3, T4, ART3-6, BD, DRT, sum_AADT`
  - MC = motorcycles (large Thailand share)
  - SV = small vehicles (cars ≤7 seats)
  - TB2/TB3/T4 = 2/3/4-axle trucks
  - ART3-6 = articulated trucks
  - BD = buses
- **Matching**: exact OSM `ref` with Thai-prefix road codes — `นบ.3021` (Nonthaburi 3021), `นฐ.3004` (Nakhon Pathom), `กท.1001` (Bangkok), `ส.009` (bridge)
- **Top roads**:

| Road | Name | AADT (2024) | Motorcycles |
|---|---|---:|---:|
| นบ.3021 | Thanon Ratchaphruek (Nonthaburi) | 136,447 | 82,745 (61%) |
| ส.009 | Taksin Bridge | 136,447 | 82,745 |
| ส.011 | Phra Pinklao Bridge | 75,136 | 24,902 |
| ส.007 | Rama 7 Bridge | 73,391 | 29,715 |
| กท.1001 | Thanon Kanlapaphruek | 68,911 | 6,983 |

- **Result**: **370,838 OSM segments** matched by exact `ref`

### Tier 2 — DOH Motorway + national trunk highway defaults

The **Department of Highways (กรมทางหลวง, DOH)** operates the motorway (ทางหลวงพิเศษระหว่างเมือง) and trunk highway network. DOH publishes per-segment AADT as CSVs on `opendata.doh.go.th`, but that portal is TCP-blocked from our egress and the reachable MOT CKAN copy of the DOH file reports annual vehicle-km (not vehicle count), so per-section DOH AADT cannot be derived openly.

Motorways and trunk highways are therefore enriched by **numeric `ref` match to a hand-built AADT table** (e.g. Motorway 7, Highway 32), with values calibrated from DOH 2021 annual vehicle-km rankings + published corridor volumes — not ingested per-segment DOH counts:

| Highway | Name | AADT (rural) | AADT (Bangkok) |
|---|---|---:|---:|
| Motorway 7 | Bangkok ↔ Chonburi ↔ Pattaya | 120,000 | — |
| Motorway 9 | Outer Ring Road (Bangkok) | 100,000 | — |
| Motorway 34 | Bang Na ↔ Chachoengsao (Burapha Withi) | 80,000 | 120,000 |
| Motorway 35 | Dao Khanong ↔ Pak Tho (Rama II Rd) | 90,000 | 130,000 |
| Highway 32 | Bangkok ↔ Nakhon Sawan (Asian Highway 2) | 45,000 | 85,000 |
| Highway 1 | Phahonyothin (Bangkok ↔ Mae Sai) | 35,000 | 95,000 |
| Highway 2 | Mittraphap (Bangkok ↔ Nong Khai) | 30,000 | 85,000 |
| Highway 4 | Phetkasem (Bangkok ↔ Padang Besar) | 28,000 | 80,000 |
| Highway 3 | Sukhumvit (Bangkok ↔ Trat) | 25,000 | 75,000 |

**Result**: 8,966 motorway segments + 113,716 trunk segments matched.

### Tier 4 — class defaults (engine cascade)

Segments with no `ref` match above are **not stamped by this enricher**; rows still unenriched after all enrichers resolve through the engine's traffic-default cascade (city → country → continent → world). Thailand carries explicit cascade arms for classes 0–5 (local/service classes 6–12 use the generic world defaults):

| OSM class | TH rural (country default) | Bangkok (metro default) |
|---|---:|---:|
| 0 motorway | 60,000 | 90,000 |
| 1 trunk | 30,000 | 45,000 |
| 2 primary | 15,000 | 22,500 |
| 3 secondary | 6,000 | 9,000 |
| 4 tertiary | 2,500 | 3,750 |
| 5 residential | 1,200 | 1,800 |

**Thai vehicle split**: 62/10/13/**15** rural and 60/8/7/**25** Bangkok (light/medium/heavy/motorcycle). Motorcycles are the single largest vehicle class in central Bangkok and dominate rural routes too.

*A measured DRR-derived arm (M6.3) was parked because the census number-band → engine-class crosswalk proved invalid: 1xxx–5xxx sections are dominantly engine class 4, not 3, so band-median defaults biased class-3 roads low. It re-lands after class attribution via exact-ref joins.*

**Coverage**: 12M+ OSM road segments scanned across 402 Thai hexes.

## Railway

### Namtang GTFS (นามทาง)

The **Office of Transport and Traffic Policy and Planning (สนข., OTP)** under the Ministry of Transport publishes a unified daily-updated GTFS feed covering all major Thai transit and rail operators:

- **URL**: [namtang-api.otp.go.th/download/namtang-gtfs.zip](https://namtang-api.otp.go.th/download/namtang-gtfs.zip) (anonymous, 27 MB zip / 160 MB uncompressed, updated daily)
- **Agencies**: 57 total, including all Bangkok rail + national SRT
- **Routes**: 1,777 total, 198 rail-like

| Agency | Full name | Type | Count |
|---|---|---|---|
| **BTSC** | BTS Skytrain (Bangkok Mass Transit System) | Tram/LRT | 5 routes (Sukhumvit, Silom, Gold, Yellow, Pink) |
| **BEM** | Bangkok Expressway and Metro | Subway/Metro | 4 routes (Blue, Purple, Yellow, Pink) |
| **SRTET** | Airport Rail Link + SRT Red Line | Rail | 2+ routes |
| **SRT** | State Railway of Thailand | Heavy rail | 189 national routes |
| BMTA | Bangkok Mass Transit Authority (buses) | Bus | 211 routes (excluded from rail enrichment) |

**Busiest rail/transit stops in Thailand (from GTFS 2024-12-31 Wednesday)**:

| Stop | Operator | Trains/day |
|---|---|---:|
| **MRT Tao Poon** (เตาปูน) | MRT Blue/Purple interchange | **1,595** ⚠️ |
| **MRT Tha Phra** (ท่าพระ) | MRT Blue | 1,269 ⚠️ |
| **MRT Bangwa** (บางหว้า) | MRT Blue | 1,134 ⚠️ |
| **BTS Siam** (สยาม) | BTS Sukhumvit/Silom interchange | 894 ✓ |
| **BTS Mo Chit** (หมอชิต) | BTS Sukhumvit | 844 ✓ |
| MRT Lak Song / Bang Khae / Phasi Charoen | MRT Blue | 756 each ⚠️ |

⚠️ = MRT stops parsed but NOT in noise map (subway bug, see below)
✓ = BTS stops correctly matched to OSM `railway=light_rail` segments

### Critical pipeline limitation: MRT Bangkok subway NOT extracted

**Bangkok MRT Blue + Purple + Yellow + Pink lines are all tagged `railway=subway` in OSM.** The pipeline's OSM extractor (`engine/osm-extract/src/classify/ways.rs:28-30`) only accepts `rail | tram | light_rail | narrow_gauge | funicular`, so **all 4 MRT lines (~120 km) are missing from `railways.arrow`**.

The Namtang GTFS parser correctly processes MRT frequency data (1,595 trains/day at Tao Poon), but there are no matching OSM segments to write to.

This is the same bug affecting Dubai Metro, Taipei Metro, Kaohsiung Metro, Seoul Metro, Singapore MRT, Tokyo Metro, Hong Kong MTR, and Mexico City Metro. Adding `"subway"` to the extractor accept list is a single-line fix that would unlock all 9+ Asian metro systems simultaneously.

### State Railway of Thailand (SRT) — national rail

SRT operates the 4,000+ km national rail network (mostly 1000mm meter gauge):

- **Northern Line**: Bangkok (Krung Thep Aphiwat) ↔ Lop Buri ↔ Ayutthaya ↔ Lampang ↔ Chiang Mai (751 km)
- **Northeastern Line**: Bangkok ↔ Nakhon Ratchasima ↔ Udon Thani ↔ Nong Khai (621 km; connects to Laos-China Railway at Vientiane)
- **Eastern Line**: Bangkok ↔ Aranyaprathet (255 km; Cambodia border)
- **Southern Line**: Bangkok ↔ Hua Hin ↔ Chumphon ↔ Surat Thani ↔ Hat Yai ↔ Padang Besar (Malaysia) + Sungai Kolok (945 km)
- **Maeklong Line**: Wongwian Yai ↔ Samut Songkhram (33 km, famous market-track)

For SRT segments inside Bangkok hexes, GTFS stop frequencies are applied via spatial match. Long-distance rural SRT segments fall back to CNOSSOS defaults: 20 passenger + 10 freight trains/day on mainlines, 6+4 on branches.

## Buildings

GHSL Built-H R2023A 100 m global raster + Overture Maps Foundation building footprints. **GISTDA (Geo-Informatics and Space Technology Development Agency)** at [gistda.or.th](https://www.gistda.or.th/) and **Department of Lands (DOL)** cadastre are auth-gated. HOTOSM Thailand building extract is just OSM-derivative. No Thailand-specific building enhancement applied.

## Industrial

### Power plants — WRI GPPD

WRI Global Power Plant Database via `/enrich-global` covers **196 Thai power plants**. Major noise sources:

- **Coal (lignite)**: Mae Moh (2.4 GW, Lampang — largest fossil plant in Thailand)
- **Gas CCGT**: Bang Pakong (3.67 GW, Chachoengsao), Ratchaburi (3.65 GW), Wang Noi (2.1 GW), South Bangkok (1.1 GW), North Bangkok
- **Oil/gas**: Krabi (0.96 GW)
- **Hydroelectric**: Bhumibol (780 MW, Tak), Sirikit (500 MW, Uttaradit), Lam Takhong pumped storage (1 GW, Nakhon Ratchasima), Pak Mun (136 MW, Ubon Ratchathani)
- **Wind**: Huai Bong / Lam Takhong (~200 MW) — Nakhon Ratchasima (one of Thailand's first utility-scale wind farms), Theparak Phatthana, West Huai Bong

### Industrial registry (gap)

- **Department of Industrial Works (กรมโรงงานอุตสาหกรรม, DIW)** at [diw.go.th](https://www.diw.go.th/) maintains a ~100k-factory registry with address + industry category. The `api.diw.go.th` REST API requires OAuth2 registration — not anonymous.
- **Industrial Estate Authority of Thailand (IEAT, การนิคมอุตสาหกรรม)** at [ieat.go.th](https://www.ieat.go.th/) operates 68 industrial estates including **Map Ta Phut** (Rayong — major petrochemical), **Eastern Seaboard** (Rayong/Chonburi), **Laem Chabang** (Chonburi — port + industrial), **Pinthong**, **Bangpoo**, **Lat Krabang**, **Hemaraj**. No machine-readable open data — all visible only as OSM `landuse=industrial` polygons.
- **PTT / Siam Cement Group (SCG) / Thai Oil** — no open facility registries.

No NACE sector enrichment is applied to Thai industrial sites. Map Ta Phut, Laem Chabang, Bang Pu, and Eastern Economic Corridor facilities use the generic OSM industrial base profile with no NACE override.

## Validation

Thailand implements noise regulation via:

- **Pollution Control Department (PCD, กรมควบคุมมลพิษ)** at [pcd.go.th](https://www.pcd.go.th/) — ambient noise standards, monitoring stations
- **Ministry of Natural Resources and Environment (MNRE, กระทรวงทรัพยากรธรรมชาติและสิ่งแวดล้อม)**
- **Environmental Quality Standards Act 1992** (พระราชบัญญัติส่งเสริมและรักษาคุณภาพสิ่งแวดล้อมแห่งชาติ)
- **Notification of PCD re Noise Standards** — 70 dBA day / 60 dBA night ambient limits

Notable noise zones include:

- **Motorway 7 (Bangkok↔Chonburi↔Pattaya)** — one of the busiest motorways in Southeast Asia
- **Highway 34 (Bang Na↔Chachoengsao) / Burapha Withi** — elevated expressway along the Eastern Seaboard industrial corridor
- **Highway 35 (Rama II Road)** — Dao Khanong↔Pak Tho southbound, chronic congestion
- **Bangkok inner city** — Sukhumvit, Phahonyothin, Phetkasem arterials with massive motorcycle volumes
- **Airport Rail Link elevated viaduct** — Suvarnabhumi↔Phaya Thai
- **BTS Skytrain elevated lines** — Sukhumvit (Mo Chit↔Kheha), Silom (National Stadium↔Bang Wa), Gold Line (Krung Thon Buri↔Khlong San)
- **SRT Hua Lamphong / Krung Thep Aphiwat central station corridors** — national rail convergence
- **Map Ta Phut industrial estate (Rayong)** — petrochemical + refinery complex with documented community complaints
- **Suvarnabhumi (VTBS) and Don Mueang (VTBD) airports** — covered by the global aircraft layer
