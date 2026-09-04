/**
 * The OSM extract tree's readiness rule and the ONE-root invariant.
 * Run: `npx tsx --test pipeline/lib/data-year.test.ts`
 *
 * Two bug classes, both silent-degradation:
 *   1. A missing or bare OSM extract tree read as "this world has no buildings":
 *      every reader selects cells by the presence of buildings.arrow, so an
 *      unmounted disk makes the buildings enrichers report 0 hexes and the
 *      structure builder write Overture-only tables over the emission stock.
 *   2. Two roots: the extract writing where the chain does not read. The shell
 *      driver and this module must resolve the SAME <repo>/data, and neither may
 *      take a root override — the TypeScript half never had one, so a shell knob
 *      could only pair a fresh extract with somebody else's cells.
 */

import { test } from 'node:test'
import assert from 'node:assert/strict'
import { mkdirSync, mkdtempSync, readFileSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'
import { DATA_YEAR, H3R4_DIR, OSM_EXTRACT_DIR, requireOsmExtractTree } from './data-year.ts'

const REPO_ROOT = resolve(import.meta.dirname, '..', '..')

test('an OSM extract tree that is absent, bare, or holds no R4 cell is refused', () => {
  const root = mkdtempSync(join(tmpdir(), 'osm-extract-tree-'))
  try {
    assert.throws(() => requireOsmExtractTree(join(root, 'never-created')), /missing or holds no R4 cell/)

    const bare = join(root, 'bare')
    mkdirSync(bare)
    assert.throws(() => requireOsmExtractTree(bare), /missing or holds no R4 cell/)

    // A directory of things that are not cells must not pass for a world.
    mkdirSync(join(bare, 'lost+found'))
    mkdirSync(join(bare, '841e309fffffffff')) // 16 chars — not an R4 id
    assert.throws(() => requireOsmExtractTree(bare), /missing or holds no R4 cell/)

    mkdirSync(join(bare, '841e309ffffffff')) // Dobříš
    requireOsmExtractTree(bare) // one real cell is enough
  } finally {
    rmSync(root, { recursive: true, force: true })
  }
})

test('both trees hang from the one repository data root, in shell and in TypeScript', () => {
  assert.equal(H3R4_DIR, resolve(REPO_ROOT, 'data', 'prepared', DATA_YEAR, 'h3r4'))
  assert.equal(OSM_EXTRACT_DIR, resolve(REPO_ROOT, 'data', 'source', 'osm-extract', DATA_YEAR, 'h3r4'))

  // The extract driver derives both outputs from one DATA_DIR anchored on the
  // same <repo>/data, and offers no override for either. A root knob on the
  // shell side alone is the two-roots bug.
  const driver = readFileSync(resolve(REPO_ROOT, 'scripts', 'osm-to-h3r4.sh'), 'utf-8')
  assert.match(driver, /^DATA_DIR="\$PROJECT_DIR\/data"$/m)
  assert.match(driver, /^OUTPUT_DIR="\$DATA_DIR\/prepared\/\$\{YEAR\}\/h3r4"/m)
  assert.match(driver, /^OSM_EXTRACT_DIR="\$DATA_DIR\/source\/osm-extract\/\$\{YEAR\}\/h3r4"/m)
  assert.doesNotMatch(driver, /\$\{(DATA_ROOT|DATA_DIR|OUTPUT_DIR|OSM_EXTRACT_DIR):-/)
})
