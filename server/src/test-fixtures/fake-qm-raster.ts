#!/usr/bin/env node
// A stand-in for `qm-raster` in the route tests: --layer forest fails on stderr with exit code 1,
// --layer barriers waits 400 ms first; any other layer prints the PNG signature and the arguments
// at once. With FAKE_RASTER_SPAWN_LOG set, every start appends its --x to that file, and a slow
// tile that was not killed its --x and "drawn".
import { appendFileSync } from 'node:fs'

const argv = process.argv.slice(2)
const value = (key: string) => argv[argv.indexOf(`--${key}`) + 1]
if (process.env.FAKE_RASTER_SPAWN_LOG) appendFileSync(process.env.FAKE_RASTER_SPAWN_LOG, `${value('x')}\n`)
if (value('layer') === 'barriers') {
  await new Promise(resolve => setTimeout(resolve, 400))
  if (process.env.FAKE_RASTER_SPAWN_LOG) appendFileSync(process.env.FAKE_RASTER_SPAWN_LOG, `${value('x')} drawn\n`)
}
if (value('layer') === 'forest') {
  process.stderr.write('/prepared-release/2026 is not a complete release\n')
  process.exit(1)
}
process.stdout.write(Buffer.concat([Buffer.from([0x89, 0x50, 0x4e, 0x47]), Buffer.from(JSON.stringify(argv))]))
