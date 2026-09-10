import assert from 'node:assert/strict'
import { EventEmitter } from 'node:events'
import test, { type TestContext } from 'node:test'
import {
  NoiseOnflyRequestError,
  NoiseOnflySupervisor,
  type NoiseOnflyWorker,
} from './noise-onfly-supervisor.js'

function deferred<T>() {
  let resolve!: (value: T | PromiseLike<T>) => void
  let reject!: (reason?: unknown) => void
  const promise = new Promise<T>((innerResolve, innerReject) => {
    resolve = innerResolve
    reject = innerReject
  })
  return { promise, resolve, reject }
}

async function waitFor(predicate: () => boolean, timeoutMs = 250): Promise<void> {
  const startedAt = Date.now()
  while (!predicate()) {
    if (Date.now() - startedAt > timeoutMs) {
      throw new Error('waitFor timeout')
    }
    await new Promise((resolve) => setTimeout(resolve, 5))
  }
}

class FakeWorker extends EventEmitter implements NoiseOnflyWorker {
  readonly postMessages: Array<{ id: number; lat: number; lng: number; op?: string }> = []
  terminateCalls = 0

  postMessage(message: { id: number; lat: number; lng: number; op?: string }): void {
    this.postMessages.push(message)
  }

  terminate(): Promise<number> {
    this.terminateCalls += 1
    return Promise.resolve(0)
  }

  replyAt(index: number, resultJson = '{}'): void {
    const message = this.postMessages[index]
    assert.ok(message, `missing postMessage at index ${index}`)
    this.emit('message', {
      id: message.id,
      ok: true,
      resultJson,
    })
  }
}

test('queue cap rejects instead of spawning extra work', async (t) => {
  const workers: FakeWorker[] = []
  const supervisor = new NoiseOnflySupervisor({
    createWorker: () => {
      const worker = new FakeWorker()
      workers.push(worker)
      return worker
    },
    maxQueue: 1,
    queueTimeoutMs: 1000,
    workTimeoutMs: 1000,
  })
  t.after(async () => {
    await supervisor.close()
  })

  const first = supervisor.queryNoiseAtPoint(50.1, 14.4)
  await waitFor(() => workers.length === 1 && workers[0].postMessages.length === 1)

  const second = supervisor.queryNoiseAtPoint(50.2, 14.5)
  await assert.rejects(
    supervisor.queryNoiseAtPoint(50.3, 14.6),
    (error: unknown) =>
      error instanceof NoiseOnflyRequestError &&
      error.statusCode === 503 &&
      error.code === 'NOISE_ONFLY_BUSY',
  )

  workers[0].replyAt(0, '{"first":true}')
  assert.equal(await first, '{"first":true}')

  await waitFor(() => workers[0].postMessages.length === 2)
  workers[0].replyAt(1, '{"second":true}')
  assert.equal(await second, '{"second":true}')
  assert.equal(workers.length, 1)
})

test('aborting a queued request frees its slot', async (t) => {
  const workers: FakeWorker[] = []
  const supervisor = new NoiseOnflySupervisor({
    createWorker: () => {
      const worker = new FakeWorker()
      workers.push(worker)
      return worker
    },
    maxQueue: 1,
    queueTimeoutMs: 1000,
    workTimeoutMs: 1000,
  })
  t.after(async () => {
    await supervisor.close()
  })

  const first = supervisor.queryNoiseAtPoint(50.1, 14.4)
  await waitFor(() => workers.length === 1 && workers[0].postMessages.length === 1)

  const queuedAbort = new AbortController()
  const second = supervisor.queryNoiseAtPoint(50.2, 14.5, queuedAbort.signal)
  queuedAbort.abort()
  await assert.rejects(second, (error: unknown) => error instanceof Error && error.name === 'AbortError')

  const third = supervisor.queryNoiseAtPoint(50.3, 14.6)

  workers[0].replyAt(0, '{"first":true}')
  assert.equal(await first, '{"first":true}')

  await waitFor(() => workers[0].postMessages.length === 2)
  workers[0].replyAt(1, '{"third":true}')
  assert.equal(await third, '{"third":true}')
})

const FULL = '{"total_lden":70.3,"segments":[{"a":1}],"segments_meta":{"n":1},"compute_time_ms":"__QM_COMPUTE_TIME_MS__"}'

function timeoutSupervisor(t: TestContext) {
  const workers: FakeWorker[] = []
  const supervisor = new NoiseOnflySupervisor({
    createWorker: () => {
      const worker = new FakeWorker()
      workers.push(worker)
      return worker
    },
    maxQueue: 4,
    queueTimeoutMs: 1000,
    workTimeoutMs: 25,
  })
  t.after(async () => supervisor.close())
  return { workers, supervisor }
}

test('a work timeout answers 504, never terminates the worker, and reuses it after the late reply', async (t) => {
  const { workers, supervisor } = timeoutSupervisor(t)
  const first = supervisor.queryNoiseAtPoint(50.1, 14.4)
  await waitFor(() => workers.length === 1 && workers[0].postMessages.length === 1)
  const second = supervisor.queryNoiseAtPoint(50.2, 14.5)

  await assert.rejects(
    first,
    (error: unknown) =>
      error instanceof NoiseOnflyRequestError &&
      error.statusCode === 504 &&
      error.code === 'NOISE_ONFLY_TIMEOUT',
  )

  // The slot stays busy while the native call runs: nothing else is dispatched
  // and no worker is spawned or terminated (terminating mid-call dlclose'd the
  // addon under live Rust threads — the SIGSEGV that took prod down twice).
  await new Promise((resolve) => setTimeout(resolve, 40))
  assert.equal(workers.length, 1)
  assert.equal(workers[0].postMessages.length, 1)
  assert.equal(workers[0].terminateCalls, 0)

  // The late reply frees the slot, still fills the point cache, and the next
  // request runs on the same worker.
  workers[0].replyAt(0, FULL)
  await waitFor(() => workers[0].postMessages.length === 2)
  assert.equal(supervisor.cachedFull(50.1, 14.4), FULL)
  workers[0].replyAt(1, '{"second":true}')
  assert.equal(await second, '{"second":true}')
  assert.equal(workers.length, 1)
  assert.equal(workers[0].terminateCalls, 0)
})

test('aborting an active request keeps its slot busy until the late reply', async (t) => {
  const { workers, supervisor } = timeoutSupervisor(t)
  const abort = new AbortController()
  const first = supervisor.queryNoiseAtPoint(50.1, 14.4, abort.signal)
  await waitFor(() => workers.length === 1 && workers[0].postMessages.length === 1)
  const second = supervisor.queryNoiseAtPoint(50.2, 14.5)

  abort.abort()
  await assert.rejects(first, (error: unknown) => error instanceof Error && error.name === 'AbortError')

  // No second dispatch onto the worker still inside the native call, and no
  // work-timeout firing on the already settled request.
  await new Promise((resolve) => setTimeout(resolve, 40))
  assert.equal(workers[0].postMessages.length, 1)
  assert.equal(workers[0].terminateCalls, 0)

  workers[0].replyAt(0, '{"late":true}')
  await waitFor(() => workers[0].postMessages.length === 2)
  workers[0].replyAt(1, '{"second":true}')
  assert.equal(await second, '{"second":true}')
})

test('a worker that dies while parked after a 504 frees its slot and is replaced', async (t) => {
  const { workers, supervisor } = timeoutSupervisor(t)
  const first = supervisor.queryNoiseAtPoint(50.1, 14.4)
  await waitFor(() => workers.length === 1 && workers[0].postMessages.length === 1)
  const second = supervisor.queryNoiseAtPoint(50.2, 14.5)
  await assert.rejects(first, (error: unknown) => error instanceof NoiseOnflyRequestError && error.statusCode === 504)

  workers[0].emit('exit', 1)
  await waitFor(() => workers.length === 2 && workers[1].postMessages.length === 1)
  assert.equal(workers[0].terminateCalls, 0)
  workers[1].replyAt(0, '{"second":true}')
  assert.equal(await second, '{"second":true}')
})

test('readiness uses a real pool worker without querying a point', async (t) => {
  const workers: FakeWorker[] = []
  const supervisor = new NoiseOnflySupervisor({
    createWorker: () => {
      const worker = new FakeWorker()
      workers.push(worker)
      return worker
    },
    maxQueue: 1,
    queueTimeoutMs: 1000,
    workTimeoutMs: 1000,
  })
  t.after(async () => supervisor.close())

  const ready = supervisor.checkReady()
  await waitFor(() => workers.length === 1 && workers[0].postMessages.length === 1)
  assert.equal(workers[0].postMessages[0].op, 'ready')
  assert.equal(workers[0].postMessages[0].lat, 0)
  assert.equal(workers[0].postMessages[0].lng, 0)
  workers[0].replyAt(0, '{"ready":true}')
  await ready
})

test('building lookup dispatches containment-only worker operation', async (t) => {
  const workers: FakeWorker[] = []
  const supervisor = new NoiseOnflySupervisor({
    createWorker: () => {
      const worker = new FakeWorker()
      workers.push(worker)
      return worker
    },
    maxQueue: 1,
    queueTimeoutMs: 1000,
    workTimeoutMs: 1000,
  })
  t.after(async () => supervisor.close())

  const lookup = supervisor.queryBuildingAt(49.7910, 14.1963)
  await waitFor(() => workers.length === 1 && workers[0].postMessages.length === 1)
  assert.equal(workers[0].postMessages[0].op, 'building-at')
  workers[0].replyAt(0, '{"height_m":3,"building_type":"building"}')
  assert.equal(await lookup, '{"height_m":3,"building_type":"building"}')
})

function pointSupervisor(t: TestContext) {
  const workers: FakeWorker[] = []
  const supervisor = new NoiseOnflySupervisor({
    createWorker: () => {
      const worker = new FakeWorker()
      workers.push(worker)
      return worker
    },
    maxQueue: 8,
    queueTimeoutMs: 1000,
    workTimeoutMs: 1000,
  })
  t.after(async () => supervisor.close())
  return { workers, supervisor }
}

test('second identical point query is a cache hit (no second dispatch)', async (t) => {
  const { workers, supervisor } = pointSupervisor(t)
  const first = supervisor.queryNoiseAtPoint(50.0, 14.0)
  await waitFor(() => workers.length === 1 && workers[0].postMessages.length === 1)
  workers[0].replyAt(0, FULL)
  assert.equal(await first, FULL)
  assert.equal(await supervisor.queryNoiseAtPoint(50.0, 14.0), FULL)
  assert.equal(workers[0].postMessages.length, 1)
})

test('summary view drops segments but keeps everything else', async (t) => {
  const { workers, supervisor } = pointSupervisor(t)
  const first = supervisor.queryNoiseAtPoint(50.0, 14.0)
  await waitFor(() => workers.length === 1 && workers[0].postMessages.length === 1)
  workers[0].replyAt(0, FULL)
  await first
  const summary = supervisor.cachedSummary(50.0, 14.0)
  assert.ok(summary)
  assert.ok(!summary.includes('"segments"'))
  const parsed = JSON.parse(summary)
  assert.equal(parsed.total_lden, 70.3)
  assert.deepEqual(parsed.segments_meta, { n: 1 })
  assert.equal(supervisor.cachedFull(50.0, 14.0), FULL)
})

test('concurrent identical queries share one worker dispatch', async (t) => {
  const { workers, supervisor } = pointSupervisor(t)
  const a = supervisor.queryNoiseAtPoint(50.0, 14.0)
  const b = supervisor.queryNoiseAtPoint(50.0, 14.0)
  await waitFor(() => workers.length === 1 && workers[0].postMessages.length === 1)
  workers[0].replyAt(0, FULL)
  assert.equal(await a, FULL)
  assert.equal(await b, FULL)
  assert.equal(workers[0].postMessages.length, 1)
})

test('one waiter aborting does not fail the other waiter on the same point', async (t) => {
  const { workers, supervisor } = pointSupervisor(t)
  const controllerA = new AbortController()
  const a = supervisor.queryNoiseAtPoint(50.0, 14.0, controllerA.signal)
  const b = supervisor.queryNoiseAtPoint(50.0, 14.0)
  await waitFor(() => workers.length === 1 && workers[0].postMessages.length === 1)
  controllerA.abort()
  await assert.rejects(a, /aborted/)
  workers[0].replyAt(0, FULL)
  assert.equal(await b, FULL)
  assert.equal(workers[0].postMessages.length, 1)
})

test('answer without a segment list is cached and served as its own summary', async (t) => {
  const { workers, supervisor } = pointSupervisor(t)
  const empty = '{"total_lden":22.1,"sources":[],"compute_time_ms":"__QM_COMPUTE_TIME_MS__"}'
  const first = supervisor.queryNoiseAtPoint(51.0, 15.0)
  await waitFor(() => workers.length === 1 && workers[0].postMessages.length === 1)
  workers[0].replyAt(0, empty)
  assert.equal(await first, empty)
  // Served from cache (no second dispatch), summary === full here.
  assert.equal(supervisor.cachedSummary(51.0, 15.0), empty)
  assert.equal(await supervisor.queryNoiseAtPoint(51.0, 15.0), empty)
  assert.equal(workers[0].postMessages.length, 1)
})

test('already-aborted request fails even on a cached point', async (t) => {
  const { workers, supervisor } = pointSupervisor(t)
  const first = supervisor.queryNoiseAtPoint(50.0, 14.0)
  await waitFor(() => workers.length === 1 && workers[0].postMessages.length === 1)
  workers[0].replyAt(0, FULL)
  await first
  const controller = new AbortController()
  controller.abort()
  await assert.rejects(supervisor.queryNoiseAtPoint(50.0, 14.0, controller.signal), /aborted/)
})

test('aborted client still populates the cache for the next query', async (t) => {
  const { workers, supervisor } = pointSupervisor(t)
  const controller = new AbortController()
  const first = supervisor.queryNoiseAtPoint(50.0, 14.0, controller.signal)
  await waitFor(() => workers.length === 1 && workers[0].postMessages.length === 1)
  controller.abort()
  await assert.rejects(first, /aborted/)
  workers[0].replyAt(0, FULL)
  await waitFor(() => supervisor.cachedSummary(50.0, 14.0) !== null)
  assert.equal(await supervisor.queryNoiseAtPoint(50.0, 14.0), FULL)
  assert.equal(workers[0].postMessages.length, 1)
})


test('slow preview initialization outlives expired requests without worker churn, then serves normally', async (t) => {
  const initialized = deferred<void>()
  const worker = Object.assign(new FakeWorker(), { ready: initialized.promise })
  let spawned = 0
  const supervisor = new NoiseOnflySupervisor({
    createWorker: () => { spawned++; return worker },
    maxQueue: 2, queueTimeoutMs: 30, workTimeoutMs: 10,
  })
  t.after(() => supervisor.close())
  supervisor.warmWorkers()
  assert.equal(spawned, 1)
  await assert.rejects(supervisor.querySurfaceCornerPreview(50, 14),
    (error: unknown) => error instanceof NoiseOnflyRequestError && error.code === 'NOISE_ONFLY_QUEUE_TIMEOUT')
  assert.equal(worker.postMessages.length, 0)
  assert.equal(spawned, 1)
  initialized.resolve()
  const query = supervisor.querySurfaceCornerPreview(50, 14)
  await waitFor(() => worker.postMessages.length === 1)
  worker.replyAt(0, 'null')
  assert.equal(await query, 'null')
  assert.equal(spawned, 1)
})
