---
title: The noise map
intro: What the colours mean, and how they are computed ahead of time for every pixel with the click's physics.
---

The colours show how loud each place sounds, in sone, as a click there would answer: the map is
computed ahead of time, pixel by pixel, from the same data and physics. For the exact answer at a
place, click it. A place not yet painted is clear, without the quiet zones' green, and its hover
reads "—"; a click there computes it all the same.

## Colours

One colour for each doubling of loudness: under 1 sone, about as loud as a quiet room, the map is
clear; then 2 sone pale green, 4 yellow, 8 orange, 16 red and 32 maroon, held flat above. Colours
blend smoothly in between, and their opacity rises with loudness over the base map. The order is the
one air-quality maps taught, and the lightness falls with loudness, so the scale still reads in grey
and with red-green colour blindness.

## Layers

The map shows any of the noise layers together: roads, railways, industry, buildings, ships and
aircraft. All six read the painted total and one alone its own loudness. Loudness does not add
like energy, so two to five layers are estimated from each layer's own: their loudness raised to a
power that grows with the loudest layer's level (1.0 at 22 phon, 1.9 at 62, 3.1 at 87 phon),
summed and taken back. Against the painted total of all six this is within 1 phon at 90 % of the
places where two layers or more sound, and within 2 phon at 98 %. Quiet zones wash green every
place where the total stays at or below a chosen loudness, from 0.25 to 4 sone.

Advanced shows the data every click computes over: elevation, forest, hard ground, buildings by
height, road traffic (vehicles a day), trains a day, the other sources (turbines, bells, terraces,
car parks, airports) and noise barriers; each layer switched on shows its legend.

## How it is painted

The map is painted in tiles of zoom 12, 1,024 × 1,024 pixels (6 m apart in Prague), each in
blocks of 16 × 16 pixels (100 m in Prague). A ground source counts as loud at a point where its
upper bound there reaches 20 dB.

- At every block corner, each loud source is computed exactly.
- A block's pixels compute exactly the loud sources that cross the block or one of the eight around
  it and, layer by layer, the loudest at the block's corners until what each corner leaves out
  weighs at most 20 % of the layer's loud sources at the block's quietest corner.
- What is left out is blended between the block's four corners, and weakened at a pixel as much as
  the block's computed far sources are there, as behind a house.
- The hum, the sources under 20 dB, is estimated with the click's physics and error budget at
  points 64 pixels apart (390 m in Prague), and blended between them in decibels: it falls off
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
<g fill="currentColor"><text x="30" y="291">exact: loud sources in P's 3 × 3 blocks</text><text x="30" y="309">block corners (16 px, 100 m): every loud source</text><text x="30" y="327">a piece the corners admit, exact at P</text><text x="30" y="345">hum points (64 px, 390 m): quiet sources</text></g>
</svg>
<figcaption style="margin-top: 0.5rem; font-size: 0.8125rem; line-height: 1.45; color: var(--muted-foreground)">Part of a tile painted at 1,024 pixels, distances in Prague. Road A crosses the blocks around pixel P and is computed exactly there. Road B is computed at the corners of P's block; its loudest pieces there are computed exactly at P too, their rays here passing a house, and the rest of it is blended between the corners. The quiet sources come from the hum points.</figcaption>
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
energy by at most 0.1 dB in every period. Otherwise the painter uses the click's bound-proportional
sampling and 0.05 dB confidence budget, batches the selected rays on the graphics card and
applies the sampling weights to their spectra.
A pixel inside a building shows the building's loudest façade, as a click
inside it does: of the receivers 0.1 m in front of its walls, a party wall's left out, the three
whose painted pixel in front is loudest are computed like pixels, without the building's own
reflection, and the loudest of them counts. Both choices go by the ground sources, as a click's
does: the flights do not pick the façade.

A pixel's loudness is computed as a click computes it, at a coarser grain: the levels over time in
steps of 2 dB, two states of the weather, and four groups of the day's hours, one of the evening's
and three of the night's, where a click takes 0.5 dB, ten states and every hour. Each layer and the
total are stored in steps of half a phon, 3.5 % of loudness; zoomed out, a pixel is the mean
loudness of the pixels under it.

## How close it comes to a click

The painter is held to two references. The etalon is the same tile with every loud ground source
and the flights computed exactly at every outdoor pixel, the hum, the flights' spectra and the
loudness grain taken as the painter takes them: over the pixels of 1 sone or more, each layer and
the total may differ from it by more than 0.5 phon at no more than 20 % of them, by more than 1
phon (7 % of loudness) at no more than 1 %, by more than 3 phon at 0.01 %
and by more than 6 phon at 0.001 %, with a bias under 0.5 phon; at most 0.25 % may fall on the
other side of 1 sone, and a pixel under 1 sone may be off by at most 10 phon. And where the map is
clicked, it should agree with the click within 1 phon.

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

## In the code

`paint/` is the method and `paint-gpu/` (`qm-paint`) runs it by squares of zoom 12 in three stages:
the cores read the next square's tiles once, to the reach of its farthest pixel (the fine terrain
when a ray first needs it), and take its corners; two squares paint their pixels at once (a square
of many sources alone); a writer publishes the squares before. The exact pairs and the aircraft boxes' sums at the block corners'
lattices run on the first CUDA card (`paint-gpu/kernels/`, the popup's physics in single
precision with fast math, each function citing its Rust) or on the cores. Four passes of a square
share the card.
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
streams, identical tile grids encoded and stored once. A writer compresses and publishes it while
the next squares are painted; at most five squares' cell buffers are resident (one written, two
waiting, two painted), and a square is complete only after its bundle is durable
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
`compare` scores a map against it, and `popup` writes the click's loudness at random pixels as a
sparse reference.
