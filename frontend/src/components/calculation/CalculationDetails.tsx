// The detailed calculation of an answered click, opened inside the popup under its list: the day,
// evening and night with their levels, shares of Lden and loudness, the layers with their levels
// and shares, and then every source with its computed pieces and their rays, drawn on the map.
// For finding out why a number is what it is, and where the data or the physics went wrong.
import type { ReactNode } from 'react'
import type { PopupUpdate, SegmentFan } from '../../types/noise'
import { fmtSone } from '../../utils/formatters'
import { SOURCE_LABELS } from '../noise/labels'
import { SEGMENTS_EXPLAINED, SegmentsSection } from '../noise/segments/SegmentsSection'
import { HoverText } from '../ui/info-tip'

/** END periods: their hours of the day and the penalty Lden adds, for each period's share. */
const PERIODS = [
  { key: 'ld', sone: 'day', name: 'Day', hours: 12, penalty: 0 },
  { key: 'le', sone: 'evening', name: 'Evening', hours: 4, penalty: 5 },
  { key: 'ln', sone: 'night', name: 'Night', hours: 8, penalty: 10 },
] as const

const level = (db: number | null | undefined) => (db == null || db <= 0 ? '—' : db.toFixed(1))

const COMPASS = ['N', 'NNE', 'NE', 'ENE', 'E', 'ESE', 'SE', 'SSE', 'S', 'SSW', 'SW', 'WSW', 'W', 'WNW', 'NW', 'NNW']

const WEATHER_EXPLAINED = 'How often the weather here bends sound down toward the ground\n'
  + '(wind behind it or a temperature inversion: CNOSSOS-EU\'s favourable\n'
  + 'conditions, which carry it further) for sound arriving from each\n'
  + 'direction, by period, from ERA5 1991-2020; and how much the air\n'
  + 'absorbs per km in each octave (ISO 9613-1 at the place\'s climate)'

function Section({ title, hint, children }: { title: string, hint: string, children: ReactNode }) {
  return (
    <section className="mt-3">
      <h3 className="mb-0.5 text-[11px] font-medium uppercase tracking-[0.08em] text-muted-foreground">
        <HoverText title={hint}>{title}</HoverText>
      </h3>
      {children}
    </section>
  )
}

/** A table whose numeric columns are right-aligned. */
function Table({ head, rows }: { head: ReactNode[], rows: ReactNode[][] }) {
  return (
    <table className="w-full text-[11px] font-mono tabular-nums">
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
  )
}

export default function CalculationDetails({ data, onFan }: {
  data: PopupUpdate
  /** Draws the listed pieces and their rays on the map; null clears them. */
  onFan?: (fan: SegmentFan | null) => void
}) {
  const [lat, lng] = data.center
  const periodEnergy = PERIODS.map(p => {
    const db = data.total[p.key]
    return db == null ? 0 : p.hours * 10 ** ((db + p.penalty) / 10)
  })
  const allEnergy = periodEnergy.reduce((a, b) => a + b, 0)
  const total = data.total_lden ?? 0
  const l5 = data.percentiles?.l5
  const sone = data.loudness?.n5_sone
  const layers = data.sources
    .filter(layer => (layer.lden ?? 0) > 0)
    .sort((a, b) => (b.lden ?? 0) - (a.lden ?? 0))
  const share = (lden: number | null) => (lden == null ? '—' : `${Math.round(100 * 10 ** ((lden - total) / 10))} %`)

  return (
    <div data-testid="calculation">
      <Section title="Day, evening, night" hint={'Each period\'s level, its share of the Lden energy (the evening\ncounts 5 dB and the night 10 dB up, EU Directive 2002/49), the\nlevel it exceeds 5 % of the time (L5) and the loudness of its\nspectrum at L5 (ISO 532-1, N5); the whole day\'s loudness\nweighs the periods as Lden does'}>
        <Table
          head={['', 'dB', 'Share', 'L5 dB', 'Sone']}
          rows={PERIODS.map((p, k) => [
            p.name,
            level(data.total[p.key]),
            allEnergy > 0 ? `${Math.round(100 * periodEnergy[k] / allEnergy)} %` : '—',
            level(l5?.[p.sone]),
            sone?.[p.sone] ? fmtSone(sone[p.sone] as number) : '—',
          ])}
        />
      </Section>

      <Section title="Layers" hint={'Each layer\'s levels by period, its Lden and its share of the\nLden energy'}>
        <Table
          head={['', 'Day', 'Eve', 'Night', 'Lden', 'Share']}
          rows={layers.map(layer => [
            SOURCE_LABELS[layer.source_type] ?? layer.source_type,
            level(layer.ld),
            level(layer.le),
            level(layer.ln),
            <b key="l">{level(layer.lden)}</b>,
            share(layer.lden),
          ])}
        />
      </Section>

      {data.weather && (
        <Section title="Weather here" hint={WEATHER_EXPLAINED}>
          <Table
            head={['Sound from', 'Day', 'Eve', 'Night']}
            rows={COMPASS.map((name, k) => [
              name,
              // Sound arriving from the south travels north: sector k + 8.
              ...data.weather!.favourable_percent.map(row => `${row[(k + 8) % 16]} %`),
            ])}
          />
          <Table
            head={['Air, dB/km', '63', '125', '250', '500', '1k', '2k', '4k', '8k']}
            rows={[['', ...data.weather.alpha_db_per_km.map(alpha => alpha.toFixed(2))]]}
          />
        </Section>
      )}

      <Section title="Sources, pieces and rays" hint={SEGMENTS_EXPLAINED}>
        <SegmentsSection
          lat={lat}
          lng={lng}
          building={data.building}
          reflectionDb={data.reflection_db ?? 0}
          layers={data.sources}
          onFan={onFan}
        />
      </Section>
    </div>
  )
}
