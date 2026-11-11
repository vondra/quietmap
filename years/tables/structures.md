# Structure height tables

`structure-height-model.json` holds the common height ladder's coefficients: measured bounds,
minimum usable Overture height, ascending area edges and their terminal bin, piecewise affine
floor ranges, inverse storey coefficients and wall means. Heights are metres and areas m².
The manifest admits their observations. Completion requires byte equality with the published
table. The model never selects a year, publisher or country-specific algorithm.

## Frozen prepared model

The 2026 table transcribes `scripts/structures/structure_heights.py` at `24980735` exactly.
Its regression oracle reads this committed table. These are retained producer observations,
not a fresh validation of the fit:

- The area typology was compared on September 25 against NRW LoD1 mean roof heights, IGN
  BD TOPO mean of eave and ridge, and 3DBAG's 70th roof percentile. In the 60–150 m² bin,
  German farmhouses averaged 9.1 m (282 buildings), French ones 6.5 m (108); the retained
  coefficient is 7.4 m. The French edition is not pinned, so the manifest conservatively
  dates this comparison to its recorded comparison year.
- Floor heights were checked against 105,409 IPR Prague buildings, 2,022 in Saint-Maur,
  787 in Auneau, 1,666 in Amersfoort and 1,586 in Oosterwolde. One floor includes an attic;
  two or three use 3 m per floor, and taller buildings add 2 m for the roof.
- The inverse storey estimator subtracts 1 m and divides by 3 m; the original comparison
  reported 0.41 storeys mean absolute error on 6,061 registry-counted buildings in five windows.
- The 2.5 m Overture threshold followed 1,002,248 zero-height footprints and 13,211 exactly
  1 m footprints in the inspected Australian window. Measured heights were clamped to
  2.5–250 m. These retained bounds are not observations for an earlier map.
- Unmapped walls retain 3 m globally and means of 3.88 m in Germany, 4.45 m in the US and
  3.6 m in Austria. Sources are BMDV/FBA's *Lärmschutz an Bundesfernstraßen 2022*
  (10.0 million m² / 2,576 km), FHWA's 2022 inventory (8,783 walls, length-weighted), and
  ASFINAG's end-2022 inventory (about 5 million m² / 1,406 km). The rounded aggregates do
  not re-derive every decimal of the retained means; the original coefficients are preserved.

## The year's fit

`qm-build structure-height-model` fits the same table format. The year run runs it as a step after
`osm` and `structure-observations`, before `structures`, over the squares structures prepares
(every square in a world run). It reads the squares a survey can reach: those meeting the coverage
of a measured relative-height raster or touching a degree tile of an admitted
`prepare-building-heights` survey. Each mapped building there with an outline, not a ground
activity or an open roof, takes on its own outline the roof structures would take as measured
(`structures::prepare::surveyed_roof`): a raster's mean, needing three valid pixels and 30 %
coverage, replaced by the survey footprint matched at an overlap of at least 0.5 of the union or
by containment. A roof below 2 m, the raster's pixel rule, is no roof: a survey's empty or zero
records leave the building out instead of lowering the table's bound.

Least squares of these roofs on mapped floors give the metres a floor and the roof; five area
quantiles of the buildings mapping neither height nor floors, and their median roofs, give the
area bins; the median mapped wall of the same squares gives the unmapped wall default. The lowest
roof is the low measured and Overture cutoff, and the output's Int16 range the upper bound. Every
building weighs one, so the largest survey dominates. A selection whose surveyed squares hold
fewer than 100 floor pairs, 500 such buildings or 100 mapped walls has no fit and builds no
structures. The run publishes the table and its support (`structure-height-model.samples.json`:
the counts and coefficients, and each surveyed square's buildings and walls) in the year tree's
`tables/`; the release commit copies it here.

The 2024 table fitted on Prague's raster alone (2.456 m a floor plus 4.796 m; area medians 3 to
7 m) read low wherever it was not fitted: its area table by 1.1 m in the Netherlands, 2.2 m in
North Rhine-Westphalia and 5.4 to 5.7 m in Tokyo and Osaka. The same two laws fitted on Prague,
3DBAG and North Rhine-Westphalia's LoD1 cut the bias in the held-out New York and Japanese surveys
by 0.8 to 1.7 m; Japan still reads 1.4 to 3.7 m low under any European fit, so its own PLATEAU
cities enter the fit. The 2026 table predates the step (the frozen model above).

The existing obstacle reader still stands an unmeasured wall at 4 m. This port preserves that
reader and its prepared-input contract; resolving the inherited override requires a separate
validated change to the obstacle outputs.
