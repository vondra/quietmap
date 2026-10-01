// The ground under one ray, from the source (left) to the receiver (right): the terrain line, its
// ground factor along the bottom (soft ground green, hard grey), the buildings (blocks) and walls
// (bars) the ray crosses, and the straight sight line between the source and the receiver.
import type { PieceTrace } from '../../../types/noise'

const WIDTH = 300
const HEIGHT = 90
const PAD = { left: 4, right: 4, top: 6, bottom: 10 }

/** The ground altitude at `distance` along the profile, linear between samples. */
function groundAt(profile: [number, number, number][], distance: number): number {
  for (let k = 1; k < profile.length; k++) {
    const [d0, z0] = profile[k - 1]
    const [d1, z1] = profile[k]
    if (distance <= d1) return d1 > d0 ? z0 + (z1 - z0) * (distance - d0) / (d1 - d0) : z1
  }
  return profile.at(-1)?.[1] ?? 0
}

export function ProfileDiagram({ trace, crossings }: {
  trace: PieceTrace
  /** Distance from the receiver (m), height above the ground (m), footprint id. */
  crossings: [number, number, string][]
}) {
  const profile = trace.profile
  if (profile.length < 2) return null
  const length = profile.at(-1)![0]
  // The two walls of a building share its id: one block between them; a lone crossing is a wall.
  const byFootprint = new Map<string, { from: number, to: number, height: number }>()
  for (const [fromReceiver, height, id] of crossings) {
    const distance = length - fromReceiver
    const block = byFootprint.get(id)
    if (block) {
      block.from = Math.min(block.from, distance)
      block.to = Math.max(block.to, distance)
      block.height = Math.max(block.height, height)
    } else {
      byFootprint.set(id, { from: distance, to: distance, height })
    }
  }
  const tops = [...byFootprint.values()].map(block => ({
    ...block,
    ground: groundAt(profile, (block.from + block.to) / 2),
  }))
  const altitudes = [
    ...profile.map(([, z]) => z),
    trace.source_altitude_m,
    trace.receiver_altitude_m,
    ...tops.map(top => top.ground + top.height),
  ]
  const low = Math.min(...altitudes) - 2
  const high = Math.max(...altitudes) + 2
  const x = (distance: number) => PAD.left + (distance / Math.max(length, 1)) * (WIDTH - PAD.left - PAD.right)
  const y = (altitude: number) => PAD.top + (1 - (altitude - low) / (high - low)) * (HEIGHT - PAD.top - PAD.bottom)
  const ground = profile.map(([d, z]) => `${x(d).toFixed(1)},${y(z).toFixed(1)}`).join(' ')
  const area = `${x(0).toFixed(1)},${(HEIGHT - PAD.bottom).toFixed(1)} ${ground} ${x(length).toFixed(1)},${(HEIGHT - PAD.bottom).toFixed(1)}`
  return (
    <svg
      viewBox={`0 0 ${WIDTH} ${HEIGHT}`}
      className="w-full h-auto my-1"
      role="img"
      aria-label={`Ground under the ray: ${Math.round(length)} m, altitudes ${Math.round(low + 2)}–${Math.round(high - 2)} m`}
    >
      <polygon points={area} className="fill-muted" />
      <polyline points={ground} className="fill-none stroke-muted-foreground" strokeWidth={1} />
      {profile.slice(1).map(([d1, , g], k) => (
        <rect
          key={k}
          x={x(profile[k][0])}
          y={HEIGHT - PAD.bottom + 2}
          width={Math.max(x(d1) - x(profile[k][0]), 0.5)}
          height={4}
          className={g >= 0.5 ? 'fill-green-600/60' : 'fill-zinc-400/70'}
        />
      ))}
      {tops.map((top, k) => (
        <rect
          key={k}
          x={x(top.from) - 1}
          y={y(top.ground + top.height)}
          width={Math.max(x(top.to) - x(top.from), 0) + 2}
          height={Math.max(y(top.ground) - y(top.ground + top.height), 0.5)}
          className="fill-foreground/50"
        />
      ))}
      <line
        x1={x(0)} y1={y(trace.source_altitude_m)} x2={x(length)} y2={y(trace.receiver_altitude_m)}
        className="stroke-sky-600" strokeWidth={0.8} strokeDasharray="3 2"
      />
      <circle cx={x(0)} cy={y(trace.source_altitude_m)} r={2.2} className="fill-red-600" />
      <circle cx={x(length)} cy={y(trace.receiver_altitude_m)} r={2.2} className="fill-sky-700" />
    </svg>
  )
}
