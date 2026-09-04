/**
 * Tests for the write-side helpers in provenance.ts.
 *
 * Registry integrity and shouldOverwrite semantics are tested in
 * sources.test.ts; this file covers only `updateRow` and `withArrowWrite`.
 *
 * Run: `npx tsx --test pipeline/lib/provenance.test.ts`
 */

import { test } from 'node:test'
import assert from 'node:assert/strict'
import { promises as fs } from 'node:fs'
import os from 'node:os'
import path from 'node:path'
import {
  Field,
  Int32,
  RecordBatch,
  Schema,
  Table,
  Uint16,
  makeTable,
  makeVector,
  tableFromIPC,
  tableToIPC,
  vectorFromArray,
} from 'apache-arrow'
import { updateRow, withArrowWrite } from './provenance.js'

// ─── updateRow ──────────────────────────────────────────────────────────────

test('updateRow: calls writePayload + returns true when self wins', () => {
  let wrote = false
  const r = updateRow(0, 10, () => {
    wrote = true
  })
  assert.strictEqual(r, true)
  assert.strictEqual(wrote, true)
})

test('updateRow: skips writePayload + returns false when self loses', () => {
  let wrote = false
  const r = updateRow(20, 10, () => {
    wrote = true
  })
  assert.strictEqual(r, false)
  assert.strictEqual(wrote, false)
})

// ─── withArrowWrite ─────────────────────────────────────────────────────────

test('withArrowWrite round-trips a table and atomically replaces it', async () => {
  const tmpDir = await fs.mkdtemp(path.join(os.tmpdir(), 'arrow-prov-'))
  const arrowPath = path.join(tmpDir, 'test.arrow')
  try {
    const initial = makeTable({
      value: new Int32Array([1, 2, 3, 4, 5]),
      source_id: new Uint16Array([0, 0, 0, 0, 0]),
    })
    await fs.writeFile(arrowPath, Buffer.from(tableToIPC(initial, 'file')))

    await withArrowWrite(arrowPath, t => {
      const values = t.getChild('value')!.toArray() as Int32Array
      return makeTable({
        value: new Int32Array(values),
        source_id: new Uint16Array([10, 10, 10, 10, 10]),
      })
    })

    const reread = tableFromIPC(await fs.readFile(arrowPath))
    const ids = Array.from(reread.getChild('source_id')!.toArray() as Uint16Array)
    assert.deepStrictEqual(ids, [10, 10, 10, 10, 10])
  } finally {
    await fs.rm(tmpDir, { recursive: true })
  }
})

// ─── withArrowWrite shape preservation (popup batch pruning) ────────────────
// A bare-makeTable callback must not destroy
// schema metadata (contract stamps) nor collapse the extractors' record-batch
// boundaries; a stale qm_batch_bboxes must be DELETED when the shape changed.

/** Two-batch fixture (rows 0..5 | 6..9) with contract + qm_batch_bboxes. */
async function writeTwoBatchFixture(arrowPath: string): Promise<void> {
  const schema = new Schema(
    [
      new Field('id', new Int32(), false),
      new Field('source_id', new Uint16(), false),
    ],
    new Map([
      ['test_contract', 'v1'],
      ['qm_batch_bboxes', '[[50,14,50.05,14.1],[50.05,14.1,50.1,14.2]]'],
    ]),
  )
  const bare = makeTable({
    id: vectorFromArray(Array.from({ length: 10 }, (_, i) => i), new Int32()),
    source_id: vectorFromArray(new Array(10).fill(0), new Uint16()),
  } as never) as unknown as Table
  const full = new Table(schema, bare.batches.map(b => new RecordBatch(schema, b.data)))
  const twoBatches = new Table(schema, [
    ...full.slice(0, 6).batches.map(b => new RecordBatch(schema, b.data)),
    ...full.slice(6, 10).batches.map(b => new RecordBatch(schema, b.data)),
  ])
  await fs.writeFile(arrowPath, Buffer.from(tableToIPC(twoBatches, 'file')))
}

test('withArrowWrite: bare-makeTable patch keeps metadata, batches, qm key', async () => {
  const tmpDir = await fs.mkdtemp(path.join(os.tmpdir(), 'arrow-prov-'))
  const arrowPath = path.join(tmpDir, 'patch.arrow')
  try {
    await writeTwoBatchFixture(arrowPath)

    await withArrowWrite(arrowPath, table => {
      const n = table.numRows
      const src = new Uint16Array(n).fill(7)
      // Deliberately the buggy idiom the enrichers use: bare makeTable —
      // drops metadata and collapses chunks; withArrowWrite must restore both.
      return makeTable({
        id: table.getChild('id')!,
        source_id: makeVector(src),
      } as never) as unknown as Table
    })

    const out = tableFromIPC(await fs.readFile(arrowPath))
    assert.strictEqual(out.numRows, 10)
    assert.strictEqual(out.batches.length, 2, 'batch boundaries restored')
    assert.strictEqual(out.batches[0].numRows, 6)
    assert.strictEqual(out.schema.metadata.get('test_contract'), 'v1', 'contract survived')
    assert.strictEqual(
      out.schema.metadata.get('qm_batch_bboxes'),
      '[[50,14,50.05,14.1],[50.05,14.1,50.1,14.2]]',
      'bboxes survived a shape-preserving patch',
    )
    assert.strictEqual(out.getChild('source_id')!.get(3), 7, 'patch applied')
  } finally {
    await fs.rm(tmpDir, { recursive: true })
  }
})

test('withArrowWrite: row-count change deletes stale qm_batch_bboxes, keeps contract', async () => {
  const tmpDir = await fs.mkdtemp(path.join(os.tmpdir(), 'arrow-prov-'))
  const arrowPath = path.join(tmpDir, 'reshape.arrow')
  try {
    await writeTwoBatchFixture(arrowPath)

    await withArrowWrite(arrowPath, table => {
      const keep = table.slice(0, 9) // drops a row → any bbox map is stale
      return makeTable({
        id: keep.getChild('id')!,
        source_id: keep.getChild('source_id')!,
      } as never) as unknown as Table
    })

    const out = tableFromIPC(await fs.readFile(arrowPath))
    assert.strictEqual(out.numRows, 9)
    assert.strictEqual(out.schema.metadata.get('test_contract'), 'v1')
    assert.strictEqual(
      out.schema.metadata.get('qm_batch_bboxes'),
      undefined,
      'stale bboxes deleted',
    )
  } finally {
    await fs.rm(tmpDir, { recursive: true })
  }
})
// ─── stale-lock recovery (#31.5) ─────────────────────────────────────────────


test('withArrowWrite: reaps an EMPTY lockfile only after the creation grace', async () => {
  // The wx empty-inode window: a crashed-mid-create lock has an empty body.
  // Backdate its mtime past the 2 s grace — must be reaped, not block 5 min.
  const dir = await fs.mkdtemp(path.join(os.tmpdir(), 'prov-lock-empty-'))
  const arrowPath = path.join(dir, 'roads.arrow')
  const table = new Table({ v: vectorFromArray([1], new Int32()) })
  await fs.writeFile(arrowPath, Buffer.from(tableToIPC(table, 'file')))
  await fs.writeFile(`${arrowPath}.lock`, '')
  const old = new Date(Date.now() - 60_000)
  await fs.utimes(`${arrowPath}.lock`, old, old)
  await withArrowWrite(arrowPath, t => t)
  assert.equal((await fs.readdir(dir)).includes('roads.arrow.lock'), false)
  await fs.rm(dir, { recursive: true, force: true })
})

test('withArrowWrite: reaps a dead holder\'s lockfile instead of timing out', async () => {
  const dir = await fs.mkdtemp(path.join(os.tmpdir(), 'prov-lock-'))
  const arrowPath = path.join(dir, 'roads.arrow')
  const table = new Table({ v: vectorFromArray([1, 2, 3], new Int32()) })
  await fs.writeFile(arrowPath, Buffer.from(tableToIPC(table, 'file')))
  // A SIGKILLed/OOMed writer leaves its lockfile behind; PID 2^22+1 is above
  // every default pid_max ceiling, so it is provably not a live process.
  await fs.writeFile(`${arrowPath}.lock`, String(2 ** 22 + 1))
  await withArrowWrite(arrowPath, t => t) // must acquire promptly, not block 5 min
  assert.equal((await fs.readdir(dir)).includes('roads.arrow.lock'), false, 'lock released after the reaped acquisition')
  await fs.rm(dir, { recursive: true, force: true })
})

test('withArrowWrite: an unreadable lock object fails instead of spinning until timeout', async () => {
  const dir = await fs.mkdtemp(path.join(os.tmpdir(), 'prov-lock-invalid-'))
  const arrowPath = path.join(dir, 'roads.arrow')
  const table = new Table({ v: vectorFromArray([1], new Int32()) })
  await fs.writeFile(arrowPath, Buffer.from(tableToIPC(table, 'file')))
  await fs.mkdir(`${arrowPath}.lock`)
  await assert.rejects(withArrowWrite(arrowPath, value => value), /EISDIR/)
  await fs.rm(dir, { recursive: true, force: true })
})
