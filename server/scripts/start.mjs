import { resolve } from 'node:path'
import { pathToFileURL } from 'node:url'
import { resolveNamedRelease } from './release-layout.mjs'

const selectedRelease = process.env.SERVER_DIST ?? 'dist'
if (selectedRelease !== 'dist' && selectedRelease !== 'dist.next') {
  throw new Error(`invalid SERVER_DIST ${JSON.stringify(selectedRelease)}`)
}
const releasePath = resolveNamedRelease(selectedRelease)
// Pin the static root to THIS immutable release: /api/ready must judge the
// release's own bundled frontend, never a mutable checkout fallback that a
// stale frontend/dist could quietly satisfy instead (codex review 2026-07-19).
// An explicitly provided FRONTEND_DIST (tests, smoke) still wins.
process.env.FRONTEND_DIST ??= resolve(releasePath, 'frontend')
await import(pathToFileURL(resolve(releasePath, 'server.js')).href)
