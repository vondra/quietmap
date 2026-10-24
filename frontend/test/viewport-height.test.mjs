// Nothing is sized by the large viewport: on a phone vh reaches under the browser's toolbar, where
// the fixed controls do not reach, so a box of it put the map's corner controls below their row.
import assert from 'node:assert/strict'
import { globSync, readFileSync } from 'node:fs'
import { join } from 'node:path'
import test from 'node:test'

const SRC = new URL('../src', import.meta.url).pathname

test('a height of the viewport is its dynamic height (dvh), never vh or h-screen', () => {
  const found = globSync('**/*.{ts,tsx,css}', { cwd: SRC }).flatMap(file => {
    const code = readFileSync(join(SRC, file), 'utf8').replace(/\/\*[\s\S]*?\*\/|(?:^|\s)\/\/.*$/gm, '')
    return [...code.matchAll(/\b\d+(?:\.\d+)?l?vh\b|\b(?:min-|max-)?h-(?:screen|lvh)\b/g)].map(match => `${file}: ${match[0]}`)
  })
  assert.deepEqual(found, [])
})
