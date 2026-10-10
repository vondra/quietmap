// The detailed calculation of an answered click, opened inside the popup under its list: the day,
// evening and night with their levels, shares of Lden and loudness, the layers with their levels
// and shares, the place's weather, and then every source with its computed pieces and their rays,
// drawn on the map. For finding out why a number is what it is, and where the data or the physics
// went wrong.
import type { ReactNode } from 'react'
import type { PopupUpdate, SegmentFan } from '../../types/noise'
import { fmtDbValue as level, fmtSone } from '../../utils/formatters'
import { SOURCE_LABELS } from '../noise/labels'
import { SEGMENTS_EXPLAINED, SegmentsSection } from '../noise/segments/SegmentsSection'
import { CAPTION, DETAIL_TEXT, DetailTable } from '../noise/shared'
import { HoverText } from '../ui/info-tip'

/** END periods: their hours of the day and the penalty Lden adds, for each period's share. */
const PERIODS = [
  { key: 'ld', sone: 'day', name: 'Day', hours: 12, penalty: 0 },
  { key: 'le', sone: 'evening', name: 'Evening', hours: 4, penalty: 5 },
  { key: 'ln', sone: 'night', name: 'Night', hours: 8, penalty: 10 },
] as const

const COMPASS = ['N', 'NNE', 'NE', 'ENE', 'E', 'ESE', 'SE', 'SSE', 'S', 'SSW', 'SW', 'WSW', 'W', 'WNW', 'NW', 'NNW']

const PERIODS_EXPLAINED = 'Each period\'s level, its share of Lden (the evening counts 5 dB and the\n'
  + 'night 10 dB louder) and how loud it sounds on average (ISO 532-1)'

const WEATHER_EXPLAINED = 'How often the wind or an inversion bends sound down to the ground and\n'
  + 'carries it further (CNOSSOS-EU\'s favourable conditions), for sound\n'
  + 'from each direction (ERA5 1991–2020); and what the air absorbs\n'
  + 'in each octave (ISO 9613-1, the place\'s climate)'

function Section({ title, hint, children }: { title: string, hint: string, children: ReactNode }) {
  return (
    <section className="mt-4">
      <h3 className={`${CAPTION} mb-1`}>
        <HoverText title={hint}>{title}</HoverText>
      </h3>
      {children}
    </section>
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
  const sone = data.loudness?.mean_sone
  const layers = data.sources
    .filter(layer => (layer.lden ?? 0) > 0)
    .sort((a, b) => (b.lden ?? 0) - (a.lden ?? 0))
  const share = (lden: number | null) => (lden == null ? '—' : `${Math.round(100 * 10 ** ((lden - total) / 10))} %`)

  return (
    <div data-testid="calculation" className={DETAIL_TEXT}>
      <Section title="Day, evening, night" hint={PERIODS_EXPLAINED}>
        <DetailTable
          head={['', 'dB', 'Share', 'Sone']}
          rows={PERIODS.map((p, k) => [
            p.name,
            level(data.total[p.key]),
            allEnergy > 0 ? `${Math.round(100 * periodEnergy[k] / allEnergy)} %` : '—',
            sone?.[p.sone] != null ? fmtSone(sone[p.sone] as number) : '—',
          ])}
        />
      </Section>

      <Section title="Layers" hint="Each layer's levels and its share of Lden">
        <DetailTable
          head={['', 'Day', 'Evening', 'Night', 'Lden', 'Share']}
          rows={layers.map(layer => [
            SOURCE_LABELS[layer.source_type] ?? layer.source_type,
            level(layer.ld),
            level(layer.le),
            level(layer.ln),
            level(layer.lden),
            share(layer.lden),
          ])}
        />
      </Section>

      {data.weather && (
        <Section title="Weather" hint={WEATHER_EXPLAINED}>
          <div className="space-y-2">
            <DetailTable
              head={['Sound from', 'Day %', 'Evening %', 'Night %']}
              rows={COMPASS.map((name, k) => [
                name,
                // Sound arriving from the south travels north: sector k + 8.
                ...data.weather!.favourable_percent.map(row => String(row[(k + 8) % 16])),
              ])}
            />
            <DetailTable
              head={['Hz', '63', '125', '250', '500', '1k', '2k', '4k', '8k']}
              // Three significant figures, so the eight bands fit the card's width (0.12 to 113).
              rows={[['Air dB/km', ...data.weather.alpha_db_per_km.map(alpha => alpha.toFixed(alpha < 10 ? 2 : alpha < 100 ? 1 : 0))]]}
            />
          </div>
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
