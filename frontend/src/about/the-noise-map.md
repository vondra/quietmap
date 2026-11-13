---
title: The noise map
intro: What the colours mean, and how close the painted map comes to the popup.
---

<!-- SECTIONS -->

The colours show how loud each place sounds, as the popup there would answer. Kenwood's lawn on
Hampstead Heath in London is yellow; Soho Square is red. For the exact answer at a
place and what makes it, open the popup there.

## Colours

One colour for each doubling of loudness: each sounds twice as loud as the one before it. The places
are clicked in the popup, October 2026.

| Colour | Sone | A place like it (sone, Lden) |
|---|---|---|
| clear | under 1, a quiet room | Haystacks in the Lake District, 1 km above the road (0.9, 34&nbsp;dB) |
| <span style="white-space:nowrap"><span style="display:inline-block;width:14px;height:14px;border-radius:3px;vertical-align:middle;border:1px solid rgba(0,0,0,.15);background:#ffeeae"></span> yellow</span> | 3 | Kenwood's lawn on Hampstead Heath, London (3.6, 47&nbsp;dB) |
| <span style="white-space:nowrap"><span style="display:inline-block;width:14px;height:14px;border-radius:3px;vertical-align:middle;border:1px solid rgba(0,0,0,.15);background:#ffac6a"></span> orange</span> | 6 | Hyde Park by the Serpentine, London (6.3, 50&nbsp;dB) |
| <span style="white-space:nowrap"><span style="display:inline-block;width:14px;height:14px;border-radius:3px;vertical-align:middle;border:1px solid rgba(0,0,0,.15);background:#e85953"></span> red</span> | 12 | Soho Square, London (10.8, 60&nbsp;dB) |
| <span style="white-space:nowrap"><span style="display:inline-block;width:14px;height:14px;border-radius:3px;vertical-align:middle;border:1px solid rgba(0,0,0,.15);background:#9e57a3"></span> purple</span> | 24 | The end of Heathrow's runway 27L, London (26.7, 79&nbsp;dB) |
| <span style="white-space:nowrap"><span style="display:inline-block;width:14px;height:14px;border-radius:3px;vertical-align:middle;border:1px solid rgba(0,0,0,.15);background:#592d6b"></span> dark purple</span> | 48 and more | The Hanger Lane gyratory on the A40, London (62.9, 85&nbsp;dB) |

The colours blend smoothly in between. A quiet place stays clear on every base map, satellite and
terrain too; the loud ones grow dark and opaque. The swatches show them over a white map. What a sone is, and how it
differs from a decibel, is on [Noise at a point](/about/noise-at-a-point#what-the-numbers-mean).

A place not yet painted is clear too, without the quiet zones' green, and its hover reads "—". The
popup computes it all the same.

### Lden

The switch Noise in, under Advanced in the layers, shows everything in Lden instead: the map, its
hover, the quiet zones, the popup's level and its sources' shares, and the places you opened. Lden is
the EU's day-evening-night level, the sound's energy averaged over the day with the evening 5 dB and
the night 10 dB louder. A short loud event weighs more in it than in sone: a few loud flights a day raise
Lden far more than they raise how loud the place sounds over the day. The same colours stand at decibels the EU's noise maps and the
WHO use: clear under 35 dB, yellow from 45 (the WHO's guideline for aircraft), orange from 55 (the
lowest band the EU's maps report; the WHO's guideline for roads is 53), red from 65, purple from 70
and dark purple from 75 dB, the EU's top band. The map holds Lden only where it was painted with it,
since 30 October 2026; elsewhere the Lden map is clear until those squares are painted again. The
popup gives Lden everywhere.

<details>
<summary>Why these colours</summary>

The colours are the air-quality index's, in its order: yellow, orange, red and purple, and a darker
purple for the loudest, where the index has maroon; its green is the clear map. The darkest marks
the loudest places: 48 sone and more is 0.3 % of London's painted map, the lanes of its busiest
roads and junctions. Lighter colours read as calmer and more saturated ones as more urgent, and the lightness falls
with loudness, so the scale still reads in grey and with red-green colour blindness. The colours are
blended in CIELAB by the logarithm of the loudness; their opacity rises from 0.45 at 3 sone to 0.94
at 48 sone (`frontend/src/lib/heatmap-palette.ts`).

</details>

## Layers and quiet zones

The map shows any of the noise layers together: roads, railways, industry, buildings, ships and
aircraft. All six show the painted total, one alone its own loudness, and two to five an estimate
from each layer's own. Quiet zones wash green every place where the total stays at or below a
chosen loudness, from 0.25 to 4 sone, or a chosen Lden, from 20 to 55 dB.

<details>
<summary>How two to five layers are combined</summary>

Loudness does not add like energy, so each layer's loudness is raised to a power that grows with
the loudest layer's level (1.0 at 22 phon, 1.9 at 62, 3.1 at 87 phon), summed and taken back.
Against the painted total of all six this is within 1 phon at 90 % of the places where two layers
or more sound, and within 2 phon at 98 %. Lden adds as energy, so its layers are summed exactly.

</details>

Advanced shows the data the popup computes over; each layer switched on shows its legend:

- elevation, forest and hard ground;
- buildings by height, and noise barriers;
- road traffic (vehicles a day) and trains a day;
- the other sources: turbines, bells, terraces, car parks, airports.

Data layers stay above the green quiet zones. On the standard and satellite maps, labels do too.

## Places to stay

Set a maximum price per night in euros for the selected dates and guests. Turn on Quiet zones to
keep stays within its total-noise limit. A reachable area shows how far you can travel; stays
beyond it remain visible. The area keeps its starting point while you open stays; Clear in Layers
removes it. A place without a quote is excluded by a price limit. The quiet filter hides stays
where the noise map has no value; clicking the map still calculates noise at that point.

The noise filter reads the map at its finest resolution, even when zoomed out. It describes outdoor
noise averaged over the year, not the room or the dates of the stay. Open a stay for the detailed
calculation at its point. The offers returned for each view are a selection, not a complete list.

## How close it comes to the popup

The map's target is to stay within 1 phon of the popup, about 7 % of loudness, at 95 % of the
points where the popup gives at least 1 sone, and within 3 phon at 99.5 %. Buildings, individual
noise layers and points quieter than the map's first colour can differ more.

On 13 November 2026, newly calculated tiles were within 1 phon at 98.2 % of sampled points where
the popup gave at least 1 sone, and within 3 phon at 99.95 %, across 54 squares around the world.
These checks compare computations; [Validation](/about/validation) compares the model with
microphone measurements.

<details>
<summary>The limits and the last check</summary>

The independent popup check samples 300 random pixel centres in each of 54 squares, including
buildings. The squares were drawn from groups with different numbers of sources and calculation
costs across the world's land and coastal grid, including already painted areas. Each popup uses
its own source selection, façade choice and finer loudness calculation. The sample covers all
continents except Antarctica, but its percentages are not a population-weighted global error rate.
Existing tiles can predate the computation tested here.

Of 16,200 sampled points, 5,965 have a stored popup total of at least 1 sone. At those points,
641 differ by more than 0.5 phon, 109 by more than 1 phon, three by more than 3 phon and none by
more than 6 phon. The mean signed difference is +0.25 phon. By layer:

| Layer | Popup points ≥ 1 sone | More than 1 phon apart |
|---|---:|---:|
| Roads | 5,209 | 2.38 % |
| Railways | 833 | 0.96 % |
| Industry | 904 | 2.10 % |
| Buildings | 525 | 4.76 % |
| Ships | 113 | 1.77 % |
| Aircraft | 372 | 0.81 % |
| Total | 5,965 | 1.83 % |

Across all points, including those below 1 sone, 1,821 differ by more than 1 phon; the largest
quiet-total difference is 18.5 phon. There are 79 crossings of the 1-sone boundary beyond its
half-phon rounding band. No sampled point is missing from the map. Total Lden differs by more
than 1 dB at 5 of the 5,916 points where the popup gives at least 35 dB, and at 291 of 14,290
points with a nonnegative popup level; the largest difference between stored nonnegative values
is 14.5 dB. Sixteen nonnegative popup values become below zero in the map, and 29 cross the
other way.

The target follows what the map shows (12 November 2026). Where the popup gives at least 1 sone,
the total must lie within 1 phon at 95 % of the points and within 3 phon at 99.5 %, with a mean
signed difference within ±0.3 phon; Lden has the same limits in decibels where the popup gives at
least 35 dB. Below the first colour, at 1 sone and 35 dB, a difference shows only at the edge of
the quiet zones; there a patch is interpolated when its total agrees within 0.1 sone and 2 dB.
The layers have no limit of their own. This sample meets the target.

A second check isolates the painting approximations in four squares in central Bohemia. Its
outdoor reference evaluates every retained loud ground source and the flights exactly at one
pixel in each 4 × 4 cell, omitting buildings. It shares the map's background hum, flight spectra
and loudness resolution, so it cannot reveal an error common to those parts of both computations.
The independent popup check is needed alongside it. On 12 November, before the flights were
heard as passes, the outdoor total differed
by more than 1 phon at 196 of 257,377 reference pixels (0.08 %), with a mean difference of −0.01
phon and a largest difference of 5.5 phon. Total Lden differed by more than 1 dB at 122 of those
pixels, with a largest difference of 4 dB. A layer alone differed by more than 1 phon at 0.5 % of
its pixels of at least 1 sone for roads, 0.7 % for railways, 1.6 % for industry and 3.7 % for
buildings.

</details>

## How it is painted

The map is painted ahead of time from the popup's data and physics, at pixels 6 m apart in London.
A pixel inside a building shows the building's loudest façade, as the popup does.

<details>
<summary>How a tile is painted</summary>

The map is painted in squares of 1,024 × 1,024 pixels. Every loud source is computed exactly at the
corners of blocks 16 pixels wide and at the pixels it is near, or blended from exact samples there
where the sound is smooth. Of the far sources, a pixel computes the loudest of each direction; the
rest is blended between the corners and follows them. The faint hum of far, quiet sources is
computed on a coarser grid.

A square is a tile of the web map's zoom 12, in blocks of 16 × 16 pixels (95 m in London). A ground
source counts as loud at a point where its upper bound there reaches 20 dB.

- At every block corner, each loud source is computed exactly.
- A block's pixels compute exactly, of the sources loud at the pixel, those that cross the block or
  one of the eight around it, and those its corners select. A corner selects, for each layer, its
  loudest far source in each of 16 directions that holds at least 1 % of the layer's far energy
  there, then its loudest sources until at most half of the layer's energy there, and half of its
  far sources' energy, is left out (a fifth for the railways, whose few lines differ more), or
  what is left out is under 25 dB.
- What is left out is blended between the block's four corners by layer and direction. At a pixel
  it is scaled by how the block's computed far sources of that direction arrive there, against
  their own blend between the corners that hear them: weakened behind a house, as they are.
- Where that scaling changes more than 30 % of what a pixel hears of its loud sources, the pixel also computes,
  layer by layer, the loudest sources at the block's corners until what each corner leaves out
  weighs at most 20 % of the layer at the block's quietest corner, 5 % for the railways: blended
  between the corners, a railway's few trains are heard at the wrong loudness.
- The hum, the sources under 20 dB, is estimated with the popup's physics and error budget at
  points 64 pixels apart (380 m in London), and blended between them in decibels: it falls off
  steeply from its nearest sources, and a blend in energy read quiet places by their loudest point.

An open block hearing only the hum and flights takes its loudness from shared corners, without
constructing a receiver at every pixel. A block that no source or screening edge crosses, itself
or its margin, first computes one probe pixel in each quarter. Where all four agree with the
block's four corners interpolated, the corners paint the block: they hear every loud source.
Otherwise the block computes a 3 × 3 grid of pixel receivers (its edges and middle) and is
interpolated from that grid under the same test. The remaining blocks use a 5 × 5 grid, the first
grid's pixels among it, with one further pixel probe for each of its sixteen patches. A patch
interpolates energy and loudness only when no source or screening edge crosses it or the margin
protecting the pixels used to rank façades, and its probe agrees as far as the map shows: the
total within half a phon from 0.8 sone and half a decibel from 32 dB, a layer within 1 phon from
1 sone and 1 dB from 35 dB. Quieter values may differ more: a total by 0.1 sone and 2 dB, a layer
by 0.2 sone and, from 20 dB, by 3 dB. Energy and loudness are interpolated in the logarithm. The
other pixels keep their ray computations; there the exact lattice results are reused.
Where a patch's probe confirms it, those pixels take their loudness from the patch's lattice
points, moved to each pixel's own energies along the block's measured relation between loudness
and energy, instead of a loudness calculation of their own; a pixel whose energies lie more than
half a decibel outside the lattice points' range keeps its own calculation. A probe pixel is
painted as its patch is.
Between samples, a shadow cast from outside the patch can still change sharply.
Buildings and their façade receivers keep the exact computations.

<figure style="margin: 1.75rem 0; color: var(--foreground)">
<svg viewBox="0 0 400 354" role="img" aria-label="A part of a painted tile around pixel P: road A crosses the blocks around P and is computed exactly at P; road B is computed at the corners of P's block, its loudest pieces there exactly at P too, past a house, the rest of it blended between the corners; the hum comes from points 64 pixels apart" style="display: block; width: 100%; max-width: 440px; height: auto; margin: 0 auto" font-size="12">
<rect x="40" y="136" width="96" height="96" fill="#2f7fd8" fill-opacity=".14"/>
<path d="M40 8V264M72 8V264M104 8V264M168 8V264M200 8V264M232 8V264M8 40H264M8 72H264M8 104H264M8 168H264M8 200H264M8 232H264" fill="none" stroke="currentColor" stroke-opacity=".16" stroke-width=".75"/>
<path d="M136 8V264M8 136H264" fill="none" stroke="currentColor" stroke-opacity=".45"/>
<rect x="8" y="8" width="256" height="256" fill="none" stroke="currentColor" stroke-opacity=".6"/>
<path d="M14 250L120 120" fill="none" stroke="#d9622b" stroke-width="3" stroke-linecap="round"/>
<path d="M318 18L365 262" fill="none" stroke="#d9622b" stroke-width="3" stroke-linecap="round"/>
<rect x="196" y="160" width="22" height="18" fill="currentColor" fill-opacity=".2" stroke="currentColor" stroke-opacity=".7"/>
<path d="M344 154L90 184" fill="none" stroke="#2f7fd8" stroke-width="1.6"/>
<g fill="currentColor"><circle cx="72" cy="168" r="2.4"/><circle cx="104" cy="168" r="2.4"/><circle cx="72" cy="200" r="2.4"/><circle cx="104" cy="200" r="2.4"/></g>
<g fill="none" stroke="currentColor" stroke-width="1.3"><circle cx="8" cy="8" r="7"/><circle cx="136" cy="8" r="7"/><circle cx="264" cy="8" r="7"/><circle cx="8" cy="136" r="7"/><circle cx="136" cy="136" r="7"/><circle cx="264" cy="136" r="7"/><circle cx="8" cy="264" r="7"/><circle cx="136" cy="264" r="7"/><circle cx="264" cy="264" r="7"/></g>
<rect x="86" y="180" width="8" height="8" fill="#2f7fd8"/>
<g fill="currentColor" font-weight="600"><text x="98" y="196">P</text><text x="34" y="250">A</text><text x="330" y="36">B</text></g>
<rect x="8" y="280" width="14" height="14" fill="#2f7fd8" fill-opacity=".14" stroke="#2f7fd8" stroke-opacity=".5"/>
<circle cx="15" cy="305" r="2.4" fill="currentColor"/>
<path d="M8 323H22" fill="none" stroke="#2f7fd8" stroke-width="1.6"/>
<circle cx="15" cy="341" r="6" fill="none" stroke="currentColor" stroke-width="1.3"/>
<g fill="currentColor"><text x="30" y="291">exact: loud sources in P's 3 × 3 blocks</text><text x="30" y="309">block corners (16 px, 95 m): every loud source</text><text x="30" y="327">a piece the corners admit, exact at P</text><text x="30" y="345">hum points (64 px, 380 m): quiet sources</text></g>
</svg>
<figcaption style="margin-top: 0.5rem; font-size: 0.8125rem; line-height: 1.45; color: var(--muted-foreground)">Part of a tile painted at 1,024 pixels, distances in London. Road A crosses the blocks around pixel P and is computed exactly there. Road B is computed at the corners of P's block; its loudest pieces there are computed exactly at P too, their rays here passing a house, and the rest of it is blended between the corners. The quiet sources come from the hum points.</figcaption>
</figure>

Each block blends only the sources it leaves out, so neighbouring blocks blend different
remainders; at the corners they share, the exact and the blended sound together meet. Aircraft
boxes are summed at the hum's 17 × 17 points; their near share, within three hum spacings, is
computed with the runway rolls every second block corner, and between those where the energy varies
by more than 1 dB. Their spectra use 33 × 33 points:
alternating points and cell centres are computed, others blended only where the spectra change
little and their centre agrees. The square's edges are computed. Cruise boxes use 5 × 5 points
where screening does not apply, every corner reads the same boxes within reach, and their pieces
and average flight lines stay at least 8 km away. Their energies are blended to the block corners
in the popup's classes of the level a flight has while it passes, 3 dB apart, each with the share
of the time its flights are passing, and the runway rolls with them. At a corner, eight
neighbouring classes then form one stream of passes that keeps their energy and, by the loudness
curve of a spectrum flat over the third octaves, their loudness while they pass; a pixel hears
the streams blended from its corners. Against every class as a stream of its own, as the popup
takes them, the aircraft layer alone reads 0.3 phon louder. The
quiet sources may be omitted only when their summed bounds can change a layer's exact loud
energy by at most 0.1 dB in every period. Otherwise the painter uses the popup's bound-proportional
sampling and 0.05 dB confidence budget, batches the selected rays on the graphics card and
applies the sampling weights to their spectra.
A pixel inside a building shows the building's loudest façade, as the popup
inside it does: of the receivers 0.1 m in front of its walls, a party wall's left out, the three
whose painted pixel in front is loudest are computed like pixels, without the building's own
reflection, and the loudest of them counts. Both choices go by the ground sources, as the popup's
do: the flights do not pick the façade. A building crossing a painting square's boundary ranks all its exposed
receivers by the popup's rule, using the 100 ground-source pieces with the greatest bound at any
façade. Its chosen receiver computes every loud ground source and the flights exactly; only its
background hum and flight spectrum use the nearest pixel's fields. Both halves therefore choose
from the complete building, even when its loudest façade is in the neighbouring square. New
painting uses this rule; existing squares keep their earlier façade choice until repainted.

A pixel's loudness is computed as the popup computes it, at a coarser grain: the levels over time
in steps of 2 dB and four groups of the day's hours, one of the
evening's and three of the night's, where the popup takes 0.5 dB and every hour. Both use the same
ten weather states, preserving how long favourable propagation lasts. For each period the map
groups consecutive states in which every source group's mean energy varies by at most 1 dB and
gives each group its average energy over the run, keeping the run's full time and the group's
energy; the popup keeps the ten states' own means. New painting uses this rule; existing squares
keep their earlier weather calculation until repainted. Each
layer and the total are stored in steps of half a phon, 3.5 % of loudness; zoomed out, a pixel is
the mean loudness of the pixels under it.

</details>

## In the code

<details>
<summary>The painter's code: paint/ and paint-gpu/, the tiles it writes and its checks</summary>

`paint/` is the method and `paint-gpu/` (`qm-paint`) runs it by squares of zoom 12 in three stages:
the cores read the next square's tiles once, to the reach of its farthest pixel, give the card the
fine terrain of every land tile among them, and take its corners, hum and aircraft lattices; two
squares paint their pixels at once (a square of many sources alone); two writers publish the squares
before. The exact pairs, the hum's sampled pairs, the aircraft boxes' sums at the block corners'
lattices and every pixel's loudness run on the first CUDA card (`paint-gpu/kernels/`, the popup's
physics in single precision, with fast math except the loudness; a ray's crossing of a building's
edge is found in double wherever single precision cannot rule it out; each function cites its Rust) or
on the cores. The runway rolls, the flight spectra and a building across the square's edge stay on
the cores. A square's samples, then its pixels, then its
façades run in passes, four at once, at most two launching their pairs on the card while the
others list and sound their pixels on the cores beside them; a pass gathers its next launch's pairs while the card evaluates the
one before.
The cores assemble sounds directly into chunks of at most 8,192 pixels, preparing the next chunk
while the card computes the current chunk's loudness.
The passes return in spatial order, so equal façade energies keep the same first candidate.
Each file's header says what it does; the sources a corner selects are
`paint/src/corner_sources.rs`, the blocks `blocks.rs`, what reaches a pixel of the sources left
out `transfers.rs`, the pixels' rays `traced.rs`, the interpolation `quiet.rs`,
`quiet_corners.rs`, `block_samples.rs`, `quiet_samples.rs`, `visible.rs` and `lattice_blend.rs`,
the hum
`paint/src/hum.rs` and `hum_samples.rs`, the aircraft boxes `paint/src/flights.rs` and
`flight_boxes.rs` (on the card `paint-gpu/src/aircraft.rs`), their streams of passes
`paint/src/flight_streams.rs`, the aircraft spectra `paint/src/flight_spectra.rs`, a
building's façade `paint/src/buildings.rs`, a pixel's loudness `paint/src/sone.rs` at
`popup::percentiles::Resolution::MAP`, and the tiles `paint/src/hm3.rs` and `hm3_square.rs`: per layer and the total,
four tiles of zoom 13 of 512 × 512 cells of 80 + 20 log₂ of the loudness in sone, twice the loudness
level in phon from 1 sone up (80 one sone, a doubling 20; 254 silence under 1/16 sone, 255 not
assessed). Each tile is compressed when painted: PNG's Paeth predictor uses
the left, above and upper-left cells (zero outside the tile), and the cell minus that prediction,
modulo 256, enters one lossless Brotli stream with a 512 KiB window. The painter uses quality 10
for new tiles; existing streams at other quality settings decode in the same way.
The browser's HTTP stack decompresses, and `frontend/src/lib/hm3-decoder.ts` restores the same prediction in place;
every cell, including silence and not assessed, returns exactly.

The square format supports one atomic bundle containing these Nden streams and Lden
in decibels for each layer and the total. Lden comes from the same propagated energies, summed before taking
the logarithm, and is stored in half-decibel steps from 0 to 126.5 dB, capped above that range:
254 means below 0 dB, 255 not assessed. It is
compressed with the same predictor and lossless Brotli at quality 9. In open background blocks,
Lden uses the original hum and aircraft energy fields at each pixel, while Nden keeps its shared
corner blend. A sampled source patch must also agree with its exact probe in Lden, within the
limits above; otherwise its remaining pixels are calculated individually.
The bundle's directory points at compressed tile streams, identical tile grids encoded and stored
once. Container version 8 holds both quantities; already painted version-7 bundles contain only
Nden and remain complete. The Nden tile streams keep HM3 version 7; Lden streams use HMD version 1,
so decibels cannot be mistaken for loudness. Verification decodes both quantities, checks their
assessment masks and Lden totals, and refuses any cell of 120 phon or 120 dB and more. Two
writers compress and publish the bundles while the next squares are painted; at most six squares' cell buffers are resident (two written,
two waiting, two painted), and a square is complete only after its bundle is durable
(`paint-gpu/src/square_writer.rs`). `qm-paint merge` copies
independent painters' bundles into one tree atomically, without decoding or replacing completed
squares. Identical completed copies are skipped on retry; conflicting squares stop the merge.
`paint/src/pyramid.rs` reads each compressed square once a quantity and carries the assessed-pixel counts and
the unrounded mean through the same means, the branches on all cores at once; it compresses each lower
tile once. A lower zoom's cell is the mean over its assessed pixels of what each quantity averages
over the day: the loudness in sone for Nden, the energy for Lden. Lden's pyramid covers only the
squares that hold it, so an older square stays without a level there rather than reading quiet; at
zoom 2, where a cell spans four squares, it is the mean of those that hold Lden.
A region of 16 × 16 squares (a tile of zoom 8) is packed as soon as it is painted
(`qm-paint pack-region`, `paint/src/region.rs`): its tiles from zoom 12 to 8 and its exact means are
kept for each quantity (`12_34.region`, `12_34.lden.region`), and the pack reuses them while the
region's bundles are unchanged.
`pack.rs` copies these streams into one immutable PMTiles archive per layer and quantity (`road`,
`lden-road`), zooms down to 2,
ordered by zoom and Hilbert tile id. Identical streams share an archive range; gzip compresses its
index. An archive is read by byte range, so a tile needs no scan or server decompression, and the
same files support object storage. The manifest appears only after every archive is complete; the
server sends the stored stream with `Content-Encoding: br`. The manifest URL belongs to the tile
format's version, so an open page with an earlier decoder keeps its last compatible immutable build
until the page is refreshed.

Loud is one rule for the painter, its reference and the hum: `popup::candidates::loud`, a bound reaching
`physics::bound::REACH_EDGE_LDEN_DB` (20 dB). `qm-paint-gpu check` measures the card against the
cores on random pairs, `check-loudness` on random pixels' loudness and `check-flights` on a square's
aircraft lattices; `etalon` paints the reference,
`compare` scores a map against it, and `popup` writes the popup's loudness at random pixels as a
sparse reference.

</details>
