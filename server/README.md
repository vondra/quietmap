# Server

Fastify on Node 24 (TypeScript run as is, no build step). It serves the built map, the heatmap
tiles, the geocoder proxies and the streamed popup:

| route | answer |
|---|---|
| `GET /api/popup?lat=&lon=[&year=]` | one click, streamed as `application/x-ndjson` (below) |
| `GET /api/tiles-manifest` | the published heatmap generation: `{build, zoom, layers}` |
| `GET /api/tiles/:build/:layer/:z/:x/:y.bin` | one HM3 tile, Brotli, immutable; a missing tile is an empty 200 |
| `GET /api/search?q=&lat=&lon=`, `GET /api/reverse?lat=&lon=` | address suggestions and place names (public Photon geocoder) |
| everything else | the built frontend (`../frontend/dist`), or a 404 |

## The popup stream

`/api/popup` runs `qm-popup --prepared DIR --year YEAR --lat LAT --lon LON` for the click and
forwards every line it prints as one line of the response, flushed at once: the first after the
clicked tile and its neighbours are read, later ones as rings are added. Each line is the whole
answer so far (`partial` is true until the last one), never a delta; the fields are those of
`qm-popup` (`frontend/src/types/noise.ts`). The benchmark flags of `qm-popup` are never passed.

- `400` with `{"error"}`: `lat` must be a number within ±85.05, `lon` a number (wrapped to
  -180..180), `year` one of `QM_YEARS` (the first when absent).
- `503` with `{"error"}` and `Retry-After`: every slot computes and the queue is full. A few clicks
  compute at once (each uses every core); two per slot may wait.
- `429`: more than 5 requests per second from one client (an IPv4 address or an IPv6 /64). Tiles
  are never limited, local unproxied callers never are.
- A child that fails, exits without a final update, writes something that is no update, or runs
  longer than 30 s ends the stream with one line `{"error": "..."}` in words for the visitor; the
  exit status and stderr go to the server log. The updates before it are incomplete.
- A client that disconnects has its child killed at once, whether it computes or waits.

The response is never compressed and says `Cache-Control: no-store, no-transform` and
`X-Accel-Buffering: no`: a proxy in front must pass it through unbuffered, and it must pass the
visitor's address in `X-Forwarded-For` (forwarding headers are trusted from loopback only).

## Environment

| variable | meaning |
|---|---|
| `PORT` | the port to listen on (required) |
| `HOST` | the address to listen on; default `127.0.0.1` |
| `QM_POPUP_BIN` | the `qm-popup` executable (`cargo build --release` writes `target/release/qm-popup`) |
| `QM_PREPARED_DIR` | the prepared release: one directory per year beside the global tables |
| `QM_YEARS` | comma-separated years the release serves, e.g. `2026,2025`; the first is the default |
| `QM_TILES_DIR` | the heatmap tiles: `current.json` and the `{layer}.{build}.pmtiles` archives it names |
| `QM_POPUP_CONCURRENCY` | clicks computed at the same time; default `2` |
| `QM_NOINDEX` | `1` marks every response `X-Robots-Tag: noindex` (a host search engines must skip) |

The server refuses to start without the binary, a year directory or the tiles directory.

## Run and test

    npm ci --prefix frontend && npm --prefix frontend run build
    npm ci --prefix server
    PORT=... QM_POPUP_BIN=... QM_PREPARED_DIR=... QM_YEARS=... QM_TILES_DIR=... npm --prefix server start

    npm --prefix server run check     # typecheck, lint, tests (the popup route against a fake qm-popup)
    npm --prefix frontend run check   # typecheck, lint, unit tests
    npm --prefix frontend run e2e     # the built map in Chromium, every API answer mocked in the page

`scripts/check-fast.sh` runs both `check` scripts.
