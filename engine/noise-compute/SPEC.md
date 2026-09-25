# Popup propagation contract

The Rust engine is the acoustic source of truth. This document specifies the
current screening contract; it is not a claim of full CNOSSOS-EU compliance.
The implementation and regressions live in `src/propagation/path_effects.rs`,
`diffraction.rs` and `arc_screening.rs`.

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

Local roads (residential, living street and unclassified) without a higher-priority
observation use S2p before finalization: `T[cell] × (through ? c : 1) + k × G_street`.
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
into other streets. Background cells remain row-local across class/urban boundaries.
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

These tables remain authoritative for that build step. Serving consumes the
prepared counts verbatim: no traffic default, oneway share, lane or access
factor is re-applied at runtime, and a row stamped `source_id` 0 with positive
counts is a valid prior. A heavy-only count emits without inventing other
classes; a true total zero is silent and is never resurrected by a class
default; tunnels still do not emit. At serving time OSM direction never scales
a count. Effective speed retains the shared posted, taper, country-legal and
class-default rules. Contributor and segment traces report the dominant
segment's four class values, the estimated bitmask
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
Overture-matched rows (`structures-builder-4`), so a canopy cannot acquire an
indoor attenuation from an absent or generic Overture class. These rows and
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

## 4.7 Vector screening

One source-to-receiver ray shares its bare-earth raster profile between terrain
and exact building/barrier crossings. The existing source-platform clamp and
source/receiver height floors apply to both. Buildings inside the source's
exclusion radius are omitted; explicit barriers are not. Paths shorter than
30 m or with fewer than three profile samples have no screening term.

Bare terrain retains its existing single max-path-difference edge. Every admitted
vector crossing is evaluated at its exact path fraction, with interpolated bare
ground plus its height. Each uses the same existing single-edge diffraction
function, bare-earth mean-ground fit, Rayleigh admission, favourable-condition
geometry, meteorological mixture and band caps.

For each frequency band, with terrain attenuation `T` and crossing attenuations
`C_j`, return `S = max(0, max_j(C_j) - T)`. Thus `T + S` is the band envelope,
not the sum of obstacle losses. The empty candidate set gives exactly `S = 0`.
Different crossings can supply different bands: selecting one maximum path
difference before evaluating attenuation is not equivalent. Adding a candidate
must not reduce any band's envelope. Raising a wall or building must not make
the receiver louder in the competing-roof regression.

Line-source angular integration is unchanged: interval rays use their own
terrain and crossings, and energy-average their ground-or-barrier composite.
The existing ground, vegetation, atmospheric and emission models are unchanged.

### Popup trace

The schema retains one real representative crossing: greatest incremental loss
in any band, then greatest path difference; exact ties retain input order.
Its position, height and path difference describe that crossing only. Other
crossings may supply other bands, and other rays may supply the line-source fan.
No positive increment means no representative edge. Scalar impact remains the
A-weighted difference between full and no-screening Lden, not this edge's loss.

### Model boundary

This envelope is Quiet Map's existing single-edge approximation applied to all
crossings, not a multiple-diffraction path construction. Full multiple-obstacle
geometry and split ground-reflection corrections are outside this change. The
normative context is [Directive 2021/1226, Annex II propagation amendments](https://eur-lex.europa.eu/eli/dir_del/2021/1226/oj/eng).
