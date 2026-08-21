#!/usr/bin/env node

import assert from 'node:assert/strict'
import { existsSync, statSync } from 'node:fs'
import { join, resolve } from 'node:path'

const addonArgument = process.argv[2]
if (!addonArgument) {
  console.error('usage: node scripts/check-source-reader-napi.mjs <target/debug directory or addon>')
  process.exit(2)
}

function resolveAddon(argument) {
  const candidate = resolve(argument)
  if (!statSync(candidate).isDirectory()) return candidate
  const filename = process.platform === 'win32'
    ? 'source_reader.dll'
    : process.platform === 'darwin'
      ? 'libsource_reader.dylib'
      : 'libsource_reader.so'
  // cargo test emits the cdylib under deps/, while another local build may
  // also leave the canonical target/debug copy. Exercise the newest output so
  // a stale sibling can never make the contract check accidentally green.
  const outputs = [join(candidate, 'deps', filename), join(candidate, filename)]
    .filter(existsSync)
    .sort((a, b) => statSync(b).mtimeMs - statSync(a).mtimeMs)
  assert.ok(outputs.length > 0, `source-reader addon not found under ${candidate}`)
  return outputs[0]
}

const addonPath = resolveAddon(addonArgument)
const stat = statSync(addonPath)
assert.ok(stat.isFile() && stat.size > 0, `native addon is empty or not a file: ${addonPath}`)

// Load the Cargo cdylib directly. Requiring it would first require a redundant
// copy with a .node suffix; process.dlopen exercises the same Node-API entrypoint.
const nativeModule = { exports: {} }
process.dlopen(nativeModule, addonPath)

const workerExports = [
  'sourceInit',
  'sourceValidateReference',
  'queryBuildingAt',
  'queryNoiseAtPoint',
  'queryNoiseAtPointUnfiltered',
]

for (const exportName of workerExports) {
  assert.equal(
    typeof nativeModule.exports[exportName],
    'function',
    `source-reader worker export ${exportName} is missing or not a function`,
  )
}

console.log(`source-reader Node worker exports OK: ${workerExports.join(', ')}`)
