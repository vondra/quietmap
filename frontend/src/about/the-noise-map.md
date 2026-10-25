---
title: The noise map
intro: What the colours mean, and how close the painted map comes to the popup.
---

<!-- SECTIONS -->

The colours show how loud each place sounds, as the popup there would answer. Kenwood's lawn on
Hampstead Heath in London is amber; 70 m from the Westway the map turns red. For the exact answer at
a place and what makes it, open the popup there.

## Colours

One colour for each doubling of loudness: each sounds twice as loud as the one before it. The places
are clicked in the popup, October 2026.

| Colour | Sone | A place like it (sone, Lden) |
|---|---|---|
| clear | under 1, a quiet room | Haystacks in the Lake District, 1 km above the road (0.9, 34&nbsp;dB) |
| <span style="white-space:nowrap"><span style="display:inline-block;width:14px;height:14px;border-radius:3px;vertical-align:middle;border:1px solid rgba(0,0,0,.15);background:#caefdb"></span> sage</span> | 2 | Box Hill in Surrey, the M25 4 km away (2.5, 45&nbsp;dB) |
| <span style="white-space:nowrap"><span style="display:inline-block;width:14px;height:14px;border-radius:3px;vertical-align:middle;border:1px solid rgba(0,0,0,.15);background:#fddb8d"></span> amber</span> | 4 | Kenwood's lawn on Hampstead Heath, London (3.6, 47&nbsp;dB) |
| <span style="white-space:nowrap"><span style="display:inline-block;width:14px;height:14px;border-radius:3px;vertical-align:middle;border:1px solid rgba(0,0,0,.15);background:#fe994c"></span> orange</span> | 8 | Bibury in the Cotswolds, 17 m from its main road (8.5, 59&nbsp;dB) |
| <span style="white-space:nowrap"><span style="display:inline-block;width:14px;height:14px;border-radius:3px;vertical-align:middle;border:1px solid rgba(0,0,0,.15);background:#d84c4c"></span> red</span> | 16 | 70 m from the Westway in London (19, 66&nbsp;dB) |
| <span style="white-space:nowrap"><span style="display:inline-block;width:14px;height:14px;border-radius:3px;vertical-align:middle;border:1px solid rgba(0,0,0,.15);background:#9a4f9f"></span> purple</span> | 32 and more | 70 m from the M25 beside Heathrow (35, 76&nbsp;dB) |

The colours blend smoothly in between. The quiet ones stay pale and see-through over the base map,
the loud ones dark and opaque; the swatches show them over a white map. What a sone is, and how it
differs from a decibel, is on [Noise at a point](/about/noise-at-a-point#what-the-numbers-mean).

A place not yet painted is clear too, without the quiet zones' green, and its hover reads "—". The
popup computes it all the same.

<details>
<summary>Why these colours</summary>

Lighter colours read as calmer and more saturated ones as more urgent, so the quiet colours stay
soft and the loud ones grow saturated, dark and opaque. The order is the one air-quality maps
taught, and the lightness falls with loudness, so the scale still reads in grey and with red-green
colour blindness. The colours are blended in CIELAB by the logarithm of the loudness; their opacity
rises from 0.5 at 2 sone to 0.92 at 32 sone (`frontend/src/lib/heatmap-palette.ts`).

</details>

## Layers and quiet zones

The map shows any of the noise layers together: roads, railways, industry, buildings, ships and
aircraft. All six show the painted total, one alone its own loudness, and two to five an estimate
from each layer's own. Quiet zones wash green every place where the total stays at or below a
chosen loudness, from 0.25 to 4 sone.

<details>
<summary>How two to five layers are combined</summary>

Loudness does not add like energy, so each layer's loudness is raised to a power that grows with
the loudest layer's level (1.0 at 22 phon, 1.9 at 62, 3.1 at 87 phon), summed and taken back.
Against the painted total of all six this is within 1 phon at 90 % of the places where two layers
or more sound, and within 2 phon at 98 %.

</details>

Advanced shows the data the popup computes over; each layer switched on shows its legend:

- elevation, forest and hard ground;
- buildings by height, and noise barriers;
- road traffic (vehicles a day) and trains a day;
- the other sources: turbines, bells, terraces, car parks, airports.

## How close it comes to the popup

The map should agree with the popup within 1 phon, 7 % of loudness, at any point. The painter is
checked against a reference that computes every loud source exactly at every outdoor pixel. In the
last check, in October 2026, the total missed it by more than 1 phon at 0.07 % of 248,909 pixels.
Single layers miss more often: the railways at 2.40 % of the pixels, the buildings at 1.21 %.

<details>
<summary>The limits and the last check</summary>

The painter is held to two references. The etalon is the same tile with every loud ground source
and the flights computed exactly at every outdoor pixel, the hum, the flights' spectra and the
loudness grain taken as the painter takes them: over the pixels of 1 sone or more, each layer and
the total may differ from it by more than 0.5 phon at no more than 20 % of them, by more than 1
phon (7 % of loudness) at no more than 1 %, by more than 3 phon at 0.01 %
and by more than 6 phon at 0.001 %, with a bias under 0.5 phon; at most 0.25 % may fall on the
other side of 1 sone, and a pixel under 1 sone may be off by at most 10 phon. And at any point it
should agree with the popup within 1 phon.

On one pixel in 16 of four squares in central Bohemia (248,909 of 1 sone or more, October 2026)
the total meets every limit: 0.07 % over 1 phon, 3 pixels over 3 phon, none over 6 phon or across
1 sone, a bias of −0.07 phon. Single layers miss some: the railways and the buildings over 1 phon
(2.40 and 1.21 %), the roads, railways, industry and buildings over 3 phon (0.03 to 0.11 %), the
roads, industry and buildings over 6 phon (2 to 4 pixels); the railways, aircraft and buildings
across 1 sone (0.52 to 1.33 %); the railways, industry and buildings under 1 sone (up to 21 phon).

The exact computations run on a graphics card in single precision or on the processor's
cores in double; a pair the card refuses is computed on the cores. On 2 million random pairs of
source and pixel in Prague (October 2026) the two agree within 0.006 dB for 99 % of them and
0.03 dB for 99.9 %. Single precision puts 90 pairs more than 1 dB apart, up to 23 dB, where a ray
grazes the terrain or crosses a building's edge; all but one of them lie more than 20 dB below the
loudest source at their pixel.

</details>

## How it is painted

The map is painted ahead of time from the popup's data and physics, at pixels 6 m apart in London.
A pixel inside a building shows the building's loudest façade, as the popup does.

<details>
<summary>How a tile is painted</summary>

The map is painted in tiles of 1,024 × 1,024 pixels. Every loud source is computed exactly at the
corners of blocks 16 pixels wide and at every pixel it is near. What is far is blended between the
corners, and the faint hum of far, quiet sources is computed on a coarser grid.

The tiles are the web map's of zoom 12, each in blocks of 16 × 16 pixels (95 m in London). A ground
source counts as loud at a point where its upper bound there reaches 20 dB.

- At every block corner, each loud source is computed exactly.
- A block's pixels compute exactly the loud sources that cross the block or one of the eight around
  it and, layer by layer, the loudest at the block's corners until what each corner leaves out
  weighs at most 20 % of the layer's loud sources at the block's quietest corner.
- What is left out is blended between the block's four corners, and weakened at a pixel as much as
  the block's computed far sources are there, as behind a house.
- The hum, the sources under 20 dB, is estimated with the popup's physics and error budget at
  points 64 pixels apart (380 m in London), and blended between them in decibels: it falls off
  steeply from its nearest sources, and a blend in energy read quiet places by their loudest point.

An open block hearing only the hum and flights takes its loudness from shared corners, without
constructing a receiver at every pixel. Other blocks use a 5 × 5 grid of actual pixel receivers,
with one further pixel probe for each of its sixteen patches. A patch interpolates energy
and loudness only when its probe agrees within half a phon in the total and in every layer of
half a sone or more (a tenth of a sone in a quieter layer, which the map shows clear), and no
source or screening edge crosses it or the margin protecting the pixels used to rank façades.
The other pixels keep their ray computations; exact lattice and probe results are reused.
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
boxes are summed at the hum's 17 × 17 points; their near share is computed every second block corner,
and between those where the energy varies by more than 1 dB. Their spectra use 33 × 33 points:
alternating points and cell centres are computed, others blended only where the spectra change
little and their centre agrees. The square's edges are computed. Cruise boxes use 5 × 5 points
where screening does not apply, every corner reads the same boxes within reach, and their pieces
and average flight lines stay at least 8 km away. Their energies and energies times the flights' rate and passing
duration are blended to the block corners, keeping how their noise changes over time. The
quiet sources may be omitted only when their summed bounds can change a layer's exact loud
energy by at most 0.1 dB in every period. Otherwise the painter uses the popup's bound-proportional
sampling and 0.05 dB confidence budget, batches the selected rays on the graphics card and
applies the sampling weights to their spectra.
A pixel inside a building shows the building's loudest façade, as the popup
inside it does: of the receivers 0.1 m in front of its walls, a party wall's left out, the three
whose painted pixel in front is loudest are computed like pixels, without the building's own
reflection, and the loudest of them counts. Both choices go by the ground sources, as the popup's
do: the flights do not pick the façade.

A pixel's loudness is computed as the popup computes it, at a coarser grain: the levels over time
in steps of 2 dB, two states of the weather, and four groups of the day's hours, one of the
evening's and three of the night's, where the popup takes 0.5 dB, ten states and every hour. Each
layer and the total are stored in steps of half a phon, 3.5 % of loudness; zoomed out, a pixel is
the mean loudness of the pixels under it.

</details>

## In the code

<details>
<summary>The painter's code: paint/ and paint-gpu/, the tiles it writes and its checks</summary>

`paint/` is the method and `paint-gpu/` (`qm-paint`) runs it by squares of zoom 12 in three stages:
the cores read the next square's tiles once, to the reach of its farthest pixel (the fine terrain
when a ray first needs it), and take its corners; two squares paint their pixels at once (a square
of many sources alone); two writers publish the squares before. The exact pairs and the aircraft boxes' sums at the block corners'
lattices run on the first CUDA card (`paint-gpu/kernels/`, the popup's physics in single
precision with fast math, each function citing its Rust) or on the cores. Four passes of a square
run at once, at most two launching their pairs on the card while the others list and sound their
pixels on the cores beside them; a pass gathers its next launch's pairs while the card evaluates the
one before.
The cores assemble sounds directly into chunks of at most 8,192 pixels, preparing the next chunk
while the card computes the current chunk's loudness.
The passes return in spatial order, so equal façade energies keep the same first candidate.
Each file's header says what it does; the blocks are `paint/src/blocks.rs`, their quiet
interpolation `paint/src/quiet.rs`, `quiet_corners.rs` and `quiet_samples.rs`, the hum
`paint/src/hum.rs` and `hum_samples.rs`, the aircraft boxes `paint/src/flights.rs` and
`flight_boxes.rs` (on the card `paint-gpu/src/aircraft.rs`), the aircraft spectra
`paint/src/flight_spectra.rs`, a
building's façade `paint/src/buildings.rs`, a pixel's loudness `paint/src/sone.rs` at
`popup::percentiles::Resolution::MAP`, and the tiles `paint/src/hm3.rs`: per layer and the total,
512 × 512 cells of twice the loudness level in phon (80 one sone, a doubling 20; 254 silence under
1/16 sone, 255 not assessed). Each tile is compressed when painted: PNG's Paeth predictor uses
the left, above and upper-left cells (zero outside the tile), and the cell minus that prediction,
modulo 256, enters one Brotli stream at quality 11 with a 512 KiB window. Among the measured
Brotli choices, this fixed predictor and quality fit the projected volume budget. The served
archives in the measured urban area shrink by 26 % against raw-cell Brotli at quality 9.
The browser's HTTP stack decompresses, and `frontend/src/lib/hm3-decoder.ts` restores the same prediction in place;
every cell, including silence and not assessed, returns exactly.

The painter writes one atomic HM3 square bundle, its directory pointing at these compressed tile
streams, identical tile grids encoded and stored once. Two writers compress and publish the bundles
while the next squares are painted; at most six squares' cell buffers are resident (two written,
two waiting, two painted), and a square is complete only after its bundle is durable
(`paint-gpu/src/square_writer.rs`). `qm-paint merge` copies
independent painters' bundles into one tree atomically, without decoding or replacing completed
squares. Identical completed copies are skipped on retry; conflicting squares stop the merge.
`paint/src/pyramid.rs` reads each compressed square once and carries the assessed-pixel counts and
unrounded loudness through the same means, the branches on all cores at once; it compresses each lower
tile once. A region of 16 × 16 squares (a tile of zoom 8) is packed as soon as it is painted
(`qm-paint pack-region`, `paint/src/region.rs`): its tiles from zoom 12 to 8 and its exact means are
kept, and the pack reuses them while the region's bundles are unchanged.
`pack.rs` copies these streams into one immutable PMTiles archive per layer, zooms down to 2,
ordered by zoom and Hilbert tile id. Identical streams share an archive range; gzip compresses its
index. An archive is read by byte range, so a tile needs no scan or server decompression, and the
same files support object storage. The manifest appears only after every archive is complete; the
server sends the stored stream with `Content-Encoding: br`. The manifest URL belongs to the tile
format's version, so an open page with an earlier decoder keeps its last compatible immutable build
until the page is refreshed.

Loud is one rule for the painter, its etalon and the hum: `popup::candidates::loud`, a bound reaching
`physics::bound::REACH_EDGE_LDEN_DB` (20 dB). `qm-paint-gpu check` measures the card against the
cores on random pairs, `check-loudness` on random pixels' loudness and `check-flights` on a square's
aircraft lattices; `etalon` paints the reference,
`compare` scores a map against it, and `popup` writes the popup's loudness at random pixels as a
sparse reference.

</details>
