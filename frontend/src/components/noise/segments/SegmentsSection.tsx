// The professional view of a click: the loudest computed pieces of each layer, each with its data,
// what it emits, what its ray loses on the way and the ground and buildings it crosses. Collapsed;
// opening it computes the click again with the pieces listed.
import { useEffect, useState } from 'react'
import { streamPopup } from '../../../lib/popup-stream'
import type { Contributor, PeriodLevels, PopupPiece } from '../../../types/noise'
import { fmtDbValue, fmtInt } from '../../../utils/formatters'
import { HoverText } from '../../ui/info-tip'
import { contributorLabel, lineRow, SOURCE_LABELS } from '../shared'
import { MetadataRows } from '../source/MetadataRows'
import { ProfileDiagram } from './ProfileDiagram'

const PERIODS_TEXT = (levels: PeriodLevels) =>
  `${fmtDbValue(levels.ld)}/${fmtDbValue(levels.le)}/${fmtDbValue(levels.ln)}`

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
            <HoverText title={'Ground and screening together (CNOSSOS-EU 2.5),\nhomogeneous / favourable propagation'}>
              Ground + screening
            </HoverText>,
            pair(trace.boundary_db),
          )}
          {(trace.without_ground_db[0] > 0.05 || trace.without_ground_db[1] > 0.05) && lineRow(
            <HoverText title={'The screening term alone, the ground left out:\nhomogeneous / favourable'}>Screening, no ground</HoverText>,
            pair(trace.without_ground_db),
          )}
          {lineRow(
            <HoverText title="Air absorption over the ray (ISO 9613-1, the place's yearly air)">Air</HoverText>,
            `${trace.air_db.toFixed(1)} dB`,
          )}
          {lineRow(
            <HoverText title={'Share of time sound bends down towards the ground\n(downwind or at night), by day, evening and night'}>
              Favourable D/E/N
            </HoverText>,
            trace.p.map(p => `${Math.round(100 * p)}`).join('/') + ' %',
          )}
          {buildings > 0 && lineRow('Buildings crossed', String(buildings))}
          <ProfileDiagram trace={trace} crossings={piece.crossings} />
        </>
      )}
    </div>
  )
}

function SegmentRow({ piece }: { piece: PopupPiece }) {
  const [open, setOpen] = useState(false)
  return (
    <div>
      <button
        type="button"
        className="w-full flex justify-between gap-2 text-left hover:bg-muted/30"
        aria-expanded={open}
        onClick={() => setOpen(!open)}
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

export function SegmentsSection({ lat, lng }: { lat: number; lng: number }) {
  const [open, setOpen] = useState(false)
  const [pieces, setPieces] = useState<PopupPiece[] | null>(null)
  const [error, setError] = useState<string | null>(null)
  useEffect(() => {
    if (!open) return
    const controller = new AbortController()
    setPieces(null)
    setError(null)
    void streamPopup({ lat, lng }, controller.signal, {
      onUpdate: update => { if (!update.partial) setPieces(update.pieces ?? []) },
      onError: setError,
    }, { segments: true })
    return () => controller.abort()
  }, [open, lat, lng])
  const layers = pieces ? Object.keys(SOURCE_LABELS).filter(layer => pieces.some(p => p.source_type === layer)) : []
  return (
    <div data-testid="segments" className="mt-2 text-[11px] font-mono text-muted-foreground">
      <button
        type="button"
        className="text-[11px] font-medium uppercase tracking-[0.08em] hover:text-foreground"
        aria-expanded={open}
        onClick={() => setOpen(!open)}
      >
        {open ? '▾' : '▸'} Segments: how it is computed
      </button>
      {open && !pieces && !error && <div className="animate-pulse">computing…</div>}
      {error && <div className="text-destructive">{error}</div>}
      {pieces && layers.map(layer => (
        <div key={layer} className="mt-1">
          <div className="text-muted-foreground/70">{SOURCE_LABELS[layer] ?? layer}</div>
          {pieces.filter(p => p.source_type === layer).map((piece, k) => (
            <SegmentRow key={`${piece.id}-${k}`} piece={piece} />
          ))}
        </div>
      ))}
    </div>
  )
}
