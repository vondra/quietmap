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

    npm ci --prefix frontend && npm --prefix frontend run build
    npm ci --prefix server
    PORT=... QM_POPUP_BIN=... QM_RASTER_BIN=... QM_PREPARED_DIR=... QM_YEARS=... QM_TILES_DIR=... npm --prefix server start

    ./scripts/check-fast.sh           # the gate: formatting, lints and tests of every part
    npm --prefix frontend run e2e     # the built map in Chromium, every API answer mocked in the page

The server's routes and settings are in [`server/README.md`](server/README.md).

## Licence

[Apache License 2.0](LICENSE). If you build on this code or improve it, we would love to hear from
you at info@quietmap.org: a request, not a condition. Third-party material in the repository keeps
its own terms ([`NOTICE`](NOTICE)).
