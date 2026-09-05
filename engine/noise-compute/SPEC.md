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

Prepared airborne observations are copied once to each supported receiver cell;
aircraft copies from neighboring cells are never added again. Equal original
observations retain their multiplicity. Cruise aggregates canonical cells once,
then copies complete final rows to their representative-length support cells.
Ground sources retain their spatial owners. Surface owner selection enumerates
the existing midpoint-gate envelope, including wrapped longitude and high
latitudes; listing requests use their own radius with unchanged per-row gates.
Present aircraft schemas must carry a positive sampling-window stamp, including
empty files; selected rows cannot redefine the observation window.

## Aircraft local geometry

Doc 29 keeps its receiver-latitude scale and infinite-line CPA. Both the
receiver-to-segment-start longitude and the segment's own longitude extent use
the short arc from `grid::geo`; separately wrapping both endpoints would still
stretch a short segment beside the receiver's opposite meridian. Popup pruning,
CPA, reach gates and prepared-row kernels share this convention. Terrain-horizon
samples use the same canonical longitude interval. Moving a flight and its ridge
across ±180° must preserve the received SEL, screening and displayed CPA.

Airborne selection uses the periodic 16 km axis envelope, with the same f32
receiver-bound rounding at batch, row and segment gates. A raw aggregate bbox
at least 180° wide cannot identify its contained short arcs, so it retains the
latitude gate but defers longitude pruning to individual segments. Publication
support encloses those decoded segment arcs and the receiver rounding bins;
it does not copy every flight to the dateline. This corrects the former seam
selection bypass and false negatives; it is not universal output parity with
that bypass. Cruise retains its separate representative-length centroid gate.

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
