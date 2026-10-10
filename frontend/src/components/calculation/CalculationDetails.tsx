// The detailed calculation of an answered click, opened inside the popup under its list: where the
// levels are computed (a building's façade, the reflection of the walls around), the day, evening
// and night with their levels, shares of Lden and loudness and the levels exceeded 5 to 90 % of
// each, the layers with their levels and shares, and the place's weather. For finding out why a
// number is what it is, and where the data or the physics went wrong; how each source's sound
// arrives opens in its row.
import type { ReactNode } from 'react'
import type { BuildingAnswer, PopupUpdate } from '../../types/noise'
import { fmtDbValue as level, fmtSone } from '../../utils/formatters'
import { SOURCE_LABELS } from '../noise/labels'
import { CAPTION, compassPoint, DETAIL_TEXT, DetailTable, lineRow } from '../noise/shared'
import { REFLECTIONS } from '../noise/source/PathTable'
import { HoverText } from '../ui/info-tip'

/** END periods: their hours of the day and the penalty Lden adds, for each period's share. */
const PERIODS = [
  { key: 'ld', sone: 'day', name: 'Day', hours: 12, penalty: 0 },
  { key: 'le', sone: 'evening', name: 'Evening', hours: 4, penalty: 5 },
  { key: 'ln', sone: 'night', name: 'Night', hours: 8, penalty: 10 },
] as const

const COMPASS = ['N', 'NNE', 'NE', 'ENE', 'E', 'ESE', 'SE', 'SSE', 'S', 'SSW', 'SW', 'WSW', 'W', 'WNW', 'NW', 'NNW']

const RECEIVER_EXPLAINED = 'Where every level is computed: 4 m above the ground'

const PERIODS_EXPLAINED = 'Each period\'s level, its share of Lden (the evening counts 5 dB and the\n'
  + 'night 10 dB louder) and how loud it sounds on average (ISO 532-1)'

const PERCENTILES_EXPLAINED = 'L5, L10, L50, L90: the level exceeded 5, 10, 50 and 90 % of the period\'s time'

const WEATHER_EXPLAINED = 'How often the wind or an inversion bends sound down to the ground and\n'
  + 'carries it further (CNOSSOS-EU\'s favorable conditions), for sound\n'
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

/** Where every level of the click is computed, when that is not just the clicked point: a
 *  building's façade, and the reflection of walls close behind the receiver. */
function ReceiverSection({ building, reflectionDb }: { building: BuildingAnswer | null, reflectionDb: number }) {
  const facade = building?.facade ?? null
  if (!facade && reflectionDb <= 0) return null
  return (
    <Section title="Receiver" hint={RECEIVER_EXPLAINED}>
      {facade && building && lineRow(
        <HoverText title={'Inside a building the level is computed 0.1 m in front of points along\nits façades; the loudest by Lden is shown'}>Façade</HoverText>,
        `facing ${compassPoint(facade.bearing_deg)}, loudest of ${building.facade_receivers} façade points`,
      )}
      {reflectionDb > 0 && lineRow(
        <HoverText title={REFLECTIONS}>Reflections</HoverText>,
        `+${reflectionDb.toFixed(1)} dB`,
      )}
    </Section>
  )
}

export default function CalculationDetails({ data }: { data: PopupUpdate }) {
  const periodEnergy = PERIODS.map(p => {
    const db = data.total[p.key]
    return db == null ? 0 : p.hours * 10 ** ((db + p.penalty) / 10)
  })
  const allEnergy = periodEnergy.reduce((a, b) => a + b, 0)
  const total = data.total_lden ?? 0
  const sone = data.loudness?.mean_sone
  const percentiles = data.percentiles
  const layers = data.sources
    .filter(layer => (layer.lden ?? 0) > 0)
    .sort((a, b) => (b.lden ?? 0) - (a.lden ?? 0))
  const share = (lden: number | null) => (lden == null ? '—' : `${Math.round(100 * 10 ** ((lden - total) / 10))} %`)

  return (
    <div data-testid="calculation" className={DETAIL_TEXT}>
      <ReceiverSection building={data.building} reflectionDb={data.reflection_db ?? 0} />

      <Section title="Day, evening, night" hint={PERIODS_EXPLAINED}>
        <div className="space-y-2">
          <DetailTable
            head={['', 'dB', 'Share', 'Sone']}
            rows={PERIODS.map((p, k) => [
              p.name,
              level(data.total[p.key]),
              allEnergy > 0 ? `${Math.round(100 * periodEnergy[k] / allEnergy)} %` : '—',
              sone?.[p.sone] != null ? fmtSone(sone[p.sone] as number) : '—',
            ])}
          />
          {percentiles && (
            <DetailTable
              head={[<HoverText title={PERCENTILES_EXPLAINED}>Percentile levels, dB</HoverText>, 'L5', 'L10', 'L50', 'L90']}
              rows={PERIODS.map(p => [
                p.name,
                ...[percentiles.l5, percentiles.l10, percentiles.l50, percentiles.l90].map(levels => level(levels[p.sone])),
              ])}
            />
          )}
        </div>
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
    </div>
  )
}
