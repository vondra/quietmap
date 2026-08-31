# Quiet Map

Global environmental noise atlas: computed sound levels (Lden) from roads,
railways, aircraft, industry, buildings and settlements, for the whole world
at about 12 m base resolution (with an optional ~6 m detail tier), served as an
interactive web map with per-point source
breakdowns. The acoustic model follows CNOSSOS-EU for surface sources with
ISO 9613-2 propagation and a Doc 29-inspired aircraft NPD; formulas, constants
and intentional simplifications are specified in `engine/noise-compute/SPEC.md`.

Live instance: <https://quietmap.org>

## Repository map

```
engine/       Rust crates — noise-compute (the shared acoustic kernel),
              tile-painter (batch tile builder + tile store), osm-extract and
              aircraft-extract (OSM PBF / ADS-B → H3-res-4 arrow),
              raster-reader, source-reader (NAPI addon for point queries),
              noise-gpu (CUDA ports of the scatter kernels)
frontend/     The map — React 19 + MapLibre + deck.gl (Vite build)
server/       Fastify — tile serving, point noise queries, search
pipeline/     TypeScript enrichment — patches OSM defaults with
              measured/registry data (traffic, industry, buildings) per country
scripts/      Product entry points: run-extraction.sh (rasters + OSM extract),
              build-heatmap.sh (whole tile build), osm-to-h3r4.sh,
              check-fast.sh, dataset-year.json
docs/         about/ (public About + methodology pages) · standards/
benchmarks/   Validation fixtures (popup/world point sets)
```

## Build & run

Requirements: Node.js 22 and the Rust toolchain pinned by
`rust-toolchain.toml`.

```bash
./start.sh
```

`start.sh` is standalone: it builds the engine native addon, the frontend and
an immutable compiled server release, activates the release, and serves on
`$PORT` (default 8520) — no external orchestration needed. Callers that own
the process lifecycle themselves (e.g. a systemd unit) build and activate
without serving:

```bash
QM_BUILD_ONLY=1 ./start.sh
```

## Quality gate

```bash
./scripts/check-fast.sh [node|rust]
```

The required, data-free gate: server smoke/tests, frontend lint/build, offline
pipeline tests, and rustfmt + Clippy + tests for the engine crates. The
optional first argument runs only one side (CI split jobs). Keep it green
before every commit.

## Dataset year

The default dataset year is pinned in `scripts/dataset-year.json`
(`current_year`); the `DATA_YEAR` environment variable overrides it. The pin
controls which `data/prepared/<year>/` and `data/tiles/<year>/` trees a build
produces and the server reads.

## Data layout (not in git)

`data/` is gitignored and large. Expected shape:

```
data/prepared/dem/             1°×1° DEM rasters, 30 m (shared across years)
data/prepared/rasters/         forest/IMD rasters (shared)
data/prepared/<year>/h3r4/     H3-res-4 source + vector-obstacle Arrow extracts
data/tiles/<year>/pmtiles/     built tile pyramids + manifest
```

`H3R4_DIR` / `PMTILES_DIR` env vars point a server at data living outside the
checkout. `scripts/run-extraction.sh` fetches and prepares the inputs, then
`scripts/build-heatmap.sh` computes the tiles.

## Scope

This repo is the whole product: model, pipeline, tiles, web app. Deployment
and orchestration of the author's own cluster (multi-checkout hosting,
distributed world-build fleet) is intentionally out of scope here and lives in
a separate private repository.

## License & contact

See `docs/about/credits.md` for data-source attribution (OSM, GLO-30, SRTM,
Overture, WorldCover, IMD, ADS-B) and terms. info@quietmap.org.
