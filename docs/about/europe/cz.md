---
title: Czech Republic
intro: Noise mapping data sources and validation for Czechia.
map: { center: [15.5, 49.8], zoom: 7 }
---

## Road traffic data

- **[Czech Road and Motorway Directorate (ŘSD)](https://www.rsd.cz/)** — [National traffic census](https://scitani.rsd.cz/) (Celostátní sčítání dopravy) with vehicle counts, speed, and heavy vehicle share for all classified roads (motorways, class I, II, III)
- **OpenStreetMap** — Road geometry, surface type, and speed limits
- Traffic volumes are assigned to road segments using ŘSD census points and interpolation

## Railway data

- **[CIS JR (Centrální informační systém jízdních řádů)](https://portal.cisjr.cz/pub/draha/celostatni/szdc/2026/)** — JR2026.zip national timetable: 13,252 train XML definitions, 8,556 trains running on a typical weekday, parsed into 6,742 station-pair segments (173,689 total passenger train movements per day)
- **Matching**: CZPTT station codes matched to OSM railway stations by name, adjacent station-pair segments mapped to OSM railway geometry by GPS triangulation
- **Result**: 528,123 Czech railway segments enriched with real passenger train counts (34.7% coverage on segments in matched hexes)
- **Busiest**: Praha hl.n. ↔ Pha hl.n. Lc105-102 at 276 trains/day, Brno hl.n. přednádr. ↔ Brno hl.n. at 246/day
- **[SŽ maximum line speeds](https://provoz.spravazeleznic.cz/portal/Show.aspx?path=/Data/Mapy/rychlosti.pdf)** — "Největší traťové rychlosti" map, used alongside OSM `maxspeed` tags
- **Speed-dependent emission** using CNOSSOS-EU Annex IV / RMR: `Lw'/m = Lw0 + 10·log₁₀(Q / (T·1000·v)) + 30·log₁₀(v/v_ref)` where `Q` = trains in the period (mainline passenger counts split 70/20/10 day/evening/night, trams 70/25/5; freight is night-heavy at ≈34/11/55), `T` = period hours, `v` = line speed in km/h. See `engine/noise-compute/SPEC.md §3.2` for the current contract.
- Typical Czech corridor speeds: I. corridor (Praha–Brno) up to 160 km/h, regional lines 80–100 km/h, tram 25 km/h (enrichment to measured line speeds is a follow-up)
- Vehicle mapping: four emission-coefficient families — passenger (disc-braked, v_ref 100 km/h), freight (cast-iron block brakes, v_ref 80 km/h, ~10 dB louder), tram (v_ref 50 km/h), and light rail (v_ref 80 km/h)

### Freight data gap

The CIS JR JR2026.zip dataset contains **passenger trains only** (all 13,252 files are `PA_` prefix = passenger). Freight timetables (GVD) are managed internally by Správa železnic and **not publicly distributed** as machine-readable data. Only aggregated annual statistics are available in PDF form (Statistická ročenka SŽ).

For now, `trains_freight` remains 0 in the enriched data. CNOSSOS-EU defaults are applied for major freight corridors (Děčín–Praha–Břeclav E65, Praha–Plzeň–Cheb E55, Brno–Přerov–Ostrava E30). The Czech freight fleet remains predominantly block-braked wagons, matching the freight coefficient family.

## Industrial data

- **OpenStreetMap** — Industrial/commercial landuse polygons with `industrial=*` sub-tags (factory, warehouse, sawmill, scrap_yard, wastewater_plant, etc.)
- **E-PRTR (European Pollutant Release and Transfer Register)** — supplies 2-digit NACE sector codes for Czech industrial complexes (CZ is an E-PRTR reporter), spatially joined to OSM industrial sites within 2 km via the continental industrial pass (`enrich-global-industrial.ts`). GPPD (power plants, NACE 35) and the GEM steel/cement/coal-mine trackers add coverage via `/enrich-global`.
- **[IRZ (Integrovaný registr znečišťování)](https://www.irz.cz/)** — the Czech national pollution register (ČHMÚ) is registered as a higher-priority national source but is not yet ingested; Czech sector codes currently come from E-PRTR.
- **[SHM 2022 industrial contours](https://geoportal.mzcr.cz/server/rest/services/SHM2022/INSPIRE/MapServer)** — Official industrial noise contours in 6 agglomerations (Praha, Brno, Ostrava, Plzeň, Olomouc, Liberec), used to cross-check input coverage and methodology

## Wind turbines

- **OpenStreetMap** — ~260 wind turbines in Czech Republic with hub height, rotor diameter, and rated power metadata
- Emission model: literature-based Lw by rated power class (IEC 61400-11)

## Aircraft data

- **[adsb.lol](https://adsb.lol)** — Historical ADS-B trajectories over Czech airspace
- Flight paths, altitudes, and aircraft types extracted for noise computation

## Terrain elevation

- **[Copernicus GLO-30 DEM](https://spacedata.copernicus.eu/collections/copernicus-digital-elevation-model)** — 30 m global DEM from TanDEM-X (<4 m LE90 accuracy), with [SRTM](https://www.usgs.gov/centers/eros/science/usgs-eros-archive-digital-elevation-shuttle-radar-topography-mission-srtm-1) as fallback
- Critical for accurate terrain diffraction — Czech landscape has many valleys and ridges that significantly affect noise propagation

## Building heights (screening)

- **[IPR Praha — Relativní výšky budov](https://opendata.geoportalpraha.cz/maps/ad9aca20e9c042d2b52eb31ff18961b6)** (CC BY) — Prague's photogrammetric building model minus terrain at 1 m resolution; sampled per building footprint into measured screening heights for the whole city
- **[GHS-BUILT-H ANBH](https://human-settlement.emergency.copernicus.eu/ghs_buH2023.php)** (JRC, CC BY 4.0, epoch 2018) — global 100 m average building height; replaces the flat 8 m default wherever no better height exists
- **OpenStreetMap / RÚIAN** — mapped heights and floor counts (floors × 3 m) where present
- Building height controls barrier diffraction: raising a courtyard block from a wrong 3–8 m to its real ~20 m deepens the acoustic shadow behind it by several dB

## Noise barriers

- **SHM barrier database** — Known noise wall locations along major roads and railways
- **OpenStreetMap** — Additional barrier data from community mapping
- Barriers can reduce noise by 5–15 dB depending on height and position

## Reference measurements and validation

- **[Strategic Noise Maps (SHM)](https://shm.env.cz/) / [CENIA](https://www.cenia.cz/)** — Official strategic noise maps used to explain deviations in input coverage and methodology, never as a calibration target
- **[Prague Geoportal](https://atlas.geoportalpraha.cz/)** — Prague noise maps with layers:
  - Noise level — day (6:00–22:00) and night (22:00–6:00) per Czech national definition (differs from END standard 07–19/19–23/23–07; quietmap.org uses the END periods for its own Lden calculation)
  - Strategic noise map 2022 (SHM) — Ldvn bands (day) and Ln bands (night)
  - Useful for per-street cross-checks in Prague

## Real estate — in preparation

Land plots and houses shown directly on the map, each with the computed Lden
at its location and a noise slider to hide listings above your threshold. We
are preparing data partnerships with Czech listing portals; the feature itself
already works end-to-end.
