# Data pipeline runbook

Release `r260904` is a best-available snapshot, not a promise of full-calendar-2026 observations.

## Data layout

Pass the source root and working prepared year explicitly to each producer. Keep source
inputs immutable and write intermediates into a separate working tree. Publish a complete,
validated prepared generation together with its catalogs and source receipts.

Every consumer uses current-release sources. Retain any separate structures output while
prepared squares link to it. Materialize links before removing a work tree or publishing a
standalone copy. Never synthesize an empty structure file to cover an unfinished square.

## One-command world build

`scripts/build-world.py --config WORLD.toml --output NEW_GENERATION --scratch NEW_SCRATCH`
coordinates a fresh world through all seven Arrow layers. Add `--plan` to print its
actual commands without starting producers. The two destinations must be empty and
separate from sources. Existing partial work is retained on failure; the controller
never adopts it implicitly. Use the recorded producer commands for a reviewed recovery.

The TOML file has `[build]` keys `as_of_date` (YYYYMMDD string), `aircraft_anchor`
(YYYY-MM string), `memory_gib` and `threads` (positive integers). `[sources]` supplies
absolute paths named `planet`, `rasters`, `enrichment`, `boundaries`, `city_boundaries`,
`overture`, `ghsl`, `regional_heights`, `airline` and `general_aviation`.
`rasters` is an already published native raster year, `city_boundaries` is the ADM2
cache, and `regional_heights` retains the measured regional raster or VRT dependencies.
Download and validate new source versions before freezing these inputs.

The controller records source/code SHA-256 and execution receipts in `build.sqlite`,
including resolved height-raster dependencies. It prepares the complete z9 directory
set before parallel writers, then runs OSM, admin, national buildings, structures,
ordered road/rail/industry chains and the pinned hybrid aircraft window. Buildings
precede structures; both precede service-tree and built-up road inference. Disjoint
writers can overlap within the configured cgroup memory budget. Source cache changes
invalidate completion, including replaced symlink targets or newly added files.

Only a successful all-layer audit writes the final `prepared/YEAR/inputs.sqlite`
Arrow manifest and marks `build.sqlite` complete. Linked native rasters remain required
for the generation's lifetime. Source pinning and the fixed sampling dates make the
inputs reproducible; whole-world byte-identical output has not yet been demonstrated.
This controller does not repaint heatmaps or change a served generation.

## Base rasters and OSM

The geophysical channels are DEM, forest and IMD. Their runtime files are
`z9/x/y/dem.i16be`, `forest.u8`, `imd.u8`; `rasters.sqlite` is the publication
authority. Each channel covers all 262144 z9 coordinates. A catalog entry with
no digest declares an empty square; it does not require an empty raster file.

The source converters live in `scripts/rasters/`; their geographic intermediate
rasters must be repacked using the native z9 raster writer before consumption.
Keep the catalog with its native files. A legacy `<year>/rasters/` directory
alone is not a complete native prepared year.

`scripts/osm-extract.sh` writes roads, railways, buildings, industrial, leisure,
barriers, airport areas and airport lines into the same z9 layout. Absent OSM
layer files mean no rows for that layer. A roads square need not contain buildings.
Leisure contributes to the building noise layer; barriers provide screening.
These are not seven independent raster inputs.

## Structures and geography

`scripts/structures/build-structures.py` joins OSM buildings/barriers with
Overture footprints, GHSL heights, and regional height rasters where available.
The builder writes even completed empty squares and validates the OSM emission
view. Preserve the regional IPR input for the two Prague reference squares.

If structures were built into a separate tree, validate their schema and grid before
joining them into the prepared year. Reconcile every pending square after the builder
finishes. Preserve existing files and producer completion receipts.

Run `scripts/admin/build_admin.py --prepared-dir YEAR --boundaries CGAZ --jobs N`
after extraction. It writes `admin.bin` and embedded road/rail/industrial country
columns. It holds `YEAR/.admin-build.lock` exclusively. Do not overlap a writer
that ignores this lock with admin.

## Enrichment and parallel work

The manifest in `pipeline/chain/manifest.ts` is the ordered list of ported writers.
From `pipeline/`:

```bash
npx tsx chain/run.ts --scope world \
  --prepared-dir "$PREPARED_YEAR_DIR" \
  --enrichment-dir "$ENRICHMENT_DIR" \
  --boundaries "$BOUNDARIES" \
  --as-of-date 20260909 \
  --dry-run
```

Remove `--dry-run` only for an authorized run. A regional canary uses an isolated
prepared tree with real copies of writable Arrows and a complete read-only halo.
The old `--scope country:CZ` was unsafe: global writers still changed the whole
input tree. It is rejected.

`--layer buildings|roads|railways|industrial` selects an independent output family.
National buildings can run during admin: they use their source coordinates and only
write buildings. After admin finishes, railways and industrial may run in parallel.
Finish national buildings before starting the roads chain: service-tree derives
traffic demand from their attributes. Finish structures footprints before built-up.
Within each layer keep manifest order; do not run two writers of one layer together.
Each child exclusively locks its output family. Roads, railways and industrial also
hold the admin lock shared, excluding the admin writer for their lifetime. Step exit and elapsed seconds
are emitted as JSON. `--from STEP` resumes within the selected family.

Service-tree visits every road square, including those without buildings. Empty
building demand retracts its own stale estimates and preserves measured traffic.
National buildings writes only existing `buildings.arrow` rows.

After national building refinement, refresh affected `structures.arrow` files with
the original GHSL/regional inputs. Both emission attributes and screening heights
are embedded in structures; enrichment alone cannot update them. Refresh downstream
obstacle indexes after this step. Do not reuse a stale index as a final artifact.

National road coverage is limited to actual manifest adapters. Compare used
`source_id` distributions with the reference generation before claiming equal quality. Missing adapters
are a backlog, not evidence that global defaults are equivalent to measured censuses.

GTFS uses `lib/railway-gtfs-feeds.ts` for layout and validity. Verify all selected
feeds before a long rail run. An expired feed must be refreshed from its real source;
never alter calendar dates to bypass the gate. Download before enrichment, preserve
the source archive and digest, then use `--enrich-only` for national roads.

## Aircraft

`engine/aircraft-extract` and `scripts/run-aircraft-extract.sh` are already ported
to z9. Reuse validated Stage 0/1 segment files; do not re-extract them just because
the world prepared tree has no aircraft output yet.

The current primary window has 12 dates, 2025-10-01 through 2026-09-01. GA uses the
2025-09-02 through 2026-09-01 source window with the absent 2026-05-06 day excluded
by receipts. Use the recorded day list and class normalization, not a hardcoded
365 divisor. Primary segments are split across two roots; the CLI accepts repeated
`--segments-dir` arguments. Keep the input paths and receipts in the execution record.

After Stage 0/1: world shuffle, airport discovery, Stage 2A airborne, Stage 2B
cruise, Stage 2C ground operations and local airport summaries. This work can overlap
admin/enrichment because it writes separate artifact names. It requires complete
rasters, airport inputs, verified windows, and a measured shuffle disk budget.
Prague-only throughput is not a world completion forecast. New aircraft support
squares require final structures/admin coverage before serving.

## Final derived artifacts and serving

Validate all seven noise layers: road, rail, building, industrial, aircraft airborne,
aircraft cruise and aircraft ground. Validate absence through producer coverage and
receipts, not by requiring an Arrow file for an empty layer in every ocean square.

After the final Arrow/structures generation, refresh obstacle indexes. Any persistent
popup acceleration output must identify its input generation and preserve the exact
kernel as its correctness reference.

Compare actual popup levels, source provenance, counts and screening against the reference generation
on city, airport, quiet, coast, border and polar cases. Include adjacent clicks in
one process. A same-coordinate result-cache hit does not demonstrate nearby-click
speed. Preserve the exact kernel as the reference.

Publish only after validation, with all linked inputs materialized or retained for the
lifetime of the generation. A future repaint consumes that validated generation. Imported
PMTiles must match one pinned publisher manifest by size and digest; their presence does
not prove that the popup inputs are complete.
