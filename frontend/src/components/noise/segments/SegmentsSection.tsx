// The professional view of a click: where its level is computed, then per layer, loudest first,
// its sources with the pieces of them the click computed (the loudest of the layer). Each piece
// row reads what the ground and the screening take from its ray in calm air and when the sound is
// bent down, and the Lden it delivers; opened, its data, sound power, ray and ground profile.
// The pieces and their rays are drawn on the map in the colour of their row's dot. Opening the
// view computes the click again with the pieces listed.
import { useEffect, useMemo, useState, type ReactNode } from 'react'
import { streamPopup } from '../../../lib/popup-stream'
import type { BuildingAnswer, Contributor, LayerLevels, PopupPiece, RayTerms, SegmentFan } from '../../../types/noise'
import { fmtInt } from '../../../utils/formatters'
import { FadingText } from '../../ui/fading-text'
import { HoverText } from '../../ui/info-tip'
import { contributorLabel, SOURCE_LABELS } from '../labels'
import { Chevron, COLUMN_NAME, formatDist, lineRow } from '../shared'
import { MetadataRows } from '../source/MetadataRows'
import { ProfileDiagram } from './ProfileDiagram'

/** One grid for the whole list: label, ground and screening in calm air and bent down, Lden, and
 *  the chevron of what opens. */
const GRID = 'grid grid-cols-[minmax(0,1fr)_2.9rem_2.9rem_2.6rem_0.75rem] gap-x-2'
const COMPASS_POINTS = ['N', 'NE', 'E', 'SE', 'S', 'SW', 'W', 'NW'] as const
const EARTH_M_PER_DEGREE = 111_320
/** The map first frames the listed pieces within this of the loudest: those that make the level
 *  (a piece 20 dB under the loudest adds a hundredth of its energy). */
const OVERVIEW_BELOW_LOUDEST_DB = 20

/** A piece's colour in the list and on the map by how far below the loudest listed piece it is:
 *  red the loudest, violet 10 dB below, blue 20, grey 30 and more. */
const COLOR_STOPS: [number, [number, number, number]][] = [
  [-30, [0x94, 0xa3, 0xb8]],
  [-20, [0x25, 0x63, 0xeb]],
  [-10, [0x7c, 0x3a, 0xed]],
  [0, [0xdc, 0x26, 0x26]],
]

function pieceColor(belowLoudestDb: number): string {
  const db = Math.min(0, Math.max(-30, belowLoudestDb))
  const k = Math.max(1, COLOR_STOPS.findIndex(([at]) => db <= at))
  const [[from, a], [to, b]] = [COLOR_STOPS[k - 1], COLOR_STOPS[k]]
  const t = (db - from) / (to - from)
  return `#${[0, 1, 2].map(c => Math.round(a[c] + t * (b[c] - a[c])).toString(16).padStart(2, '0')).join('')}`
}

function compassPoint(bearingDeg: number): string {
  return COMPASS_POINTS[Math.round((((bearingDeg % 360) + 360) % 360) / 45) % 8]
}

/** East and north metres from `a` to `b`, both [lat, lon]. */
function offsetM(a: [number, number], b: [number, number]): [number, number] {
  return [
    (b[1] - a[1]) * EARTH_M_PER_DEGREE * Math.cos((a[0] * Math.PI) / 180),
    (b[0] - a[0]) * EARTH_M_PER_DEGREE,
  ]
}

/** What a term does to the level: a loss with a minus, a gain with a plus, one decimal. */
function term(attenuationDb: number): string {
  if (Math.abs(attenuationDb) < 0.05) return '0.0'
  return `${attenuationDb > 0 ? '−' : '+'}${Math.abs(attenuationDb).toFixed(1)}`
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

/** Where every level of the click is computed, when it is not the clicked point itself: a
 *  building's façade, and the reflection of walls close behind the receiver. */
function ReceiverRows({ building, reflectionDb }: { building: BuildingAnswer | null, reflectionDb: number }) {
  const facade = building?.facade ?? null
  if (!facade && reflectionDb <= 0) return null
  return (
    <div className="mb-2">
      {facade && building && (
        <>
          {lineRow(
            <HoverText title={'Inside a building the level is computed 0.1 m in front of its\nfaçades, 4 m up; the loudest by Lden is shown'}>Receiver</HoverText>,
            `façade facing ${compassPoint(facade.bearing_deg)}`,
          )}
          {lineRow('Façade', `loudest of ${building.facade_receivers} façade points`)}
        </>
      )}
      {reflectionDb > 0 && lineRow(
        <HoverText title="Walls close behind the receiver reflect sound back to it (CNOSSOS-EU), for every source">Reflection</HoverText>,
        `+${reflectionDb.toFixed(1)} dB`,
      )}
    </div>
  )
}

interface SummedRay {
  from: [number, number]
  /** What reaches the receiver along it per radian of the piece's angle, against the clearest
   *  ray of the piece (dB, 0 for the clearest): what the way takes, the divergence apart. */
  belowClearestDb: number
  terms: RayTerms
}

/** The rays a piece was summed over, each with how much less reaches the receiver along it than
 *  along the piece's clearest ray, and its terms. */
function summedRays(piece: PopupPiece): SummedRay[] {
  const all = piece.rays ?? []
  // A line's rays stand for their angle; a point source has one ray, of no angle.
  const rays = all.flatMap(([lat, lon, angle, lden, terms]) => lden == null || (angle <= 0 && all.length > 1)
    ? []
    : [{ from: [lat, lon] as [number, number], density: angle > 0 ? lden - 10 * Math.log10(angle) : lden, terms: terms ?? null }])
  const clearest = Math.max(...rays.map(ray => ray.density))
  return rays.map(({ from, density, terms }) => ({ from, belowClearestDb: density - clearest, terms }))
}

/** Every ray of a piece with its terms, one row each: what reaches along it against the clearest
 *  ray, its length, ground and screening in calm air and bent down, the air. */
function RayTable({ rays }: { rays: SummedRay[] }) {
  const cell = 'text-right tabular-nums text-foreground'
  return (
    <div data-testid="ray-table" className="grid grid-cols-[1.4rem_3.2rem_3.4rem_3.2rem_3.2rem_2.6rem] gap-x-1.5 mt-1">
      <span className={COLUMN_NAME}>#</span>
      <span className={`text-right ${COLUMN_NAME}`}>
        <HoverText title={'What reaches the receiver along the ray, for its share of the\npiece, against the clearest ray (geometric spreading aside)'}>reaches</HoverText>
      </span>
      <span className={`text-right ${COLUMN_NAME}`}>m</span>
      <span className={`text-right ${COLUMN_NAME}`}>
        <HoverText title="Ground and screening together, in calm air">calm</HoverText>
      </span>
      <span className={`text-right ${COLUMN_NAME}`}>
        <HoverText title="Ground and screening together, the sound bent down (wind from the source, inversion)">bent</HoverText>
      </span>
      <span className={`text-right ${COLUMN_NAME}`}>air</span>
      {rays.map((ray, k) => (
        <RayRow key={k} index={k + 1} ray={ray} cell={cell} />
      ))}
    </div>
  )
}

function RayRow({ index, ray, cell }: { index: number, ray: SummedRay, cell: string }) {
  const t = ray.terms
  return (
    <>
      <span className="text-muted-foreground">{index}</span>
      <span className={cell}>{ray.belowClearestDb > -0.05 ? '0.0' : ray.belowClearestDb.toFixed(1)}</span>
      <span className={cell}>{t ? fmtInt(t[5]) : '–'}</span>
      <span className={cell}>{t ? term(t[0]) : '–'}</span>
      <span className={cell}>{t ? term(t[1]) : '–'}</span>
      <span className={cell}>{t ? term(t[4]) : '–'}</span>
    </>
  )
}

/** One piece opened: its data, sound power, rays, the terms of its nearest ray and the ground
 *  under that ray. */
function PieceDetail({ piece }: { piece: PopupPiece }) {
  const trace = piece.trace
  const line = piece.ends.length > 1
  const length = line ? Math.hypot(...offsetM(piece.ends[0], piece.ends[1])) : 0
  const buildings = new Set(piece.crossings.map(([, , id]) => id)).size
  const rays = summedRays(piece)
  const weakest = Math.min(...rays.map(ray => ray.belowClearestDb))
  const [showRays, setShowRays] = useState(false)
  const share = (p: number) => `${Math.round(100 * p)} %`
  const row = (label: ReactNode, calm: string, bent: string) => (
    <>
      <span className="min-w-0 truncate">{label}</span>
      <span className="text-right tabular-nums text-foreground">{calm}</span>
      <span className="text-right tabular-nums text-foreground">{bent}</span>
    </>
  )
  return (
    <div data-testid="segment-piece" className="col-span-5 ml-3 mt-0.5 mb-2 space-y-1 pl-2 border-l-2 border-border/60">
      <MetadataRows c={asContributor(piece)} />
      {lineRow(
        <HoverText title="This piece's own A-weighted sound power by day, per metre of a road or track">This piece</HoverText>,
        `${piece.emission.ld?.toFixed(1) ?? '–'} dB(A)${line ? '/m' : ''} by day`,
      )}
      {length >= 1 && lineRow('Piece', `${fmtInt(length)} m long, ${formatDist(Math.round(piece.distance_m))} away`)}
      {rays.length > 1 && lineRow(
        <HoverText title={'The piece is summed over the angle it fills from the receiver: five\nequal parts, and behind buildings a ray for every hidden stretch and\nfree gap (CNOSSOS-EU). On the map each ray is coloured by what reaches\nthe receiver along it: red the clearest, violet 10 dB less, blue 20,\ngrey 30 and more'}>
          Rays summed
        </HoverText>,
        <button
          type="button"
          className="inline-flex items-center gap-1 hover:bg-muted/40"
          onClick={() => setShowRays(shown => !shown)}
          aria-expanded={showRays}
        >
          {weakest < -0.05 ? `${rays.length}, weakest ${term(-weakest)} dB` : `${rays.length}, all clear alike`}
          <Chevron open={showRays} />
        </button>,
      )}
      {rays.length > 1 && showRays && <RayTable rays={rays} />}
      {trace && (
        <>
          {lineRow(
            <HoverText title="The ray from the piece's nearest point: the terms below and the profile are its">Nearest ray</HoverText>,
            `${fmtInt(trace.slant_m)} m, ${Math.round(trace.source_altitude_m)} → ${Math.round(trace.receiver_altitude_m)} m a.s.l.`,
          )}
          {buildings > 0 && lineRow('Buildings crossed', String(buildings))}
          <div className="grid grid-cols-[minmax(0,1fr)_3.6rem_3.6rem] gap-x-2">
            <span />
            <span className={`text-right ${COLUMN_NAME}`}>calm air</span>
            <span className={`text-right ${COLUMN_NAME}`}>bent down</span>
            {row(
              <HoverText title="Ground reflection and screening by terrain, buildings and walls together (CNOSSOS-EU)">Ground + screening</HoverText>,
              term(trace.boundary_db[0]),
              term(trace.boundary_db[1]),
            )}
            {(trace.without_ground_db[0] > 0.05 || trace.without_ground_db[1] > 0.05) && row(
              <HoverText title="The same with the ground left out: what the obstacles alone take">Screening alone</HoverText>,
              term(trace.without_ground_db[0]),
              term(trace.without_ground_db[1]),
            )}
            {row(
              <HoverText title="Absorption in the air over the ray (ISO 9613-1, the place's yearly air)">Air</HoverText>,
              term(trace.air_db),
              term(trace.air_db),
            )}
            {row(
              <HoverText title={'How often the sound travels each way: bent down when the wind blows\nfrom the source or the air is inverted, mostly at night (ERA5)'}>Share of the day</HoverText>,
              share(1 - trace.p[0]),
              share(trace.p[0]),
            )}
            {row('Share of the night', share(1 - trace.p[2]), share(trace.p[2]))}
          </div>
          <ProfileDiagram trace={trace} crossings={piece.crossings} />
          <div className={COLUMN_NAME}>
            <span className="text-red-600">●</span> source <span className="text-sky-700">●</span> receiver
            {' '}<span className="text-sky-600">- -</span> calm air <span className="text-amber-600">···</span> bent down
          </div>
        </>
      )}
    </div>
  )
}

interface ListedPiece {
  piece: PopupPiece
  /** Its index in the click's listing. */
  index: number
  lden: number
  color: string
}

interface SourceGroup {
  key: string
  label: string
  /** The whole source's Lden. */
  lden: number
  pieces: ListedPiece[]
}

interface LayerGroup {
  layer: LayerLevels
  sources: SourceGroup[]
}

/** The layers above 0 dB, loudest first, each with its sources and their pieces above 0 dB. */
function groupPieces(pieces: PopupPiece[], layers: LayerLevels[]): LayerGroup[] {
  const loudest = Math.max(...pieces.map(piece => piece.received.lden ?? -Infinity))
  return layers
    .filter(layer => (layer.lden ?? 0) > 0)
    .sort((a, b) => (b.lden ?? 0) - (a.lden ?? 0))
    .map(layer => {
      const groups = new Map<string, ListedPiece[]>()
      pieces.forEach((piece, index) => {
        const lden = piece.received.lden
        if (piece.source_type !== layer.source_type || lden == null || lden <= 0) return
        const list = groups.get(piece.id) ?? []
        list.push({ piece, index, lden, color: pieceColor(lden - loudest) })
        groups.set(piece.id, list)
      })
      const sources = [...groups.entries()].map(([id, list]) => ({
        key: `${layer.source_type}:${id}`,
        label: contributorLabel(asContributor(list[0].piece)),
        lden: list[0].piece.source_lden ?? 0,
        pieces: list.sort((a, b) => b.lden - a.lden),
      })).sort((a, b) => b.lden - a.lden)
      return { layer, sources }
    })
}

/** How a layer was computed, under its heading. */
function layerNote(layer: LayerLevels): ReactNode {
  if (layer.source_type === 'aircraft') {
    return (
      <HoverText title={'A box sums the year\'s flights through one map cell at one height for\none aircraft group; an airport\'s ground operations are pieces like a\nroad\'s. The loudest flights are in the Aircraft row of the list'}>
        a year of ADS-B flights (ECAC Doc 29): {fmtInt(layer.evaluated)} boxes and pieces computed
      </HoverText>
    )
  }
  if (layer.evaluated >= layer.candidates) return `all ${fmtInt(layer.candidates)} pieces within reach computed`
  return (
    <HoverText title={'The rest are far or screened: together they add at most 0.1 dB,\nor a weighted sample of them stands for them within 0.05 dB'}>
      {fmtInt(layer.evaluated)} of {fmtInt(layer.candidates)} pieces within reach computed
    </HoverText>
  )
}

/** A listed piece's row: its colour on the map, direction and distance, its ray's ground and
 *  screening in calm air and bent down, and the Lden it delivers. */
function PieceRow({ listed, open, onToggle, onHover }: {
  listed: ListedPiece
  open: boolean
  onToggle: () => void
  onHover: (hovered: boolean) => void
}) {
  const { piece, lden, color } = listed
  const trace = piece.trace
  const [east, north] = trace?.ray ? offsetM(trace.ray[1], trace.ray[0]) : [0, 0]
  const direction = trace?.ray ? compassPoint((Math.atan2(east, north) * 180) / Math.PI) : ''
  return (
    <>
      <button
        type="button"
        className={`col-span-5 grid grid-cols-subgrid items-baseline py-px pl-3 text-left hover:bg-muted/40 ${open ? 'bg-muted/50' : ''}`}
        aria-expanded={open}
        onClick={onToggle}
        onMouseEnter={() => onHover(true)}
        onMouseLeave={() => onHover(false)}
      >
        <span className="truncate">
          <span style={{ color }}>●</span> <span className="inline-block w-5">{direction}</span> {formatDist(Math.round(piece.distance_m))}
        </span>
        <span className="text-right tabular-nums text-foreground">{trace ? term(trace.boundary_db[0]) : '–'}</span>
        <span className="text-right tabular-nums text-foreground">{trace ? term(trace.boundary_db[1]) : '–'}</span>
        <span className="text-right tabular-nums text-foreground">{lden.toFixed(1)}</span>
        <Chevron open={open} />
      </button>
      {open && <PieceDetail piece={piece} />}
    </>
  )
}

/** How the level comes about, for the view's heading. */
export const SEGMENTS_EXPLAINED = 'Every source within reach is cut into pieces. The sound of each piece\n'
  + 'is followed along its rays over the terrain and past buildings, in calm\n'
  + 'air and bent down by the wind or a night inversion, each as often as it\n'
  + 'happens, to 4 m above the ground. The level is the energy sum of all\n'
  + 'pieces; the loudest are listed and drawn on the map.'

export function SegmentsSection({ lat, lng, building, reflectionDb, layers, onFan }: {
  lat: number
  lng: number
  building: BuildingAnswer | null
  reflectionDb: number
  /** The click's layers. */
  layers: LayerLevels[]
  /** Draws the listed pieces and their rays on the map; null clears them. */
  onFan?: (fan: SegmentFan | null) => void
}) {
  const [pieces, setPieces] = useState<PopupPiece[] | null>(null)
  const [error, setError] = useState<string | null>(null)
  // The open sources by key; until one is toggled, the loudest source of each layer.
  const [open, setOpen] = useState<Set<string> | null>(null)
  const [opened, setOpened] = useState<number | null>(null)
  const [hovered, setHovered] = useState<number | null>(null)
  useEffect(() => {
    const controller = new AbortController()
    setPieces(null)
    setError(null)
    setOpen(null)
    setOpened(null)
    void streamPopup({ lat, lng }, controller.signal, {
      onUpdate: update => { if (!update.partial) setPieces(update.pieces ?? []) },
      onError: setError,
    }, { segments: true })
    return () => controller.abort()
  }, [lat, lng])
  const grouped = useMemo(
    () => (pieces ? groupPieces(pieces, layers) : []),
    [pieces, layers],
  )
  const openKeys = open ?? new Set(grouped.flatMap(({ sources }) => sources.slice(0, 1).map(s => s.key)))
  const receiver: [number, number] = building?.facade?.receiver ?? [lat, lng]
  const selected = hovered ?? opened
  useEffect(() => {
    if (!onFan) return
    const listed = grouped.flatMap(({ sources }) => sources.flatMap(s => s.pieces))
    const drawn = listed.flatMap(({ piece, index, color }) =>
      piece.trace?.ray ? [{ ends: piece.ends, ray: piece.trace.ray, color, selected: index === selected }] : [])
    const chosen = listed.find(({ index }) => index === selected)?.piece
    const rays = chosen ? summedRays(chosen).map(({ from, belowClearestDb }) => ({ from, color: pieceColor(belowClearestDb) })) : []
    const loudest = Math.max(...listed.map(({ lden }) => lden))
    const overview = listed
      .filter(({ lden }) => lden >= loudest - OVERVIEW_BELOW_LOUDEST_DB)
      .flatMap(({ piece }) => [...piece.ends, ...(piece.trace?.ray ? [piece.trace.ray[0]] : [])])
    const open = listed.find(({ index }) => index === opened)?.piece
    onFan(drawn.length
      ? {
          receiver,
          pieces: drawn,
          rays,
          overview,
          opened: open && opened !== null
            ? { index: opened, points: [...open.ends, ...summedRays(open).map(ray => ray.from)] }
            : null,
        }
      : null)
    // The receiver is read from `lat`, `lng` and the building, all fixed for one click.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [grouped, selected, opened, onFan])
  useEffect(() => () => onFan?.(null), [onFan])
  const toggle = (key: string) => {
    const next = new Set(openKeys)
    if (next.has(key)) next.delete(key)
    else next.add(key)
    setOpen(next)
  }
  return (
    <div data-testid="segments">
      <ReceiverRows building={building} reflectionDb={reflectionDb} />
      {!pieces && !error && <div className="animate-pulse">computing the pieces…</div>}
      {error && <div className="text-destructive">{error}</div>}
      {pieces && (
        <div className={`${GRID} items-baseline`}>
          <span />
          <HoverText
            className={`col-span-2 text-center ${COLUMN_NAME}`}
            title={'What the ground and the screening by terrain, buildings and walls do\nto the ray from the piece\'s nearest point, in calm air and with the\nsound bent down by the wind or an inversion (CNOSSOS-EU)'}
          >
            ground + screening
          </HoverText>
          <span className="col-span-2" />
          <HoverText
            className={COLUMN_NAME}
            title={'A piece\'s direction and distance. Its dot is its colour on the map:\nred the loudest piece, violet 10 dB below it, blue 20, grey 30 and more'}
          >
            Source, piece
          </HoverText>
          <span className={`text-right ${COLUMN_NAME}`}>calm</span>
          <span className={`text-right ${COLUMN_NAME}`}>bent</span>
          <span className={`text-right ${COLUMN_NAME}`}>Lden</span>
          <span />
          {grouped.map(({ layer, sources }) => (
            <div key={layer.source_type} className="contents">
              <div className="col-span-5 mt-2 pt-1 border-t border-border font-medium text-foreground">
                {SOURCE_LABELS[layer.source_type] ?? layer.source_type}
              </div>
              <span className={`col-span-5 mb-0.5 ${COLUMN_NAME}`}>{layerNote(layer)}</span>
              {sources.map(source => {
                const isOpen = openKeys.has(source.key)
                return (
                  <div key={source.key} className="contents">
                    <button
                      type="button"
                      className="col-span-5 grid grid-cols-subgrid items-baseline py-px text-left hover:bg-muted/40"
                      aria-expanded={isOpen}
                      onClick={() => toggle(source.key)}
                    >
                      <FadingText className="col-span-3 text-foreground">{source.label}</FadingText>
                      <span className="text-right tabular-nums text-foreground">
                        {source.lden.toFixed(1)}
                      </span>
                      <Chevron open={isOpen} />
                    </button>
                    {isOpen && source.pieces.map(listed => (
                      <PieceRow
                        key={listed.index}
                        listed={listed}
                        open={opened === listed.index}
                        onToggle={() => setOpened(opened === listed.index ? null : listed.index)}
                        onHover={on => setHovered(on ? listed.index : null)}
                      />
                    ))}
                  </div>
                )
              })}
            </div>
          ))}
        </div>
      )}
    </div>
  )
}
