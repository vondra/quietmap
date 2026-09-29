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
| `aircraft` | aircraft boxes (energy per period at the NPD distances) and the tile's flight table |

- Coordinates are int16 relative to the tile centre, step = tile width / 32,768: the tile spans
  +-16,384 steps and half a tile of margin fits on every side. Longer geometry is split.
- Ownership: every emission piece lives in exactly one tile, the one holding its midpoint.
  An outline is listed in every 50 m cell it crosses, the traversal running on into cells of
  neighbouring tiles within the margin, so every tile's file lists every outline that crosses
  its cells. Footprint and source ids are global and stable across tiles; a popup meeting one
  outline in two files sees one footprint. Holes are kept; edges created by a split are marked
  and never screen.
- A release is complete when its builder wrote the completion marker last. Only then does a
  missing file mean "empty"; an unfinished build is never served.
- Global tables ship with the program: physics tables (NPD, CNOSSOS coefficients) and one
  weather table (favourable probability per period and 16 sectors, 0.5 deg). Air absorption is
  ISO 9613-1 at 15 C and 70 %; road emission is frozen at the CNOSSOS reference temperature.
- Reading is always whole files (mmap with MADV_WILLNEED), all files of a ring requested at once.

## Popup

Read the clicked tile and ring 1 (9 tiles, all kinds), compute, send the first answer; then
ring 2, 3, ... until every kind's reach is covered (ground sources ~12 km, aircraft ~16 km).
A ring beyond a kind's reach skips that kind's files. One HTTP response streams the updates as
lines of JSON (each <= 100 KB).

1. Upper bound per source: free field plus the mixed ground and diffraction gain (6 dB
   homogeneous, 18 dB favourable, mixed at the global p_max) plus the 3 dB receiver
   reflection; aircraft boxes use their own bound. Sort per layer.
2. Full physics from the loudest. One omitted-energy account per layer and period runs across
   all rings: a source is skipped only while the bounds of everything skipped so far stay below
   (10^(0.1/10) - 1) x the energy evaluated. Never a per-source threshold.
3. Lines through the point-sum quadrature; aircraft boxes by the click-time equation.
4. One parallel pool over all layers and rays; totals first, then display details (what-if
   variants, traces) only for the contributors shown.
5. A click inside a building answers for its loudest facade receiver (no indoor attenuation);
   rings accumulate per facade receiver before the loudest is chosen. A building never screens
   its own emission.
6. Until every ring is read the answer says it is partial. A failed read is an error, never a
   quieter answer. Exact mode (benchmark only) is the same loop with the stop rule off.

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
