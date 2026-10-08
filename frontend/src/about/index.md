---
title: quietmap.org
intro: Find your quiet place. A world atlas of environmental noise — roads, railways, aircraft, ships, industry, and the buildings around you.
map: { center: [15, 30], zoom: 2 }
---

## Mission

**Make noise visible. Make quiet possible.**

quietmap.org shows how loud the world really is — and helps you find the quiet.

What gets measured gets managed. Mapping noise is the first step toward a quieter planet — one where nature can be heard again and people can thrive.

1. **Find quiet places** — search any address, explore the map, discover where to live, work, or relax without noise
2. **Understand noise** — see which sources contribute (roads, railways, aircraft, ships, industry, buildings) and how terrain, buildings, and noise barriers reduce it
3. **Build a comparable record** — each published dataset generation is frozen, so later generations can be compared honestly

Human-made noise is not the same as natural sound. A forest at 50 dB with birdsong feels quiet. A road at 50 dB with traffic feels loud. quietmap.org models environmental noise from human sources — transport, industry, and urban activity — not nature.

→ **[What's new](/about/news)** — recent improvements and what we are working on.

## How the map works

Three steps:

1. **Sources emit noise.** Roads, trains, planes, ships, factories, wind turbines and buildings — each modelled from real data: traffic counts, timetables, a year of flight tracks, ship positions, registries.
2. **Sound travels and fades.** Hills and buildings block it, noise barriers screen it, soft ground and the air absorb it — computed with the EU method CNOSSOS-EU, and for aircraft with ECAC Doc 29.
3. **You see the result.** Click any point and the level there is computed on the spot from every source around it; the map shows the levels in colour, from pale (quiet) through yellow and orange to deep purple (80+ dB).

Each source layer — roads, railways, aircraft, ships, industrial, buildings — is modelled independently and toggles on its own in the map.

![quietmap.org — noise visualization](map-overview.jpg)

→ **[Read the full methodology](/about/methodology)** — source layers, propagation,
the standards they use, known limits, and validation against real measurements.

## Click anywhere

Every point on the map can explain itself. Click, and a panel shows the Lden at that spot, its day, evening and night levels and how loud it sounds, then every source audible there, loudest first — a road as one row, a factory, the aircraft — each with the share it contributes.

Open a source and you see its data (vehicles a day and their mix, speed, surface, trains a day, the flights and their types…) and how it is heard: a steady hum, so many cars an hour, a train every so many minutes. The ten loudest flights are listed.

**Detailed calculation** is the raw computation: per period and layer, the weather, and the pieces of road, track and flight path that count most, each drawn on the map with what the distance, the air, the ground, the terrain and the buildings took off on its path, band by band.

<p>
<img src="click.jpg" alt="A click on náměstí Míru in Prague: the level and its sources, the loudest road opened with its data" style="display:inline-block;width:300px;max-width:48%;vertical-align:top;margin:0 12px 0 0">
<img src="calculation.jpg" alt="The same click's detailed calculation: each period, each layer, the weather, and the paths from the loudest pieces" style="display:inline-block;width:300px;max-width:48%;vertical-align:top;margin:0">
</p>

Nothing on the map is a black box — if a number surprises you, two clicks show where it came from.

## Data and enrichment

The map combines OpenStreetMap geometry with public traffic, rail, flight, ship, building,
terrain, land-cover, wind-turbine and industrial-registry data. Local measurements and
registries override class defaults where they exist; a street nobody counted carries the
trips its buildings make; otherwise a source inherits a documented default for its class.
Matching is class-aware: a motorway count does not become a residential-street count, and
a tram timetable does not become a mainline estimate.

The [methodology](/about/methodology) explains the model and its limits. Country pages
say what is measured, what is estimated, and what is still missing in each place; use the
region list below to explore them.

<!-- REGION_CHILDREN -->

## Dataset generations

Each published generation is a frozen worldwide build: one OpenStreetMap planet extract, one
year of flight observations, and the public traffic and registry data available at build
time. The current generation dates from October 2026, with the OpenStreetMap planet of
September 2026 and flights from September 2025 to August 2026.

## What you see on the map

### The noise indicator: Lden

The map shows **Lden** (day-evening-night level), the European standard from [END 2002/49/EC](https://eur-lex.europa.eu/eli/dir/2002/49/oj/eng). It weights evening noise +5 dB and night noise +10 dB to reflect the greater annoyance of noise during rest periods:

```
Lden = 10 × log₁₀((12 × 10^(Ld/10) + 4 × 10^((Le+5)/10) + 8 × 10^((Ln+10)/10)) / 24)
```

Day: 07:00–19:00, evening: 19:00–23:00, night: 23:00–07:00.

[WHO 2018 guidelines](https://www.who.int/europe/publications/i/item/9789289053563) recommend: road < 53 dB, rail < 54 dB, aircraft < 45 dB Lden.

### Point and grid

A click computes its own point, 4 m above ground as the END prescribes, or the loudest façade of the building clicked. The map's colours are painted on a Web-Mercator raster, fine enough to tell the street side of a building from its garden side; coarser levels of the same paint serve the zoomed-out views.

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

### Layers

- **Source layers:** Roads, Railways, Industrial, Buildings, and Aircraft (ground ops, airborne, cruise), each toggleable independently; Quiet zones highlight the places below a chosen level.
- **Advanced:** the data every click computes over — elevation, forest, hard ground, buildings by height, road traffic (vehicles a day), trains a day, the other sources (turbines, bells, terraces, car parks, airports…) and noise barriers.

## FAQ

**Is this measured or computed?**
Computed — a physics model (CNOSSOS-EU for ground sources, ECAC Doc 29 for aircraft) over public data. The model is continuously checked against real monitoring stations; see [Validation](/about/methodology).

**How accurate is it?**
It's an engineering estimate, not a certificate. A gap against a measurement or official map is first attributed to better input data, a justified methodology difference, or a model defect; only defects become fixes. For a single address, read the value as "around X dB" — and click the point to see exactly what the number is built from.

**Why does my quiet street show 50 dB?**
Click it. Most surprises have a visible cause: a road with no measured traffic carries the trips its buildings make or a class default, a nearby factory is classified by registry sector, or the dominant source is something you've tuned out. If the inputs are genuinely wrong for your street, [tell us](mailto:info@quietmap.org) — reports with an address are how the map gets better.

**Why are there no low-flying aircraft where I live?**
The aircraft layer sees what volunteer ADS-B receivers see. Where no feeder is nearby, low-altitude flights aren't received and only high-altitude cruise noise (~20 dB) appears — a limit of the data source, not the model. Hosting a receiver in a blank spot fixes it for everyone.

**Why does the map show nothing below 30 dB?**
By design: the color scheme marks under 30 dB as "no color". A blank area can also mean missing source data. A click still gives the level there, however low.

**Can I use screenshots or embed the map?**
Yes, free, with visible "quietmap.org" attribution — details in [credits & terms](/about/credits).

## Help us make it better

**See something wrong on your street?** Write to [info@quietmap.org](mailto:info@quietmap.org) with the address. Every confirmed report feeds the validation loop — real-world corrections are the most valuable data we get.

**Have data? We're looking for** (in order of impact):

1. **Road traffic from navigation apps** — per-street average counts of cars / trucks / motorcycles by time of day, at Waze / Google Maps / TomTom scale. This is the single biggest accuracy lever the map has.
2. **Commercial flight tracking** — denser coverage than the open feeds we use today (e.g. Flightradar24-grade data).
3. **Railway traffic** — timetables and train counts per line, freight above all: almost no country publishes it, and freight runs at night.
4. **Real noise measurements** — station exports, long-term campaigns, monitoring-network data anywhere in the world, quiet places above all. These feed the validation loop directly: every honest measurement makes the model demonstrably better.
5. **Better national data for any country** — traffic censuses, facility registries, turbine inventories.
6. **Shipping** — vessel positions inside ports and on rivers, where the open AIS products are thinnest.

If you work somewhere that has this data — or know who does — [we'd love to talk](mailto:info@quietmap.org).

## Who builds this

quietmap.org is built by one person working with AI coding agents: **Claude** as lead developer, with **Codex**, **Grok** and a rotating cast of other models — open-source ones included — as reviewers and second opinions. Development started in June 2025 on Opus 4; every major Opus, GPT, and Gemini release since has been tried on this codebase. Progress accelerated markedly with [OpenClaw](https://openclaw.ai/) and Opus 4.6, and it has kept getting better since.

Some of it was built while hiking the forests of La Palma — changes discussed with the models over Telegram through OpenClaw, on a mobile signal that kept cutting out. It worked surprisingly well. Fitting, for a map about quiet.

quietmap.org is an internal project of [Miton](https://www.miton.cz/en/).

The [product code is open source](https://github.com/vondra/quietmap), and the computations are transparent and reproducible from public data.

## Credits & terms

→ **[Data credits, usage terms & privacy](/about/credits)**

## Contact & status

- **Email:** [info@quietmap.org](mailto:info@quietmap.org)
- **Service status:** [status.quietmap.org](https://status.quietmap.org) — live uptime of the map and tiles
