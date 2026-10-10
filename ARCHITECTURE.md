# Architecture

Quiet Map is a global noise atlas. A visitor clicks the map and reads how loud the place sounds:
Nden, the day's mean loudness in sone, over Lden, the levels of day, evening and night per layer,
how the level spreads over time, the loudest contributors and the flights. Physics: CNOSSOS-EU
(Directive 2015/996 as amended by 2021/1226) for ground sources, ECAC Doc 29 (4th ed.) for
aircraft. The answer must be exact within a stated error budget and fast from a cold disk; the
heatmap shows what the popup would answer at each pixel. Read this page first. How each source is
modelled, with its data and numbers, is told once, in `frontend/src/about/methodology.md`, and in
its builder's file header.

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

Standard web-map XYZ numbering (y grows southwards). A z12 tile is 6.3 km wide in Prague and
9.8 km at the equator; a z9 directory holds 64 tiles.

| kind | content |
|---|---|
| `terrain` | raster: terrain height and ground type; neighbours share their seam samples |
| `obstacles` | buildings and walls: outlines and heights, listed per z19 cell (128 x 128 a tile, 49 m in Prague) with a cell -> offset table |
| `sources` | every ground source as a point or a straight piece in one of six layers (road, railway, industry, building, ship, aircraft ground operations), emission per octave band (63 Hz-8 kHz) and period as u16 in 0.01 dB; a few display fields |
| `aircraft` | aircraft boxes (energy, flights and installation mix per period at the NPD distances), each with its two loudest flight pieces, and the tile's flight table |
| `aircraft-far` | the same with boxes four times larger, read for tiles beyond 3 km of the click |
| `aircraft-events` | per z16 cell (16 x 16 a tile) the flights of an average day whose loudest moment reaches 50, 60 and 70 dB, those at night, their mean height and commonest type, and the helicopters; no file where none reaches 50 dB |

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
  programs. The weather table is a file beside the year roots (0.5 deg, ERA5 1991-2020:
  favourable probability per period and 16 sectors, and the yearly mean air absorption per
  octave, ISO 9613-1): a click reads its own four nodes, the painter and the builders all of it.
  Ground rays take the place's absorption, and the NPD curves are moved to the place's air from
  the AIR-1845 atmosphere they come in (Doc 29 Appendix D, with the impedance adjustment), a
  box's at the centre of its z9 square; roads roll at the place's yearly air temperature
  (WorldClim 2.1, CNOSSOS 2.2.10), with the gradient and junction terms.
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

Read the clicked tile and ring 1 (9 tiles, all kinds) and the aircraft events at the receiver,
compute, send the first answer; then ring 2, 3, ... until every kind's reach is covered (ground
sources 12 km, aircraft 16 km). A ring beyond a kind's reach skips that kind's files. One HTTP
response streams the updates as lines of JSON. The receiver stands 4 m up and gains 0, 1.5 or
3 dB from the reflecting facades around it.

1. Upper bound per ground source: free field plus the ground and diffraction gain (18 dB in
   either state: an elevated source gains more than the 6 dB homogeneous corner of a ground
   source) plus the receiver reflection. Sort per layer. Every aircraft box is evaluated (well
   under a microsecond each); its LAmax bound only orders the flight list's search.
2. Full physics from the loudest. One omitted-energy account per layer and period runs across
   all rings: a source is skipped only while the bounds of everything skipped so far stay below
   (10^(0.1/10) - 1) x the energy evaluated. Never a per-source threshold. A layer that would
   need more than 1,024 evaluations samples the rest in proportion to their bounds
   (Hansen-Hurwitz, 512 to 32,768 draws, until two standard errors are within 0.05 dB, seeded by
   the click, so a click always answers the same): the usual path in cities.
3. Lines through the point-sum quadrature; aircraft boxes by the click-time equation.
4. One parallel pool over all layers' pieces; totals first. Each contributor also sums its
   rays' energy after each term (distance alone, air, screening, then ground, in either
   meteorological state): its sound path, whose terms add up to its level. An opened row's
   segments come from a second run of the same click with the row's parts (`--source`): every
   piece of them evaluated, how all of it arrives in calm air, and the 24 loudest with each ray
   and the terms it was summed with.
5. A click inside a building answers at its loudest CNOSSOS-EU 2.8 facade receiver, without
   indoor attenuation: after the first read, the eleven sources with the greatest bound at any
   facade are evaluated at every facade, the highest Lden wins, and the ring loop answers there;
   a building without an exposed facade is not assessed. A building's source is never screened
   by a footprint containing it (its own, or another outline of the same building).
6. Until every ring is read the answer says it is partial. A failed read is an error, never a
   quieter answer. Exact mode (benchmark only) is the same loop with the stop rule off.
7. The final update also carries how the level spreads over each period: roads, railways and
   aeroways are lines of Kurze's Poisson statistics at their own lambda, all flights one line at
   their energy-weighted lambda, events (bells, calls) on for the share of the period they
   sound, the rest steady; for every hour and weather state the lines are added on a 0.1 dB
   grid, then the steady energy (computed, not drawn). From it come the levels exceeded 5, 10,
   50 and 90 % of the time and Nden, the headline: the mean over the day of Zwicker's loudness
   (ISO 532-1, sone) of every moment, the received third-octave spectrum (each ground layer's
   octave bands as its evaluated pieces arrive, the flights' as the class the loudest listed
   flight flew, through the place's air; a helicopter, which Doc 29 gives no spectral class,
   takes the fallback's) set to the moment's level, the evening 5 dB and the night 10 dB up, the
   periods by their hours. The list (30 rows, the rest in one row, the aircraft in one) ranks
   every heard source by its own Nden, alone; the rest row's Nden takes its lines over 1 % of it
   at their period's mean flow. Each row also carries its share of Nden (`shares.rs`): every
   moment's loudness shared among the sources by their A-weighted energy at that moment, the
   flows at their period's mean, so the rows add up to the whole; a moving line's part comes
   from the moments before it (the distributions, forward) and the loudness per energy the lines
   after it leave (backward). Partial updates rank by Lden. The final update also carries the
   listed flights' tracks.

## Heatmap

`qm-paint` (crate `paint-gpu`) paints z12 squares one after another at zoom 12 or 13 (512 or 1,024
pixels a side), skipping those painted; a square's ground, obstacles, sources and boxes are read
once, to the reach of its farthest pixel, and the exact evaluations run on a CUDA card (the popup's
pair physics in f32) or on the cores. A ground source is loud at a point where its bound there
reaches 20 dB (`physics::bound::REACH_EDGE_LDEN_DB`); the quiet rest, never dropped, is the hum,
estimated by the popup's own selection at a coarse lattice's points (every 128 pixels at z13) and
blended. Every loud source is evaluated exactly at a far lattice's points (every 64 pixels, a far
cell, and one cell beyond the square) and, within two far cells, at the corners of 16-pixel blocks.
A pixel evaluates exactly the loud sources crossing its block and the blocks around; every other
loud source is blended, its near share (whole within 0.75 far cells of the block's centre, none
beyond 2) from the block's corners and its far share from its far cell's points, in groups by layer,
10-degree direction and doubling distance around the far cell's centre. A group holding 0.3 % of its
layer gets one exact ray from its loudest member to the pixel, which scales the group: the shadow
there. The blocks of a far cell group by the same directions and distances, and a pixel within a
quarter block of a far cell's edge, the square's too (the far lattice reaches one cell beyond the
square), blends the two cells' groupings: no grouping switches at an edge, only a block's exact
sources and its sources' near shares change from block to block. Ground sources take the pixel's
receiver reflection, the flights (blended from the block corners) do not; a pixel inside an enclosed
building has no level. The tiles: per layer and the total, 512 x 512 cells of twice the Lden (255
none); `qm-paint pack` builds every zoom down to 2 (energy means) into one PMTiles archive per layer
and `current.json`, which the server publishes. `qm-paint-gpu etalon` paints the reference (every
loud source exact at every pixel, the same hum), `compare` scores a map against it cell by cell and
`popup` scores a map against popup clicks.

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
