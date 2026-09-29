#!/usr/bin/env node
// A stand-in for `qm-popup` in the route tests: the integer part of --lat picks the behaviour,
// every update echoes the arguments and the process id.
//   1 streams a partial update, then the final one 300 ms later
//   2 streams a partial update, then fails on stderr with exit code 1
//   3 streams a partial update, then hangs until killed
//   4 streams a partial update and exits cleanly without a final one
//   5 writes a line that is no update, then hangs until killed
// With FAKE_POPUP_SPAWN_LOG set, every start appends its --lat to that file.
import { appendFileSync } from 'node:fs'

const argv = process.argv.slice(2)
const value = (key: string) => argv[argv.indexOf(`--${key}`) + 1]
const lat = Number(value('lat'))
const lon = Number(value('lon'))

function update(seq: number, partial: boolean): void {
  const line = { seq, partial, center: [lat, lon], total_lden: 60, year: value('year'), argv, pid: process.pid }
  process.stdout.write(`${JSON.stringify(line)}\n`)
}

const hang = () => setInterval(() => {}, 1000)

if (process.env.FAKE_POPUP_SPAWN_LOG) appendFileSync(process.env.FAKE_POPUP_SPAWN_LOG, `${value('lat')}\n`)

switch (Math.trunc(lat)) {
  case 1:
    update(1, true)
    setTimeout(() => update(2, false), 300)
    break
  case 2:
    update(1, true)
    process.stderr.write(`qm-popup: ${value('prepared')}/${value('year')}/276/173/2209_1391.sources: read failed\n`)
    process.exitCode = 1
    break
  case 3:
    update(1, true)
    hang()
    break
  case 4:
    update(1, true)
    break
  case 5:
    process.stdout.write('not an update\n')
    hang()
    break
}
