---
title: quietmap.org
intro: Find your quiet place. A world atlas of environmental noise — roads, railways, aircraft, ships, industry, and the buildings around you.
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
2. **Sound travels and fades.** Hills and buildings block it, noise barriers screen it, soft ground, trees and the air absorb it — computed with the EU method CNOSSOS-EU (trees with ISO 9613-2), and for aircraft with ECAC Doc 29.
3. **You see the result.** Click any point and the level there is computed on the spot from every source around it; the map shows how loud each place sounds, in colour: clear under 1 sone (a quiet room), then sage green, amber, orange, red and maroon, a colour for each doubling of loudness, maroon from 32 sone.

Each source layer — roads, railways, aircraft, ships, industrial, buildings — is modelled independently; the map shows any of them together.

![Prague on the map, coloured by how loud each place sounds](map-overview.jpg)

## In depth

Five pages, each from the simple to the exact, down to the code:

- [Data sources](/about/data-sources): where every number comes from, and what is counted or
  estimated in your country.
- [Data preparation](/about/data-preparation): how a road, a railway, a flight or a factory gets
  its sound power.
- [Noise at a point](/about/noise-at-a-point): what a click shows, and how it is computed.
- [The noise map](/about/the-noise-map): what the colours mean, and how they are painted.
- [Validation](/about/validation): how close the model comes to real measurements.

## FAQ

**Is this measured or computed?**
Computed — a physics model (CNOSSOS-EU for ground sources, ECAC Doc 29 for aircraft) over public data. The model is continuously checked against real monitoring stations; see [Validation](/about/validation).

**How accurate is it?**
It's an engineering estimate, not a certificate. A gap against a measurement or official map is first attributed to better input data, a justified methodology difference, or a model defect; only defects become fixes. For a single address, read the value as "around X dB" — and click the point to see exactly what the number is built from. The [validation](/about/validation) page shows the gaps network by network.

**Why does my quiet street show 50 dB?**
Click it. Most surprises have a visible cause: a road with no measured traffic carries the trips the buildings around it make or a class default, a nearby factory is classified by registry sector, or the dominant source is something you've tuned out. If the inputs are genuinely wrong for your street, [tell us](mailto:info@quietmap.org) — reports with an address are how the map gets better.

**Why are there no low-flying aircraft where I live?**
The aircraft layer sees what volunteer ADS-B receivers see. Where no feeder is nearby, flights aren't received and the layer reads too quiet or empty: near Lagos and Cairo airports 5–20 dB low, over parts of Siberia and the Amazon with no aircraft at all — a limit of the data source, not the model. Hosting a receiver in a blank spot fixes it for everyone.

**Why is the map clear in places?**
By design: under 1 sone, about as loud as a quiet room, the map has no colour. A blank area can also mean missing source data, or a place the map has not painted yet. A click still gives the level there, however low.

**Can I use screenshots or embed the map?**
Yes, free, with visible "quietmap.org" attribution — details in the [terms of use](/about/terms).

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

The [product code is open source](https://github.com/vondra/quietmap/tree/main) under the Apache License 2.0, and the computations are transparent and reproducible from public data.

## Credits & terms

→ **[Data sources and credits](/about/data-sources)** · **[Terms of use and privacy](/about/terms)**

## Contact & status

- **Email:** [info@quietmap.org](mailto:info@quietmap.org)
- **Service status:** [status.quietmap.org](https://status.quietmap.org) — live uptime of the map and tiles
