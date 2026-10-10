// The ground under one ray as the computation sampled it, from the source (left) to the receiver
// (right): the terrain line, its ground factor along the bottom (green as soft as G, gray hard), the
// walls it looked at (buildings gray, barriers brown) at their heights, the straight ray of calm air
// and the ray bent down downwind (CNOSSOS-EU favourable propagation: an arc of radius
// max(1000 m, 8 d)), which clears a crest the straight ray hits. The receiver's altitude is the
// piece's own (its trace's): the ray's samples end at the ground under it.
import type { RayProfile } from '../../../types/noise'

const WIDTH = 300
const HEIGHT = 90
const PAD = { left: 4, right: 4, top: 6, bottom: 10 }

/** The ground altitude at `distance` along the samples, linear between them. */
function groundAt(ground: [number, number, number][], distance: number): number {
  for (let k = 1; k < ground.length; k++) {
    const [d0, z0] = ground[k - 1]
    const [d1, z1] = ground[k]
    if (distance <= d1) return d1 > d0 ? z0 + (z1 - z0) * (distance - d0) / (d1 - d0) : z1
  }
  return ground.at(-1)?.[1] ?? 0
}

export function ProfileDiagram({ ray, receiverAltitudeM, slantM }: {
  ray: RayProfile
  receiverAltitudeM: number
  /** The ray's length, which sets the bent ray's radius. */
  slantM: number
}) {
  const { ground, walls } = ray
  if (ground.length < 2) return null
  const length = ground.at(-1)![0]
  const source = ray.source_altitude_m
  const tops = walls.map(([distance, height, building]) => ({ distance, building, foot: groundAt(ground, distance), height }))
  // The bent ray rises above the straight one by x (d - x) / (2 R), R = max(1000, 8 d).
  const radius = Math.max(1000, 8 * slantM)
  const straight = (distance: number) => source + (receiverAltitudeM - source) * distance / Math.max(length, 1)
  const bent = (distance: number) => straight(distance) + distance * (length - distance) / (2 * radius)
  const arc = Array.from({ length: 33 }, (_, k) => (k / 32) * length)
  const altitudes = [...ground.map(([, z]) => z), source, receiverAltitudeM, ...tops.map(top => top.foot + top.height), bent(length / 2)]
  const low = Math.min(...altitudes) - 2
  const high = Math.max(...altitudes) + 2
  const x = (distance: number) => PAD.left + (distance / Math.max(length, 1)) * (WIDTH - PAD.left - PAD.right)
  const y = (altitude: number) => PAD.top + (1 - (altitude - low) / (high - low)) * (HEIGHT - PAD.top - PAD.bottom)
  const line = ground.map(([d, z]) => `${x(d).toFixed(1)},${y(z).toFixed(1)}`).join(' ')
  const area = `${x(0).toFixed(1)},${(HEIGHT - PAD.bottom).toFixed(1)} ${line} ${x(length).toFixed(1)},${(HEIGHT - PAD.bottom).toFixed(1)}`
  return (
    <svg
      viewBox={`0 0 ${WIDTH} ${HEIGHT}`}
      className="w-full h-auto my-1"
      role="img"
      aria-label={`Terrain profile: ${Math.round(length)} m, altitudes ${Math.round(low + 2)}–${Math.round(high - 2)} m, with the calm and the downwind ray`}
    >
      <polygon points={area} className="fill-muted" />
      <polyline points={line} className="fill-none stroke-muted-foreground" strokeWidth={1} />
      <rect x={x(0)} y={HEIGHT - PAD.bottom + 2} width={x(length) - x(0)} height={4} className="fill-zinc-400/70" />
      {ground.slice(1).map(([d1, , g], k) => (
        <rect
          key={k}
          x={x(ground[k][0])}
          y={HEIGHT - PAD.bottom + 2}
          width={Math.max(x(d1) - x(ground[k][0]), 0.5)}
          height={4}
          className="fill-green-600"
          fillOpacity={g}
        />
      ))}
      {tops.map((top, k) => (
        <rect
          key={k}
          x={x(top.distance) - 0.75}
          y={y(top.foot + top.height)}
          width={1.5}
          height={Math.max(y(top.foot) - y(top.foot + top.height), 0.5)}
          className={top.building ? 'fill-zinc-500' : 'fill-orange-800/80'}
        />
      ))}
      <line
        x1={x(0)} y1={y(source)} x2={x(length)} y2={y(receiverAltitudeM)}
        className="stroke-sky-600" strokeWidth={0.8} strokeDasharray="3 2"
      />
      <polyline
        points={arc.map(d => `${x(d).toFixed(1)},${y(bent(d)).toFixed(1)}`).join(' ')}
        className="fill-none stroke-amber-600" strokeWidth={0.8} strokeDasharray="1 2"
      />
      <circle cx={x(0)} cy={y(source)} r={2.2} className="fill-red-600" />
      <circle cx={x(length)} cy={y(receiverAltitudeM)} r={2.2} className="fill-sky-700" />
    </svg>
  )
}
