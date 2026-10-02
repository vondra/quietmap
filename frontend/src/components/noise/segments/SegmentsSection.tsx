// The professional view of a click: where the level is computed, then each layer's loudest
// computed pieces, each with its data, what it emits, what its ray loses on the way and the ground,
// buildings and walls it crosses; their rays drawn on the map from the receiver. Opening it computes
// the click again with the pieces listed.
import { useEffect, useState } from 'react'
import { streamPopup } from '../../../lib/popup-stream'
import type { BuildingAnswer, Contributor, PeriodLevels, PopupPiece, SegmentFan } from '../../../types/noise'
import { fmtDbValue, fmtInt } from '../../../utils/formatters'
import { HoverText } from '../../ui/info-tip'
import { contributorLabel, lineRow, SOURCE_LABELS } from '../shared'
import { MetadataRows } from '../source/MetadataRows'
import { ProfileDiagram } from './ProfileDiagram'

const PERIODS_TEXT = (levels: PeriodLevels) =>
  `${fmtDbValue(levels.ld)}/${fmtDbValue(levels.le)}/${fmtDbValue(levels.ln)}`

const COMPASS_POINTS = ['N', 'NE', 'E', 'SE', 'S', 'SW', 'W', 'NW'] as const

function compassPoint(bearingDeg: number): string {
  return COMPASS_POINTS[Math.round((((bearingDeg % 360) + 360) % 360) / 45) % 8]
}

/** A piece as a contributor, so that its data reads as the source rows read it. */
function asContributor(piece: PopupPiece): Contributor {
  const m = piece.metadata ?? {}
  const text = (key: string) => (typeof m[key] === 'string' ? m[key] as string : '')
  return {
    id: piece.id,
    source_type: piece.source_type,
    name: text('name') || text('ref'),
    subtype: text('road_class') || null,
    distance_m: piece.distance_m,
    received_lden: piece.received.lden,
    received: piece.received,
    metadata: piece.metadata,
  }
}

/** Where the level is computed: the click, or the loudest façade of the building clicked. */
function ReceiverRows({ lat, lng, building, elevationM, reflectionDb }: {
  lat: number
  lng: number
  building: BuildingAnswer | null
  elevationM: number
  reflectionDb: number
}) {
  const facade = building?.facade ?? null
  const [rLat, rLng] = facade ? facade.receiver : [lat, lng]
  return (
    <div className="mb-1.5">
      {lineRow(
        <HoverText title={'Every level of this click is computed here,\n4 m above the ground (EU noise mapping)'}>Receiver</HoverText>,
        facade
          ? `façade facing ${compassPoint(facade.bearing_deg)}`
          : 'the clicked point',
      )}
      {facade && building && lineRow(
        <HoverText title={'Inside a building the level is computed 0.1 m in front of\nits façades; the loudest by Lden is shown'}>Façade</HoverText>,
        `loudest of ${building.facade_receivers} façade points`,
      )}
      {lineRow('Position', `${rLat.toFixed(5)}, ${rLng.toFixed(5)}`)}
      {elevationM > 0 && lineRow('Ground', `${Math.round(elevationM)} m a.s.l.`)}
      {reflectionDb > 0 && lineRow(
        <HoverText title={'Walls close behind the receiver reflect sound back to it\n(CNOSSOS-EU): added to every source'}>Reflection</HoverText>,
        `+${reflectionDb.toFixed(1)} dB`,
      )}
    </div>
  )
}

function SegmentDetail({ piece }: { piece: PopupPiece }) {
  const trace = piece.trace
  const line = piece.ends.length > 1
  const pair = (values: [number, number]) => `${values[0].toFixed(1)} / ${values[1].toFixed(1)} dB`
  const buildings = new Set(piece.crossings.map(([, , id]) => id)).size
  return (
    <div className="ml-2 mb-1.5">
      <MetadataRows c={asContributor(piece)} />
      {lineRow(
        <HoverText title={line ? 'Sound power per metre of this piece (A-weighted)' : 'Sound power (A-weighted)'}>
          Emission D/E/N
        </HoverText>,
        `${PERIODS_TEXT(piece.emission)} dB(A)${line ? '/m' : ''}`,
      )}
      {lineRow('Received D/E/N', `${PERIODS_TEXT(piece.received)} dB`)}
      {trace && (
        <>
          {lineRow('Ray', `${fmtInt(trace.slant_m)} m`)}
          {lineRow(
            <HoverText title={'Ground and screening together (CNOSSOS-EU 2.5): calm air\n(straight ray) / sound bent down by wind or inversion'}>
              Ground + screening
            </HoverText>,
            pair(trace.boundary_db),
          )}
          {(trace.without_ground_db[0] > 0.05 || trace.without_ground_db[1] > 0.05) && lineRow(
            <HoverText title={'The screening term alone, the ground left out:\ncalm air / bent down'}>Screening, no ground</HoverText>,
            pair(trace.without_ground_db),
          )}
          {lineRow(
            <HoverText title="Air absorption over the ray (ISO 9613-1, the place's yearly air)">Air</HoverText>,
            `${trace.air_db.toFixed(1)} dB`,
          )}
          {lineRow(
            <HoverText title={'Share of time the sound bends down towards the ground\n(downwind or at night), by day, evening and night;\nthe rest is calm air'}>
              Bent down D/E/N
            </HoverText>,
            trace.p.map(p => `${Math.round(100 * p)}`).join('/') + ' %',
          )}
          {buildings > 0 && lineRow('Buildings crossed', String(buildings))}
          <ProfileDiagram trace={trace} crossings={piece.crossings} />
          <div className="text-[10px] text-muted-foreground/70">
            <span className="text-red-600">●</span> source <span className="text-sky-700">●</span> receiver
            {' '}<span className="text-sky-600">- -</span> calm air <span className="text-amber-600">···</span> bent down
          </div>
        </>
      )}
    </div>
  )
}

function SegmentRow({ piece, open, onToggle }: { piece: PopupPiece, open: boolean, onToggle: () => void }) {
  return (
    <div>
      <button
        type="button"
        className="w-full flex justify-between gap-2 text-left hover:bg-muted/30"
        aria-expanded={open}
        onClick={onToggle}
      >
        <span className="truncate">{contributorLabel(asContributor(piece))}</span>
        <span className="shrink-0 text-foreground">
          {fmtDbValue(piece.received.lden)} dB · {fmtInt(piece.distance_m)} m
        </span>
      </button>
      {open && <SegmentDetail piece={piece} />}
    </div>
  )
}

export function SegmentsSection({ lat, lng, building, elevationM, reflectionDb, onFan }: {
  lat: number
  lng: number
  building: BuildingAnswer | null
  elevationM: number
  reflectionDb: number
  /** Draws the listed pieces' rays on the map; null clears them. */
  onFan?: (fan: SegmentFan | null) => void
}) {
  const [pieces, setPieces] = useState<PopupPiece[] | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [opened, setOpened] = useState<number | null>(null)
  useEffect(() => {
    const controller = new AbortController()
    setPieces(null)
    setError(null)
    setOpened(null)
    void streamPopup({ lat, lng }, controller.signal, {
      onUpdate: update => { if (!update.partial) setPieces(update.pieces ?? []) },
      onError: setError,
    }, { segments: true })
    return () => controller.abort()
  }, [lat, lng])
  const receiver: [number, number] = building?.facade?.receiver ?? [lat, lng]
  useEffect(() => {
    if (!onFan) return
    const rays = (pieces ?? []).flatMap((piece, k) => piece.trace?.ray
      ? [{ ray: piece.trace.ray, lden: piece.received.lden ?? 0, selected: k === opened }]
      : [])
    onFan(rays.length ? { receiver, rays } : null)
    // The receiver is read from `lat`, `lng` and the building, all fixed for one click.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [pieces, opened, onFan])
  useEffect(() => () => onFan?.(null), [onFan])
  const layers = pieces ? Object.keys(SOURCE_LABELS).filter(layer => pieces.some(p => p.source_type === layer)) : []
  return (
    <div data-testid="segments" className="text-[11px] font-mono text-muted-foreground">
      <ReceiverRows lat={lat} lng={lng} building={building} elevationM={elevationM} reflectionDb={reflectionDb} />
      {!pieces && !error && <div className="animate-pulse">computing…</div>}
      {error && <div className="text-destructive">{error}</div>}
      {pieces && layers.map(layer => (
        <div key={layer} className="mt-1">
          <div className="text-muted-foreground/70">{SOURCE_LABELS[layer] ?? layer}</div>
          {pieces.map((piece, k) => piece.source_type === layer && (
            <SegmentRow
              key={`${piece.id}-${k}`}
              piece={piece}
              open={opened === k}
              onToggle={() => setOpened(opened === k ? null : k)}
            />
          ))}
        </div>
      ))}
    </div>
  )
}
