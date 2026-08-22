---
title: Europe
intro: EU noise mapping framework — standards, directives, and methodology.
map: { center: [15, 50], zoom: 4 }
---

## Environmental Noise Directive (END)

The EU [Environmental Noise Directive 2002/49/EC](https://eur-lex.europa.eu/legal-content/EN/TXT/?uri=celex%3A32002L0049) requires member states to produce strategic noise maps for major roads, railways, airports, and cities. quietmap.org uses the same methodology but extends coverage to all sources and locations, not just those above the directive's thresholds.

## CNOSSOS-EU

The [Common Noise Assessment Methods](https://publications.jrc.ec.europa.eu/repository/handle/JRC72550) (CNOSSOS-EU) is the official EU reference method for strategic noise mapping. It defines:

- **Road emission model** — Noise power per vehicle category, speed, and road surface
- **Railway emission model** — Noise per train type, speed, track properties
- **Industrial emission model** — NACE sector-differentiated source power levels (20 NACE sector profiles + 12 OSM sub-type profiles), anchored to measured literature values, with sector codes from IRZ/E-PRTR facility registry data
- **Propagation model** — Sound attenuation through distance, ground, terrain, buildings, and atmosphere

quietmap.org implements CNOSSOS-EU emission models for road (Annex II), railway (Annex IV), and industrial sources, with ISO 9613-2 propagation. Aircraft noise uses an NPD-based approach inspired by ECAC Doc 29 (referenced by CNOSSOS-EU §2.7) but is not a certified implementation — see [methodology page](/about#aircraft) for details.

## ISO 9613-2

The propagation model follows [ISO 9613-2](https://www.iso.org/standard/61049.html) (Acoustics — Attenuation of sound during propagation outdoors), which defines octave-band calculation of:

- Geometric divergence
- Atmospheric absorption
- Ground effect
- Screening by obstacles (terrain, buildings, barriers)

## Propagation factors

Click any point on the map to see how much terrain, forest, and buildings attenuate noise there — the popup shows a read-only per-source breakdown of each path effect. The map's advanced settings can additionally display the input rasters (elevation, building heights, forest, noise barriers) as overlays.

| Factor | What it does |
|--------|-------------|
| **Geometric divergence** | Sound energy spreads over a larger area with distance |
| **Atmospheric absorption** | Air absorbs sound energy (depends on temperature and humidity) |
| **Ground effect** | Soft ground (grass, soil) absorbs more than hard surfaces (concrete, water) |
| **Terrain diffraction** | Hills and ridges can block sound — a ridge can reduce noise by 10 dB or more |
| **Building screening** | Buildings between source and receiver block and reflect sound |
| **Forest attenuation** | Dense vegetation absorbs and scatters sound energy |
| **Meteorological** | `P_FAV = 0.5` long-term homogeneous/favourable ground and diffraction mix; no local wind/inversion input |

## How far noise travels

Different sources propagate different distances. A motorway is audible much further than a local road.

| Source | Maximum distance | Why |
|--------|-----------------|-----|
| Motorway | 10 km | High speed, heavy traffic, continuous noise |
| Trunk road | 7 km | Moderate-heavy traffic |
| Primary road | 5 km | Moderate traffic |
| Secondary road | 3 km | Local traffic |
| Tertiary road | 1.6 km | Low traffic |
| Residential road | 800 m | Very low traffic |
| Railway (incl. tram) | 2–10 km, solved per line | Each line reaches to where its own free-field level falls to 25 dB — quiet branch lines stop early, busy freight corridors carry far |
| Aircraft corridor | Overhead — all altitudes (NPD extrapolated beyond 25,000 ft) | Doc 29 kernel with real lateral geometry: closest point of approach + lateral attenuation per sub-segment; 124 ANP-derived profiles in 15 noise classes (proxy profile only for unknown typecodes) |
| Industrial facility | up to 4 km | Varies by sector (NACE-differentiated) |
| Wind turbine | up to 4 km | Elevated point source (actual hub height), 98–106.5 dB |
| Settlement building | 1–2 km | Per-building area-law sound-power model, 14 OSM classes |

## WHO guidelines

The [WHO Environmental Noise Guidelines for the European Region](https://www.who.int/europe/publications/i/item/9789289053563) (2018) recommend:

| Source | Indicator | Recommended level |
|--------|-----------|-------------------|
| Road traffic | Average annual noise | Below 53 dB |
| Railway traffic | Average annual noise | Below 54 dB |
| Aircraft noise | Average annual noise | Below 45 dB |
| Wind turbines | Average annual noise | Below 45 dB |

These guidelines inform the color scale — locations above WHO thresholds appear in warmer colors.

## Noise indicators

- **Average annual noise (Lden)** — Day-evening-night weighted average over a full year. Evening noise gets a +5 dB penalty, night noise +10 dB, reflecting human sensitivity to noise during rest hours. Computed as: Lden = 10 × log₁₀((1/24) × (12 × 10^(Ld/10) + 4 × 10^((Le+5)/10) + 8 × 10^((Ln+10)/10))), where Ld is daytime (07:00-19:00), Le is evening (19:00-23:00), and Ln is nighttime (23:00-07:00).
- **Night noise (Lnight)** — Average noise during 23:00-07:00, used for sleep disturbance assessment.

## Continental enrichment

European noise data is enriched at three levels: global baseline → continental datasets → national data.

### Applied datasets

| Dataset | Coverage | Impact | Status |
|---------|----------|--------|--------|
| **EU city traffic (AADT)** | 36 cities across 16 countries | Road segments get real traffic counts instead of defaults | Applied — 335K+ segments |
| **E-PRTR industrial registry** | 32 EU/EEA countries, full NACE spectrum | Industrial sites get registry NACE sector (not just power plants) | Applied — 85,601 registered facilities (50,488 reporting year 2024) |
| **GTFS railway timetables** | ~17 European countries (18 feeds) | Railway segments get real train frequencies | Applied |
| **GPPD power plants** | ~35K plants worldwide (EU subset) | Industrial sites get NACE 35 classification | Applied — direct to industrial.arrow |
| **Copernicus IMD** | Europe-wide 10m raster | Ground effect G-factor overlay on WorldCover | Applied — in raster pipeline |

### EU city traffic

Source: "Harmonized Annual Averaged Traffic Data at Street Segment Level for European Cities" (Nature Scientific Data, 2025), CC BY 4.0. Cities: Vienna, Brno, Copenhagen, Helsinki, Paris, Grenoble, Toulouse, Lyon, Lille, Bordeaux, Rennes, Marseille, Rouen, Montpellier, Tours, Berlin, Hamburg, Dublin, Milan, Luxembourg, Amsterdam, Oslo, Lisbon, Valencia, Barcelona, Madrid, Malmö, Stockholm, Zurich, Geneva, London, Birmingham, Manchester, Glasgow, Edinburgh, Cardiff.

### GTFS railway

Train frequencies from public GTFS feeds across ~17 European countries: DELFI (DE), opentransportdata.swiss (CH), ÖBB (AT), NS (NL), Trafikverket (SE), Entur (NO), Fintraffic (FI), NMBS/SNCB (BE), SNCF + Transilien (FR), CFL (LU), Hellenic Train (GR), Pasažieru Vilciens (LV), Peatus.ee (EE), Sofia Traffic (BG), HŽ (HR), MÁV-START (HU), ŽSR (SK). Busiest Wednesday selected as reference day.

### National road & rail enrichment

Beyond the harmonized 36-city dataset, several European countries carry their own country-specific road/rail enrichment. Data source and method vary by country (published surveys, corridor-tier, network-derived tuning) — see each country page:

- **Road enrichment applied** — Czechia, Germany, Spain, France, United Kingdom, Italy, Poland, Denmark, Finland, Norway, Ireland, Russia, Turkey, Ukraine.
- **Rail enrichment applied** — Czechia, Spain, Italy, Poland, Portugal, Sweden, Denmark, Finland, Ireland, Russia, Ukraine.

### Known gaps

- **Traffic outside enriched coverage**: Roads outside the 36 cities and the national AADT countries use default AADT by road class.

## Non-EU per-country enrichment

Enriched European countries outside the EU continental dataset:

- **Serbia, Bosnia, Montenegro, North Macedonia, Albania, Kosovo** (Western Balkans 6) — GEM + corridor railway defaults. See individual pages.
- **Ukraine, Belarus, Moldova** — GEM + corridor defaults. See individual pages.
- **Iceland** — GEM (100% renewable). No railway (never had one). See [Iceland](is).
- **Andorra** ✅ — 2 GEM plants / 46 MW (FEDA hydro). Pyrenees duty-free. **29k roads (68%)**. See [Andorra](ad).
- **Monaco** ✅ — 0 GEM plants. F1 Grand Prix. **0 roads (FR overlap)**. See [Monaco](mc).
- **San Marino** ✅ — 0 GEM plants. Oldest republic (301 AD). **0 roads (IT overlap)**. See [San Marino](sm).
- **Liechtenstein** ✅ — 0 GEM plants. Double-landlocked. Banking. **0 roads (CH/AT overlap)**. See [Liechtenstein](li).

## Validation

Model validation prioritizes commensurable public monitoring measurements. Official strategic maps are cross-checks, not calibration targets; see [methodology](/about/methodology#validation).
