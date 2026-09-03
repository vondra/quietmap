/**
 * Fail-closed contract of requireAdminIso: country-dependent enrichment must
 * not run against a missing/empty admin tree (it would stamp WORLD defaults
 * planet-wide without an error — /gg Codex CRITICAL, 2026-07), and a record
 * sitting in the wrong cell must never be believed.
 *
 * Run: `npx tsx --test pipeline/lib/admin-iso.test.ts`
 */

import { test } from 'node:test'
import assert from 'node:assert/strict'
import { mkdirSync, mkdtempSync, writeFileSync, rmSync } from 'node:fs'
import { join } from 'node:path'
import { tmpdir } from 'node:os'
import { readAdminIso, requireAdminIso } from './admin-iso.js'

const DOBRIS = '841e309ffffffff'

/** Write one cell's 13-byte record; `storedHex` defaults to the cell itself. */
function writeCell(h3r4Dir: string, cell: string, iso: string, storedHex = cell): void {
  mkdirSync(join(h3r4Dir, cell), { recursive: true })
  const record = Buffer.alloc(13)
  record.writeBigUInt64LE(BigInt('0x' + storedHex), 0)
  record[8] = 1 // continent
  record[9] = iso.charCodeAt(0) || 0
  record[10] = iso.charCodeAt(1) || 0
  writeFileSync(join(h3r4Dir, cell, 'admin.bin'), record)
}

test('readAdminIso stays lenient (diagnostics), requireAdminIso validates hard', () => {
  const root = mkdtempSync(join(tmpdir(), 'admin-iso-'))
  try {
    const missing = join(root, 'nope')
    assert.equal(readAdminIso(missing).size, 0, 'lenient reader: empty map')
    assert.throws(() => requireAdminIso(missing), /h3r4 tree missing/)

    // A tree whose cells carry no country record is as useless as no tree.
    const unresolved = join(root, 'unresolved')
    writeCell(unresolved, DOBRIS, '')
    assert.throws(() => requireAdminIso(unresolved), /no cell under .* country record/)

    // A record truncated by a torn write must not half-load.
    const torn = join(root, 'torn')
    mkdirSync(join(torn, DOBRIS), { recursive: true })
    writeFileSync(join(torn, DOBRIS, 'admin.bin'), Buffer.alloc(9))
    assert.throws(() => requireAdminIso(torn), /holds 9 bytes/)

    // A record copied into another cell is caught by its own hex id.
    const misplaced = join(root, 'misplaced')
    writeCell(misplaced, '841e30bffffffff', 'CZ', DOBRIS)
    assert.throws(() => requireAdminIso(misplaced), /holds cell 841e309ffffffff/)

    // One valid record → strict reader returns it; a cell directory without an
    // admin record is simply not a prepared cell.
    const good = join(root, 'good')
    writeCell(good, DOBRIS, 'CZ')
    mkdirSync(join(good, 'properties'), { recursive: true })
    assert.equal(requireAdminIso(good).get(DOBRIS), 'CZ')
    assert.equal(requireAdminIso(good).size, 1)
  } finally {
    rmSync(root, { recursive: true, force: true })
  }
})
