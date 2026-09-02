# Noise Compute Engine — current production contract

This document is the contract for the noise-compute production model. It describes the
quantity painted by the engine, its standards basis, simplifications, source heights,
output-affecting constants, and the boundaries between shared code and generated data.
Executable coefficients and generated registries are authoritative in the linked source;
this document does not duplicate generated traffic tables.

The public product combines roads, railways, aircraft, industrial sources, buildings,
and leisure sources into broadband or eight-band noise levels. CPU, GPU, popup traces,
and reach calculations use the same source normalization and propagation definitions
where their source type supports them. A change to an output-affecting rule requires
the production profile, benchmark harness, and OUTPUT_VER to move together.

## 1. Scope, standards, and notation

The model follows these standards where they define a useful production quantity:

- ISO 9613-1 for atmospheric absorption; IEC 61672-1 for A-weighting.
- ISO 9613-2 and CNOSSOS-EU principles for outdoor source and ground propagation.
- ECAC Doc 29, including the segment SEL construction, for aircraft.
- The official CNOSSOS ground test cases are regression evidence, not a promise that
  every optional standard feature is enabled.

The product quantity is an energetic level in decibels. L is a band level, L_A is
A-weighted level, L_W is a sound-power level, L_W' is sound power per metre, and
SEL is sound-exposure level. Distances are metres unless stated otherwise; frequencies
are the eight octave-band centres 63, 125, 250, 500, 1000, 2000, 4000, and 8000 Hz.

For independent contributions, levels are summed as
  10 log10(sum(10^(L_j/10))).
A loss is subtracted from a source level. A positive reflection or ground gain is
added. All intermediate band computations use linear energy; rounding is display-only.

The source coordinate is the local tangent-plane position used by the tile. Terrain
heights come from the shared DEM and source/receiver heights are above the terrain
surface. The standard receiver height is 4 m. For aircraft, the Doc29 height and
slant geometry are used as described in section 6.

## 2. Production constants

The following constants affect painted output:

- BAND_FREQ = [63, 125, 250, 500, 1000, 2000, 4000, 8000] Hz.
- A_WEIGHTING = [-26.2, -16.1, -8.6, -3.2, 0, 1.2, 1, -1.1] dB.
- ALPHA_ATM = [0.1, 0.4, 1, 1.9, 3.7, 8.7, 22, 58.4] dB/km.
- ALPHA_VEG = [0.01, 0.015, 0.02, 0.025, 0.03, 0.04, 0.045, 0.06] dB/m.
- MAX_VEG_ATTEN = [2, 3, 4, 5, 6, 8, 9, 12] dB.
- SPEED_OF_SOUND = 340 m/s.
- PENUMBRA_DELTA_FLOOR_M = -SPEED_OF_SOUND / 63 / 20.
- DEFAULT_RECEIVER_HEIGHT = 4 m.
- P_FAV = 0.5 and FAVOURABLE_MIXING = true.
- FAV_RAY_CURVATURE_MIN_M = 1000 m and
  FAV_RAY_CURVATURE_PER_DSR = 8 m per metre of source-receiver distance.
- SINGLE_DIFF_CAP = 20 dB.
- GROUND_HARD_FLOOR_DB = -3 dB and GROUND_GAIN_UB_DB = 3 dB.
- BUILDING_DEFAULT_HEIGHT_M = 8 m, BUILDING_FLOOR_HEIGHT_M = 3 m,
  BUILDING_HEIGHT_MAX_M = 828 m, and ENCLOSURE_RADIUS_M = 75 m.
- Road V_REF_ROAD = 70 km/h and HEAVY_SPEED_CAP = 80 km/h.
- Road surface corrections for asphalt, sett, cobble, concrete, and gravel are
  [0, 4, 4, 1, 2] dB on the rolling component.
- ROAD_MAX_RADIUS by road class is
  [10000, 7000, 5000, 3000, 1600, 800, 400, 500, 300, 2000, 1200, 900, 600] m.
- RAILWAY_REACH_CLAMP_MIN = 2000 m, RAILWAY_REACH_CLAMP_MAX = 10000 m,
  RAILWAY_REACH_TARGET_LDEN = 25 dB, and RAILWAY_REACH_CEILING = 10000 m.
- Industrial non-wind point reach is 4000 m; building and leisure reach is 2000 m.
- Aircraft NPD reach is 16000 m; ground-operation reach is 5000 m for runway,
  3000 m for taxiway, and 1500 m for apron.
- Road source height is 0.05 m, rail source height is 0.5 m, and leisure source
  height is 1.5 m. Active industrial source heights are specified in section 7.
- Longitude metres-per-degree uses max(cos(latitude), 0.01), preventing a pole
  singularity while preserving the local tangent-plane geometry.

The complete coefficient arrays for road, rail, aircraft NPD, industrial, settlement,
and leisure profiles remain in their canonical modules. Constants here are the values
that define shared geometry, propagation, normalization, reach, and layer behaviour.

Canonical shared constants and formulas: [src/constants.rs](src/constants.rs),
[src/propagation/](src/propagation/), and [src/periods.rs](src/periods.rs).

## 3. Source normalization

### 3.1 Roads

Road categories are light vehicle, medium vehicle, heavy vehicle, and motorcycle.
For category i and speed v in km/h, the rolling and propulsion components are

  L_WR,i = A_R,i + B_R,i log10(v / 70)
  L_WP,i = A_P,i + B_P,i (v - 70) / 70

and the category power is
  L_W,i = 10 log10(10^(L_WR,i/10) + 10^(L_WP,i/10)).

The surface correction is applied to rolling noise. Traffic is converted from vehicles
per hour to line power per metre as

  L_W',i = L_W,i + 10 log10(Q_i / (1000 v)).

Categories are then summed in linear energy. Speed is clamped to 20–130 km/h and
heavy vehicles are capped at 80 km/h. A roundabout caps speed at 30 km/h. One-way
roads use the direction factor 0.5. Access codes 2 and 4 are dropped unless valid
measured traffic has positive light AADT; tunnels are always dropped.

The speed resolver applies the tagged speed, then the extracted taper, then the
country and built-up (zástavba, built_up) default, then the class fallback. A
maxspeed sentinel for derestricted roads resolves to 130 km/h. Traffic enrichment is
accepted only when its provenance and light AADT pass the enrichment predicate;
otherwise the measured/default cascade and lane ratio supply the category flow.

The time split is 65/20/15 percent for motorway classes 0, 1, 10, and 11, and
70/18/12 percent for urban roads. Access reductions are bypassed only for measured
provenance. These values are annualisation inputs, not a second propagation model.

Implementation and provenance:
[src/emission/road.rs](src/emission/road.rs),
[src/normalize/road.rs](src/normalize/road.rs),
[src/defaults.rs](src/defaults.rs),
[scripts/gen-country-defaults-rs.mjs](../../scripts/gen-country-defaults-rs.mjs), and
[scripts/gen-country-speed-defaults-rs.mjs](../../scripts/gen-country-speed-defaults-rs.mjs).

### 3.2 Railways

Rail categories are Rail, Tram, LightRail, NarrowGauge, and Funicular. The source
height is 0.5 m. For a vehicle category,

  L_vehicle,i =
    10 log10(10^((A_rolling,i + 30 log10(v / v_ref,i))/10)
              + 10^(A_traction,i/10)).

Line power is

  L_W',i = L_vehicle,i + 10 log10(Q_i / (T_h 1000 v)),

where T_h is the number of operating hours represented by the source record. The
rolling speed correction has a 30 dB-per-decade slope. Passenger and freight speeds
and their clamps are owned by railway.rs: passenger
reference/max 100/300 km/h, freight 80/120, tram 50/70, light rail 80/120;
narrow gauge follows light rail and funicular follows passenger.

The three day periods are 12, 4, and 8 hours. For EU ISO countries (the EU27 plus
CH, NO, and GB), freight uses the exact period weights
96.75/284, 32.25/284, and 155/284; passenger uses 0.7/0.2/0.1. Outside that
country set, freight uses 12/24, 4/24, 8/24 and passenger uses 0.7/0.2/0.1.
Tram, light rail, narrow gauge, and funicular use 0.7/0.25/0.05 everywhere.
Country is baked per row; unknown country uses the world split. Popup, loader,
and reach calculations call the same RailTimeDist period method.

Service tracks multiply counts by 0.02. Parallel tracks divide
the per-track count. Missing maxspeed uses the high-speed default where applicable;
the exact defaults and subtype handling are in the linked code and are not copied
here.

Rail reach is solved per row for free-field Lden 25 dB under the hard-ground reference,
then clamped to 2–10 km. The same reach is used by popup and heatmap.

Implementation:
[src/emission/railway.rs](src/emission/railway.rs),
[src/normalize/rail.rs](src/normalize/rail.rs), and
[src/compute/railways.rs](src/compute/railways.rs).

### 3.3 Area and point sources

Area sources are discretised by the shared area-source routine. A building footprint
larger than 2000 m² uses a 30 m grid; an industrial or leisure footprint larger than
5000 m² uses a 75 m grid. Each cell is sampled on a 5 by 5 subcell pattern. Cell
energy is area-weighted, and each point has an exclusion radius sqrt(cell_area / pi).
A centroid is used unless the grid produces at least two usable cells. Missing
industrial area is 10000 m², missing building area is 100 m², and leisure uses
its profile reference area.

Industrial profile lookup is four-digit NACE, then two-digit NACE, then subtype,
then source type. Profile constants and evidence comments are code-owned.

Buildings use their footprint and height ladder: explicit height, then floors times
3 m, then 8 m, clamped to 828 m. Missing floors are inferred with ceil(height / 3).
A containing footprint's actual height supplies the source height actual_height / 2.
Shed-like classes use footprint area; other classes use footprint area times floors.

Settlement and leisure emission laws and profile selection are specified in sections
7 and 8. Their normalized bands retain the exact broadband A-weighted level.

## 4. Shared propagation

The rules in this section are the shared definitions. Where the heatmap painters
deliberately depart from them for speed (ray cadence, the z12 point-layer stride, and
the production block partition), section 12 states each departure, its measured cost,
and where it is disabled.

### 4.1 Geometry and divergence

The shared slant distance is geo::slant_dist. Point-source divergence is

  A_div,point = 20 log10(d_slant) + 11 dB.

Line-source divergence uses

  A_div,line = 10 log10(2 pi d_slant) dB.

The line formula is paired with a finite-line correction. Horizontal distance for
screening and ground integration remains distinct from slant distance used for
divergence and atmospheric absorption. Distances below the geometric floors in the
canonical helpers are clamped before logarithms.

### 4.2 Terrain profile

PathProfile is the bilateral terrain/land-cover contract. It uses endpoint samples,
10 m probes near both ends, then 30/60/120 m spacing and 240 m spacing toward the
middle. DEM, forest, and impervious-surface arrays are sampled through this one
cadence. Source-platform samples used for diffraction and screening are clamped so
bare-earth samples within one source cell (about 30.7 m) cannot exceed the source
cell elevation. Raw ground and vegetation integrations retain their sampled values.

Implementation: [src/propagation/path_profile.rs](src/propagation/path_profile.rs).

### 4.3 Ground attenuation

The path ground factor is computed by path-integrating impervious surface:
G = 1 - IMD / 100. The source-end factor G' blends path G with the source-end
factor while d / (30 (z_s + z_r)) <= 1. Hard ground and bridges use G = 0.

For homogeneous and favourable states,

  A_H = max(H_analytic, -3 (1 - G'))
  A_F = max(F_analytic, -3 (1 - G')).

The production mixture is

  A_ground =
    -10 log10((1 - P_FAV) 10^(-A_H/10)
              + P_FAV 10^(-A_F/10)),

with P_FAV = 0.5. Favourable propagation uses the CNOSSOS ground alpha0
2e-4 and delta-z coefficient 6e-3, and applies the curvature defined in section
4.4. There is no per-period favourable probability.

Non-aircraft point sources and direct surface-line rays form a ray-specific
CNOSSOS GroundPath and use the full ground mixture. Arc and segment transport
reuse the characteristic point's GroundPath vector for their nested rays.
Aircraft ground operations intentionally retain the band-mean GROUND_CF
correction: [-1.5, -0.7, 1.5, 2.5, 2.0, 1.3, 0.7, 0.2] dB.

The ground implementation is in [src/propagation/path_effects.rs](src/propagation/path_effects.rs)
and [src/propagation/iso9613.rs](src/propagation/iso9613.rs).

### 4.4 Favourable curvature

For a source-receiver slant distance d_SR, favourable ray curvature is

  Gamma = max(1000 m, 8 d_SR).

The curved ray is used for the favourable delta and is mixed with the homogeneous
delta at P_FAV = 0.5. A curvature smaller than 1000 m is never used.

### 4.5 Finite line correction

For a segment, d_perp is the infinite-line perpendicular distance with a 0.5 m
floor. Signed along-line distances to the endpoints are d1 and d2. The subtended
angle is

  theta = atan(d1 / d_perp) + atan(d2 / d_perp)

with the signed difference form for a receiver beyond an endpoint. The correction is

  FLC = 10 log10(theta / pi) + 10 log10(d_div / d_perp).

d_div is the endpoint-clamped propagation distance. The correction is added to line
divergence and preserves energy as a segment tends to its infinite-line limit.

The geometry and conservation test are in [src/propagation/geo.rs](src/propagation/geo.rs).

### 4.6 Terrain diffraction

For each path, the engine considers the maximum signed delta among bare-earth samples
above the direct line of sight. It is a single-edge model. A blocked edge uses

  A_diff = min(20, 10 log10(3 + 20 delta f / c)) dB.

A near-miss unblocked edge uses

  A_diff = 10 log10(3 + 40 delta / lambda) dB,

down to delta = -lambda / 20, where the correction is zero. The Rayleigh 2021/1226
criterion is applied only to unblocked negative delta:
delta <= lambda / 4 - delta_star rejects diffraction. The same verdict is used for
homogeneous and favourable arms.

delta_star is formed by mirroring the profile over the mean-ground planes of the
unweighted optical line of sight. Bare earth is used for this verdict. Favourable
curvature changes the signed delta but not the edge selection. There is no lateral
diffraction and no CNOSSOS delta-ground split. A single diffraction correction is
capped at 20 dB.

Implementation: [src/propagation/diffraction.rs](src/propagation/diffraction.rs) and
[src/propagation/path_effects.rs](src/propagation/path_effects.rs).

### 4.7 Vector obstacles and barriers

Building screening is vector-only. Building footprints are polygon edges in
ObstacleSet; building footprints are read from this vector index. Barrier rows from
barriers.arrow carry endpoints and height and use the same exact segment-intersection
primitive. An untagged barrier defaults to 3 m. Obstacle kinds are Building and Barrier.

ObstacleSet indexes vector polygon edges. crossings and crossings_pruned use exact
segment_intersection_t, then sort candidates by the conservative lower-bound
distance. Missing obstacle shards fail the loader unless the .ingested-tiles
manifest proves that the shard is empty. This prevents absent data from silently
becoming clear terrain.

For a candidate obstacle, its signed delta competes with the terrain edge delta.
The terrain candidate is linearly interpolated between profile samples. A source's
own containing footprint is excluded within exclusion_radius. The screen is

  A_screen = max(0, A_combined - A_terrain).

Popup traces expose this incremental screen; composited maps use
A_terrain + A_screen exactly once.

Road line sources use sequential skyline admissions and then parallel profile/arc
evaluation with deterministic accumulation. The complete vector skyline is a
candidate/admission hint; every evaluated bucket recomputes exact ray crossings and
the terrain profile. The shipped arc-bounds policy has a 12 km safety radius, no
delta prune, and no output-dependent candidate cap. Segment buckets apply the
3 degree minimum-span gate. These are production policies, not caller options.

Canonical code:
[engine/tile-painter/src/source_loader_obstacle.rs](../tile-painter/src/source_loader_obstacle.rs),
[engine/tile-painter/src/source_loader_barrier.rs](../tile-painter/src/source_loader_barrier.rs),
[src/propagation/arc_screening.rs](src/propagation/arc_screening.rs), and
[src/propagation/seg_sampling.rs](src/propagation/seg_sampling.rs).

### 4.8 Vegetation

For each contiguous forest run, depth is the sum of segment length times forest
fraction / 100. Physical runs shorter than 10 m are discarded. Band loss is

  A_veg,i = min(MAX_VEG_ATTEN_i, ALPHA_VEG_i depth).

The forest array comes from the shared PathProfile, so popup and painted paths use
the same run segmentation.

### 4.9 Reflection and enclosure

ObstacleSet::enclosure_db probes a 3 by 3 receiver neighbourhood within
ENCLOSURE_RADIUS_M = 75 m. Only enclosing buildings above 5 m count. The receiver
reflection addition is +3 dB when occupancy is above 0.5, +1.5 dB when above 0.2,
otherwise 0 dB, with a +3 dB maximum. It is pre-baked by
bake_tile_vector_rx_refl and shared by CPU, GPU, and popup evaluation.

### 4.10 Received and A-weighted level

For each band,

  L_received =
    L_emission - A_div - A_atm - A_ground_or_barrier - A_veg
    + A_refl + FLC.

For a line source, FLC is present; for a point source it is zero. The obstacle term is
ground_or_barrier_db(ground, terrain, screen): max(ground, terrain + screen) when
there is a barrier, otherwise ground/terrain attenuation. A-weighting is

  L_A = 10 log10(sum_i 10^((L_received,i + A_WEIGHTING_i) / 10)).

### 4.11 Audibility, radii, and reach

The engine may reject a source before expensive path effects only when its conservative
free-field level is below the target. The point bound is 20 log10(d) + 11 plus the
conservative A-weighted atmospheric coefficient 0.002 dB/m. The line bound is
10 log10(d) + 8 plus the same coefficient. Path effects cannot make a source louder
except for the 3 dB ground headroom, which is included through GROUND_GAIN_UB_DB.

Road classes use ROAD_MAX_RADIUS. Rail uses the per-row reach solve. Industrial
points use 4 km, buildings and leisure 2 km, aircraft 16 km, and ground operations
use 5/3/1.5 km for runway/taxi/apron. These are admission bounds, not a replacement
for exact path evaluation.

## 5. Period aggregation and Lden

The three day periods are day, evening, and night. A source record stores its raw
count or energy and its period duration. The period conversion is

  period_level = 10 log10(total / (n_days seconds_in_period)).

The shared periods are 43200, 14400, and 28800 seconds. Day-boundary and IANA timezone
handling happen upstream before records reach the compute kernel. Evening has a 5 dB
penalty and night has a 10 dB penalty. The resulting periods combine as

  Lden = 10 log10(
    (12 10^(L_day/10)
     + 4 10^((L_evening + 5)/10)
     + 8 10^((L_night + 10)/10)) / 24
  ).

The implementation is [src/periods.rs](src/periods.rs). It is shared by roads, rail,
aircraft, industrial, settlement, and leisure sources; it is not valid to annualise
one layer with a second duration convention.

## 6. Aircraft

Aircraft has three production source families: airborne flight paths, airport ground
operations, and cruise paths. The layer and source-ID mapping are owned by the tile
store and aircraft compute modules.

### 6.1 Airborne Doc29 segments

Airborne emission follows Doc29 Eq. 4-8b:

  SEL_seg = L_E(P, d_p) + DeltaV + DeltaI(phi) - Lambda(beta, l) + DeltaF.

NPD profiles are generated from EASA ANP v2.3 by
scripts/build-aircraft-profiles.py and stored in
[src/emission/profiles_generated.rs](src/emission/profiles_generated.rs). NpdLuts
is the runtime registry.

CPA energy uses the infinite-line, unclamped geometry; display CPA is clamped to the
actual segment [0, 1]. DeltaV is 10 log10(v_ref / v) when speed is above 10.
Installation correction DeltaI is selected by wing, fuselage, or propeller. Lambda
applies only to wing installation and not to airport ground sources. DeltaF is the
exact finite-segment approximation.

The NPD distance reference and far-field threshold are 7620 m. Horizontal/slant
processing is capped at 16 km for production reach. A profile's NPD threshold is
40 dB and the kernel event floor is 20 dB. Current airborne filtering accepts only
the endpoint/altitude condition implemented by segment_filters: the segment must be
outside the endpoint and the extrapolated altitude must exceed 30 m under endpoint
terrain. Stage 2 stores endpoint terrain only; the removed quarter-, midpoint-, and
three-quarter-chord terrain check is not applied. Terrain cuts are precomputed for
accepted airborne paths.

Airborne sources use Doc29 geometry and NPD emission; surface terrain, vegetation, and
building screening are not applied to airborne broadband levels.

### 6.2 Airport ground operations

airport_traffic.arrow contains two kinds of event:

- Aircraft microsegments (veh_kind 0) store per-metre Z-weighted line power.
- Ground-support equipment (veh_kind 1) stores per-event SEL at 25 m.

Raw records are summed over n_days. The reader and painter convert them using
period_leq(total, n_days, period_seconds). The shared ground-operation geometries
are runway, taxiway, and apron. Their source height is 4 m and their reach radii
are 5 km, 3 km, and 1.5 km.

An aircraft line uses stored_lin times theta / d_perp. GSE uses stored_lin times
25 / d_endpoint as a point source. Both then use the shared path effects. Airport
area overlap uses a 50 m buffer. OSM runway/taxiway lines and apron areas map to
ops_kind 1, 2, and 3.

The reference anchors are generated as
GROUND_OPS_REFERENCE_LW_PER_METER_DB in profiles_generated.rs. Nominal speeds are
70, 18, and 12 knots for runway, taxiway, and apron; dwell adjustment is
-10 log10(v / v_nom), clamped to plus/minus 3 dB, and runway departure adds 2 dB.
The exact spectra are code-owned. GSE band SEL is

  SEL_band = Lw_band
    + 10 log10(atan(L / (2 r)) / (2 pi v r)), with r = 25 m.

### 6.3 Cruise

Cruise is evaluated with the same Doc29 segment kernel and production source registry
as its source class requires. Cruise paths are geometry-driven in the current
heatmap and popup implementations; they do not acquire surface screening by an
unused development switch.

Canonical aircraft code:
[src/emission/aircraft/doc29.rs](src/emission/aircraft/doc29.rs),
[src/emission/aircraft/npd/mod.rs](src/emission/aircraft/npd/mod.rs),
[src/emission/aircraft/segment_sel/mod.rs](src/emission/aircraft/segment_sel/mod.rs),
[src/emission/aircraft/segment_filters.rs](src/emission/aircraft/segment_filters.rs),
[src/emission/airport_traffic.rs](src/emission/airport_traffic.rs),
[src/emission/aircraft/ground_ops.rs](src/emission/aircraft/ground_ops.rs),
[src/compute/aircraft_v6/](src/compute/aircraft_v6/), and
[scripts/build-aircraft-profiles.py](../../scripts/build-aircraft-profiles.py).

## 7. Industrial sources

### 7.1 Point and area industrial sources

For an industrial source with broadband base Lw, profile spectrum S, and area A,

  Lw_source = base_lw + A_weighted_total(S)
               + 10 log10(clamp(A, 100, cap) / 10000).

The default cap is 500000 m². Heavy NACE 05, 08, 19, 20, 23, and 24, and OSM
subtypes 3–6, use 3000000 m². Power NACE 35 remains at the 500000 m² cap.
Bands are normalized as

  bands_i = Lw_source + S_i - A_weighted_total(S),

so their A-weighted sum is exactly Lw_source.

Quarry sources (source_type 1) use height 8 m. NACE 05, 08, 23, 24, and 35 use
height 10 m; other industrial sources use height 5 m. The wind hub source type is
separate. Non-wind point propagation uses the 4 km reach bound.

The profile chain is NACE four-digit, NACE two-digit, subtype, and source type.
Industrial profile values remain in
[src/emission/industrial.rs](src/emission/industrial.rs) and
[src/normalize/points.rs](src/normalize/points.rs), which are the source of truth.

### 7.2 Wind turbines

Wind is a point source with source_type 10. Turbine broadband Lw is selected by
rating: unknown or 0 kW 105 dB; below 1 MW 98 dB; 1–2 MW 104 dB; 2–3 MW 105 dB;
3–5 MW 106 dB; at least 5 MW 106.5 dB. Ratings above 8000 kW are treated as
unknown for this lookup. The normalized spectrum is
[-2, -1, 0, 1, 1, 0, -2, -5] dB with its A-weighted total removed. Hub height
defaults to 105 m and is clamped to 175 m. Default power is 2000 kW.

Implementation: [src/emission/wind.rs](src/emission/wind.rs).

## 8. Settlement and leisure

### 8.1 Buildings

Building classes 0–9 are the ordinary settlement classes; SILENT is class 10,
HOUSE class 11, FOOD_RETAIL class 12, and HOSPITALITY class 13. Shed-like classes
2, 5, 8, FOOD_RETAIL, and HOSPITALITY use footprint area. Other classes use
footprint area times floors.

Each class uses the code-owned fixed and per-area terms:

  Lw = 10 log10(10^(fixed / 10) + area 10^(per_m2 / 10)).

Its bands are normalized so their A-weighted sum equals Lw. A building source height
is actual_height / 2. The honest Lw-derived maximum distance is capped at 2 km.
SILENT is intentionally sub-audible and remains a source-class identity, not a
missing-data fallback.

Implementation and profile registry:
[src/emission/settlement.rs](src/emission/settlement.rs),
[src/lib.rs](src/lib.rs), and
[src/source_names.rs](src/source_names.rs), and [src/types/mod.rs](src/types/mod.rs).

### 8.2 Leisure

Leisure has eight source classes: pitch, padel, tennis, basketball, playground, pool,
outdoor seating, and stadium. It uses the same area law and normalized spectra,
without floors, with open source height 1.5 m and a 2 km reach bound. A polygon with
no usable cells uses the profile reference area. Leisure is rendered in the building
layer with LEISURE_TYPE_BASE = 100.

The profile constants and the evidence status of classes that need measured values
remain in [src/emission/leisure.rs](src/emission/leisure.rs); the renderer and
normalizer must not invent a second leisure registry.

## 9. Indoor display estimate

Indoor envelope is a current display feature. It is an estimate for enclosed cells,
not a regulatory indoor-noise map. The envelope class and facade reductions are:

- Outdoor: 0 dB.
- Residential: 30 dB.
- Commercial: 35 dB.
- Industrial: 20 dB.
- Historic: 28 dB.
- Default: 25 dB; at height 6 m or below, its effective class is Industrial (20 dB).

Building footprints and containing heights come from the same vector obstacle set.
The loader computes an InteriorEstimate per tile. Within a 512 by 512 tile, the exact
integer Felzenszwalb–Huttenlocher nearest outdoor donor is selected with deterministic
tie-breaking. The estimate applies max(0, L_facade - envelope_delta) only to enclosed
cells and their donors; outdoor cells are unchanged. If no donor exists, the value is
NO_DATA. It is applied after collapse and area fill to all supported layers by the CPU,
GPU-host, and aircraft runners.

Implementation:
[src/envelope.rs](src/envelope.rs) and
[engine/tile-painter/src/source_loader_obstacle.rs](../tile-painter/src/source_loader_obstacle.rs).

## 10. Generated data, provenance, and ownership

Generated traffic defaults and generated aircraft profiles are checked-in build
products with provenance in their generating scripts. They are not reproduced as
tables here.

- Country road defaults are generated by
  scripts/gen-country-defaults-rs.mjs and scripts/gen-country-speed-defaults-rs.mjs;
  runtime selection is in src/defaults.rs.
- Rail defaults and periods are in src/emission/railway.rs and
  src/normalize/rail.rs; the country on each row selects the period split.
- Aircraft NPD and ground-operation anchors are generated by
  scripts/build-aircraft-profiles.py and stored in
  src/emission/profiles_generated.rs.
- Industrial, settlement, leisure, and wind profile registries are maintained in
  their emission modules and normalized by the linked source code.

A source_id is the provenience of an input value. Measured, extracted, inferred,
and default values must remain distinguishable through normalization. A hash
verifies bytes only against an independently anchored expected identity; no file
proves itself.

## 11. Verification and canonical tests

These tests are the executable acceptance points for the contract:

- Road source vectors K1 and K2 in src/emission/road.rs (79.11 and 80.07 dB).
- The single-edge diffraction vector in src/propagation/diffraction.rs (about
  15.28 dB at 1 kHz).
- Period aggregation K8 in src/periods.rs (Lden 60 dB).
- Official CNOSSOS ground cases TC01, TC02, and TC03 in tests/tc_ground.rs.
- Finite-line energy conservation in src/propagation/geo.rs.
- A-weighted spectrum normalization in src/emission/spectrum.rs.
- Vector obstacle and clear-terrain cases in src/propagation/path_effects.rs.
- Exact barrier screening in ../../tile-painter/tests/barrier_screening.rs.
- Indoor envelope height matrix in src/envelope.rs.

The production path must preserve deterministic accumulation, the shared PathProfile
cadence, exact vector intersections, source provenance, and the single annualisation
contract. Any future model proposal belongs in the private future-plans documentation
until it is implemented, tested, and promoted into this contract.

## 12. Heatmap painter approximations

Section 1 says the tile and the popup share source normalization and propagation
definitions. Three shipped rules depart from that on the tile side only. Each is
recorded here with what it changes, what it was measured to cost, and where it is off.
None of them touches the popup.

### 12.1 The coarse-middle surface ray cadence

The CPU surface painter marches each source-receiver terrain ray at a reduced cadence in
the deep middle of a long ray. Beyond the near-end zones the deep-middle ray is stepped
at 3x the 245 m coarse step (about 737 m): `SHADOW_MID_STRIDE = 3`
([engine/tile-painter/src/scatter_band.rs:258](../tile-painter/src/scatter_band.rs)).
The dense 10/30/60/120 m bilateral ramp is kept within 600 m of each endpoint
(`SHADOW_SRC_ZONE_M` / `SHADOW_RX_ZONE_M`, `scatter_band.rs:270-271`), which is where
berms and near-receiver walls make the shadow sharp.

Short rays are exempt structurally. At or below `EXACT_CADENCE_MAX_DIST_M = 400 m`
(`scatter_band.rs:319`) the tile samples exactly where the popup samples
(`cadence_for_ray`, `:336-338`): the near field is where a screening error lives and
where tile and popup are compared point for point, so the approximation is confined to
the long rays it was measured on.

`coarse_mid_cfg()` (`scatter_band.rs:277-303`) resolves the configuration once per
process. `SURFACE_SHADOW_STRIDE` overrides the stride and the value `1` disables the
rule entirely (`None` = the exact cadence); `SURFACE_SHADOW_SRC_ZONE_M` and
`SURFACE_SHADOW_RX_ZONE_M` move the zones. One configuration feeds all three surface
kernels, line (`scatter_line.rs:92`), point (`scatter_point.rs:84`, `:128`) and airport
ground operations (`scatter_band.rs:1012`), through the single profile builder
`build_surface_profile` (`scatter_band.rs:345`), so the cadence cannot differ between
them.

Measured provenance of the two constants is in the code (`scatter_band.rs:249-257`,
`:262-271`): with the 600 m zones, stride 3 against stride 2 buys 11 points more
deep-middle reduction at essentially the same error (exceeding on at most 4.5 % of
cells, DEV p99 at most 0.8 dB against the method's own 2.6-5.2 dB raster-phase noise
floor), while a 200 m zone exceeded that floor on 20-38 % of cells. The near-field
guarantee is pinned by `near_field_cadence_matches_the_popup_beyond_it_stays_coarse`
(`scatter_band.rs:2298`).

The exact z13 reference tiles were painted with `SURFACE_SHADOW_STRIDE=1`, so the
reference the accuracy contract scores against carries the exact cadence while the
shipped CPU painter does not.

### 12.2 Point-layer adaptive stride at z12

[engine/tile-painter/src/point_w1.rs](../tile-painter/src/point_w1.rs) reconstructs the
industrial and building layers from a sparse receiver set instead of painting every
pixel exactly. It renders a direct-local surrogate over the whole tile, computes exact
physics only at a stride-5 anchor lattice (`STRIDE = 5`, `point_w1.rs:25`), derives a
whole-block refinement mask from anchor tri-state, raw-anchor residual range and
surrogate-predicted numeric tri-state, then computes the selected blocks exactly
(`point_w1.rs:1-11`, `render` at `:81`). Most pixels therefore never run a terrain or
obstacle query: they carry the surrogate plus the anchor residual.

It is opt-in and zoom-fenced. The switches are `QM_W1_INDUSTRIAL_POLICY` and
`QM_W1_BUILDING_POLICY`, each accepted only with the value `adaptive-stride5`
(`point_w1.rs:36-41`), and the policy is structurally restricted to zoom 12
(`policy_applies_at_zoom`, `:44-46`; `enabled_for_zoom`, `:51-53`). Every other zoom,
the line layers, and the popup keep the exact path.

Where it is accepted. The serving contract publishes two quality profiles: the z12 base
`w1-z12-accepted-v1`, whose numerical environment is exactly these two switches
(`W1_ACCEPTED_NUMERICAL_ENVIRONMENT`,
[server/src/generation-contract.mjs:41-44](../../server/src/generation-contract.mjs)),
and the z13 tier `w2-z13-spatial-v1`, whose numerical environment must be empty
(`validateNamedQualitySemantics`, `:245-247`). A z13 generation therefore cannot be
painted with this policy, and the engine gate agrees with the contract because the
module refuses every zoom but 12.

One contradiction is worth naming rather than resolving here. The serving contract
accepts both switches on the z12 base, and the module header states that both point
layers pass their drift contracts with the policy, building at 0.000 % on every
amplitude rung (`point_w1.rs:2-6`); the gate's own comment inside the same module says
building "is ported but not yet accepted" and that its switch "stays unlisted"
(`:33-35`), which its own code contradicts, since `policy_enabled` maps `building` to
`QM_W1_BUILDING_POLICY` (`:36-41`). The code and the serving contract agree with each
other: both switches exist, both are z12-only, and that comment is the outlier.

### 12.3 Relevant-source block partition

[engine/relevant-source-gpu](../relevant-source-gpu) is the production GPU painter for
the five surface layers, road, rail, industrial, building and airport ground operations
(`src/relevant_source_runner.rs:71-73`). Its approximation is one rule: per 16-pixel
block, evaluate the sources that matter at every pixel exactly, and carry the rest as a
smooth background.

**Blocks and corners.** A tile is partitioned into fixed 16x16-pixel blocks
(`BLOCK_PIXEL_SIDE = 16`, `src/source_frame.rs:21`), so a 512 px tile holds 32x32 = 1024
blocks over a shared lattice of 33x33 = 1089 corners (`source_frame.rs:23-26`); one CUDA
thread block is one block of 256 threads (`kernels/block_source_partition.cu:182-183`).
Each corner's per-source, per-period energy is evaluated on the card first
(`evaluate_corner_source_pairs_kernel`, `block_source_partition.cu:7-32`).

**Admission.** A block always keeps every source in its own 3x3 block neighbourhood
exactly, unconditionally and outside the budget (`src/relevance_partition.rs:139-141`;
the neighbourhood is built in `src/tile_source_incidence.rs:85-107`, `:189-232`, and at a
tile edge uses the adjacent tile's exact block edges, `:60-75`). Beyond that, each of the
block's four corners admits its remaining sources in descending Lden-weighted energy
order (`relevance_partition.rs:204-233`) until the energy it has NOT admitted is at most
`DROP_BUDGET_FRACTION = 0.15` (`:47`) of the SMALLEST of the block's four corner totals
(`:142-147`, admission loop `:148-165`). The retained set is the union over the four
corners plus the neighbourhood set.

The budget is measured against the block's quietest corner rather than each corner's own
total because the damage is done at the quietest pixel: source-weighted over the four
benchmark cells, a rail block's four corner totals stand a median 1.5 dB apart but
10.0 dB at the 95th percentile, so 15 % of the loud corner's energy can exceed the whole
answer at the quiet one (`relevance_partition.rs:25-31`).

**The dropped tail.** For each corner and period, the energy of everything not retained
is accumulated as one linear-energy constant, floored at zero
(`relevance_partition.rs:167-182`, field doc `:55-65`). The paint kernel seeds every
pixel's accumulator by blending those four corner constants bilinearly at the pixel
centre, lerp in x along the top and bottom corner pairs, then lerp in y
(`block_source_partition.cu:53-70`), and then adds each retained source evaluated at
that pixel's own receiver position, altitude and reflection (`:71-81`). Only the tail is
approximated; the retained physics is the exact kernel.

Two rejected alternatives are recorded with their cost: refining the background lattice
to 8 pixels still leaves 2605 of rail's 4994 cells over 3 dB and costs +50 GPU s of a
237 s wave, and giving every pixel its block's quietest corner leaves 1867 against a
limit of 1379 (`relevance_partition.rs:58-64`). The blend was therefore left alone and
the budget made to keep the dropped tail below the quietest answer in the block.

**Cadence by zoom.** The painter reproduces each wave's cadence: z12 runs the surface
heatmap's coarse middle of section 12.1 and z13 runs the exact popup cadence
(`coarse_middle_cadence`, `src/relevant_source_runner.rs:49`, `:53-59`; applied at
`kernels/relevant_source_path.cuh:127-128`, `:163-165`). Airport ground-operation
sources keep the exact cadence at both zooms (`kernels/relevant_source_pair.cuh:126`).
The CUDA constants are generated from the CPU constants rather than re-declared, with a
build-time assertion that the generated stride is 3
(`relevant_source_build.rs:279-299`, `:480`). Any zoom other than 12 or 13 is refused
(`src/bin/relevant_source_surface.rs:32-37`).

**Measured contract status.** On r9950 (RTX 5070), the four benchmark cells, five
surface layers in one process, with seconds and drift read from the same run
(`relevance_partition.rs:33-46`): at fraction 0.15 the wave costs 293.8 GPU s (331.0 s
wall) and rail lands 1101 cells over 3 dB against a limit of 1379 with industrial at 48
against 921, both pass; at 0.20 rail is 1.3x over the limit and at 0.30 it is 2.3x
over. Every other rung of every layer passes at all three fractions. The rule's own
effect at 0.15 was rail 4994 -> 1101 and industrial 1984 -> 48, bought with +23.7 % of
the wave's GPU seconds against the per-corner budget it replaced (which is why the 237 s
wave quoted for the rejected alternatives above is the pre-budget wave, not this one).
The owner chose 0.15 on 2026-09-02 as the only measured value that meets the whole
accuracy contract.
