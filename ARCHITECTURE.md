# Architecture

Quiet Map is a global noise atlas. A visitor clicks the map and reads Lden (and day, evening,
night) per layer with the loudest contributors. Physics: CNOSSOS-EU (Directive 2015/996 as
amended by 2021/1226) for ground sources, ECAC Doc 29 (4th ed.) for aircraft. The answer must
be exact within a stated error budget and fast from a cold disk. Read this page first.

## Data flow

    fetch/   one download script per dataset (URL and licence inside)
      -> sources/<dataset>-<yyyy-mm-dd>/
    build/   one builder per kind: emission is computed here, never at the click
      -> prepared/<year>/<x9>/<y9>/<x12>_<y12>.<kind>      (z12 tiles in z9 directories)
    popup/   the ring loop: read whole files, propagate, stream the answer
    server/  HTTP: the map, the tiles, the streamed popup
    frontend/ the map and the popup (redraws on every streamed update)

`physics/` holds the kernels (ray, line quadrature, bounds, Doc 29) with the standards' test
cases; `tiles/` holds each kind's file format, writer and reader (the only code that knows the
bytes); `bench/` holds the benchmark points and the runner.

## Tiles

Standard web-map XYZ numbering (y grows southwards). A z12 tile is 6.3 km wide in Prague and
9.8 km at the equator; a z9 directory holds 64 tiles.

| kind | content |
|---|---|
| `terrain` | raster: terrain height and ground type; neighbours share their seam samples |
| `obstacles` | buildings and walls: outlines and heights, listed per 50 m cell with a cell -> offset table |
| `sources` | every ground source as points or polylines, emission per octave band (63 Hz-8 kHz) and period as u16 in 0.01 dB; a few display fields |
| `aircraft` | aircraft boxes (energy per period at the NPD distances), each with its two loudest flight pieces, and the tile's flight table |
| `aircraft-far` | the same with boxes four times larger, read for tiles beyond 3 km of the click |

- Coordinates are int16 relative to the tile centre, step = tile width / 32,768: the tile spans
  +-16,384 steps and half a tile of margin fits on every side. Longer geometry is split.
- Ownership: every emission piece lives in exactly one tile: a line is split at tile edges and
  each part stored in the tile it lies in, a point in the tile holding it, so the sources of a
  ring never reach into a ring not yet read. An outline is listed in every 50 m cell it crosses, the traversal running on into cells of
  neighbouring tiles within the margin, so every tile's file lists every outline that crosses
  its cells. Footprint and source ids are global and stable across tiles; a popup meeting one
  outline in two files sees one footprint. Holes are kept; edges created by a split are marked
  and never screen.
- A release is complete when its builder wrote the completion marker last. Only then does a
  missing file mean "empty"; an unfinished build is never served.
- Global tables ship with the program: physics tables (NPD, CNOSSOS coefficients) and one
  weather table (0.5 deg, ERA5 1991-2020: favourable probability per period and 16 sectors, and
  the yearly mean air absorption per octave, ISO 9613-1). Ground rays take the place's absorption,
  and the NPD curves are moved to the place's air from the AIR-1845 atmosphere they come in (Doc
  29 Appendix D, with the impedance adjustment); roads roll at the place's yearly air temperature
  (WorldClim 2.1, CNOSSOS 2.2.10), with the gradient and junction terms.
- Traffic is local where data says so: the road converter keeps counted flows and splits, gives
  every guessed split the counted medians of its class (16 countries), the cars the country's
  battery-electric share (IEA, rolling noise only) and the heavy vehicles their country's limit;
  guessed rail counts follow the country's Eurostat train-km. Thailand's national highways take
  the highway department's vehicle-km per province over their rows. Where no one counted, the
  buildings make the traffic (`qm-build traffic`): every building (OSM's, and the Overture
  footprints OSM lacks as buildings of unknown use) makes daily trip ends that join its nearest
  road and travel down the local streets to the nearest main road along a shortest-path tree, so
  a street carries what the buildings behind it make and never more than the street it drains
  into; secondary, tertiary and unclassified roads take the counted roads' relation to the trip
  ends generated within 2-5 km (13 European countries, capped where denser cities drive less;
  Thailand's rural road network its own counts), major roads in 20 countries a world fit on the
  trip ends within 1, 5 and 15 km.
- Homes emit the outdoor units their country's households own (heat pumps and air conditioners
  per household) running the hours their climate asks for (WorldClim degree days); other
  buildings follow the area law of their class. Church bells are events: Europe's Christian
  churches and bell towers (OpenStreetMap) ring the prayer bells three times a day and a Sunday
  peal, German and Swiss clocks strike the quarters; the popup's time levels count an event only
  in the share of the period it sounds.
- Reading is always whole files with plain reads, all files of a ring at once, one reader per
  file (cold on NVMe 5-8x faster than mmap with MADV_WILLNEED, whose faults read 32 KB at a
  time).

## Aircraft

Stage 0/1 turns a year of ADS-B traces into flight segments per day (`build/src/aircraft`):
altitudes above EGM2008 (geometric altitude is ellipsoidal: the flight's own offset, else a
regional one, else pressure), Doc 29 phases and powers (the force balance with the observed
climb and acceleration; below the landing configuration height an arrival's flap and gear, Doc 29
B11). Every designator flies its own ANP aircraft (a noise class per ANP aircraft, a proxy
where the ANP has none; `scripts/aircraft-classes.py`): its Eq. B-1 or propeller (B-5) ratings,
weight, drag and NPD rows, at 160 kt; a power past the rows reads the edge row, as Doc 29 gives
Eq. 4-3 between tabulated powers only. `aircraft-shuffle` sorts the window's segments into z9
squares once; `aircraft-boxes` then builds a square at a time. A box is a web-map cell x a
clearance slab above the highest terrain within one edge; the edge grows with clearance so
that it spans D = 3 dB of the sizing slope there, the steepest of dev4's classes' rows (first
layer about 50 m).
A segment is cut at tile edges, then into the pieces of the boxes it crosses; a box sums its
pieces per period at the ten NPD distances (an average day of the window: primary flights over
the baseline days, flights only the secondary provider saw over the increment days) with the
geometry of one average piece, read as two at the gradient plus and minus the spread of its
pieces' gradients (climbs and descents extended to where they pass a receiver), and keeps its two
loudest pieces for the flight list (by LAmax at their height above the box's ground, one per
flight). The click reads every box through the click-time equation
(`physics/src/doc29/boxes.rs`); for tiles beyond 3 km of the click it reads the far boxes (D = 12
dB). The flight list is ranked by Lmax: each box states its kept pieces' loudest LAmax at 1,000
ft, and boxes are searched loudest bound first until the bound falls below the list's entry
level, so the list is what every kept piece would give.

Airport ground operations (`build/src/airport`) put the window's ground legs on the aeroway lines
(OSM runways and taxiways, and lines found where legs of ten flights on three days run off every
mapped line): a leg on a runway rolls at 40 kt or faster or as a take-off roll, and taxis
otherwise. A flight seen low over a runway but without a roll of its own (no receiver saw it on
the ground) gets its class's typical take-off or landing roll there. The lines become sources
with their sound power per metre.

## Popup

Read the clicked tile and ring 1 (9 tiles, all kinds), compute, send the first answer; then
ring 2, 3, ... until every kind's reach is covered (ground sources ~12 km, aircraft ~16 km).
A ring beyond a kind's reach skips that kind's files. One HTTP response streams the updates as
lines of JSON (each <= 100 KB).

1. Upper bound per source: free field plus the ground and diffraction gain (18 dB in either
   state: an elevated source gains more than the 6 dB homogeneous corner of a ground source)
   plus the receiver reflection; aircraft boxes use their own bound. Sort per layer.
2. Full physics from the loudest. One omitted-energy account per layer and period runs across
   all rings: a source is skipped only while the bounds of everything skipped so far stay below
   (10^(0.1/10) - 1) x the energy evaluated. Never a per-source threshold.
3. Lines through the point-sum quadrature; aircraft boxes by the click-time equation.
4. One parallel pool over all layers and rays; totals first, then display details (what-if
   variants, traces) only for the contributors shown.
5. A click inside a building answers at its loudest CNOSSOS-EU 2.8 facade receiver, without
   indoor attenuation: after the first read, the eleven sources with the greatest bound at any
   facade are evaluated at every facade, the highest Lden wins, and the ring loop answers there.
   A building's source is never screened by a footprint containing it (its own, or another
   outline of the same building).
6. Until every ring is read the answer says it is partial. A failed read is an error, never a
   quieter answer. Exact mode (benchmark only) is the same loop with the stop rule off.
7. The final update also carries the levels exceeded 5, 10, 50 and 90 % of the time (each
   contributor a line of Kurze's Poisson statistics at its own lambda, the sum drawn with
   stratified draws seeded by the click) and the loudness N5 per period: Zwicker's loudness
   (ISO 532-1, sone) of the received third-octave spectrum (each ground layer's octave bands as
   its evaluated pieces arrive, the flights' as the loudest flight's Doc 29 spectral classes
   through the place's air) set to the level exceeded 5 % of the time.

## Web

`server/` runs `qm-popup` once per click (a few clicks at once, a short queue, 503 when it is
full) and forwards its lines as `application/x-ndjson`, flushed at once and never compressed; a
visitor who leaves has the child killed, and a failed click ends with one `{"error"}` line. It
also serves the heatmap tiles and the built map. `frontend/` redraws the popup on every line,
marks a partial answer as still refining, shows an error line as an error, and aborts the
previous request on a new click. `server/README.md` has the routes and the environment.

## Budgets (checked by `bench/` after every change)

| cold | NVMe | HDD RAID10 |
|---|---|---|
| first answer | <= 0.2 s | <= 0.5 s |
| full answer | <= 0.5 s | <= 1.5 s |

- Error of the fast answer against the full computation on unaggregated inputs (every source
  and flight segment on its own), per layer and for day, evening, night and Lden: p95 <= 0.1 dB,
  max <= 0.3 dB.
- Each streamed update <= 100 KB. Bytes and files read are reported per point.
- Code: about 60k lines in total, files about 300 lines.
