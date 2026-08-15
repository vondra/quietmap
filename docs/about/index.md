---
title: quietmap.org
intro: Find your quiet place. A world atlas of environmental noise — roads, railways, aircraft, and industry.
map: { center: [15, 30], zoom: 2 }
---

## Mission

**Make noise visible. Make quiet possible.**

quietmap.org shows how loud the world really is — and helps you find the quiet.

1. **Find quiet places** — search any address, explore the map, discover where to live, work, or relax without noise
2. **Understand noise** — see which sources contribute (roads, railways, aircraft, industry) and how terrain, buildings, and forests reduce it
3. **Track change over time** — regular updates make noise measurable, so communities and governments can see whether things are getting quieter

Human-made noise is not the same as natural sound. A forest at 50 dB with birdsong feels quiet. A road at 50 dB with traffic feels loud. quietmap.org measures environmental noise from human sources — transport, industry, and urban activity — not nature.

→ **[What's new](/about/news)** — recent improvements and what we are working on.

## How the map works

Three steps:

1. **Sources emit noise.** Roads, trains, planes, factories and buildings — each modelled from real data: traffic counts, flight tracks, registries.
2. **Sound travels and fades.** Hills block it, buildings screen it, forests absorb it — simulated with ISO 9613-2 physics.
3. **You see the result** on a ~12-meter raster, from pale (quiet) through yellow and orange to deep purple (80+ dB).

Each of the five source layers — roads, railways, aircraft, industrial, buildings — is modelled independently and toggles on its own in the map.

![quietmap.org — noise visualization](map-overview.jpg)

→ **[Read the full methodology](/about/methodology)** — per-layer emission standards (CNOSSOS-EU, Doc 29, IEC 61400-11), the propagation physics, where the model simplifies vs the standards, and the ongoing accuracy validation against real measurement stations.

## Click anywhere

Every point on the map can explain itself. Click, and a panel shows the total Lden at that spot with two views:

**Noise sources** groups everything audible there the way you'd name it — a road as one row, a factory, "aircraft — airborne" — sorted by how much each contributes. Open a row and you see the source's data (speed, traffic and vehicle mix, surface, trains per day…) and its **sound path** to your point: how much the distance, air, ground, terrain, buildings and vegetation each took off, in dB. Underlined terms explain themselves on hover; aircraft rows link to the actual flight traces.

**Segments** is the raw computation: the individual pieces the model actually sums — a few dozen meters of road each, single flights, single buildings. Filter by kind, open any piece, and you get its emission inputs, a terrain profile of the exact path from that piece to your point (elevation, buildings, forest, ground hardness), the attenuation table, day/evening/night levels — and what-if toggles: what would this be with no terrain in the way, no vegetation, free field?

<p>
<img src="popup-source-detail.png" alt="Noise sources — a road with its sound path breakdown" style="display:inline-block;width:300px;max-width:48%;vertical-align:top;margin:0 12px 0 0">
<img src="popup-segment.png" alt="Segments — one road segment with the terrain profile" style="display:inline-block;width:300px;max-width:48%;vertical-align:top;margin:0">
</p>

Nothing on the map is a black box — if a number surprises you, two clicks show where it came from.

## Defaults and enrichment

Each layer's [methodology](/about/methodology) page lists **fallback defaults** — what we assume when no measured data exists. Where real data is available it overrides them, resolved through a four-tier cascade: **city → country → continent → world**. A place with a local traffic survey uses it; otherwise it inherits its country's value, then its continent's, then a global default.

**Enrichment is class-aware.** A measured motorway count is matched only to motorway-class segments — a residential street never inherits a neighbouring highway's traffic, and a tram siding never inherits a mainline's train count. Coverage today (and growing):

- **Roads** — 53 countries with national traffic data (US HPMS, EU 36-city harmonized AADT, national surveys), plus the global service-tree estimate for minor roads.
- **Railways** — ~50 countries from GTFS passenger timetables + national freight-corridor estimates, family-aware (tram / siding / mainline kept separate).
- **Industrial** — ~124 countries with industrial enrichment: the EU-wide E-PRTR pollution registry (~85,600 registered facilities, 50,488 reporting year 2024), the Global Power Plant Database, and national wind-turbine and power-plant registries; wind turbines from a global turbine inventory.

Everything else falls back to the class-defaults. Each country page lists exactly which extra data its country uses — explore by region:

<!-- REGION_CHILDREN -->

## How fresh is the data

The current map is the **2026 dataset** — one worldwide computation generation, built from:

- **OpenStreetMap** — planet extract from May 2026 (roads, railways, buildings, industrial sites, airports)
- **Airline traffic** — [ADSBExchange](https://www.adsbexchange.com/) samples: the 1st of every month, July 2025 – June 2026 (12 days)
- **General aviation & helicopters** — [adsb.lol](https://adsb.lol/) community feeds: every day from 2 June 2025 through 1 June 2026 (364 days — one day was never published upstream). A full year of days, aligned with the airline window, because occasional flights need a whole year to be weighted honestly; our archive already spans 2024–2026 for future datasets
- **Traffic counts & registries** — the latest published national data at build time (per-country details on the country pages)

The plan is one frozen dataset per year: when the 2027 map arrives, you'll be able to compare — did your street get quieter?

## What you see on the map

### The noise indicator: Lden

The map shows **Lden** (day-evening-night level), the European standard from [END 2002/49/EC](https://eur-lex.europa.eu/eli/dir/2002/49/oj/eng). It weights evening noise +5 dB and night noise +10 dB to reflect the greater annoyance of noise during rest periods:

```
Lden = 10 × log₁₀((12 × 10^(Ld/10) + 4 × 10^((Le+5)/10) + 8 × 10^((Ln+10)/10)) / 24)
```

Day: 07:00–19:00, evening: 19:00–23:00, night: 23:00–07:00.

[WHO 2018 guidelines](https://www.who.int/europe/publications/i/item/9789289053563) recommend: road < 53 dB, rail < 54 dB, aircraft < 45 dB Lden.

### Grid

A Web-Mercator raster at zoom 12 (512-pixel tiles, ~12 m per pixel at 50°N, varies with latitude) — fine enough to distinguish the street-facing vs garden side of a building. A zoom pyramid (z2–12) serves coarser tiles when zoomed out.

### Color scale

The colors are not ours. They come from Beate Tomio (Weninger), ["A Color Scheme for the Presentation of Sound Immission in Maps"](https://www.researchgate.net/publication/280488890_A_color_scheme_for_the_presentation_of_sound_immission_in_maps), EuroNoise 2015 — a scheme tested with 232 respondents — as published in its current revision **v5.b (eleven classes, 30–80 dB)** on [coloringnoise.com](https://www.coloringnoise.com/theoretical_background/new-color-scheme/) (licensed CC BY-NC-ND 4.0). We use the class colors unmodified, hex for hex, dB boundary for dB boundary.

Below 30 dB the map is transparent (the scheme's "no color"); 80 dB is the terminal shade, held flat above it rather than inventing a darker one. Colors interpolate smoothly between rows — a cell at 62 dB gets a blended shade between the 60 and 65 dB rows, never a hard jump. Opacity is our rendering adaptation, not part of the scheme: the paper is opaque, but we overlay a legible basemap, so alpha rises with dB — the paper's own "louder = more salient" intent, executed via alpha.

| Lden | Swatch | Hex | Opacity |
|------|--------|-----|---------|
| < 30 dB | — | — | 0% — not shown |
| 30 dB | <span style="display:inline-block;width:12px;height:12px;border-radius:3px;vertical-align:middle;border:1px solid rgba(0,0,0,.15);background:#82A6AD"></span> | `#82A6AD` | 40% |
| 35 dB | <span style="display:inline-block;width:12px;height:12px;border-radius:3px;vertical-align:middle;border:1px solid rgba(0,0,0,.15);background:#A0BABF"></span> | `#A0BABF` | 45% |
| 40 dB | <span style="display:inline-block;width:12px;height:12px;border-radius:3px;vertical-align:middle;border:1px solid rgba(0,0,0,.15);background:#B8D6D1"></span> | `#B8D6D1` | 50% |
| 45 dB | <span style="display:inline-block;width:12px;height:12px;border-radius:3px;vertical-align:middle;border:1px solid rgba(0,0,0,.15);background:#CEE4CC"></span> | `#CEE4CC` | 55% |
| 50 dB | <span style="display:inline-block;width:12px;height:12px;border-radius:3px;vertical-align:middle;border:1px solid rgba(0,0,0,.15);background:#E2F2BF"></span> | `#E2F2BF` | 60% |
| 55 dB | <span style="display:inline-block;width:12px;height:12px;border-radius:3px;vertical-align:middle;border:1px solid rgba(0,0,0,.15);background:#F3C683"></span> | `#F3C683` | 65% |
| 60 dB | <span style="display:inline-block;width:12px;height:12px;border-radius:3px;vertical-align:middle;border:1px solid rgba(0,0,0,.15);background:#E87E4D"></span> | `#E87E4D` | 70% |
| 65 dB | <span style="display:inline-block;width:12px;height:12px;border-radius:3px;vertical-align:middle;border:1px solid rgba(0,0,0,.15);background:#CD463E"></span> | `#CD463E` | 75% |
| 70 dB | <span style="display:inline-block;width:12px;height:12px;border-radius:3px;vertical-align:middle;border:1px solid rgba(0,0,0,.15);background:#A11A4D"></span> | `#A11A4D` | 80% |
| 75 dB | <span style="display:inline-block;width:12px;height:12px;border-radius:3px;vertical-align:middle;border:1px solid rgba(0,0,0,.15);background:#75085C"></span> | `#75085C` | 85% |
| 80+ dB | <span style="display:inline-block;width:12px;height:12px;border-radius:3px;vertical-align:middle;border:1px solid rgba(0,0,0,.15);background:#430A4A"></span> | `#430A4A` | 90% |

### Toggles

- **Source layers:** Roads, Railways, Industrial, Buildings, and Aircraft (ground ops, airborne, cruise) — each toggleable independently
- **Overlays:** Quiet zones (areas below a threshold)

## Overlays

### Real estate — in preparation

Property listings on the map, filtered by noise: each listing will carry the computed Lden at its location — sampled from the same tiles the map shows — with a noise slider to hide everything above your threshold. We are preparing data partnerships with listing portals; a prototype of the feature already works end-to-end.

### Quiet zones

Shades every map pixel below a configurable noise threshold (default 35 dB, slider 20–45) green. Useful for identifying quiet retreats, parks, and areas suitable for noise-sensitive development.

## FAQ

**Is this measured or computed?**
Computed — a physics model (CNOSSOS-EU emission, ISO 9613-2 propagation) over public data. No microphone network could cover the planet at 12-meter resolution. The model is continuously checked against real monitoring stations; see [Validation](/about/methodology).

**How accurate is it?**
It's an engineering estimate, not a certificate. The target is a mean error under 3 dB against official strategic noise maps for road noise, and every confirmed gap against a real measurement station becomes a fix. For a single address, read the value as "around X dB" — and click the point to see exactly what the number is built from.

**Why does my quiet street show 50 dB?**
Click it. Most surprises have a visible cause: a road with no measured traffic falls back to class defaults, a nearby factory is classified by registry sector, or the dominant source is something you've tuned out. If the inputs are genuinely wrong for your street, [tell us](mailto:hello@quietmap.org) — reports with an address are how the map gets better.

**Why are there no low-flying aircraft where I live?**
The aircraft layer sees what volunteer ADS-B receivers see. Where no feeder is nearby, low-altitude flights aren't received and only high-altitude cruise noise (~20 dB) appears — a limit of the data source, not the model. Hosting a receiver in a blank spot fixes it for everyone.

**Why does the map show nothing below 30 dB?**
By design: the [color scheme](#color-scale) marks under 30 dB as "no color" — genuinely quiet. To hunt for the quietest places, use the Quiet zones overlay, which shades everything under a threshold you pick (20–45 dB).

**Can I use screenshots or embed the map?**
Yes, free, with visible "quietmap.org" attribution — details in [credits & terms](/about/credits).

## Help us make it better

**See something wrong on your street?** Write to [hello@quietmap.org](mailto:hello@quietmap.org) with the address. Every confirmed report feeds the validation loop — real-world corrections are the most valuable data we get.

**Have data? We're looking for** (in order of impact):

1. **Road traffic from navigation apps** — per-street average counts of cars / trucks / motorcycles by time of day, at Waze / Google Maps / TomTom scale. This is the single biggest accuracy lever the map has.
2. **Commercial flight tracking** — denser coverage than the open feeds we use today (e.g. Flightradar24-grade data).
3. **Railway traffic** — timetables and passenger/freight train counts per line.
4. **Real noise measurements** — station exports, long-term campaigns, monitoring-network data anywhere in the world. These feed the validation loop directly: every honest measurement makes the model demonstrably better.
5. **Better national data for any country** — traffic censuses, facility registries, turbine inventories.
6. **Shipping** — vessel traffic and port operations, for a future marine layer.

If you work somewhere that has this data — or know who does — [we'd love to talk](mailto:hello@quietmap.org).

## Who builds this

quietmap.org is built by one person working with three AI coding agents: **Claude** as lead developer, **Codex** as second developer and code reviewer, and **Gemini** for an independent second opinion and review — with promising open-source models tried along the way as they appear. Development started in June 2025 on Opus 4; every major Opus, GPT, and Gemini release since has been tried on this codebase — progress accelerated markedly with [OpenClaw](https://openclaw.ai/) and Opus 4.6, and it's kept getting better since.

Some of it was built while hiking the forests of La Palma — changes discussed with the models over Telegram through OpenClaw, on a mobile signal that kept cutting out. It worked surprisingly well. Fitting, for a map about quiet.

quietmap.org is an internal project of [Miton](https://www.miton.cz/en/).

The code will be open-sourced once the repository is cleaned up for public release. The computations themselves are already transparent and reproducible from public data.

## Credits & terms

quietmap.org builds on the open geodata ecosystem — OpenStreetMap, Copernicus, ESA WorldCover, ADS-B community feeds, and more — and is free to use and embed with attribution, no cookies or trackers.

→ **[Data credits, usage terms & privacy](/about/credits)**

## Contact & status

- **Email:** [hello@quietmap.org](mailto:hello@quietmap.org)
- **Service status:** [status.quietmap.org](https://status.quietmap.org) — live uptime of the map and tiles
