/**
 * Write-side helpers for dataset provenance on enrichment scripts.
 *
 * All enrichment layers (roads, railways, buildings, industrial) live in
 * per-square Arrow files:
 *  - `shouldOverwrite()` / `updateRow()` gate the whole-row update (payload + id together).
 *  - `withArrowWrite()` wraps read-modify-write with an advisory lockfile
 *    (PID-liveness stale recovery) + tmp + rename.
 *
 * Callers declare their dataset id at the top of the script and pass the helper
 * a callback that writes the value columns only if the helper decided to overwrite.
 */

import { promises as fs } from 'node:fs'
import { Field, RecordBatch, Schema, Table, tableFromIPC, tableToIPC } from 'apache-arrow'
import { shouldOverwrite } from './sources.js'

/** Schema-metadata key of the per-batch bbox list written by the extractors
 *  (engine/arrow-batching). Valid ONLY while batch boundaries and row count
 *  match what the extractor wrote — see preserveArrowShape. */
const QM_BATCH_BBOXES_KEY = 'qm_batch_bboxes'

// Overwrite decision (re-exported from sources.ts for a stable call site)

/**
 * Gate for enricher writes. Returns true if `selfId` should win over the
 * row's current `existingId`. See `./sources.ts::shouldOverwrite` for the
 * full rule list (provenance rank → year → id tiebreaks, with idempotent
 * and empty-slot early returns).
 *
 * Kept here as a re-export so existing callers
 * (`import { shouldOverwrite } from './lib/provenance.js'`) are unchanged.
 */
export { shouldOverwrite }

/**
 * Gates a row update: writes payload + provenance atomically only if self wins.
 * Returns true if the row was updated.
 *
 * Usage:
 *   updateRow(existingIdCol[i], MY_ID, () => {
 *     aadtLight[i] = newLight;
 *     trafficSource[i] = 1;
 *     existingIdCol[i] = MY_ID;
 *   });
 */
export function updateRow(
  existingId: number,
  selfId: number,
  writePayload: () => void,
): boolean {
  if (!shouldOverwrite(existingId, selfId)) return false
  writePayload()
  return true
}

// Concurrent-safe write wrappers

/** Best-effort advisory lock via O_CREAT|O_EXCL on `{path}.lock` (NOT the
 *  flock(2) syscall) with PID-liveness stale-lock recovery: the lockfile body
 *  is the holder's PID; on contention the holder is probed with `kill(pid, 0)`
 *  and a DEAD holder's lock is removed and immediately re-contested. Without
 *  recovery, one SIGKILL/OOM (which skips `finally`) bricks every later writer
 *  on that square for good — and dense-square writers can hold the
 *  lock across whole-compute windows (#31.5). Same-host semantics only: PIDs
 *  mean nothing across machines, and all prepared-data writers run on the build host by
 *  design. The two-reapers window is closed by the `.reap` mutex and the
 *  empty-inode window by a 2 s creation grace (see inline comments); the
 *  residual exposure is PID reuse, which can only make us WAIT, not corrupt. */
async function acquireLock(lockPath: string, timeoutMs = 5 * 60_000): Promise<void> {
  const start = Date.now()
  while (Date.now() - start < timeoutMs) {
    try {
      await fs.writeFile(lockPath, String(process.pid), { flag: 'wx' })
      return
    } catch (err: unknown) {
      if ((err as NodeJS.ErrnoException).code !== 'EEXIST') throw err
      let body: string
      try {
        body = await fs.readFile(lockPath, 'utf-8')
      } catch (readError: unknown) {
        if ((readError as NodeJS.ErrnoException).code === 'ENOENT') continue
        throw readError
      }
      // parseInt also digests the legacy `pid-timestamp` body shape.
      const holderPid = parseInt(body, 10)
      let holderAlive: boolean
      if (!Number.isInteger(holderPid) || holderPid <= 0) {
        // Empty/garbled body: `wx` publishes the inode BEFORE the pid write
        // lands (open→write is two syscalls), so a freshly-created lock reads
        // as '' for a moment — reaping it would delete a LIVE writer's lock
        // (#31 round-2 Codex CRITICAL). Only a garbled lock that has SAT there
        // past the creation grace is genuinely orphaned (writer crashed
        // between the two syscalls).
        const st = await fs.stat(lockPath).catch(() => null)
        holderAlive = st === null || Date.now() - st.mtimeMs < 2000
      } else {
        try {
          process.kill(holderPid, 0)
          holderAlive = true
        } catch (probe: unknown) {
          // ESRCH = no such process (dead). EPERM = alive but not ours — keep waiting.
          holderAlive = (probe as NodeJS.ErrnoException).code !== 'ESRCH'
        }
      }
      if (!holderAlive) {
        // Serialize the reap through a `.reap` O_EXCL mutex: without it, two
        // waiters could both pass the body recheck in the microsecond window
        // between one's unlink and its re-create, and the second unlink would
        // delete a FRESHLY acquired lock → two live writers on one arrow
        // (/gg #31 round 2, Gemini CRITICAL). The .reap file lives microseconds,
        // so its own stale case (reaper SIGKILLed inside) is handled by the
        // same PID probe and a benign unlink race.
        const reapPath = `${lockPath}.reap`
        try {
          await fs.writeFile(reapPath, String(process.pid), { flag: 'wx' })
        } catch {
          // Same empty-inode grace as the main lock: a fresh .reap may read ''.
          const reaperPid = parseInt(await fs.readFile(reapPath, 'utf-8').catch(() => ''), 10)
          let reaperAlive: boolean
          if (!Number.isInteger(reaperPid) || reaperPid <= 0) {
            const st = await fs.stat(reapPath).catch(() => null)
            reaperAlive = st === null || Date.now() - st.mtimeMs < 2000
          } else {
            try {
              process.kill(reaperPid, 0)
              reaperAlive = true
            } catch (probe: unknown) {
              reaperAlive = (probe as NodeJS.ErrnoException).code !== 'ESRCH'
            }
          }
          if (!reaperAlive) await fs.unlink(reapPath).catch(() => {})
          await new Promise(r => setTimeout(r, 50 + Math.random() * 100))
          continue
        }
        try {
          // Holder of .reap: recheck the body, then unlink — no concurrent
          // reaper can interleave between these two lines now.
          const recheck = await fs.readFile(lockPath, 'utf-8').catch(() => null)
          if (recheck === body) await fs.unlink(lockPath).catch(() => {})
        } finally {
          await fs.unlink(reapPath).catch(() => {})
        }
        continue // re-contest via O_EXCL — exactly one waiter wins
      }
      // Lock held by a live process — back off 100–400 ms with jitter.
      await new Promise(r => setTimeout(r, 100 + Math.random() * 300))
    }
  }
  throw new Error(`acquireLock timeout on ${lockPath} after ${timeoutMs} ms (live holder — a stale lock would have been reaped)`)
}

async function releaseLock(lockPath: string): Promise<void> {
  try {
    // Ownership-verified: if a reaper (wrongly or rightly) took our lock and a
    // SUCCESSOR now holds it, a blind unlink would free the successor's lock
    // for a third writer (#31 round-2 Codex). Losing the lock mid-hold is
    // already an anomaly; never compound it.
    const body = await fs.readFile(lockPath, 'utf-8')
    if (body === String(process.pid)) await fs.unlink(lockPath)
  } catch {
    /* best-effort */
  }
}

/**
 * Read Arrow file → mutate via callback → atomic replace.
 * The callback receives the parsed `Table`; returns the updated table to write.
 * **Returning the SAME `Table` reference signals "no change" — the file is left
 * byte-for-byte untouched** (no re-serialize, no rename). This keeps no-op squares
 * bit-identical, which the refactor-verification relies on; callers that genuinely
 * changed a row must return a freshly-built `Table`.
 */
export async function withArrowWrite(
  arrowPath: string,
  fn: (table: Table) => Table | Promise<Table>,
): Promise<void> {
  const lockPath = `${arrowPath}.lock`
  const tmpPath = `${arrowPath}.tmp`
  await acquireLock(lockPath)
  try {
    const bytes = await fs.readFile(arrowPath)
    const input = tableFromIPC(bytes)
    const output = await fn(input)
    if (output === input) return   // no-op: nothing changed → leave the file untouched
    const normalized = preserveArrowShape(input, output)
    await fs.writeFile(tmpPath, Buffer.from(tableToIPC(normalized, 'file')))
    await fs.rename(tmpPath, arrowPath)
  } finally {
    await releaseLock(lockPath)
  }
}

/**
 * Re-impose the INPUT file's schema metadata, per-field flags and — when the
 * row count is unchanged — record-batch boundaries on the callback's output.
 *
 * Callbacks rebuild tables via bare `makeTable()`, which silently drops schema
 * metadata (bricking contract-gated files: `buildings_contract` aborts the
 * heatmap loader) and collapses record batches (invalidating the extractors'
 * `qm_batch_bboxes` popup-pruning metadata). Centralized HERE so no enricher
 * can forget it — the same class of fix `buildings-arrow.ts` carries locally.
 * A callback that deliberately sets metadata still wins (output keys override
 * input keys).
 *
 * `qm_batch_bboxes` is kept ONLY when row count AND final batch count match
 * the input — bboxes describe exact row/batch positions, so any reshape makes
 * them stale, and a stale value must be deleted, never carried (the reader's
 * count-guard would miss a same-count-different-rows lie).
 */
function preserveArrowShape(input: Table, output: Table): Table {
  const metadata = new Map(input.schema.metadata)
  for (const [k, v] of output.schema.metadata) metadata.set(k, v)

  const fields = output.schema.fields.map(f => {
    const orig = input.schema.fields.find(o => o.name === f.name)
    return orig ? new Field(f.name, f.type, orig.nullable, orig.metadata) : f
  })

  // Restore the input's batch boundaries when the callback reshaped them
  // (value-only patches keep row order, so the original chunking is exact).
  // Boundary equality must compare per-batch ROW COUNTS, not just the batch
  // count — same-count-different-boundaries would silently misalign the
  // bbox↔batch mapping (Gemini /gg 2026-07-10).
  const boundariesMatch = (batches: readonly RecordBatch[]): boolean =>
    batches.length === input.batches.length &&
    input.batches.every((b, i) => b.numRows === batches[i].numRows)

  let batches = output.batches
  if (output.numRows === input.numRows && input.batches.length > 1 && !boundariesMatch(batches)) {
    const resliced: RecordBatch[] = []
    let offset = 0
    for (const b of input.batches) {
      const part = output.slice(offset, offset + b.numRows)
      if (part.batches.length !== 1) { resliced.length = 0; break }
      resliced.push(part.batches[0])
      offset += b.numRows
    }
    if (resliced.length === input.batches.length) batches = resliced
  }

  const shapePreserved = output.numRows === input.numRows && boundariesMatch(batches)
  if (!shapePreserved) metadata.delete(QM_BATCH_BBOXES_KEY)

  const schema = new Schema(fields, metadata)
  return new Table(schema, batches.map(b => new RecordBatch(schema, b.data)))
}
