---
title: Noise at a point
intro: How loud a place is, what makes its noise, and how the popup computes it.
---

<!-- SECTIONS -->

Click any point on the map and the popup tells how loud it is there and what makes the noise. On
Stockwell Street in Greenwich, London, it reads 19 [sone](#what-the-numbers-mean), and two main
roads 28 m away make most of it.

The popup computes the point on the spot from every source within 12 km: roads, railways,
factories, wind turbines, buildings, ships and airports. Flights count within 16 km. A first answer
shows at once and is refined until every source in reach is heard. The same point always gives the
same answer.

## What the popup shows

The popup leads with how loud the place sounds, then lists the sources heard there, the largest
share first. A row opens to the source's data and how its sound arrives, and highlights it on the
map. An industrial site shows its mapped boundary where one is available. At Greenwich, October 2026:

| In the popup | What it tells |
|---|---|
| 19 sone Nden | How loud the place sounds over the day |
| 66.5 dB Lden | The level in decibels, the year's average |
| Greenwich Church Street | A source heard there: a road as one row, a factory, the aircraft |
| 590 veh/h | How often it passes: vehicles an hour by day, trains a day, flights a day above 50 dB, how many times a day bells ring, a call sounds or a car parks or leaves; nothing for a steady source |
| 28 m | How far it is |
| 39 % | Its share of what is heard there; the shares add up to 100 % |
| Speed 48 km/h, Traffic 8.8k/day | The source's data: vehicles a day and their mix, speed, a surface other than asphalt; trains a day; the flights and their types |
| Sound path: Level 62.2 | What distance, air, buildings and terrain, ground and trees take from its sound, and what the buildings around the point add, in calm air and downwind |
| Alone: 14 sone | How loud it would sound alone |
| Segments | Its pieces, with the rays they were computed along drawn on the map; a ray opens to its terrain profile |
| Flights a day, Loudest flights | In the aircraft row: the flights of an average day whose loudest moment here reaches 50, 60 and 70 dB, and the ten loudest flights |
| Detailed calculation | The whole place: each period and layer, the weather, the façade the point is answered at, and the levels exceeded 5 to 90 % of the time |

<p>
<img src="click.jpg" width="1500" height="880" alt="The popup at Greenwich in London: the level and its sources by share, the largest road opened with its data, sound path and loudness alone" style="display:inline-block;width:300px;max-width:48%;vertical-align:top;margin:0 12px 0 0">
<img src="calculation.jpg" width="1500" height="1390" alt="The same point's detailed calculation: the receiver, each period, each layer and the weather" style="display:inline-block;width:300px;max-width:48%;vertical-align:top;margin:0">
</p>

<details>
<summary>Which rows are listed, and why a share is not a loudness</summary>

- **The list** shows the three largest shares and every other row of at least 1 %, up to ten rows,
  on a phone too. Everything else folds into Rest, which opens to the folded rows among the 30
  largest and holds the smaller ones as one sum. A share keeps its precision through the answer;
  only the display rounds it to whole percents.
- **In Lden** (the switch above the map's layers) the popup leads with Lden, the same 30 rows are
  ranked by their Lden and each shows its share of the place's sound energy; Rest holds the energy
  of everything else. At about one place in a hundred a source of 1 to 2 % of the energy is not among
  the 30 and stays in Rest (297 of the validation points, October 2026).
- **A row** is a road or a railway by its name, or by its number where it has no name; unnamed tram
  tracks without a number stay apart, as connected tracks can belong to different lines. Joining
  rows changes the list, not the computation: every part keeps its own.
- **Share and loudness alone.** Loudness does not add up: two equal roads each sound about 0.8 of
  their sum. A row's share is its part of every moment's loudness, by its energy at that moment, so
  the shares add up to 100 %. How much of its own loudness a source keeps in the mix depends on
  whether it rises above the rest. At a point in Prague, a road 1.5 km away and the day's flights
  sounded 1.2 sone alone each; the road, steady and 9 dB under the background, kept a fifth of it
  (6 %), the flights, rising above the background as they passed, half (12 %).

</details>

## What the numbers mean

Nden is how loud the place sounds over the day, in sone: 2 sone sound twice as loud as 1, about a
quiet room. A sound counts by how loud it is and how long it lasts. A car every few hours adds
little, a constant hum all of its loudness. As in Lden, the evening counts 5 dB and the night 10 dB
louder.

Lden is the day-evening-night level of the EU's noise directive
([<abbr title="the Environmental Noise Directive">END</abbr> 2002/49/EC](https://eur-lex.europa.eu/eli/dir/2002/49/oj/eng)),
the main indicator of the EU's strategic noise maps. It is the year's average in decibels, the
evening counted 5 dB and the night 10 dB louder. Day is 07–19 h, evening 19–23 h, night 23–07 h. The
[WHO's 2018 guidelines](https://www.who.int/europe/publications/i/item/9789289053563) recommend
road traffic below 53 dB, railways below 54 dB and aircraft below 45 dB.

<details>
<summary>The Lden formula</summary>

```
Lden = 10 × log₁₀((12 × 10^(Ld/10) + 4 × 10^((Le+5)/10) + 8 × 10^((Ln+10)/10)) / 24)
```

</details>

Natural sound — wind, water, birds — is not included: where human-made sound is faint, a place
sounds louder than the map says.

## The weather

Wind blowing from a road towards you bends its sound back down over hills and walls. Cold air lying
near the ground on a clear night or morning does the same. At High Beach in Epping Forest the M25,
2 km away, sounds 17 dB louder then.

The level is the year's average. The popup counts how often the weather bends sound down, from the
climate of 1991–2020, for each direction by day, evening and night. One particular day cannot be
shown, nor the day of the week: each hour's traffic is the median of all days.

Wind blowing the other way, or ground warmed on a sunny afternoon, bends sound up and away. The
model leaves that out, so a place can then be quieter than the popup ever says.

### How to read it in the popup

A source's row opens to its sound path, computed twice: in calm air (Calm) and with its sound bent
down (Downwind). The last row, Share of time, tells how often each holds, by day · evening · night.
The M25 at High Beach, October 2026:

| Sound path, dB Lden | Calm | Downwind |
|---|---:|---:|
| Distance only | 61.8 | 61.8 |
| Air absorption | −12.0 | −12.0 |
| Buildings and terrain | −18.0 | −1.1 |
| Ground effect | −2.9 | +0.7 |
| Trees | −6.6 | −10.1 |
| Level | 22.3 | 39.2 |
| Share of time | 69&nbsp;·&nbsp;63&nbsp;·&nbsp;57&nbsp;% | 31&nbsp;·&nbsp;37&nbsp;·&nbsp;43&nbsp;% |

Bent down, the sound passes over the buildings and the terrain in its way: they take 1.1 dB instead
of 18.0. The M25's sound is bent down 31 % of the day, 37 % of the evening and 43 % of the night.
Mixed in these shares, the two levels give the 35.2 dB Lden its row shows.

The shares come from the detailed calculation's Weather, the place's weather for every source
around it. Its table Sound from tells, for sound arriving from each of 16 directions, how often it is
bent down. The M25's loudest pieces lie from north-north-west to north-north-east of High Beach:

| Sound from | Day % | Evening % | Night % |
|---|---:|---:|---:|
| NNW | 32 | 38 | 46 |
| N | 29 | 35 | 40 |
| NNE | 27 | 35 | 37 |

Each piece takes the share of its own direction, and together they make the M25's 31 · 37 · 43 %.
A road on another side is carried by other weather. Sound from the south-west, the side the wind
mostly blows from, is bent down 52 % of the day, sound from the east 23 %. Every direction is bent
down more often at night than by day.

The second table, Air dB/km, is what a kilometre of air takes from each octave band, averaged over 30
years of the place's weather. High notes fade fast, low ones carry:

| Hz | Heard as | Air dB/km at High Beach |
|---|---|---:|
| 63 | a rumble | 0.11 |
| 1k | a voice | 3.90 |
| 8k | a hiss | 103 |

Over the 2 km from the M25 its rumble loses 0.2 dB and its hiss all of it: a distant road sounds
low.

<details>
<summary>How the weather is computed</summary>

- **Two states.** Each ray is computed twice, as CNOSSOS-EU prescribes (section 2.5.9): in
  homogeneous conditions, on straight lines, and in favourable conditions, on arcs bent towards the
  ground with a radius of at least 1 km, which the edges on the way screen less. A period's
  level of the ray is 10 × log₁₀(p × 10^(L<sub>F</sub>/10) + (1 − p) × 10^(L<sub>H</sub>/10)), p
  the favourable share of the period for the ray's direction, read linearly between the two nearest
  of 16 directions.
- **The favourable share.** In every three-hourly state of
  <abbr title="the fifth reanalysis of the European Centre for Medium-Range Weather Forecasts">ERA5</abbr>
  from 1991 to 2020, the wind at 10 m, the cloud cover and whether the sun is up give a
  meteorological class of Nord2000 (Eurasto 2006, VTT-R-02530-06); a direction is favourable when
  the class's speed of sound rises with height along it. p is the share of a period's states in
  which it is, stored every half degree and interpolated to the place.
- **The air** absorbs after ISO 9613-1 at each state's temperature, humidity and pressure,
  averaged over the periods' hours.
- **The levels over time.** The moments of a period are spread over ten equally likely states of
  the place's weather, the same for all its rays, as a night's inversion bends them all at once: a
  piece with p = 0.3 is bent down in the three most favourable states. The levels exceeded 5 to 90 %
  of the time and Nden are computed over every hour and these states; the map takes two of them.

</details>

## Where the point is

The point is 4 m above the ground, as the END prescribes. Inside a building the popup answers at
the building's loudest façade, where a noise map's receivers stand. A building with no exposed
façade gets no answer. No indoor attenuation is applied.

<details>
<summary>How the façade is chosen</summary>

The façade point loudest by Lden from the 100 pieces of ground sources with the highest bound at
the building (flights do not choose it), among receivers placed as
<abbr title="Common Noise Assessment Methods in Europe, the EU's method for road, railway and industrial noise">CNOSSOS-EU</abbr>
places them: every 5 m or less along each façade, 0.1 m in front of the wall and 4 m up, the
façade's own reflection left out.

</details>

## How it is computed

Sound leaves each source, spreads and fades. The air absorbs it, the ground and the trees take from
it, and hills, walls and roofs block it. The weather can [bend it back down](#the-weather) over
them.

The popup follows the sound along rays from every source to the point, over the terrain, the
buildings and the trees. It computes each ray in eight frequency bands, by day, evening and night,
in calm air and downwind, and adds it all up. Ground sources follow
<abbr title="Common Noise Assessment Methods in Europe, the EU's method for road, railway and industrial noise">CNOSSOS-EU</abbr>,
aircraft
<abbr title="the European Civil Aviation Conference's method for noise around airports">ECAC Doc 29</abbr>.

<details>
<summary>The tiles it reads</summary>

The popup reads the point's tile, its eight neighbours and the terrain out to 8 km, then computes
and sends a first answer. It reads one more ring of tiles at a time until every ground source
within 12 km, every flight within 16 km and every runway roll starting within 8 km is covered,
sending the whole answer again
after each ring. At Greenwich in London it
reads 219 MB in 114 reads, the flight tracks and the weather among them.

Every flight uses the same horizon, built once from that terrain and the nearby buildings.

<figure style="margin: 1.75rem 0; color: var(--foreground)">
<svg viewBox="0 0 400 268" role="img" aria-label="Seven by seven tiles around a point in London: the inner three by three and terrain out to 8 km give the first answer, five by five cover the ground sources within 12 km, seven by seven the flights within 16 km" style="display: block; width: 100%; max-width: 440px; height: auto; margin: 0 auto" font-size="12">
<rect x="8" y="8" width="252" height="252" fill="currentColor" fill-opacity=".05"/>
<rect x="44" y="44" width="180" height="180" fill="currentColor" fill-opacity=".1"/>
<rect x="80" y="80" width="108" height="108" fill="currentColor" fill-opacity=".18"/>
<path d="M44 8V260M80 8V260M116 8V260M152 8V260M188 8V260M224 8V260M8 44H260M8 80H260M8 116H260M8 152H260M8 188H260M8 224H260" fill="none" stroke="currentColor" stroke-opacity=".3" stroke-width=".75"/>
<rect x="8" y="8" width="252" height="252" fill="none" stroke="currentColor" stroke-opacity=".6"/>
<circle cx="134" cy="134" r="70.9" fill="none" stroke="currentColor" stroke-width="1.3"/>
<circle cx="134" cy="134" r="94.6" fill="none" stroke="currentColor" stroke-width="1.3" stroke-dasharray="5 4"/>
<circle cx="134" cy="134" r="4" fill="#2f7fd8"/>
<text x="134" y="126" text-anchor="middle" fill="currentColor">point</text>
<text x="134" y="75" text-anchor="middle" fill="currentColor">12 km</text>
<text x="134" y="52" text-anchor="middle" fill="currentColor">16 km</text>
<rect x="276" y="20" width="16" height="16" fill="currentColor" fill-opacity=".3"/>
<rect x="276" y="76" width="16" height="16" fill="currentColor" fill-opacity=".15"/>
<rect x="276" y="132" width="16" height="16" fill="currentColor" fill-opacity=".05" stroke="currentColor" stroke-opacity=".3"/>
<g fill="currentColor"><text x="300" y="33">3 × 3 tiles</text><text x="300" y="89">5 × 5 tiles</text><text x="300" y="145">7 × 7 tiles</text><text x="276" y="204">a tile: 6.1 km</text></g>
<g fill="currentColor" fill-opacity=".7"><text x="300" y="50">first sources</text><text x="300" y="106">ground: 12 km</text><text x="300" y="162">flights: 16 km</text><text x="276" y="221">in London</text></g>
</svg>
<figcaption style="margin-top: 0.5rem; font-size: 0.8125rem; line-height: 1.45; color: var(--muted-foreground)">A point in London. The first answer uses sources from the 3 × 3 tiles around it and terrain out to 8 km; the ground sources within 12 km take 5 × 5 tiles, the flights within 16 km 7 × 7.</figcaption>
</figure>

</details>

<details>
<summary>The pieces it computes</summary>

Most pieces in those tiles are too far or too quiet to matter, and the popup proves it without
computing them. For each piece it takes an upper bound of what could arrive: its sound power over
its shortest distance to the point, absorbed by the air over that distance only, plus 18 dB, the
most that ground and screening can add under CNOSSOS-EU in either weather, plus the façades'
reflection. It computes the pieces in full, the loudest bound first, and stops when everything left
could not add 0.1 dB to any layer by day, evening or night. A layer that would need more than 1,024
pieces, as in a city, samples the rest instead: it draws pieces in proportion to their bounds, 512
at first and up to 32,768, until two standard errors fall under 0.05 dB. The draws are seeded by
the point.

</details>

<details>
<summary>A piece and its rays</summary>

A point sends one ray. A line piece is summed as CNOSSOS-EU sums a line of points: the angle under
which the receiver sees it is cut into five equal parts, each sending one ray; a piece seen under
less than 3° sends one. Five rays match a sum of 401 points within 0.1 dB on a 250 m piece. Where
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
<figcaption style="margin-top: 0.5rem; font-size: 0.8125rem; line-height: 1.45; color: var(--muted-foreground)">Seen from above, R the point. The angle under which R sees the piece is cut into five equal parts, one ray each. Two houses hide stretches of the right-hand parts, so those parts get a ray for each hidden stretch and each gap.</figcaption>
</figure>

</details>

<details>
<summary>One ray</summary>

Along each ray the popup reads the ground wherever the ray crosses a line of the one-arc-second
terrain grid (31 m north to south, 19 m east to west in London), and within 100 m of either end on
a grid six times finer where a national model that fine exists: a motorway berm 7 m high, 20 m in
front of a point 70 m from the motorway's axis, takes 3.5 dB off it, which the coarser grid missed.
Each sample gives the terrain's height, how hard the ground is, and the trees' cover and height;
every wall the ray crosses stands at its height. Over this cross-section the popup takes:

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
- the façades around the point: the share of a 150 m square around it covered by buildings taller
  than 5 m, read on a fixed fine grid. Nine probes in that square would give 1.5 dB when two or more
  fall on such buildings and 3 dB when five or more do; the bonus is that rule's average wherever
  the probes fall: about 0.9 dB at one fifth coverage, 2.2 dB at one half and 3 dB at full
  coverage. The square's edges count their fraction of each grid cell; the receiver's own building
  is left out. Flights take no bonus. Reflections are not traced.

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

</details>

<details>
<summary>From rays to the answer</summary>

A piece's rays add up to the piece, the pieces to the source, the sources to their layer and the
layers to the place, by octave band and by day, evening and night. That gives Lden.

The map does not know when a car or a plane passed; it knows how many pass and how fast: a road's
vehicles a day, spread over the hours by measured hourly profiles, a railway's trains by period,
from timetables where they exist, a flight path's flights and a runway's take-offs and landings by
period. Each is a stream of passes at random times, and the source's distance and speed tell how
much of the time the level spends at each value (Kurze's statistics of moving sources). A road with
600 vehicles an hour at 50 km/h is a series of passes 20 m away and a steady sound from about 80 m;
one flight a day 1 km away is near its peak for about half a minute; a take-off passes a house
100 m beside its start of roll in about 11 seconds, as it sets off from rest. Sources add up only within the same moment, so the sum is taken
for every hour of road traffic and ten states of the weather; bells, calls, a car park's cars, a
range's shots, courts, pitches, playgrounds, pools and terraces in use, the guests outside bars
and restaurants and the plant of shops, offices and restaurants while it runs sound in their share
of the period; industry, the other buildings and ships are steady. That gives the levels exceeded 5
to 90 % of the time.

Nden is every moment's loudness after ISO 532-1 (Zwicker), from the spectrum received in that
period, averaged over the day. A source's share is its part of every moment's loudness by its
energy then; its own Nden, as if it sounded alone, is shown when its row opens.

The popup opens these sums one level at a time:

| In the popup | In the computation |
|---|---|
| Nden and Lden | the place, every layer together |
| A row | a source, or an object of several, with its share of Nden |
| Breakdown | an object's parts; for the aircraft, the kinds of flight and the airports' ground operations |
| Sound path | the source's rays summed after each term, in calm air and downwind |
| Segments | its pieces, each computed again in full, and how its sound arrives |
| A segment's rays | one piece's rays, each with the ground and walls it was computed over |
| Detailed calculation | the receiver, each period's levels and loudness, the layers, the weather |

</details>

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
against them. It is an engineering estimate for comparison and exploration, not a measurement, a
legal noise map or a property certificate.

## In the code

<details>
<summary>The popup's code: popup/, server/ and frontend/, and its targets</summary>

`popup/` (`qm-popup`) answers a point:

- `answer.rs`: the ring loop, until every kind's reach is covered (`candidates::GROUND_REACH_M`,
  `aircraft::boxes::AIRCRAFT_REACH_M`); an update after every ring.
- `release.rs`: a ring's tiles, each read whole from its square file (`tiles/src/square.rs`).
- `candidates.rs`: every piece of the files read, with its bound (`physics/src/bound.rs`).
- `selection.rs`, `sampling.rs`: the stop rule and the sample's rounds, one omitted-energy account
  per layer and period.
- `scene.rs`: the ground under a point and along a ray, at every line of the terrain grid it crosses.
- `evaluate.rs`: one source at the receiver, its rays through `physics/src/ray.rs`, `line.rs`,
  `cnossos/` and `foliage.rs`, and its sound path.
- `building.rs`: a point inside a building, answered at its loudest façade.
- `obstacles/reflection.rs`, `obstacles/reflection_coverage.rs`: the same covered area for the popup
  and the painter, tabulated only where the bounds of tall footprints and reachable receiver windows
  intersect; `physics/src/reflection.rs` turns its share into the receiver bonus.
- `aircraft/`: the boxes through the popup's equation, the runways' rolls segment by segment
  (`physics/src/doc29/roll.rs`), the flight list, the events, the tracks.
- `rows.rs`, `percentiles.rs`, `distribution.rs`, `loudness.rs`, `shares.rs`, `lines.rs`,
  `listing.rs`, `json.rs`: the rows, the levels over time, Nden, the shares, the lines on the map,
  the listed pieces and the update as one line of JSON.

`server/` runs `qm-popup` once per point (two at once, two waiting per slot, 503 when full, stopped
after 30 s or when the visitor leaves) and forwards its lines as they come; `frontend/` redraws the
panel on every line.

The latency targets, checked with `bench/cold.py` and `bench/report.py` from a cold disk, are
0.2 s to the first answer and 0.5 s to the full answer from an NVMe disk, 0.5 s and 1.5 s from a
hard disk. Each layer's Lden is within 0.1 dB of the full computation in 95 % of the layer answers,
and within 0.3 dB in all. Each update stays under 100 KB, held by caps on the lines and tracks it
carries.

</details>

<details>
<summary>What the popup guarantees</summary>

- The bound stays an upper bound of `ray` and `line`: a source is skipped only by the
  omitted-energy account, never by a per-source threshold.
- The sample is seeded by the point, so a point always answers the same.
- Until every ring is read the answer says it is partial; each update is the whole answer so far,
  one line of JSON in one streamed HTTP response. A failed read is an error, never a quieter
  answer. Exact mode (`--exact 1`, benchmark only) is the same loop with the stop rule off.
- A building's source is never screened by a footprint containing it (its own, or another outline
  of the same building).
- A ray tangent to a building corner keeps its top as an obstacle, but does not enter or leave a
  roof: a tangent cannot make another footprint count as containing the source.

</details>
