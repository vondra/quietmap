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

Every receiver stands `DEFAULT_RECEIVER_HEIGHT` (4 m) above the bare-earth ground
unless the caller names another height of at least `RECEIVER_HEIGHT_FLOOR_M`, as
validation does for a microphone's height; every layer reads `Receiver::altitude_m`
and the answer's `receiver` names the point and height computed. Outside enclosed
buildings (streets, open ground, water, courtyard holes, outdoor-class carports
and roofs) the receiver is the clicked point or the pixel centre.

Inside an enclosed building (the tallest enclosed footprint containing the point;
equal heights go to the smallest (square, id) key) the receiver is the building
exposure: CNOSSOS §2.8 case 1 façade receivers (Directive (EU) 2021/1226,
Annex II; `facade_receivers.rs`) — every ring edge cut into the fewest equal
intervals ≤ 5 m, one receiver mid-interval; an edge of 2.5–5 m gets one; runs of
shorter adjacent edges together over 5 m are cut the same way as a polyline;
lengths within one z30 quantum of a limit read as the limit. Receivers stand
0.1 m out along the outward normal (into the courtyard for holes). A receiver
inside any enclosed footprint (a party wall) is dropped; a footprint with no
qualifying edge keeps one receiver mid its longest edge; a building whose every
receiver is dropped has no exposed façade and is not assessed. Canonical order:
parts as stored, exterior counter-clockwise and holes clockwise, each ring from
its lexicographically smallest z30 vertex, rotated to its first edge over 2.5 m.
At a façade receiver the density bonus (0/1.5/3 dB, nine probes at ±75 m)
ignores probes inside its own footprint (§2.8: the façade's own reflection is
excluded); the own building still screens sources behind it. The façade-exposure
stage evaluates every receiver of every building on the GPU and stores the one
with the highest all-source Lden (ties: lowest canonical index) with its layer
and period powers in `facade_exposure.arrow`. The popup recomputes the receiver
set, requires the stored choice to be one of its points, reloads obstacle
indexes and sources around it and evaluates it exactly; a missing file refuses
the click. There is no indoor attenuation anywhere.

A painted z13 pixel is the outdoor receiver at its centre, or, when the centre is
inside an enclosed building, that building's stored layer powers (every tile
covering the building copies the same row). Tiles (HM3 v4, `tile-painter/src/hm3.rs`)
keep coverage apart from energy: 2·Lden bytes 0–253, 254 computed silence, 255 not
assessed; a zoomed-out cell is the energy mean of its assessed children, silence
counting as zero energy.

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
chord 14–32 km east of the receiver, 44.95 dB whole, keeps its 42.28 dB first
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

## Aircraft sampling window

Aircraft rows come from two ADS-B providers merged per address and UTC day: the
primary (adsb.lol) on every admitted baseline day, the secondary (ADSBexchange)
on the admitted increment days, adding only samples the primary did not cover.
A row touching a secondary sample carries flag bit 6 (`SECONDARY_ONLY`; cruise
and ground rows a `secondary_only` column). Every aircraft file stamps
`baseline_days`, `increment_days` and the SHA-256 of each sorted day list; one
popup or painter tile refuses files whose windows differ. The mean day is the
difference estimator `Σ_primary E / baseline_days + Σ_secondary E /
increment_days`: every consumer divides by `baseline_days` and weights a
secondary-only row by `baseline_days / increment_days` (`ProvenanceWeights`),
in energy and in movement counts. A flight counts as a baseline movement when
any of its rows at the receiver, microsegment or airport category is primary.

## Prepared road direction and traffic

Local roads (residential, living street and unclassified) without a higher-priority
observation use S2p before finalization:
`T[cell] × (through and urban ? c : 1) × (singleTrack ? d : 1) + k × G_street`.
The three cells are urban residential (also living street), urban unclassified and
pooled rural (also unknown built-up). Parameters and count/source provenance live in
`pipeline/lib/local-street-demand.json`; `scripts/roads/fit_local_street_demand.py`
regenerates them from DfT manual minor-road AADF, excluding holdout squares before
feature extraction and using five CV folds grouped by square.

Buildings generate the existing trip-rate demand at the OSM emission centroid,
using `structures_v5.storeys` from the structures height ladder. The existing
multi-source Dijkstra routes that demand to motor exits. `G_street` is the maximum
total routed demand of any piece with the same name within one tree component,
or of the same OSM way when unnamed. `through` means that this street contains
an edge whose ends drain to different exits; the boundary marker is not routed
into other streets. The through premium applies only in built-up areas: rural
connectors are farm tracks without through traffic (a fitted split premium is
1.80 urban against 1.03 rural, 95% interval [0.80, 1.27]). `singleTrack` means
some street piece is mapped single-lane and none is mapped wider; untagged
streets keep the full background. Background cells remain row-local across
class/urban boundaries.
Public local streets have no 20/day floor or class cap. Class 7 keeps its historical
per-row routed demand clamped to 20–400/day, with no traffic when buildings are absent.
The producer rounds and splits the total once; source 11 and allocated basis 3 remain.
After structures and built-up change, rerun roads-service-tree → continuity → taper →
roads-finalize from parent roads. A server restart alone cannot apply this model.

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
one-way street, which also keeps the publisher's class status. A roundabout
ring (`junction` 1 or 2) is allocated as a two-way road: every point of the
ring carries about the two-way flow of one approach, so neither a count nor a
prior is halved on it, and two arcs of one ring are never two carriageways.
Every shared count, unknown scope, and any other count on a one-way row is
published with all four `traffic_estimated` bits set; otherwise the bits are
the adapter's: clear only for a class the publisher counted, never inferred
from the basis.

Priors for rows without a count (`noise_compute::defaults::resolve_traffic_default`):
hand-set city (Bangkok) and country (TH) section totals first. Otherwise
classes 0-4 take a measured prior per stored carriageway,
`MEASURED_CARRIAGEWAY_PRIORS` in `road_traffic_priors_generated.rs`, indexed by
class, direction (one-way carriageway or two-way road) and `built_up`
(unknown takes the cell fitted on every row). Motorway, trunk and primary
have a vehicles-per-lane rate used for a lanes tag of 1-6 and the whole count
of an untagged carriageway otherwise; secondary and tertiary have the whole
count only. The table is regenerated by `pipeline/fit-road-traffic-priors.ts`:
length-weighted medians of counted public carriageways (tunnels, roundabouts
and derived flows excluded) in training squares of holdout rule v1, scored on
the holdout squares; its header records the release and kilometres. The
vehicle-class split keeps the `WORLD_DEFAULT` proportions of the class. Such a
prior is per carriageway by construction: no carriageway share, one-way half
or `lane_ratio` applies, only `access_factor` (the half-share of a two-way
default on an uncounted one-way town street read -4.7 to -10.6 dB on holdout
rows and is gone). Classes 5-12 and every hand-set arm are both-directions
section totals x `normalize::road::lane_ratio` x share (1/n matched
carriageways, 0.5 for a standalone one-way row) x `access_factor`. No country
or continent factor exists: vehicles per paved km measured worse than none in
every class, and nine alternative predictors failed to beat a constant; a
metro-size term failed leave-one-country-out and is not used.

Every finalized piece also carries `cross_section_aadt`, the whole road's
vehicles per day at that piece (owner decision 2: the popup headline shows the
whole road, both directions, and the details the carriageway's four classes).
It is the row itself when the row establishes the whole road (a two-way row or
ring, a known share of a two-way total, a street's or tree's own flow), else
the sum over the carriageways found together, and 0 where only this one
direction is known (a lone directional count or a lone carriageway prior).
Emission never reads it.

These tables remain authoritative for that build step. Serving consumes the
prepared counts verbatim: no traffic default, oneway share, lane or access
factor is re-applied at runtime, and a row stamped `source_id` 0 with positive
counts is a valid prior. A heavy-only count emits without inventing other
classes; a true total zero is silent and is never resurrected by a class
default; tunnels still do not emit. At serving time OSM direction never scales
a count. Effective speed retains the shared posted, taper, country-legal and
class-default rules. Contributor and segment traces report the dominant
segment's four class values, its `cross_section_aadt`, the estimated bitmask
and the row dataset attribution; source and vehicles-per-day units are
preserved end to end.

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
On non-service tracks (`service=0`) other than preserved heritage rail (type 5),
each category is allocated once per line
cross-section: a track and each other way running beside its midpoint (same type
and usage family, no shared node; 15 m and 10° without a common ref or name,
50 m and 20° with one) form the cross-section. The highest-ranked evidence on
any of its tracks sets the line value (the sum of what a measured source counted
on each track, such as routed trips and platform stops, or the value a proxy or
residual repeats on every track); a timetable's no-service residual yields to any
ranked evidence on the line; without evidence one labelled class prior applies. Each
track carries the line value divided by the number of tracks, so a proven zero
stays zero and the cross-section sum equals the line value in every category.
Heritage rows retain type 5, observed traffic and posted speed. Missing traffic
stays zero with status 0 (unknown), with no class speed or high-speed fallback.
They are labelled heritage and emit nothing until a heritage model is available.

Popup and surface loaders require this contract and use the same validator and
normalization. Emission and audibility reach consume these period counts directly
with 12/4/8-hour periods. Serving performs no traffic fallback, daily redistribution,
parallel division or service discount. Effective speed retains the shared posted,
high-speed and type-default rules. Each category's representative speed, the
effective speed within its vehicle range (freight at most 88.9 km/h, the EBA 2023
train-weighted mean), sets both its per-train level and its line density. Contributor metadata follows the segment with
the greatest received Lden energy, including night-only traffic, and reports both
categories' status, source and matching evidence separately. Rail contributor
emission headlines use the same Lden period weighting as received levels.

## Aircraft finite-segment corrections

Popup, airborne CUDA and cruise CUDA apply Doc 29 Vol 2 Eq. 4-8b at every
slant: NPD SEL + ΔV + ΔI − Γ(ℓ)Λ(β) + ΔF. Lateral attenuation is independent
of engine installation, including helicopters via AEDT 2c Eq. 4-70; only
airport-ground evaluation bypasses it. For β < 0, Λ = 10.857 dB is still
multiplied by Γ(ℓ). Terrain and building diffraction compete with lateral
attenuation through their maximum at all distances. Installation ΔI keeps
its own wing/fuselage coefficients; propeller/helicopter ΔI is zero.

The finite-segment integral uses the class anchor's scaled distance
`d_lambda = (2/pi) V_ref t0 10^((SEL − LAmax)/10)` (Doc 29 Eq. 4-11), with
V_ref in m/s and t0 = 1 s. SEL − LAmax interpolates in log distance and
extrapolates with the nearest two NPD rows, independently of the existing
SEL energy-tail extrapolation. Profiles whose generated LAmax is the
placeholder SEL − 12 use the dipole limit d_lambda = slant (Appendix E).
Both metrics use the same 128-bin log-distance grid on CPU and CUDA.
The old constant scaled distance and 7,620 m correction cutoff are removed.
This is a runtime model change: prepared airborne/cruise rows remain valid;
recompute receiver exposure and tiles under the coordinated physics generation.

Airborne fields prepare split-piece geometry and possible predecessor links once
per scene. CUDA selects the first surviving predecessor at each receiver, then
sums each accepted chain in source row order before applying the free and received
20 dB event floors. Double precision keeps small finite-segment fractions and
floor decisions aligned with the canonical CPU scatter. Receiver batches bound
working storage; terrain marches reuse the existing bilinear DEM tile handle and
produce the same packed horizons as the uncached sampler.

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

Open parking ways use `leisure_v4` classes 8 (lot) and 9 (street strip), with no
screening geometry. Their mapped area estimates spaces at 23.8 and 13.3 m² per
space. Day sound power follows the Parkplatzlärmstudie (LfU, 6th ed. 2007):
63 dB(A) per movement/hour, 0.40 movements/space/hour and the searching term
2.5 log10(spaces − 9) above ten spaces. The 06–22 / 22–06 rates 0.40 / 0.05
are averaged over this engine's periods: evening −1.1 dB, night −6.3 dB.
These are model defaults, not measured traffic for an individual car park.

Functional grounds and underground sources retained in structures have null
screening geometry and zero screening height with the ground-activity height
source (`structures_v5`). Explicitly underground Overture footprints
are excluded from above-ground screening and matching (`structures-builder-3`);
an independently mapped above-ground OSM building keeps its own wall. Mapped
sub-metre building heights keep their mapped-height source even when the
screening height rounds to zero. Both popup and painter preserve that
distinction when normalizing emission: one
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
Overture-matched rows (`structures-builder-4`), so a point under a canopy stays
an outdoor receiver instead of taking a building exposure from an absent or
generic Overture class. These rows and
Overture `roof`/`carport` classes screen at 0 m (`structures-builder-5`): a
roof on posts has no wall to diffract over. Footprint, emission, envelope and
traffic stay. Greenhouses, grandstands and enclosed garages keep their walls.

## Screening heights

The structures builder gives every footprint one screening height, the mean
roof height, from the first available rung, and stores its `height_source`:

1. regional survey zonal mean (Prague LiDAR), clamped to 2.5–250 m;
2. mapped OSM `height`;
3. OSM, national-register or Overture floors × 3 m + 3 m roof allowance
   (Prague LiDAR vs OSM floors, 105,957 buildings: median residual 0.0 m);
4. Overture height of at least 2.5 m (lower values are artefacts);
5. GHS-BUILT-H ANBH of at least 3.5 m (its 2.5 m floor and the values just
   above it are no information), capped at 100 m and at 4 m under 30 m² of
   footprint;
6. median reference height by footprint area: < 30 m² 2.9 m, < 60 m² 5.4 m,
   < 150 m² 7.4 m, < 500 m² 8.8 m, else 10.6 m (seven EU pilot windows).

Rungs 5 and 6 are not per-building knowledge; only they take the low-profile
cap. The demand storey count `storeys` is the floor count where one is mapped,
else round((height − 3 m) / 3 m), at least 1; a structure without a screening
height counts one level. The service-tree demand reads it. Noise walls keep a
mapped OSM height; unmapped walls stand at their country's mean wall height
(DE 3.88 m, US 4.45 m, AT 3.6 m, else 3 m).

## Raster terrain and canopy inputs

The next raster generation uses bare-earth `dem.u16le`: WGS84 one-arc-second
nodes in `grid::raster::RasterWindow`, EGM2008 metres, decoded as −500 + v/5;
65535 is missing. The terrain producer area-averages the source footprint at each
node and gives national DTMs precedence over the global DTM. Runtime elevation
remains bilinear. The old signed big-endian DEM is not accepted by this reader.

`canopy.u8` records canopy top above bare earth, 0–250 metres (255 missing),
nearest sampled into `PathProfile.canopy_m` beside `forest.u8` canopy cover.
The CUDA upload carries this height at byte 6 of the existing eight-byte
`FusedPixel`; `SampledRasterPoint.canopy_m` exposes it without changing attenuation.
A zero-byte channel file denotes independently verified ocean; a missing file,
wrong length or sampled missing node fails the operation. A producer may not
turn missing canopy into zero. Height is for foliage only, never subtracted
from a surface DEM to manufacture terrain.

Source fetches retain URL, fetch and terms-check timestamps, SHA-256, byte count,
licence and licence URL. A published square carries source epochs, coverage
fractions and its output digest. National vertical transforms must use PROJ
with ballpark operations disabled and required grids present. The geoid shift
is evaluated at every target node after resampling; missing grids fail.

Changing this generation invalidates terrain-dependent altitudes, structure
bases, aircraft preprocessing and horizon calculations, façade exposure and
painted tiles. Bridge-deck/railhead geometry and the canopy propagation model
must be integrated before this generation is used for a production calculation.

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
only. A line source radiating with the CNOSSOS-EU rail track dipole `0.01 + 0.99·sin²ψ` uses ψ
between the **horizontal projections** of track and ray (2.3.15). Each node is weighted by the
integral of that horizontal directivity over its 3D in-plane Δφ. With `u = tan φ`, its dipole
part is `b² / ((u+a)²+b²)`: `a` is the projected along-track offset of the 3D perpendicular foot,
and `b` the horizontal perpendicular distance, each divided by the horizontal track speed and
the 3D perpendicular distance. Partial fractions integrate this against `du/(1+u²)`; near
coincident quadratics (dimensionless denominator < 1e-2), eight-point Gauss–Legendre avoids
cancellation. For a coplanar source and receiver this reduces to the original integral of
`cos²φ`; an elevated receiver needs the horizontal projection. Rail rows stay omnidirectional until W4's emission, fitted
with the dipole and the two source heights, lands (each height is then its own line source).
Against a fine point sum (1°/10 m nodes through the same per-ray physics,
`point-sum-oracle`) the rule is within ±0.15 dB on straight roads over G = 0, 0.5, 1
at 5 m–2 km and behind a roadside wall, and within 0.28 dB per layer at ten real receivers
(2026-09-24, the largest behind the M25 J17 barrier).

A point source–receiver pair is skipped only when the relevance bound of
`propagation::relevance_bound` stays below 0 dB in every band of every period:
`B = L_W − A_div,min(d) − α_min·d/1000 + 13.3 dB`, a line bounded by its infinite line at its
closest horizontal distance, a point by `20·lg d + 11`, α_min the smallest absorption of the
weather, 13.3 dB the largest favourable gain of the method over flat or relief ground
(p = 1 assumed): a grazing hard crest takes the favourable floor of (2.5.20), −9 dB per
side, with a blocked Δdif of at least 10·lg 3, so 2·9 − 10·lg 3 = 13.2 dB at most
(13.09 dB found, `boundary_gain_tests.rs`): a night-only source is never dropped by a
day-only gate (#31). A road or rail row reaches as far as that bound's Lden stays above
30 dB (the display floor), capped so no ray outruns the painter's 64-sample profile
(11,872 m, minus the 250 m longest piece for a line's closest point); popup and painter
share the reach, and a pair inside it is never below 0 dB in every band.

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

Until the per-square `meteorology.bin` files are consumed, every period uses the same default absorption
coefficients (dB/km), rounded here to two decimals; the implementation computes them from
ISO 9613-1 at exact mid-band frequencies, 15 °C, 70 % RH and 101.325 kPa. Variance is zero,
and each period's directional favourable probability is 0.5.

| Nominal band (Hz) | 63 | 125 | 250 | 500 | 1000 | 2000 | 4000 | 8000 |
|---|---|---|---|---|---|---|---|---|
| Mean α (dB/km) | 0.10 | 0.38 | 1.13 | 2.36 | 4.08 | 8.75 | 26.39 | 93.71 |

The W4 emission integration must supply one independently powered line per source height
A/B. The current CPU `LinePiece.source_height_m` and CUDA `DeviceLineSource.source_height_m` are relative
to the sampled terrain (the formation datum once bare earth lands): set them to
`railhead_offset_m + 0.5` and `railhead_offset_m + 4.0` respectively, and attach each
height's emission, directivity and distinct source-part identity.
Do not duplicate today's complete row emission into both heights. The deterministic CUDA
check exercises both heights above a raised railhead and distinct per-period, per-sector
weather probabilities with nonzero absorption variance.

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

## Retained OSM model evidence

The extraction contract constants live in `square-store::osm_contract`: spill
format 2, roads/railways/industrial evidence 2, `leisure_v4`, and
`transport_nodes_contract=1`. Readers reject older stamps; rebuilding requires
fresh extraction outputs. Existing country-bake and grid contracts still apply.

Road rows retain raw speed/surface/vertical-structure tags in `osm_tags` plus
numeric `maxspeed_hgv` (u16 km/h, 0 unknown, 65535 unrestricted).
`osm-extract::implicit_speed` owns the sourced passenger
implicit-rule table. Explicit `maxspeed` wins; unresolvable conditional rules
remain unknown and their original text survives. HGV implicit rules are retained
without applying passenger limits. Direction codes are 0 two-way, 1 explicit
forward, 2 reverse, 3 implied roundabout, 4 implied motorway; all forward codes
participate in continuity and directional traffic matching. Whole-way endpoint
node IDs and grid coordinates survive microsegmentation, so adjacent bridge
ways can form runs and identify abutment candidates without mistaking piece
boundaries for abutments. These endpoints are evidence, not a deck-height model.

Transport control rows retain node identity, raw crossing/signal/whistle tags,
and one incidence per road or rail way (vertex index and whole-way chainage).
Unlinked controls remain explicit null incidences; there is no proximity guess.
National whistle values and `railway:traffic_mode`, usage, service and heritage
survive. Original railway node chains and piece intervals already supply curve
geometry to rail finalization; no new curve-radius approximation is introduced.

Industrial source classes 11/12 identify wind-plant outlines and inactive
facilities: neither falls through to generic factory emission. A wind-plant
outline requires wind as the sole `plant:source`; mixed fuels and copied
generator tags do not silence a plant. Classes 13/14/15
retain solar, substation and transformer evidence for their specific models.
They currently emit nothing pending those models. Raw power/output/rating and
lifecycle tags survive, with OSM object kind to disambiguate IDs. Registry
matching does not overwrite these classes. Industrial and leisure multipolygons
retain every closed outer component as a separate row; unclosed fragments are
omitted rather than assigned an area. Inner holes remain outside the
existing single-ring geometry contract.

`leisure_v4` adds motorsport class 10 and shooting class 11, `osm_tags`, OSM kind,
geometry kind (0 point, 1 area, 2 line) and line length. Two-node raceways and
motor-sport tracks survive with open-chain geometry; enclosing polygons are
separate area rows. Open non-motorised tracks also retain their line path;
coordinate snapping does not change line/area identity. Shooting subtype and indoor/building flags survive on nodes,
ways and relations. These two classes are staged and silent until the activity
models consume them; an enclosing area must not duplicate a line's emission.
A physical building also retains its separate source row and has no generic
residential emission. Buildings keep `buildings_v5`, the existing roof/carport
use code and the unchanged height ladder.

## Meteorology climatology input

The meteorology producer streams the fixed 1991–2020 ERA5 normal at three-hour UTC
steps. Every 0.25° node uses `aircraft_extract::period::resolve_tz` for historical
local civil END periods, including daylight-saving transitions. Solar elevation,
not the END period, selects the day/night stability class. Nord2000 weather
classes use Eurasto (2006) Tables 1–7 with the dimensionally consistent logarithmic
profile coefficient A: its temperature term does not divide by Monin–Obukhov L.
A class is favourable when its representative c(10 m) − c(0) is positive.

`meteorology-contract.json` defines the producer/reader contract in one place.
Each z9 square, oceans included, holds its ERA5 nodes in `meteorology.bin`: a
16-byte header (8-byte magic naming the contract version, then the window's
west and north nodes and its column and row counts, little-endian) followed by
row-major 240-byte node records (48 UInt8 `p` percentages, then 24 Float32
`alpha_mean` and 24 Float32 `alpha_variance` values, little-endian). The window
is `grid::raster::RasterWindow::for_square_with_density` at 4 nodes per degree,
the same floor/ceil edge bracketing as the 1″ rasters, so every square
interpolates from its own file alone. Each period stores `p` (16 percentages)
and `alpha_mean`/`alpha_variance` (eight population moments of hourly ISO 9613-1
coefficients in dB/km); readers derive `p_max` as the sector maximum. Midbands
follow ISO 266; absorption uses hourly temperature, dewpoint-derived liquid-water
relative humidity and surface pressure. No missing observations are silently
discarded. Sector zero is sound travelling north, with centres every 22.5°
clockwise; meteorological wind-from bearings must be reversed. Exact hourly
favourable counts determine stored p; retained 20° wind histograms do not
quantize that calculation.

`raster_reader::meteorology::Meteorology::at` interpolates moments and probabilities
bilinearly at the receiver inside the owner square's window, wrapping longitude.
`MeteorologySample::probability` interpolates circularly between sector centres.
Invalid coordinates, wrong magic, mismatched windows, short files, nonfinite
values and invalid percentages are errors. Window maxima conservatively bound
any interpolation inside the window. No serving or painting path loads the
files yet: popup and painter use the built-in defaults above until propagation
consumes them, and release assembly attaches the files in that same change.

The streamed producer retains period × wind-class × stability-class × direction
histograms, exact favourable counts, Welford absorption moments and SHA-256 chunk
receipts. Two alternating, fsynced checkpoint slots bind these statistics to a
SHA-256-verified manifest prefix and the source, timezone and producer identities.
The producer snapshots the actual historical TZif rules and reuses them on restart;
Python dependency versions are also bound to the checkpoint identity. Restart replays
only the uncommitted interval (at most seven days); it cannot count that interval
twice. Arrow publication is atomic and only follows the full normal. The source
licence, acquisition receipts, code and table digests enter the release identity.
