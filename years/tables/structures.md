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

## Historical calibration

`qm-build structure-height-model` fits the same table format from the admitted OSM buildings
and measured relative-height raster. It uses least squares on mapped floors and measured
roofs, five area quantiles of buildings lacking both height and floors, and the median of
mapped wall heights. The receipt records the selected squares, year, sample counts and fitted
coefficients. The minimum measured roof supplies the low-height cutoff; the output's Int16
range supplies the upper representation bound. Later survey coefficients are not copied.

The 2024 table uses IPR's 2023 survey and the OSM state immediately before 2025. Its roofs come
only from Prague, although its mapped walls cover the selected squares. This is a reproducible
prototype, with the historical map still unbuilt: it has no independent global validation.
The 2026 fit used several countries and manually chosen partitions; the historical fit derives
its partitions from its own observations. Both use the same height ladder and table format,
but a map difference cannot yet be attributed solely to changes in physical buildings.
Reproducing the frozen table takes precedence over changing that fitted method in this port.

The existing obstacle reader still stands an unmeasured wall at 4 m. This port preserves that
reader and its prepared-input contract; resolving the inherited override requires a separate
validated change to the obstacle outputs.
