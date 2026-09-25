# Quiet Map

Global environmental noise atlas. Computes Lden from roads, railways, aircraft, ships,
industry and buildings for the whole world and serves it as an interactive map with a
per-point source breakdown.

Live instance: <https://quietmap.org> · Methodology: [`docs/about/methodology.md`](docs/about/methodology.md)

## Model

- **Emission:** CNOSSOS-EU for roads, railways and industry; ECAC Doc 29 with EASA ANP
  noise-power-distance data for aircraft; AIS vessel density with measured source levels
  for ships; a project-specific model for buildings and leisure facilities.
- **Propagation:** ISO 9613-2 in eight octave bands, CNOSSOS-EU ground effect, diffraction
  over terrain and vector building footprints, canopy-density vegetation loss. Receiver
  height 4 m.
- **Output:** a 512 px Web-Mercator raster at zoom 13 (about 6 m at 50°N) per layer, and
  an exact on-demand computation for any clicked point.

Formulas, constants and intentional simplifications are specified in
[`engine/noise-compute/SPEC.md`](engine/noise-compute/SPEC.md).

## Repository map

```
engine/       Rust workspace
                noise-compute        shared acoustic kernel (emission, propagation)
                grid                 Web-Mercator z9 square and z30 coordinate math
                osm-extract          OSM planet → per-square Arrow layers
                aircraft-extract     ADS-B archives → airborne, cruise and ground Arrow
                roads-finalize, railways-finalize, structures-finalize
                                     block layout and indexes for point queries
                raster-reader, square-store, arrow-batching
                source-reader        point-query engine (Node addon)
                tile-painter, relevant-source-gpu
                                     batch heatmap painter (CUDA)
pipeline/     TypeScript enrichment: national traffic censuses, GTFS timetables,
              industrial registries, building attributes (chain/manifest.ts is the order)
scripts/      build-world.py (one-command world build), osm-extract.sh, rasters/,
              structures/, ships/, square-country-city/, run-aircraft-extract.sh,
              check-fast.sh
server/       Fastify: tiles, point noise queries, search, About pages
frontend/     React + MapLibre + deck.gl (Vite)
docs/about/   Public About, methodology, credits and per-country data notes
benchmarks/   Reference point sets for popup regression
```

## Data layout

`data/` is not in git. All prepared data is keyed by Web-Mercator z9 square:

```
data/prepared/<year>/z9/<x>/<y>/
    roads.arrow  railways.arrow  buildings.arrow  leisure.arrow  industrial.arrow
    ships.arrow  airborne.arrow  cruise.arrow  airport_traffic.arrow
    airport_areas.arrow  airport_lines.arrow  barriers.arrow
    square-country-city.bin                  continent, country and metro of the square
    structures.arrow  structures.qoix        screening footprints and their edge index
    dem.u16le  canopy.u8  forest.u8  imd.u8  terrain, canopy height/cover, ground sealing
data/tiles/<year>/pmtiles/                   one PMTiles archive per layer, plus total
```

A raster file is present for every one of the 262,144 squares; a zero-byte file declares
verified absence (ocean). A missing file is an error. There is no catalog database:
identity is the release name plus the code version.

[`PIPELINE.md`](PIPELINE.md) is the runbook for building a generation from a planet
extract, ADS-B archives and the enrichment sources.

## Build and run

Requirements: a current Node.js LTS release and the Rust toolchain pinned in `rust-toolchain.toml`.

```bash
cargo build --release --manifest-path engine/Cargo.toml
npm --prefix server run build:native          # Node addon for point queries
npm --prefix frontend ci && npm --prefix frontend run build
npm --prefix server ci
(cd server && node scripts/build.mjs --require-native --frontend-dir ../frontend/dist \
           && node scripts/activate-build.mjs && npm start)
```

Each release directory holds `release.json`: the product commit, whether the checkout
was dirty, build time and SHA-256 of the packaged server source, frontend and native addon.
`PREPARED_YEAR_DIR` points the server at a prepared year outside the checkout;
`DATA_YEAR` selects the year; `PORT` and `HOST` select the listener.

## Quality gate

```bash
./scripts/check-fast.sh [node|rust]
```

Data-free: frontend lint, build and tests; server and pipeline type checks and tests;
Python script tests; Clippy with warnings denied and tests for the engine workspace. It must pass before every commit. [`AGENTS.md`](AGENTS.md) holds the coding
conventions.

## Scope

This repository is the whole product: model, pipeline, tile painter and web application.
Deployment and fleet orchestration are out of scope and live elsewhere.

## Credits and contact

Data-source attribution and terms of use: [`docs/about/credits.md`](docs/about/credits.md).
Contact: info@quietmap.org.
