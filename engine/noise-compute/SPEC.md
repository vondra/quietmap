# Popup propagation contract

The Rust engine is the acoustic source of truth. This document specifies the
propagation contract of road, rail and point sources (CNOSSOS-EU 2015/996 as amended by
2021/1226, checked against ISO/TR 17534-4) and the prepared-source rules around it. The
implementation and regressions live in `src/propagation/cnossos/`, `ray_path.rs`,
`ray_transfer.rs`, `line_quadrature.rs`, `relevance_bound.rs`, `meteorology.rs` and
`air_absorption.rs`; the CUDA painter mirrors them in `relevant_source_cnossos_*.cuh`.
Airport ground operations keep their single-edge path (`path_effects.rs`, `diffraction.rs`)
until their own campaign moves them.

## Receiver and prepared-source selection

Inside an enclosed building, retain the clicked footprint's envelope class and
clicked coordinates for presentation. The existing cardinal search, one-metre
steps up to 100 metres, selects the facade receiver before any source gate.
Reload obstacle indexes at that receiver, and use its position and elevation
for source selection and propagation. Project the facade result to the indoor
estimate only after computation. If that search finds no exterior point, retain
the clicked position as before.

Prepared airborne sub-segments are stored once, as rows of the z9 square that
owns the midpoint of their stored geometry (`airborne_segments_z9_v2`); the
popup loads owner squares within `AIRBORNE_QUERY_RADIUS_M` (16 km reach + half
the length cap), prunes batches by their full-geometry envelope and
accumulates a flight across squares by `flight_id`. The shuffle splits a chord
whose stored (z30-quantized, Mercator-clamped) geometry is longer than
`AIRBORNE_SUB_SEGMENT_MAX_LENGTH_M` (4 km) into the fewest equal pieces that
fit the cap — linear in latitude, longitude, altitude and the two endpoint
terrain samples, same period and date, flagged `SPLIT_PIECE` with
`CHORD_START`/`CHORD_END` at the ends — so the reader pad never depends on the
longest chord in a file, at the poles included. The popup chains the pieces it
evaluated back into one chord through their shared endpoints: the chord takes
the kernel's free-field and the received 20 dB floors on its summed SEL (an
unsplit sub-segment takes them inside the kernel), holds one Noise Segments
slot ranked by its summed energy, and is drawn as one polyline of its pieces.
Pieces whose geometry lies beyond the 16 km envelope or the class reach are
dropped like any sub-segment there: each such piece is on its own below the
40 dB reach threshold, and the energy lost is the dropped pieces' own energy,
those under the kernel's 20 dB floor bounded by that floor (pinned by
`pieces_beyond_reach_are_dropped_and_the_loss_is_their_own_level`: an 18 km
chord 14–32 km east of the receiver, 33.6 dB whole, keeps its 30.9 dB first
piece). Splitting
otherwise changes only the Doc 29 finite-segment terms: per layer and period
the energy of an unsplit chord is unchanged, a split chord within reach moves
Lden by at most 0.01 dB, keeps the same top flights and draws the same total
length. Equal original observations retain their multiplicity. Cruise aggregates canonical z15 cells once
and publishes them only in the owner z9 (`cruise_owner_z9_v2`). Each bucket key
retains the nearest of eight axial headings (22.5-degree spacing, modulo
180 degrees); opposite travel directions share an axis, crossing axes do not.
Non-null UInt8 `heading_bin` is required and must be in 0..7. The shared
`cruise_geometry` constructs a cell-local line with that bearing and the local
z15 cell diagonal length, in the same projection as Doc 29. Its weight is
`sum_length_m / geometry_length_m`, including fractions below one. Both the
popup and cruise field use `cruise_segment`; no source-chord representative
length, weighted length accumulator or 50 km clamp remains. Original carried
length, distinct flight identities and the sampling-day divisor are unchanged.
Owner queries include 16 km reach plus half the maximum cell diagonal; batch
bounds contain the complete synthetic line, including wrapped longitude.

This replaces directionless long diagonal smoothing, not the Doc 29 emission
model. Thirty 200 km trajectory checks across ten headings (including 135 degrees
and bin boundaries) and heights 7.5, 11 and 15 km require aggregate error below
0.1 dB against the same unsimplified kernel. This is a bounded regression, not a
global accuracy guarantee. The cruise field retains its separate 0.5 dB
interpolation limit against exact prepared-row computation.

Ground sources retain their spatial owners. Surface owner selection enumerates
the existing midpoint-gate envelope, including wrapped longitude and high
latitudes.
Present aircraft schemas must carry a positive sampling-window stamp, including
empty files; selected rows cannot redefine the observation window.

## Prepared road direction and traffic

Final road Arrow carries `road_traffic_contract=1`, four non-null Float64
`aadt_{light,medium,heavy,moto}` values (finite, nonnegative, EFFECTIVE
vehicles/day for this row) and non-null UInt8 `traffic_estimated`, a bitmask
0..15 where light=1, medium=2, heavy=4, moto=8 and a set bit marks that
category's value as an estimate or prior rather than an observed count.
Build-only source columns (`traffic_count_basis`, `traffic_observation_id`)
are removed at finalization. Road `oneway` is non-null UInt8: 0 two-way,
1 forward, 2 reverse. Popup and surface loaders share one strict reader
(`source_reader::road_traffic::RoadTrafficColumns` plus the `oneway`
validator) and reject missing contract metadata, wrong column types, nulls,
non-finite or negative counts, out-of-domain bitmasks and out-of-domain
direction data, including invalid schemas on empty files.

Rows with a measured period profile additionally carry non-null UInt16
`traffic_profile_id` (0 = none: absence means genuinely unknown, the
class-default split applies — never a compatibility variant) plus a
`roads_time_profiles` schema-metadata dictionary `{source, entries:
[{station, window, days, status, profile}]}`; ids are 1-based into entries,
restricted to entries referenced by that file. A profile holds per-class
day/evening/night shares of the 24 h volume (local periods 07–19/19–23/23–07);
an absent class in an entry is unmeasured and keeps the class default, and an
optional `total` share of the unclassified volume backs every class without a
class-specific observation as an explicitly transferred estimate (US TMAS
hourly totals) — never a measured class profile.
One canonical validation lives in `normalize::RoadTimeProfile::validate`.
The reader rejects a wrong-typed/null column, an id past the dictionary,
unknown class keys and malformed entries — malformed never degrades to
absence. `write::stage` passes the column and dictionary through allocation
and z14 reblocking verbatim (children inherit the parent's reference).
Emission consumes per-class shares via `NormalizedRoad::period_pcts` into the
single `build_period_flows` — there is no second day/night split. Unprofiled
rows keep the class-default `TIME_DIST_{MOTORWAY,URBAN}` behaviour unchanged.

The producer (`roads-finalize`) resolves observations, priors, directional
allocation, lane and access factors before publication.

Count basis (`traffic_count_basis`, build-only): 0 unknown, 1 directional,
2 both-directions, 3 already allocated, 4 street-cross-section. A directional
count is kept as published. Every other count is shared 1/n between the n
parallel one-way carriageways of the same class, country and corridor found
within 50 m. A national census publishes the two-way total of a road
(release r260910, classes 0-1: paired one-way rows sit at a median 0.50 of the
adjacent two-way count, lone rows at 1.00), so a both-directions count on a
LONE one-way row of class 0-2 with a ref or name takes 0.5: its sibling was
missed. Classes 3+ and links keep a lone count whole (30 % of their measured
one-way km are genuine one-way streets). A street-cross-section count (city
profile counters) is that one street's own total and stays whole on a lone
one-way street, which also keeps the publisher's class status. Every shared
count, unknown scope, and any other count on a one-way row is published with
all four `traffic_estimated` bits set; otherwise the bits are the adapter's:
clear only for a class the publisher counted, never inferred from the basis.

Priors for rows without a count (`noise_compute::defaults::resolve_traffic_default`):
hand-set city (São Paulo, Rio, Bangkok) and country (TH, BR) section totals
first. Otherwise motorway, trunk and primary take a measured world rate per
lane and per stored carriageway, vehicles/lane/day one-way / two-way:
motorway 6,379 / 3,010, trunk 4,533 / 2,594, primary 4,250 / 2,800
(length-weighted medians of measured rows of release r260910 over 174k / 218k /
200k km; leave-one-country-out MAE 3.2-3.7 dB, |bias| < 0.6 dB). The lanes tag
counts when 1-6; a row without one takes the median whole count of untagged
measured rows (motorway 5,200 / 6,019, trunk 1,810 / 3,045, primary
5,882 / 3,719). The vehicle-class split keeps the `WORLD_DEFAULT` proportions
of the class. Such a prior is per carriageway by construction: no carriageway
share, one-way half or `lane_ratio` applies, only `access_factor`. Classes
3-12 and every hand-set arm are both-directions section totals x
`normalize::road::lane_ratio` x share (1/n matched carriageways, 0.5 for a
standalone one-way row) x `access_factor`. No country or continent factor
exists: vehicles per paved km measured worse than none in every class, and
nine alternative predictors failed to beat a constant. These tables
remain authoritative for that build step. Serving consumes the prepared
counts verbatim: no traffic default, oneway share, lane or access factor is
re-applied at runtime, and a row stamped `source_id` 0 with positive counts
is a valid prior. A heavy-only count emits without inventing other classes;
a true total zero is silent and is never resurrected by a class default;
tunnels still do not emit. OSM direction never scales a count. Effective
speed retains the shared posted, taper, country-legal and class-default
rules. Contributor and segment traces report the dominant segment's four
class values, the estimated bitmask and the row dataset attribution;
source and vehicles-per-day units are preserved end to end.

## Prepared railway traffic

Final railway Arrow carries `rail_traffic_contract=1`, six non-null Float64
`trains_{passenger,freight}_{day,evening,night}` values, per-category UInt8
`{passenger,freight}_status` (0 unknown, 1 known, 2 estimated), UInt16
`{passenger,freight}_source_id`, and UInt8 `{passenger,freight}_matching`
(0 other, 1 estimated relation alignment, 2 estimated graph alignment; bitwise OR).
Counts are finite nonnegative expected passages per representative day in each
period. The producer clips geometry and resolves counts, missing-traffic priors,
service/parallel allocation and any estimated period split before publication.
Daily-only timetable evidence receives an explicitly estimated period allocation.
Unknown freight is not a known zero; a known numeric zero remains zero.
On non-service tracks (`service=0`), each unknown category receives its own
labelled class prior, independently of evidence in the other category. Existing
category values, including zero, are preserved; new priors are shared once.

Popup and surface loaders require this contract and use the same validator and
normalization. Emission and audibility reach consume these period counts directly
with 12/4/8-hour periods. Serving performs no traffic fallback, daily redistribution,
parallel division or service discount. Effective speed retains the shared posted,
high-speed and type-default rules. Contributor metadata follows the segment with
the greatest received Lden energy, including night-only traffic, and reports both
categories' status, source and matching evidence separately. Rail contributor
emission headlines use the same Lden period weighting as received levels.

## Aircraft local geometry

Doc 29 keeps its receiver-latitude scale and infinite-line CPA. Both the
receiver-to-segment-start longitude and the segment's own longitude extent use
the short arc from `grid::geo`; separately wrapping both endpoints would still
stretch a short segment beside the receiver's opposite meridian. Popup pruning,
CPA, reach gates and prepared-row kernels share this convention. Terrain-horizon
samples use the same canonical longitude interval. Moving a flight and its ridge
across ±180° must preserve the received SEL, screening and displayed CPA.

Airborne selection uses the periodic 16 km axis envelope, with the same f32
receiver-bound rounding at the batch and segment gates. A raw aggregate bbox
at least 180° wide cannot identify its contained short arcs, so it retains the
latitude gate but defers longitude pruning to individual segments. Owner
squares are selected in the same metre-per-degree metric that bounds a row's
length, so a decoded arc the envelope accepts always lies in a loaded square.
Cruise retains its separate cell-diagonal centroid gate.

Ground-operation line divergence uses half a canonical surface pixel at receiver
latitude: the z13 tile has 512 pixels. Popup and GPU use the same grid-derived
floor. This corrects the retired z12 floor, which was twice the current half
pixel and reduced the near-line contribution by approximately 3 dB.

## Ship traffic cells

A `ships.arrow` row is one AIS water cell: EMODnet 2024 uses 1 km ETRS89-LAEA cells;
Global Fishing Watch uses 0.01° cells with latitude-dependent area. Activity is expressed in mean
vessel-hours per month. Rows contain `centroid_gx/gy` (z30 grid), `area_m2`, `hours_large`,
`hours_work`, `hours_leisure`, `source_id`; stamps `grid=z30`,
`ships_contract=ships_v1`, `qm_blocks`. GFW window totals are converted to monthly
averages and contain large/work classes only (`hours_leisure=0`). Cells below 0.5
vessel-hours per month are not written (at most 76 dB(A) for large ships).

Emission (`emission/ships.rs`): the mean ships present per class is `hours / 730.5`
(365.25 · 24 / 12); the cell's A-weighted sound power is the energy sum of
`N_class · 10^(Lw_class/10)` with `Lw` 108 dB(A) for large ships (cargo, tanker, passenger,
high-speed craft, military, unknown), 98 dB(A) for work boats (tug, service, dredging,
fishing, other) and 88 dB(A) for leisure craft (sailing, pleasure), one value for sailing and
moored ships (Fredianelli et al. 2020 pass-by line levels 82.6–89.0 dB(A)/m; Bernardini et al.
2022 moored and small-vessel levels; NEPTUNES). Octave spectrum 63 Hz … 8 kHz relative
`[0, 0, −3, −6, −9, −13, −18, −25]` dB, normalized so the A-weighted total equals `Lw`. Source
height 15 / 5 / 3 m and the popup class label follow the class carrying most of the energy.
Ships run around the clock: day, evening and night bands are equal, so Lden = Leq + 6.4 dB.

Geometry (`normalize::prepare_ship_points`): the cell is a square area source of `area_m2`
around its centre, gridded at 250 m by the shared area discretizer (each sub-cell carries its
area share of the energy and a self-screening exclusion radius √(A/π)), propagated by the
ISO 9613-2 / CNOSSOS-EU point kernel of buildings and industry (water reads IMD 100 → G = 0).
Reach: the Lw-derived audibility radius capped at `SHIP_MAX_RADIUS_M` = 11 800 m, inside the
painter's 11 872 m profile cadence; the popup reads cell centres within 12 507.2 m
(the reach cap plus a fixed 707.2 m pad). Popup contributors group sub-cells by the cell's
identity `(gx << 32) | gy`. A cell beyond the cap contributes nothing. EMODnet takes
precedence where its raster samples a cell centre; GFW supplies other covered waters.
Waters absent from both products have no rows.

## Open parking and emission-only grounds

Open parking ways use `leisure_v3` classes 8 (lot) and 9 (street strip), with no
screening geometry. Their mapped area estimates spaces at 23.8 and 13.3 m² per
space. Day sound power follows the Parkplatzlärmstudie (LfU, 6th ed. 2007):
63 dB(A) per movement/hour, 0.40 movements/space/hour and the searching term
2.5 log10(spaces − 9) above ten spaces. The 06–22 / 22–06 rates 0.40 / 0.05
are averaged over this engine's periods: evening −1.1 dB, night −6.3 dB.
These are model defaults, not measured traffic for an individual car park.

Functional grounds and underground sources retained in structures have null
screening geometry and zero screening height at default height tier 2
(`structures-builder-2` and later). Explicitly underground Overture footprints
are excluded from above-ground screening and matching (`structures-builder-3`);
an independently mapped above-ground OSM building keeps its own wall. Mapped
sub-metre building heights retain tier 0 even when the screening height rounds
to zero. Both popup and painter preserve that distinction when normalizing
emission: one
mapped ground area, no floor multiplier, source height 1.5 m (the existing
open-air activity convention). Raw building height/floor tags cannot turn such
an area into a facade source. A real building with unavailable geometry keeps
its nonzero screening height and normal building defaults. Popup traces report
zero building height and floors for ground activities, including open parking;
propagation still reports the actual source height.

Explicit OSM open structures (`building=carport`, `building=roof`, or
`amenity=parking` with `parking=carports`) carry outdoor `building_use=3`
in `buildings_v5`.
The structures builder preserves that outdoor envelope for OSM-only and
Overture-matched rows (`structures-builder-4`), so a canopy cannot acquire an
indoor attenuation from an absent or generic Overture class. Enclosed garages
retain their existing classification. This changes enclosure only: screening
geometry, height, emission and traffic remain unchanged.

## Line sources: the CNOSSOS point sum

A road or rail piece is a straight 3D line between its endpoints' ground plus the
source height. Directive 2015/996 §2.5.3 splits a line into incoherent points of
`A_div = 20·lg r + 11`; for a straight piece `dx/r² = dφ/d⊥` (φ the angle in the
plane that holds the line and the receiver, d⊥ the 3D distance from the receiver to
the line, floored at 0.5 m), so the point sum is exactly
`E = W′/(10^1.1·d⊥)·∫ 10^(−A_path(φ)/10) dφ`. In free field this is
`L_W′ + 10·lg θ − 10·lg d⊥ − 11`; an infinite line reads `L_W′ − 10·lg d⊥ − 6.03`
(the retired `−10·lg(2π·d) + 10·lg(θ_horizontal/π)` chain sat 1.9533 dB lower and took
the finite-line angle in plan, #5 and #28).

The integral uses one rule in popup and painter (`propagation::line_quadrature`,
CUDA `relevant_source_arc.cuh`/`relevant_source_pair.cuh`): five buckets of equal Δφ,
each node on its own ray from the piece to the receiver with its own profile, ground,
terrain, screening, vegetation and air absorption at its own slant distance, weight
Δφ. A bucket spanning at least 3° of horizontal azimuth replaces its node by
geometry-placed nodes: every obstacle edge within reach that stands at least a metre
in front of the piece marks a 128-bin blocked mask over the bucket's azimuths (walls
lower than the source height, and grid cells whose tallest edge is, are skipped);
every blocked run and clear gap is split into parts of at most 0.26 rad (at most nine
per run), each part one node weighted by its own Δφ, obstacles read on blocked parts
only. A line source radiating with the CNOSSOS-EU rail track dipole `0.01 + 0.99·sin²ψ` (ψ between
track and ray; sin²ψ = cos²φ in the in-plane angle) weights each node by the dipole's closed-form
integral over its Δφ instead of Δφ; rail rows stay omnidirectional until W4's emission, fitted
with the dipole and the two source heights, lands (each height is then its own line source).
Against a fine point sum (1°/10 m nodes through the same per-ray physics,
`point-sum-oracle`) the rule is within ±0.15 dB on straight roads over G = 0, 0.5, 1
at 5 m–2 km and behind a roadside wall, and within 0.28 dB per layer at ten real receivers
(2026-09-24, the largest behind the M25 J17 barrier).

A source–receiver pair is skipped only when the relevance bound of
`propagation::relevance_bound` stays below 0 dB in every band of every period:
`B = L_W − A_div,min(d) − α_min·d/1000 + 9.6 dB`, a line bounded by its infinite line at its
closest horizontal distance, a point by `20·lg d + 11`, α_min the smallest absorption of the
weather, 9.6 dB the largest favourable gain of the method over flat ground (p = 1 assumed): a
night-only source is never dropped by a day-only gate (#31). A road or rail row reaches as far
as that bound's Lden stays above 30 dB (the display floor), capped so no ray outruns the
painter's 64-sample profile (11,872 m, minus the 250 m longest piece for a line's closest
point); popup and painter share the reach. Over relief the method itself gains up to 13.1 dB (a
grazing hard crest takes the favourable floor on both sides, `boundary_gain_tests.rs`); the
bound does not cover that yet.

## One ray: CNOSSOS-EU per meteorological state

Every line quadrature node and every point source runs one ray (`ray_transfer.rs`); the
painter runs the same ray in f32 (`relevant_source_cnossos_stream.cuh`).

- Profile: the bare-earth samples of the bilateral cadence, G = 1 − IMD/100 per sample, both
  linear between samples. Within the source's platform half-width the terrain may not rise
  above the source ground (road: lanes × 3.5 m / 2 + 1.5 m, two lanes when untagged; rail
  2.5 m; points 0) — this replaces the 30.9 m source clamp that erased berms.
- Obstacles: every crossing of the ray with a building wall or barrier; its top is the
  terrain there plus its height. Building crossings nearer a point source than its footprint
  radius are its own building. A footprint's crossings pair into roofs in chainage order
  (entry, exit); an unpaired last crossing has none; roofs are taken in the order the ray
  leaves them and each starts no earlier than where the roofs before it end (overlapping
  footprints are 0.4 % of roof length on the oracle's real rays). Roofs are hard raised ground
  (G = 0) in the mean planes and ground factors, the ISO/TR 17534-4 geometry; walls are not
  ground. Footprints are named by index and id, so two squares' footprints never pair.
- Candidates: the bare terrain samples and the obstacle tops.
- Diffraction points per state: homogeneous rays are straight; favourable rays are arcs of
  radius Γ = max(1000 m, 8·d). A candidate above the state's ray (favourable: lowered by the
  arc's height above the chord) blocks it; the points are then the upper hull of S, the
  blocking candidates and R (the rubber band, any number of edges). An unblocked state takes
  the one candidate with the largest path difference (homogeneous −(SO + OR − SR) below the
  chord; favourable (2.5.26) above the straight chord, else (2.5.27)), admitted per band by the
  Rayleigh criterion δ > −λ/20 and δ > λ/4 − δ*, S* and R* mirrored in the side planes.
- Mean planes: the continuous least-squares line of the roofed ground over the whole path, and
  over the ground before the first and after the last diffraction point; heights orthogonal to
  the plane, a negative one taken as 0 with its sign kept; dp the projected distance.
- A_ground (2.5.14)–(2.5.20): the homogeneous state uses G′path, blending in the source ground
  Gs on short paths; the favourable state uses the modified heights of (2.5.19) and the lower
  bound (2.5.20) on the unmodified heights; a hard path is −3 dB homogeneous and the bound
  favourable. Gs: road carriageway and bridge decks 0, ballast 1, embedded tram track 0; a point
  source the ground under it.
- A_dif (2.5.21)–(2.5.32): Δdif = 10·lg(3 + 40·C″·δ/λ) with C_h = 1 and C″ for two or more
  points spanning more than 0.3 m; the ground correction split on both sides; only Δdif(S,R)
  is capped at 25 dB; a source or receiver below its side's plane takes that side's A_ground
  whole and the mirrored Δdif. There is no minimum path length.
  **Numerical domain:** if either ground-split logarithm has a non-positive argument and
  hence a non-finite result, that side takes its whole A_ground and its image Δdif,
  source then receiver. This follows NoiseModelling's `AttenuationCnossos.aDif`; the
  published equations do not specify this fallback. It also covers the zero-argument
  (infinite) limit. A recorded Prague two-roof path tests this domain separately from the
  ISO accuracy fixture. Any other non-finite or negative linear energy fails its receiver
  instead of flooring to a quiet layer.
- The states are mixed only at the end, per period and propagation direction:
  `10^(−A/10) = p·10^(−A_F/10) + (1 − p)·10^(−A_H/10)` with p of the period and of the
  direction's 16-sector climatology (0.5 everywhere until W6 delivers it).
- A_atm: ISO 9613-1 at exact mid-band frequencies, per period and band from the mean μ and
  variance σ² of the hourly coefficient: `max(μ·d − (ln 10/20)·σ²·d², α_min·d)`, d the slant
  distance in km; until W6 delivers, 15 °C / 70 % (the CNOSSOS default) with no variance.
- Forest: the plan-view depth rule is unchanged until the bare-earth DEM and the canopy height
  land together (ISO 9613-2 A.2.2 on each state's ray, S4); on today's surface model the
  canopy is terrain.
- Popup hypotheses: free field is the whole-path A_ground alone; no terrain leaves the terrain
  out of the candidates; no screening removes every crossing (tops and roofs); no ground drops
  every ground term; no forest; no air absorption.
- Acceptance: all 28 ISO/TR 17534-4 Direct cases, LH and LF, within ±0.1 dB in every band
  (`iso_tr_17534_4_tests.rs`); the painter against the popup's CPU ray on synthetic scenes
  (`surface-cuda-check`): flat ground of four ground factors within 0.001 dB, a ridge with
  touching, overlapping and courtyard buildings and two walls within 0.16 dB (the largest a
  wide-bucket mask bin at a wall edge moving in f32).
- The literal standard is not monotone in obstacle height (W2 `edge-height-monotonicity.txt`);
  what holds is that adding a candidate never shortens the rubber band.

The painter streams the ray: samples and crossings in chainage order (the scene's obstacles are
one merged grid, each cell taking the crossings inside its own chainage window) feed both
states' monotone-chain hulls, and every hull entry carries the ground moments of its two sides,
so the side planes of whichever points end up first and last come out without storing roofs. A
ray that outruns a fixed capacity (64 samples, 32 hull points, 32 crossings in one cell, 16
open footprints) fails its cell instead of painting.

### Popup trace

A ray's trace shows the homogeneous state's diffraction points: terrain points as the terrain
edges with the terrain-only path difference, and the obstacle top standing highest above the
straight line of sight as the representative crossing with the full path difference. A line
piece's trace shows its loudest quadrature node's ray, and its fan lists every node with its
horizontal azimuth stretch and 1 kHz terrain and screening effect. The terrain, screening and
forest impacts are the A-weighted differences between the full and the hypothesis Lden.
