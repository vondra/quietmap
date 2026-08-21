# Noise Compute Engine — Specification

Engineering formulas inspired by CNOSSOS-EU 2021/1226, ISO 9613-2:2024, and ECAC Doc 29 4th Edition. This is NOT a certified implementation of any standard. Simplifications are documented in each section.

**Purpose**: Global noise atlas for public information ("where do I hear noise"). Not regulatory END mapping.

*Last verified against code: 2026-06-10 (whole-file audit of `noise-compute`, `source-reader`, `aircraft-extract`, `raster-reader`); targeted re-sync 2026-07-03 (C1 rail split · per-row rail reach · settlement v2/v3 + leisure · GA 365-day hybrid · C2 airborne horizon · 15 aircraft classes).*

## Constants

### Receiver
- **Height**: 4.0 m (END standard facade height)
- **Temperature**: 15 °C
- **Humidity**: 70% RH
- **Pressure**: 101.325 kPa

### Octave bands
8 bands: 63, 125, 250, 500, 1000, 2000, 4000, 8000 Hz

### A-weighting (IEC 61672-1)
```
A = [-26.2, -16.1, -8.6, -3.2, 0.0, 1.2, 1.0, -1.1] dB
```

### Atmospheric absorption (ISO 9613-1, 15°C 70%RH)
```
α_atm = [0.1, 0.4, 1.0, 1.9, 3.7, 8.7, 22.0, 58.4] dB/km
```

### Vegetation attenuation (ISO 9613-2:2024 Annex A.2.2 × 0.5 Central Europe calibration)
```
α_veg = [0.01, 0.015, 0.02, 0.025, 0.03, 0.04, 0.045, 0.06] dB/m
max = [2, 3, 4, 5, 6, 8, 9, 12] dB per band
```
Reason for × 0.5: ESA WorldCover class 10 covers canopy ≥ 10 %; ISO A.2.2 calibrated for dense
foliage in full leaf. Scalar approximates average Central European mixed forest canopy density
(~50 %). See `docs/future-plans/forest-continuous-density.md` for the continuous-density plan
(Copernicus HRL TCD + Hansen GFC) that replaces the scalar with per-pixel canopy fraction.

### Aircraft ground-operations compatibility factors
```
CF = [-1.5, -0.7, 1.5, 2.5, 2.0, 1.3, 0.7, 0.2]
A_ground,aircraft[i] = max(CF[i] × G, 0) − 3 × (1 − G)    where G = 1 - IMD/100 (imperviousness raster)
```
This is the explicitly carved-out, byte-stable aircraft ground-operations
formation. Surface line and point sources use the literal per-band CNOSSOS
core in §3.3 instead.

---

## 1. Road Emission (CNOSSOS-EU Annex II)

### Source height
h_s = 0.05 m

### Vehicle categories
| Category | Code | Description | Speed cap |
|----------|------|-------------|-----------|
| 1 | cat1 | Light vehicles (cars, vans) | — |
| 2 | cat2 | Medium heavy (delivery trucks) | — |
| 3 | cat3 | Heavy (HGV, buses) | 80 km/h |
| 4b | cat4b | Motorcycles | — |

Note: Category 4a (mopeds) and 5 (open category) not implemented. Known simplification.

Emission speed is clamped to **[20, 130] km/h** before the rolling/propulsion formulas (`road.rs`); cat3 additionally capped at 80 km/h per the table above.

OSM `maxspeed` is parsed unit-aware at extract (`osm-extract::classify::parse_maxspeed_kmh`: first `;`-token; `mph`/`knots`/`walk`; numeric clamp ≤ 400; `signals`/garbage → 0). The roads column stays u8: `maxspeed=none` (derestricted) stores sentinel **255** (`SPEED_LIMIT_DERESTRICTED`), real limits clamp to 254; `normalize_road` resolves the sentinel to **130 km/h** (`DERESTRICTED_SPEED_KMH` — BASt 2025 measured 124.1 km/h mean on derestricted Autobahn; CNOSSOS validity cap 130).

**Untagged maxspeed (0)** first passes through the R7 `speed_taper` (a graded
effective speed from the road class and geometry, `normalize/road.rs`), then
resolves through the country's LEGAL implicit limit before the world table (`defaults.rs::resolve_speed_default`, table generated from the OSM-wiki legal-defaults dataset — `scripts/gen-country-speed-defaults-rs.mjs`): class 0 → motorway, 1 → motorroad-else-rural, 2/3/4/9 → urban/rural by the `built_up` roads.arrow column (building-raster sample at the segment midpoint; 0 = unknown → skip to the world table, never guessed rural). Local classes 5-8 and links 10-12 stay on the world table by design — a national urban limit would overstate them by +20-30 km/h (/gg 2026-07-03). Rationale: one global default (50) painted a ±5-6 dB colour seam at every tagged/untagged boundary mid-road (Wetherby A168 case, task #15). **Country comes from the segment itself wherever the M3 bake has run** (`country_iso`/`city_id`/`continent` columns): absent columns keep the old receiver-country approximation at borders; a baked `00` resolves `Admin::UNKNOWN` (WORLD), never the receiver's country.

### Rolling noise per band (CNOSSOS-EU §2.4.6)
```
L_WR,i = A_R,i + B_R,i × log₁₀(v / v_ref)
```
where v_ref = 70 km/h. Coefficients A_R, B_R from CNOSSOS-EU Annex II Table 2.3.a (2021/1226 consolidation).

### Propulsion noise per band
```
L_WP,i = A_P,i + B_P,i × (v - v_ref) / v_ref
```
Coefficients A_P, B_P from CNOSSOS-EU Table 2.3.b.

### Surface correction (CNOSSOS-EU §2.4.8)
```
L_WR,i += ΔL_WR    (same scalar applied to rolling noise only in all bands)
```
| Surface | ΔL_WR |
|---------|-------|
| asphalt (default) | 0 dB |
| sett/cobblestone | +4 dB |
| concrete | +1 dB |
| gravel/unpaved | +2 dB |

### Combined emission per band
```
L_W,i = 10 × log₁₀(10^(L_WR,i/10) + 10^(L_WP,i/10))
```

### Line source power density per meter
```
L_W'/m,i = L_W,i + 10 × log₁₀(Q / (1000 × v))
```
where Q = vehicles/hour, v = speed in km/h. The `1/(1000·v)` term converts flow to vehicle density per meter.

**Simplification**: ISO 9613-2 is point-source only — it requires subdividing line sources into representative point sources. We use a line-source approximation with finite-line correction (FLC), which is standard practice in noise mapping software (NoiseModelling, LIMA, SoundPLAN) though not literally ISO 9613-2.

### Total emission (all categories)
```
L_W_total,i = 10 × log₁₀(Σ_cat 10^(L_W'/m,cat,i / 10))
```

### Day/evening/night split

Fixed per-class: 65/20/15 % for motorway + trunk + their ramps (classes
0/1/10/11), 70/18/12 % otherwise. Applied even on measured AADT — sub-daily census not currently sourced.

`access_factor` reductions are bypassed only when `Provenance::is_measured()` (City/National/Continental/GlobalMeasured); NationalProxy, Heuristic and Baseline rows still get access reductions — a national proxy is a class-default estimate, not a measurement, so it must be down-scaled on restricted-access roads.

A row counts as enriched only when `provenance.has_data() && aadt_light > 0` (`normalize::has_enriched_traffic`) — an enriched row with zero light but nonzero heavy AADT falls back to full class defaults (known edge case). Un-enriched rows with ≥ 3 lanes get a `lane_ratio` default multiplier (ŘSD-calibrated bucket medians, `normalize::lane_ratio`): motorway oneway 3 lanes ×1.42; primary 3/4 lanes ×1.37/×2.13; secondary 3 lanes ×1.83; all other buckets ×1.0. Measured rows bypass it.

---

## 2. Railway Emission (CNOSSOS-EU Annex IV / RMR)

### Source height
h_s = 0.5 m (wheel-rail contact)

### Emission per band
```
L_vehicle,i = 10 × log₁₀(10^((A_rolling,i + 30 × log₁₀(v / v_ref))/10) + 10^(A_traction,i / 10))
L_W'/m,i   = L_vehicle,i + 10 × log₁₀(Q / (T_h × 1000 × v))
```
where:
- A_rolling / A_traction: entire-train A-weighted reference spectrum per vehicle type, peaked at 500–1000 Hz (ISO 3095 / CNOSSOS rail spectrum)
- v in km/h, clamped to `[20, v_max]`
- v_ref per vehicle type (see Rail vehicle types below)
- Q = trains **in the period** after the per-region day/evening/night split of the daily count (see "Day/evening/night split" below)
- T_h = period hours: 12 day / 4 evening / 8 night
- B_rolling = 30 (speed-dependent rolling noise exponent)

This is the CNOSSOS Annex IV line-source density (NoiseModelling-compatible).

**Known issue / history**: a prior revision used `L_W = L_vehicle + 10·log₁₀(Q_per_day)` with SRM-II-style coefficients peaked at 4 kHz. Because 4 kHz carries ~22 dB/km atmospheric absorption, rail signal collapsed at range. Current coefficients are calibrated so a typical mainline corridor matches EU END reference levels in the 0–5 km range. See the header comment in `src/emission/railway.rs`.

### Rail vehicle types
| `rail_type` | Enum | v_ref (km/h) | v_max (km/h) | Coefficient table |
|-------------|------|--------------|--------------|-------------------|
| 0 | Rail (mixed pax + freight) | 100 (pax) / 80 (frt) | 300 / 120 | PASSENGER + FREIGHT |
| 1 | Tram | 50 | 70 | TRAM |
| 2 | LightRail | 80 | 120 | LIGHT_RAIL |
| 3 | NarrowGauge | 80 | 120 | LIGHT_RAIL (reused) |
| 4 | Funicular | 100 | 300 | PASSENGER (fallback) |

High-speed passenger (`v > 200 km/h`) is served by the passenger rolling spectrum scaled via `30·log₁₀(v/v_ref)` — not a dedicated aerodynamic model.

### Day/evening/night split (C1 — per-region, per-category)

`rail_time_dist(admin, rail_type)` (`emission/railway.rs::RailTimeDist`)
replaced the old flat 65/20/15 that was applied to passenger and freight
alike — the cause of rail `L_night` always being exactly `Lden − 7.91 dB`
(audit rail-report §G.4). Shares of the daily count (END periods 12/4/8 h):

| Region × category | day | evening | night |
|---|---|---|---|
| EU freight | 0.3407 | 0.1136 | **0.5458** |
| EU passenger | 0.70 | 0.20 | 0.10 |
| Tram / light-rail / narrow-gauge / funicular (all regions) | 0.70 | 0.25 | 0.05 |
| Non-EU freight (continuous 24/7, uniform) | 0.50 | 0.1667 | 0.3333 |
| Non-EU passenger | 0.70 | 0.20 | 0.10 |

EU freight is measured-derived from EP IPOL-TRAN ET(2012)474533 Table 22
(Rheintalbahn 129 day-trains / 155 night-trains), corroborated by EBA
Lärm-Monitoring 2023; "EU" is a 30-country ISO whitelist (EU27 + CH/NO/GB)
keyed on country code — NOT geographic Europe. The ISO comes from the
segment's own baked `country_iso` when present (M3/M5), else from the
process-wide `h3r4-admin.bin` table (`admin.rs::admin_for_latlng`); a baked
`00` resolves non-EU (WORLD split), never the receiver's country. All
three consumers — popup kernel (`compute/railways.rs`), heatmap loader
(`normalize/rail.rs`), and the reach solver — iterate one shared
`RailTimeDist::periods()`, so the share model cannot fork.

Post-adjustments applied even on measured counts: `service > 0` → counts × **0.02**; `parallel_divisor > 1` → counts divided by that factor.

### Defaults when un-enriched (`railway.rs::default_traffic` / `default_speed`)

| `rail_type` / usage | trains/day (pax + freight) | default speed |
|---|---|---|
| Rail, main line | 80 + 20 | 80 km/h |
| Rail, branch | 30 + 5 | 80 km/h |
| Rail, industrial siding | 0 + 15 | 80 km/h |
| Rail, unknown usage | 40 + 10 | 80 km/h |
| Tram | 120 + 0 | 25 km/h |
| LightRail | 80 + 0 | 60 km/h |
| NarrowGauge | 10 + 0 | 40 km/h |
| Funicular | 40 + 0 | 20 km/h |

`maxspeed` tag wins when present (unit-aware parse at extract — mph postings like WCML "125 mph" now convert instead of dropping to 0); missing `maxspeed` with `highspeed=yes` → 300 km/h (`normalize::normalize_rail`). The railways `maxspeed` column is **UInt16** since 2026-06 (300+ km/h overflowed u8); readers (`hex_store::col_u16_or_u8`, `source_loader_rail`) also accept legacy UInt8 arrows until the next world OSM re-extract.

---

## 3. Propagation (ISO 9613-2)

### 3.1 Geometric divergence

**Line source (V1 legacy; retired by the V2 evaluator in §3.5f.3)**:
```
A_div,i = 10 × log₁₀(2π × d_slant)
```

**Point source**:
```
A_div,i = 20 × log₁₀(d_slant) + 11
```

where d_slant = √(d_horizontal² + Δh²), Δh = (h_source + z_source) - (h_receiver + z_receiver)

### 3.2 Atmospheric absorption
```
A_atm,i = α_atm,i × d_slant / 1000    [dB]
```

### 3.3 Ground effect (CNOSSOS-EU §2.5.14--2.5.20)
```
A_ground,H,i = max(analytic(w_i, cf_i, f_i, d_p, z_s, z_r), −3 × (1 − G′_path))
A_ground,F,i = max(analytic(w_i, cf_i, f_i, d_p, z_s,F, z_r,F), −3 × (1 − G′_path))
A_ground,i   = −10 × log10((1−P_FAV)×10^(−A_ground,H,i/10) + P_FAV×10^(−A_ground,F,i/10))
P_FAV = 0.5
```
`w_i` is §2.5.17, `cf_i` §2.5.16, the homogeneous state §2.5.15 and the
favourable state §2.5.20. `G′_path` is §2.5.14's source-end correction: for
`d_p/(30(z_s+z_r)) ≤ 1`, it blends path-average `G_path` with the source IMD
factor; otherwise it equals `G_path`. `d_p` is horizontal distance. `z_s` and
`z_r` are measured against the one bare-earth mean-ground OLS plane shared
with the diffraction `δ*` machinery. `G_path = 1 − IMD/100` is sampled over
the ray for line, area, and point sources alike; bridge paths force both
factors to zero.

2015/996 states the hard-ground case verbatim: *"if Gpath = 0: Aground,H = −3 dB"*. ISO 9613-2 Table 3 agrees
(`As + Ar = −1.5 − 1.5`). The physics is the image source: over a reflective
surface the direct and reflected rays arrive in phase, so the level sits ~3 dB
ABOVE free field — an attenuation of −3 dB, not zero.

The lower bound is a replacement, not an addend. Surface sources form the
literal vector once in `propagation::iso9613::ground_atten_bands` (with
`ground_atten_db` as its per-band scalar); the
CPU popup/tile kernels and `scatter.cu` use that vector. CUDA receives the
Rust-owned physical constants only through `noise-gpu/build.rs -D`; no ground
constant is hand-maintained in the kernel. The official TC01/TC02/TC03 fixtures
pin the homogeneous direct state band-by-band in `tests/tc_ground.rs`.

Aircraft ground operations intentionally keep their own band-mean `CF` function
above: they have no independent ground-core validation lane and must remain
byte-stable until one exists.

**Transitional line-fan cause (V1 review):** the existing arc increment
transport applies the characteristic-point literal ground vector to its fan
subrays; node evaluation replaces that transport with each ray's complete
ground/barrier composite. This standalone core does not change the increment
payload or its non-negative clamp.

Consequence for the energy-budget skip in `tile-painter`: the largest ground
GAIN any band can reach is `GROUND_GAIN_UB_DB = 3.0 dB`, attained by the
`-3*(1-G_prime)` floor at `G_prime = 0` in every band, replacing the old
per-band `max(−CF[i], 0)` (1.5 / 0.7 / 0…). The bound is
`constants::GROUND_GAIN_UB_DB`; leaving the old values in place would
make the skip's `ub ≥ exact` invariant false over hard ground and drop audible
sources silently.

Water is hard (ISO 9613-2 §7.3.1 groups water with paving/concrete): the
WorldCover→IMD LUT maps water to 100; snow/ice stays 0 (porous snow cover →
soft). A missing IMD tile defaults to 100 — the converted set has a tile for
every land tile, so an absent tile is open ocean. Partial-tree caveat: a dev
box may carry only a subset (e.g. 34–59°N plus synced Scandinavia), where
missing northern LAND reads hard too; the production host's complete raster
tree is the truth.

Current implementation:
- **Surface line and point sources**: popup and pipeline use the same
  **path-averaged** `G_path` from source/closest point to receiver, plus the
  §2.5.14 source-end factor.
- **Aircraft ground operations**: retain the isolated band-mean compatibility
  formation until their own validation lane exists.

Barrier interaction:
```
A_ground_or_barrier,i = max(A_ground,i, A_terrain,i + A_screen,i)   if barrier exists
                      = A_ground,i                                   otherwise
```
Ground and barrier attenuation are **not** added together.

### 3.4 Finite-line correction (V1 legacy line sources only)
Uses **HORIZONTAL** distance and angle subtended:
```
d1, d2 = along-segment horizontal offsets from the foot of the receiver's
         perpendicular to the two segment endpoints, SIGNED (d1 < 0 when the
         foot lies past the segment's start — the receiver is off the end)
d_perp = perpendicular horizontal distance from receiver to the segment's
         INFINITE line (unclamped foot), floored at 0.5 m
d_div  = the distance the divergence term was evaluated at (§3.1) — the
         endpoint-clamped `dist_m` the source loaders precompute

θ   = atan(d1/d_perp) + atan(d2/d_perp)              [radians]
FLC = 10 × log₁₀(θ / π) + 10 × log₁₀(d_div/d_perp)   [dB]
```
Note: Uses HORIZONTAL distances, not 3D slant. This is a fix from V33/V44 which incorrectly used 3D.

The exact free-field energy of a straight finite line is `∝ θ/d_perp`
(`∫dy/(d_perp² + y²)`), so `θ` and the divergence distance must be the same
`d_perp`; the second term re-references the divergence the kernel already took
at `d_div`, leaving atmospheric absorption on the true (endpoint) distance.
Both terms vanish when the receiver's perpendicular foot lies ON the segment
(`d_div == d_perp`) — the common case, and the only geometry the pre-2026-08-03
form (endpoint distance + clamped fraction) was valid for. Off the end it read
a 250 m segment up to +1.9 dB loud, ≈ +0.9 dB on a whole line
(`screening_fixture` scene B). Consequence: subdividing a microsegment now
conserves received energy exactly (`geo.rs::off_end_split_conserves_energy`).
The audibility cull keeps using the endpoint-clamped distance.

### 3.5 Terrain diffraction (ISO 9613-2 §7.3 + CNOSSOS-EU §2.5.6(c), single-edge)
```
δ = path_via_edge - direct_path    [m]

Single edge (§7.3):
A_bar,i = min(20, 10 × log₁₀(3 + 20 × δ × f[i] / 340))

Rayleigh criterion — UNBLOCKED RAYS ONLY
(Commission Delegated Directive (EU) 2021/1226 point (9)(c)):
if δ < 0 and δ ≤ λ/4 − δ*  then  A_bar,i = 0
```

**The `δ < 0` scope is the whole criterion, and it was missing until
2026-08-05.** The amendment's sentence opens "*If the direct ray is not
blocked*" and notes the path differences there are negative; ISO/TR 17534-4:2020
§5.9 states the agreed interpretation outright — "*If the line of sight is
blocked, diffraction is always calculated*" — and NoiseModelling's
`AttenuationCnossos.isValidRcrit` is the same predicate verbatim
(`pp.delta >= 0 || (pp.delta > -lambda/20 && pp.delta > lambda/4 - pp.deltaPrime)`).
Applying it to a blocked ray was this engine's own addition and made
`A_bar` a step function of obstacle height: the cut sits where the formula
already reads `10·lg(8 − 20δ*/λ)`, i.e. **9.03 dB at δ\* = 0 and 7.40 dB on
flat ground** (where `δ* = |δ|` exactly). Measured: a 200 m path, 0.05 m
source, 4 m receiver, wall at mid-path — 4.1229 m of wall gave 0 dB at 1 kHz
and 4.1249 m gave 7.48 dB. Obstacle heights are quantised, so that step drew
building outlines into the map. Continuity is now a live gate
(`diffraction::attenuation_is_continuous_in_obstacle_height`, sweeping a wall
from below the sight line past `δ = λ₆₃/4` against the analytic Lipschitz bound
of the band function).

MEASURED on 156 Prague receivers (popup engine, `queryNoiseAtPoint`, before vs
after): max |ΔLden| **1.81 dB**, 21 receivers (13.5 %) move more than 0.5 dB,
78 (50 %) more than 0.1 dB, 137 (87.8 %) move at all — and the direction is
**one-way quieter** (136 of 137; mean −0.24 dB). That is the attenuation the
gate was discarding behind low and mid-height obstacles: per band it restores
+1.8 to +8.1 dB in whichever bands sat below the old cut, and nothing at all
once `δ > λ₆₃/4 = 1.35 m` (a ~13.6 m wall at mid-path on 200 m), where the old
gate already passed every band.

The criterion is NOT decoration on the negative arm: without it a 0.5 m wall
1.5 m *below* the sight line screens 3.8 dB at 63 Hz, because on a 200 m path
no obstacle can reach `δ = −λ₆₃/20 = −0.27 m` at all and the whole penumbra
window stands open (`path_effects::far_below_candidate_stays_silent`).

ONE verdict per path, taken on the homogeneous δ and spent on both
meteorological states — a deviation from ISO/TR 17534-4 §5.9 ("*made separately
for homogeneous and favourable conditions*") forced by δ\* being
straight-geometry here: testing a curved δ_F against a straight δ\* gave the
favourable arm its own admission step at `δ_F = 0`, worth **3.13 dB across 2 mm
of wall height** at an arbitrary wall height. Revisit if δ\* ever goes
per-state.

**OPEN — the sight-line step.** A band with `δ* ≤ λ/4` is rejected at δ = 0⁻ and
takes the blocked branch's `10·lg 3 = 4.77 dB` at δ = 0⁺ (≤ 4.32 dB measured
after the (2.5.9) mix). That edge is CNOSSOS's own: its "otherwise" branch
swaps the two split mean ground planes for one common plane and returns
`Aground` instead (2.5.30–2.5.32), a switch §3.3's
`max(A_ground, A_terrain + A_screen)` does not implement. Bounded and pinned
(`diffraction::the_sight_line_step_is_the_standards_own_and_bounded`); closing
it means implementing the Δground split, not another gate.

**Edge selection (N = 1, single-edge model):** among profile samples above the
source→receiver line-of-sight, the edge with the **largest path-length
difference δ** wins (`propagation/horizon.rs::max_delta_idx`). Max-δ ranking
deliberately replaced LOS-excess ranking, which systematically under-weighted
barriers close to either endpoint. The multi-edge upper-convex-hull cascade
(double/triple Fresnel, C₃ thick-barrier term, 25 dB cap) was **removed
2026-06-01** in the single-edge δ rewrite (commits `efba2c1b`…`f2b526ce`); the
last unreachable double-edge remnant (`maekawa_bands` C₃ arm + 25 dB cap +
the `is_double`/`edge_distance` trace fields) was deleted 2026-07-03. The GPU kernel (`noise-gpu/kernels/scatter.cu`) mirrors the same
single-edge selection. The attenuation cap is therefore **20 dB in all cases**.

δ* is the path-length difference computed using the same dominant edge D but with mirror source S\* and mirror receiver R\* reflected **vertically** across their respective mean ground planes. Each mean ground plane is an unweighted least-squares line fit over the DEM profile samples on that side (including D itself). D is the single max-δ edge selected above.

**δ\* fits on bare-earth elevation only.** When the combined-screening entrypoint (`combined` terrain+building+barrier top profile) invokes diffraction, δ* continues to fit on `elevation_m` so the mean-ground planes represent the *ground reflection* surface that CNOSSOS §2.5.6(c) physically defines. Feeding building heights to the OLS fit would drag the mean-ground plane up to rooftops and silently break ground-reflection physics.

Simplifications vs. strict CNOSSOS:
- **Single edge only (N = 1)** — multiple diffraction (ISO §7.4 / CNOSSOS §2.5.23) removed 2026-06-01, see above.
- We use **vertical** reflection across the fitted plane (standard acoustic practice in NMPB / NoiseModelling), not perpendicular-to-plane.
- The **−λ/20** near-miss clause (penumbra) is implemented for VECTOR obstacle
  candidates: a crossing whose top sits below the line of sight keeps its
  geometry and takes the NEGATIVE path difference `δ = −(d_SO + d_OR − d_SR)`,
  and `maekawa_bands` evaluates the CNOSSOS §2.5.6(c) branch
  `10·log₁₀(3 + (40/λ)·δ)` down to `δ = −λ/20`, where it reaches exactly 0 dB
  and meets the ISO `20/λ` arm at `δ = 0` (both 10·log₁₀3 ≈ 4.8 dB). The δ\*
  Rayleigh criterion applies to this arm — that is the branch the 2021
  amendment wrote it for — so grazing geometry over flat ground stays
  owned by the ground term. Two deliberate scope limits: (a) sampled BARE-EARTH
  edges do not take the branch — every ground sample sits below the LOS, so a
  negative branch there would make flat terrain diffract and double-count the
  ground effect; (b) the near-miss δ_F takes the standard's own unblocked
  branch (2.5.27) — see §3.9.
- `Δground` additive combination (CNOSSOS §2.5.31) is not implemented — we still combine ground and barrier via `max(A_ground, A_terrain + A_screen)` in §3.3.
- Favourable-conditions curved rays ((2.5.24)) are implemented behind the
  `FAVOURABLE_MIXING` flag, live since 2026-07-28 — see §3.9.
- Lateral diffraction around vertical edges (§2.5.6(i)) is not implemented.

#### 3.5.1 Source platform clamp (2026-08-20)

**Within one DEM cell (`CELL_M` ≈ 30.7 m) of the source, bare-earth profile
samples may not exceed the source cell's own elevation.** The road body is not
a diffraction obstacle: CNOSSOS puts the point source 0.05 m above the ROAD
SURFACE (2021/1226 §2.2, "*this point source is placed 0,05 m above the road
surface*"), so the terrain term must integrate over the road platform, not over
the DEM's rendering of it. A 30 m DEM cannot resolve the bench: the cells
flanking a road mix the embankment crown with the falling terrain and read up
to **+1.3 m** above the road cell (measured on the D4 at Voznice, fine-probe
crest +0.98 m at 7.5 m from the centreline). On a downhill ray the steep
near-source sight line grazes that phantom hump and the terrain term flipped
0 ↔ −9.6 dB between receivers 25 m apart — razor-straight shadow wedges
radiating from every embankment road (owner verdict 2026-08-20; measured popup
grid: adjacent-receiver |Δterrain| max **8.5 dB**). The clamp removes the
phantom while keeping the genuine plateau-edge graze (the embankment-toe
shadow the DEM actually supports) and every obstacle beyond one cell.

Applied at the diffraction evaluation points only —
`path_profile::clamp_source_platform` inside `compute_terrain_diffraction`
(exact march + popup trace), the same clamp inside
`screening_attenuation_with_meta` (otherwise the phantom re-enters through the
composite as a spurious screening increment), and the read-time form
`source_platform_clamped` inside `terrain_subset_delta_lower_bound` (the M3b
bound stays sound: the rule is pointwise in (t, e) given the shared endpoint
e0, so subset-of-carved = carved-of-subset). Ground effect and vegetation keep
the raw profile (their integrals are blind to a sub-cell hump). The clamp is
idempotent and never raises a sample.

Explicit trade, recorded for review: a REAL feature rising within one cell of
the source (cut-slope toe, unmapped berm) no longer screens — at 30 m
resolution the source cell's own value is the only defensible platform, and
intra-cell geometry is unresolvable by construction. Mapped noise walls are
NOT affected (exact ray×segment barrier crossings with their own heights; the
wall's terrain base LERP takes the same clamped profile, so a wall and its
source stay on one consistent platform). Pin:
`path_effects::tests::phantom_shoulder_hump_is_carved_to_the_platform`,
`cut_slope_beyond_one_cell_still_screens`. Receiver-end edge, pinned by
`annulus_receiver_carve_cannot_fork_the_terrain_term`: on 30 ≤ dist < CELL_M
rays the receiver sample itself sits inside the zone, so an uphill receiver's
ground is carved while the march's `rcv_h` stays derived from the raw sample
(screening and the CUDA kernel derive it post-carve) — the value-level fork
is unobservable because the whole sub-cell ray carves flat, leaving no edge
under either height order. The GPU kernel
(`noise-gpu/kernels/scatter.cu`) grows the same clamp in the same landing
set (2026-08-20): `clamp_source_platform` carves the per-ray scratch before
the terrain march (the composite base and candidate LERPs inherit it, as on
the CPU), and `ray_path_bands` re-reads the raw near-source samples before
returning so the caller's ground mean-plane stays on the uncarved earth —
the same isolation as the CPU.

Alternative considered and rejected (Occam): denser near-source sampling
(fix B). It does not remove the phantom — the hump is a whole DEM cell — it
only makes every azimuth see it CONSISTENTLY: measured at the Voznice
transect the crest (+0.98 m at 7.5 m) would diffract uniformly at δ ≈ 0.21 m
→ −9.6…−20 dB across bands, i.e. stripes traded for a uniform phantom shadow
≈ +4…+7 dB over the clean azimuths (a new bias), plus cadence growth on every
ray and a GPU resync. The clamp fixes the physics; denser sampling entrenches
the artifact.

See §3.5b for the V2 node evaluator's shared path sampling scheme.
The V2 clauses below are the D1 review draft; they become the live contract
only with the V2 evaluator landing.  The demoted V1 text is retained solely as
an implementation/archive record until its verified deletion package lands.

### 3.5a Element decomposition and deterministic line-node placement (V2 draft)

Model V2 evaluates every source as incoherent **point-like emitting elements**
at the receiver.  It does not transport a characteristic-point path effect over
a line fan.  For a receiver `R` and emitting geometry `S`, the received energy
is the sum of the full transfer of its nodes:

```text
[N-01]  L(R) = 10 lg Σ_nodes 10^((E_node - A(node, R)) / 10)
```

The decomposition policy is selected once per source/receiver block, then its
nodes stream one at a time.

* **Point:** one node, preserving the present point-emitter semantics.
* **Area:** the existing `point_sources` grid is the area policy verbatim; each
  grid point is one node.  A far-field collapse is not part of V2.
* **Line:** roads and railways use the closed-form placement below for every
  storage piece and receiver.  Uniform angular buckets, adaptive fan
  requadrature and blocked-fraction averaging are not line policies in V2.

`D_FLOOR` is a pinned, per-layer physical constant: emitter height plus half
the lane/track width.  It is not an accuracy or capacity dial.  The
soundness note in §3.5f records the numerical values and their provenance.  Line
storage pieces satisfy `L <= L_MAX = 250 m`; an over-length input is split at
load, fail-closed if that invariant cannot be established.  The storage split
is an emission-attribution boundary, and a 1/2/N split-merge fixture pins
invariance.

The following labels are normative for both `node_eval.rs` and the CUDA mirror
under the frozen model-v2 plan, §6
(`PLAN-MODEL-V2-NODE-QUADRATURE-20260815.md`).
All placement arithmetic is explicitly ordered f64 in the pair's p2s frame;
placement expressions do not contract into FMA.  `qm_atan` and `qm_tan` are
the shared, bit-pinned transcendentals of that frozen-plan contract.

```text
[N-02 FRAME]  t      = unclamped foot parameter of R on line(A, B)
               F      = A + t * (B - A)
               x(s)   = signed abscissa from F on the storage piece
               d_perp = horizontal |R - F|
               h      = max(d_perp, D_FLOOR)
               u(s)   = qm_atan(x(s) / h)
               d_pl(x) = sqrt(h^2 + x^2)
               W_per_m = W_piece / chord_length

[N-03 BOUNDS] U = {u(A), u(B)} plus u=0 when the foot is interior.
               The foot is always a boundary: every cell lies on one arm,
               so no cell evaluates sign(0).

[N-04 HINTS]  Add only the admitted §3.5c skyline transitions.  Hints are
               refinement geometry, never blocked/clear truth.

[N-04a HINT-AZIMUTH] For d_perp >= D_FLOOR,
               u_hint = phi_transition - phi_foot.

[N-04b HINT-RANGE] For d_perp < D_FLOOR, a candidate arc of own range b
               admits each physical crossing d_perp < b <= piece_range as
               x = +/-sqrt(b^2 - d_perp^2), u_hint = qm_atan(x / h).
               Both roots are independent candidates; d_perp, never h,
               is the radicand quantity.

[N-04c ADMISSION] Clip to the piece, retain at most H_MAX candidates in
               ascending range stratum, then (start, end, source_id), then
               u_hint ascending order; dedupe only f64-equal u values and
               sort U ascending. Surplus hints increment an always-on soft
               counter: they mean less refinement, never less screening.

[N-05 CELLS]  For each [U_k, U_k+1],
               K = ceil((U_k+1 - U_k) / THETA_MAX).
               Cells are half-open [u_i, u_i+1); piece ends remain exact.
               Every interior cell boundary is s_i = F + h*qm_tan(u_i).
               arm = -1 left of F and +1 right of F; no cell crosses F.

[N-06 CHUNKS] On each one-arm cell, advance from its near end in d_pl.
               Cut while d_pl_far - d_pl_cur > S_live(d_pl_cur), where
               S_live(d) = THETA_MAX * R_ATM_BASE *
                   (ALPHA_ATM[7] / alpha_live(d)),
               alpha_live(d) = max { ALPHA_ATM[b] :
                   ALPHA_ATM[b] * d / 1000 <= A_LIVE_DB }.
               R_ATM_BASE = 1,000 m/rad; A_LIVE_DB = 25 dB.
               d_pl_cut = d_pl_cur + S_live(d_pl_cur), then invert only as
               s = F + arm * sqrt(d_pl_cut^2 - h^2).

[N-07 NODE]   For a chunk [s1, s2], length ell = s2 - s1 and
               Delta_u = u(s2) - u(s1):
               D^2 = ell * h / Delta_u;
               s_node = F + arm * sqrt(D^2 - h^2).
               The node evaluates its exact 3D slant and azimuth, not d_pl.

[N-08 WEIGHT] W_node = W_per_m * ell.
```

`d_pl` is only the placement metric.  Evaluation uses the node's exact 3D
slant.  The free-field-exact position in `[N-07]` makes the weighted node sum
reproduce the chunk's `integral ds / d_pl^2`; weights telescope to the storage
piece. This is the required free-field normalization identity, not a midpoint
approximation; placement-metric versus exact-3D residual is <= 0.005 dB
A-weighted (Seat A B3 pins realized max 0.00099 dB at `d_perp=5 m`, 3°).
`ALPHA_ATM[7] = 58.4 dB/km` is a derived constant of the frozen model-v2 plan, §6
(`PLAN-MODEL-V2-NODE-QUADRATURE-20260815.md`), not an accuracy dial.

For the production working point `THETA_MAX = 3 degrees`, `H_MAX = 32`, and
`L_MAX = 250 m`, the generator has the proven bound

```text
[N-09 BOUND] N <= ceil(pi / THETA_MAX) + H_MAX + floor(L / S_min) + 2
             S_min = THETA_MAX * R_ATM_BASE
```

which is 98 nodes per pair.  Node overflow remains an always-on hard fault:
its receipt count must be zero, because a non-zero count means the invariant
or implementation is broken, not that a cap was undersized.  Judge mode uses
the same generator with `THETA_MAX -> eps_judge`, uncapped hints and scaled
`S_live`; it is CPU-only and converges constructively for on-source, off-end,
radial and broadside geometry.

### 3.5b One node evaluator and shared path profile (V2 draft)

Every node takes the same complete path evaluation:

```text
[N-10 EVAL] A(node, R) = A_div(r) + A_atm(r) + A_ground_or_barrier
                          + A_vegetation + A_meteo
```

The evaluator marches the node's own DEM/forest/IMD profile, derives its own
terrain diffraction and exact building/barrier crossing race, and evaluates
vegetation and literal CNOSSOS ground on that path.  A skyline-clear node may
skip the vector obstacle walk, but never the terrain, ground or vegetation
march. `node_eval` is the one CPU evaluator; CUDA implements the labelled
mirror under the frozen model-v2 plan, §6
(`PLAN-MODEL-V2-NODE-QUADRATURE-20260815.md`).

Every V2 lane samples DEM, Overture building height, WorldCover forest and IMD
along a node ray by the single bilateral cadence in
`path_profile::fill_t_values`. Terrain diffraction, screening, vegetation depth
and path-mean ground `G` consume the same `PathProfile`.

The ground upgrade is the analytic CNOSSOS ground core **inside** the existing
Quiet Map composite, not a claim of full screened-ground CNOSSOS:

```text
[N-11 COMPOSITE] A_ground_or_barrier = max(A_ground, A_terrain + A_screen)
```

Ground is replaced under a winning barrier and never added to it.  The
CNOSSOS 2.5.30--2.5.32 screened-ground split remains open; the sight-line step
described in §3.5 terrain diffraction remains explicitly unresolved. Aircraft
ground operations retain their isolated legacy `aircraft_ground_atten_db`
formation until an aircraft validation lane exists.

### 3.5c Vector skyline: admission and node-boundary ABI (V2 draft)

The receiver-local vector skyline survives only as an **admission and hint
provider**.  It contains candidate footprint-edge and barrier intervals, not
screening truth.  Every admitted node re-derives its actual terrain and vector
screening from its own ray; no skyline miss is permitted to discard a screen.

Intervals are half-open `[start, end)` azimuths in the pair p2s frame.  A seam
crossing is split first; candidates then iterate in the shared canonical total
order `(start, end, source_id)`.  The union sweep gives transition ownership to
the run it opens.  Per-pair `near_m` is computed after span clipping and
multiply-emitted physical edges dedupe by stable edge/source identity.  CPU and
CUDA share these rules and their fault semantics: interval overflow drops toward
less screening and is counted; node overflow is hard; hint drops are counted
softly. The frozen model-v2 plan, §6
(`PLAN-MODEL-V2-NODE-QUADRATURE-20260815.md`) is the detailed CPU/CUDA ABI
contract for these labels.

The old fan quantities `f_i`, `D_bar` and the transported
`A_screen = max(0, D_bar - A_terrain)` increment do not exist in V2.  This
removes the hard-ground partial-screening clamp structurally: a node returns
its complete composite, and a segment is the energy sum of its nodes.

### 3.5d Production accuracy and CPU judge (V2 draft)

Production uses the finite dialled generator of §3.5a.  The CPU judge uses the
same equations with `eps_judge`, uncapped hints and scaled live-range chunks.
The required convergence receipt halves `eps_judge` and shows every gated cell
moves by at most 0.01 dB.  The V3 production sweep then compares the complete
production evaluator against that judge on the fixed cells including dense
Praha; its required bound is at most 0.5 dB per pixel and zero paint-presence
flips.  No uniform-node, blocked-fraction or distant-LOD result is reusable as
this proof.

### 3.5e Popup node contract (V2 draft)

The base popup retains one aggregate segment row and adds only its `node_count`.
It contains no per-node rows.  Its uncompressed response limit is
`BASE_PAYLOAD_MAX_BYTES = 3,200,000`; rows are emitted in descending segment
Lden contribution, tie-broken by stable row id.  At the limit it truncates at a
row boundary and returns `truncated` plus a last-row cursor; continuation uses
the same order and limit.

Expanding a segment lazily returns compact node rows
`{angle, r, screening_class, node_lden}` under
`LAZY_NODE_RESPONSE_MAX_BYTES = 65,536`. Clicking a node lazily returns its
complete engine propagation breakdown under that same cap. Node identity is
`(layer, source_row_id, piece_index, node_index)`.  Every lazy request binds
the kernel identity hash, data-build identity, receiver coordinate and layer;
a mismatch after deploy or data publication is rejected and the client refetches
the base response.  Show-all returns aggregate rows only.  The frontend derives
no node physics: every displayed node value is engine-computed, and the segment
row is the energy sum of its nodes. An end-to-end popup receipt at the pinned
Praha receiver verifies aggregate segment energy, a lazy node page and a lazy
node breakdown against the same engine identity.

The frozen V8 plan's two identically valued lazy-response names resolve to this
single public contract constant; changing either response limit changes the one
cap and requires the same popup receipt.

### 3.5f Soundness derivation note for the frozen model-v2 plan §4 (D1 review draft)

**Evidence record.** This signed derivation merges the independently authored
S2 notes against frozen V8 plan
`2524216d6d2ba529d46055e2374222037eb45441e34df7ea950a7bef5fb56b47`:
Seat A byte-stop/normalization
`a62a5b6c363c4c0e264b67c03e55ec95116e80ba32dfc11ebc9443d8d1be98a0`,
Seat B ground/reach
`0b78f51b0c18b66f176b10fef7414abaca92e0986ab8fb4889efb955da5cfec7`, and
Seat C cull/split/D_FLOOR
`a1e618e7f71c60274013087be112da76ec11d900b8173de563450159bb5dc3ca`.
Every fast-path bound below is a pre-implementation gate: the affected lever
cannot land until its named fixture is green.

#### §3.5f.1 Node byte-stop upper bound

For a line piece of total power `W_piece`, define `d_near_h` as the
**horizontal endpoint-clamped minimum** receiver-to-piece distance (`dend`).
It is never a closest-foot slant and never `d_pl`.  The one v2 construction is

```text
[S-01 UB] d_ub = max(d_near_h, 1.0 m)
          ub_b = UB_SAFETY * W_piece *
                 10^((refl + GROUND_GAIN_UB_DB + A_weight_b
                      - 20 lg(d_ub) - 11 - alpha_b*d_ub/1000) / 10)
          ub = sum_b ub_b
```

`W_piece` includes the piece length because the source spectrum is per metre;
the old FLC performed that integration implicitly.  Every node lies on the
piece (`[N-07]`), so its horizontal distance is at least `d_near_h`; its exact
3D slant is at least that horizontal distance; the shared 1 m divergence floor
and non-negative atmospheric absorption preserve the inequality.  Ground is
bounded by §3.5f.2, terrain/screening/vegetation are non-negative, and
`sum(W_node) = W_piece`.  Therefore `ub >= exact` term by term for any node
placement.  The horizontal definition is load-bearing: a 30% sloping source
line makes foot-slant `d_near` understate by 0.374 dB, far beyond the 0.0004 dB
`UB_SAFETY` margin.  It also removes the elevation read from the cheap pass.

The required `the_node_bound_is_never_below_the_node_sum` fixture (note A A1-A7) covers
on-source, end-on, off-end, dense-row and far-broadside pieces; all ground,
screening and vegetation corners; `theta_max` 5/3/1 degrees; and 0.06/0.30
slope.  It asserts strict `ub >= exact`, containment, weight telescoping,
far-tail slack <= 0.05 dB at >=1 km, D_FLOOR bit-independence and that
on-source pairs are never byte-stopped.  The G1-CPU receipt reports the new
skip rate and the separately measured saved elevation reads; it must not net
one against the other silently.

#### §3.5f.2 Literal ground gain bound

For the implemented CNOSSOS 2.5.15/2.5.20 pair, both meteorological states
end in `max(analytic, -3*(1-G_prime))`, where §2.5.14 makes `G_prime` a convex
blend in `[0,1]`.  The favourable energy mix preserves the maximum, hence

```text
[S-02 GROUND] A_ground,b >= -3.0 dB,  so  GROUND_GAIN_UB_DB = 3.0 dB.
```

This is independent of band, `z_s`, `d_p` and the open `z_s -> 0` / `d_p -> 0`
branches.  The evidence sweep covered 202,376 numerical states plus 1,800
implementation states with zero violations and a 4.9e-13 dB Rust/Python
maximum difference. Seat A fixtures A1--A7 pin the plain floor at `G=0`.
Seat B carries `tc_ground_v2::ground_gain_upper_bound_v2_corner_sweep`
(12 × 11 × 6 × 3): its plain, non-NoiseModelling sweep reaches exactly
`-GROUND_GAIN_UB_DB`, and its tripwire (`G=1e-9`, `d_p=10,000 m`,
`z_s=0.05 m`) records that a literal NoiseModelling favourable substitution
would require 6.93 dB. That substitution is a named V4 cause and tripwire,
not a silent library substitution.

#### §3.5f.3 Normalization, reach and decided rail ceiling

The node sum replaces the retired FLC/line-divergence pair.  With the code's
rounded point term `20 lg(r) + 11`, rather than an exact `10 lg(4*pi)`, the
realized emission-reference shift is

```text
[S-03 NORMALIZATION] Delta_line = 10 lg(2*pi^2*10^(-1.1))
                                      = +1.953297 dB
                     geometric reach multiplier = 1.567941.
```

`+1.9612 dB` / `pi/2` is the exact-`4*pi` counterfactual and is not the live
constant.  Fixture B4 pins `1.9533 +/- 0.0010 dB`; the 0.0079 dB difference
is explicit rather than rounded away. Node weight is `W_node = W_per_m * ell`,
point divergence is `20 lg(r)+11`, and no FLC or line re-reference
survives in V2.  The free-field node sum equals the closed-form line integral
for any node count; B1--B6 pin broadside, off-end, end-on, non-horizontal and
split cases. V1 test vectors K4/K5 describe the retired FLC/line-divergence
formation only; V2 must re-specify them as piece geometry plus node-sum vectors
before a V2 pin is admitted.

All free-field exits add the 3.0 dB ground headroom.  The v2 line exit is
`L + 3 - (10 lg(d_end) + 6.0) - 0.002*d_end < threshold`; the old `+8`
form can silently over-cull by 4.97 dB.  Point exits add the same headroom and
their uncapped radii scale by 1.412538. Road caps remain unchanged: the
crossing-derived classes' V2 free-field bounds at their present caps are
29.7--31.0 dB, i.e. the render-floor convention. Links and deliberately short
local classes retain separate caps because they run beside or into their louder
parent corridor, not because of a crossing solve. Industrial, building, leisure
and aircraft-ground caps also stay
unchanged with a 0.00 dB node-normalization shift for those policies.

**DECIDED OWNER POLICY — rail halo ceiling.**
`RAILWAY_REACH_CLAMP_MAX = 11,000 m`, and the halo-sizer alias equals it.  Rail
reach is solved independently per row from its traffic, type and speed before
the `[2,000, 11,000] m` clamp.  The additional halo therefore applies only to
rows whose solved crossing exceeds 10 km: the busy main corridors by
construction; quiet branches retain their smaller solved reach without a
separate class rule.

The default mainline's V2 crossing is 10,178.8 m, so it is inside the decided
cap. The loudest corridor families cross 30 dB at 12,416 m in Europe, 13,053 m
on the Tokaido and 15,116 m on the sampled US freight corridor — all strictly
less truncated than under the retired 10 km ceiling, so there is no regression.
The rail-only halo disk cost is `(11/10)^2 = x1.21`, measured by G1-CPU. The
25 dB target and 2,000 m minimum are unchanged. The fixture pins the
unclamped default mainline at 10,178.8 +/- 1 m and the decided 11,000 m clamp;
`reach_box_half_extents_deg` is unchanged geometry but includes 11,000 m in
its containment fixture list.

#### §3.5f.4 Indexed source-cull invariance

The indexed source list remains a strict superset of the authoritative
per-pixel cull.  In the same segment p2s frame, endpoint-clamped distance is
1-Lipschitz and the box's cos=1 half-diagonal dominates every pixel-centre
distance.  Thus `dend(P) <= R_s` implies
`dend(center) <= R_s + R_box`; every contributing source is retained.  The
per-pixel cull stays above node generation and all accumulation, and stable
ascending source-id compaction preserves the exact accumulation sequence;
therefore this is byte identity, not merely presence neutrality.

This is a prerequisite to §3.5f.1's residual bound: that bound covers a whole
tail only after every source whose endpoint-clamped distance can contribute is
retained by this indexed superset.

The invariant is: every cull, reach solver and halo uses endpoint-clamped
`dend`; none reads `d_perp`, `d_pl` or `D_FLOOR`; there is no new per-node
cull below the pair cull.  The barrier-ABI source-id bump derives the
candidate-tail offset from the same `BARR_STRIDE` used by the packer, with a
poison-last-barrier-row fixture.  Receiver longitudes remain unwrapped and
monotone.  The required differential runs indexed and parent exact arms on
Praha road and rail and asserts raw HM3 identity, equal faults and equal
source-list hashes.

#### §3.5f.5 Split-at-load and D_FLOOR pins

`L_MAX = 250.0 m` becomes a Rust-owned SSOT and CUDA-injected constant.  Each
road/rail loader passes a conforming row byte-identically; for an over-length
row it emits `ceil(length/L_MAX)` equal chord pieces, preserves intensive
per-metre emission and reports an always-on split counter.  Weight uses the
same length it integrates: `W_piece = emission_per_m * stored_length` and
`W_per_m = W_piece / chord_length`.  This resolves the present <=0.0032%
chord/polyline discrepancy as a chord redistribution rather than an energy
leak.  Split/merge fixtures cover 1, 2, 33, asymmetric and loader-produced
pieces; free-field energy is tight to 1e-9 relative and full production
agreement is <=0.1 dB. Node counts are deliberately not a split-invariance
fixture: decomposition may change counts while preserving emitted energy.

`D_FLOOR` is placement-only and physically pinned: road = 0.05 + 3.50 =
3.55 m; rail = 0.50 + 0.7175 = 1.2175 m.  New SSOT constants are
`ROAD_HALF_WIDTH_M` (half a 7.0 m two-lane carriageway) and
`RAIL_HALF_WIDTH_M` (half standard 1,435 mm gauge); both are injected through
the frozen model-v2 plan, §6 (`PLAN-MODEL-V2-NODE-QUADRATURE-20260815.md`), never tuned by V3.  The node theorem is independent of D_FLOOR (98 at 3
degrees / 72 at 5 degrees).  Pins assert values, CPU/CUDA parity,
bound-independence and placement-only isolation.

### 3.5z Legacy V1 archive (non-normative)

#### Legacy V1 §3.5a — Unified path sampler

Within this V1 archive, every unqualified `§3.5a`--`§3.5e` reference means the
identically labelled Legacy V1 section, never the V2 draft above.

DEM, Overture building height, WorldCover forest cover and IMD imperviousness are all sampled along the source→receiver line by a single bilateral cadence — density highest near endpoints (Fresnel zone narrowest), coarsest in the middle. Implementation + cadence rationale: `propagation::path_profile::fill_t_values`. Terrain diffraction, building screening, vegetation depth, and ground-effect G all read from the resulting `PathProfile`. The surface **heatmap** additionally runs a coarser distance-dependent middle cadence (`fill_t_values_coarse_mid`) — a heatmap-only speed approximation; the popup always uses the exact cadence.

#### Legacy V1 §3.5b — Combined terrain + building + barrier screening

Diffraction is computed once over a composite top profile (`elevation + building_h`), avoiding the terrain+screening double-count that would otherwise occur when a building sits on a hill. The δ* OLS mean-ground fit stays on bare-earth elevation. Implementation + caller API split (`terrain_attenuation` vs `screening_attenuation`): `propagation::path_effects::screening_attenuation_with_meta`. Ground G and vegetation depth are path integrals weighted by interval length so non-uniform bilateral spacing doesn't bias endpoints.


**Vector obstacle candidates (geodata-v2, `QM_VECTOR_BUILDINGS` — ON by default
since the Wave-1 cutover 2026-07-31, commit 9cf166b; only an explicit
`QM_VECTOR_BUILDINGS=0` restores the raster channel — see the `ENABLED` gate in
`propagation::obstacle_index`):** building screening stops reading the 30 m
raster channel. Exact footprint crossings from the per-cell obstacle store
(`ObstacleIndex`, ray×edge intersections) compete with the cadence composite
edge on δ; the winning candidate is evaluated by `compute_single_edge_at`
(explicit edge point; the §2.5.6(c) mean-ground fits include the bare-ground
point D on BOTH sides, so a candidate at a sample's t reproduces the raster
result bit-for-bit). Candidates never enter the cadence sample arrays —
ground/vegetation integrals and the GPU sample envelope are untouched.

*Obstacle heights (the store's `height_m`/`height_tier`, 2026-08-09 ladder):*
tier 0 mapped per-building height (OSM/Overture), tier 1 floors × 3 m, tier 2
flat 8 m world default (`BUILDING_{FLOOR_HEIGHT,DEFAULT_HEIGHT}_M`), tier 3
city/national measured per-building zonal from a 1 m DSM−DTM raster (IPR Praha
first), tier 4 GHS-BUILT-H ANBH 100 m areal average at the centroid replacing
only the flat default. The low-profile shed cap (`low_profile`, 3 m on a
matched garage/shed-class OSM footprint) applies to tiers 2 AND 4 — the two
tiers that carry no per-building knowledge — never to 0/1/3. Producers:
`scripts/obstacles/ingest-overture-obstacles.py` (0–2),
`scripts/obstacles/enrich-obstacle-heights.py` (3–4, regenerates promoted
cells from staging + rasters).

**Noise barriers: EXACT ray×segment crossings (2026-08-03 fix-pack Fix 3).**
A wall is a polyline element with two endpoints, so whether a path crosses it —
and where — is a closed-form intersection, not a proximity question. Every
`types::Barrier` of the per-tile (heatmap) or per-receiver (popup) slice is
intersected with the source→receiver ray by the same primitive the building
edges use (`obstacle_index::segment_intersection_t`), and each hit becomes an
ordinary dominant-edge candidate at its exact chainage, δ-ranked against the
buildings and the cadence edge. Walls therefore never enter the composite top
profile and never enter `ObstacleIndex` (they arrive per tile from
`barriers.arrow`), and the popup names the wall by its OSM way id.
*Superseded:* until this fix a barrier was screened when its segment MIDPOINT
projected onto the ray within a ±50 m perpendicular radius, then snapped to the
nearest profile sample — which missed a real crossing far from a long wall's
midpoint and screened a near-midpoint pass that never crossed (measured on the
Voznice D4 wall, 618 m/5 segments: 22 % of all (D4 microsegment × receiver)
pairs in a ±500 m grid got the wrong verdict — 18 % false screens, 4 % missed
crossings). The scan's early-break horizon is `path_len +
BARRIER_PATH_HORIZON_M` (125 m max wall half-segment + 50 m flat-earth slack):
a crossing point lies on the path, but the crossing wall's midpoint — the point
`dist_m` is measured to — can sit a half-segment beyond it.

#### Legacy V1 §3.5c — Arc-clipped screening for line sources

A road/rail microsegment runs up to 250 m and subtends a wide angle at a nearby
receiver (136° at 50 m), but §3.5b evaluates screening ONE ray per segment — to
its characteristic point — and applies that verdict to the whole segment's
emission. A 30 m building covering 33° of the fan therefore screened everything
or nothing, toggling as the cp ray hit or missed it: the measured
constant-width shadow stripes behind buildings (−8..−15 dB where line-source
physics gives −1..−2 dB).

`propagation::arc_screening` replaces the verdict with an angular energy
average, for line callers holding a vector obstacle store:

```
span      = angular span of the segment at the receiver (2 × atan2)
skyline   = the receiver's merged blocked azimuth arcs, LAYERED BY STRATUM —
            every obstacle edge within `radius` projected to its short arc,
            each carrying the nearest range `b` it reaches. No absolute top is
            carried: nothing downstream of admission reads one (see `blocked_i`).
            Built ONCE PER RECEIVER and shared by all its segments. Two arcs
            merge only inside ONE stratum of height (3 m bands) AND of range
            (geometric bands of ratio 1.5): a merge hands `b = min` and
            `h = max` to the UNION of the merged azimuth ranges, which is
            honest only where every member describes the same obstacle to
            within those bands. Inside a stratum the arcs are sorted and
            disjoint; across strata they overlap freely and each faces the
            admission test with its OWN range.
blocked_i = `skyline` clipped to `span`, keeping only arcs that stand in FRONT
            of the source and not under the receiver's feet: `b ≥ 1` and
            `a = d − b > 1`, in metres. GEOMETRY ONLY — no sight-line or δ
            test. Admission is a FORK between two ray marches that differ in
            one thing, whether obstacles are consulted: a false negative
            deletes a real screen outright, while a false positive only
            requadratures (its endpoints move the sampled midpoints, and a
            piece covering the cp azimuth reuses the cp verdict) — bounded
            against unbounded. Both lanes' δ prefilters were unsound, in
            opposite directions, and a sound one measured slower AND less
            accurate than none (2026-08-09). The real δ, penumbra branch
            included, is re-derived per ray by §3.5b from the marched profile.
A_i       = §3.5b screening on the ray to the EXACT point on the segment at
            interval i's centre azimuth (its own path profile + terrain, which
            serve only as that call's increment base); an interval wider than
            0.26 rad is split into 3 sub-intervals, once
f_i       = |blocked_i| / span

D_i    = max(A_ground, A_terrain + A_i)   per §3.3   (D_clear for the rest)
D̄      = −10 × log₁₀( f_clear·10^(−D_clear/10) + Σ f_i·10^(−D_i/10) )
A_screen = max(0, D̄ − A_terrain)
```

Averaging is done on the §3.3 ground/barrier term, NOT on the bare screening
increment: the two are not independent, and a 14 %-blocked fan yields ~0.3 dB
of mean screening, which `max(A_ground, …)` would discard whole. Handing back
`D̄ − A_terrain` makes the caller's own `max()` reproduce `D̄` exactly —
**whenever `D̄ ≥ A_terrain`**, which is every case except one:

> `A_screen` is non-negative by contract and the caller reads
> `A_terrain + A_screen = 0` as "no barrier", so `D̄ < A_terrain_cp` cannot be
> transported — `A_terrain_cp` being the cp ray's terrain, the one the caller
> re-adds. There are **two** ways to land under it:
>
> 1. `A_terrain_cp = 0`: it arises exactly when `D̄ < 0`, i.e. when the fan
>    averages to a net boost — which the §3.3 hard-ground floor
>    (`A_ground = −3 dB`) makes true for fans blocked less than ≈50 %.
>    **Louder than exact by `D̄ − A_ground` ≤ 3.0 dB**, on hard ground only,
>    peaking at ≈50 % blocked and vanishing at both ends.
> 2. `A_terrain_cp > 0` while the FAN's terrain is smaller. Each `D_i` is
>    `≥ A_terrain_i` — its OWN direction's terrain, which stopped being the cp
>    ray's when the fan's terrain went per-direction (2026-08-08 §3.5c
>    correction). So "each `D_i ≥ A_terrain`, hence so does their mean" is no
>    longer a theorem, and a cp ray running over the fan's one crest can set a
>    bar the flatter directions average under. **Magnitude unmeasured** — the
>    fixture's only relief scene (I) is dominated by case 1 — so this is an
>    open item, not a bounded one.
>
> Either way the clamp returns 0, the caller falls back to
> `max(A_ground, A_terrain_cp)`, and the pixel keeps that rather than the fan
> mean. Over-prediction is the safe direction here, but closing the gap needs a
> signed `A_screen` plus an explicit "barrier present" flag on the CPU and CUDA
> lanes — a contract change, deliberately not taken with the hard-ground fix.
> OPEN.

#### Bounds (owner ruling 2026-08-03 §4b: ≤ 0.5 dB at every pixel)

Exact arc clipping is an AREA query per (segment, receiver) pair, which measured
60× the GPU line kernel on a dense rail cell. Four bounds replace per-pair work
with per-receiver work; all four come from the same law that makes the profile
cadence dense at both endpoints, δ ≈ h²·d/(2ab):

- **skyline** — the candidate set is per RECEIVER, not per pair. No
  approximation, and a better candidate set than the pair's triangle (a
  footprint is no longer split by the query boundary).
- **grazing prune** (`δ_min = λ/8 at 8 kHz = 0.0053 m`) — a grid cell whose
  tallest edge cannot reach `h = sqrt(2·δ_min·b)` at its own range is skipped
  whole: `maekawa_bands` gates such a path to zero in every band.
- **span floor** (`min_span`) — below it the segment radiates in essentially one
  direction and the cp ray IS that direction. Pre-gated by the exact bound
  `span ≤ L/d`, so a narrow pair costs one compare, not two atan2.
- **skyline radius** — `L/(2·tan(min_span/2)) + L`, a CONSEQUENCE of the span
  floor rather than a free parameter: anything screening a path stands between
  receiver and segment, so a radius that reaches every span-eligible segment
  reaches every screen too, which is what lets the clear fraction of the fan be
  treated as exactly clear. Grown on demand and snapped UP to a range-stratum
  boundary, so a stratum is either gathered whole or not at all.

**REPRODUCIBILITY IS PART OF THE MODEL.** The skyline is grown to whatever the
CURRENT segment needs and every later segment of that receiver reads the grown
result, so the arcs a segment is clipped against depend on which sources came
before it. That is harmless only while the merge keeps each arc's own range: an
arc standing beyond the segment then falls to `b < d` whatever dragged it in.
Merged across ranges it does not, and reordering the loader's rows — a
transformation that must change nothing — moves the map. MEASURED on a
production road tile with the energy-budget skip OFF, so both orders do
bit-identically the same 70 939 403 path calls: with a height-only fuse,
RMS 0.074 dB / **max +10.70 dB** / 543 pixels over 0.5 dB; with the strata,
RMS 0.005 dB / max +0.39 dB / **0 pixels over 0.5 dB**, and 0.0004 dB max over
the ≥45 dB pixels the map actually shows. Pinned by
`arc_screening::source_order_never_changes_the_answer`, which shuffles the source
order over four geometries × 12 permutations and asserts bit equality (it reports
11.64 dB against the height-only rule).

The literal direct-ground vector, vegetation and finite-line correction stay on
the cp ray. Each bucket and each interval ray already evaluates its own terrain;
the tile path consumes that composite directly, while the popup's legacy
increment-return transport can carry only its non-negative increment above the
cp terrain. A future full-node evaluator would remove that transport loss. In
the **tile-painter CPU and CUDA lanes**, no vector obstacle store means no
complete skyline to clip, so the cp verdict stands unchanged even when a noise
wall exists; otherwise the arc rays would erase real raster-building screening.
The popup engine has no raster-building fallback and may safely construct a
wall-only skyline from an empty vector set. Point sources have no span. Validation
(`src/bin/screening_fixture.rs`, 697 receivers × 3 scenes,
vs a 33-sub-segment reference integration): shadow-zone error 7.19 → 0.73 dB
behind a 30 × 10 × 8 m box, 1.07 → 0.31 dB behind a 3 m barrier, no-obstacle
parity unchanged at 0.25 dB, for ~2× the screening cost of one cp ray.

#### Legacy V1 §3.5d — Uniform angular quadrature

§3.5c integrates the fan with ADAPTIVE quadrature: nodes taken from the
receiver's obstacle skyline. `propagation::seg_sampling` integrates the SAME
term with UNIFORM quadrature: the span is cut into `N` equal angular buckets,
each evaluated on the exact source point at its centre azimuth, and
`max(A_ground, A_terrain + A_screen)` energy-averaged over them; `N → ∞` is a
reference for both rules. The current literal ground vector and vegetation stay
on the cp ray in both.
`N = 1` here is NOT §3.5b's cp verdict — one bucket rays the fan's CENTRE
azimuth, the cp is its CLOSEST point — so the tile kernel keeps `N = 1` on the
cp path instead of calling this. `QM_SEG_SAMPLES=1` alone does NOT restore the
pre-2026-08-05 bytes any more: it only routes the painter down the cp fallback,
which with a complete vector store has since 2026-08-08 arc-clipped segments
wider than 3° and left narrower ones on a plain cp verdict. Raster fallback has
no complete skyline authority and remains cp-only. On the vector-backed path,
the pair `QM_SEG_SAMPLES=1 QM_ARC_MIN_SPAN_DEG=0` is what reproduces the old rule
(byte parity verified on all four tiles when the switch landed, i.e. before the
3° gate existed — **not re-verified since**).

WHICH RULE RUNS WHERE, and why they are not the same choice:

* **Tiles** run uniform `N = 5`, with §3.5c ON *per bucket* for any bucket wider
  than `seg_sampling::SEG_ARC_MIN_SPAN_RAD` (3°) and off for the rest
  (`tile_painter::scatter_band::{seg_samples, tile_arc_bounds}`; `QM_SEG_SAMPLES`
  / `QM_ARC_MIN_SPAN_DEG` move both). Measured 2026-08-05 on four production
  tiles, one binary, sandwiched arms, CPU-seconds and RMS dB against an `N = 9`
  reference from the same binary:

  | tile | §3.5c (was) | §3.5d (is) | cheaper | closer |
  |---|---|---|---|---|
  | praha | 28542 s / 0.77 dB | 7443 s / 0.42 dB | 3.83× | 1.84× |
  | suburb | 7596 s / 0.40 dB | 1960 s / 0.15 dB | 3.88× | 2.62× |
  | d1open | 4585 s / 0.43 dB | 1096 s / 0.14 dB | 4.18× | 2.99× |
  | rail2206 | 5703 s / 0.34 dB | 1243 s / 0.06 dB | 4.59× | 5.24× |

  The two corrections also OVERLAP (r = 0.67-0.80, same sign on 82-97 % of the
  pixels either moves) rather than add, so composing them AT WHOLE-SEGMENT
  GRANULARITY is worse than the quadrature alone at 2.5-3× the price. Composing
  per BUCKET is a different trade and the one that ships — see §3.5e.
* **The popup** keeps §3.5c on its single cp ray. One receiver can afford
  adaptive quadrature, and it is the accuracy etalon.
* **The CUDA lane** paints §3.5e since 2026-08-19: `scatter.cu`'s `line_source`
  runs the same bucketed quadrature with the same per-bucket 3° gate, both
  constants injected by `build.rs` from `SEG_SAMPLES_DEFAULT` and
  `SEG_ARC_MIN_SPAN_RAD` so the lanes cannot drift. Since 2026-08-20 the bucket
  count is span-adaptive (the L3 lever): a pair whose whole fan fits under the
  3° gate marches ONE span-centre bucket instead of `SEG_SAMPLES` — 98.8 % of
  walked pairs qualify (seg_sampling census 2026-08-19; a 250 m piece past
  ~4.8 km subtends < 3°). That count is the kernel's ONE deliberate
  approximation against this section's rule — an accuracy spend the W1
  aggregate gates score (evidence: `research/ai-labs-2026-08-19/
  FINDINGS-k3port2.md` in the ops repo), not a reach cut: every pair still
  marches once and the per-bucket gate+escalation is unchanged. A CPU-vs-GPU
  tile comparison therefore runs BOTH lanes at defaults and its residual now
  includes this n=1 narrow-span class —
  `noise_gpu::ensure_no_cpu_only_arc_levers` refuses every CPU-only override.
  (`QM_SEG_SAMPLES=1` /
  `QM_ARC_MIN_SPAN_DEG=0` are the PRE-PORT kernel's rule, and before the port
  they were the pins this comparison had to run under, which hid the fork by
  construction.)
  Candidate-4 deletes one output-exact subphase in the complete,
  non-degenerate vector-fan branch: the cp ray still marches DEM+cover and
  supplies terrain, the ground plane, G and vegetation, but it does not scan
  obstacle/barrier crossings or build a screening composite. That screening
  value is dead because every bucket supplies its own composite; raster
  fallback, an incomplete/degenerate fan, and every bucket/interval/H0 ray keep
  screening unchanged. Even the complete-fan `used==0` fallback consumes only
  the cp terrain and ground terms, never its screening result.
  `CP_SCREEN_DELETE=0` is the reviewed measurement control.
  Its arc geometry is f64. The reviewed `SEG_ISECT_F32` switch narrows the
  shared ray-edge chainage solve after its coordinate differences are formed in
  f64. In the repaired official RTX role-attribution series this role contributed
  a repeatable **7.64--15.94 % road** and **5.13--9.60 % rail** gain; every
  role-attribution pair was byte-identical. Candidate-4 explicitly spends the
  accepted accuracy headroom and defaults this lever **ON**, while retaining
  `SEG_ISECT_F32=0` as the otherwise-identical f64 isolation control. Strict
  intersection boundaries can still change topology; the composed candidate
  is therefore scored against the regenerated bcb872e CPU reference rather
  than treating the earlier byte identity as a proof. Implementation and
  control: `scatter.cu` at `SEG_ISECT_F32`.

The uniform rule hands back the ground/barrier COMPOSITE, not a screening
increment, so at the OUTER level — the quadrature's own average over buckets —
there is no non-negative channel to saturate and no `D̄ < A_terrain` clamp.
Pinned by `tile_painter::scatter_line::hard_ground_keeps_its_partial_screening`
(2.2 dB where the handback gave 0.0).

That does **not** close the hole on the tile path. §3.5e runs §3.5c *inside* every
bucket wider than 3° (`seg_sampling.rs` → `arc_screened_attenuation`), and that
call still returns an increment through the same `max(0, D̄ − A_terrain)` clamp —
so the hole recurs PER BUCKET, on the bucket's own terrain, wherever a wide
bucket is partially blocked over hard ground. What the composite handback removes
is one clamp at whole-segment granularity, not the clamp itself. OPEN on all
three lanes (tiles and — since the 2026-08-19 port — CUDA per bucket, popup per
segment).

Both rules take each ray's own `A_terrain`, never the cp ray's — see §3.5c.

#### Legacy V1 §3.5e — Near-field gate

§3.5d shipped with a known TAIL: on the eleven `screening_fixture` scenes,
uniform `N = 5` reached **2.4-7.6 dB** at its worst receiver where §3.5c holds
0.6-0.9 dB, breaching the owner's 1.0 dB-anywhere line on NINE of the eleven.
Tile RMS still favoured it because those pairs are a small pixel share, so the
two metrics disagreed and both rules stayed in the tree.

**The tail is aliasing, not under-resolution, and that is why `N` cannot fix it.**
Uniform nodes at pitch `span/N` beat against a building row's own pitch: a
bucket count that lands the nodes in the gaps reports the row as open. Swept from
this binary, worst receiver per scene: `N = 5` → 2.17 dB on scene F, `N = 6` →
**8.21 dB**, `N = 33` → 1.09, `N = 49` → 1.19. The MEAN falls as `N^-1` (0.40 →
0.27 dB) exactly as §3.5d predicts; the WORST POINT is not monotone in `N` at
all, and nothing under `N ≈ 65` — 13× the ray budget, i.e. dearer than the §3.5c
it replaced — is reliably inside 1.0 dB. Two targeted `N` rules were built and
swept, and both are rejected for the same reason (bucket-width cap: six scenes
get WORSE at the proposed 0.26 rad; disagreement-driven refinement: halves the
mean, leaves the tail at 1.6-3.2 dB, and REGRESSES on top of the gate). The
sweeps live in `propagation::seg_sampling`'s module docs.

**What ships is a SELECTOR over the two existing rules.** `seg_sampling` could
always nest §3.5c inside a bucket, over that bucket's own sub-span; it was off.
It is now on for any bucket wider than `SEG_ARC_MIN_SPAN_RAD = 3°`, and off
below — so the nodes come from the GEOMETRY exactly where five of them cannot
describe the fan, and nowhere else. It is a WIDTH test on the bucket, which is
what makes it affordable: a bucket is `1/N` of the segment, so a 250 m
microsegment 3 km out subtends 0.017 rad per bucket and never asks, and far pairs
are what a tile's cost is made of.

Threshold from a sweep, worst receiver over all eleven scenes: 15° → 2.75 dB, 7°
→ 2.00, 5° → 1.33 (fails), **4° → 0.96 (passes)**, **3° → 0.85**, 1° → 0.85. A
clean knee between 5° and 4°, and a plateau from 3° down that is not quadrature
error at all — §3.5c itself reads 0.88 dB there and refining the fixture's own
reference moves it. 3° is one notch inside the knee AND on the plateau.

Result, all eleven scenes, `|reference33 − rule|` (`v5` arm, which reports through
§3.5c's non-negative increment channel and is therefore a LOWER bound on what the
tile path gets):

| | worst receiver | mean of scene means |
|---|---|---|
| §3.5d alone (`N = 5`) | 7.60 dB | 0.305 dB |
| §3.5d + this gate | **0.85 dB** | **0.262 dB** |
| §3.5c alone (the etalon) | 0.93 dB | 0.261 dB |

Every receiver on every scene is now inside 1.0 dB, and the suite's remaining
breaches are scenes C/D at their own 0.35 dB limit, where §3.5c and the
pre-port CUDA rule (one cp ray + a per-segment arc pass) breached identically
(0.44-0.48) — a floor on the 3 m wall shared by all the rules, not this one's
tail.

COST, and READ THIS BEFORE QUOTING §3.5d's SPEEDUP: the four-tile table in §3.5d
was measured 2026-08-05 and its 3.8-4.6× is STALE, because §3.5c's own machinery
got ~1.8-2.1× faster in the same session (`ARC_QUADRATURE_MIN_RAD` window
coalescing, the sector wedge). Re-measured 2026-08-08 on the same four tiles and
windows, one binary, sandwiched arms, CPU-seconds:

| tile | §3.5c (v3) | §3.5d alone | vs v3 | §3.5d + gate | vs v3 | gate costs |
|---|---|---|---|---|---|---|
| praha | 13432 s | 7334 s | 1.83× | 7638 s | 1.76× | +4.1 % |
| suburb | 5858 s | 1880 s | 3.12× | 1978 s | 2.96× | +5.2 % |
| d1open | 2581 s | 1043 s | 2.47× | 1122 s | 2.30× | +7.6 % |
| rail2206 | 2979 s | 1223 s | 2.44× | 1244 s | 2.39× | +1.7 % |
| **together** | **24850 s** | **11480 s** | **2.16×** | **11981 s** | **2.07×** | **+4.4 %** |

So the gate keeps 92-98 % of the uniform rule's own speedup. What it does NOT do is
restore §3.5d's 3.8-4.6× — that number was never the gate's to lose. Re-confirmed
on a LATER tree state (the exact-cadence change of the same day, which lengthens
every ray under 400 m and re-prices both arms): d1open v3 11619 s, `N = 5` 5250 s
(2.21×), gated 5578 s (2.08×, +6.2 %); rail2206 v3 12952 s, `N = 5` 5383 s
(2.41×), gated 5454 s (2.37 ×, +1.3 %). The surcharge is stable across both tree
states; the BASELINE is what moves.

ACCURACY on the same four tiles has to be scored against a reference converged in
BOTH parameters, because an `N = 9` UNIFORM reference shares the aliasing defect
under test and therefore flatters the rule that has it (against `N = 9` the gate
reads a WORSE RMS on three tiles and a better p95 on all four — the signature of a
biased reference, not of a worse rule). `N = 9` with the gate at 1° is converged:
on the fixture, `N = 9 → 17` and `1° → 0.5°` each move the verdict by ≤0.01 dB.
Against it (RMS / p95 / share of pixels over 1 dB):

| tile | §3.5c (v3) | §3.5d alone | §3.5d + gate |
|---|---|---|---|
| praha | 0.700 / 1.380 / 9.1 % | 0.380 / 0.703 / 2.2 % | **0.321 / 0.523 / 1.2 %** |
| suburb | 0.348 / 0.461 / 1.2 % | 0.154 / 0.245 / 0.3 % | **0.149 / 0.138 / 0.1 %** |
| d1open | 0.395 / 0.647 / 1.6 % | 0.140 / 0.218 / 0.2 % | **0.124 / 0.139 / 0.1 %** |
| rail2206 | 0.315 / 0.612 / 1.3 % | 0.070 / 0.120 / 0.02 % | **0.069 / 0.107 / 0.02 %** |

Pareto-better than §3.5d alone on every tile and every column, and 2.2-4.6× lower
RMS than §3.5c — so the gate is not a tail-for-bulk trade. It buys the fixture's
worst receiver AND the tile's bulk, for 4.4 % of the paint.

**THE CUDA TILE LANE PAINTS THIS RULE SINCE 2026-08-19.** Before the port the
CUDA lane painted §3.5c per pair and had no bucket to gate, so its analogue of
this section was `ARC_MIN_SPAN` in `noise-gpu/kernels/scatter.cu`: whether a
(segment, receiver) pair got the arc walk at all, default OFF — a fork the
parity harness hid by pinning the CPU back onto the kernel's rule. The port
makes the constant the SAME gate the CPU runs: `ARC_MIN_SPAN` now defaults to
`SEG_ARC_MIN_SPAN_RAD` (injected by `build.rs` through the same `to_radians`
spelling, so the two lanes hold bit-identical thresholds), and it decides per
BUCKET, by the chord law, whether that bucket escalates to a nested arc query
over its own sub-span. In the raster-fallback branch (no complete vector
obstacle store) the cp verdict stands, including for wall-bearing paths, so that
the missing footprint skyline cannot erase raster-building screening.
`-DARC_MIN_SPAN=<x>` remains the A/B lever on the gate width.

Accuracy evidence, labelled by what it is evidence FOR. For the RULE: the CPU
measurements of this section (the fixture sweep and the four-tile table above)
— they price §3.5e itself and carry over to the kernel unchanged, because the
kernel port substitutes the `gob` term and nothing else. For the PORT: the
pin-free `e2-full` lane gate, which pre-port compared two different rules BY
DEFAULT (the fork: 69,005 cells moved, 88.5 % CPU-louder, on rail z12/2206/1391)
and post-port compares the same rule on both lanes — its residual is the
pre-existing vector-candidate evaluation fork documented at
`scatter.cu`'s `arc_screen_bands` (the per-edge-range work: 2,298 cells /
5.91 dB / 39.5 % GPU-louder on that tile), which the port neither fixes nor
hides. The `-DPROF_ABLATE=1` build measures §3.5d (buckets, no escalation) on
the GPU; there is no GPU build of pre-port §3.5c any more.

The port applies the 3° rule twice, just like the CPU: the outer chord test
rejects buckets that are point-like in combined angle/range, and the inner
`ARC_MIN_SPAN_REALISED` test rejects buckets whose actual endpoint azimuths are
narrower than 3°. `ARC_DEGENERATE_SPAN` remains the independent numerical guard
against division by a near-zero span. This second test matters for long segments
seen nearly end-on; defaulting it to only the numerical floor would be a hidden
CUDA-only rule. The record below is kept as the history of the PRE-PORT kernel —
its 5° sweep measured a segment-granularity gate on the old rule, not this one.

<details><summary>Pre-port record (2026-08-10): the segment-granularity
eligibility lever on the old §3.5c-per-pair kernel</summary>

The CUDA lane used to paint §3.5c per pair and had no bucket to gate, so its
analogue of this section was `ARC_MIN_SPAN`: whether a (segment, receiver) pair
got the arc walk at all. That one constant drove TWO tests, which is why every
A/B of it measured them together:

* the **pre-gate** kept the pair when `L > dend·ARC_MIN_SPAN`, with `dend` the
  distance to the segment's NEAREST point. In ONE flat-earth frame `L/dend` bounds
  both ways a segment can fail to be a point — the ANGLE
  (`span ≤ 2·atan(L/(2·dend)) ≤ L/dend`) and the RANGE spread (distance to the
  receiver is 1-Lipschitz along the segment, so its max less its min is `≤ L`, and
  that min IS `dend`) — so what it rejects is point-like in both senses, which is
  what a point-source test has to mean. It is CONSERVATIVE, not provable, for two
  reasons. FRAMES: `sp[0]` is the host's `length_m`, `p2s` scales longitude by the
  segment-MIDPOINT latitude through `__cosf` (f32), and the realised span uses the
  RECEIVER's in f64 — differing by `cos(lat_rcv)/cos(lat_mid)`, ~0.2 % over 10 km
  of latitude at 50°. ROUNDING: `length_m` is serialised to one decimal by the
  extractor, so it can sit up to 0.05 m below the true chord (0.5 % on a 10 m
  segment), always in the rejecting direction. At a 5° setting it therefore decides
  pairs whose realised span is 5° ± a few hundredths of a degree: physically nil,
  formally not a bound.
* the **realised-span test** inside `arc_screen_bands` compared the ACTUAL atan2
  span. It bounds the angle alone, so it also discards a long segment seen nearly
  END-ON — 250 m of range at ~0.1° of azimuth — where the cp ray does not
  represent the segment and the point-source argument does not hold. It reads
  `ARC_MIN_SPAN_REALISED`, whose pre-port default was the degenerate threshold:
  its one sound job in that old rule was numerical, because the interval
  arithmetic divides by the span.

Both defaulted to the degenerate threshold, so the shipped kernel was unchanged,
shown two ways: the emitted PTX was byte-identical to a build of the kernel
without these levers, and painting a reference tile with each binary gave
cell-for-cell identical output. (The end-window guard is `#if`, not `if (MACRO >
0)`, for that reason — a runtime test on a macro is eliminated all the same but
perturbs instruction scheduling, which breaks hash-verifiable inertness without
changing behaviour.) This constant's CPU counterpart was
`arc_screening::ARC_BOUNDS_DEFAULT.min_span_rad`, which the CPU swept on its own
lane and set to 0.0 on 2026-08-04 — at 0.01 rad dense Praha put 25 road and 37
rail receivers over 1.0 dB (maxima 7.8 / 11.4 dB) to buy 1.15-1.33×.

**MEASURED, ON THE PRE-PORT KERNEL (2026-08-10).** Every figure below was
measured with BOTH thresholds at 5°
(`-DARC_MIN_SPAN=0.0872664626 -DARC_MIN_SPAN_REALISED=0.0872664626`), the
pre-split behaviour of the single constant, on the pre-port rule — do not attach
these numbers to the shipped §3.5e port.
Two independent agent labs swept the angle on different hardware and found the same
5° knee. Against the
CPU production tile rule on three reference cells, the gate's own marginal (gated vs
ungated GPU) splits like this by the map's own paint bands — the frontend ramp starts
at 30 dB and renders nothing below it, so sub-30 dB drift cannot be seen:

Measured on one RTX 5070 Ti with an exclusive GPU and a complete obstacle halo,
tile 2206/1391 rail, against a CPU production-rule reference painted from the same
data. The gate's own marginal against the UNGATED kernel, by band:

| operating point | kernel | ×vs ungated | ≥60 dB >1 dB | 30-60 dB >1 dB | max |
|---|---|---|---|---|---|
| ungated | 275.0 s | 1.0× | — | — | — |
| sound gate 5°, +600 m window | 24.4 s | 11.3× | 0 | **0** | 1.0 |
| sound gate 8°, +600 m window | 18.6 s | 14.8× | 0 | 13 | 2.0 |
| both tests 5°, +600 m window | 17.4 s | 15.8× | 0 | 5 | 1.5 |
| both tests 8°, +600 m window | 16.4 s | 16.8× | 0 | **210** | 4.0 |

Three things this settles. In the LOUD band no operating point moves a single cell
over 1 dB. The SOUND formulation is the accuracy winner outright — zero cells over
1 dB in every band at 5°, and an order of magnitude better than the unsound one at
8° (13 vs 210). And 8° is Pareto-dominated by 5° on this tile: 6 % more speed for
42× the visible drift. On the rural holdout no operating point moves any visible
cell by more than 0.5 dB, and its rail tile is entirely below 30 dB.

THE ERROR IS ONE-SIDED, AND THAT MATTERS MORE THAN ITS SIZE. `compare_hm3`
historically reported magnitude only, which is positive by construction and can
hide a systematic lean. It cannot be ignored here because `build-pyramid` averages
ENERGY: a bias that leans one way PERSISTS into the lower-zoom overview instead of
cancelling there, and the overview is the first thing a visitor sees. Signed means,
with the share of MOVED cells that got louder:

| comparison | band | signed mean | of moved, louder |
|---|---|---|---|
| gate marginal, dense rail | ≥60 dB | +0.0013 dB | 59 % |
| gate marginal, dense rail | 30-60 dB | +0.0258 dB | 69 % |
| gate marginal, dense city rail | ≥60 dB | −0.0056 dB | 3 % |
| gate marginal, dense city rail | 30-60 dB | −0.0396 dB | 3 % |
| pre-existing fork, dense rail | 30-60 dB | −0.1321 dB | 10 % |
| pre-existing fork, dense city rail | ≥60 dB | +0.1021 dB | 92 % |

So the gate's error IS one-sided — and its SIGN FLIPS BY TILE, louder on dense
rail, quieter on dense city. That is precisely why it partly cancels the fork on
dense city, where the fork is 92 % louder, and slightly adds on dense rail, where
the fork is 90 % quieter. Sizes: the gate's lean is ≤0.04 dB, about a twelfth of
the 0.5 dB HM3 quantum, so it cannot by itself move a rendered byte; the fork's
0.10-0.21 dB can, and is the one to fix. Note the lean PERSISTS at overview zoom
rather than growing — energy-averaging N cells each biased by +0.03 dB yields
+0.03 dB, not N × 0.03 dB.

Decomposition, same tile: the gate alone is worth 10.6×, the 600 m window alone
only 1.20×; the window earns its 1.49× only AFTER the gate removes the arc walk,
because what remains is the cp-ray obstacle DDA it cuts.

Enabling it is nonetheless a separate, owner-level decision, on two grounds. First it
re-forks the lanes, so `noise_gpu::ensure_no_cpu_only_arc_levers` requires every
CPU-only override to stay unset and the GPU build consumes the compiled CPU default
directly. Second — and this is the one to carry forward — the dense-city
result is partly ERROR CANCELLATION: the ungated kernel's own vector-candidate fork
runs GPU-louder on that tile while this gate is uniformly quieter, so the gate scores
well there by cancelling a fork it does not own. Its 419-cell marginal is 100 %
one-directional, which is the signature. When that fork closes the cancellation closes
with it. RE-MEASURE THE GATE AT THAT POINT rather than inheriting this verdict.

Read `min_span_rad` carefully when comparing lanes: the tile path takes it from
`seg_sampling::seg_arc_bounds`, which substitutes `SEG_ARC_MIN_SPAN_RAD` (3°) when
`QM_ARC_MIN_SPAN_DEG` is unset — but there it gates the PER-BUCKET nesting of
§3.5e, not whether a segment gets angular treatment at all. `ARC_BOUNDS_DEFAULT`'s
0.0 is the exact/popup value. Before the port neither lane's default was the
other's, which is why the comparison pinned `QM_SEG_SAMPLES=1
QM_ARC_MIN_SPAN_DEG=0` explicitly; since 2026-08-19 both lanes' defaults ARE the
same rule, and the guard requires every CPU-only lever to stay unset instead.

</details>

The same file carries `CAND_END_WINDOW_M`, a DEV-ONLY input prefilter letting the
per-ray obstacle DDA skip cells lying wholly inside the middle band `[W, dist−W]`
of the path. It filters CELLS, not obstacles: edges are listed in every supercover
cell they cross and the edge loop solves each referenced edge's full intersection
without requiring the crossing to lie in the current cell, so a mid-path edge also
listed in a retained END cell still screens. It is therefore grid-dependent, leaky
in the safe direction, and not a clean "obstacles near the ends only" rule.
It is likewise DEFAULT-OFF, for a blunter reason: no non-zero `W` is exact for all
geometry — one tall mid-path building is precisely what it discards — against the
model invariant that path effects are computed at every distance inside a source's
reach. The error is not literally unbounded, being capped by this section's 20 dB
per-band screening ceiling, but that ceiling is far too coarse to serve as a bound.
What
it prices is the per-receiver candidate structure that would replace it.

### 3.6 Building screening (ISO 9613-2, per-band)
Current V1 samples the Overture Maps 30m building raster at the Legacy V1 §3.5a
bilateral cadence. V2 uses §3.5b's per-node cadence. Explicit `noise_barrier`
geometries compete with raster buildings. For industrial sources, screening
samples inside the source's own footprint are skipped via an exclusion radius.

**Barrier consumers (B8/C9, 2026-06-11).** Popup and ALL current V1 CPU surface heatmap kernels (road/rail `scatter_line`, industrial/building `scatter_point`, aircraft `ground_ops`) feed `barriers.arrow` vector barriers into the Legacy V1 §3.5b exact-crossing candidate race via the shared `screening_attenuation`; the heatmap prepares one slice per z13 tile (`tile-painter::source_loader_barrier::BarrierData::for_tile` — sorted ascending by a conservative lower-bound distance; contract documented on `types::Barrier`). The **GPU line kernels (`noise-gpu`, road/rail on GPU cluster boxes) screen the SAME vector barriers behind the `QM_GPU_BARRIERS` gate** (default ON since 2026-08-02 — in the ENGINE itself, after the v2 orchestrator lost the wrapper-supplied env and fleet GPU paints ran wall-blind): the per-tile `for_tile` slice is uploaded with both endpoints and the kernel runs the identical ray×segment intersection (`scatter.cu` `barrier_best_candidate`, sharing `seg_isect_t` with the building walk; the pre-Fix-3 projection-and-snap measured on RTX 5070 mean 0.002 / max 1.5 dB vs the CPU truth, and the host-side crossing replicas are pinned to the CPU oracle in `tile-painter/tests/barrier_screening.rs`). Burning barriers into the 30 m cover raster was the rejected alternative — it was measured acoustically unsound, the Legacy V1 §3.5a bilateral cadence (≥ ~30 m sample spacing) stepping over a one-cell-thin burned wall on most paths (mean +3.7 / max +13.8 dB under-screening at wall-adjacent shadow pixels vs the vector path; decision record: `tile-painter/tests/barrier_screening.rs`). With the gate OFF the GPU lane uploads no barriers and the C9 orchestrator gate (build-heatmap.sh / the world orchestrator) routes barrier-carrying R4s (~1.5% of hexes) to the CPU builders, as before; `QM_GPU_BARRIERS=0` is the explicit barrier-blind baseline (tests; A/B) — since 2026-08-02 the ON default lives in the ENGINE (owner-directed 2026-06-13, mean 0.002 / max 1.5 dB validated), so no launcher can lose it again.

**Screening is not computed standalone.** In current V1, raster buildings enter the Legacy V1 §3.5b composite top profile (`elevation + building_h`) while vector footprints and barriers enter as exact crossings, and diffraction is computed once by the §3.5 single-edge algorithm over the δ-winner (max-δ edge; the Rayleigh criterion gates only the unblocked arm). The per-band screening cap inherits from §3.5 — 20 dB — not a dedicated building-only cap.

The popup-facing `screening_attenuation` value returned by the engine is the increment of the combined result over bare-earth terrain diffraction, i.e. `atten_combined − atten_terrain` (clamped ≥ 0). With that definition `A_terrain + A_screen ≡ A_combined`, which is what §3.3 feeds into the ground/barrier combination. See Legacy V1 §3.5b for the motivating double-count problem the merge fixes.

### 3.7 Vegetation (ISO 9613-2:2024 A.2.2, Central Europe × 0.5 calibration)
```
A_veg,i = min(MAX_VEG_ATTEN[i], α_veg[i] × depth_m)
```
where depth_m = density-weighted forest depth along the source-receiver path: `Σ Δlength × forest[i]/100` with right-endpoint sampling over contiguous forested intervals (see Legacy V1 §3.5a for current V1). Runs whose PHYSICAL extent is shorter than 10 m are discarded to avoid scattered-tree false positives. On the current binary raster (0/100) this equals the plain run length bit-for-bit; continuous canopy-density tiles (geodata-v2 2a) scale each interval by its density.

Constants (`ALPHA_VEG`, `MAX_VEG_ATTEN`) are ISO 9613-2:2024 Table A.1 values × 0.5 — see
the constants block at the top of this SPEC. Rationale: binary WorldCover forest raster
treats any canopy ≥ 10 % as dense foliage; scalar compensates for over-application.

### 3.8 Urban reflection (ISO 9613-2 §7.5)
Per-RECEIVER boost based on building enclosure (`raster-reader::building_enclosure`):
3×3 probe at 75 m metric spacing (`ENCLOSURE_RADIUS_M`) around the receiver;
buildings taller than 5 m count.
```
density > 0.5 → A_refl = +3.0 dB;  density > 0.2 → +1.5 dB;  else 0 dB
```
Applied ONCE per receiver, not per source-receiver path. Maximum is 3 dB
(0 / 1.5 / 3.0 by probe density; the former `reflection.rs` clamp helper was
dead code and is deleted).

VECTOR MODE (geodata-v2 — ON by default since the Wave-1 cutover 2026-07-31,
commit 9cf166b; opt out only via `QM_VECTOR_BUILDINGS=0`, same `ENABLED` gate
as Legacy V1 §3.5b): the SAME nine probes, radius, height gate, and
thresholds, but each probe is an exact point-in-footprint parity test
against the obstacle store (`obstacle_index::enclosure_db`) instead of a
30 m raster cell read. The pipeline pre-bakes it into `rx_refl_db` per
receiver (`tile-painter::surface_region`); semantics at footprint EDGES
differ from the raster by up to one occupancy step — a deliberate,
flag-scoped representation change, quantified in the 1.9 A/B gate. The
POPUP swaps the probe with the store too (1.4b:
`VectorReflectionSampler` wraps the raster sampler once per query, so
every popup kernel — roads, rail, points, airport ground — reads the
same vector enclosure). The GPU line lane mirrors the CPU
(scatter.cu obstacle-candidate DDA + `single_edge_bands_cand`; the host
pre-bakes vector `rx_refl` before upload) with two BOUNDED deviations
documented at the kernel: ulp-level winner ties at f64-equal δ, and vertex
double-hits evaluated twice instead of deduped (identical bands in fp32).
e2-full run with `QM_VECTOR_BUILDINGS=1` is the parity gate (hard-fails on
zero-sided cells, a missing store, or a tile where no candidate fires).

Vector-mode raster residuals CLOSED by the bldgfix branch (2026-08-20):
the surface batch no longer ALSO bakes the raster 3×3 probe into `rx_refl_db`
(`TileBatch::build_opt_rx_refl(…, false)` — the vector pre-bake is the one
writer for painted tiles, so the raster bake was pure waste), and the
roads/rail group-histogram probe ("N of M segments had obstacles", popup
transparency only — no dB reads it) answers from exact footprint crossings
(`ObstacleSet::max_height_crossed`) instead of the raster cadence walk; the
raster walk survives only on the raster-fallback path. The all-or-raster
loader policy additionally treats a shard-less cell whose every overlapped
1-degree tile is listed in the world ingest manifest (`.ingested-tiles`,
`obstacle_ingest_coverage`) as INGESTED-EMPTY rather than missing — vector
mode proceeds without it, because our building raster derives from the same
Overture release and would contribute nothing there. Polar pentagons are
refused coverage (their vertex bbox is not longitude-conservative); a
building-raster restage from a NEWER Overture release must delete the
manifest in the same change (operational invariant, documented in
`obstacle_ingest_coverage`). Known remaining raster
consumer in vector mode: `PathProfile::building_h_m` feeds the composite
top profile that terrain diffraction (§3.5) rides — removing it changes
model semantics (buildings as topography) and is scheduled separately from
this branch.

### 3.9 Favourable meteorological conditions (CNOSSOS-EU §2.5.21)
✅ LIVE — `FAVOURABLE_MIXING = true` since 2026-07-28 (eb8a432; OUTPUT_VER
bump for the 5 surface layers in 0db-private 66b1d3ff; world repaint
pending the combined post-geodata-v2 wave — rollout record:
docs/dev/favourable-propagation-plan.md in 0db-private).

Mechanism (2015/996 formulas (2.5.9), (2.5.24), (2.5.25)), scoped to the two
terms where favourable/homogeneous physically diverge in the surface kernel —
ground and diffraction:

- For the max-δ edge, `compute_single_edge` also computes δ_F on the
  favourable curved ray: every straight chord is replaced by the arc of a
  circle with Γ = max(1000 m, 8·d_SR) (slant d_SR), arc = 2Γ·asin(ℓ/2Γ)
  (`curved_path_difference`). δ_F < δ for every geometry where diffraction
  still matters (the rare counter-shapes have δ deep in Maekawa saturation);
  at km-scale paths a sub-metre
  δ collapses to negative — the curved ray clears the hill (the audible
  distant-motorway-under-inversion mechanism).
- BOTH branches of (2.5.25) are implemented, selected on the STRAIGHT ray S–R
  as the standard prescribes: (2.5.26) `δ_F = ŜO + ÔR − ŜR` when the direct ray
  is broken, (2.5.27) `δ_F = 2ŜA + 2ÂR − ŜO − ÔR − ŜR` when it is not, with A
  the crossing of the straight S–R and the vertical through the edge
  (`curved_path_difference_near_miss`). The favourable ray is concave toward
  the ground, so it arches ≈`d²/8Γ` ABOVE the chord and a near miss is screened
  LESS, not more, in this state. The two expressions agree exactly at A = O,
  which is what keeps δ_F — and therefore the rule "a taller screen can never
  make a receiver louder" — continuous through the sight line
  (`arc_screening::taller_screen_never_makes_the_receiver_louder`, a live gate
  over 216 geometries × 33 heights).
- `diffraction_attenuation_mixed` then mixes the two Maekawa band
  attenuations energetically with `P_FAV = 0.5` (`mix_fav_hom`, (2.5.9)).
  Both states share every other chain term, so mixing the attenuation is
  identical to mixing received levels; with the single flat p it is also
  identical to per-period or Lden-level mixing.

Deliberate simplifications (review-pinned): the Rayleigh criterion stays on
straight geometry under the favourable state, and is asked ONCE on the
homogeneous δ for both states (§3.5 — asking it per state against a straight
δ* put a 3.13 dB step at δ_F = 0); edge selection stays max-δ on straight
geometry (second-order on multi-bump profiles); no per-period p (owner
2026-07-28); no Cmet. Aircraft does not take a favourable-condition shortcut
until an aircraft validation lane admits one.

### 3.10 Transport-specific adjustments
Applied in pipeline and popup:
- **Bridge**: G=0 (hard surface, overrides IMD raster)
- **Tunnel**: segment skipped entirely (sound contained inside) — the only unconditional drop
- **Access codes 2 (no) / 4 (legacy motor_vehicle=no)**: segment dropped UNLESS it carries measured AADT (`is_measured` + `aadt_light > 0`) — see the exception paragraph below (Neratovice case)
- **Oneway road**: AADT × 0.5 (approximation: half the traffic of two-way)
- **Junction**: speed capped at 30 km/h (junction code 1 = roundabout; mini-roundabouts (code 2) currently NOT capped — known gap)
- **Service railway** (yard/siding/spur): counts × 0.02
- **Parallel railway ways**: counts divided by `parallel_divisor`
- **Industrial exclusion radius**: R=√(area/π) — buildings within R of source point are not counted as screening (prevents self-screening from source's own footprint)

Road `access` and `road_class` u8 enums (codes, OSM mappings, AADT-reduction factors): see `engine/osm-extract/src/classify.rs` and the consumer in `engine/noise-compute/src/normalize/road.rs::access_factor`. The reduction is bypassed only when `Provenance::is_measured()` is true (City/National/Continental/GlobalMeasured); `NationalProxy`, `Heuristic`, `Baseline` and `None` rows still get access reductions.

`access=no` / `motor_vehicle=no` (codes 2/4) DROP the segment from emission entirely — except when the row carries measured AADT (`is_measured` + `aadt_light > 0`): a national census counting traffic on a "closed" road proves the flattened OSM tag hides an exception (bus lanes, destination modifiers, stale closures), so measured reality wins (Neratovice "Nádražní" case, 2026-07-10; ~500 such segments in CZ). Heuristic estimates never resurrect a closed road; tunnels are always dropped.

Link rationale (codes 10-12, `*_link` slip roads / ramps carry 15% of mainline AADT — HCM 7 / FEHRL / CERTU lower-range, validated against Pasito Blanco GC-1 popup): see `defaults.rs` ramp rows. `secondary_link` / `tertiary_link` stay on mainline codes (3/4) because their flow is closer to regular urban streets. For `highway=track` without a `surface` tag, the extractor defaults to `unpaved` (+2 dB rolling correction — §1 surface table).

### 3.11 Total received level per band
```
L_received,i = L_emission,i - A_div,i - A_atm,i - A_ground_or_barrier,i - A_veg,i + A_refl + FLC

where A_ground_or_barrier,i = max(A_ground,i, A_terrain,i + A_screen,i) if barrier exists
```

### 3.12 A-weighted total
```
L_A = 10 × log₁₀(Σ_i 10^((L_received,i + A[i]) / 10))
```

### 3.13 Audibility cutoffs (output-affecting)

Per-source maximum propagation radii (`constants.rs`) — beyond these a source
is not evaluated at all:

| Source | Max radius |
|---|---|
| Road (by class) | motorway 10 km · trunk 7 km · primary 5 km · secondary 3 km · unclassified 2 km · tertiary 1.6 km · motorway_link 1.2 km · trunk_link 900 m · residential 800 m · primary_link 600 m · service 500 m · living_street 400 m · track 300 m |
| Railway | current V1: per-row 25 dB free-field Lden, clamped [2 km, 10 km]; V2: [2 km, 11 km] after its reach solver lands (see below) |
| Industrial | 4 km |
| Building | fade radius from Lw, capped 2 km |
| Aircraft ground ops | runway 5 km · taxi 3 km · apron 1.5 km |
| Aircraft airborne/cruise | 16 km horizontal envelope + per-class slant reach (§5.1) |

**Rail reach is solved per row** (`emission/railway.rs::rail_reach_m`):
each segment reaches to the distance where its OWN free-field Lden falls
to `RAILWAY_REACH_TARGET_LDEN_DB = 25 dB` (bisection over log-distance, 40
steps in [100 m, 50 km]). Current V1 code clamps **[2 km, 10 km]**. When the
V2 reach solver lands, `RAILWAY_REACH_CLAMP_MAX = 11,000 m` and its halo-sizer
alias use the decided **[2 km, 11 km]** interval from §3.5f.3. The old blanket
7 km ("all types") is retired — a quiet branch line truncating at the same
distance as a 300 km/h corridor was a correctness bug. The current post-C1
`default_mainline_reach_post_c1` test is about 9.2 km; the V2 ground-aware solve is 10,178.8 m.
Quiet rows shrink, loud/HS corridors extend toward the applicable ceiling.
Known shared convention gap (documented on the constants): the solve is
free-field UNREFLECTED while kernels add receiver reflection (up to ~+5 dB) —
affects only the 25–30 dB fringe at facades; the 7 km blanket had the same gap.

Additionally a **free-field early-exit**
(`geo.rs::below_free_field_threshold[_line]`): when emission minus a
conservative divergence + atmospheric bound is already below the caller's
threshold, the full path computation is skipped. The current V1 premise that
path effects only attenuate is stale because literal hard ground can add up to
3.0 dB. V2 makes the gate conservative by adding `GROUND_GAIN_UB_DB` before
the comparison; only then is the skipped contribution provably inaudible.
Both gates trade the strict "compute at all distances" principle for speed;
radii are sized so the dropped contribution sits well below audibility for
that class's maximum plausible emission.

---

## 4. Lden (END 2002/49/EC)

```
Lden = 10 × log₁₀((12 × 10^(Ld/10) + 4 × 10^((Le+5)/10) + 8 × 10^((Ln+10)/10)) / 24)
```

Penalty: +5 dB evening, +10 dB night.

---

## 5. Aircraft

Aircraft splits into THREE public layers (separate tile trees + popup sub-tabs):
- **Airborne overflights** (`aircraft-airborne`): Doc 29-inspired empirical NPD model, terminal-area traffic
- **Airport ground ops** (`aircraft-ground`): runway / taxi / apron line sources propagated through Section 3 ISO 9613-2 path effects
- **Cruise** (`aircraft-cruise`): FL100+ overflight, Doc 29 NPD over per-R7 hex buckets

### 5.1 Airborne aircraft (Doc 29 4th Edition)

SEPARATE from ISO 9613-2. Airborne Doc 29 is empirical NPD-based, not path-tracing.

### Master equation (Eq. 4-8b)
```
SEL_seg = L_E(d_p) + ΔV + ΔI(φ) - Λ(β, l) + ΔF
```

- **L_E**: NPD lookup at slant distance d_p (feet). 124 per-typecode profiles auto-generated from EASA ANP v2.3, bucketed at 15 aircraft noise classes (10 Wing + 2 Fuselage + 2 Prop + 1 Helicopter — `NUM_CLASSES` in `profiles_generated.rs`). The kernel evaluates the **class anchor curve**, not the per-typecode curve (mean within-class spread ~0.8 dB); unknown typecodes route through a similarity table before falling back to `FALLBACK_PROFILE_IDX` (B738-equivalent). See `scripts/build-aircraft-profiles.py`.
- **ΔV**: Speed/duration correction (Eq. 4-14)
- **ΔI**: Engine installation angle correction (Eq. 4-15)
- **Λ**: Lateral attenuation (Eq. 4-18/19) — Wing-mounted jets only per Doc 29 §4.5.4 / FAA AEDT TM §6.2.4. Fuselage-mounted, propeller, and helicopter installations get Λ = 0 (gated by `installation` parameter in `fast_lateral_attenuation`).
- **ΔF**: Finite segment dipole correction (Eq. 4-20, full α/(1+α²) terms)

### Geometry (§4.4.1)
CPA (Closest Point of Approach) computed on segment EXTENSION (unclamped).
d_p = slant distance at CPA. β = elevation angle.

Energy uses the unclamped CPA everywhere (ΔF needs it). **Display** outputs
(popup Lmax, distance, altitude) use `clamped_display_cpa` — CPA clamped to
the observed endpoints — so curving departures don't show phantom near-passes
(2026-05-27 popup-honesty sweep). Lmax NPD lookup uses the clamped distance.

### Input and preprocessing

Non-obvious thresholds, periods, and routing rules (constants live in
`aircraft-extract`):

- **Typecode → NPD profile**: unknown typecode falls back to
  `FALLBACK_PROFILE_IDX` (B738/737800-equivalent).
- **`is_departure`**: per-sample-pair, ±5-sample smoothed ROCD median
  thresholded against Doc 29 §A.3.2 — climb > 500 fpm = Departure, and
  shallow cruise descents (`avg_alt > 10 000 ft && rocd > -500 fpm`)
  also use Departure NPD because en-route thrust ≈ T/O thrust.
- **Period**: segment-midpoint → IANA timezone (tzf-rs + chrono-tz,
  DST-aware) → END 2002/49/EC boundaries (day 07-19, eve 19-23,
  night 23-07).
- **Airport-ground candidacy** (Stage 1 `ground_inference.rs`, layered):
  the raw transponder ground bit is trusted only when AGL ≤ 80 ft (≈ 24.4 m)
  AND speed ≤ 140 kt AND |baro rate| ≤ 2000 fpm; without the bit, a surface
  signature (AGL ≤ 30 ft ≈ 9.1 m, speed ≤ 90 kt, |baro rate| ≤ 1200 fpm) can
  mark ground; strong-airborne neighbours (AGL ≥ 165 ft OR speed ≥ 130 kt)
  veto edge points within a 32-point window. (The old "60 m AGL both
  endpoints" predicate survives in `segment_filters.rs` as dead code.)
- **Stale-ground filter**: `on_ground` or `≤ 15 m AGL` with no airport
  context → dropped.
- **Derived-speed plausibility**: implied `length_m / dt_s >
  MAX_PLAUSIBLE_SPEED_KT` (1500 kt) dropped as mode-S decode errors.
  A 200 km/30 min oceanic gap survives; 200 km/30 s does not.
- **Phase-aware gap budget**: Cruise 3600 s (oceanic dropouts OK),
  Airborne 120 s (terminal area should be dense), Ground 60 s.
- **Phase priority at transitions**: Airborne wins over Ground and
  Cruise so takeoff / flare stays audible and Cruise→Airborne descents
  get the approach NPD, not forced-Departure cruise routing.
- **Flight split**: `split_flights` splits the trace at on-ground rests
  ≥ `MIN_TURNAROUND_S` (5 min); airborne dropouts of any duration
  preserve `flight_id`.

### Data-quality gating (post-K3 split)

Since the v16/K3 refactor the gates are split by stage; the historical
`is_valid_airborne_segment` bundle is **no longer applied to airborne
segments** in either popup or heatmap (it survives in `segment_filters.rs`
as dead code; cruise checks run through `is_valid_airborne_with_terrain`).

Enforced at Stage 1 (extract) for airborne pairs:
- **endpoint AGL**: `start_agl < -30 m` or `end_agl < -30 m` rejected
  (DEM-relative, so subsea-level airports like Schiphol/Atyrau pass)
- **impossible jet speed**: `speed_kt < 80` for jet classes
  (`IS_JET[noise_class]`; Turboprop / LightGA / Rotorcraft exempt)

Enforced only for cruise synthetic segments (`cruise.rs`):
- **midpoint underground**: `max(start_alt, end_alt) < midpoint_terrain - 30 m`
- **line goes under terrain**: 25%/75% interpolated samples ≤ terrain - 30 m
- **jet too low**: `max_alt < midpoint_terrain + 150 m`

⚠ **Known gap (under review)**: the chord (25/75) and jet-too-low checks were
dropped for airborne segments during K3 with the intent to re-add them in
Stage 2A (`stage_1.rs` comment); Stage 2A never received them, and six stale
comments cite a non-existent `airborne_chord_clears_peaks`. Filter D (below)
covers part of the exposure.

### Filter D — per-receiver sub-terrain extrapolation rejection

`compute_cpa` uses the infinite-line (unclamped) CPA for all outputs (Doc 29
§4.4.1). Filter D then rejects (segment × receiver) pairs whose CPA foot falls
outside the observed endpoints AND whose extrapolated altitude lies > 30 m below
terrain. Airport-ground segments bypass the filter. Full rationale (geometry,
30 m margin, replaced-blanket-filter history) lives in the rustdoc on
`segment_energy_kernel` (`emission/aircraft/doc29.rs`).

### Shared kernel approximations

`compute_aircraft_v6` runs the same Doc 29 kernel
(`segment_energy_kernel` in `aircraft/doc29.rs`) on every airborne sub-
segment and every cruise synthetic segment; see its rustdoc for the
four approximations (NPD via 128-bin LUT, ΔF via `fast_delta_f`, Λ via
`fast_lateral_attenuation`, ΔI via inline `u²/v²`) and the < 0.15 dB
per-segment combined error. ΔF applies at every slant range — the
CFFK far-field fast path (slant > 7.62 km) keeps Λ and ΔI skipped but
retains ΔF so that N collinear per-sample sub-segments correctly sum
to one event (regression: `cffk_partition_preserves_linear_energy` in
`doc29.rs`).

Output-affecting kernel gates: segments whose SEL lands **< 20 dB** return no
energy (`doc29.rs`, both CFFK and full paths); per-class × direction slant
**reach envelopes** at the 40 dB NPD threshold (`REACH_SQ_TABLE`, `aircraft/npd/mod.rs`)
reject far geometry before evaluation; NPD lookup distance floors at 100 ft.
The heatmap additionally routes far receivers through a coarse lattice
(`NEAR_SLANT_M = 500 m` exact zone, coarse bands at 2 km / 8 km) — heatmap-only
approximation; the popup is always exact.

### Cross-flight aggregation

Stage 2A/2B/2C produce three per-R4 popup arrows: `airborne.arrow`
(per-flight sub-segments with bbox envelope + per-pair period/date_id/
flags), `cruise.arrow` (per-R7/FL-bin/class/period bucket; v14 carries a
bounded `top_candidates` list + scalar `unique_count` instead of the old
full `cruise_flight_ids` — tail flight ids undercount band counters;
annual-only — no `date_id`), and
`airport_traffic.arrow` (sparse per-microsegment counters — see §5.2).
Schemas in `aircraft-extract/src/arrow_schemas.rs`. Consumers:
`compute_aircraft_v6` (popup), `tile-painter`, and `noise-gpu` read the
same arrows.

### Cross-hex visibility

Popup loads target R4 + 6 ring-1 neighbours so R4-straddling rows stay
visible. Per-row prune radius: `AIRCRAFT_MAX_HORIZONTAL_REACH_M = 16 km`
(in `aircraft/npd/mod.rs`); airborne uses baked per-row bbox, cruise uses
R7-cell-centre + half-diagonal. Antimeridian-crossing rows skip the
bbox prune (degenerate global envelopes).

### Per-period energy
```
E_period = Σ_segments_in_period 10^(SEL_seg / 10)
```

### Per-period Leq (§5, Eq. 5-1)
```
Leq_period = 10 × log₁₀(E_period / (n_days × T_period))
T_day = 43200s, T_evening = 14400s, T_night = 28800s
```

**GA 365-day hybrid weighting**: every airborne/ground row's energy AND
movement count is additionally scaled by a per-class weight
`w[c] = n_days / sample_days_by_class[c]`
(`emission/aircraft/npd/mod.rs::ClassWeights`) before the `÷ n_days`
above. Airline classes are sampled over the same `n_days` window (w = 1);
GA classes (`PROP_C172`, `HELICOPTER`) are sampled over **365 days**
(w = n_days/365 ≈ 0.033 at n_days = 12), so a one-off GA flight
contributes 1/365 of its energy instead of 1/12 — kills the +14.8 dB
Kytín phantom while leaving genuinely-daily GA patterns unchanged. The
per-class day vector rides in arrow metadata (`sample_days_by_class`);
parsing FAILS LOUD on pre-hybrid arrows (no uniform fallback —
re-extract). GA movement counts divide by `ga_n_days`; the popup exposes
`sample_days` / `ga_sample_days` transparency fields.

### Octave bands
**Airborne / cruise**: ❌ BROADBAND ONLY. Doc 29 NPD returns a single SEL
value; per-band data is not fabricated for the airborne kernel. Path
effects (Section 3) skip the per-band chain — the airborne kernel uses
only NPD lookup + Δv/ΔI/Λ/ΔF and lateral attenuation, never per-band
terrain / screening / vegetation.

**One optional exception (C2)**: receiver terrain-horizon screening
(`emission/aircraft/horizon.rs`) — ISO 9613-2 §7.4 single-edge Dz over a
32-sector × 6-range-band quantized DEM horizon, applied AEDT-style as
`max(Dz, Λ)` (LOS blockage and lateral attenuation are mutually exclusive,
never summed), capped at 18 dB, broadband (λ_eff = 0.685 m ≈ 500 Hz).
Default OFF (byte-identical output without it); popup-only behind
`QM_AIRBORNE_HORIZON=1`; cruise is structurally exempt.

**Ground ops**: 8-band emission via per-class spectrum templates
(`GROUND_OPS_RUNWAY_SPECTRUM_SHAPE` / `GROUND_OPS_TAXI_SPECTRUM_SHAPE` /
`GROUND_OPS_APRON_SPECTRUM_SHAPE` in `aircraft/ground_ops.rs`). Popup
builds the Section-3 path-effect variants inline per row from a
per-microsegment path cache (same 6-variant contract as
`propagate_variants_full`) on every band — see §5.2.

### Per-event peak Lmax (informational only)

```
Lmax_event = lookup_lmax(class_idx, is_departure, log10(d_display_ft))
```
where `d_display` is the clamped display CPA distance (see Geometry above).

Per-class LAmax NPD LUTs ingested from the ANP CSVs alongside the SEL
NPDs (`build_lmax_lut` in `emission/aircraft/npd/mod.rs`). 200–25 000 ft
log-linear interpolation; below 200 ft extends the first-two-point slope;
above 25 000 ft extrapolates as `−20·log10(d/d_ref)` + per-profile
`alpha_eff` (same fit as SEL — LAmax and SEL share source spectrum).
Popup `peak_lmax` per flight = max across segments.

**Dropped vs full Doc 29 Eq. 4-12**: ΔI and Λ are NOT applied to
`Lmax_event` — only the NPD curve. Doc 29 applies them per segment; the
residual at low elevation angles for wing-mounted jets is < 2 dB,
acceptable for informational peak ranking and avoids a second kernel
pass.

**Sources**: ECAC Doc 29 4th Ed §A.3.1 (LAmax NPDs), FAA AEDT Tech
Manual §6.4 (LAmax interpolation), EASA ANP v2.3 + v9 supplement
L_MAX_A / L_MAX_D tables.

### 5.2 Airport ground ops (per-microsegment model, `airport_traffic.arrow`)

Separate submodel inside the aircraft layer. Inputs: Stage-1
`Phase::Ground` segments + OSM aeroway microsegments
(`airport_lines.arrow`, ≤ 250 m pieces) + aerodrome polygons
(`airport_areas.arrow`) + Stage-1.5 DBSCAN synthetic strips for
OSM-missing airfields.

**Row semantics** (`airport_traffic.arrow` v7, keyed by `airport_key,
osm_id, segment_idx, ops_kind, is_departure, veh_kind, class_idx,
period`):

- `band_energy_lin[8]`: linear Z-weighted source energy, raw Σ over
  n_days. Storage units branch on `veh_kind`:
  - aircraft (`veh_kind = 0`): density-weighted per-metre `LW'`
    (= LW'_lin × `overlap / line.length_m`). Refinement-invariant by
    Chasles theorem at receiver.
  - GSE (`veh_kind = 1`): per-event SEL@25m from the kinematic
    moving-point integral over `length_within_segment_m`.
  Consumer divides by `n_days × period_seconds` via `period_leq`.
- Scalar `unique_*_count` columns: distinct `flight_id`s touching this
  row, segmented by ops_kind/is_dep/veh_kind/class_idx.
- Row-replicated `microseg_unique_*` columns: UNION across all rows
  sharing the same `(osm_id, segment_idx)`.

**Leg-to-microsegment projection**: buffer each microsegment by
`AIRPORT_LINE_SNAP_BUFFER_M = 50 m` perpendicular; the leg's overlap
inside that rectangle contributes. When a leg covers multiple segments
their overlap lengths are renormalised **downward** when Σ overlap exceeds
the leg length (prevents +3 dB inflation on adjacent parallel taxiways; legs
partially off the network keep Σ < leg length). Legs that
miss every microsegment are dropped (no fallback emission); coverage
for OSM-missing airfields comes from the Stage 1.5 DBSCAN synthetic
strips below.

**`ops_kind` from OSM** `aeroway_type` (`ops_kind_from_aeroway` in
`airport_traffic_writer.rs`): runway / stopway / airstrip →
`runway_roll`, taxiway → `taxi`. Aprons are area features (in
`airport_areas.arrow`), not lines, so the writer doesn't emit
`apron_movement` rows; any other aeroway value is corrupt input and
skipped. No speed classifier — OSM geometry is the source of truth.

**Emission kernel** (`emission/airport_traffic.rs`):
- Aircraft (`compute_aircraft_lw_per_meter_lin`): per-class anchor
  `GROUND_OPS_REFERENCE_LW_PER_METER_DB[class][ops_kind]` (= legacy
  1 km event-SEL anchor + `+9.01 dB = 10·log10(25/π)`; taxi = runway
  −12 dB, apron = −18 dB); dwell speed adjust `−10·log10(v/v_nom)`
  (Doc 29 Eq 4-14: per-metre energy ∝ 1/v, slower → louder), ±3 dB
  clamp vs nominals 70/18/12 kt, applied only when `speed_kt > 1`;
  runway departure
  +2 dB (Doc 29 §A.3); spectrum shape per `ops_kind`; source height
  4 m. Returns per-metre `LW'` density.
- GSE (`compute_gse_band_energy_lin`): closed-form kinematic
  moving-point integral over `segment_length_m` at perpendicular
  distance 25 m. One term replaces stationary-Lp + duration + FLC.

**Receiver math** (`compute/aircraft_v6/airport_traffic/mod.rs`):
- Aircraft rows: `received_band_lin[i] = row.band_energy_lin[i] × (θ /
  d_perp_extended)` per CNOSSOS-EU §2.5.5 line-source formula, with
  signed-fraction `θ` math so receivers past either endpoint get the
  correct (small) angle.
- GSE rows: `received_band_lin[i] = row.band_energy_lin[i] × (25 /
  d_endpoint)` (point-source divergence from the 25 m anchor).
  ⚠ Known issue: the kinematic model's own far field decays ~1/d²; the
  1/d scaling over-estimates GSE at range (~12 dB at 1.5 km for a 250 m
  segment). Bounded by GSE being ≪ 5 % of microsegment energy and the
  ground-ops reach caps.
- Both apply shared CNOSSOS-EU §2.5 path effects (atmospheric, ground,
  terrain, screening, vegetation, reflection). Variants are built inline
  per row from a per-microsegment path-effect cache. `d_perp` /
  `d_endpoint` are floored at half a z13 pixel for heatmap parity
  (popup would otherwise read ~5-8 dB hot directly on the line).
  Per-`ops_kind` reach caps: runway 5 km, taxi 3 km, apron 1.5 km
  (`ground_ops_max_radius`, shared popup + heatmap; see §3.13).

Airport-level `arrivals_per_day` / `departures_per_day` come from
HashSet UNION over `flight_ids` — one rotation crossing 30 microsegments
counts once per direction. The UNION runs at extract time (per-R4
`airport_summary_parts/` → global reduce → `airport_summary.arrow`
sidecar); only RUNWAY_ROLL touches count toward arr/dep. The popup reads
the sidecar and **refuses** (renders zeros / "—") when it is missing —
per-row re-summing is forbidden (would over-count ~N×).

**Stage 1.5 DBSCAN auto-discovery**: miss-snap ground vertices
clustered (eps = 200 m, min_samples = 5). Accepted clusters emit
synthetic strips into `synth_airport_lines.arrow` /
`synth_airport_areas.arrow` sidecars, unioned with real OSM lines in
the Stage 2C cache (consumed identically). Clusters near an existing
airport reattribute to the **real** airport key; standalone ones get
`airport_key = "auto-<R11-hex>"`; lines with no aerodrome within range
get `strip:<R7>`. Accept gates: line-shaped, ≤ 4 km, ≤ 20 k vertices;
GSE vertices excluded.

### 5.3 Aircraft contribution to confidence

Confidence rubric in `confidence.rs::assess`. Aircraft quirk:
`compute_at_point_inner` runs the rubric with `has_aircraft = false`
because the popup arrows aren't visible at that stage; the bump and
note removal are applied post-merge by
`source-reader::aircraft_v6::add_v6_aircraft_to_result`.

### 5.4 Per-receiver SegmentTrace breakdown (popup output)

`aircraft_subtype: u8` splits the popup into three sub-tabs:
1. **Ground** — one trace per (airport microsegment × `ops_kind`); geometry = microsegment polyline.
2. **Airborne sub-segment** — one per Stage 2A sub-segment.
3. **Cruise R7 hex** — one per crossed R7 cell (hex polygon).

`received_lden.full` is per-segment: per-period energy / `(n_days ×
T_period)` through the standard `variants_to_lden` mix. Energy-summing
the visible segments approaches source-aggregate Lden modulo per-sub-tab
top-K cap. Path-effect variants (`no_terrain`, `no_screening`, …) stay
zero — aircraft propagation does not expose them per-event — so the
popup gates path-effect detail rows for `aircraft_subtype` 2/3.

`apply_segment_top_k_with_cap` (`source-reader/src/lib.rs`) budgets
top-K separately per `aircraft_subtype` so a loud sub-tab (e.g. ground
ops) can't crowd quieter sub-tabs out.

Airborne sub-segments with event `Lmax < 25 dB` are skipped at trace level
(`AIRBORNE_TRACE_CUTOFF_DB` — display-only; period energy still accumulates,
and an `airborne_above_cutoff` counter feeds the truncation flag).

---

## 6. Industrial Emission

### Source geometry
Discretized at load/query time in `normalize::prepare_industrial_points`
(shared by popup and heatmap — parity by construction):
- **Wind turbine** (`source_type = 10`): single point at centroid
- **Non-wind, area ≤ 5000 m²**: centroid
- **Non-wind, area > 5000 m² and polygon available**: **75 m square metric
  grid, area-weighted** (`wkb::wkb_area_grid_points`, 5×5-subsampled cells;
  cell areas renormalised so Σ = polygon area)
- fallback to centroid if polygon/grid generation fails
- Energy split per point: `Lw_point = Lw_total − 10×log₁₀(area_total /
  area_point)` — area-weighted; equals −10·log₁₀(N) only for equal cells
- Sources with resolved Lw < 10 dB are dropped; missing area defaults to
  10 000 m² (at that default, Lw = baseLw + a_weighted_total(spectrum) under
  the spectral-debt restore)

Each discretized point also carries:
```
R_excl = √(area_per_point / π)
```
R_excl both suppresses self-screening (§3.6) and floors the propagation
distance (`geo.rs::effective_area_source_dist` — prevents the 1/r²
singularity for receivers inside the source polygon).

### Emission
```
Lw = baseLw + a_weighted_total(spectrum) + 10 × log₁₀(clamp(area_m², 100, cap) / 10000)
```
`cap = sector_area_cap_m2` (Fix B, 2026-07): 50 ha default; **300 ha** for heavy
divisions that radiate across their whole footprint (coal/mining 05|08,
coke+chemicals 19|20, cement 23, metallurgy 24) and their OSM subtypes
(quarry/chemical/cement/steel) — a 538 ha steelworks is no longer clamped to
50 ha. Power (NACE 35) stays 50 ha (concentrated source); the I-04 area-density
model was evaluated and rejected. The `a_weighted_total(spectrum)` term is the
**C1 spectral-debt restore** (2026-07): it adds the exact per-profile scalar
into Lw, so — via the normalization invariant below (`a_weighted_total(bands)
== Lw`) — the radiated dB(A) is lifted by that scalar, recovering the SHM-era
level WITHOUT reverting normalization. This closed the −4.9..−6.4 dB undershoot
described next.

**Normalization invariant** (audit 2026-06 B4+B6, `emission/spectrum.rs`):
emission bands are `Lw + spectrum_i − a_weighted_total(spectrum)`, so
`a_weighted_total(bands) == Lw` — Lw IS the radiated dB(A) total.
Pre-fix the relative spectra silently added +4.9..+6.4 dB(A) per profile
family (audit I-03); effective industrial emission dropped by exactly that
much. Test-locked at 1e-9 for every profile.

baseLw from a resolution chain — NACE 4-digit → NACE 2-digit → OSM
`site_subtype` (12 profiles) → `source_type`. Values were authored against
Czech SHM 2022 while the bands still carried the hidden spectrum surplus. The
2026-06 normalization removed it (a −4.9..−6.4 dB undershoot); the C1
spectral-debt restore (2026-07, above) adds it back, so these baseLw values now
radiate ~as authored. A residual per-sector calibration (C2) was measured NOT
warranted — near-plant matches official maps in 3 countries (Wave 2 finding):
- Heavy industry (cement, steel, mining, quarry): 99-100 dB
- Power: thermal (NACE 3511) 97 dB, hydro 90 dB, solar (synthetic NACE 3599) 55 dB
- Medium industry (chemical, food, works): 88-95 dB
- Light industry (warehouse, commercial, farm): 55-86 dB (office/commercial 60)

Every profile carries `evening_offset` / `night_offset` (e.g. quarry −20 dB
night, wastewater 0 = 24/7, school −25 dB night); wind turbines are flat 24/7.

Area scaling clamped to [100 m², cap] (50 ha default / 300 ha heavy — see
`sector_area_cap_m2` above) to prevent OSM polygon artifacts.

### Source height
- quarry (`source_type = 1`): 8m
- heavy industry (NACE divisions 05/08/23/24/35 — coal mining, mining & quarrying, cement/minerals, metallurgy, power): 10m
- other industrial: 5m
- wind turbine: `hub_height`

### Wind turbines (IEC 61400-11)
```
LwA = rating_lookup(rated_power_kw)
```
Published max LwA is a flat band, nearly independent of rating across
1.8–6.6 MW (audit I-10 — per-type sources cited in `wind.rs::turbine_lw`):

| rated power | LwA | anchors |
|---|---|---|
| < 1 MW | 98 | pre-audit value kept |
| 1–2 MW | 104 | V90-2.0 = 104.0, E-82 E2 = 104.0 |
| 2–3 MW | 105 | E-92 (2.35 MW) = 105.0 |
| 3–5 MW | 106 | V112-3.0 = 106.5, N149 = 106.1 |
| ≥ 5 MW | 106.5 | N163 = 106.4, E-160 = 106.0, V150-6.0 = 104.9 |
| unknown (0) | 105 | mid-band |

Spectrum: [-2, -1, 0, 1, 1, 0, -2, -5] dB relative, normalized so
`a_weighted_total(bands) == LwA` (pre-fix the shape added +6.4 dB(A)).

Fallbacks + tag-error clamps (audit I-10b):
- `hub_height` default = **105 m** (known-data median in our arrows;
  context: WindGuard DE 2024 avg 143 m, LBNL US 2023 avg 103.4 m);
  clamp ≤ 175 m (4,792 OSM hubs >170 m are tag errors)
- `rated_power_kw` default = **2000 kW**; values > 8000 kW treated as
  unknown (23 OSM powers ≥20 MW are tag errors)

### Propagation
ISO 9613-2 point source (point-source divergence; distance floored at
R_excl). Max radius 4 km (§3.13); popup queries sources within 5 km.

---

## 7. Settlement (buildings)

### Source geometry
Discretized at load/query time in `normalize/points.rs::prepare_building_points`
(shared by popup and heatmap) via the shared area discretizer
`discretize_area_source` — ONE mechanism for building / industrial / leisure:
- small / missing-polygon buildings: centroid
- buildings with `area > 2000 m²` and polygon available: interior grid at **30 m** spacing
- if grid generation yields only one point, fallback to centroid
- energy split is **area-weighted**, identical to industrial:
  `Lw_cell = Lw − 10×log₁₀(area_total / area_cell)` (energy-conserving);
  each cell carries a self-screening exclusion radius `√(area_cell/π)`
- missing area defaults to 100 m²; missing height → floors × 3 m → 8 m;
  missing floors → ceil(height / 3); sources with Lw < 10 dB are dropped
- **shed-type classes scale on FOOTPRINT, not floors**
  (`settlement.rs::is_shed_type` — warehouse/factory, church, farm,
  FOOD_RETAIL, HOSPITALITY): a tall single-story hall must not mint
  `height/3` phantom floors, and ground-floor activity (kitchen,
  refrigeration) must not multiply up a 6-floor block

Each source gets a fade-out radius solved from its **honest radiated Lw**
(`settlement.rs::building_max_dist`), capped at **2 km** (popup query
radius 2 km); test `building_cull_radius_matches_honest_lw`.

### Emission (custom model, NOT standardized)
```
Lw = 10 × log₁₀(10^(Lw_fixed/10) + GFA × 10^(Lw_per_m²/10))
where GFA = area_m² × floors   (footprint only for shed-types, see above)
```
**14 building classes** (settlement v2 phase 2, 2026-06-12): phase-1
classes 0–9 (residential, commercial, warehouse, school, hospital, church,
hotel, garage, farm, public) plus `SILENT` (10 — sheds/roofs/huts, Lw 0;
kills the ~18 M phantom residential emitters), `HOUSE` (11 — split from
apartments, gentler −8 night cut), `FOOD_RETAIL` (12 — 24/7 rooftop
refrigeration, night −2 dB instead of −20), `HOSPITALITY` (13 — kitchen
extract + evening voices). Class ids are byte-stable (the arrow stores the
raw u8 — renumbering would silently re-profile every cached building).
Type / geometry / area resolution chains live in `settlement.rs`.

Constants are **honest measured-anchored dB(A)** (settlement v2 phase 1,
2026-06-12): the pre-v2 heuristic values and their W7 net-zero spectrum
compensation are GONE — do not reintroduce offsets; band normalization
happens once in `spectrum::normalized_emission_bands`, and
`a_weighted_total(bands) == Lw` is pin-tested by
`radiated_dba_equals_lw_for_all_classes`. Per-class anchors cite measured
plant/activity literature inline (`settlement.rs::building_profile`);
settlement v3 re-tuned residential `Lw_per_m²` 21→25 and warehouse/factory
21→45 (fixes a ~30 dB standalone-factory undershoot). The C8a
recalibration backlog now applies to **industrial only** (§6).

### Source height
height/2 (mid-facade). Consistent in emission AND propagation (fix V33 mismatch).

### Propagation
ISO 9613-2 point source.

### 7a. Leisure areas (sport / play / open-air hospitality)

Settlement v2 phase-2 sibling family for OSM features that carry no
`building=*` (`emission/leisure.rs`): 8 classes — generic pitch, padel,
tennis, basketball, playground, pool, outdoor seating (biergarten /
`outdoor_seating=yes`), stadium. Own `leisure.arrow`;
`normalize/points.rs::prepare_leisure_points` discretises through the
shared area-weighted discretizer (industrial 75 m grid + `√(cell/π)`
self-screening). Differences from buildings are physical only: source
height ~1.5 m (voices/rackets, not roof plant), no floors, reach
`min(Lw fade radius, 2 km)`.

Levels use the SAME area-law as buildings (`settlement::area_lw` over the
polygon area; a node with no polygon falls back to the profile's reference
footprint). Each class anchors an ACTIVE measured sound power (cited
inline; padel 90 dB(A) is the loudest — the 2024–26 complaint class) minus
a transparent annualization cut (season −3 dB, daily duty −6 dB; pool −5
season; stadium −12 match-days-only) — END/CNOSSOS has no sport model, so
the annual-Lden convention is ours and its assumptions are listed on
/about. `PROP-MEAS`-flagged classes (playground, pool) ship conservative
placeholders, never presented as measured. Rendered within the
**building** layer (`LEISURE_TYPE_BASE = 100` id offset,
`types/source_names.rs`).

---

## Reference Test Vectors

| Test | Input | Expected | Source |
|------|-------|----------|--------|
| K1 | Cat1, 50 km/h, asphalt, 10000 AADT, day | 79.11 dB(A)/m | CNOSSOS-EU |
| K2 | Cat3, 80 km/h, cobblestone (+4dB), 500 AADT, day | 80.07 dB(A)/m | CNOSSOS-EU |
| K4 (V1 legacy) | Propagation 100m, G=0, line source | 25.56 dB attenuation | ISO 9613-2 |
| K5 (V1 legacy) | Propagation 100m, G=1, line source | 31.66 dB attenuation | ISO 9613-2 |
| K6 | Single barrier 50m, δ=0.5m, G=0 | 15.28 dB barrier atten | ISO 9613-2 |
| K7 | removed — the double-edge band math was deleted 2026-07-03 (single-edge only; K6 covers Maekawa) | — | — |
| K8 | Lden: Ld=60, Le=55, Ln=50 | 60.00 dB | END 2002/49/EC |

**K4 was 28.58 dB until 2026-08-05, and that figure pinned a bug rather than
the standard it cites.** It is exactly 25.56 + 3.00: the engine formed
`A_ground = GROUND_CF[i] · G`, which is 0 dB at G = 0, while ISO 9613-2
Table 3 (`As + Ar = −1.5 − 1.5`) and CNOSSOS-EU 2015/996 (2.5.15) — *"if
Gpath = 0: Aground,H = −3 dB"* — both put the hard-ground term at −3 dB in
every band. Quoting the old number as an ISO reference made the omission look
verified. The floor correction landed first in the old `CF` surrogate; surface
sources now use the literal §2.5.14--2.5.20 formulation in §3.3, pinned
band-by-band against the official TC01/TC02/TC03 fixtures.

---

## Research Archive

Provenance for the numbers sprinkled through the atlas that are **not**
direct standard quotes. Each row is either a cited external source or an
explicitly-pragmatic heuristic with a one-line rationale. Target audience
is the next reviewer who asks "where did this number come from?".

### Trip generation per dwelling (per-country, 2026-07)

The service-tree enricher multiplies residential DWELLINGS by
`vehicle_trips_per_occupied_dwelling` from the per-country fleet table
(`scripts/country-fleet.json` → generated
`pipeline/lib/country-fleet.generated.ts`). The WORLD terminal of the
cascade is **3.68** = 4.0 base × 0.92 occupancy (OECD HM1-1 vacancy) —
bit-identical to the pre-2026-07 global `TRIPS_PER_DWELLING`, so countries
without a table row behave exactly as before.

Quantity definition (one per row, /gg 2026-07 review — the earlier table
mixed person-trips, car availability and vehicle-trips): rows are entered
as **motor-vehicle trips per OCCUPIED dwelling (household) per day, both
directions, annual average, all vehicle classes including motorcycles** —
the surveys' native unit — and the generator multiplies by 0.92 stock
occupancy (OECD HM1-1) because the trip model counts GFA-estimated STOCK
dwellings; the compiled `tripsPerDwelling` is per stock dwelling. Per-row
source/year/confidence live in the JSON; curated examples: US 3.5 (FHWA
NHTS 2022 Summary of Travel Trends tab. 2-8 — the often-quoted 5.9 is the
2017 vintage), DE 3.9 (MiD 2017), JP 2.5 / KR 2.9 (PT/KTDB surveys),
continent bands for the rest (EU 3.7, Asia 2.5, Africa 1.5, SA 2.2).
Clamp [0.8, 6.0]. Every Natural Earth country carries a row (continent
band when no better source), so the country → continent → WORLD cascade
is materialised at generation time.

### Daily trip generation per building (ITE rates, 2026-07)

`pipeline/lib/trip-rates.ts::estimateBuildingLoad` maps building geometry
to `{dwellings, trips}` — residential arms return dwellings (× country
trips/dwelling above), every other arm returns **vehicle trips/day
directly** with an EXPLICIT ×0.3 ITE manual→field damping (ITE measures
US suburban auto-oriented sites; VTPI's TDM encyclopedia documents 2–4×
overestimation elsewhere). Basis: GFA = footprint × floors, except
`footprint` for ground-floor-activity classes typed from interior POIs.

| Class | ITE code | Basis | Rate | Min–Cap (trips) |
|---|---|---|---|---|
| 0 apartments | 221 | GFA → 1 dw / 80 m², cap 200 dw | ×tpd | — |
| 11 HOUSE | 210 | GFA → 1 dw / 120 m², cap 4 dw | ×tpd | — |
| 1 commercial/office | 820+710 ×0.3 | GFA | 12 / 100 m² | 5–3 000 |
| 2 industrial bldg | 110 ×0.3 | GFA | 1.6 / 100 m² | 4–1 500 |
| 3 school (staff-only) | 520 | GFA | 0.46 / 100 m² | 4–368 |
| 4 hospital | 610 | GFA | 33.5 / 100 m² | 20–1 104 |
| 5 church | 560 peak-only | fixed | 7.36 | — |
| 6 hotel | 310 ×0.5 occ ×0.6 car | GFA | 9.7 / 100 m² | 8–1 472 |
| 7 garage (+parking) | — | fixed | 3.68 | — |
| 8 farm | — | GFA | 1.84 / 100 m² | 2–184 |
| 9 civic | — | GFA | 1.23 / 100 m² | 2–368 |
| 10 SILENT | — | — | **0** | — |
| 12 FOOD_RETAIL | 820+850 ×0.3 | footprint | 30 / 100 m² | 20–20 000 |
| 13 HOSPITALITY | 932 ×0.3 | footprint | 30 / 100 m² | 15–2 000 |

Intentional 2026-07 golden deltas (pinned by
`pipeline/lib/trip-rates.test.ts`): classes 10–13 previously fell into the
residential default — SILENT sheds/roofs (~18 M footprints) generated
phantom trips, and hospitality/food-retail (the Thailand Krabi owner
report: guesthouse+restaurant roads at 3 moto/day) were billed as houses.
Commercial was raised 4→12 trips/100 m² — the old divisor sat ~10× below
its own cited ITE 820 rate. Classes 3/4/6/8/9 are numerically restated
(continuous rate instead of per-building `ceil`, same asymptote and caps).

### Local-road vehicle mix (per-country fleet table, 2026-07)

`splitAADT` writes medium 1 % / heavy 2 % (world-constant — no per-country
signal for local roads) and **moto from `moto_traffic_share`** in the same
fleet table; light is the exact remainder. WORLD moto = 1 % (pre-2026-07
bit-preserved).

`moto_traffic_share` is a TRAFFIC share, not ownership: registered-fleet
shares (WHO GSRRS 2023 country profiles, "powered 2-/3-wheelers") are only
a prior, mapped by a continent-calibrated usage factor (Asia 0.40 — TH
ownership 0.517 vs DOH-calibrated traffic 0.20; Europe/NA/Oceania 0.15 —
CZ ownership 0.20 vs ŘSD-measured ~0.03; SA 0.20; Africa never derives —
registration is blind to informal boda-boda/okada fleets) and clamped to
[0.01, 0.45]. National-enricher-calibrated overrides win (TH 0.20, IN
0.30, NG 0.30, EG 0.18, KE 0.20), plus curated rows where the Asia band
would mislead: car-dominant East Asia (JP 0.02 MLIT, KR 0.03, CN 0.05 —
urban moto bans, e-bikes are not cat-4), scooter-dominant TW 0.35 (MOTC,
no WHO profile), and no-breakdown SE Asia (KH/LA/MM 0.30, VN/TH neighbour
analogy). Interior hexes resolve their country
via `h3r4-admin.bin` (built BEFORE the road passes — fail-closed);
border hexes per segment midpoint through CGAZ polygons
(`pipeline/lib/hex-country.ts`). The audit gate's R2 moto-scramble rule
keeps its strict global form; the service-tree dataset declares
`highMoto` because its split is computed, not column-mapped.

### Cascade defaults (city → country → continent → world)

`engine/noise-compute/src/defaults.rs` implements a four-level cascade.
Each non-world row comes from an actual national enricher table (spatial
or ref-level data) converted to a class-default tuple; the world row is
an EU-generic fall-back.

| Level | Source table | Derivation |
|---|---|---|
| City São Paulo / Rio (class 0) = 100 000 | BR `CLASS_AADT` × `tierMultiplier(1)` × `splitVehicles(tier=1)` | `pipeline/enrich-roads-br.ts` (DNIT 2023 federal AADT estimates + IBGE metro tier) |
| City Bangkok (class 0) = 90 000 | TH `DOH_MOTORWAY_AADT` averaged over Bangkok refs × `thaiClassSplit(isBangkok=true)` | `pipeline/enrich-roads-th.ts` (DOH 2023 motorway traffic report) |
| Country BR rural (class 0) = 50 000 | BR `CLASS_AADT` × `tierMultiplier(0)` × `splitVehicles(tier=0)` | same source, rural (tier-0) arm |
| Country TH rural (class 0) = 60 000 | TH_RURAL class defaults × `thaiClassSplit(isBangkok=false)` | DOH 2023 rural motorway monitoring stations |
| Continent Africa (class 0) ≈ 31 700 | EU baseline × continent_scale(Africa) = 30 000 × 1.057 | `country_defaults_generated.rs::continent_scale()`. Pop-density-weighted blend of wiki + density indexes (see `scripts/gen-country-defaults-rs.mjs`). |
| World (class 0) = 30 000 | EU-generic motorway | Pragmatic: spans the 20 000-40 000 band of BAST-Zählstellen / TMC / MOBIS national censuses |

On top of the cascade, a **242-country scaling layer** (Wikipedia
vehicles-per-km fleet density relative to DE,
`country_defaults_generated.rs`) multiplies motorway / trunk / primary
(+ link) class defaults, clamped to **[0.7, 1.3]**. Lane-count default
multipliers: see §1 (`lane_ratio`).

All cascade arms in `defaults.rs` carry a `// Source:` comment pointing at
the TypeScript enricher they were derived from. When an enricher
re-calibrates, the defaults table must be regenerated to stay in sync
(no build-time check today — tracked as a future follow-up).

### Link AADT as fraction of mainline

| Reference | Link fraction of mainline AADT |
|---|---|
| HCM 7th Ed (TRB 2022) Exhibit 15-4 | 0.10 – 0.30 |
| FEHRL *TASK-CS* 2011 ramp study | 0.12 – 0.28 |
| CERTU *Les bretelles d'autoroute* 2008 | 0.15 – 0.25 |

**Chosen = 0.15** (A.6). Lower-range of the published band, based on
Pasito Blanco GC-1 popup validation (user perception). Previous 0.20 was
the published mid-range; feedback from popup testing indicated the
mid-range over-estimates a typical quiet regional link.

### Track access factor

`engine/noise-compute/src/normalize/road.rs::access_factor` collapses
`highway=track` segments with `access=0` (untagged) to the same
multiplier as explicit `access=agricultural` (×0.1 of the class-8
default = 0.5 veh/day). Rationale:

| OSM class-8 `access` distribution | Count | Share |
|---|---|---|
| access=0 (untagged) | 475 M | 94.7 % |
| access=yes (1) | 12 M | 2.4 % |
| access=agricultural (7) | 8 M | 1.6 % |
| access=forestry (8) | 5 M | 1.0 % |
| other | 1.5 M | 0.3 % |

Pragmatic — not an OSM-convention citation. The long tail of untagged
tracks in practice carries about the same as explicit agricultural; only
the minority that mappers actively tag as `yes` are public. Validated on
Kytín "alej loupežníka Babinského" (49.8467°N, 14.2182°E) → effective
~0.5/day post-fix (was 24/day with `access_factor=1.0`).

### Service-tree per-class cap

`SERVICE_TREE_CAP_PER_CLASS` in `pipeline/enrich-roads-service-tree.ts`
caps dwelling-driven Dijkstra flow to prevent pathological accumulations
where a minor road carries disproportionate stamped flow.

| Class | Cap (veh/day) | Rationale |
|---|---|---|
| 5 residential | 1 200 | 2.4× the world default (480 → 1 200). Dense Prague Karlín blocks can legitimately sit at this level. Matches observed urban residential at AADT stations in medium Czech cities. |
| 6 living_street | 250 | 2.5× the world default (98 → 250). Shared-surface street would saturate around ~300 vehicles/day before it stops feeling shared. |
| 7 service | 400 | 1.7× the world default (240 → 400). Apartment driveway / parking aisle hard cap; OSM `service=*` sub-tag would let us split apartment driveway vs aisle but extractor doesn't preserve it. |
| 9 unclassified | 2 000 | ~1.7× the world default (1 200 → 2 000). Rural connector between two villages with real through-traffic. |

All four caps are pragmatic — they set an upper bound so a service-tree
flow accumulation cannot exceed what a human observer on the road would
call plausible. Validated by the "no urban residential > 2 000 veh/day"
rule of thumb used by several city AADT-modelling agencies (e.g. TfL
LATA 2019 §3.2.1, which caps residential model outputs similarly).

### CGAZ geopolitical policy

`scripts/build-h3-admin.ts` + `data/prepared/h3r4-admin.bin` use **CGAZ
ADM0** (`geoBoundariesCGAZ_ADM0`, geoBoundaries v6.0.0, CC-BY 4.0 —
attribution required and carried in `pipeline/lib/country-polygon.ts`) for
country polygons, plus hand-curated metro polygons in
`scripts/h3-admin-metros.json`. (Natural Earth 1:10 m was retired in M2,
2026-07-28 — its generalization mis-assigned the Hlučínsko salient.)

Assignment is per-hex: centroid PIP via the ONE CGAZ resolver
(`pipeline/lib/admin-at.ts`), with fine interior-grid max-share sampling
for hexes whose centroid falls over water (coastal/island hexes like Koh
Phangan). CGAZ's numeric US-DoS disputed-area codes carry no ISO identity;
the project policy-maps the three road-bearing ones
(`DISPUTED_SHAPEGROUP_ISO` in `admin-at.ts`): Falklands → FK, Aksai Chin →
CN (administering power), Abyei → SD. This is a practical simplification
for traffic-default lookup, not a political statement; a different boundary
view can be baked by changing `admin-at.ts` without touching arrow data.

### Full source URLs

| Tag | URL / reference |
|---|---|
| ITE Trip Gen 11 | https://www.ite.org/technical-resources/topics/trip-and-parking-generation/ |
| NHTS 2022 | https://nhts.ornl.gov/ |
| UK NTS 2023 | https://www.gov.uk/government/statistics/national-travel-survey-2023 |
| MiD 2017 | https://www.mobilitaet-in-deutschland.de/ |
| EMP 2019 (FR) | https://www.statistiques.developpement-durable.gouv.fr/enquete-mobilite-des-personnes-2018-2019 |
| KTDB 2022 | https://www.ktdb.go.kr/ |
| OECD HM1-1 | https://www.oecd.org/housing/data/affordable-housing-database/ |
| HCM 7 TRB 2022 | https://www.trb.org/Main/Blurbs/175169.aspx |
| Promotur tourism | https://turismodeislascanarias.com/en/analysis-tourism-canary-islands |
| geoBoundaries CGAZ ADM0 (CC-BY 4.0) | https://www.geoboundaries.org/ |
| DNIT (BR AADT) | https://servicos.dnit.gov.br/vmt/ |
| DOH (TH motorway) | https://www.doh.go.th/ |
| BAST Zählstellen (DE) | https://www.bast.de/ |
| TMC traffic data | https://tmcconsortium.org/ |

The table above is a pointer, not a reproduction — numbers above cite the
**year / table / section** within each source so future reviewers can
re-verify. If a link rots, the underlying paper is still traceable via
the tag.

### Indoor envelope display estimate

The 4 m map receiver inside an enclosed Overture footprint publishes a
closed-window indoor estimate, not a statutory END indoor map. The footprint
class is assigned once at Overture ingest and survives the height materializer
unchanged. The stored `u8` values are:

| value | class | ΔL |
|---:|---|---:|
| 0 | `OUTDOOR` | not applied |
| 1 | `RESIDENTIAL` | 30 dB |
| 2 | `COMMERCIAL` | 35 dB |
| 3 | `INDUSTRIAL` | 20 dB |
| 4 | `HISTORIC` | 28 dB |
| 5 | `DEFAULT` | 25 dB |

`OUTDOOR` is only the official Overture class set `carport`, `roof`,
`greenhouse`, `glasshouse`, `bridge_structure`, and `grandstand`. All other
official classes are enclosed (unlisted official classes fall through to
`DEFAULT`), and unknown or old rows also fall through to `DEFAULT`. The
precedence is `class → subtype → DEFAULT`; subtype fallback is used only for
a null/unknown class. `is_underground=true` overrides both to `OUTDOOR` at the
4 m receiver. No ML solar/carport inference is claimed when the row has no
semantic class. Courtyards remain outdoor through the existing footprint hole
parity. Overlapping enclosed footprints choose the tallest containing
footprint, then the lower index ordinal and lower footprint ordinal.

The display equation is:

```text
L_indoor = max(0, L_facade - ΔL[class])
```

For the popup, `L_facade` is the aggregate total from the normal outdoor-donor
query. The source rows, contributor rows, and segment traces remain at those
façade values because they describe the actual source-to-receiver physics.
Only the aggregate display totals (`total_lden` and `total_lden_free`) receive
the reduction; the popup presents the façade total, the building-envelope
step, and the resulting indoor estimate as one calculation breakdown. In the
plain-language UI, “building envelope” is shown as “Walls & windows” followed
by the selected building type.

The default 25 dB closed-window value follows the newer WHO Environmental Noise
Guidelines for the European Region (2018) context. WHO 2018's fully-open
window context is approximately 10 dB; the popup's existing tilted/open
variant remains 15 dB. These are external comparison anchors, not a change to
the indoor estimate semantics. The residential, commercial/office,
industrial, and historic values above remain EN 12354-practice product
assumptions. EN ISO 12354-3 / ISO 16283-3 describe façade-to-indoor
methodology. Occupant behaviour dominates; the estimate is typically
uncertain by ±8–12 dB. Propagation physics, source reach, speed floors, and
HM3 format are unchanged.

The same ΔL is applied to the aggregate total regardless of the contributing
source layers; it is not applied to individual source, contributor, or segment
rows. Applying the traffic correction `C_tr` to rail and aircraft is a
documented product simplification.

#### Donor transform and two-pass paint

The donor is an exact integer Felzenszwalb–Huttenlocher two-pass squared
Euclidean distance transform over the 3×3 receiver-tile window (1536×1536
pixels at the current 512-pixel tile size). The implementation uses signed
integer `i32`/`i64` arithmetic, one `i32` squared-distance grid `g`, one
`u16` nearest-site-y grid `sy`, and two lower-envelope arrays. The column
forward/backward sweeps retain the smaller site-y on a tie. The row envelope
uses floor Euclidean division plus one for the first strictly-closer integer
separator, pops on `<=`, and queries on `<=`; therefore an equal squared-distance
tie chooses the smaller absolute site x, then the smaller site y. A brute-force
small-lattice fixture, including diamond ties and the no-site case, is
mandatory because an EDT tie bug changes every indoor pixel near a façade.

Both CPU and GPU-host painters use the same two passes:

1. Pass A scatters, collapses, and area-fills every painted tile and its
   8-neighbour halo. Halo cells are memory-only; they are never written as
   output by this owner.
2. Pass B bakes one class raster and one geometric donor map for the 3×3
   window, then applies that donor map to every layer's already-collapsed
   centre tile. For a finite donor, each layer uses the donor HM3 value and
   `L_indoor`; a missing/`NO_DATA` donor stays `NO_DATA`. The donor is never
   chosen from source loudness and is never self-screened.

The GPU lane stashes collapsed Pass-A cells on the host and runs this same
Pass B; no `.cu` kernel change is permitted. Aircraft `region_runner` does the
same after cruise and airborne collapse, so aircraft and all ground layers
share geometry and donor offsets. This is display-only and deliberately does
not alter the line/point/aircraft propagation kernels. The QOIX obstacle index
layout is version 2, while HM3 remains version 3; the content-hashed layer
code-version machinery therefore invalidates affected output stamps.
