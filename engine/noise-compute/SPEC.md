# Popup propagation contract

The Rust engine is the acoustic source of truth. This document specifies the
current screening contract; it is not a claim of full CNOSSOS-EU compliance.
The implementation and regressions live in `src/propagation/path_effects.rs`,
`diffraction.rs` and `arc_screening.rs`.

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
