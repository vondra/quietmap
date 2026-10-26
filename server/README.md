# Server routes for the map, heatmap archives and streamed point answers

Fastify on Node 24 (TypeScript run as is, no build step). It serves the built map, the heatmap
tiles, the data layers, the geocoder proxies, the places to stay, the optional sign-in and the
streamed popup:

| route | answer |
|---|---|
| `GET /api/popup?lat=&lon=[&year=][&source=ID,...[&piece=K]]` | one click, streamed as `application/x-ndjson` (below) |
| `GET /api/tiles-manifest/7` | the published heatmap generation for the HM3 decoder: `{build, zoom, layers}` |
| `GET /api/tiles/:build/:layer/:z/:x/:y.bin` | one HM3 tile, Brotli, immutable; a missing tile is an empty 200 |
| `GET /api/raster/:layer/:z/:x/:y.png` | one 256-pixel map tile of a data layer, drawn by `qm-raster` from the default year: `elevation`, `forest`, `hard` (zoom 10-16), `buildings`, `barriers`, `traffic` (13-16), `trains` (11-16) and `others` (12-16); 400 for no such tile, 503 when eight draw and 256 wait, 500 when the drawing failed; kept an hour |
| `GET /api/search?q=&lat=&lon=`, `GET /api/reverse?lat=&lon=` | address suggestions and place names (public Photon geocoder) |
| `GET /api/stay?swlat=&swlng=&nelat=&nelng=&checkin=&checkout=[&adults=][&type=][&minstars=][&minscore=]` | the places to stay with a room in a view (a box within ±180°) for a stay (calendar dates `YYYY-MM-DD`, check-in from today in UTC), hotels or the rest (`type=hotel`, `rental`, both when absent), from Stay22: `{listings, nights, currency, expiresIn, failure}`, each listing at its cheapest supplier's price for the whole stay, all of them gone after `expiresIn` seconds (Stay22's 55 minutes from when they were asked; kept in memory till then, never after, and `no-store`), `failure` saying what part of the search failed when the rest is answered (such an answer is not kept); the view (its middle 16° when wider) is answered for its box moved out to a grid of 1, 2 or 5 times a power of ten, at most a quarter of the view's longer side; 400 for a wrong query or one Stay22 refuses (in its words), 502 when Stay22 fails, 503 without a Stay22 account and, with `Retry-After`, when Stay22 is asked too often: the key's 150 calls a minute are spent, the visitor's 50 of them are (an IPv4 address or an IPv6 /64), four searches run, or Stay22 answered 429 (believed up to a minute); limited per client like the popup |
| `POST /api/auth/email` `{email}` | mails a sign-in link to `{origin}/signin#{token}`: 204 whether or not the address has an account, and without a mail once the address had its three links of the hour; 400 for no plain-ASCII address, 502 when the mail could not be sent (that link neither works nor counts), 503 once the server sent its 50 sign-in mails of the day; ten a client an hour |
| `POST /api/auth/link` `{token}` | the address a link signs in, `{email}`, while it works (15 minutes, once): uses nothing; else 404; thirty a client an hour, with the next |
| `POST /api/auth/link/use` `{token}` | signs in with the link, once: the `__Host-qm_session` cookie (HttpOnly, Secure, SameSite=Lax, 180 days, renewed at most daily while used); else 404 |
| `GET /api/me` | the signed-in visitor's `{email, titles, recent}`: the names given places (the later of two for one place within 30 m) and the five recent places; 401 when signed out |
| `PUT /api/places/title` `{lat, lng, title, updatedAt}` | names a place (`title` null removes its name) under the 30 m rule, a name given later kept: `{titles}` |
| `PUT /api/places/recent` `{recent}` | replaces the five recent places, one a place: 204 |
| `POST /api/places/import` `{titles, recent}` | adds a browser's places at the visitor's word: names merged, recent places only where the account has none: `{titles, recent}` |
| `POST /api/auth/signout`, `DELETE /api/me` | signs out; deletes the account, its recordings and photos, its places, its sessions and its links not yet used |
| `PUT /api/media/:id` | keeps a recording (`audio/wav`: 16-bit PCM, one channel, 8-96 kHz, at most 28.8 M samples, ten minutes at 48 kHz) or a photo (`image/jpeg`, at most 16 MiB, kept without its EXIF, XMP, IPTC and comments) at a place, under `:id`, 32 hex digits the page chose; `X-Quietmap-Media` is URI-encoded JSON `{lat, lng, takenAt, details}` (`details` an object of at most 4,096 characters): the item `{id, kind, lat, lng, takenAt, bytes, durationS}`; the same bytes again answer the item kept, other bytes under its id 409; 400 for a wrong body or header, 413 too large, 507 when the account keeps 200 items or 2 GiB or the disk has less than 100 GiB free; checked before the body is read; sixty a client an hour |
| `GET /api/media?lat=&lng=` | the account's items within 30 m of a point, the latest first: `{items}` |
| `GET /api/media/:id`, `DELETE /api/media/:id` | the item's file to its account alone (ranges, uncompressed, `nosniff`); deletes it with its file: 204; else 404 |
| `GET /signin` | the map's page a link opens (`../frontend/src/components/SignInPage.tsx`), never in another site's frame |
| everything else | the built frontend (`../frontend/dist`), or a 404 |

A request that changes anything must come from the map's own origin (`QM_PUBLIC_ORIGIN`): its
`Origin` header is checked. One that changes an account names it in `X-Quietmap-Account`, and is
refused with 409 when this browser has signed in as another since. A longitude from a repeated
world copy of the map is wrapped. An address that asked for a link is forgotten an hour later and a
session when it ends (checked every ten minutes). Every account answer is `no-store`, and every
sign-in route answers 503 without the four `QM_ACCOUNTS_DB`, `QM_PUBLIC_ORIGIN`, `QM_MAIL_KEY_FILE`
and `QM_MAIL_FROM`. The recordings and photos lie beside the database, in
`<QM_ACCOUNTS_DB>-media/<account id>/`.

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
  -180..180), `year` one of `QM_YEARS` (the first when absent), `source` one to 32 ids of 16
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
| `QM_ACCOUNTS_DB` | the optional sign-in's SQLite file: accounts, sessions, links and places; only tokens' SHA-256 are kept |
| `QM_PUBLIC_ORIGIN` | the map's own origin, e.g. `https://quietmap.org`: the links point there and only its pages change an account |
| `QM_MAIL_KEY_FILE`, `QM_MAIL_FROM` | the file holding the mail API's key (Resend) and the sender of the sign-in mails, e.g. `Quiet Map <sign-in@quietmap.org>` |

The server refuses to start without either binary, a year directory or the tiles directory, with
only one of the two Stay22 values, or with only some of the sign-in's four.

How to build, run and test it is in the repository's `README.md`; its tests (`npm --prefix server
run check`) run the popup route against a fake `qm-popup`, with Stay22 mocked.
