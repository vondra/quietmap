import assert from 'node:assert/strict'
import { EventEmitter } from 'node:events'
import test from 'node:test'
import {
  NoiseOnflyRequestError,
  NoiseOnflySupervisor,
  type NoiseOnflyWorker,
} from './noise-onfly-supervisor.js'

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

  const first = supervisor.queryNoise(50.1, 14.4, 'summary')
  await waitFor(() => workers.length === 1 && workers[0].postMessages.length === 1)

  const second = supervisor.queryNoise(50.2, 14.5, 'summary')
  await assert.rejects(
    supervisor.queryNoise(50.3, 14.6, 'summary'),
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

  const first = supervisor.queryNoise(50.1, 14.4, 'summary')
  await waitFor(() => workers.length === 1 && workers[0].postMessages.length === 1)

  const queuedAbort = new AbortController()
  const second = supervisor.queryNoise(50.2, 14.5, 'summary', queuedAbort.signal)
  queuedAbort.abort()
  await assert.rejects(second, (error: unknown) => error instanceof Error && error.name === 'AbortError')

  const third = supervisor.queryNoise(50.3, 14.6, 'summary')

  workers[0].replyAt(0, '{"first":true}')
  assert.equal(await first, '{"first":true}')

  await waitFor(() => workers[0].postMessages.length === 2)
  workers[0].replyAt(1, '{"third":true}')
  assert.equal(await third, '{"third":true}')
})

test('a work timeout answers 504, never terminates the worker, and reuses it after the late reply', async (t) => {
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
  t.after(async () => {
    await supervisor.close()
  })

  const first = supervisor.queryNoise(50.1, 14.4, 'summary')
  await waitFor(() => workers.length === 1 && workers[0].postMessages.length === 1)

  const second = supervisor.queryNoise(50.2, 14.5, 'summary')

  await assert.rejects(
    first,
    (error: unknown) =>
      error instanceof NoiseOnflyRequestError &&
      error.statusCode === 504 &&
      error.code === 'NOISE_ONFLY_TIMEOUT',
  )

  // The slot stays busy while the native call runs: nothing else is dispatched
  // and no worker is spawned or terminated.
  await new Promise((resolve) => setTimeout(resolve, 40))
  assert.equal(workers.length, 1)
  assert.equal(workers[0].postMessages.length, 1)
  assert.equal(workers[0].terminateCalls, 0)

  // The late reply frees the slot; the next request runs on the same worker.
  workers[0].replyAt(0, '{"late":true}')
  await waitFor(() => workers[0].postMessages.length === 2)
  workers[0].replyAt(1, '{"second":true}')
  assert.equal(await second, '{"second":true}')
  assert.equal(workers.length, 1)
  assert.equal(workers[0].terminateCalls, 0)
})

test('aborting an active request keeps its slot busy until the late reply', async (t) => {
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
  t.after(async () => {
    await supervisor.close()
  })

  const abort = new AbortController()
  const first = supervisor.queryNoise(50.1, 14.4, 'summary', abort.signal)
  await waitFor(() => workers.length === 1 && workers[0].postMessages.length === 1)
  const second = supervisor.queryNoise(50.2, 14.5, 'summary')

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
  t.after(async () => {
    await supervisor.close()
  })

  const first = supervisor.queryNoise(50.1, 14.4, 'summary')
  await waitFor(() => workers.length === 1 && workers[0].postMessages.length === 1)
  const second = supervisor.queryNoise(50.2, 14.5, 'summary')
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
