# Server

Fastify on Node 24 (TypeScript run as is, no build step). It serves the built map, the heatmap
tiles, the data layers, the geocoder proxies, the places to stay and the streamed popup:

| route | answer |
|---|---|
| `GET /api/popup?lat=&lon=[&year=][&source=ID,...[&piece=K]]` | one click, streamed as `application/x-ndjson` (below) |
| `GET /api/tiles-manifest` | the published heatmap generation: `{build, zoom, layers}` |
| `GET /api/tiles/:build/:layer/:z/:x/:y.bin` | one HM3 tile, Brotli, immutable; a missing tile is an empty 200 |
| `GET /api/raster/:layer/:z/:x/:y.png` | one 256-pixel map tile of a data layer, drawn by `qm-raster` from the default year: `elevation`, `forest`, `hard` (zoom 10-16), `buildings`, `barriers`, `traffic` (13-16), `trains` (11-16) and `others` (12-16); 400 for no such tile, 503 when eight draw and 256 wait, 500 when the drawing failed; kept an hour |
| `GET /api/search?q=&lat=&lon=`, `GET /api/reverse?lat=&lon=` | address suggestions and place names (public Photon geocoder) |
| `GET /api/stay?swlat=&swlng=&nelat=&nelng=&checkin=&checkout=[&adults=][&type=][&minstars=][&minscore=]` | the places to stay with a room in a view (a box within ±180°) for a stay (calendar dates `YYYY-MM-DD`, check-in from today in UTC), hotels or the rest (`type=hotel`, `rental`, both when absent), from Stay22: `{listings, nights, currency, expiresIn, failure}`, each listing at its cheapest supplier's price for the whole stay, all of them gone after `expiresIn` seconds (Stay22's 55 minutes from when they were asked; kept in memory till then, never after, and `no-store`), `failure` saying what part of the search failed when the rest is answered (such an answer is not kept); the view (its middle 16° when wider) is answered for its box moved out to a grid of 1, 2 or 5 times a power of ten, at most a quarter of the view's longer side; 400 for a wrong query or one Stay22 refuses (in its words), 502 when Stay22 fails, 503 without a Stay22 account and, with `Retry-After`, when Stay22 is asked too often: the key's 150 calls a minute are spent, the visitor's 50 of them are (an IPv4 address or an IPv6 /64), four searches run, or Stay22 answered 429 (believed up to a minute); limited per client like the popup |
| everything else | the built frontend (`../frontend/dist`), or a 404 |

## The popup stream

`/api/popup` runs `qm-popup --prepared DIR --year YEAR --lat LAT --lon LON` for the click and
forwards every line it prints as one line of the response, flushed at once: the first after the
clicked tile and its neighbours are read, later ones as rings are added. Each line is the whole
answer so far (`partial` is true until the last one), never a delta; the fields are those of
`qm-popup` (`frontend/src/types/noise.ts`). `source=ID,...` (an opened row's sound path, its parts'
group ids) adds how all of it arrives and its loudest pieces with their data and rays
(`--source`); with `piece=K` only its Kth loudest piece, each ray with the ground and walls under it
(`--piece`); the benchmark's `--exact` is never passed.

- `400` with `{"error"}`: `lat` must be a number within ±85.05, `lon` a number (wrapped to
  -180..180), `year` one of `QM_YEARS` (the first when absent), `source` one to eight ids of 16
  lowercase hex digits when given, `piece` 0 to 23 with a source.
- `503` with `{"error"}` and `Retry-After`: every slot computes and the queue is full. A few clicks
  compute at once (each uses every core); two per slot may wait.
- `429`: more than 5 requests per second from one client (an IPv4 address or an IPv6 /64), as for
  the geocoder and the places to stay. Tiles are never limited, local unproxied callers never are.
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
| `QM_RASTER_BIN` | the `qm-raster` executable (`target/release/qm-raster`), which draws the data layers |
| `QM_PREPARED_DIR` | the prepared release: one directory per year beside the global tables |
| `QM_YEARS` | comma-separated years the release serves, e.g. `2026,2025`; the first is the default |
| `QM_TILES_DIR` | the heatmap tiles: `current.json` and the `{layer}.{build}.pmtiles` archives it names |
| `QM_POPUP_CONCURRENCY` | clicks computed at the same time; default `2` |
| `QM_NOINDEX` | `1` marks every response `X-Robots-Tag: noindex` (a host search engines must skip) |
| `QM_PHOTON_URL` | the Photon geocoder's base URL for search and place names; default the public `https://photon.komoot.io` |
| `STAY22_AID`, `STAY22_API_KEY` | the Stay22 affiliate id and API key the places to stay are searched with; without them `/api/stay` answers 503 (Stay22 answers no search without a key) |

The server refuses to start without either binary, a year directory or the tiles directory, or with
only one of the two Stay22 values.

## Run and test

    npm ci --prefix frontend && npm --prefix frontend run build
    npm ci --prefix server
    PORT=... QM_POPUP_BIN=... QM_RASTER_BIN=... QM_PREPARED_DIR=... QM_YEARS=... QM_TILES_DIR=... npm --prefix server start

    npm --prefix server run check     # typecheck, lint, tests (the popup route against a fake qm-popup, Stay22 mocked)
    npm --prefix frontend run check   # typecheck, lint, unit tests
    npm --prefix frontend run e2e     # the built map in Chromium, every API answer mocked in the page

`scripts/check-fast.sh` runs both `check` scripts.
