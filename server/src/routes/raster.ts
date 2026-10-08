// GET /api/raster/:layer/:z/:x/:y.png: one map tile of a data layer (the terrain's elevation, forest
// and hard ground; the obstacles' buildings and noise barriers; the sources' road traffic and trains),
// drawn by `qm-raster` from the release's default year. A few tiles draw at once behind a short
// queue; a full queue answers 503, a failed drawing 500, and neither is cached.
import { spawn } from 'node:child_process'
import type { FastifyInstance } from 'fastify'

/** Each layer's zooms, as `qm-raster` draws them: the terrain from 10 (16 files a tile), the
 *  obstacles and the roads from 13, the railways from 11; the map enlarges zoom 16 beyond. */
export const RASTER_ZOOMS: Readonly<Record<string, readonly [number, number]>> = {
  elevation: [10, 16],
  forest: [10, 16],
  hard: [10, 16],
  buildings: [13, 16],
  barriers: [13, 16],
  traffic: [13, 16],
  trains: [11, 16],
  others: [12, 16],
}

/** A view asks for about 35 tiles a layer, all five layers 175; one draws in 3-13 ms, so 256
 *  waiting are at most about a quarter of a second behind eight drawing. */
export const RASTER_CONCURRENCY = 8
const QUEUE_LENGTH = 256
const TIMEOUT_MS = 10_000
const STDERR_TAIL_BYTES = 1000

export interface RasterOptions {
  rasterBin: string
  preparedDir: string
  year: string
}

/** The tile's numbers, or `null` for a layer, zoom or position that has no tile. */
export function parseRasterTile(params: { layer: string; z: string; x: string; y: string }): [number, number, number] | null {
  const zooms = Object.hasOwn(RASTER_ZOOMS, params.layer) ? RASTER_ZOOMS[params.layer] : null
  const [z, x, y] = [params.z, params.x, params.y].map(text => (/^[0-9]{1,9}$/.test(text) ? Number(text) : NaN))
  if (!zooms || !(z >= zooms[0] && z <= zooms[1]) || !(x < 2 ** z) || !(y < 2 ** z)) return null
  return [z, x, y]
}

/** Runs `qm-raster` for one tile; aborting `signal` kills it. */
function draw(bin: string, args: string[], signal: AbortSignal): Promise<Buffer> {
  return new Promise((resolve, reject) => {
    const child = spawn(bin, args, { stdio: ['ignore', 'pipe', 'pipe'], signal, killSignal: 'SIGKILL' })
    const chunks: Buffer[] = []
    let stderr = ''
    const timer = setTimeout(() => child.kill('SIGKILL'), TIMEOUT_MS)
    child.stdout.on('data', (chunk: Buffer) => chunks.push(chunk))
    child.stderr.setEncoding('utf8')
    child.stderr.on('data', (chunk: string) => {
      stderr = (stderr + chunk).slice(-STDERR_TAIL_BYTES)
    })
    child.on('error', (error) => {
      clearTimeout(timer)
      reject(error)
    })
    child.on('close', (code, exitSignal) => {
      clearTimeout(timer)
      if (code === 0) resolve(Buffer.concat(chunks))
      else reject(new Error(`qm-raster ${exitSignal ? `signal ${exitSignal}` : `exit code ${code}`}: ${stderr.trim()}`))
    })
  })
}

export async function rasterRoutes(app: FastifyInstance, options: RasterOptions): Promise<void> {
  // A finished tile hands its slot straight to the next one waiting. A map that panned on cancels
  // its tiles: one whose visitor leaves gives up its place in the queue, or has its drawing killed.
  let running = 0
  const queue: ((start: boolean) => void)[] = []
  const acquire = (): { ready: Promise<boolean>; leave: () => void } | null => {
    if (running < RASTER_CONCURRENCY) {
      running += 1
      return { ready: Promise.resolve(true), leave: () => {} }
    }
    if (queue.length >= QUEUE_LENGTH) return null
    let wake: (start: boolean) => void = () => {}
    const ready = new Promise<boolean>(resolve => { wake = resolve })
    queue.push(wake)
    const leave = () => {
      const at = queue.indexOf(wake)
      if (at < 0) return
      queue.splice(at, 1)
      wake(false)
    }
    return { ready, leave }
  }
  const release = () => {
    const next = queue.shift()
    if (next) next(true)
    else running -= 1
  }

  app.get<{ Params: { layer: string; z: string; x: string; y: string } }>(
    '/api/raster/:layer/:z/:x/:y.png',
    async (request, reply) => {
      const tile = parseRasterTile(request.params)
      if (!tile) return reply.code(400).send({ error: 'No such map tile.' })
      const slot = acquire()
      if (!slot) return reply.code(503).header('Retry-After', '1').send({ error: 'The map layers are busy.' })
      const left = new AbortController()
      reply.raw.once('close', () => {
        if (reply.raw.writableFinished) return
        slot.leave()
        left.abort()
      })
      if (!(await slot.ready)) return reply
      const [z, x, y] = tile
      try {
        const png = await draw(options.rasterBin, [
          '--prepared', options.preparedDir, '--year', options.year, '--layer', request.params.layer,
          '--z', String(z), '--x', String(x), '--y', String(y),
        ], left.signal)
        return reply.type('image/png').header('Cache-Control', 'public, max-age=3600').send(png)
      } catch (error) {
        if (left.signal.aborted) return reply
        request.log.error({ err: error, tile: request.params }, 'raster failed')
        return reply.code(500).send({ error: 'The map layer could not be drawn.' })
      } finally {
        release()
      }
    },
  )
}
