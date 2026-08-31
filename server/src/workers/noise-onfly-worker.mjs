// Noise-onfly NAPI worker: loads libsource_reader and answers point queries.
//
// Addon-copy invariant (2026-07-10): the parent prepares one stable shared
// path before it creates any pool Worker. Never use per-thread paths: every
// DISTINCT PATH dlopen'd into the server process consumes glibc's fixed
// static-TLS surplus and worker terminate does NOT give it back —
// per-threadId copies made every recycle (request timeout, worker error)
// exhausted it after a few dozen recycles, after which EVERY worker spawn
// failed with "cannot allocate memory in static TLS block" and the popup
// 503'd until process restart (hit live in production 2026-07-09). With a stable
// path, glibc keys the already-loaded object by name and returns the cached
// handle — no TLS growth, and also NO new code until process restart (the
// name match wins over the fresh inode — verified empirically 2026-07-09:
// unlink+copy then dlopen same path does NOT re-run constructors, 60×; the
// distinct-path variant is what exhausted TLS live in production). The size/mtime
// parent-side size/mtime check keeps that copy current for the NEXT server
// process without racing pool startup.

import { parentPort, workerData } from 'node:worker_threads'
import { existsSync, statSync } from 'node:fs'
import { createRequire } from 'node:module'

const req = createRequire(import.meta.url)

const { sourceReaderNodePath, h3r4Dir } = workerData
const readinessReferenceHex = '841e309ffffffff'
// ONE shared copy path for every worker (2026-07-10): glibc name-caches
// dlopen by path, so all pool workers get the SAME library instance — the
// Rust hex cache and sidecar cache are shared across the pool (an area
// loads once, not once per worker) and static TLS is consumed once. The
// name-cache semantics were verified empirically (same-path reload does not
// re-run constructors). Rust statics are lock-protected; source_init is
// idempotent for an unchanged data dir.
if (!existsSync(sourceReaderNodePath)) {
  throw new Error(
    `prepared source-reader addon not found at ${sourceReaderNodePath} — restart the server`,
  )
}
const st = statSync(sourceReaderNodePath)
console.log(
  `noise-onfly-worker: loaded ${sourceReaderNodePath} ` +
  `(mtime=${st.mtime.toISOString()} size=${st.size})`,
)

const sourceModule = req(sourceReaderNodePath)
if (existsSync(h3r4Dir)) {
  const msg = sourceModule.sourceInit(h3r4Dir)
  console.log(`noise-onfly-worker: ${msg}`)
}

parentPort?.on('message', ({ id, lat, lng, lat2, lng2, op }) => {
  try {
    if (op === 'ready') {
      // Re-run the idempotent init so a worker created during a transient data
      // outage cannot later report ready with an unset native source dir.
      sourceModule.sourceInit(h3r4Dir)
      const referenceRows = sourceModule.sourceValidateReference(
        h3r4Dir,
        readinessReferenceHex,
      )
      if (!Number.isInteger(referenceRows) || referenceRows <= 0) {
        throw new Error(`invalid readiness reference row count: ${referenceRows}`)
      }
      parentPort?.postMessage({ id, ok: true, resultJson: '{"ready":true}' })
      return
    }
    if (op === 'footprints') {
      // bbox: lat/lng = south-west, lat2/lng2 = north-east (supervisor contract).
      const resultJson = sourceModule.queryObstacleFootprints(lat, lng, lat2, lng2)
      parentPort?.postMessage({ id, ok: true, resultJson })
      return
    }
    if (op === 'building-at') {
      const resultJson = sourceModule.queryBuildingAt(lat, lng)
      parentPort?.postMessage({ id, ok: true, resultJson })
      return
    }
    const fn = op === 'unfiltered'
      ? sourceModule.queryNoiseAtPointUnfiltered
      : sourceModule.queryNoiseAtPoint
    const resultJson = fn(lat, lng)
    parentPort?.postMessage({ id, ok: true, resultJson })
  } catch (err) {
    parentPort?.postMessage({
      id,
      ok: false,
      error: err instanceof Error ? err.message : String(err),
    })
  }
})
