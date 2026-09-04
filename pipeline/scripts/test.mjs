/** Deterministic offline discovery for the currently ported pipeline units. */

import { readdirSync } from 'node:fs'
import { spawnSync } from 'node:child_process'
import { dirname, relative, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const tests = [root, resolve(root, 'lib')]
  .flatMap(directory => readdirSync(directory)
    .filter(name => name.endsWith('.test.ts'))
    .map(name => relative(root, resolve(directory, name))))
  .sort()

if (tests.length === 0) throw new Error('no pipeline tests discovered')
console.log(`pipeline tests: ${tests.length} files (offline)`)
const result = spawnSync(process.execPath, ['--import', 'tsx', '--test', ...tests], {
  cwd: root,
  stdio: 'inherit',
})
process.exit(result.status ?? 1)
