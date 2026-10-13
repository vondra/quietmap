---
title: The noise map
intro: What the colours mean, and how they are computed ahead of time for every pixel with the click's physics.
---

The colours show how loud each place sounds, in sone, as a click there would answer: the whole map
is computed ahead of time, pixel by pixel, from the same data and physics. For the exact answer at a
place, click it.

## Colours

One colour for each doubling of loudness: under 1 sone, about as loud as a quiet room, the map is
clear; then 2 sone pale green, 4 yellow, 8 orange, 16 red and 32 maroon, held flat above. Colours
blend smoothly in between, and their opacity rises with loudness over the base map. The order is the
one air-quality maps taught, and the lightness falls with loudness, so the scale still reads in grey
and with red-green colour blindness.

## Layers

The map shows every noise layer together, or one alone: roads, railways, industry, buildings, ships
and aircraft. Quiet zones wash green every place at or below a chosen loudness.

Advanced shows the data every click computes over: elevation, forest, hard ground, buildings by
height, road traffic (vehicles a day), trains a day, the other sources (turbines, bells, terraces,
car parks, airports) and noise barriers.

## How it is painted

The map is painted one zoom-12 tile at a time, 1,024 × 1,024 pixels (6 m apart in Prague), in
blocks of 16 × 16 pixels (100 m in Prague). A ground source counts as loud at a point where its
upper bound there reaches 20 dB.

- At every block corner, each loud source is computed exactly.
- A block's pixels compute exactly the loud sources that cross the block or one of the eight around
  it and, layer by layer, the loudest at the block's corners until what is left out weighs at most
  20 % of its quietest corner's sound.
- What is left out is blended between the block's four corners, and weakened at a pixel as much as
  the block's computed far sources are there, as behind a house.
- The hum, the sources under 20 dB, is computed as a click computes it at points 64 pixels apart
  (390 m in Prague), and blended. It is never dropped.

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

Neighbouring blocks share their corners, so what is blended meets at them. The flights are blended
from the block corners, and a pixel inside a building shows the loudest outdoor pixel along its
walls, without the building's own reflection.

A pixel's loudness is computed as a click computes it, at a coarser grain: the levels over time in
steps of 2 dB, two states of the weather, and four groups of the day's hours, one of the evening's
and three of the night's, where a click takes 0.5 dB, ten states and every hour. Each layer and the
total are stored in steps of half a phon, 3.5 % of loudness; zoomed out, a pixel is the mean
loudness of the pixels under it.

## How close it comes to a click

The painter is held to two references. The etalon is the same tile with every loud source computed
exactly at every pixel: over the pixels of 1 sone or more, each layer and the total may differ from
it by more than 1 phon (7 % of loudness) at no more than 1 % of them, by more than 3 phon at 0.01 %
and by more than 6 phon at 0.001 %, with a bias under 0.5 phon. And where the map is clicked, it
should agree with the click within 1 phon: in central Prague (October 2026) it does at 96 % of the
outdoor pixels and at half of those inside buildings, which read a median 0.7 phon louder.

The exact computations run on a graphics card in single precision or on the processor's
cores in double; on 20,000 random pairs of source and pixel in Prague the two agree within 0.004 dB
for 99 % of them and 0.03 dB for 99.9 %.

## In the code

`paint/` is the method and `paint-gpu/` (`qm-paint`) runs it, a square of zoom 12 at a time: its
files read once to the reach of its farthest pixel, the exact pairs on the first CUDA card
(`paint-gpu/kernels/`, the popup's physics in single precision, each function citing its Rust) or on
the cores. Each file's header says what it does; the blocks are `paint/src/blocks.rs`, a pixel's
loudness `paint/src/sone.rs` at `popup::percentiles::Resolution::MAP`, and the tiles
`paint/src/hm3.rs`: per layer and the total, 512 × 512 cells of twice the loudness level in phon (80
one sone, a doubling 20; 254 silence under 1/16 sone, 255 not assessed), packed by `pack.rs` into one
PMTiles archive per layer with every zoom down to 2.

Loud is one rule for the painter, its etalon and the hum: `popup::candidates::loud`, a bound reaching
`physics::bound::REACH_EDGE_LDEN_DB` (20 dB). `qm-paint-gpu check` holds the card equal to the cores
on random pairs, `etalon` paints the reference, `compare` scores a map against it, and `popup` writes
the click's loudness at random pixels as a sparse reference.
