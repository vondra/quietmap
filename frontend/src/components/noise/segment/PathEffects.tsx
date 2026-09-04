import type { SegmentTrace } from '../../../types/noise'
import { HoverText } from '../../ui/info-tip'
import { BAND_LABELS, Section, InlineTable, bandsTooltip, fmtDbSigned } from './display'
import { ScreeningFanRow } from './ScreeningFanRow'

export function Section4PathEffects({ trace }: { trace: SegmentTrace }) {
  // Doc 29 aircraft (airborne / cruise) doesn't have CNOSSOS path effects.
  // Future: render a Section4Doc29 with NPD / ΔV / ΔI / Λ / ΔF instead.
  if (trace.propagation.model !== 'cnossos') return null
  const { received_lden } = trace
  const { baseline, terrain, screening, vegetation, ground, path_profile } = trace.propagation
  // Aircraft ground-ops microsegments populate attenuation_bands and
  // factor_g but intentionally NOT the structural arrays (edges,
  // obstacle, forest_runs) — those would 3-4× the JSON payload at
  // 3 k microsegments per LKPR popup. Detect the "scalar-only" mode
  // via empty path_profile and suppress the "(none)" parentheticals
  // that imply data is missing when really it's just not serialized.
  const isScalarOnly = path_profile.t.length === 0

  const groundDelta = received_lden.full - received_lden.no_ground
  const atmosphericDelta = received_lden.full - received_lden.no_atmospheric
  const terrainDelta = received_lden.full - received_lden.no_terrain
  const screeningDelta = received_lden.full - received_lden.no_screening
  const vegetationDelta = received_lden.full - received_lden.no_vegetation
  // Engine (iso9613.rs): free_field = base − A_gr + A_flc (+ refl_v). Total
  // obstruction delta = full − free_field = A_bar + A_fol contributions.
  // A_div, A_atm, A_gr, A_flc, A_refl are baseline corrections — shown above
  // the Total for transparency, not as sum-terms feeding it.
  const totalPathDelta = received_lden.full - received_lden.free_field

  // Baseline corrections — ordered per ISO 9613-2 §7.1 → §7.5 then FLC.
  // Split-tooltip rule: LABEL tooltip = concept only (what the term is +
  // standard symbol); VALUE tooltip = how this specific number was
  // computed (engine source, per-band breakdown where applicable,
  // variant-delta origin). Plain labels, symbols live only in tooltips.
  const baselineRows: [React.ReactNode, React.ReactNode][] = [
    [
      <HoverText
        title={
          'A_div — Geometric divergence (ISO 9613-2 §7.1).\n\n' +
          'Inverse-distance spreading of the source power. Always applied,\n' +
          'grows with distance. Line sources: cylindrical spreading; point\n' +
          'sources: spherical spreading plus a source-geometry adapter.'
        }
      >
        Geometric divergence
      </HoverText>,
      <HoverText
        title={
          `baseline.geometric_db = ${baseline.geometric_db.toFixed(2)} dB (engine scalar).\n\n` +
          'Formula: line → 10·log₁₀(2π·d_slant);\n' +
          '        point → 20·log₁₀(d_slant) + 11.\n' +
          `d_slant = ${trace.d_slant_m.toFixed(1)} m.\n` +
          'Sign-flipped here because it subtracts from L_w.'
        }
      >
        {fmtDbSigned(-baseline.geometric_db)}
      </HoverText>,
    ],
    [
      <HoverText
        title={
          'A_atm — Atmospheric absorption (ISO 9613-2 §7.2).\n\n' +
          'Frequency-dependent air absorption. Standard atmosphere\n' +
          '(15 °C, 70 % RH). Higher frequencies attenuate much more\n' +
          'per kilometre than low frequencies.'
        }
      >
        Atmospheric absorption
      </HoverText>,
      <HoverText
        title={bandsTooltip(baseline.atmospheric_bands, {
          title: 'A_atm per band — α[i] × d_slant / 1000',
          signed: true,
          note:
            `d_slant = ${trace.d_slant_m.toFixed(1)} m.\n` +
            'Scalar = A-weighted ΔL_A (full − no_atmospheric Lden).',
        })}
      >
        {fmtDbSigned(atmosphericDelta)}
      </HoverText>,
    ],
    [
      <HoverText
        title={
          'A_gr — Ground effect (ISO 9613-2 §7.3.1 / CNOSSOS §2.5.15).\n\n' +
          'Interaction of direct + ground-reflected rays — destructive at\n' +
          'mid bands, constructive at LF over soft ground. SIGNED: over\n' +
          'soft ground (G → 1) the 63/125 Hz bands can BOOST energy.'
        }
      >
        Ground effect (G={ground.factor_g.toFixed(2)})
      </HoverText>,
      <HoverText
        title={bandsTooltip(ground.attenuation_bands, {
          title: `A_gr per band — CF[i] × G (G = ${ground.factor_g.toFixed(2)})`,
          signed: true,
          note:
            'G is path-averaged from the receiver\'s imperviousness raster\n' +
            '(0 = hard, 1 = soft).\n' +
            'Scalar = A-weighted ΔL_A (full − no_ground Lden).',
        })}
      >
        {fmtDbSigned(groundDelta)}
      </HoverText>,
    ],
  ]
  if (baseline.reflection_boost_db > 0.05) {
    baselineRows.push([
      <HoverText
        title={
          'A_refl — Urban reflection boost (ISO 9613-2 §7.5).\n\n' +
          'Per-receiver 0..3 dB boost from local building enclosure\n' +
          '(nine exact-footprint probes around the receiver). Reflected energy adds to\n' +
          'the direct path — always positive, same scalar for every\n' +
          'segment at this receiver.'
        }
      >
        Urban reflection
      </HoverText>,
      <HoverText
        title={
          `baseline.reflection_boost_db = +${baseline.reflection_boost_db.toFixed(2)} dB (engine scalar).\n\n` +
          'Computed once per receiver from the local building_enclosure()\n' +
          'vector-footprint probe (0 / 1.5 / 3 dB). Not a variant delta — the\n' +
          'same value appears on every segment at this point.'
        }
      >
        +{baseline.reflection_boost_db.toFixed(1)} dB
      </HoverText>,
    ])
  }
  if (Math.abs(baseline.finite_line_corr_db) > 0.05) {
    baselineRows.push([
      <HoverText
        title={
          'A_flc — Finite-line correction.\n\n' +
          'Line sources only (roads, railways). Compensates for the\n' +
          'segment subtending a finite angle at the receiver instead\n' +
          'of the infinite line assumed by cylindrical divergence.\n' +
          'Standard practice in NMPB / NoiseModelling / CNOSSOS.'
        }
      >
        Finite-line correction
      </HoverText>,
      <HoverText
        title={
          `baseline.finite_line_corr_db = ${baseline.finite_line_corr_db.toFixed(2)} dB (engine scalar).\n\n` +
          'Computed from the segment endpoint geometry at the receiver —\n' +
          'closer / shorter segments subtend less angle and attenuate\n' +
          'more than the infinite-line approximation predicts.'
        }
      >
        {fmtDbSigned(baseline.finite_line_corr_db)}
      </HoverText>,
    ])
  }

  // Obstruction rows — terrain + screening + vegetation. Compose totalPathDelta.
  // Same split rule: LABEL concept, VALUE computation details.
  const terrainRow: [React.ReactNode, React.ReactNode] = (() => {
    const labelTooltip =
      'A_bar — Terrain diffraction (ISO 9613-2 §7.3 / CNOSSOS §2.5.6).\n\n' +
      'Sound bending over hills, cuttings, berms — any DEM feature\n' +
      'above the line-of-sight. Maekawa formula per band, capped at\n' +
      '20 dB. The engine picks the single edge with the largest\n' +
      'path-length difference δ (max-δ selection).'
    const deltaStr = terrain.delta_m > 0
      ? `δ = ${terrain.delta_m.toFixed(2)} m, single edge`
      : 'no obstruction'
    const valueTooltip =
      `${deltaStr}.\n\n` +
      'Maekawa per-band output below.\n' +
      'Scalar = A-weighted ΔL_A (full − no_terrain Lden).\n' +
      'Rayleigh δ* gate is reported on its own row when it zeroes any band.'
    const terrainParens = terrain.delta_m > 0
      ? ` (δ ${terrain.delta_m.toFixed(2)} m, single edge)`
      : isScalarOnly
        ? ''
        : ' (none)'
    return [
      <HoverText title={labelTooltip}>Terrain diffraction{terrainParens}</HoverText>,
      <HoverText title={bandsTooltip(terrain.attenuation_bands, { title: valueTooltip })}>
        {fmtDbSigned(terrainDelta)}
      </HoverText>,
    ]
  })()
  const screeningRow: [React.ReactNode, React.ReactNode] = (() => {
    const obs = screening.obstacle
    const screenLabel = obs
      ? `${obs.edge.kind} ${obs.edge.height_m.toFixed(1)} m`
      : isScalarOnly
        ? ''
        : 'none'
    const labelTooltip =
      'A_bar — Building / barrier screening component (SPEC §4.7).\n\n' +
      'For a road or railway segment that carries a Screening fan row, the\n' +
      'engine clips its angular fan against the receiver skyline,\n' +
      'evaluates one exact source-point ray per interval, then energy-averages\n' +
      'max(A_ground, A_terrain + A_screen). Narrow spans, empty skylines and\n' +
      'point sources use the characteristic-point/source ray alone. This row\n' +
      'is the resulting increment over pure terrain. On each ray, each band\n' +
      'retains the maximum attenuation across terrain and exact building /\n' +
      'barrier crossings. The listed representative propagation edge is real,\n' +
      'but does not alone explain every band or the whole fan.'
    const edgeDetail = obs
      ? `\n\nRepresentative propagation edge: ${obs.edge.kind} ${obs.edge.height_m.toFixed(1)} m @ t=${obs.edge.t.toFixed(2)} (+${obs.edge.screen_h_m.toFixed(1)} m above LOS)`
      : ''
    const valueTooltip =
      'Band envelope: per-band increment over terrain, energy-averaged\n' +
      'across interval rays when a Screening fan is present.\n' +
      'Scalar = A-weighted ΔL_A (full − no_screening Lden).' +
      edgeDetail
    return [
      <HoverText title={labelTooltip}>
        Building/barrier{screenLabel ? ` (${screenLabel})` : ''}
      </HoverText>,
      <HoverText title={bandsTooltip(screening.attenuation_bands, { title: valueTooltip })}>
        {fmtDbSigned(screeningDelta)}
      </HoverText>,
    ]
  })()
  const foliageRow: [React.ReactNode, React.ReactNode] = [
    <HoverText
      title={
        'A_fol — Foliage / vegetation (ISO 9613-2:2024 Annex A.2.2).\n\n' +
        'Forest along the path absorbs mid-to-high frequencies; depth\n' +
        'is weighted by canopy density where the data carries it.\n' +
        'Capped at ~200 m effective depth. Project applies a ×0.5\n' +
        'Central-Europe calibration against over-counting sparse\n' +
        'canopy as dense foliage.'
      }
    >
      Foliage
      {vegetation.forest_depth_m > 0
        ? ` (${vegetation.forest_depth_m.toFixed(0)} m forest, 0.5× adj.)`
        : isScalarOnly
          ? ''
          : ' (none)'}
    </HoverText>,
    <HoverText
      title={bandsTooltip(vegetation.attenuation_bands, {
        title: `A_fol per band — α_veg[i] × min(depth, 200 m)`,
        note:
          `Forest depth: ${vegetation.forest_depth_m.toFixed(0)} m across ${vegetation.forest_runs.length} run${vegetation.forest_runs.length === 1 ? '' : 's'}.\n` +
          'Already includes the ×0.5 Central-Europe calibration.\n' +
          'Scalar = A-weighted ΔL_A (full − no_vegetation Lden).',
      })}
    >
      {fmtDbSigned(vegetationDelta)}
    </HoverText>,
  ]
  const fan = screening.fan

  // Rayleigh criterion indicator: which bands the engine zeroed by the
  // 2021/1226 point (9)(c) δ ≤ λ/4 − δ* rule. Read from the engine's zeroed
  // bands — no re-derivation. Only a NEAR MISS can be gated: the criterion is
  // scoped to an unblocked direct ray ("If the direct ray is not blocked"),
  // so a blocking edge (δ > 0) never has a band zeroed this way.
  const gatedBands = terrain.delta_m < 0 && terrain.delta_star_m > 0
    ? BAND_LABELS.filter((_, i) => terrain.attenuation_bands[i] === 0)
    : []

  return (
    <Section>
      <div className="text-muted-foreground/60 font-normal pb-0.5">
        <HoverText
          title={
            'Baseline corrections: ISO 9613-2 §7 + CNOSSOS-EU §2.5.\n' +
            'Obstructions: ISO 9613-2 §7.4 Maekawa barrier formula +\n' +
            'the CNOSSOS-EU 2021/1226 Rayleigh criterion on near misses.\n' +
            'Hover individual rows for concept + value breakdown.'
          }
        >
          Attenuations
        </HoverText>
      </div>
      <InlineTable rows={baselineRows} />
      <div className="h-2" aria-hidden="true" />
      {fan ? (
        <>
          <InlineTable rows={[terrainRow, screeningRow]} />
          {/* The existing obstacle label can be wider than 200 px; a separate
              grid leaves the required fan value readable in the 320 px popup. */}
          <ScreeningFanRow fan={fan} />
          <InlineTable rows={[foliageRow]} />
        </>
      ) : (
        <InlineTable rows={[terrainRow, screeningRow, foliageRow]} />
      )}
      {gatedBands.length > 0 && (
        <HoverText
          title={
            `Rayleigh criterion — CNOSSOS-EU 2021/1226 point (9)(c), per band:\n` +
            `an edge that does NOT break the sight line diffracts only where\n` +
            `δ > λ/4 − δ*. Engine computed δ* = ${terrain.delta_star_m.toFixed(2)} m for this edge\n` +
            `(mirror fit over bare-earth OLS planes). Bands that fail the test\n` +
            `contribute 0 dB of diffraction attenuation in the total. A BLOCKING\n` +
            `edge is never tested — it always diffracts (ISO/TR 17534-4 §5.9).`
          }
        >
          <div className="mt-0.5 text-[10px] text-muted-foreground italic">
            Rayleigh criterion zeroed: {gatedBands.join(', ')}
          </div>
        </HoverText>
      )}
      <div className="mt-1 grid grid-cols-[auto_1fr] gap-x-3 border-t border-border/40 pt-0.5">
        <span className="text-muted-foreground/70 font-medium">
          <HoverText
            title={
              'NOT a sum of the rows above — this is the engine variant\n' +
              'delta: received_lden.full − received_lden.free_field.\n\n' +
              'Free-field covers divergence + atmospheric + ground + FLC\n' +
              '(iso9613.rs:253). Everything else applied in Full shows up\n' +
              'here: terrain diffraction (A_bar), building/barrier screening\n' +
              '(A_bar increment), foliage (A_fol), and — when non-zero —\n' +
              'the urban reflection boost (A_refl). 0 dB means no obstruction\n' +
              'and no reflection changed the outcome.\n\n' +
              'The per-effect rows above ARE deltas too (full vs full-with-\n' +
              'one-effect-removed), but they overlap rather than add up\n' +
              'cleanly — the ISO 9613-2 max-rule between ground and\n' +
              'barrier means dropping one effect can shift another.'
            }
          >
            Combined path effect
          </HoverText>
        </span>
        <span className="text-foreground font-mono font-medium text-right">
          <HoverText
            title={
              `received_lden.full − received_lden.free_field\n` +
              `= ${received_lden.full.toFixed(2)} − ${received_lden.free_field.toFixed(2)}\n` +
              `= ${totalPathDelta.toFixed(2)} dB (A-weighted).\n\n` +
              'Covers obstructions (A_bar terrain + A_bar building +\n' +
              'A_fol foliage) and — when non-zero — the urban reflection\n' +
              'boost (A_refl), since reflection is applied in Full but\n' +
              'not in free_field.'
            }
          >
            {fmtDbSigned(totalPathDelta)}
          </HoverText>
        </span>
      </div>
    </Section>
  )
}
