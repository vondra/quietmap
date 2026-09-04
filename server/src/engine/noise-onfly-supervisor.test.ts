import assert from 'node:assert/strict'
import { EventEmitter } from 'node:events'
import test from 'node:test'
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
  private terminatePromise: Promise<number> = Promise.resolve(0)
  private terminateResolve: ((value: number) => void) | null = null

  postMessage(message: { id: number; lat: number; lng: number; op?: string }): void {
    this.postMessages.push(message)
  }

  terminate(): Promise<number> {
    return this.terminatePromise
  }

  holdTerminate(): void {
    const pending = deferred<number>()
    this.terminatePromise = pending.promise
    this.terminateResolve = pending.resolve
  }

  releaseTerminate(code = 0): void {
    this.terminateResolve?.(code)
    this.terminateResolve = null
    this.terminatePromise = Promise.resolve(code)
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

test('worker timeout waits for terminate before dispatching the next request', async (t) => {
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
    for (const worker of workers) {
      worker.releaseTerminate()
    }
    await supervisor.close()
  })

  const first = supervisor.queryNoiseAtPoint(50.1, 14.4)
  await waitFor(() => workers.length === 1 && workers[0].postMessages.length === 1)
  workers[0].holdTerminate()

  const second = supervisor.queryNoiseAtPoint(50.2, 14.5)

  await assert.rejects(
    first,
    (error: unknown) =>
      error instanceof NoiseOnflyRequestError &&
      error.statusCode === 504 &&
      error.code === 'NOISE_ONFLY_TIMEOUT',
  )

  await new Promise((resolve) => setTimeout(resolve, 40))
  assert.equal(workers.length, 1)
  assert.equal(workers[0].postMessages.length, 1)

  workers[0].releaseTerminate(1)
  await waitFor(() => workers.length === 2 && workers[1].postMessages.length === 1)

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
