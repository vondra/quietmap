---
title: What's new
intro: Recent improvements to the data and the model, and what we are working on next.
nav: hidden
---

## What's new

The map has a new name and a new address: **quietmap.org**.

We stopped treating buildings as a coarse grid and moved to their real outlines
and real heights. In Prague we replaced a flat "eight metres" guess with over
174,000 buildings measured from aerial survey data, so the tenement blocks of
Žižkov finally stand at their true 19 to 27 metres. Where screening decides the
answer, the change is large: at a receiver 120 m from the M25 motorway in
England our last estimate before the fix (28 July 2026) was 70.09 dB, 12.9 dB
above the official English noise map's 57.22 dB at that spot, because the
road's cutting and its noise wall were missing from our data. It now sits about
two decibels above it.

A physics review also turned up several errors we have since fixed — ground
attenuation was disappearing over paved surfaces, and the shadow behind a
wall switched on too abruptly. Forest is no longer a yes/no flag but a
continuous canopy density, and traffic data improved for Thailand, Mexico
and Japan.

Ground attenuation now follows the European CNOSSOS-EU standard exactly,
computed separately for each frequency band — low and high tones behave
differently over grass than over asphalt. We check it against the standard's
own published test cases.

These improvements reach the map with the next worldwide recomputation —
the tiles you see today still show the previous model run.

## What we are working on

We are building a new version of the computation model. Today a stretch of
road is computed in a simplified way — through a single representative path
to each location — and the shadow behind an obstacle comes out too smooth,
so the map can show noise travelling too "straight" past a building. In the
new version every computed point gets its own geometry: its own distance,
its own terrain, its own obstacles.

In practice you will see the difference behind buildings: a more realistic
noise shadow with sharp edges at the real corners of the building, instead
of today's blurry transition. No dates promised — accuracy gates come first.

*Last updated 2 September 2026.*
