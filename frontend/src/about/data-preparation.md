---
title: Data preparation
intro: How every road, railway, flight, ship, factory and building gets its sound power, and how it is stored for a click.
---

Every source on the map gets a sound power: how much noise it makes, in eight octave bands from
63 Hz to 8 kHz, by day, evening and night. It comes from counts where somebody counted — the D4
motorway at Mníšek carries the Czech national census of 2020 — and from documented defaults where
nobody did. The defaults are the largest source of error; the [country pages](/about/data-sources)
say where they apply.

## Roads

A road's sound power follows CNOSSOS-EU from its traffic, vehicle mix, speed, surface and gradient,
with acceleration at junctions and the place's yearly air temperature. Doubling the traffic adds
about 3 dB.

Traffic is the largest source of error. Of the world's 5.8 million km of motorways, trunk and
primary roads, 31 % carry a dataset (in India, China and a few other countries a national estimate
rather than counts), 14 % a model of the trips made within 1, 5 and 15 km fitted on counted roads,
and 54 % a fixed estimate for their class or country (October 2026). Secondary and tertiary roads
without a count follow a fit of the same kind, made on 13 European countries. A street nobody
counted carries the trips its buildings make, OpenStreetMap's and Overture's, routed along the
street network to the nearest main road.

The vehicle mix is counted where counted; otherwise a main road takes the counted median of its
class, a local street 1 % medium and 2 % heavy vehicles, a US road its state's mix (FHWA, table
VM-4). Day, evening and night shares are measured in the United States, Japan, Germany, the
Netherlands, Great Britain, Spain and Thailand, elsewhere taken from the region's median or a
default. Battery-electric cars, at their country's 2024 share (IEA), make rolling noise only;
motorcycles run 3.8 dB louder than cars, 2.9 dB pulling away (ASJ RTN-Model 2018), at shares from
counts and national fleets (WHO 2023).

Speeds are the posted limit. An unsigned road takes its country's legal limit for its class, or the
median signed speed of its class where that is lower or no legal limit is known; a residential
street 30 km/h, a living street, service road or track 20. On a two-way road outside towns, other
than a motorway, at its country's open-road limit of 80 km/h or more, cars drive at 0.845 of it,
the free-flow share measured in Great Britain. Lorries and buses drive at most 80 km/h, 90 to 105 in
thirteen countries. Uncounted roads also carry the buses of mapped bus routes. A road on a bridge
emits from its deck, straight between the ground at the bridge's two ends.

<details>
<summary><strong>Default traffic by road class</strong></summary>

Defaults are length-weighted medians of counted public roads, fitted in September 2026 by class,
direction and surroundings; local counts, the models above and national tables come first.
Motorways, trunk and primary roads with 1 to 6 lanes take these vehicles a day per lane (a
three-lane urban one-way motorway: 3 × 12,543 = 37,629):

| Road | Direction | Rural | Urban | Surroundings unknown |
|---|---|---:|---:|---:|
| Motorway | One-way | 6,174 | 12,543 | 7,331 |
| Motorway | Two-way | 2,305 | 4,396 | 2,305 |
| Trunk | One-way | 2,950 | 6,188 | 3,652 |
| Trunk | Two-way | 1,959 | 4,000 | 2,205 |
| Primary | One-way | 3,314 | 4,980 | 4,579 |
| Primary | Two-way | 1,977 | 4,136 | 2,636 |

Without a plausible lane count, and on secondary and tertiary roads, the whole carriageway takes
these vehicles a day:

| Road | Direction | Rural | Urban | Surroundings unknown |
|---|---|---:|---:|---:|
| Motorway | One-way | 5,208 | 30,980 | 6,008 |
| Motorway | Two-way | 6,297 | 7,640 | 6,297 |
| Trunk | One-way | 5,061 | 9,744 | 6,652 |
| Trunk | Two-way | 4,260 | 8,289 | 4,840 |
| Primary | One-way | 4,966 | 7,963 | 7,103 |
| Primary | Two-way | 3,644 | 9,322 | 4,500 |
| Secondary | One-way | 1,030 | 3,222 | 1,500 |
| Secondary | Two-way | 2,061 | 6,445 | 3,000 |
| Tertiary | One-way | 2,266 | 4,298 | 4,132 |
| Tertiary | Two-way | 1,002 | 2,562 | 1,506 |

Local streets in squares with too few mapped buildings keep the earlier version's estimate.
Directional counts are kept as published; counts of both directions are shared between
carriageways, and a divided main road whose other carriageway is missing takes half.

</details>

## Railways

Each kind of train — passenger, high-speed, freight, tram — has one sound power spectrum, its
rolling noise growing with speed, set to a typical train by CNOSSOS-EU's per-vehicle computations:
a passenger train is the mean of Germany's railway noise monitoring in 2023 (26 axles, 176 m), and
freight in the EU is the EU fleet's, quieter than elsewhere. The trains of a period are spread along
the line as CNOSSOS-EU spreads them; a heavy train's sound power is taken at no less than 50 km/h, a
light-rail train's at 30, and freight runs at no more than 89 km/h.

Train counts come from GTFS feeds and national timetables, every trip routed along the track
network; a stretch of track between two switches carries one count. Freight is in almost no public
timetable, so it comes from defaults, scaled in 29 European countries to the country's official
goods train-kilometres (Eurostat 2024; the United Kingdom 2019). In the EU it is weighted 4 on a
TEN-T core corridor, 2 on another TEN-T line and 0.5 off them, and 37.6 % of it runs between 22
and 06 h, as counted at the German monitors. Lines mapped as passenger-only take no default
freight. Above-ground metro uses the light-rail model; track underground emits nothing outdoors.

At US and Canadian public level crossings, locomotives sound their horns over up to a quarter mile
before the crossing, half the crossing's trains in each direction; US quiet zones stay silent, and
partial ones at night.

<details>
<summary><strong>Default trains a day</strong></summary>

Where no count or national estimate exists; six countries have defaults of their own (Germany 24.5
freight trains a day on a main line, Switzerland 31.4). Main, branch and industrial use must be
mapped; other lines take the unclassified row. Parallel tracks share their line's count.

| Railway | Passenger | Freight |
|---|---:|---:|
| Main line | 80 | 20 |
| Branch | 30 | 5 |
| Industrial | 0 | 15 |
| Unclassified | 40 | 10 |
| Tram | 120 | 0 |
| Light rail or metro | 80 | 0 |
| Narrow gauge | 10 | 0 |
| Funicular | 40 | 0 |

</details>

## Aircraft

A year of flights as volunteer receivers recorded their ADS-B signals: adsb.lol from September 2025
to August 2026, with the flights only ADSBExchange received added from 11 sample days. Each type
flies on its noise-power-distance tables from EASA's ANP database (a similar aircraft where the
database has none, a generic jet where the type is unknown), by ECAC Doc 29, 4th edition: height
above the geoid, thrust from the observed climb and acceleration, flaps and gear on approach.
Helicopters take their type's certification level.

- Ground: taxiing, take-off and landing rolls on the runways and taxiways, mapped or found where
  the flights' ground tracks run.
- Airborne: up to 7,620 m (25,000 ft) above the ground, the last distance of the noise tables.
- Cruise: above it.

The year is summed into boxes of airspace: a cell of the map's grid and a slab of height above the
highest ground around it. A box holds its flights' sound energy by day, evening and night at the
ten distances of the noise tables, and its two loudest flights for the click's list; a box's Lden
reads back within 0.2 dB of its flights summed one by one, a period's within 0.3 dB. Where no
receiver is near, low flights are missing: near nine of 59 large airports (Beijing, Shanghai
Pudong, Cairo, Casablanca, Riyadh, Tehran, Accra, Moscow Sheremetyevo, Lagos) the map reads 5 to
20 dB too quiet.

## Ships

AIS vessel density: EMODnet 2024 in European seas (a 1 km grid) and Global Fishing Watch elsewhere,
on rivers and in harbours where AIS sees ships; leisure boats only in European seas. A large ship
emits 108 dB(A), a work boat 98 and a leisure boat 88
([Fredianelli et al. 2020](https://doi.org/10.3390/su12051740),
[Schiavoni et al. 2022](https://pmc.ncbi.nlm.nih.gov/articles/PMC9518360/)). Speed is not
modelled, so ships under way may be a few decibels louder, and activity is constant over the day,
which overstates leisure boats at night.

## Industry

Sites and their kind are OpenStreetMap's; E-PRTR in Europe, the Global Power Plant Database, Global
Energy Monitor's steel, cement and coal trackers and 14 national registers add the sector. Sound
power follows the sector and the site's area, its base levels calibrated on the Czech strategic
noise maps of 2022. Coal and lignite mines run day and night; other mines, quarries and offices
keep daytime hours; warehouses run 3 dB quieter in the evening and 8 dB at night; inactive sites
are silent.

Wind turbines are the standing ones of OpenStreetMap and eight registers (the United States,
Canada, Germany, Denmark, Sweden, Norway, Castilla-La Mancha, the Netherlands with Belgium), which
add the turbines OpenStreetMap lacks and remove those they call dismantled. Each emits its annual
operating level: the published maximum for its rated power, about 2 dB less for calm and
part-load hours. Solar farms hum from their inverters by day (88 dB(A) a MW at full output;
0.55 MW a hectare where untagged), substations from their transformers around the clock (IEC 551).

## Buildings and places

- Homes emit the outdoor units of the heat pumps and air conditioners their country's households
  own (surveys in 35 countries, the United States, Canada and the Gulf), for the hours their climate
  asks: a Prague home 45 dB(A) by day, a Bangkok home 51. Buildings known only from Overture's
  footprints emit nothing.
- Shops, schools, hotels and other buildings emit by type, area and assumed hours — estimates, not
  measurements; the data often cannot tell a warehouse from a workshop.
- Sports pitches radiate Sport England's typical in-use level (58 dB LAeq,1h at 10 m) for their
  hours of play; outdoor shooting ranges fire 20,000 shots a year by day.
- Car parks follow the [Bavarian parking study](https://www.lfu.bayern.de/publikationen/get_pdf.htm?art_nr=lfu_lae_00045&pdf_nr=0)
  (LfU 2007): 63 dB(A) for one movement an hour, 0.4 movements a space an hour from 06 to 22 h and
  0.05 at night; street parking carries only the cars of the buildings around it.
- Church bells ring in Europe and Cyprus: prayer bells three times a day and a Sunday peal; German
  and Swiss church clocks strike the quarters. A peal is 114 dB(A), a cathedral's 122.
- Calls to prayer sound at the place's prayer times, five a day at 118 dB(A) from a minaret's horns;
  where mosques rarely call aloud, as in most of Europe and the Americas, only a mosque with a
  mapped minaret calls, at Friday noon.
- People sit on the terraces and stand at the doors of bars and restaurants by their mapped opening
  hours, else the country's custom; terrace seats come from Madrid's and Melbourne's registers, and
  how full they are is an assumption, as nothing is measured.

Buildings and noise walls also screen. Heights come from measured surveys (North Rhine-Westphalia,
the Netherlands, Prague), then mapped heights, floor counts (Czechia's RÚIAN, Spain's Catastro),
Overture, and last the typical height for the footprint. Walls come from official inventories
(Poland, the Netherlands, Washington, Florida, Virginia) and OpenStreetMap; a wall without a height
stands 4 m. Open car parks, carports, open roofs and underground footprints do not screen.

Not modelled: roads and railways in tunnels, rail yards, heritage lines, curve squeal, motorsport,
deliveries, building sites, garden machinery, music and festivals.

## Terrain, ground and weather

Terrain is bare earth: national terrain models in 17 countries, most from laser surveys, over the
GEDTM30 world model. Near the ends of a ray, a national model of 5 m or finer adds what the
arc-second grid misses (Czechia, Germany, Switzerland, the Netherlands, Belgium and part of
Portugal). Ground is hard where it is sealed, built over or water. Weather — how often sound bends
down to the ground from each direction, and what the air absorbs — is a 1991–2020 ERA5
climatology. The map's Advanced layers show the terrain, the trees, the hard ground, the buildings
and walls, and every source.

## The files

Everything a click or the painter reads lies in one tree of files. The world is cut into the
standard web-map tiles of zoom 12, 6.3 km wide in Prague and 9.8 km at the equator. Each tile has
one file per kind of data, the 64 tiles of a zoom-9 square share a directory, and a tile with
nothing of a kind has no file of it: the open sea has no terrain file. A file holds its kind's
records in one zstd frame, decoded once as it is read, so only the records stay in memory; the
terrain's files carry their own coder, the residual of a planar prediction of the heights. The
world takes 1.5 TB (October 2026).

<figure style="margin: 1.75rem 0; color: var(--foreground)">
<svg viewBox="0 0 400 242" role="img" aria-label="A zoom-9 square of eight by eight tiles with tile 2212/1387 of Prague's centre marked, and the directory holding its seven files, beside the weather table and the flight tracks" style="display: block; width: 100%; max-width: 440px; height: auto; margin: 0 auto" font-size="12">
<rect x="8" y="34" width="128" height="128" fill="none" stroke="currentColor" stroke-width="1.2"/>
<path d="M24 34V162M40 34V162M56 34V162M72 34V162M88 34V162M104 34V162M120 34V162M8 50H136M8 66H136M8 82H136M8 98H136M8 114H136M8 130H136M8 146H136" fill="none" stroke="currentColor" stroke-opacity=".3" stroke-width=".75"/>
<rect x="72" y="82" width="16" height="16" fill="#2f7fd8"/>
<text x="8" y="22" fill="currentColor" font-weight="600">z9 square 276/173</text>
<text x="8" y="180" fill="currentColor" fill-opacity=".7">8 × 8 tiles of zoom 12,</text>
<text x="8" y="196" fill="currentColor" fill-opacity=".7">each 6.3 km in Prague</text>
<path d="M165 27V54M165 36H171M165 54H171M181 63V228M181 72H187M181 228H187" fill="none" stroke="currentColor" stroke-opacity=".45"/>
<rect x="198" y="83" width="192" height="116" rx="4" fill="#2f7fd8" fill-opacity=".13"/>
<g fill="currentColor" font-family="ui-monospace, SFMono-Regular, Menlo, Consolas, monospace">
<text x="160" y="22">release/</text>
<text x="176" y="40">weather</text>
<text x="176" y="58">2026/</text>
<text x="192" y="76">276/173/</text>
<g font-size="11.5">
<text x="206" y="99">2212_1387.terrain</text>
<text x="206" y="115">2212_1387.terrain-fine</text>
<text x="206" y="131">2212_1387.obstacles</text>
<text x="206" y="147">2212_1387.sources</text>
<text x="206" y="163">2212_1387.aircraft</text>
<text x="206" y="179">2212_1387.aircraft-far</text>
<text x="206" y="195">2212_1387.aircraft-events</text>
</g>
<text x="192" y="232">aircraft-tracks/</text>
</g>
<text x="206" y="215" fill="currentColor" fill-opacity=".7" font-size="11.5">… and 63 more tiles</text>
<text x="314" y="232" fill="currentColor" fill-opacity=".7" font-size="11.5">256 files</text>
</svg>
<figcaption style="margin-top: 0.5rem; font-size: 0.8125rem; line-height: 1.45; color: var(--muted-foreground)">Tile 2212/1387 in Prague's centre keeps its seven files in the directory of its zoom-9 square, 276/173. The weather table lies beside the year's tiles, the flight tracks among them.</figcaption>
</figure>

| Kind | A file holds | The world |
|---|---|---|
| `terrain` | the ground's height every arc-second (31 m north to south), how much of it is sealed, its tree cover and how tall the trees stand | 202 GB in 7.0 million files |
| `terrain-fine` | where a 5 m national model exists, what the arc-second grid misses, every sixth of an arc-second | 6.2 GB in 15,289 files |
| `obstacles` | the outlines and heights of buildings and walls, listed by cells 49 m wide in Prague | 233 GB in 1.4 million |
| `sources` | every ground source, as points and straight pieces | 229 GB in 2.2 million |
| `aircraft` | a year of flights, summed into boxes of airspace | 674 GB in 2.6 million |
| `aircraft-far` | the same in boxes four times larger, read beyond 3 km | 91 GB in 2.6 million |
| `aircraft-events` | per cell 390 m wide in Prague, the flights of a day whose loudest moment reaches 50, 60 and 70 dB | 17 GB in 1.6 million |

Beside the tiles lie the year's flight tracks, 50 GB in 256 files, read only to draw a listed
flight, and the weather table, 16.6 MB for every half degree.

A sources file holds pieces and the records they share. A piece is a point or a straight line —
its two ends, to 19 cm in Prague — and the number of its record. A record holds what the pieces of
one source have in common: its layer and group, its height above the ground, the ground under it,
its sound power in eight octave bands by day, evening and night, and what the click panel shows of
it, such as a road's vehicles a day, speed and surface. A piece on a bridge also stores the deck's
altitude at its two ends.

<figure style="margin: 1.75rem 0; color: var(--foreground)">
<svg viewBox="0 0 400 240" role="img" aria-label="A road's way drawn as straight pieces, a long segment split in two and the way cut at the tile edge; in the tile's sources file five pieces point to two records, and both records to one text" style="display: block; width: 100%; max-width: 440px; height: auto; margin: 0 auto" font-size="12">
<defs><marker id="hiw-arrow-records" viewBox="0 0 8 8" refX="7" refY="4" markerWidth="6" markerHeight="6" orient="auto"><path d="M0 0L8 4L0 8z" fill="currentColor"/></marker></defs>
<path d="M262 6V122" fill="none" stroke="currentColor" stroke-opacity=".55" stroke-dasharray="4 3"/>
<text x="254" y="16" text-anchor="end" fill="currentColor" fill-opacity=".7" font-size="11.5">tile 2212/1387</text>
<text x="270" y="16" fill="currentColor" fill-opacity=".7" font-size="11.5">2213/1387</text>
<path d="M16 100L64 96L112 88L166 70L220 56L262 50" fill="none" stroke="#d9622b" stroke-width="3" stroke-linejoin="round"/>
<path d="M262 50L316 44L388 40" fill="none" stroke="#d9622b" stroke-opacity=".35" stroke-width="3" stroke-linejoin="round"/>
<g fill="currentColor"><circle cx="16" cy="100" r="2.6"/><circle cx="64" cy="96" r="2.6"/><circle cx="112" cy="88" r="2.6"/><circle cx="166" cy="70" r="2.6"/><circle cx="220" cy="56" r="2.6"/><circle cx="262" cy="50" r="2.6"/><circle cx="316" cy="44" r="2.6"/><circle cx="388" cy="40" r="2.6"/></g>
<g fill="currentColor" fill-opacity=".75" font-size="11" text-anchor="middle"><text x="40" y="91">0</text><text x="87" y="85">0</text><text x="136" y="72">1</text><text x="190" y="56">1</text><text x="240" y="46">0</text></g>
<text x="160" y="108" text-anchor="middle" fill="currentColor" fill-opacity=".7" font-size="11.5">split into pieces ≤ 250 m</text>
<text x="268" y="72" fill="currentColor" fill-opacity=".7" font-size="11.5">cut at the tile edge</text>
<g fill="none" stroke="currentColor" stroke-opacity=".5" marker-end="url(#hiw-arrow-records)"><path d="M46 160Q117 110 187 159"/><path d="M70 160Q129 118 187 159"/><path d="M142 160Q165 142 187 159"/><path d="M94 160Q170 106 245 159"/><path d="M118 160Q182 114 245 159"/><path d="M189 184Q262 222 334 186"/><path d="M247 184Q291 206 334 186"/></g>
<g fill="#d9622b" fill-opacity=".15" stroke="currentColor" stroke-opacity=".6"><rect x="34" y="160" width="24" height="24"/><rect x="58" y="160" width="24" height="24"/><rect x="82" y="160" width="24" height="24"/><rect x="106" y="160" width="24" height="24"/><rect x="130" y="160" width="24" height="24"/></g>
<g fill="currentColor" fill-opacity=".08" stroke="currentColor" stroke-opacity=".6"><rect x="160" y="160" width="58" height="24"/><rect x="218" y="160" width="58" height="24"/><rect x="284" y="160" width="108" height="24"/></g>
<g fill="currentColor" text-anchor="middle" font-size="11.5"><text x="46" y="176">0</text><text x="70" y="176">0</text><text x="94" y="176">1</text><text x="118" y="176">1</text><text x="142" y="176">0</text><text x="189" y="176">record 0</text><text x="247" y="176">record 1</text><text x="338" y="176">text</text></g>
<g fill="currentColor" fill-opacity=".7" text-anchor="middle" font-size="11.5"><text x="94" y="232">pieces, 12 bytes</text><text x="218" y="232">records, 36 bytes</text><text x="338" y="232">texts</text></g>
</svg>
<figcaption style="margin-top: 0.5rem; font-size: 0.8125rem; line-height: 1.45; color: var(--muted-foreground)">A way's straight segments become pieces of at most 250 m, cut where the way leaves the tile. In the tile's sources file the five pieces point to two records, the stretch on a slope having its own, and both records point to one text.</figcaption>
</figure>

| Layer | A piece | A source in the click's list |
|---|---|---|
| Roads | a straight stretch of an OpenStreetMap way, at most 250 m, cut at tile edges | a road by name, else number; with neither, its way |
| Railways | the same along a track | a line by name, else number; with neither, its way (unnamed tram tracks have no route or street identity) |
| Industry | a point, or a point every 75 m over a site above 5,000 m²; a turbine at its hub | a site; a turbine |
| Buildings | a point, or a point every 30 m over a building above 2,000 m² and every 75 m over a pitch or car park above 5,000 m² | a building; a pitch or car park; the guests of a bar; a church's bells |
| Ships | a point every 250 m over a cell of vessel density | a cell |
| Airports | a straight stretch of a runway or taxiway, at most 250 m | an airport's ground operations, in the aircraft row |

Three tiles of central Prague hold 46,543 road pieces, half of them shorter than 21 m and the
longest 250 m; 6,977 railway pieces; 2,938 industrial points and 74,149 points of the buildings
layer. Flights are not pieces: they are the boxes described under Aircraft.

## In the code

- `fetch/` downloads the extra datasets: noise barriers, bus routes, national terrain models,
  wind turbines, venues, WorldClim, places of worship, the TEN-T rail network.
- `build/` (`qm-build`) writes the tiles and every source's sound power. The ground inputs are still
  an earlier version's prepared tables (September 2026, its code on the `qm260904` branch),
  converted by `qm-build dev4` (`build/src/dev4.rs`); builders that read the raw sources replace
  them one input at a time. Its other steps: `weather`, `terrain-fine`, `traffic` (the trips buildings
  make), `airport-traffic`, `geoid`, the aircraft chain and `complete`, the marker written last.
- `physics/` holds the emission models the builders run, with the standards' test cases.
- `tiles/` holds each kind's file format, writer and reader, the only code that knows the bytes;
  `tiles/src/stored.rs` frames a file's records, `tiles/src/read.rs` reads files whole and decodes
  them.

<details>
<summary><strong>What the readers rely on</strong></summary>

| kind | module | contract |
|---|---|---|
| `terrain` | `tiles/src/terrain.rs` | the one-arc-second node window that brackets the tile: neighbours share their seam nodes; a sample touching a node without data is refused, never guessed; height, sealing, tree cover and crown height a node |
| `terrain-fine` | `tiles/src/terrain_fine.rs` | a national model's detail on the 1/6" lattice, its height less its own 1" average, faded towards its edges and voids; blocks of 128 x 128 cells, each one zstd frame behind a directory, so a click decodes only the blocks its rays' ends touch |
| `obstacles` | `tiles/src/obstacles.rs` | outlines stored whole, listed per z19 cell (128 x 128 a tile) through a cell -> offset table |
| `sources` | `tiles/src/sources.rs` | 12-byte pieces, 36-byte records, each distinct emission row (per octave band and period a u16 in 0.01 dB) and display text once, 8-byte deck entries (a bridge piece's deck altitude at its ends) |
| `aircraft`, `aircraft-far` | `tiles/src/aircraft.rs` | the boxes, each with only the periods it is heard in and its two loudest flight pieces, its slab a row of the tile's band table; the tile's flight table; a box finds its periods and pieces from the counts every 32 boxes |
| `aircraft-events` | `tiles/src/aircraft_events.rs` | 16 x 16 z16 cells a tile; no file where no flight reaches 50 dB |
| `aircraft-tracks` | `tiles/src/aircraft_tracks.rs` | 256 files by the address's low byte, each sorted by address and start behind a fence of every 256th part's key, so a listed flight reads one 4 KB page of the directory |
| `weather` | `physics/src/weather.rs` | 0.5 deg nodes: favourable probability per period and 16 sectors in percent, absorption per octave in 0.01 dB/km |

- `tiles::tile_path` names a tile's file in the standard web-map XYZ numbering (y grows
  southwards). Coordinates are int16 relative to the tile centre, step = tile width / 32,768: the
  tile spans +-16,384 steps and half a tile of margin fits on every side.
- Every emission piece lives in exactly one tile: a line is split at tile edges, a point lies in
  the tile holding it, so the sources of a ring never reach into a ring not yet read. An outline is
  stored whole in every tile whose cells it crosses; a wall longer than the frame is clipped into
  parts within it, a ring larger than the frame is left out (the builder counts it). Footprint and
  source ids are global and stable across tiles; holes are kept.
- A release is complete when its builder wrote the completion marker last. Only then does a
  missing file mean "empty"; an unfinished build is never served.
- The physics tables (NPD, CNOSSOS coefficients, the Doc 29 classes) are compiled into the
  programs. Ground rays take the place's absorption; the NPD curves are moved to the place's air
  from the AIR-1845 atmosphere they come in (Doc 29 Appendix D, with the impedance adjustment), a
  box's at the middle of its z9 square; roads roll at the place's yearly air temperature
  (WorldClim 2.1, CNOSSOS-EU Eq. 2.2.10), with the gradient and junction terms.
- Tiles are read whole with plain reads, one task per file, a ring's files asked for together.
  The exceptions read by position: a fine terrain file's blocks on first use, the weather table's
  nodes, and the flight tracks (a file's directory, then each listed flight's points).

</details>

<details>
<summary><strong>The aircraft chain</strong></summary>

Stage 0/1 turns a year of ADS-B traces into flight segments per day (`build/src/aircraft`):
altitudes above EGM2008, each segment's Doc 29 phase and operation (A.3.2), and each flight's class,
decided once: its designator's ANP aircraft (55 classes, `scripts/aircraft-classes.py`), else by its
transponder category or slow flight, else a fallback; surface vehicles are ground traffic.
`aircraft-shuffle` sorts the window's segments into z9 squares once; `aircraft-boxes` builds a
square at a time and gives each segment its power by the force balance with the observed climb and
acceleration (`physics/src/doc29/thrust.rs`; an arrival's flap and gear below the landing
configuration height, B11); a power past the NPD rows reads the edge row, as Doc 29 gives Eq. 4-3
between tabulated powers only. A box is a web-map cell x a clearance slab above the highest terrain
within one edge; the edge grows with clearance so that it spans D = 3 dB of the steepest NPD slope
of the earlier version's 15 classes there, up to 3.8 dB of today's steeper curves (first layer
about 50 m).

A segment is cut at tile edges, then into the pieces of the boxes it crosses; a box sums its pieces
per period at the ten NPD distances (an average day of the window: primary flights over the
baseline days, flights only the secondary provider saw over the increment days) with the geometry
of one average piece, read as two at the gradient plus and minus the spread of its pieces'
gradients, counts its flights and installation mix per period, and keeps its two loudest pieces for
the flight list (by LAmax at their height above the box's ground, one per flight). Far boxes, read
beyond 3 km, span D = 12 dB.

`aircraft-events` counts at every z16 cell each flight once, at its loudest moment there, with the
boxes' day weights. `aircraft-tracks` keeps every flight of the year flat, its segments' ends within
20 m by Douglas-Peucker, broken at the antimeridian and at any step over 300 km. `aircraft-check`
holds the boxes against every segment, the list against the exact ten loudest and the events
against exact counts. Airport ground operations (`build/src/airport`) put the window's ground legs
on the aeroway lines: a leg on a runway rolls at 40 kt or faster or as a take-off roll, and taxis
otherwise; a flight seen low over a runway without a roll of its own gets its class's typical roll.

</details>
