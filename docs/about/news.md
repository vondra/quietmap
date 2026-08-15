---
title: What's new
intro: Recent improvements to the data and the model, and what we are working on next.
nav: hidden
---

## What's new

The map has a new name and a new address: **quietmap.org**.

Over the past weeks we stopped treating buildings as a coarse grid and moved
to their real outlines and real heights. In Prague we replaced a flat
"eight metres" guess with over 174,000 buildings measured from aerial survey
data, so the tenement blocks of Žižkov finally stand at their true 19 to 27
metres. We checked the approach against official UK measurements along the
M25 motorway: our estimate used to be 13 dB too loud there; it now sits a
quarter of a decibel from the measured value.

A physics review also turned up several errors we have since fixed — ground
attenuation was disappearing over paved surfaces, and the shadow behind a
wall switched on too abruptly. Forest is no longer a yes/no flag but a
continuous canopy density, and traffic data improved for Thailand, Mexico
and Japan.

These improvements reach the map with the next worldwide recomputation —
the tiles you see today still show the previous model run.

## What we are working on

We are building a new version of the computation model. Today a stretch of
road is computed in a simplified way — through a single representative path
to each location — and the shadow behind an obstacle comes out too smooth,
so the map can show noise travelling too "straight" past a building. In the
new version every computed point gets its own geometry: its own distance,
its own terrain, its own obstacles.

At the same time we are moving to ground attenuation exactly as the
European CNOSSOS-EU standard defines it, computed separately for each
frequency band — low and high tones behave differently over grass than over
asphalt.

In practice you will see the difference behind buildings: a more realistic
noise shadow with sharp edges at the real corners of the building, instead
of today's blurry transition. No dates promised — accuracy gates come first.
