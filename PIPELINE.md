# Data pipeline runbook (release r260904, world rebuild)

## Vintage policy (owner 2026-09-04)

`2026` is a release SERIES (`r260904…`), not a vintage promise. Each release
is the best-available snapshot: planet dated ~Aug 31, ADS-B airline window
Jul–Jun, rasters mixed fixed vintages (WorldCover 2021, Hansen loss→2024,
TCD 2023, GHSL R2023A heights). A true "2026 as of Dec 31" (full-year
aircraft, winter planet) rebuilds as a refresh in early 2027 — this runbook
is written so an agent can rerun it unchanged.

Reproduce the whole world from fresh sources, in order. Every step reads
finished sources off `readmostly1`, works on mixeduse (`/tmp` + spill on the
`mixeduse1` disk, intermediates on `mixeduse2/r260904/work`), and promotes
finished trees to `readmostly2` once, at the end (see `AGENTS.md` §Disks).
`<src>` = `/data/readmostly1/r260904/source/2026`,
`<work>` = `/data/mixeduse2/r260904/work`,
`<prep>` = `/data/readmostly2/r260904/prepared/2026`.

## 1. Sources

| source | fetch | refresh next year |
|---|---|---|
| OSM planet | `scripts/source/download-planet.sh YYMMDD` → `<src>/osm/` | new Monday dated file, rename release |
| Overture buildings | `OVERTURE_PARQUET_DIR=<src>/overture/parquet scripts/overture/download-overture-world.sh` (resume-safe) | rerun, same dir |
| Copernicus DEM GLO-30 | already in `<src>/dem/copernicus-glo30` (no fetch script yet — TODO: script the AWS `copernicus-dem-30m` sync) | re-sync COG tree |
| WorldCover 2021 | `scripts/rasters/download-worldcover.py` → `<src>/vegetation/worldcover-2021` | fixed vintage 2021 unless ESA re-releases |
| Hansen GFC + TCD + IMD | reflinked from frozen pre2609 (`cp --reflink=always`) → `<src>/forest-sources/`, `<src>/imd/` | Hansen: new GFC release; TCD: EEA CLMS; IMD: Copernicus CLMS |
| GHSL heights | `scripts/rasters/download-ghsl.sh` → `<src>/buildings/ghsl` | fixed R2023A vintage unless GHSL re-releases |
| ADS-B airlines | `scripts/download-adsbexchange.py` (port from dev1) → 12 first-of-month days, already in `/data/readmostly1/adsb` (33 GB) | rerun with new anchor for new days |
| ADS-B full (GA+airlines) | already harvested: 882 prod days 2024–2026 (`vYYYY.MM.DD-planes-readsb-prod-0.tar.aa/ab`) on he84 `/storagebox/adsb` (2.2 TB, May-2026 harvest per scc archive); rsync read-only to `mixeduse2/r260904/source/adsb` (too big for readmostly free space — never written by the pipeline) | top up new days from `adsblol/globe_history_YYYY` GitHub releases |

## 2. Rasters → `<work>/rasters/{dem,forest,imd}`

DEM (~1 h), WorldCover IMD proxy + fallback forest (~1 h), and continuous
forest run in parallel; IMD overlay runs after WorldCover:

```bash
DEM_SRC=<src>/dem/copernicus-glo30 DEM_DST=<work>/rasters/dem JOBS=12 \
  scripts/rasters/convert-dem-copernicus.sh
WC_SRC=<src>/vegetation/worldcover-2021 \
  FOREST_DST=<work>/rasters/forest-fallback IMD_DST=<work>/rasters/imd JOBS=8 \
  scripts/rasters/convert-worldcover.sh
TCD_DIR=<src>/forest-sources/tcd/2023 \
  HANSEN_DIR=<src>/forest-sources/hansen/GFC-2024-v1.12 \
  FOREST_DST=<work>/rasters/forest \
  scripts/rasters/convert-forest-continuous.sh --all   # needs TILE_LIST=land tiles
IMD_SRC_ROOT=<src>/imd IMD_DST=<work>/rasters/imd \
  scripts/rasters/convert-imd-overlay.sh
```

`--all` needs `TILE_LIST` (one `N50E014` per line; r260904 used the DEM COG
inventory at `<work>/forest-all.txt`). A `--list FILE` subset works for a
region first. WorldCover forest goes to `forest-fallback` (gap filler only —
`convert-forest-continuous` skips existing tiles, so the good forest must own
`rasters/forest`). After all three, fill forest gaps poleward of Hansen
coverage from the fallback dir, then promote the tree to `<prep>/rasters/`.

## 3. OSM extract → `<work>/prepared/2026`

```bash
PBF_FILE=<src>/osm/planet-YYMMDD.osm.pbf OUTPUT_DIR=<work>/prepared/2026 \
  SCRATCH_ROOT=/data/mixeduse2/scratch scripts/osm-extract.sh
```

Writes `z9/<x>/<y>/{roads,railways,buildings,industrial,barriers,leisure,
airport_areas,airport_lines}.arrow` (3–6 h). Then structures, per square
(or `--squares-file` for a region):

```bash
qm_venv_python scripts/structures/build-structures.py \
  --prepared-dir <work>/prepared/2026 \
  --overture-parquet <src>/overture/parquet \
  --ghsl <src>/buildings/ghsl/<height-tif> --squares-file <work>/structures-squares.txt
```

## 4. Enrichment — NOT YET PORTED

dev1 runs `pipeline/chain/run.ts` (manifest order: column-parents →
global-priors → national per-country → city → heuristics incl.
`roads-service-tree` → taper → `gate-invariants` audit → structures).
Port the chain to the z9 arrows before claiming popup parity: without it,
speeds/AADT/defaults fall back to WORLD values. New-per-year inputs live
behind each enricher (GTFS feeds, city tables); the chain prints
`QM_COMPLETENESS` floors — a short feed fails a full-world run by design.

## 5. Admin + aircraft

- Run `scripts/admin/build_admin.py --prepared-dir <work>/prepared/2026
  --boundaries <source-boundaries.geojson>` after extraction and before
  country-dependent enrichment. It bakes each road/rail segment's geography
  and writes receiver defaults to `z9/x/y/admin.bin`, beside the unit's arrows.
  See `scripts/admin/README.md` for source validation and scoped builds.
- Aircraft arrows (`airborne/cruise/airport_traffic.arrow`): port dev1
  `engine/aircraft-extract` to z9, run HYBRID two-window (airline pass over
  the 12 adsbexchange days with `--class-filter non-ga`, GA pass over the
  adsb.lol cache with `--class-filter ga`, then merge) — dev1
  `scripts/run-aircraft-extract.sh` documents the invocations. Without the
  GA cache the merge runs airline-only and small airfields go quiet.

## 6. Promote + validate + serve

Bulk-copy the finished `<work>/prepared/2026` and `<work>/rasters` to
`<prep>/` (keep the mixeduse copies as backup). Validate: reference-square
roads schema (`square-store`), popup parity vs dev1 on reference squares,
then point the server at `<prep>` and deploy.

## 7. Heatmap repaint — LATER

`tile-painter` + tile serving are out of scope for the popup milestone.
Repaint only after steps 1–6 are green and validated.
