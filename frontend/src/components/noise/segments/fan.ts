// What the map draws of an opened row's segments, in the colours of their lines in the list: each
// listed segment by how far below the loudest it is, the selected one's rays by what each delivers
// against its loudest ray. Pure TypeScript, without React.
import type { PopupPiece, SegmentFan } from '../../../types/noise'

const EARTH_M_PER_DEGREE = 111_320
/** The map first frames the listed segments within this of the loudest: those that make the level
 *  (one 20 dB under the loudest adds a hundredth of its energy). */
const OVERVIEW_BELOW_LOUDEST_DB = 20

/** Red at 0 dB, violet 10 dB below, blue 20, gray 30 and more. */
const COLOR_STOPS: [number, [number, number, number]][] = [
  [-30, [0x94, 0xa3, 0xb8]],
  [-20, [0x25, 0x63, 0xeb]],
  [-10, [0x7c, 0x3a, 0xed]],
  [0, [0xdc, 0x26, 0x26]],
]

/** The colour of a level `belowDb` (0 or less) under the loudest of its kind. */
export function belowColor(belowDb: number): string {
  const db = Math.min(0, Math.max(-30, belowDb))
  const k = Math.max(1, COLOR_STOPS.findIndex(([at]) => db <= at))
  const [[from, a], [to, b]] = [COLOR_STOPS[k - 1], COLOR_STOPS[k]]
  const t = (db - from) / (to - from)
  return `#${[0, 1, 2].map(c => Math.round(a[c] + t * (b[c] - a[c])).toString(16).padStart(2, '0')).join('')}`
}

/** Each segment's colour by its Lden under the loudest listed one. */
export function segmentColors(pieces: PopupPiece[]): string[] {
  const loudest = Math.max(...pieces.map(piece => piece.received.lden ?? -Infinity))
  return pieces.map(piece => belowColor((piece.received.lden ?? -Infinity) - loudest))
}

/** Each ray's colour by the Lden it delivers under the segment's loudest ray; null for a silent
 *  one. */
export function rayColors(piece: PopupPiece): (string | null)[] {
  const loudest = Math.max(...piece.rays.map(([, , , lden]) => lden ?? -Infinity))
  return piece.rays.map(([, , , lden]) => (lden == null ? null : belowColor(lden - loudest)))
}

/** East and north metres from `a` to `b`, both [lat, lon]. */
export function offsetM(a: [number, number], b: [number, number]): [number, number] {
  return [
    (b[1] - a[1]) * EARTH_M_PER_DEGREE * Math.cos((a[0] * Math.PI) / 180),
    (b[0] - a[0]) * EARTH_M_PER_DEGREE,
  ]
}

/** The map's drawing of row `source`'s listed segments at `receiver`, with the rays of the
 *  `selected` one (hovered or opened), its ray `selectedRay` marked, and the `opened` one's ends
 *  and rays to frame. */
export function segmentFan(
  source: string,
  pieces: PopupPiece[],
  receiver: [number, number],
  { selected, selectedRay, opened }: { selected: number | null, selectedRay: number | null, opened: number | null },
): SegmentFan {
  const colors = segmentColors(pieces)
  const loudest = Math.max(...pieces.map(piece => piece.received.lden ?? -Infinity))
  const chosen = selected === null ? undefined : pieces[selected]
  const origins = (piece: PopupPiece) => piece.rays.map(([lat, lon]) => [lat, lon] as [number, number])
  return {
    source,
    receiver,
    pieces: pieces.map((piece, index) => ({ ends: piece.ends, color: colors[index], selected: index === selected })),
    rays: chosen
      ? rayColors(chosen).flatMap((color, k) => (color ? [{ from: origins(chosen)[k], color, selected: k === selectedRay }] : []))
      : [],
    overview: pieces
      .filter(piece => (piece.received.lden ?? -Infinity) >= loudest - OVERVIEW_BELOW_LOUDEST_DB)
      .flatMap(piece => piece.ends),
    opened: opened === null || !pieces[opened]
      ? null
      : { index: opened, points: [...pieces[opened].ends, ...origins(pieces[opened])] },
  }
}
