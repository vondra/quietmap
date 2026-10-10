---
title: How it works
intro: How the data are stored, how a click computes the level at its point, and how the map is painted.
nav: hidden
---

Everything on the map is computed from one tree of files. A click reads the tiles around its point and computes the level there on the spot; the map is painted ahead of time from the same files, with the same physics. How each source gets its sound power, from traffic counts, timetables, flight tracks and registries, is on the [methodology](/about/methodology) page.

## The data on disk

The world is cut into the standard web-map tiles of zoom 12, 6.3 km wide in Prague and 9.8 km at the equator. Each tile has one file per kind of data, and the 64 tiles of one zoom-9 square keep their files in one directory. Where a tile holds nothing of a kind, it has no file of that kind: the open sea has no terrain file, a desert without roads no sources file. The world's tree takes 2.8 TB (October 2026).

<figure style="margin: 1.75rem 0; color: var(--foreground)">
<svg viewBox="0 0 400 226" role="img" aria-label="A zoom-9 square of eight by eight tiles with tile 2212/1387 of Prague's centre marked, and the directory holding its six files, beside the weather table and the flight tracks" style="display: block; width: 100%; max-width: 440px; height: auto; margin: 0 auto" font-size="12">
<rect x="8" y="34" width="128" height="128" fill="none" stroke="currentColor" stroke-width="1.2"/>
<path d="M24 34V162M40 34V162M56 34V162M72 34V162M88 34V162M104 34V162M120 34V162M8 50H136M8 66H136M8 82H136M8 98H136M8 114H136M8 130H136M8 146H136" fill="none" stroke="currentColor" stroke-opacity=".3" stroke-width=".75"/>
<rect x="72" y="82" width="16" height="16" fill="#2f7fd8"/>
<text x="8" y="22" fill="currentColor" font-weight="600">z9 square 276/173</text>
<text x="8" y="180" fill="currentColor" fill-opacity=".7">8 × 8 tiles of zoom 12,</text>
<text x="8" y="196" fill="currentColor" fill-opacity=".7">each 6.3 km in Prague</text>
<path d="M165 27V54M165 36H171M165 54H171M181 63V212M181 72H187M181 212H187" fill="none" stroke="currentColor" stroke-opacity=".45"/>
<rect x="198" y="83" width="192" height="100" rx="4" fill="#2f7fd8" fill-opacity=".13"/>
<g fill="currentColor" font-family="ui-monospace, SFMono-Regular, Menlo, Consolas, monospace">
<text x="160" y="22">release/</text>
<text x="176" y="40">weather</text>
<text x="176" y="58">2026/</text>
<text x="192" y="76">276/173/</text>
<g font-size="11.5">
<text x="206" y="99">2212_1387.terrain</text>
<text x="206" y="115">2212_1387.obstacles</text>
<text x="206" y="131">2212_1387.sources</text>
<text x="206" y="147">2212_1387.aircraft</text>
<text x="206" y="163">2212_1387.aircraft-far</text>
<text x="206" y="179">2212_1387.aircraft-events</text>
</g>
<text x="192" y="216">aircraft-tracks/</text>
</g>
<text x="206" y="199" fill="currentColor" fill-opacity=".7" font-size="11.5">… and 63 more tiles</text>
<text x="314" y="216" fill="currentColor" fill-opacity=".7" font-size="11.5">256 files</text>
</svg>
<figcaption style="margin-top: 0.5rem; font-size: 0.8125rem; line-height: 1.45; color: var(--muted-foreground)">Tile 2212/1387 in Prague's centre keeps its six files in the directory of its zoom-9 square, 276/173. The weather table lies beside the year's tiles, the flight tracks among them.</figcaption>
</figure>

| Kind | A file holds | The world |
|---|---|---|
| `terrain` | the ground's height every arc-second (31 m north to south), how much of it is sealed, and its forest cover | 1,109 GB in 7.0 million files |
| `obstacles` | the outlines and heights of buildings and walls, listed by cells 49 m wide in Prague | 233 GB in 1.4 million |
| `sources` | every ground source, as points and straight pieces | 229 GB in 2.2 million |
| `aircraft` | a year of flights, summed into boxes of airspace | 1,025 GB in 2.6 million |
| `aircraft-far` | the same in boxes four times larger, read beyond 3 km | 141 GB in 2.6 million |
| `aircraft-events` | per cell 390 m wide in Prague, the flights of a day whose loudest moment reaches 50, 60 and 70 dB | 17 GB in 1.6 million |

Beside the tiles lie the year's flight tracks, 50 GB in 256 files, read only to draw a listed flight, and the weather table of 16.6 MB: for every half degree of latitude and longitude, how often the weather bends sound down to the ground from each of 16 directions by day, evening and night, and what the air absorbs in each octave band (ERA5, 1991–2020).

### Pieces and records

A sources file holds pieces and the records they share. A piece is a point or a straight line: its two ends, stored to 19 cm in Prague, and the number of its record. A record holds what the pieces of one source have in common: its layer and group, its height above the ground, the ground under it, its sound power in eight octave bands from 63 Hz to 8 kHz for day, evening and night, computed when the release is built, and the fields the click panel shows, such as a road's vehicles a day, speed and surface. Pieces that differ only in where they lie share a record, and records that show the same fields share one copy of them: a stretch of road on a slope has a record of its own, as it emits more, but shows the same text.

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
<g fill="currentColor" fill-opacity=".7" text-anchor="middle" font-size="11.5"><text x="94" y="232">pieces, 12 bytes</text><text x="218" y="232">records, 80 bytes</text><text x="338" y="232">texts</text></g>
</svg>
<figcaption style="margin-top: 0.5rem; font-size: 0.8125rem; line-height: 1.45; color: var(--muted-foreground)">A way's straight segments become pieces of at most 250 m, cut where the way leaves the tile. In the tile's sources file the five pieces point to two records, the stretch on a slope having its own, and both records point to one text.</figcaption>
</figure>

What a piece is depends on the layer. A source in the click panel's list is all the pieces that share a group, across tiles:

| Layer | A piece | A source in the list |
|---|---|---|
| Roads | a straight stretch of an OpenStreetMap way, at most 250 m, cut at tile edges | a road: one name, number and class; a road with neither, its way |
| Railways | the same along a track | a line: one name, number and kind of track |
| Industry | a point, or a point every 75 m over a site above 5,000 m²; a turbine at its hub | a site; a turbine |
| Buildings | a point, or a point every 30 m over a building above 2,000 m² and every 75 m over a pitch or car park above 5,000 m² | a building; a pitch or car park; the guests of a bar; a church's bells |
| Ships | a point every 250 m over a cell of vessel density | a cell |
| Airports | a straight stretch of a runway or taxiway, at most 250 m | an airport's ground operations, in the aircraft row |

Three tiles of central Prague hold 46,543 road pieces, half of them shorter than 21 m, nine in ten shorter than 79 m and the longest 250 m; 6,977 railway pieces (median 23 m); 2,938 industrial points and 74,149 points of the buildings layer. Some sources are parts of one object and share its row: the guests of a bar or restaurant with the building they are in, a church's bells or a mosque's call with its building.

Flights are not pieces. A year of flights is summed into boxes of airspace, each a cell of the web-map grid and a slab of height above the highest ground around it. A box holds its flights' sound energy by day, evening and night at the ten distances of the aircraft noise tables, and its two loudest flights for the list of the loudest flights; boxes grow with their height above the ground. How the flights are modelled is on the [methodology](/about/methodology) page.

## How a click is computed

A click answers for a point 4 m above the ground; inside a building, for its loudest façade ([methodology](/about/methodology)).

### The tiles it reads

The click reads the clicked tile and its eight neighbours, computes, and sends a first answer. Then it reads one more ring of tiles at a time until every ground source within 12 km and every flight within 16 km is covered, sending the whole answer again after each ring, marked as refining until the last. Files are read whole, the files of a ring all at once. In Prague's Vinohrady a click reads 134 files, 295 MB.

<figure style="margin: 1.75rem 0; color: var(--foreground)">
<svg viewBox="0 0 400 268" role="img" aria-label="Seven by seven tiles around a click in Prague: the inner three by three give the first answer, five by five cover the ground sources within 12 km, seven by seven the flights within 16 km" style="display: block; width: 100%; max-width: 440px; height: auto; margin: 0 auto" font-size="12">
<rect x="8" y="8" width="252" height="252" fill="currentColor" fill-opacity=".05"/>
<rect x="44" y="44" width="180" height="180" fill="currentColor" fill-opacity=".1"/>
<rect x="80" y="80" width="108" height="108" fill="currentColor" fill-opacity=".18"/>
<path d="M44 8V260M80 8V260M116 8V260M152 8V260M188 8V260M224 8V260M8 44H260M8 80H260M8 116H260M8 152H260M8 188H260M8 224H260" fill="none" stroke="currentColor" stroke-opacity=".3" stroke-width=".75"/>
<rect x="8" y="8" width="252" height="252" fill="none" stroke="currentColor" stroke-opacity=".6"/>
<circle cx="134" cy="134" r="68.8" fill="none" stroke="currentColor" stroke-width="1.3"/>
<circle cx="134" cy="134" r="91.7" fill="none" stroke="currentColor" stroke-width="1.3" stroke-dasharray="5 4"/>
<circle cx="134" cy="134" r="4" fill="#2f7fd8"/>
<text x="134" y="126" text-anchor="middle" fill="currentColor">click</text>
<text x="134" y="77" text-anchor="middle" fill="currentColor">12 km</text>
<text x="134" y="57" text-anchor="middle" fill="currentColor">16 km</text>
<rect x="276" y="20" width="16" height="16" fill="currentColor" fill-opacity=".3"/>
<rect x="276" y="76" width="16" height="16" fill="currentColor" fill-opacity=".15"/>
<rect x="276" y="132" width="16" height="16" fill="currentColor" fill-opacity=".05" stroke="currentColor" stroke-opacity=".3"/>
<g fill="currentColor"><text x="300" y="33">3 × 3 tiles</text><text x="300" y="89">5 × 5 tiles</text><text x="300" y="145">7 × 7 tiles</text><text x="276" y="204">a tile: 6.3 km</text></g>
<g fill="currentColor" fill-opacity=".7"><text x="300" y="50">first answer</text><text x="300" y="106">ground: 12 km</text><text x="300" y="162">flights: 16 km</text><text x="276" y="221">in Prague</text></g>
</svg>
<figcaption style="margin-top: 0.5rem; font-size: 0.8125rem; line-height: 1.45; color: var(--muted-foreground)">A click in Prague. The first answer comes from the 3 × 3 tiles around it; the ground sources within 12 km take 5 × 5 tiles, the flights within 16 km 7 × 7.</figcaption>
</figure>

### The pieces it computes

Most pieces in those tiles are too far or too quiet to matter, and the click proves it without computing them. For each piece it takes an upper bound of what could arrive: its sound power spread over its shortest distance to the point and absorbed by the air over that distance only, plus 18 dB, the most that ground and screening can add under CNOSSOS-EU in either weather, plus the façades' reflection. It then computes the pieces in full, the loudest bound first, and stops when the bounds of everything left could not add 0.1 dB to any layer by day, evening or night. A layer that would need more than 1,024 pieces for that, as in a city, samples the rest instead: it draws pieces in proportion to their bounds, 512 at first and up to 32,768, until two standard errors of the estimate fall under 0.05 dB. The draws are seeded by the click, so a click always gives the same answer.

### A piece and its rays

A point sends one ray. A line piece is summed as CNOSSOS-EU sums a line of points: the angle under which the point sees the piece is cut into five equal parts, and each part sends one ray from its stretch of the piece. Five rays match a sum of 401 points within 0.1 dB on a 250 m piece. Where a part spans more than 3° of direction and buildings stand in front of the piece, the part is split at their edges as the point sees them, and every hidden stretch and every gap gets rays of its own, each weighted by its angle: a street is heard through the gaps between its houses. Seen from the village of Kytín, the D4 motorway 2.1 km away is 492 pieces, and the loudest of them has five rays.

<figure style="margin: 1.75rem 0; color: var(--foreground)">
<svg viewBox="0 0 400 270" role="img" aria-label="A line piece seen from above by the point R: five rays at equal angles where the view is open, and more rays where two houses hide parts of the piece" style="display: block; width: 100%; max-width: 440px; height: auto; margin: 0 auto" font-size="12">
<path d="M200 250L40 60M200 250L115.1 60M200 250L173.2 60M200 250L226.8 60M200 250L284.9 60M200 250L360 60" fill="none" stroke="currentColor" stroke-opacity=".25" stroke-dasharray="3 3"/>
<g fill="currentColor" fill-opacity=".07"><polygon points="224,128 237.4,60 276,60 240,150 240,128"/><polygon points="262,150 317.8,60 360,60 284.2,150"/></g>
<g fill="currentColor" fill-opacity=".2" stroke="currentColor" stroke-opacity=".7"><rect x="224" y="128" width="16" height="22"/><rect x="262" y="150" width="28" height="18"/></g>
<path d="M40 60H237.4M276 60H317.8" fill="none" stroke="#d9622b" stroke-width="3.5" stroke-linecap="round"/>
<path d="M237.4 60H276M317.8 60H360" fill="none" stroke="#d9622b" stroke-opacity=".4" stroke-width="3.5" stroke-linecap="round"/>
<path d="M200 250L80.9 60M200 250L145.4 60M200 250L200 60M200 250L232.1 60M200 250L256.3 60M200 250L280.3 60M200 250L300.6 60M200 250L338 60" fill="none" stroke="#2f7fd8" stroke-width="1.4"/>
<circle cx="200" cy="250" r="4" fill="currentColor"/>
<text x="210" y="262" fill="currentColor" font-weight="600">R</text>
<text x="40" y="48" fill="currentColor">piece, ≤ 250 m</text>
<g fill="currentColor" fill-opacity=".75"><text x="16" y="150">open view:</text><text x="16" y="166">a ray per part</text><text x="392" y="206" text-anchor="end">behind houses:</text><text x="392" y="222" text-anchor="end">a ray per hidden</text><text x="392" y="238" text-anchor="end">stretch and gap</text></g>
</svg>
<figcaption style="margin-top: 0.5rem; font-size: 0.8125rem; line-height: 1.45; color: var(--muted-foreground)">Seen from above, R the click's point. The angle under which R sees the piece is cut into five equal parts, one ray each. Two houses hide stretches of the right-hand parts, so those parts get a ray for each hidden stretch and each gap.</figcaption>
</figure>

### One ray

Along each ray the click samples the ground: 10 m from each end, where a road's embankment would stand, then three steps each of 31, 61 and 123 m from both ends, and steps of 246 m through the middle. Each sample gives the terrain's height and how hard the ground is, from soft soil to sealed ground and water. Every wall the ray crosses stands at its height, and a building's roof joins its walls as hard ground. Over this cross-section CNOSSOS-EU takes:

- the spreading of the sound with distance;
- the air's absorption in each octave band over the ray's length, averaged over 30 years of the place's weather;
- diffraction over the edges that block the line of sight, a hilltop, a roof or a wall: once in calm air, where sound travels straight, and once downwind, where it bends toward the ground along an arc of at least 1 km radius;
- the ground's effect, on either side of those edges or over the whole ray;
- how often the weather bends sound down in this ray's direction by day, evening and night, which mixes the calm and the downwind result;
- the façades around the point, which add up to 3 dB.

<figure style="margin: 1.75rem 0; color: var(--foreground)">
<svg viewBox="0 78 400 150" role="img" aria-label="A ray seen from the side: dense ground samples near both ends, a soft hill and a house; in calm air the sound bends over the hilltop and the roof's edge in straight lines, downwind along arcs bent toward the ground" style="display: block; width: 100%; max-width: 440px; height: auto; margin: 0 auto" font-size="12">
<path d="M8 180H44L90 150L150 112L196 150L240 176H286V120H326V176L384 172H392V196H8Z" fill="currentColor" fill-opacity=".07"/>
<path d="M8 180H44L90 150L150 112L196 150L240 176H286V120H326V176L384 172H392" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linejoin="round"/>
<path d="M44 180L90 150L150 112L196 150L240 176" fill="none" stroke="#3e9b57" stroke-width="3.5" stroke-linejoin="round"/>
<path d="M286 120H326" fill="none" stroke="currentColor" stroke-width="3.5"/>
<path d="M22 176L150 112L326 120L380 148" fill="none" stroke="#2f7fd8" stroke-width="1.6" stroke-linejoin="round"/>
<path d="M22 176Q78 132 150 112Q238 98 326 120Q356 125 380 148" fill="none" stroke="#2f7fd8" stroke-width="1.6" stroke-dasharray="5 3"/>
<circle cx="22" cy="176" r="3.5" fill="#d9622b"/>
<circle cx="380" cy="148" r="3.5" fill="currentColor"/>
<path d="M380 172V152" fill="none" stroke="currentColor" stroke-opacity=".6" stroke-dasharray="2 2"/>
<g fill="currentColor"><text x="12" y="166" font-weight="600">S</text><text x="386" y="142" font-weight="600">R</text><text x="374" y="166" text-anchor="end">4 m</text><text x="236" y="92" text-anchor="middle">downwind</text><text x="236" y="136" text-anchor="middle">calm air</text><text x="306" y="160" text-anchor="middle">house</text></g>
<text x="118" y="170" text-anchor="middle" fill="currentColor" fill-opacity=".75">soft ground</text>
<path d="M22 206H380M22 202V210M26 202V210M31 202V210M37 202V210M44 202V210M52 202V210M62 202V210M76 202V210M94 202V210M118 202V210M150 202V210M201 202V210M252 202V210M284 202V210M308 202V210M326 202V210M340 202V210M350 202V210M358 202V210M365 202V210M371 202V210M376 202V210M380 202V210" fill="none" stroke="currentColor" stroke-opacity=".6"/>
<text x="201" y="223" text-anchor="middle" fill="currentColor" fill-opacity=".75">ground samples</text>
</svg>
<figcaption style="margin-top: 0.5rem; font-size: 0.8125rem; line-height: 1.45; color: var(--muted-foreground)">A ray seen from the side, from the source S to the point R. The ground is sampled densely near both ends; the hill is soft ground, the roof hard. In calm air the sound bends over the hilltop and the roof's edge in straight lines; downwind it follows arcs bent toward the ground and loses less.</figcaption>
</figure>

### From rays to the answer

A piece's rays add up to the piece, each weighted by its angle; the pieces of a group add up to the source; the sources to their layer and the layers to the place, every sum by octave band and by day, evening and night, which give Lden. The sources' traffic then tells how the level spreads over time, how many cars pass in an hour and how often a train comes, and from that come the levels exceeded 5 to 90 % of the time and Nden, how loud the place sounds over the day in sone ([methodology](/about/methodology), Time and loudness). A row's share of Nden comes from dividing every moment's loudness among the sources heard in it, so the shares add up to 100 %.

The click panel opens these sums one level at a time:

| In the click panel | In the computation |
|---|---|
| Nden and Lden | the place, every layer together |
| A row | a source, or an object of several, with its share of Nden |
| Breakdown | an object's parts; for the aircraft, the kinds of flight and the airports |
| Sound path | the source's rays summed after each term, in calm air and downwind |
| Segments | its pieces, every one computed again in full, and how its sound arrives |
| A segment's rays | one piece's rays, each with the ground and walls it was computed over |
| Detailed calculation | the receiver, each period's levels and loudness, the layers, the weather |

Opened at Kytín, the D4's segments show that in calm air 19 % of its sound arrives over a free line of sight, 31 % bent over buildings and 50 % over terrain.

## How the map is painted

The map shows the level a click would give at every pixel. It is painted ahead of time, one zoom-12 tile at a time at 1,024 × 1,024 pixels (6 m in Prague), from the same files and with the click's physics. A ground source counts as loud at a pixel where its upper bound there reaches 20 dB. Each pixel takes:

- the loud sources that cross its block of 16 × 16 pixels or one of the eight blocks around it, each computed exactly at the pixel;
- every other loud source blended from where it was computed exactly, its near part at the corners of the blocks and its far part at a lattice every 64 pixels, in groups by layer, by 10° of direction and by distance doubling from 100 m. A group holding at least 0.3 % of its layer's sound has its loudest piece computed exactly at the pixel, which scales the whole group, so a house's shadow falls on the right pixels;
- the hum: the sources under 20 dB there, estimated as a click estimates them, at points 128 pixels apart, and blended. It is never dropped.

<figure style="margin: 1.75rem 0; color: var(--foreground)">
<svg viewBox="0 0 400 372" role="img" aria-label="A part of a painted tile around pixel P: road A crosses the blocks around P and is computed exactly at P; road B is blended from the block corners and the lattice points and scaled by its loudest piece computed at P, past a house; the hum comes from points 128 pixels apart" style="display: block; width: 100%; max-width: 440px; height: auto; margin: 0 auto" font-size="12">
<rect x="40" y="136" width="96" height="96" fill="#2f7fd8" fill-opacity=".14"/>
<path d="M40 8V264M72 8V264M104 8V264M168 8V264M200 8V264M232 8V264M8 40H264M8 72H264M8 104H264M8 168H264M8 200H264M8 232H264" fill="none" stroke="currentColor" stroke-opacity=".16" stroke-width=".75"/>
<path d="M136 8V264M8 136H264" fill="none" stroke="currentColor" stroke-opacity=".45"/>
<rect x="8" y="8" width="256" height="256" fill="none" stroke="currentColor" stroke-opacity=".6"/>
<path d="M14 250L120 120" fill="none" stroke="#d9622b" stroke-width="3" stroke-linecap="round"/>
<path d="M318 18L365 262" fill="none" stroke="#d9622b" stroke-width="3" stroke-linecap="round"/>
<rect x="196" y="160" width="22" height="18" fill="currentColor" fill-opacity=".2" stroke="currentColor" stroke-opacity=".7"/>
<path d="M344 154L90 184" fill="none" stroke="#2f7fd8" stroke-width="1.6"/>
<g fill="currentColor"><circle cx="72" cy="168" r="2.4"/><circle cx="104" cy="168" r="2.4"/><circle cx="72" cy="200" r="2.4"/><circle cx="104" cy="200" r="2.4"/></g>
<g fill="currentColor"><rect x="5" y="5" width="6" height="6"/><rect x="133" y="5" width="6" height="6"/><rect x="261" y="5" width="6" height="6"/><rect x="5" y="133" width="6" height="6"/><rect x="133" y="133" width="6" height="6"/><rect x="261" y="133" width="6" height="6"/><rect x="5" y="261" width="6" height="6"/><rect x="133" y="261" width="6" height="6"/><rect x="261" y="261" width="6" height="6"/></g>
<g fill="none" stroke="currentColor" stroke-width="1.3"><circle cx="8" cy="8" r="7"/><circle cx="264" cy="8" r="7"/><circle cx="8" cy="264" r="7"/><circle cx="264" cy="264" r="7"/></g>
<rect x="86" y="180" width="8" height="8" fill="#2f7fd8"/>
<g fill="currentColor" font-weight="600"><text x="98" y="196">P</text><text x="34" y="250">A</text><text x="330" y="36">B</text></g>
<rect x="8" y="280" width="14" height="14" fill="#2f7fd8" fill-opacity=".14" stroke="#2f7fd8" stroke-opacity=".5"/>
<circle cx="15" cy="305" r="2.4" fill="currentColor"/>
<rect x="12" y="320" width="6" height="6" fill="currentColor"/>
<path d="M8 341H22" fill="none" stroke="#2f7fd8" stroke-width="1.6"/>
<circle cx="15" cy="359" r="6" fill="none" stroke="currentColor" stroke-width="1.3"/>
<g fill="currentColor"><text x="30" y="291">exact: loud sources in P's 3 × 3 blocks</text><text x="30" y="309">block corners (16 px, 100 m): near parts</text><text x="30" y="327">lattice points (64 px, 390 m): far parts</text><text x="30" y="345">probe: a group's loudest piece, exact at P</text><text x="30" y="363">hum points (128 px, 785 m): quiet sources</text></g>
</svg>
<figcaption style="margin-top: 0.5rem; font-size: 0.8125rem; line-height: 1.45; color: var(--muted-foreground)">Part of a tile painted at 1,024 pixels, distances in Prague. Road A crosses the blocks around pixel P and is computed exactly there. Road B is computed at the block corners and the lattice points, blended, and scaled by its group's loudest piece computed exactly at P, whose ray here passes a house. The quiet sources come from the hum points.</figcaption>
</figure>

Near the edge of a lattice cell a pixel blends the groups of the cells on both sides, so the grouping never switches at an edge. The flights are blended from the block corners, every ground source takes the pixel's façade reflection, and a pixel inside a building has no level: the map shows the sound outdoors. Each layer and the total are stored in steps of half a decibel; the coarser zooms are energy means of the pixels under them.

The exact computations run on a graphics card in single precision, or on the processor's cores in double precision; on 40,000 pairs of source and pixel in Prague the two agree within 0.001 dB for 99 % of them. The painter is held to a reference, the etalon: the same tile with every loud source computed exactly at every pixel, and the same hum. On four tiles around Dobříš the painted total differs from the etalon by more than 1 dB at 0.05 % of 4 million pixels, and at 4,000 random pixels there the etalon's ground layers above 30 dB match a click within 0.5 dB at all but two. Flights are not yet checked this way: the test tiles have none.
