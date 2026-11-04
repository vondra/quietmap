# quietmap.org

**Make noise visible. Make quiet possible.**

[quietmap.org](https://quietmap.org) is a world atlas of environmental noise: roads, railways,
aircraft, ships, industry and the buildings around you. Click any point and the noise there is
computed on the spot from every source around it; the map shows the same physics painted ahead of
time.

How it works is told once, on the site's About pages, each from the simple to the exact and down to
the code in this repository:

- [Data sources](https://quietmap.org/about/data-sources): every dataset, with its link and date.
- [Data preparation](https://quietmap.org/about/data-preparation): how the data become the files a
  click reads.
- [Noise at a point](https://quietmap.org/about/noise-at-a-point): how a click computes the noise
  at its point.
- [The noise map](https://quietmap.org/about/the-noise-map): how the map is painted.
- [Validation](https://quietmap.org/about/validation): the model against real measurements.

Their text is in [`frontend/src/about/`](frontend/src/about/).

## Build and test

The Rust structures preparer links GEOS 3.12 or later and GDAL; install their development
libraries and `pkg-config` before building. Publisher GeoParquet input is read in Rust.

    npm ci --prefix frontend && npm --prefix frontend run build
    npm ci --prefix server
    PORT=... QM_POPUP_BIN=... QM_RASTER_BIN=... QM_PREPARED_DIR=... QM_YEARS=... QM_TILES_DIR=... npm --prefix server start

    ./scripts/check-fast.sh           # the gate: formatting, lints and tests of every part
    npm --prefix frontend run e2e     # the built map in Chromium, every API answer mocked in the page

The server's routes and settings are in [`server/README.md`](server/README.md).

## Prepare a year

Stage the manifest's raw inputs and model tables at `INPUTS/YEAR/ID`, including the
relative objects named by publisher inventories. Then run the committed builder:

    cargo build --release -p build
    target/release/qm-build year --year years/YEAR.json --sources INPUTS \
      --squares @SQUARES --out NEW_YEAR_TREE --scratch NEW_RUN_DIRECTORY

`--squares` accepts `X:Y,X:Y`, `@FILE` or `all`. The runner adds the required spatial
context: three OSM/raster rings, two structure/facility/terrain rings and one road/rail ring.
Stage publisher inputs and model tables only; the runner rejects conflicting aliases for
intermediates it produces before starting work. A world run checks its required model
tables first. The output and run directory must be new, disjoint and outside the input root. Repeating
the identical command resumes completed stages, whose logs and completion inventories
live under `steps/`. The prepared-tree view only links those completed outputs. Missing prerequisites block their descendants while independent
stages continue. `status.json` lists the result. A world receives its final completion
marker only after every stage and every required published model table passes.

The runner pins the manifest, publication tables and executable. Keep publisher inputs and
semantic options immutable during a run; `--threads` and `--memory-gb` may change on resume.
Resume checks source paths, file sizes and modification times, hashes top-level recipes up to
1 MiB, and refuses changed or missing completed outputs. This is a resume guard, not
a substitute for verifying the publisher downloads. Keep files referenced outside those
inventories, such as VRT source rasters, unchanged too. The selected road library directory
is inventoried as well, and inputs are rechecked before each stage is marked complete.
A changed input needs a new run
and output directory. A partial selection writes `partial.json` with its exact squares,
without the release completion marker, so the map cannot serve it as a world. `--boxes`
filters aircraft traces for diagnostics and also prevents a world completion marker;
it does not claim complete aircraft coverage of the selected squares. With no boxes,
all admitted flights are decoded. `--threads` controls aircraft decoding, `RAYON_NUM_THREADS` controls other
parallel stages, and `--memory-gb` limits aircraft scheduling. Other preparers have their
own memory costs. Structure observations spill footprints to disk and sort one square at a time; the
largest selected square and admitted national attributes determine their memory use.
Raster workers each own their GDAL datasets; `RAYON_NUM_THREADS` also limits their
concurrency. A raster cache is bound to its manifest, builder and normalized recipes;
retries keep complete atomic four-band squares. Other preparation stages resume at stage
boundaries. Logs retain failed attempts. Budget one scratch filesystem for all retained
intermediates and the aircraft shuffle before starting a world; completion markers keep
those outputs available for resume.

If road publisher formats need a newer GDAL runtime than the raster plan permits,
`--road-library-path DIRECTORY` supplies `LD_LIBRARY_PATH` only to road normalization.
The raster stage still verifies its declared GDAL/PROJ runtime. The runner derives bus
routes, worship places and venues from the admitted OSM state using `osmium` and Python 3;
their converters are in `fetch/` and compiled into the builder. Auxiliary climate,
water, turbine and national-wall inputs must already be staged. Historical models and input coverage must be
completed before attempting a publishable world tree.

Aircraft retain whole traces, including their remote origins. For a selected area the
runner prepares missing flight-path height squares on demand from the same admitted raw
DEM model in the aircraft stage's scratch directory. It never reads served terrain or
substitutes zero for a missing prepared file. `aircraft-segments --scratch NEW_DIRECTORY`
enables the same behavior when running that stage separately. Completed height squares
survive a retry only with the same manifest, builder and normalized source recipes. The original height model's
verified ocean coverage and water-fill rule still apply; raw global DEM coverage is required.
Each aircraft worker owns separate raw-height datasets; concurrent requests for the same
square share one computation. A small selection without diagnostic boxes still decodes
the whole flight window and may require heights worldwide. A rejected baseline or increment
day stops the year stage until the manifest's window matches its admission receipt.

## Licence

[Apache License 2.0](LICENSE). If you build on this code or improve it, we would love to hear from
you at info@quietmap.org: a request, not a condition. Third-party material in the repository keeps
its own terms ([`NOTICE`](NOTICE)).
