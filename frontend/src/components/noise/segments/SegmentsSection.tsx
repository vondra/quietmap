// An opened row's segments, the popup's second level (owner 2026-10-10): how all of its sound
// arrives in calm air (the share reaching the point over a free line of sight, over buildings or
// walls and over terrain, with what they take of it), then its loudest segments, loudest first,
// each opening to its sound path and its rays, a ray to the ground and walls under it. The
// segments are the computation's pieces: opening the section computes the same click again with
// the row's parts (`source=`), every piece of them, and the map draws the listed ones, the
// selected one's rays and the selected ray. That answer never replaces the click's own.
import { useEffect, useMemo, useState } from 'react'
import { streamPopup } from '../../../lib/popup-stream'
import type { Contributor, PieceArrival, PopupPiece, SegmentFan } from '../../../types/noise'
import { fmtDbValue, fmtInt, fmtPercent, wholePercents } from '../../../utils/formatters'
import { HoverText } from '../../ui/info-tip'
import { partLabel } from '../labels'
import { Chevron, COLUMN_NAME, compassPoint, DetailTable, formatDist } from '../shared'
import { signed } from '../source/PathTable'
import { offsetM, segmentColors, segmentFan } from './fan'
import { SegmentDetail } from './SegmentDetail'

/** The answered click a row's segments are computed for, and the map they are drawn on. */
export interface SegmentsClick {
  /** The clicked point, the answer's `center`: the same click is computed again with the row. */
  at: [number, number]
  /** Where every level is computed: the clicked point, or a building's façade. */
  receiver: [number, number]
  /** Draws a row's segments on the map; null takes that row's away. */
  onFan: (source: string, fan: SegmentFan | null) => void
}

const ARRIVAL_ROWS: Record<PieceArrival['edges'][number]['edge'], string> = {
  open: 'Line of sight',
  buildings: 'Over buildings',
  terrain: 'Over terrain',
}

const IN_CALM_AIR = 'The sound reaching the point by what it passes when the air is calm:\n'
  + 'nothing in the way, or the top of a building, wall or hill it bends over'
const SCREENING = 'What the buildings, walls and terrain in the way take of that sound'

/** How all of the row's sound arrives in calm air. */
function ArrivalTable({ arrival }: { arrival: PieceArrival }) {
  const percents = wholePercents(arrival.edges.map(edge => edge.share))
  return (
    <DetailTable
      head={[
        <HoverText title={IN_CALM_AIR}>In calm air</HoverText>,
        'Share',
        <HoverText title={SCREENING}>dB</HoverText>,
      ]}
      rows={arrival.edges.map((edge, k) => [ARRIVAL_ROWS[edge.edge], fmtPercent(percents[k]), signed(edge.screening_db)])}
    />
  )
}

/** A segment's line in the list: its colour on the map, its direction and distance from the point,
 *  the part of the row it belongs to (a row of several), and the Lden it delivers. */
function SegmentRow({ piece, color, part, open, onToggle, onHover }: {
  piece: PopupPiece
  color: string
  part: string | null
  open: boolean
  onToggle: () => void
  onHover: (hovered: boolean) => void
}) {
  const ray = piece.trace?.ray
  const [east, north] = ray ? offsetM(ray[1], ray[0]) : [0, 0]
  return (
    <button
      type="button"
      className={`col-span-3 grid grid-cols-subgrid items-baseline py-px text-left hover:bg-muted/40 ${open ? 'bg-muted/50' : ''}`}
      aria-expanded={open}
      onClick={onToggle}
      onMouseEnter={() => onHover(true)}
      onMouseLeave={() => onHover(false)}
    >
      <span className="truncate">
        <span style={{ color }}>●</span>
        <span className="ml-1 inline-block w-6">{ray ? compassPoint((Math.atan2(east, north) * 180) / Math.PI) : ''}</span>
        {formatDist(Math.round(piece.distance_m))}
        {part && <span className="ml-1.5">{part}</span>}
      </span>
      <span className="text-right text-foreground">{fmtDbValue(piece.received.lden)}</span>
      <Chevron open={open} />
    </button>
  )
}

/** The answer of the click computed with the row: its listed pieces and how all of it arrives. */
interface Segments {
  pieces: PopupPiece[]
  arrival: PieceArrival
}

export function SegmentsSection({ c, click }: { c: Contributor, click: SegmentsClick }) {
  const [open, setOpen] = useState(false)
  // Asked for once, when first opened; the answer stays while the row is open.
  const [asked, setAsked] = useState(false)
  const [segments, setSegments] = useState<Segments | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [opened, setOpened] = useState<number | null>(null)
  const [hovered, setHovered] = useState<number | null>(null)
  // The opened segment's opened and hovered ray.
  const [openedRay, setOpenedRay] = useState<number | null>(null)
  const [hoveredRay, setHoveredRay] = useState<number | null>(null)
  const parts = (c.parts?.map(part => part.id) ?? [c.id]).join(',')
  const [lat, lng] = click.at
  useEffect(() => {
    if (!asked) return
    const controller = new AbortController()
    void streamPopup({ lat, lng }, controller.signal, {
      onUpdate: update => {
        if (!update.partial) setSegments({ pieces: update.pieces ?? [], arrival: update.arrival ?? { pieces: 0, edges: [] } })
      },
      onError: setError,
    }, { source: parts.split(',') })
    return () => controller.abort()
  }, [asked, lat, lng, parts])

  const { onFan } = click
  const [receiverLat, receiverLng] = click.receiver
  const selected = hovered ?? opened
  const selectedRay = selected === opened ? hoveredRay ?? openedRay : null
  const fan = useMemo(
    () => (open && segments?.pieces.length
      ? segmentFan(c.id, segments.pieces, [receiverLat, receiverLng], { selected, selectedRay, opened })
      : null),
    [open, segments, c.id, receiverLat, receiverLng, selected, selectedRay, opened],
  )
  useEffect(() => { onFan(c.id, fan) }, [onFan, c.id, fan])
  useEffect(() => () => onFan(c.id, null), [onFan, c.id])

  const pieces = segments?.pieces ?? []
  const colors = segmentColors(pieces)
  const several = (c.parts?.length ?? 0) > 1
  return (
    <div data-testid="segments" className="space-y-2">
      {/* The chevron stands in the rows' chevron column, the numbers under the rows' shares. */}
      <div className="-mr-5">
        <button
          type="button"
          aria-expanded={open}
          className="flex w-full items-center justify-between py-0.5 text-left text-foreground hover:bg-muted/40"
          onClick={() => {
            setOpen(!open)
            setAsked(true)
          }}
        >
          <span>Segments{segments ? ` (${fmtInt(segments.arrival.pieces)})` : ''}</span>
          <Chevron open={open} />
        </button>
      </div>
      {open && error && <div className="text-destructive">{error}</div>}
      {open && !segments && !error && <div className="animate-pulse">computing…</div>}
      {open && segments && (
        <>
          {segments.arrival.edges.length > 0 && <ArrivalTable arrival={segments.arrival} />}
          {pieces.length > 0 && (
            <div className="-mr-5 grid grid-cols-[minmax(0,1fr)_auto_0.75rem] items-baseline gap-x-2">
              <span className={COLUMN_NAME}>
                {pieces.length < segments.arrival.pieces ? `${pieces.length} loudest` : 'Segment'}
              </span>
              <span className={`text-right ${COLUMN_NAME}`}>dB Lden</span>
              <span />
              {pieces.map((piece, index) => (
                <div key={index} className="contents">
                  <SegmentRow
                    piece={piece}
                    color={colors[index]}
                    part={several ? partLabel(piece.metadata, c.name) : null}
                    open={opened === index}
                    onToggle={() => {
                      setOpened(opened === index ? null : index)
                      setOpenedRay(null)
                      setHoveredRay(null)
                    }}
                    onHover={on => setHovered(on ? index : null)}
                  />
                  {opened === index && (
                    <SegmentDetail
                      piece={piece}
                      index={index}
                      at={click.at}
                      parts={parts}
                      openedRay={openedRay}
                      onRayToggle={ray => setOpenedRay(openedRay === ray ? null : ray)}
                      onRayHover={setHoveredRay}
                    />
                  )}
                </div>
              ))}
            </div>
          )}
        </>
      )}
    </div>
  )
}
