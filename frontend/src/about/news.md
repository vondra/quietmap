---
title: What's new
intro: What changed in the data and the model, and what comes next.
nav: hidden
---

## October 2026

- **Nden.** The click panel leads with Nden, how loud the place sounds on average over the day, in sone (twice the number sounds twice as loud), with Lden under it. A sound counts by how loud it is and how long it lasts: a car every few hours by a hut adds little, a city's constant hum all of its loudness. Each source in the list shows how loud it is alone.
- **Every click computed on the spot.** The click panel no longer reads a painted map: it computes its point from every source within 12 km (aircraft 16 km) with the EU method CNOSSOS-EU and ECAC Doc 29, and shows Lden, the day, evening and night levels, how often the level is exceeded and how loud it sounds.
- **The data under every click.** A new Advanced group in the layer panel shows elevation, forest, hard ground, buildings by height and noise barriers, the vehicles a day on every road, the trains a day on every track, and every other source — wind turbines, church bells, terraces, car parks, airports.
- **Forest.** Trees no longer reduce noise: CNOSSOS-EU has no term for foliage, and the ground under them counts as soft ground.
- **Roads.** Uncounted streets carry the trips their buildings make, Overture's footprints included; uncounted main roads in most of Europe, the United States, Mexico, Chile, Colombia and New Zealand follow one model fitted on counted roads. Motorcycles run 3.8 dB louder than cars; US roads carry their state's mix of lorries; day, evening and night shares come from counts in seven countries.
- **Poland's noise screens.** The 10,876 screens of the national topographic database (BDOT10k) now screen the roads: along motorways and expressways the model went from 4.1 to 3.2 dB above the monitoring points.
- **Wind turbines.** 472,924 standing turbines from OpenStreetMap and eight national registers: 4,364 added that were missing, 680 the registers call dismantled removed.
- **Railways.** A stretch of track between two switches carries one train count, so bridges no longer keep counts of their own; in 31 European countries the lines without a count carry their country's train-kilometres (Eurostat).
- **Homes, bells, calls and terraces.** Homes emit the heat pumps and air conditioners their country's households own; Europe's churches ring their bells, mosques call at their own prayer times, and the people on terraces and at the doors of 3.4 million bars and restaurants are heard by their opening hours.
- **Terrain.** National laser surveys of Poland, Ireland and Spain added: 17 countries in all.

## September 2026

- **Ships.** New worldwide layer for ship noise at sea, in ports and on inland waterways, derived from AIS vessel-density data (EMODnet 2024, Global Fishing Watch). Examples, ships only: Singapore Marina 62.5 dB, Rotterdam Waalhaven 58.0 dB, Rhine at Duisburg 48.5 dB.
- **Railways.** Train counts recomputed worldwide from current timetables. Unmatched daily departures fell from 1,417 to 89 of 14,925 in France and from 8,419 to 194 of 43,572 in Germany. Paris Transilien added.
- **Road estimates.** Uncounted main roads use defaults by lane count and direction; generic country multipliers are removed.
- **Metro and parking.** Surface metro sections are included. Open car parks emit noise without screening it; underground building footprints do not create above-ground walls.
- **New world build.** All layers rebuilt from a current OpenStreetMap planet extract; flight data covers 2 September 2025 – 1 September 2026.

## Summer 2026

- **quietmap.org.** New name and address.
- **Buildings.** Real footprints and heights replace the coarse grid. Prague uses 174,000 building heights measured from aerial survey.
- **Screening.** Road cuttings and noise walls are modelled. 120 m from the M25 the deviation from the official English noise map fell from 12.9 dB to about 2 dB.
- **Ground effect.** Computed per frequency band according to CNOSSOS-EU and verified against the standard's test cases.
- **Forest.** Continuous canopy density replaces the forest/no-forest flag.
- **Traffic data.** National counts added for Thailand, Mexico and Japan.

## In progress

- A new year of flights with a table of the flights heard at each place: how many a day above 50, 60 and 70 dB.
- Unsigned roads in the United States and Canada at their signed speeds, and US and Canadian train counts from the level-crossing inventories.
- A new painted map from the click's own computation.

*Last updated 8 October 2026.*
