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
length. Equal original observations retain their multiplicity. Cruise
aggregates canonical cells once
and publishes them only in the owner z9 (`cruise_owner_z9_v1`); the popup loads
owner squares within `CRUISE_QUERY_RADIUS_M` and prunes batches by their
synthetic-line envelope. A bucket's representative length is clamped to
`CRUISE_MAX_REP_LEN_M` (50 km): ADS-B coverage gaps up to 2 589 km stay local,
their density rises accordingly (about +2 dB along such a gap track), and no
bucket reaches beyond the query radius. Ground sources retain their spatial owners. Surface owner selection enumerates
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
receiver-bound rounding at the batch and segment gates. A raw aggregate bbox
at least 180° wide cannot identify its contained short arcs, so it retains the
latitude gate but defers longitude pruning to individual segments. Owner
squares are selected in the same metre-per-degree metric that bounds a row's
length, so a decoded arc the envelope accepts always lies in a loaded square.
Cruise retains its separate representative-length centroid gate.

Ground-operation line divergence uses half a canonical surface pixel at receiver
latitude: the z13 tile has 512 pixels. Popup and GPU use the same grid-derived
floor. This corrects the retired z12 floor, which was twice the current half
pixel and reduced the near-line contribution by approximately 3 dB.

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
