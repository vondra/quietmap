// The detailed calculation of an answered click, in its own panel (beside the popup on a desktop,
// over the whole screen on a phone): how Lden comes from day, evening and night, how the loudness
// does, the layers with their shares and how many of their pieces within reach were computed, and
// then every source with its computed pieces and their rays, drawn on the map. For finding out why
// a number is what it is, and where the data or the physics went wrong.
import { useEffect, useRef, useState, type ReactNode } from 'react'
import { X } from 'lucide-react'
import type { PopupUpdate, SegmentFan } from '../../types/noise'
import { fmtInt, fmtSone } from '../../utils/formatters'
import { SOURCE_LABELS } from '../noise/shared'
import { SEGMENTS_EXPLAINED, SegmentsSection } from '../noise/segments/SegmentsSection'
import { HoverText } from '../ui/info-tip'

/** END periods: hours and the penalty Lden adds. */
const PERIODS = [
  { key: 'ld', name: 'Day', hours: '07–19', span: 12, penalty: 0 },
  { key: 'le', name: 'Evening', hours: '19–23', span: 4, penalty: 5 },
  { key: 'ln', name: 'Night', hours: '23–07', span: 8, penalty: 10 },
] as const
const SONE_KEYS = ['day', 'evening', 'night'] as const

const level = (db: number | null | undefined) => (db == null || db <= 0 ? '—' : db.toFixed(1))

function Section({ title, hint, children }: { title: string, hint?: string, children: ReactNode }) {
  return (
    <section className="mt-4">
      <h3 className="mb-1 text-[11px] font-medium uppercase tracking-[0.08em] text-muted-foreground">
        {hint ? <HoverText title={hint}>{title}</HoverText> : title}
      </h3>
      {children}
    </section>
  )
}

/** A table whose numeric columns are right-aligned. */
function Table({ head, rows }: { head: ReactNode[], rows: ReactNode[][] }) {
  return (
    <div className="overflow-x-auto">
      <table className="w-full text-xs font-mono tabular-nums">
        <thead>
          <tr className="text-[10px] font-sans text-muted-foreground/70">
            {head.map((cell, k) => <th key={k} className={`py-0.5 font-normal ${k === 0 ? 'text-left' : 'text-right'}`}>{cell}</th>)}
          </tr>
        </thead>
        <tbody>
          {rows.map((row, r) => (
            <tr key={r} className="border-t border-border/40">
              {row.map((cell, k) => <td key={k} className={`py-0.5 ${k === 0 ? 'text-left font-sans' : 'text-right'}`}>{cell}</td>)}
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  )
}

export default function CalculationPanel({ data, onClose, onFan }: {
  data: PopupUpdate
  onClose: () => void
  /** Draws the listed pieces and their rays on the map; null clears them. */
  onFan?: (fan: SegmentFan | null) => void
}) {
  const [lat, lng] = data.center
  const panel = useRef<HTMLElement>(null)
  // The map frames the rays beside the panel: its width on a desktop, nothing on a phone where
  // the panel covers the map.
  const [insetLeftPx, setInsetLeftPx] = useState(0)
  useEffect(() => {
    const measure = () => {
      const box = panel.current?.getBoundingClientRect()
      setInsetLeftPx(box && box.width < window.innerWidth ? box.right : 0)
    }
    measure()
    window.addEventListener('resize', measure)
    return () => window.removeEventListener('resize', measure)
  }, [])
  const periodEnergy = PERIODS.map(p => {
    const db = data.total[p.key]
    return db == null ? 0 : (p.span / 24) * 10 ** ((db + p.penalty) / 10)
  })
  const allEnergy = periodEnergy.reduce((a, b) => a + b, 0)
  const total = data.total_lden ?? 0
  const loudness = data.loudness
  const l5 = data.percentiles?.l5
  const layers = data.sources
    .filter(layer => (layer.lden ?? 0) > 0)
    .sort((a, b) => (b.lden ?? 0) - (a.lden ?? 0))
  const share = (lden: number | null) => (lden == null ? '—' : `${Math.round(100 * 10 ** ((lden - total) / 10))} %`)

  return (
    <aside
      ref={panel}
      data-testid="calculation-panel"
      aria-label="Detailed calculation"
      className="pointer-events-auto fixed inset-0 z-[2100] overflow-y-auto bg-background p-3 md:inset-auto md:top-3 md:bottom-3 md:left-3 md:z-[1004] md:w-[min(600px,calc(100vw-380px))] md:rounded-md md:border md:border-black/5 md:shadow-lg"
    >
      <div className="flex items-start justify-between gap-2">
        <div>
          <h2 className="text-base font-semibold leading-tight">
            <HoverText title={SEGMENTS_EXPLAINED}>Detailed calculation</HoverText>
          </h2>
          <div className="text-xs text-muted-foreground/70 font-mono">{lat.toFixed(5)}, {lng.toFixed(5)}</div>
        </div>
        <button type="button" onClick={onClose} aria-label="Close the calculation" className="p-1 rounded-md hover:bg-black/5 text-muted-foreground hover:text-foreground">
          <X className="size-4" />
        </button>
      </div>

      <Section title="Lden from day, evening and night" hint={'Lden = 10 lg of the hours-weighted energy of the day, the evening\n5 dB up and the night 10 dB up, over 24 hours (EU Directive 2002/49)'}>
        <Table
          head={['Period', 'Level dB', 'Hours', 'Added dB', 'Share']}
          rows={[
            ...PERIODS.map((p, k) => [
              `${p.name} ${p.hours}`,
              level(data.total[p.key]),
              p.span,
              p.penalty ? `+${p.penalty}` : '0',
              allEnergy > 0 ? `${Math.round(100 * periodEnergy[k] / allEnergy)} %` : '—',
            ]),
            [<b key="l">Lden</b>, <b key="v">{level(data.total_lden)}</b>, 24, '', ''],
          ]}
        />
      </Section>

      {loudness && (
        <Section title="Loudness from day, evening and night" hint={'Each period\'s received spectrum at the level it exceeds 5 % of\nthe time (L5), its loudness by ISO 532-1 (N5); the whole day\nweighs the periods as Lden does'}>
          <Table
            head={['Period', 'L5 dB', 'N5 sone']}
            rows={[
              ...PERIODS.map((p, k) => [
                `${p.name} ${p.hours}`,
                level(l5?.[SONE_KEYS[k]]),
                loudness.n5_sone[SONE_KEYS[k]] ? fmtSone(loudness.n5_sone[SONE_KEYS[k]] as number) : '—',
              ]),
              [<b key="d">Whole day</b>, '', <b key="v">{loudness.n5_den_sone ? fmtSone(loudness.n5_den_sone) : '—'}</b>],
            ]}
          />
        </Section>
      )}

      <Section title="Layers" hint={'Each layer\'s levels, its share of the Lden energy, and how many of\nits pieces within reach were computed one by one (the rest, far or\nscreened, add under 0.1 dB or are estimated from a sample)'}>
        <Table
          head={['Layer', 'Day', 'Evening', 'Night', 'Lden', 'Share', 'Pieces']}
          rows={layers.map(layer => [
            SOURCE_LABELS[layer.source_type] ?? layer.source_type,
            level(layer.ld),
            level(layer.le),
            level(layer.ln),
            <b key="l">{level(layer.lden)}</b>,
            share(layer.lden),
            layer.evaluated >= layer.candidates ? fmtInt(layer.candidates) : `${fmtInt(layer.evaluated)} / ${fmtInt(layer.candidates)}`,
          ])}
        />
      </Section>

      <Section title="Sources, pieces and rays">
        <SegmentsSection
          lat={lat}
          lng={lng}
          building={data.building}
          reflectionDb={data.reflection_db ?? 0}
          layers={data.sources}
          contributors={data.top_contributors}
          onFan={onFan}
          insetLeftPx={insetLeftPx}
        />
      </Section>
    </aside>
  )
}
