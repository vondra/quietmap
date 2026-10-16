---
title: Noise at a point
intro: What a click shows, and how the noise at its point is computed from every source around it.
---

Click anywhere and the noise there is computed on the spot, from every road, railway, factory, wind
turbine, building, ship and airport within 12 km and every flight within 16 km. The first answer
comes from the tiles around the point and is refined, ring of tiles by ring, until everything in
reach is heard. A click always gives the same answer.

## What the click shows

The panel leads with Nden, how loud the place sounds over the day in sone, then Lden. Below are the
sources heard there, loudest first — a road as one row, a factory, the aircraft — each with how
often it passes (vehicles an hour by day, trains a day, flights a day above 50 dB, how many times a
day bells ring or a call sounds; nothing for a steady source), how far it is, its loudness alone in
sone and its share of the loudness.

The list shows the loudest three and every other row contributing at least 1 % of the loudness,
up to ten rows, on a phone too. Shares retain their precision through the popup's answer; only the
display rounds them to whole percents.
Everything else folds into Rest, which opens to the folded rows among the 30 loudest sources the
click sends and holds the quieter ones as one sum. Roads and railways join by name, or by number
where unnamed. Unnamed tram tracks without a number stay separate: the popup's records
carry no route or street identity, and connected tracks can belong to different lines. Joining
rows changes the list; every part keeps its own noise computation.

Open a source for its data (vehicles a day and their mix, speed, a surface other than asphalt;
trains a day; the flights and their types) and what distance, air, buildings, terrain, ground and
trees take from its sound and the buildings around the point add, in calm air and downwind. Its
segments show the rays they were computed along, drawn on the map. The ten loudest flights are
listed. Detailed calculation is the whole place: each period and layer, the weather,
the façade the click is answered at, and the levels exceeded 5 to 90 % of the time.

<p>
<img src="click.jpg" alt="A click on náměstí Míru in Prague: the level and its sources, the loudest road opened with its data and sound path" style="display:inline-block;width:300px;max-width:48%;vertical-align:top;margin:0 12px 0 0">
<img src="calculation.jpg" alt="The same click's detailed calculation: the receiver, each period, each layer and the weather" style="display:inline-block;width:300px;max-width:48%;vertical-align:top;margin:0">
</p>

## What the numbers mean

Nden is how loud the place sounds on average over the day, in sone: 2 sone sound twice as loud as
1, about a quiet room. A sound counts by how loud it is and how long it lasts — a car every few
hours adds little, a constant hum all of its loudness — and the evening counts 5 dB and the night
10 dB louder, as in Lden.

Lden is the day-evening-night level of the EU's noise directive
([END 2002/49/EC](https://eur-lex.europa.eu/eli/dir/2002/49/oj/eng)), the main indicator of the
EU's strategic noise maps: the year's average in decibels, the evening counted 5 dB and the night
10 dB louder. Day is 07–19 h, evening 19–23 h, night 23–07 h. The
[WHO's 2018 guidelines](https://www.who.int/europe/publications/i/item/9789289053563) recommend
road traffic below 53 dB, railways below 54 dB and aircraft below 45 dB.

```
Lden = 10 × log₁₀((12 × 10^(Ld/10) + 4 × 10^((Le+5)/10) + 8 × 10^((Ln+10)/10)) / 24)
```

Natural sound — wind, water, birds — is not included: where human-made sound is faint, a place
sounds louder than the map says.

## Where the point is

A click answers 4 m above the ground, as the END prescribes. Inside a building it answers at the
point of its façades loudest by Lden from the 100 pieces of ground sources with the highest bound
at the building (flights do not choose it), placed as CNOSSOS-EU places receivers: every 5 m or
less along each façade, 0.1 m in front of the wall and 4 m up, the façade's own reflection left
out. A building with no exposed façade gets no answer. No indoor attenuation is applied.

## How it is computed

### The tiles it reads

The click reads the clicked tile and its eight neighbours, computes and sends a first answer. Then
it reads one more ring of tiles at a time until every ground source within 12 km and every flight
within 16 km is covered, sending the whole answer again after each ring. At náměstí Míru in Prague a
click reads 127 tiles from seven files, one a kind of data, and nine files more, the flight tracks
and the weather: 131 MB.

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

Most pieces in those tiles are too far or too quiet to matter, and the click proves it without
computing them. For each piece it takes an upper bound of what could arrive: its sound power over
its shortest distance to the point, absorbed by the air over that distance only, plus 18 dB, the
most that ground and screening can add under CNOSSOS-EU in either weather, plus the façades'
reflection. It computes the pieces in full, the loudest bound first, and stops when everything left
could not add 0.1 dB to any layer by day, evening or night. A layer that would need more than 1,024
pieces, as in a city, samples the rest instead: it draws pieces in proportion to their bounds, 512
at first and up to 32,768, until two standard errors fall under 0.05 dB. The draws are seeded by
the click.

### A piece and its rays

A point sends one ray. A line piece is summed as CNOSSOS-EU sums a line of points: the angle under
which the point sees it is cut into five equal parts, each sending one ray; a piece seen under less
than 3° sends one. Five rays match a sum of 401 points within 0.1 dB on a 250 m piece. Where
buildings stand in front of a part spanning 3° or more, the part is split at their edges and every
hidden stretch and every gap gets rays of its own, one for every 15° of it: a street is heard
through the gaps between its houses.

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

Along each ray the click reads the ground wherever the ray crosses a line of the one-arc-second
terrain grid (31 m north to south, 20 m east to west in Prague), and within 100 m of either end on a
grid six times finer where a national model that fine exists:
the D4's berm at Mníšek pod Brdy, 7 m high, takes 3.5 dB off the motorway at a point 70 m from its
axis, about 20 m behind the crest, which the coarser grid missed. Each sample gives the terrain's
height, how hard the ground is, and the trees' cover and height; every wall the ray crosses stands
at its height. Over this cross-section the click takes:

- the spreading of the sound with distance;
- the air's absorption in each octave band (ISO 9613-1), averaged over 30 years of the place's
  weather;
- diffraction over the edges that block the line of sight — a hilltop, a roof, a wall — once in
  calm air and once downwind, where sound bends toward the ground along an arc of at least 1 km
  radius;
- the ground's effect, on either side of those edges or over the whole ray;
- the tree crowns the ray passes through (ISO 9613-2, Annex A, which CNOSSOS-EU leaves out): 0.02 dB
  a metre at 63 Hz to 0.12 dB at 8 kHz over at most 200 m, weighted by the tree cover;
- how often the weather bends sound down in the ray's direction by day, evening and night, which
  mixes the calm and the downwind result;
- the façades around the point: the share of a 150 m square around it covered by buildings
  taller than 5 m, read on a fixed fine grid. It is the bonus the earlier nine-point rule gives on
  average if its points fall independently anywhere in that window: about 0.9 dB at one fifth
  coverage, 2.2 dB at one half and 3 dB at full coverage. Window edges count their fraction of each
  grid cell; the receiver's own building is left out. Flights take no bonus. Reflections are not traced.

<figure style="margin: 1.75rem 0; color: var(--foreground)">
<svg viewBox="0 78 400 150" role="img" aria-label="A ray seen from the side: ground samples at every line of the terrain grid, a soft hill and a house; in calm air the sound bends over the hilltop and the roof's edge in straight lines, downwind along arcs bent toward the ground" style="display: block; width: 100%; max-width: 440px; height: auto; margin: 0 auto" font-size="12">
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
<path d="M22 206H380M22 202V210M39.9 202V210M57.8 202V210M75.7 202V210M93.6 202V210M111.5 202V210M129.4 202V210M147.3 202V210M165.2 202V210M183.1 202V210M201 202V210M218.9 202V210M236.8 202V210M254.7 202V210M272.6 202V210M290.5 202V210M308.4 202V210M326.3 202V210M344.2 202V210M362.1 202V210M380 202V210" fill="none" stroke="currentColor" stroke-opacity=".6"/>
<text x="201" y="223" text-anchor="middle" fill="currentColor" fill-opacity=".75">ground samples</text>
</svg>
<figcaption style="margin-top: 0.5rem; font-size: 0.8125rem; line-height: 1.45; color: var(--muted-foreground)">A ray seen from the side, from the source S to the point R. The ground is read at every line of the terrain grid; the hill is soft ground, the roof hard. In calm air the sound bends over the hilltop and the roof's edge in straight lines; downwind it follows arcs bent toward the ground and loses less.</figcaption>
</figure>

Every term is computed in eight octave bands from 63 Hz to 8 kHz. At 392 Polish road monitoring
stations behind forest the model read a median 2.4 dB above the measurement without the crowns'
term and 0.3 dB with it; at the 5,195 without forest it stays 0.2 dB above.

### From rays to the answer

A piece's rays add up to the piece, the pieces to the source, the sources to their layer and the
layers to the place, by octave band and by day, evening and night. That gives Lden.

The map does not know when a car or a plane passed; it knows how many pass and how fast: a road's
vehicles a day, spread over the hours by measured hourly profiles, a railway's trains by period,
from timetables where they exist, a flight path's flights by period.
Each is a stream of passes at random times, and the source's distance and speed tell how much of
the time the level spends at each value (Kurze's statistics of moving sources). A road with 600
vehicles an hour at 50 km/h is a series of passes 20 m away and a steady sound from about 80 m; one
flight a day 1 km away is near its peak for about half a minute. Sources add up only within the
same moment, so the sum is taken for every hour of road traffic and ten states of the weather; bells
and calls sound in their share of the period, industry, buildings and ships are steady. That gives
the levels exceeded 5 to 90 % of the time.

Nden is every moment's loudness after ISO 532-1 (Zwicker), from the spectrum received in that
period, averaged over the day. A source's share is its part of every moment's loudness by its
energy then, so the shares add up to 100 %; its own Nden, as if it sounded alone, does not add up:
two equal roads each sound about 0.8 of their sum.

The panel opens these sums one level at a time:

| In the panel | In the computation |
|---|---|
| Nden and Lden | the place, every layer together |
| A row | a source, or an object of several, with its share of Nden |
| Breakdown | an object's parts; for the aircraft, the kinds of flight and the airports' ground operations |
| Sound path | the source's rays summed after each term, in calm air and downwind |
| Segments | its pieces, each computed again in full, and how its sound arrives |
| A segment's rays | one piece's rays, each with the ground and walls it was computed over |
| Detailed calculation | the receiver, each period's levels and loudness, the layers, the weather |

## Standards

- [CNOSSOS-EU](https://eur-lex.europa.eu/eli/dir_del/2021/1226): road emission and the typical
  trains the railway spectra are set to; propagation; receivers at façades.
- IEC 61400-11: the wind turbines' sound power.
- [ECAC Doc 29](https://www.ecac-ceac.org/activities/environment/european-aviation-and-environment-working-group-eaeg/airmod)
  with [EASA ANP](https://www.easa.europa.eu/en/domains/environment/policy-support-and-research/aircraft-noise-and-performance-anp-data)
  data: aircraft.
- ISO 9613-1: the air's absorption; ISO 9613-2, Annex A:
  the tree crowns.
- [ISO 532-1](https://www.iso.org/standard/63077.html): loudness.
- [END 2002/49/EC](https://eur-lex.europa.eu/eli/dir/2002/49/oj/eng): Lden and the receiver's
  height.

The model follows these standards with the simplifications described here and is not certified
against them: an engineering estimate for comparison and exploration, not a measurement, a legal
noise map or a property certificate.

## In the code

`popup/` (`qm-popup`) answers a click:

- `answer.rs`: the ring loop, until every kind's reach is covered (`candidates::GROUND_REACH_M`,
  `aircraft::boxes::AIRCRAFT_REACH_M`); an update after every ring.
- `release.rs`: a ring's tiles, each read whole from its square file (`tiles/src/square.rs`).
- `candidates.rs`: every piece of the files read, with its bound (`physics/src/bound.rs`).
- `selection.rs`, `sampling.rs`: the stop rule and the sample's rounds, one omitted-energy account
  per layer and period.
- `scene.rs`: the ground under a point and along a ray, at every line of the terrain grid it crosses.
- `evaluate.rs`: one source at the receiver, its rays through `physics/src/ray.rs`, `line.rs`,
  `cnossos/` and `foliage.rs`, and its sound path.
- `building.rs`: a click inside a building, answered at its loudest façade.
- `obstacles/reflection.rs`, `obstacles/reflection_coverage.rs`: the same covered area for click
  and painter, tabulated only where the bounds of tall footprints and reachable receiver windows
  intersect; `physics/src/reflection.rs` turns its share into the receiver bonus.
- `aircraft/`: the boxes through the click-time equation, the flight list, the events, the tracks.
- `rows.rs`, `percentiles.rs`, `distribution.rs`, `loudness.rs`, `shares.rs`, `lines.rs`,
  `listing.rs`, `json.rs`: the rows, the levels over time, Nden, the shares, the lines on the map,
  the listed pieces and the update as one line of JSON.

`server/` runs `qm-popup` once per click (two at once, two waiting per slot, 503 when full, stopped
after 30 s or when the visitor leaves) and forwards its lines as they come; `frontend/` redraws the
panel on every line.

The targets, measured with `bench/cold.py` and `bench/report.py` from a cold disk: the first answer
within 0.2 s and the full answer within 0.5 s from an NVMe disk (0.5 s and 1.5 s from a hard disk);
each layer's Lden within 0.1 dB of the full computation in 95 % of the layer answers and 0.3 dB in
all. Each update stays under 100 KB, held by caps on the lines and tracks it carries.

<details>
<summary><strong>What the click guarantees</strong></summary>

- The bound stays an upper bound of `ray` and `line`: a source is skipped only by the
  omitted-energy account, never by a per-source threshold.
- The sample is seeded by the click, so a click always answers the same.
- Until every ring is read the answer says it is partial; each update is the whole answer so far,
  one line of JSON in one streamed HTTP response. A failed read is an error, never a quieter
  answer. Exact mode (`--exact 1`, benchmark only) is the same loop with the stop rule off.
- A building's source is never screened by a footprint containing it (its own, or another outline
  of the same building).
- A ray tangent to a building corner keeps its top as an obstacle, but does not enter or leave a
  roof: a tangent cannot make another footprint count as containing the source.

</details>
