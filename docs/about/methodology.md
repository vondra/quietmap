---
title: Methodology
intro: How the map computes noise — sources, propagation physics, standards used, and where the model is honest about its limits.
nav: hidden
---

## How the map works

The map computes environmental noise in three steps:

1. **Sources emit noise** — roads, trains, planes, factories, wind turbines, and buildings all produce sound. We model each one using real data: traffic counts, flight tracks, building types, and industrial classifications from OpenStreetMap and national registries.

2. **Sound travels and fades** — noise gets quieter with distance. Hills block it, buildings screen it, forests absorb it. We simulate this physics for every source-receiver pair using ISO 9613-2 propagation with 8 octave bands.

3. **You see the result** — the map shows noise at every ~12-meter raster cell, colored from pale at the quiet end through yellow and orange to red and deep purple (very loud, 80+ dB). Each source layer is independent — toggle them to see roads alone, railways alone, or everything combined.

![quietmap.org — noise visualization](map-overview.jpg)

---

## Five noise layers

Each source is modelled independently — toggle, compare, and explore them in the UI.

### Roads

Road traffic is the dominant source of environmental noise, affecting 60–80% of exposed population in most countries. A car makes noise two ways — tyres on the road and the engine — so we model each segment with the European CNOSSOS-EU standard across 4 vehicle categories (light vehicles, medium trucks, heavy trucks, motorcycles), computing rolling + propulsion noise per octave band.

- **Data:** OpenStreetMap geometry + measured/enriched traffic counts where available; otherwise class-based defaults (see country pages)
- **Key variables:** traffic volume (AADT), vehicle mix (especially heavy vehicle share), speed, road surface
- **Impact:** Doubling traffic = +3 dB. One heavy truck ≈ 5–8 cars' worth of sound energy. Surface type shifts noise by up to 4 dB.

<details>
<summary>Technical: road emission (CNOSSOS-EU Annex II)</summary>

[CNOSSOS-EU Annex II](https://eur-lex.europa.eu/eli/dir_del/2021/1226), 4 vehicle categories (light / medium-heavy / heavy / motorcycles) per 8 octave bands (63–8000 Hz). Rolling + propulsion components combined per band; line-source density `L_W'/m = L_W + 10·log₁₀(Q/(1000·v))`. Surface correction applied to rolling: asphalt 0 / cobblestone +4 / concrete +1 / unpaved +2 dB.

**Default traffic volumes** (when no census data available):

| Road class | Total AADT | Light | Medium | Heavy | Moto | Speed | Time split |
|-----------|-----------|-------|--------|-------|------|-------|------------|
| Motorway | 30 000 | 21 600 | 2 400 | 5 700 | 300 | 100 km/h | 65/20/15% |
| Trunk | 15 000 | 11 700 | 1 200 | 1 800 | 300 | 70 km/h | 65/20/15% |
| Primary | 9 000 | 7 470 | 540 | 810 | 180 | 50 km/h | 70/18/12% |
| Secondary | 3 000 | 2 640 | 120 | 180 | 60 | 50 km/h | 70/18/12% |
| Tertiary | 800 | 720 | 26 | 38 | 16 | 50 km/h | 70/18/12% |
| Residential | 500 | 480 | 5 | 10 | 5 | 30 km/h | 70/18/12% |
| Living street | 100 | 98 | 0 | 1 | 1 | 20 km/h | 70/18/12% |
| Service / driveway | 250 | 240 | 2 | 5 | 3 | 20 km/h | 70/18/12% |
| Track | 5 | 4 | 0 | 1 | 0 | 20 km/h | 70/18/12% |
| Unclassified | 1 340 | 1 200 | 30 | 80 | 30 | 50 km/h | 70/18/12% |

Time split = day (07–19) / evening (19–23) / night (23–07). Measured AADT overrides the totals; the per-period split stays fixed by class.

**Service tree.** Below the named classes, OpenStreetMap has hundreds of millions of service roads, driveways, and farm tracks. They are all modelled (not dropped) but kept quiet: untagged `highway=track` is damped to ~0.5 veh/day, and local roads near buildings (residential, living street, service, unclassified) get a *computed* count — the "service tree" — that routes each dwelling's trips through the local street network (Dijkstra flow accumulation, from cul-de-sacs down to the nearest main road) instead of a flat guess. A dead-end carries only its own houses; a collector accumulates everything that feeds into it.

→ Full derivation, all coefficients: `engine/noise-compute/SPEC.md` §1.

</details>

### Railways

Rail noise affects fewer people than roads but at higher severity — a single freight corridor can dominate nighttime exposure for kilometres. Freight wagons with cast-iron block brakes are ~10 dB louder than disc-braked passenger stock, making the passenger/freight split critical.

- **Data:** OpenStreetMap rail geometry + GTFS timetables / passenger–freight counts where available; otherwise line-type defaults (see country pages)
- **Key variables:** train count per day, passenger vs freight split, speed
- **Impact:** Speed matters a lot: doubling train speed adds roughly 9 dB. One freight train at night can outweigh 10 daytime passenger trains in Lden.

<details>
<summary>Technical: railway emission (CNOSSOS-EU Annex IV / RMR)</summary>

[CNOSSOS-EU Annex IV](https://eur-lex.europa.eu/eli/dir_del/2021/1226) RMR methodology. Per-band rolling (speed-dependent, `30·log₁₀(v/v_ref)`) + constant traction. Line density expressed per-hour, not per-day. Four emission-coefficient sets: Passenger (disc brake, v_ref 100 km/h), Freight (cast iron, v_ref 80 km/h — ~10 dB louder), Tram (50 km/h), Light rail / DMU (80 km/h); narrow-gauge reuses light-rail, funicular reuses passenger. HSR uses the passenger spectrum scaled by speed (passenger v_max 300 km/h) — no dedicated aerodynamic model.

**Default train frequencies** (when no line counts available):

| Line type | Pass/day | Freight/day | Default speed |
|-----------|---------:|------------:|---------------|
| Main line | 80 | 20 | 80 km/h |
| Branch | 30 | 5 | 80 km/h |
| Industrial siding | 0 | 15 | 80 km/h |
| Rail, unknown usage | 40 | 10 | 80 km/h |
| Tram | 120 | 0 | 25 km/h |
| Light rail | 80 | 0 | 60 km/h |
| Narrow gauge | 10 | 0 | 40 km/h |
| Funicular | 40 | 0 | 20 km/h |

Measured counts override defaults. The day/evening/night split varies by region and by passenger vs freight — EU freight is night-heavy (≈34/11/55), since much of it runs at night. Source height 0.5 m (wheel-rail contact).

→ Full derivation, coefficients, simplifications: `engine/noise-compute/SPEC.md` §2.

</details>

### Aircraft

The aircraft layer combines two models: airborne overflights from ADS-B radar trajectories, processed through NPD (Noise-Power-Distance) profiles inspired by ECAC Doc 29, and airport ground operations (runway roll, taxi, apron movement) extracted directly from low-altitude / on-ground ADS-B trajectories with the nearest mapped aerodrome attached for identity. The map shows everything together; the popup splits aircraft into three tabs — ground paths, airborne sub-segments, and cruise hexes.

- **Data:** ADS-B trajectories from two feeds — [ADSBExchange](https://www.adsbexchange.com) samples (1st of each month, 12 days/year) for airline and other scheduled traffic, and [adsb.lol](https://adsb.lol) community feeds sampled across all 365 days for GA and helicopter traffic, whose occasional flights need the full year to be weighted honestly — plus OSM aeroway lines (runways / taxiways) and aerodrome polygons. ADS-B ground legs project onto OSM microsegments to derive per-microsegment movements.
- **Per-typecode aircraft profiles** auto-generated from EASA ANP v2.3 (Aircraft Noise and Performance database) — covers Boeing 737/747/757/767/777/787, Airbus A319/A320/A321/A330/A340/A350/A380, Embraer E-Jets, ATR, Dash 8 — plus dedicated light-GA (Cessna 172) and helicopter noise classes.
- **Limitations:** Most modern jets (737 MAX, A320neo, A321neo) have dedicated profiles from EASA ANP v2.3 + supplementary v9 sources; less-common variants fall back to a similarity-based mapping by engine type and size class. Ground ops show what ADS-B observed — no synthetic backfill, so movements outside the receiver coverage don't appear. Where no volunteer ADS-B feeder is nearby, low-altitude flights are not received at all and the map shows only high-altitude cruise noise (~20 dB) — a limit of the data source, not the model. Day/evening/night periods are derived from the segment-midpoint coordinate using an IANA timezone database (DST-aware). Atlas-scale patterns, not certified airport contouring.

<details>
<summary>Technical: aircraft layer (Doc 29 + airport ground ops)</summary>

**Airborne** [ECAC Doc 29](https://www.ecac-ceac.org/activities/environment/european-aviation-and-environment-working-group-eaeg/airmod) (4th Edition). Not certified. Per-segment SEL combines NPD lookup at slant distance + speed correction + engine-installation directivity + lateral attenuation + finite-segment correction. Auto-generated from EASA ANP v2.3 (+ v9 for modern types); unknown typecodes route to the nearest anchor by engine/size class.

**Sample anchor profiles** (Approach SEL dB at 200–25 000 ft):

| Class | Anchor | Approach SEL |
|------|---------|----|
| Narrowbody jet | B738 | 94.5, 90.4, 87.4, 84.1, 78.7, 72.4, 67.5, 62.3, 54.9, 48.5 |
| Narrowbody jet | A320 | 93.1, 89.1, 86.1, 82.9, 77.7, 71.7, 67.1, 61.9, 55.8, 49.2 |
| Regional jet | CRJ9 | 90.9, 86.7, 83.3, 79.9, 74.1, 67.4, 62.4, 56.9, 50.7, 43.9 |
| Turboprop | DH8D (Dash 8) | 88.9, 84.4, 81.1, 77.7, 71.9, 65.8, 62.3, 58.7, 55.6, 52.8 |
| Light GA | C172 (Cessna) | 85.0, 80.0, 76.0, 72.0, 65.0, 58.0, 53.0, 47.0, 41.0, 35.0 |
| Helicopter | EC35 | 99.3, 95.9, 93.5, 91.0, 86.6, 81.2, 77.4, 72.7, 66.7, 59.9 |

**Airport ground ops** — per-microsegment model on OSM aeroway geometry. Each ADS-B ground leg projects onto runway / taxiway microsegments (50 m perpendicular buffer); `ops_kind` comes from OSM `aeroway_type` where a microsegment match exists (a speed-threshold fallback covers unmatched legs). Per-event SEL anchored at 25 m, propagated through Section 3 path effects. Runway-roll departures get Doc 29's +2 dB. DBSCAN auto-discovery covers OSM-missing airfields. Movements outside the ADS-B receiver footprint don't appear (no synthetic backfill).

**Popup tabs**: *Ground* (per airport microsegment + movement kind), *Airborne* (per Stage 2A sub-segment), *Cruise* (per crossed H3-R7 hex).

**Lden** per [END 2002/49/EC](https://eur-lex.europa.eu/eli/dir/2002/49/oj/eng): day 12 h, evening 4 h +5 dB, night 8 h +10 dB.

→ Full derivation, filters, ground-ops kernel, simplifications: `engine/noise-compute/SPEC.md` §5.

</details>

### Industrial and wind turbines

Industrial noise is spatially concentrated, near-constant, and locally dominant — machines, ventilation and truck movements repeat hour after hour, and a single cement plant or wind farm can define the noise environment for kilometres. We classify each site by registry NACE sector when available (what it actually *does* — a cement works is far louder than a warehouse), otherwise by OSM industrial subtype or coarse source type, then scale by site area. The range across sectors is ~30 dB: a farm (70 dB) vs a cement plant (100 dB). Wind turbines are the exception — loudness barely changes with size, so it comes from the rated power.

- **Data:** OpenStreetMap industrial landuse + NACE codes from pollution registries (EU-wide E-PRTR, Global Power Plant Database)
- **Wind turbines:** IEC 61400-11 model, emission based on rated power (98–106.5 dB(A) Lw)
- **Formula:** `Lw = base_sector + 10 × log₁₀(area / 10,000 m²)` — area capped at 500,000 m²

<details>
<summary>Technical: industrial emission profiles (ISO 8297 + NACE)</summary>

[ISO 8297](https://www.iso.org/standard/15401.html), [CNOSSOS-EU §2.4](https://eur-lex.europa.eu/eli/dir_del/2021/1226). Reference area 10 000 m² (a 100 000 m² factory adds 10 dB to its base). Profile priority: registry `nace_4digit` → OSM subtype → coarse source type. The Base Lw values below were calibrated against the EU's [END](https://eur-lex.europa.eu/eli/dir/2002/49/oj/eng) strategic noise maps (2022 round; plus EU Directive 2000/14/EC limits and 3M Noise Navigator measurements), and the radiated total deliberately stays on that calibration: each sector's A-weighted spectrum adds a fixed +4.9 to +6.4 dB(A) on top of its Base Lw. A per-sector re-anchoring against multi-country measurements is the planned next step.

**By OSM site type** (when no registry NACE):

| Type | Base Lw | Evening | Night |
|------|---------|---------|-------|
| Generic industrial | 93 dB | -3 | -10 |
| Quarry | 99 dB | -5 | -20 |
| Farmyard | 70 dB | -5 | -20 |
| Works/factory | 94 dB | -3 | -8 |
| Wastewater plant | 89 dB | 0 | 0 (24/7) |

**By NACE sector** (when enriched with registry data):

| Sector | NACE | Base Lw | Evening | Night |
|--------|------|---------|---------|-------|
| Cement / glass / minerals | 23 | 100 dB | -2 | -4 |
| Metallurgy | 24 | 100 dB | -2 | -4 |
| Mining / quarrying | 08 | 99 dB | -8 | -20 |
| Power generation | 35 | 97 dB | -1 | -2 |
| Waste / recycling | 38 | 95 dB | -3 | -8 |
| Chemical industry | 20 | 94 dB | -2 | -4 |
| Metal fabrication | 25 | 93 dB | -5 | -10 |
| Motor vehicles | 29/30 | 93 dB | -5 | -12 |
| Wood / paper | 16/17 | 93 dB | -5 | -15 |
| Food / beverage | 10/11 | 90 dB | -5 | -12 |
| Electrical / mechanical | 27/28 | 90 dB | -5 | -12 |
| Rubber / plastics | 22 | 90 dB | -5 | -10 |
| Textiles / leather | 13-15 | 88 dB | -5 | -15 |
| Wastewater | 37 | 89 dB | 0 | 0 |
| Warehousing | 52 | 86 dB | -3 | -8 |
| Retail / logistics | 46/47 | 84 dB | -8 | -20 |
| Agriculture | 1-3 | 70 dB | -5 | -20 |

The tabulated Base Lw is a calibration anchor, not the radiated total: the site radiates Base Lw plus its sector spectrum's A-weighted sum (+4.9..+6.4 dB(A)), preserving the END strategic-noise-map calibration the bases were authored against.

Source height: 8 m (quarry), 10 m (heavy industry NACE 5/8/23/24/35), 5 m (other), hub height for wind turbines (default 105 m, tag errors clamped at 175 m).

**Wind turbines** (IEC 61400-11): published max LwA is nearly flat across ratings — 98 dB(A) (< 1 MW), 104 (1–2 MW), 105 (2–3 MW + unknown default), 106 (3–5 MW), 106.5 (≥ 5 MW); ratings above 8 MW are treated as OSM tag errors (unknown).

→ Full emission/area/height resolution chains: `engine/noise-compute/SPEC.md` §6.

</details>

### Buildings, settlements & leisure

Everyday activity makes noise: rooftop air-conditioning and refrigeration, kitchen extracts, deliveries, voices on a restaurant terrace, a school yard, a padel court. We model each building — and each open-air sports / play / hospitality area — as its own noise source, sized by **how big it is** (a bigger source is louder, the same way a bigger loudspeaker is).

This is a **custom extension, not a CNOSSOS-EU standard source**: the EU Environmental Noise Directive maps only road, rail, aircraft and large industry, and treats buildings purely as *obstacles* that block sound. Our settlement layer rests on real engineering standards (EN ISO 12354-4 façade breakout, VDI 2571 / 3770, DIN 18005, the Bavarian *Parkplatzlärmstudie*) — but it is an extension, and we say so plainly.

- **Data:** OpenStreetMap building polygons (type, height, floors, area) + leisure / sport areas
- **Model:** one **area-law** — a bigger source is louder; the same rule for a warehouse, a supermarket, a restaurant terrace and a football pitch
- **Impact:** twice as big ≈ +3 dB; a 10-floor block over its footprint ≈ +10 dB vs one floor

#### Indoor estimate at an enclosed building

The map receiver is normally a façade/outdoor receiver. When a click is
inside an enclosed Overture footprint, the popup also gives a closed-window
indoor estimate. This is a display-level use of the building envelope (the
wall-and-window reduction), not a second propagation run:

| Building type | Product reduction ΔL |
|---|---:|
| House | 30 dB |
| Office | 35 dB |
| Industrial hall | 20 dB |
| Historic building | 28 dB |
| Other building | 28 dB |

These reductions are product assumptions, not ISO or WHO class values. The
method follows the façade-to-indoor approach described by EN ISO 12354-3 and
ISO 16283-3. The popup shows the real computation as three tidy total-level
steps: **outside at the wall** → **walls & windows** → **indoors**. Source
groups, source-receiver segments, angles, and path effects remain the actual
façade values computed by the engine; only the aggregate display total uses
the reduction. A tilted/open-window variant uses the product's 15 dB context
value (WHO); occupant behaviour dominates, so the estimate is typically
uncertain by ±8–12 dB.

#### The exact maths

Each source radiates a total **sound power** `Lw` (like the wattage of a loudspeaker — how much sound it pours out *before* any travels to you):

```
Lw = 10 · log₁₀( 10^(fix/10)  +  area_m² · 10^(per_m²/10) )
```

- `fix` = a small fixed floor in dB (even a tiny shop has one humming AC unit).
- `per_m²` = how many dB each square metre adds; **it differs per category** (a restaurant kitchen radiates more per m² than a garage).
- `area_m²` = footprint **× floors** for a stacked block (flats/offices); **footprint only** for one tall volume (a warehouse, a church nave, a supermarket hall — it is not "4 floors" just because it is 12 m tall).

Every ×10 of area adds ~10 dB. **But you never hear all of `Lw`** — a big building is chopped into a 30 m grid and each piece travels to you separately, so from up close only the near wall is loud (the far corners are far away). A 114 000 m² hall has `Lw` 100 dB but lands ~44 dB at 50 m. `Lw` is the *engine*, not the *level at your window*.

> **Worked example — a shop (`fix 55, per_m² 48`).** An 80 m² *večerka* → Lw ≈ 67 dB · a 1 000 m² supermarket → ≈ 78 · a 5 000 m² hypermarket → ≈ 85. The small shop is ~18 dB quieter than the hypermarket — size finally matters.

**Sport — how a yearly average comes from a loud-while-playing level.** A court is only in use part of the day, part of the year. So:

```
yearly Lden = active Lw  −  seasonal  −  daily-duty
                            ~6 mo/yr     ~6 of 24 h in use
                             −3 dB         −6 dB         ≈ −9 dB
```

Every number above is a **stated assumption**, not a measurement — listed here so you can argue with it. (Stadiums: match days only ~25/yr → −12 dB. Indoor halls look identical to outdoor in OSM, so we treat them as outdoor — a known limitation.)

<details>
<summary>Technical: deployed building profiles + statistics</summary>

Per type from `settlement.rs::building_profile`. **Scale** = whether floors multiply the area. Evening / Night are dB offsets on the daytime level (the END +5 / +10 annoyance penalties are added separately when the three periods collapse to Lden). **Share** = fraction of all buildings worldwide in our data (sampled).

| Type | OSM tags | fix | per_m² | Scale | Eve | Night | Share |
|------|----------|-----|--------|-------|-----|-------|-------|
| Apartments | apartments, residential, `yes` | 57 | 25 | × floors | −5 | −10 | 86 % |
| House | house, detached, terrace | 57 | 22 | × floors | −5 | −8 | 8 % |
| Silent | sheds, roofs, huts, greenhouses, ruins | — | — | — | — | — | 1.7 % |
| Garage | garage, parking | 41 | 18 | × floors | −5 | −15 | 1.2 % |
| Commercial / office | commercial, office | 70 | 30 | × floors | −5 | −10 | 0.7 % |
| Warehouse / factory | warehouse, industrial, manufacture | 58 | 45 | footprint | −3 | −8 | 0.5 % |
| School | school, kindergarten | 66 | 28 | × floors | −10 | −25 | 0.4 % |
| Farm | farm, barn | 56 | 20 | footprint | −5 | −15 | 0.4 % |
| Restaurant / bar | restaurant, café, pub, bar | 68 | 50 | footprint | 0 | −5 | 0.3 % |
| Church | church, chapel, monastery | 72 | 26 | footprint | −5 | −20 | 0.2 % |
| Shop / supermarket | supermarket, convenience, mall | 55 | 48 | footprint | −2 | −2 | 0.2 % |
| Public / civic | civic, government, stadium | 62 | 25 | × floors | −8 | −20 | 0.1 % |
| Hospital | hospital, clinic | 72 | 26 | × floors | −3 | −5 | 0.1 % |
| Hotel | hotel, hostel | 58 | 22 | × floors | −2 | −10 | 0.1 % |

`building=yes` (~79 % of all polygons, no specific type) defaults to apartments. **~1.7 % are *silent*** (sheds, roofs, ruins, greenhouses — uninhabited / unheated) and emit nothing. Each building radiates from height / 2 (mid-facade) as an ISO 9613-2 point source.

**A specific structural tag wins over an amenity POI inside it** (fixed 2026-06): a `building=warehouse` or `building=stadium` with an `amenity=bar` is a warehouse / stadium, **not** a restaurant the size of the whole envelope. (This was the Strahov-Stadium-as-100 dB-restaurant bug.) Only generic envelopes (`building=yes`/`commercial`) take their type from the amenity.

</details>

<details>
<summary>Technical: leisure profiles + statistics</summary>

**Leisure uses the identical area-law** (`leisure.rs`) — sized by polygon, radiating at ~1.5 m (voices and rackets, not roof plant), no floors. `Lw @ ref` is the **annualised** year-average at the reference size; the active (loud-while-playing) anchor and its source are listed. **Share** = fraction of all leisure areas worldwide (sampled).

| Area | per_m² | ref size | Lw @ ref | active anchor (source) | Share |
|------|--------|----------|----------|------------------------|-------|
| Padel court | 58 | 200 m² | 81 dB | 90 — racket on glass (padelcreations + Higgins) | 0.1 % |
| Football pitch | 40 | 7 000 m² | 78 dB | 88 — 58 LAeq @10 m (Sport England AGP) | 47 % |
| Stadium | 40 | 7 000 m² | 78 dB | pitch + crowd, ~25 match days/yr | 0.9 % |
| Swimming pool | 50 | 400 m² | 76 dB | lido splash/voice — PROP-MEAS | 2.7 % |
| Tennis court | 50 | 260 m² | 74 dB | 84 — 58.4 dB/strike (TU München) | 13 % |
| Playground | 48 | 200 m² | 71 dB | child play — PROP-MEAS | 19 % |
| Basketball court | 42 | 420 m² | 68 dB | tennis − 6 (ball on hard court) | 7 % |
| Outdoor seating | 55 | 12 m² (bare node) | 66 dB | 71 dB/guest (Lärmfibel Biergarten) | 9 % |

A leisure node with no polygon (most `outdoor_seating=yes` points) assumes the small reference footprint, not a 50-seat beer garden. **PROP-MEAS** = no clean measured Lw exists; a conservative flagged placeholder, queued for measurement — never shown as measured.

</details>

<details>
<summary>Honest provenance — the weak spots</summary>

**Calibration.** Building per-m² is anchored to measured façade levels so the *median* building did **not** change — only size-scaling was added: Chodov shopping centre ~45 dB, Staroměstské náměstí ~43 dB. Per-family sources: factory / warehouse breakout (EN ISO 12354-4, VDI 2571, DIN 18005); retail & terrace activity (Bavarian *Parkplatzlärmstudie*, VDI 3770, LfU Biergarten); residential heat-pump floor (EU 813/2013, Daikin EN 14825); HVAC (ASHRAE Ch. 48); assessment frame (BS 4142, TA Lärm).

**What is thin — we don't pretend otherwise.**
- No measured *whole-factory* Lw vs its m² exists in the literature; the per-m² slope is planning data (DIN 18005) + breakout physics (EN 12354-4).
- The **sport annualisation** (active → yearly) has **no standard** — END/CNOSSOS does not model sport. Our duty cuts (season, hours) are reasoned assumptions, listed above so they can be checked, not derived from a norm. The nearest standard, German 18. BImSchV, rates a single venue, not a yearly map.
- Per-store refrigeration is built up from unit data; terrace-patron levels are never measured with a clean breakdown; some figures come from secondary *Gutachten*.
- Indoor sports halls are indistinguishable from outdoor courts in OSM.

→ Discretization (centroid vs interior grid), the solid-footprint tile fill, fallback chains: `engine/noise-compute/SPEC.md` §7.

</details>

---

## How sound travels

Sound gets quieter as it travels. On flat open ground, a road drops about 3 dB every time you double your distance. But the real world has hills, buildings, and forests that block sound further — and hard surfaces like asphalt that reflect it.

![The computed noise field around Prague, shown without the basemap — every filament is a road or railway, every halo a propagation calculation](noise-field-prague.jpg)

We simulate these effects for every source-receiver pair using [ISO 9613-2](https://www.iso.org/standard/74047.html) and [CNOSSOS-EU](https://eur-lex.europa.eu/eli/dir_del/2021/1226), computed per 8 octave bands (63–8000 Hz), then A-weighted.

Road, railway, industrial, settlement, and aircraft ground ops use the same propagation engine. Airborne aircraft uses NPD tables where atmospheric absorption is already included.

| Effect | What it does | Data source | Max effect |
|--------|-------------|-------------|-----------|
| Distance | Sound spreads out — 3 dB per doubling (line), 6 dB (point) | Geometry | Baseline |
| Atmosphere | Air absorbs high frequencies over long distances | ISO 9613-1 (15°C, 70% RH) | Baseline |
| Ground | Soft ground (grass) absorbs; hard ground (asphalt) reflects | ESA WorldCover land-cover → imperviousness proxy (global) refined by Copernicus IMD where sourced (currently CZ) → G-factor | ~3 dB |
| Terrain | Hills block sound via diffraction | Copernicus GLO-30 DEM (30m) | 20 dB |
| Buildings | Buildings screen sound like walls | Overture building-height raster (30m) | 20 dB/band |
| Vegetation | Forests absorb sound, especially high frequencies | ESA WorldCover 2021 | 2–12 dB/band |
| Reflections | Urban canyons bounce sound, increasing levels | Building enclosure heuristic | +3 dB |
| Weather | Downwind/inversion conditions can carry sound further | Not currently modelled | — |

**Key rule:** When a barrier (hill or building) is present, it replaces the ground effect — you get the larger of the two, not both (ISO 9613-2 §7.3.1). Vegetation attenuation is always additive.

<details>
<summary>Technical: propagation</summary>

**Total received level per band:**
```
L_received,i = L_emission,i − A_div,i − A_atm,i − max(A_ground,i, A_terrain,i + A_screen,i) − A_veg,i + A_refl + FLC
```

**Geometric divergence**: line source `A_div = 10·log₁₀(2π·d_slant)`; point source `A_div = 20·log₁₀(d_slant) + 11`.

**Atmospheric absorption** (ISO 9613-1, 15°C 70% RH, dB/km):

| 63 Hz | 125 | 250 | 500 | 1k | 2k | 4k | 8 kHz |
|-------|-----|-----|-----|-----|-----|------|-------|
| 0.1 | 0.4 | 1.0 | 1.9 | 3.7 | 8.7 | 22.0 | 58.4 |

**A-weighting** (IEC 61672-1): `[-26.2, -16.1, -8.6, -3.2, 0.0, 1.2, 1.0, -1.1]` dB.

**Diffraction**: DEM + building heights + explicit barriers merge into one composite top-profile sampled along source→receiver; diffraction computed once over the composite so a building on a hill can't double-count Fresnel. Capped at 20 dB in all cases (the multi-edge 25 dB cascade was retired 2026-06; one composite edge now). The popup splits the combined attenuation back into `terrain` + `screening` for UI breakdown, but the physics is computed together. Building reflections (§7.5) are a 0 / 1.5 / 3 dB boost keyed to local enclosure density.

**Vegetation** (ISO 9613-2:2024 A.2.2 × 0.5 Central Europe calibration):

| | 63 Hz | 125 | 250 | 500 | 1k | 2k | 4k | 8 kHz |
|--|-----|-----|-----|-----|-----|-----|-----|------|
| dB/m | 0.01 | 0.015 | 0.02 | 0.025 | 0.03 | 0.04 | 0.045 | 0.06 |
| Max | 2 | 3 | 4 | 5 | 6 | 8 | 9 | 12 dB |

× 0.5 reflects that WorldCover binary forest raster fires at ≥ 10 % tree cover but ISO defaults assume dense canopy.

**Ground**: G = 1 − IMD/100 (G = 0 hard, G = 1 soft). Per-band correction factors `[-1.5, -0.7, 1.5, 2.5, 2.0, 1.3, 0.7, 0.2]` × G.

**Favourable weather**: not currently applied. `P_FAV = 0.5` placeholder in code; no wind / inversion boost.

→ Full derivation, edge selection, Rayleigh δ\* gate, simplifications: `engine/noise-compute/SPEC.md` §3.

</details>

---

## Simplifications

This model is an engineering approximation for a continental-scale noise atlas — not a certified implementation of CNOSSOS-EU or ISO 9613-2.

| Area | Standard says | We do | Impact |
|------|-------------|-------|--------|
| Source height (roads) | CNOSSOS-EU: 0.05 m (rolling) / 0.30 m (propulsion) | 0.05 m for both | Minor — propulsion height difference negligible at atlas scale |
| Terrain profile | Professional SW: 5–10 m spacing | Adaptive 30 m spacing (8–50 points) | May miss narrow barriers (<30 m wide) |
| Aircraft type mapping | Doc 29 / ANP: aircraft-specific certified profiles + procedural steps + weights | Per-ICAO-typecode NPD profiles auto-generated from EASA ANP v2.3 (+ v9 supplement), bucketed at a fixed set of noise classes for aggregation (see SPEC §5) | ±1-2 dB for ANP-mapped types; similarity fallback for unmapped typecodes routes to closest anchor by engine/size class |
| Aircraft timing | Airport-local time and operational preprocessing | Segment midpoint → IANA timezone (tzf-rs) → DST-aware local time (chrono-tz); END default period boundaries | Global local time; only airport-local operational-preprocessing differences remain |
| Aircraft ground ops | Curated surface-movement inventories + airport-local operational data | ADS-B legs projected onto OSM aeroway microsegments (runway/taxiway); per-microsegment movement counters; DBSCAN auto-discovery for OSM-missing airfields | Near-runway levels depend on ADS-B coverage; movements outside the receiver footprint don't appear (no synthetic backfill) |
| Tile propagation | Operational studies may expose a per-effect propagation breakdown | Tiles store one combined full-propagation Lden per source layer (z12 HM3); the click popup's `traces` expose per-leg / per-sub-segment detail | No per-effect (terrain/screening/vegetation) isolation at tile resolution |
| Receiver grid | END: facade receivers (4 m height, 2 m from wall) | z12 Web-Mercator raster pixel centers (~12 m at 50°N, 4 m height) | Area average, not per-facade |
| Road corrections | CNOSSOS-EU: gradient, intersection, temperature | Not implemented | ±1–3 dB on steep/cold roads |
| Building reflections | ISO 9613-2 §7.5: image-source ray tracing | Simplified: local enclosure heuristic, 0–3 dB boost | May underestimate in complex geometries |
| Settlement noise | Not standardised (END covers road/rail/aircraft/industry only) | Unified area-law: 14 building types + leisure areas | Extension — built on EN ISO 12354-4 / VDI / DIN engineering data |
| Atmospheric conditions | Variable: temperature, humidity, wind speed | Fixed: 15°C, 70% RH; favourable-weather boost not applied | Seasonal/hourly variation not captured |

These simplifications target MAE < 3 dB against national strategic noise maps for road noise. Formal cross-country MAE validation is still pending, but an internal accuracy loop against real measurement stations is active — several hundred station-years across airport monitoring terminals (Prague, Zürich, Amsterdam, Frankfurt, San Francisco), city networks (Barcelona, Paris, Dublin), and German rail-corridor stations, with findings tracked openly in the repository.

---

## Validation

Accuracy is a continuous loop, not a one-off benchmark: measure the gap against real monitoring stations → attribute the cause → fix the data or the model → re-measure. Every confirmed gap becomes a short finding committed openly in the repository.

- **Primary reference — real measurements.** Annual per-station levels from public monitoring networks: city networks (Barcelona, Dublin), airport monitoring terminals (Prague, Zürich, Amsterdam, Frankfurt, San Francisco), and German rail-corridor stations (EBA) — several hundred station-years across five countries, and growing. Each station carries commensurability metadata (metric variant, receiver height, coordinate uncertainty) so a comparison is only made where it is honest.
- **Honest comparison.** A street microphone hears everything at once — traffic, voices, weather — so a single modelled layer is checked there only as an *upper bound* (the model must never exceed the measured total). Event-classified airport terminals and near-source geometries allow a full two-sided check.
- **Official maps as a benchmark, not a target.** The EU's END strategic noise maps cross-check our results and probe our input data — a systematic gap that tracks a data regime points at enrichment, not physics — never as a calibration target.
- **Already corrected by the loop.** A street-tram default speed and the double-counting of parallel rail tracks were each surfaced by a measurement station and fixed, then re-verified against the same stations.
- **Target:** mean absolute error < 3 dB vs national strategic maps for road noise. A formal cross-country MAE is still pending — the per-network gap tables and a layer × country uncertainty breakdown are the interim measure.
