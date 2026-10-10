// One segment opened: a line's length and sound power, its sound path as the computation summed it
// over its rays, and every ray, one line each: what buildings and terrain and then the ground take
// or add along it in calm air and downwind and the Lden it delivers (its dot in its colour on the
// map). A ray opens to the angle it stands for, its length, its air absorption and the ground and
// walls it was computed over; a point's one ray, which its sound path already tells, shows its
// ground alone. Opening the segment computes the click again for it alone (`piece=`), every ray
// with its ground and walls (owner 2026-10-10: "pod každým paprskem (úhlem) detail horizontů").
import { useEffect, useState } from 'react'
import { streamPopup } from '../../../lib/popup-stream'
import type { PopupPiece, RayProfile, RayTerms } from '../../../types/noise'
import { fmtDbValue, fmtInt } from '../../../utils/formatters'
import { HoverText } from '../../ui/info-tip'
import { Chevron, COLUMN_NAME, lineRow } from '../shared'
import { PathTable, signed } from '../source/PathTable'
import { offsetM, rayColors } from './fan'
import { ProfileDiagram } from './ProfileDiagram'

const SOUND_POWER = 'The segment\'s own A-weighted sound power, per meter'
const RAYS = 'The segment\'s sound is summed over the angle it fills seen from the point: five equal\n'
  + 'parts, and where buildings stand in front, a ray for every hidden stretch and gap'
const ANGLE = 'The part of the segment\'s angle, seen from the point, that the ray stands for'
const PROFILE = 'The ground under the ray, from the segment to the receiver, as the computation\n'
  + 'sampled it, and the walls it looked at'

/** The rays' columns: the ray, buildings and terrain and the ground calm and downwind, the Lden it
 *  delivers, and the chevron of what opens. */
const RAY_COLUMNS = 'grid grid-cols-[minmax(0,1fr)_repeat(5,auto)_0.75rem] items-baseline gap-x-2'

/** What a ray was computed over, once the segment's own run has answered: undefined until then,
 *  null where the answer has nothing for it. */
type Ground = { ray: RayProfile, receiverAltitudeM: number } | null | undefined

/** The `k`th ray's ground in the segment's own run (`detailed`: undefined while it computes). */
function groundOf(detailed: PopupPiece | null | undefined, k: number): Ground {
  if (detailed === undefined) return undefined
  const ray = detailed?.rays[k]?.[5]
  const receiverAltitudeM = detailed?.trace?.receiver_altitude_m
  return ray && receiverAltitudeM != null ? { ray, receiverAltitudeM } : null
}

/** The ground and walls under a ray, its altitudes from source to receiver, and the diagram's key. */
function TerrainProfile({ ground, slantM }: { ground: Ground, slantM: number }) {
  if (ground === undefined) return <div className="animate-pulse">computing…</div>
  if (!ground) return null
  const { ray, receiverAltitudeM } = ground
  const has = (building: boolean) => ray.walls.some(wall => wall[2] === building)
  return (
    <div>
      {lineRow(
        <HoverText title={PROFILE}>Terrain profile</HoverText>,
        `${Math.round(ray.source_altitude_m)} → ${Math.round(receiverAltitudeM)} m a.s.l.`,
      )}
      <ProfileDiagram ray={ray} receiverAltitudeM={receiverAltitudeM} slantM={slantM} />
      <div className={COLUMN_NAME}>
        <span className="text-red-600">●</span> source <span className="text-sky-700">●</span> receiver
        {' '}<span className="text-sky-600">- -</span> calm <span className="text-amber-600">···</span> downwind
        <br />
        <span className="text-green-600">▬</span> soft ground <span className="text-zinc-400">▬</span> hard ground
        {has(true) && <> <span className="text-zinc-500">▮</span> building</>}
        {has(false) && <> <span className="text-orange-800">▮</span> barrier</>}
      </div>
    </div>
  )
}

/** An opened ray: the angle it stands for, its length and air absorption, and its ground. */
function RayDetail({ angleRad, terms, ground }: { angleRad: number, terms: RayTerms, ground: Ground }) {
  const degrees = (angleRad * 180) / Math.PI
  return (
    <div data-testid="ray" className="col-span-7 mr-5 mb-1.5 ml-1.5 space-y-1 border-l-2 border-border/60 pl-2">
      <div>
        {lineRow(<HoverText title={ANGLE}>Angle</HoverText>, `${degrees.toFixed(degrees < 1 ? 2 : 1)}°`)}
        {lineRow('Length', `${fmtInt(terms[5])} m`)}
        {lineRow('Air absorption', signed(-terms[4]))}
      </div>
      <TerrainProfile ground={ground} slantM={terms[5]} />
    </div>
  )
}

/** Every ray of a line segment, one line each, opening to its detail. */
function RaysTable({ piece, detailed, openedRay, onRayToggle, onRayHover }: {
  piece: PopupPiece
  detailed: PopupPiece | null | undefined
  openedRay: number | null
  onRayToggle: (ray: number) => void
  onRayHover: (ray: number | null) => void
}) {
  const colors = rayColors(piece)
  const column = `text-right ${COLUMN_NAME}`
  return (
    <div data-testid="rays" className={`-mr-5 ${RAY_COLUMNS}`}>
      <span />
      <span className={`col-span-2 text-center leading-tight ${COLUMN_NAME}`}>Buildings and terrain</span>
      <span className={`col-span-2 text-center leading-tight ${COLUMN_NAME}`}>Ground effect</span>
      <span className="col-span-2" />
      <span className={COLUMN_NAME}><HoverText title={RAYS}>Rays, dB</HoverText></span>
      <span className={column}>Calm</span>
      <span className={column}>Downwind</span>
      <span className={column}>Calm</span>
      <span className={column}>Downwind</span>
      <span className={column}>Lden</span>
      <span />
      {piece.rays.map(([, , angleRad, lden, terms], k) => {
        // The terms come as losses: buildings and terrain alone, then with the ground.
        const [calm, downwind, screenedCalm, screenedDownwind] = terms
        const open = openedRay === k
        return (
          <div key={k} className="contents">
            <button
              type="button"
              aria-expanded={open}
              className={`col-span-7 grid grid-cols-subgrid items-baseline py-px text-left hover:bg-muted/40 ${open ? 'bg-muted/50' : ''}`}
              onClick={() => onRayToggle(k)}
              onMouseEnter={() => onRayHover(k)}
              onMouseLeave={() => onRayHover(null)}
            >
              <span><span style={colors[k] ? { color: colors[k] } : undefined}>●</span> {k + 1}</span>
              <span className="text-right text-foreground">{signed(-screenedCalm)}</span>
              <span className="text-right text-foreground">{signed(-screenedDownwind)}</span>
              <span className="text-right text-foreground">{signed(screenedCalm - calm)}</span>
              <span className="text-right text-foreground">{signed(screenedDownwind - downwind)}</span>
              <span className="text-right text-foreground">{fmtDbValue(lden)}</span>
              <Chevron open={open} />
            </button>
            {open && <RayDetail angleRad={angleRad} terms={terms} ground={groundOf(detailed, k)} />}
          </div>
        )
      })}
    </div>
  )
}

export function SegmentDetail({ piece, index, at, parts, openedRay, onRayToggle, onRayHover }: {
  piece: PopupPiece
  /** Its place in the row's loudest-first list, which its own run is asked by. */
  index: number
  at: [number, number]
  /** The row's parts' ids, joined by commas. */
  parts: string
  openedRay: number | null
  onRayToggle: (ray: number) => void
  onRayHover: (ray: number | null) => void
}) {
  // The segment as its own run answers it, the one piece it lists: undefined while it computes,
  // null after a failure.
  const [detailed, setDetailed] = useState<PopupPiece | null | undefined>(undefined)
  const [error, setError] = useState<string | null>(null)
  const [lat, lng] = at
  useEffect(() => {
    const controller = new AbortController()
    void streamPopup({ lat, lng }, controller.signal, {
      onUpdate: update => { if (!update.partial) setDetailed(update.pieces?.[0] ?? null) },
      onError: message => {
        setError(message)
        setDetailed(null)
      },
    }, { source: parts.split(','), piece: index })
    return () => controller.abort()
  }, [lat, lng, parts, index])

  const line = piece.ends.length > 1
  return (
    <div data-testid="segment" className="col-span-3 mr-5 mb-1.5 ml-1.5 space-y-2 border-l-2 border-border/60 pl-2">
      {line && (
        <div>
          {lineRow('Length', `${fmtInt(Math.hypot(...offsetM(piece.ends[0], piece.ends[1])))} m`)}
          {piece.emission.ld != null && lineRow(
            <HoverText title={SOUND_POWER}>Sound power</HoverText>,
            `${piece.emission.ld.toFixed(1)} dB(A)/m by day`,
          )}
        </div>
      )}
      <PathTable path={piece.path} />
      {error && <div className="text-destructive">{error}</div>}
      {piece.rays.length > 1
        ? <RaysTable piece={piece} detailed={detailed} openedRay={openedRay} onRayToggle={onRayToggle} onRayHover={onRayHover} />
        : piece.rays.length === 1 && <TerrainProfile ground={groundOf(detailed, 0)} slantM={piece.rays[0][4][5]} />}
    </div>
  )
}
