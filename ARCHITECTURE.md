# Architecture

Quiet Map is a global noise atlas. A visitor clicks the map and reads how loud the place sounds:
Nden, the day's mean loudness in sone, over Lden, the levels of day, evening and night per layer,
how the level spreads over time, the loudest contributors and the flights. Physics: CNOSSOS-EU
(Directive 2015/996 as amended by 2021/1226) for ground sources, ECAC Doc 29 (4th ed.) for
aircraft. The answer must be exact within a stated error budget and fast from a cold disk; the
heatmap shows what the popup would answer at each pixel. Read this page first. Two About pages
tell the rest once, for visitors and developers alike: how the data are stored and how a click
and the heatmap are computed, in `frontend/src/about/how-it-works.md` (the site's "How it
works"); how each source is modelled, with its data and numbers, in
`frontend/src/about/methodology.md` and in its builder's file header. This page holds what a
developer needs beyond them: which code does what, the contracts and invariants between the
parts, the commands and the budgets.

## Data flow

    inputs    an earlier version's prepared tables and rasters (dev4, 2026-09-24): roads with
              counts and speeds, railways, buildings, industry, leisure, ships, aerodromes,
              terrain, land cover, weather; the extra datasets of fetch/; ADS-B archives; EGM2008
    build/    qm-build: converts the inputs and computes the emission (the click computes only
              that of the flight list's kept pieces)
      -> <release>/<year>/<x9>/<y9>/<x12>_<y12>.<kind>     (z12 tiles in z9 directories)
         <release>/<year>/aircraft-tracks/<xx>.aircraft-tracks, <release>/<year>/complete
         <release>/weather
    popup/    qm-popup: one click, the ring loop: read whole files, propagate, stream the answer
    paint/    qm-paint: the heatmap, every pixel's Lden per layer on the popup's physics
    raster/   qm-raster: one map tile of a data layer, drawn from the tiles as a PNG
    server/   HTTP: the map, the heatmap, the data layers, the streamed popup, geocoder, stays
    frontend/ the map and the popup (redraws on every streamed update)

The ground inputs are still the earlier version's tables, converted by `qm-build dev4`
(`build/src/dev4.rs`); builders that read the raw sources (the OpenStreetMap planet, traffic
counts, terrain models, ERA5) replace it one input at a time. `fetch/` downloads the extra
datasets (noise barriers, bus routes, national terrain models, tents, wind turbines, venues,
WorldClim, places of worship), each into the directory its caller names. `qm-build`'s other
steps: `weather`, `traffic` (the trips buildings make), `airport-traffic`, `geoid`, the aircraft
chain (`aircraft-segments`, `aircraft-shuffle`, `aircraft-boxes`, `aircraft-events`,
`aircraft-tracks`, checked by `aircraft-check`) and `complete`, the marker written last.

`physics/` holds the kernels (ray, line quadrature, bounds, Doc 29), the emission models the
builders run, ISO 9613-1 air absorption, ISO 532-1 loudness and Kurze's statistics, with the
standards' test cases; `tiles/` holds each kind's file format, writer and reader (the only code
that knows the bytes); `bench/` the benchmark points and runner; `scripts/` the gate
(`check-fast.sh`) and the generator of the Doc 29 class tables (`aircraft-classes.py`).

## Tiles

What each kind of file holds, with the world's sizes, is in How it works. Each kind's bytes are in
its module's header, the only code that knows them; `tiles::tile_path` names a tile's file in the
standard web-map XYZ numbering (y grows southwards). What the readers rely on beyond the page:

| kind | module | contract |
|---|---|---|
| `terrain` | `tiles/src/terrain.rs` | the one-arc-second node window that brackets the tile: neighbours share their seam nodes; a sample touching a node without data is refused, never guessed |
| `obstacles` | `tiles/src/obstacles.rs` | outlines stored whole, listed per z19 cell (128 x 128 a tile) through a cell -> offset table |
| `sources` | `tiles/src/sources.rs` | 12-byte pieces, 80-byte records, each distinct display text once; emission per octave band and period as u16 in 0.01 dB |
| `aircraft`, `aircraft-far` | `tiles/src/aircraft.rs` | the boxes, each with its two loudest flight pieces, and the tile's flight table |
| `aircraft-events` | `tiles/src/aircraft_events.rs` | 16 x 16 z16 cells a tile; no file where no flight reaches 50 dB |
| `aircraft-tracks` | `tiles/src/aircraft_tracks.rs` | 256 files by the address's low byte, each sorted by address and start |
| `weather` | `physics/src/weather.rs` | 0.5 deg nodes: favourable probability per period and 16 sectors in percent, absorption per octave in 0.01 dB/km |

- Coordinates are int16 relative to the tile centre, step = tile width / 32,768: the tile spans
  +-16,384 steps and half a tile of margin fits on every side.
- Ownership: every emission piece lives in exactly one tile: a line is split at tile edges and
  each part stored in the tile it lies in, a point in the tile holding it, so the sources of a
  ring never reach into a ring not yet read. An outline is stored whole in every tile whose cells
  it crosses and listed in each of those cells; a wall longer than the frame is clipped into
  parts within it, a ring larger than the frame is left out (the builder counts it). Footprint
  and source ids are global and stable across tiles; a popup meeting one outline in two files
  sees one footprint. Holes are kept.
- A release is complete when its builder wrote the completion marker last. Only then does a
  missing file mean "empty"; an unfinished build is never served.
- The physics tables (NPD, CNOSSOS coefficients, the Doc 29 classes) are compiled into the
  programs. A click reads the weather table's four nodes around it, the painter and the builders
  all of it. Ground rays take the place's absorption, and the NPD curves are moved to the place's
  air from the AIR-1845 atmosphere they come in (Doc 29 Appendix D, with the impedance
  adjustment), a box's at the centre of its z9 square; roads roll at the place's yearly air
  temperature (WorldClim 2.1, CNOSSOS 2.2.10), with the gradient and junction terms.
- Tiles are read whole with plain reads, one task per file, a ring's files asked for together
  (cold on NVMe, 26 MB in 3-5 ms against 23-29 ms with mmap and MADV_WILLNEED, whose faults read
  32 KB at a time). The flight tracks are the exception: a file's directory, then each listed
  flight's points, by positioned reads.

## Sources

The builders make every ground source's emission from its inputs: road traffic from counts where
counted; where not, a street carries the trips the buildings behind it make (`qm-build traffic`:
daily trip ends routed down the local streets to the nearest main road along a shortest-path
tree, so a street never carries more than the street it drains into), and a main road a model
fitted on counted roads where the region has them, else a fixed estimate for its class; the
vehicle mix, speeds and fleets by country; trains from timetables routed onto the tracks, guessed
ones scaled to national train-km; industry, leisure, car parks and wind turbines by area or
rating; homes by the heat pumps and air conditioners their households own; church bells and
calls to prayer as events; the people outside bars and restaurants; airport ground operations on
the aeroways; ships from AIS density.

## Aircraft

Stage 0/1 turns a year of ADS-B traces into flight segments per day (`build/src/aircraft`):
altitudes above EGM2008, each segment's Doc 29 phase and operation (A.3.2), and each flight's
class, decided once: its designator's ANP aircraft (55 classes, `scripts/aircraft-classes.py`),
else by its transponder category or slow flight, else a fallback; surface vehicles are ground
traffic. `aircraft-shuffle` sorts the window's segments into z9 squares once; `aircraft-boxes`
then builds a square at a time and gives each segment its power by the force balance with the
observed climb and acceleration (`physics/src/doc29/thrust.rs`; an arrival's flap and gear below
the landing configuration height, B11); a power past the NPD rows reads the edge row, as Doc 29
gives Eq. 4-3 between tabulated powers only. A box is a web-map cell x a clearance slab above the
highest terrain within one edge; the edge grows with clearance so that it spans D = 3 dB of the
steepest NPD slope there (first layer about 50 m).

A segment is cut at tile edges, then into the pieces of the boxes it crosses; a box sums its
pieces per period at the ten NPD distances (an average day of the window: primary flights over
the baseline days, flights only the secondary provider saw over the increment days) with the
geometry of one average piece, read as two at the gradient plus and minus the spread of its
pieces' gradients (climbs and descents extended to where they pass a receiver), counts its
flights and installation mix per period, and keeps its two loudest pieces for the flight list
(by LAmax at their height above the box's ground, one per flight). The click reads every box
through the click-time equation (`physics/src/doc29/boxes.rs`); for tiles beyond 3 km of the
click it reads the far boxes (D = 12 dB). The flight list is ranked by Lmax: each box states its
kept pieces' loudest LAmax at 1,000 ft, and boxes are searched loudest bound first until the
bound falls below the list's entry level, so the list is what every kept piece would give. A
segment's closest point is the perpendicular to its inclined path (Doc 29); one maximum-level
function (Eq. 4-8a with the receiver's impedance) serves the list, the events and the checker.

`aircraft-events` counts at every z16 cell each flight once, at its loudest moment there, with
the boxes' day weights; the popup reads the cells around its receiver bilinearly, and the
aircraft row says how many flights pass above 50 dB. A listed flight's line on the map is its
whole track (`aircraft-tracks`: every flight of the year flat, its segments' ends kept within
20 m by Douglas-Peucker, broken at the antimeridian and at any step over 300 km, in 256 files by
the address's low byte), clipped to 20 km around the click and to 100 points in the final
answer. `aircraft-check` holds the boxes against every segment, the list against the exact ten
loudest and the events against exact counts.

Airport ground operations (`build/src/airport`) put the window's ground legs on the aeroway lines
(OSM runways, taxiways, stopways and airstrips, and lines found where legs of ten flights on
three days run off every mapped line): a leg on a runway rolls at 40 kt or faster or as a take-off
roll, and taxis otherwise. A flight seen low over a runway but without a roll of its own gets its
class's typical take-off or landing roll there; where the primary provider saw it low, the roll
is the primary's alone, one per movement. The lines become sources with their sound power per
metre.

## Popup

How a click computes, ring by ring, piece by piece and ray by ray, and how the popup's levels map
onto those sums, is in How it works. The code, in the order of a click (`popup/src/`):

- `answer.rs`: the ring loop. The clicked tile and ring 1 (9 tiles, all kinds) and the aircraft
  events at the receiver, then ring 2, 3, ... until every kind's reach is covered
  (`candidates::GROUND_REACH_M`, `aircraft::boxes::AIRCRAFT_REACH_M`); a ring beyond a kind's
  reach skips that kind's files. An update after every ring.
- `candidates.rs`: every piece of the files read, with its bound (`physics/src/bound.rs`).
- `selection.rs`: the stop rule and the sample, one omitted-energy account per layer and period
  across all rings, the evaluations of all layers in one parallel pool.
- `evaluate.rs`: one source at the receiver, a point's ray or a line's quadrature nodes
  (`physics/src/line.rs`), each through `physics/src/ray.rs` and `cnossos/`; its energy per
  period and band, and its sound path, its rays' energy summed after each term.
- `building.rs`: a click inside a building, answered at its loudest façade.
- `aircraft/`: every box through the click-time equation, screened by the receiver's horizons;
  the flight list, the events at the receiver, the listed flights' tracks.
- `rows.rs` (the list, a row an object), `percentiles.rs` with `distribution.rs` (how the level
  spreads over time), `loudness.rs` (Nden), `shares.rs` (each row's share), `lines.rs` (the
  listed sources' lines for the map), `listing.rs` (the listed pieces and their rays, asked for
  by `bin/qm-popup.rs`'s options), `json.rs` (an update as a line of JSON).

Invariants:

- The bound stays an upper bound of `ray` and `line`: a source is skipped only by the
  omitted-energy account, never by a per-source threshold.
- The sample is seeded by the click, so a click always answers the same.
- Until every ring is read the answer says it is partial; each update is the whole answer so far,
  one line of JSON in one streamed HTTP response. A failed read is an error, never a quieter
  answer. Exact mode (`--exact 1`, benchmark only) is the same loop with the stop rule off.
- A building's source is never screened by a footprint containing it (its own, or another
  outline of the same building).

## Heatmap

How the map is painted is in How it works. `qm-paint` (crate `paint-gpu`) paints z12 squares one
after another at zoom 12 or 13 (512 or 1,024 pixels a side, `--squares` or `--bbox`), skipping
those painted; a square's files are read once, to the reach of its farthest pixel, and its exact
evaluations run on the first CUDA card (`paint-gpu/kernels/`, the popup's pair physics in f32,
each function citing its Rust) or on the cores. The method is the `paint` crate: `square.rs` (a
square's neighbourhood, its sources indexed by place and reach), `exact.rs` (the points and the
exact evaluation of (point, source) pairs), `lattice.rs` (the loud sources exact and the hum at a
lattice), `paint.rs` and `groups.rs` (the pixels), `flights.rs` and `etalon.rs`. Loud is one rule
for the painter, its etalon and the hum: `popup::candidates::loud`, a bound reaching
`physics::bound::REACH_EDGE_LDEN_DB` (20 dB).

The tiles (`hm3.rs`): per layer and the total, 512 x 512 cells of twice the Lden (255 none);
`qm-paint pack` (`pack.rs`) builds every zoom down to 2 (energy means) into one PMTiles archive
per layer and `current.json`, which the server publishes. `qm-paint-gpu check` holds the card
equal to the cores on random pairs, `pair` compares one pair, `etalon` paints the reference,
`compare` scores a map against it cell by cell and `popup` scores a map against popup clicks.

## Web

`server/` (its README has the routes, limits and environment) runs `qm-popup` once per click (two
at once by default, two waiting per slot, 503 when full, killed after 30 s or when the visitor
leaves) and forwards its lines as `application/x-ndjson`, flushed at once and never compressed; a
failed click ends with one `{"error"}` line. It serves the built map and the heatmap's archives,
proxies the geocoder (Photon) and the places to stay (Stay22, kept in memory no longer than
Stay22 allows), limits each client to 5 requests a second on the popup, geocoder and stays, and
runs `qm-raster` once per tile of a data layer the visitor switched on: the "Advanced" group,
elevation, hard ground, buildings, noise barriers, road traffic, trains and other sources as the
computation reads them, and the forest cover it leaves out. `frontend/` redraws the popup on every
line, marks a partial answer as still refining, shows an error line as an error, aborts the
previous request on a new click, and draws the places to stay as priced pins.

## Budgets

Measured by hand with `bench/cold.py` (the tile files evicted from the page cache, checked) and
`bench/report.py`; the gate does not run them.

| cold | NVMe | HDD RAID10 (older years; not yet measured) |
|---|---|---|
| first answer | <= 0.2 s | <= 0.5 s |
| full answer | <= 0.5 s | <= 1.5 s |

- Error of the fast answer against the full computation on unaggregated inputs (every source
  and flight segment on its own), per layer and for day, evening, night and Lden: p95 <= 0.1 dB,
  max <= 0.3 dB. `bench/report.py` holds the fast answer against exact mode on the same tiles,
  `qm-build aircraft-check` the boxes against every flight segment.
- Each streamed update <= 100 KB, held by caps on what it carries (2,000 points of the sources'
  lines, 100 points a flight's track); bytes and files read are reported per point.
- Code: about 60k lines in total, files about 300 lines.
