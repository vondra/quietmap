type SupervisorLogLevel = 'info' | 'warn' | 'error'

export type NoiseOnflyWorkerReply = {
  id: number
  ok: boolean
  resultJson?: string
  error?: string
  /** Native call wall time measured inside the worker (excludes postMessage). */
  nativeMs?: number
}

export type NoiseOnflyOp = 'point' | 'unfiltered' | 'ready' | 'footprints' | 'building-at' | 'surface-preview'

export interface NoiseOnflyWorker {
  /** Optional one-time initialization, outside all visitor request deadlines. */
  ready?: Promise<void>
  postMessage(message: { id: number; lat: number; lng: number; lat2?: number; lng2?: number; op?: NoiseOnflyOp }): void
  terminate(): Promise<number>
  on(event: 'message', listener: (message: NoiseOnflyWorkerReply) => void): this
  on(event: 'error', listener: (err: Error) => void): this
  on(event: 'exit', listener: (code: number) => void): this
}

/**
 * Factory receives the pool slot index for callers that keep slot-local
 * resources. Factories that don't care may ignore it.
 */
export type NoiseOnflyWorkerFactory = (slotIndex: number) => NoiseOnflyWorker

type SupervisorLogger = (
  level: SupervisorLogLevel,
  message: string,
  meta?: Record<string, unknown>
) => void

export class NoiseOnflyRequestError extends Error {
  readonly code: string
  readonly statusCode: number

  constructor(message: string, code: string, statusCode: number) {
    super(message)
    this.name = 'NoiseOnflyRequestError'
    this.code = code
    this.statusCode = statusCode
  }
}

export type NoiseOnflyTiming = {
  op: NoiseOnflyOp
  /** Enqueue → dispatch (pool contention). */
  queueMs: number
  /** Dispatch → worker reply (native + postMessage). */
  workMs: number
  /** Native call only, as reported by the worker. */
  nativeMs?: number
  /** resultJson bytes. */
  bytes: number
}

type RequestEntry = {
  id: number
  lat: number
  lng: number
  /** bbox ops ('footprints'): north-east corner; lat/lng carry south-west. */
  lat2?: number
  lng2?: number
  op: NoiseOnflyOp
  enqueuedAt: number
  dispatchedAt: number
  /**
   * Entry-owned abort: fires only when EVERY waiter is gone (or the entry
   * itself times out). Individual client signals are waiter registrations
   * (see addWaiter) — one client's abort must never kill a shared compute
   * other clients still await (gg finding 1).
   */
  ctrl: AbortController
  waiterTokens: Set<symbol>
  /** A signal-less waiter pins the entry: it never auto-aborts. */
  pinned: boolean
  resolve: (resultJson: string) => void
  reject: (err: Error) => void
  queueTimer: NodeJS.Timeout | null
  workTimer: NodeJS.Timeout | null
  worker: NoiseOnflyWorker | null
  signal?: AbortSignal
  abortHandler?: () => void
  clientSettled: boolean
}

/**
 * One slot in the worker pool. Each slot owns at most one live worker and
 * at most one active request at any time. The queue is shared across all
 * slots; dispatching picks the first idle slot.
 */
type Slot = {
  index: number
  worker: NoiseOnflyWorker | null
  active: RequestEntry | null
  initializing: boolean
  recyclingWorker: NoiseOnflyWorker | null
  recycling: Promise<void> | null
}

/**
 * Result cache: one entry holds the full worker string (raw, WITH the
 * `compute_time_ms` sentinel — the route stamps it per serve) plus the
 * once-derived summary string, so a summary hit never re-parses megabytes.
 * Point ops only: bbox ops (`footprints`) need lat2/lng2 in the key and are
 * deliberately uncached. Immutable data per deploy ⇒ restart invalidates.
 */
type CacheEntry = {
  full: string
  summary: string
  bytes: number
}

/** Point-op cache key. `String(f64)` is unique per double except -0/0
 * (the same place — harmless). Exact keys only: a quantized key would
 * serve one point's numbers to another. Bbox ops (`footprints`) are never
 * keyed here — they need lat2/lng2 and stay uncached. */
function pointCacheKey(op: 'point' | 'unfiltered', lat: number, lng: number): string {
  return `${op}|${lat}|${lng}`
}

const RESULT_CACHE_MAX_ENTRIES = 32
const RESULT_CACHE_MAX_BYTES = 150 * 1024 * 1024

export type NoiseOnflySupervisorConfig = {
  createWorker: NoiseOnflyWorkerFactory
  maxQueue: number
  queueTimeoutMs: number
  workTimeoutMs: number
  /**
   * Simultaneous native queries; defaults to one. Workers share native caches,
   * while queries in different areas pin their own decoded data. Size the pool
   * against measured concurrent memory demand and queue latency.
   */
  poolSize?: number
  logger?: SupervisorLogger
  /** Per-request timing tap (phase A0 measurement; cheap, one call per reply). */
  onTiming?: (timing: NoiseOnflyTiming) => void
}

function toError(value: unknown): Error {
  return value instanceof Error ? value : new Error(String(value))
}

function abortError(): Error {
  const error = new Error('noise-onfly request aborted')
  error.name = 'AbortError'
  return error
}

function queueFullError(): NoiseOnflyRequestError {
  return new NoiseOnflyRequestError('noise-onfly busy', 'NOISE_ONFLY_BUSY', 503)
}

function queueTimeoutError(queueTimeoutMs: number): NoiseOnflyRequestError {
  return new NoiseOnflyRequestError(
    `noise-onfly queue timeout after ${queueTimeoutMs} ms`,
    'NOISE_ONFLY_QUEUE_TIMEOUT',
    503,
  )
}

function workTimeoutError(workTimeoutMs: number): NoiseOnflyRequestError {
  return new NoiseOnflyRequestError(
    `noise-onfly timeout after ${workTimeoutMs} ms`,
    'NOISE_ONFLY_TIMEOUT',
    504,
  )
}

function unavailableError(message: string): NoiseOnflyRequestError {
  return new NoiseOnflyRequestError(message, 'NOISE_ONFLY_UNAVAILABLE', 503)
}

export class NoiseOnflySupervisor {
  private readonly createWorker: NoiseOnflyWorkerFactory
  private readonly maxQueue: number
  private readonly queueTimeoutMs: number
  private readonly workTimeoutMs: number
  private readonly logger?: SupervisorLogger
  private readonly onTiming?: (timing: NoiseOnflyTiming) => void
  private readonly slots: Slot[]

  private readonly queue: RequestEntry[] = []
  private nextRequestId = 1
  private closed = false

  private readonly resultCache = new Map<string, CacheEntry>()
  private resultCacheBytes = 0
  private readonly inflight = new Map<string, { promise: Promise<string>; entryId: number }>()

  constructor(config: NoiseOnflySupervisorConfig) {
    this.createWorker = config.createWorker
    this.maxQueue = Math.max(0, config.maxQueue)
    this.queueTimeoutMs = Math.max(1, config.queueTimeoutMs)
    this.workTimeoutMs = Math.max(1, config.workTimeoutMs)
    this.logger = config.logger
    this.onTiming = config.onTiming
    const poolSize = Math.max(1, config.poolSize ?? 1)
    this.slots = Array.from({ length: poolSize }, (_, index) => ({
      index,
      worker: null,
      active: null,
      initializing: false,
      recyclingWorker: null,
      recycling: null,
    }))
  }

  async queryNoiseAtPoint(lat: number, lng: number, signal?: AbortSignal): Promise<string> {
    return this.queryPointCached('point', lat, lng, signal)
  }

  async queryNoiseAtPointUnfiltered(lat: number, lng: number, signal?: AbortSignal): Promise<string> {
    return this.queryPointCached('unfiltered', lat, lng, signal)
  }

  /**
   * Point query with result cache + in-flight dedup. The shared computation
   * carries NO client signal: one client's abort must never fail the other
   * clients waiting on the same point (gg finding 1). Each waiter races the
   * shared compute against its own signal instead. The abort check still
   * runs BEFORE the lookup so an already-aborted request fails exactly
   * like an uncached one.
   */
  private async queryPointCached(
    op: 'point' | 'unfiltered', lat: number, lng: number, signal?: AbortSignal,
  ): Promise<string> {
    if (signal?.aborted) {
      throw abortError()
    }
    const key = pointCacheKey(op, lat, lng)
    const hit = this.resultCache.get(key)
    if (hit) {
      this.resultCache.delete(key)
      this.resultCache.set(key, hit)
      return hit.full
    }
    const pending = this.inflight.get(key)
    if (pending) {
      const entry = this.findEntry(pending.entryId)
      if (entry) {
        this.addWaiter(entry, signal)
        return this.withSignal(pending.promise, signal)
      }
      // The entry settled between lookup and join: its result is cached
      // by now, or the compute failed and a fresh one is due.
      const late = this.resultCache.get(key)
      if (late) {
        this.resultCache.delete(key)
        this.resultCache.set(key, late)
        return late.full
      }
      this.inflight.delete(key)
    }
    const entryRef: { id?: number } = {}
    // The client signal travels into enqueue as the FIRST waiter (registered
    // there); later joiners register via addWaiter above. Never both.
    const run = this.enqueue(lat, lng, op, signal, undefined, undefined, entryRef)
    const entry = entryRef.id === undefined ? undefined : this.findEntry(entryRef.id)
    if (!entry) {
      // Rejected before queuing (closed/queue-full): nothing to share.
      return run
    }
    const tracked = run.finally(() => {
      const current = this.inflight.get(key)
      if (current?.promise === tracked) this.inflight.delete(key)
    })
    this.inflight.set(key, { promise: tracked, entryId: entry.id })
    return this.withSignal(tracked, signal)
  }

  private findEntry(id: number): RequestEntry | undefined {
    return (
      this.queue.find((entry) => entry.id === id)
      ?? this.slots.map((slot) => slot.active).find((active) => active?.id === id)
      ?? undefined
    )
  }

  /**
   * Register one client's interest in a shared entry. A signal-less waiter
   * pins the entry (internal callers never abort); otherwise the waiter
   * leaves when its own signal fires, and the LAST waiter out aborts the
   * entry-owned controller — freeing a queued slot while an active worker
   * keeps computing for the cache.
   */
  private addWaiter(entry: RequestEntry, signal?: AbortSignal): void {
    if (!signal) {
      entry.pinned = true
      return
    }
    const token: symbol = Symbol('waiter')
    entry.waiterTokens.add(token)
    signal.addEventListener(
      'abort',
      () => {
        entry.waiterTokens.delete(token)
        if (!entry.pinned && entry.waiterTokens.size === 0) {
          entry.ctrl.abort()
        }
      },
      { once: true },
    )
  }

  /** Race a shared compute against one client's own abort signal. */
  private async withSignal(shared: Promise<string>, signal?: AbortSignal): Promise<string> {
    if (!signal || signal.aborted) {
      if (signal?.aborted) throw abortError()
      return shared
    }
    let onAbort!: () => void
    try {
      return await Promise.race([
        shared,
        new Promise<never>((_, reject) => {
          onAbort = () => reject(abortError())
          signal.addEventListener('abort', onAbort, { once: true })
        }),
      ])
    } finally {
      signal.removeEventListener('abort', onAbort)
    }
  }

  /**
   * Summary projection: the full answer minus the one `segments` key.
   * Key-deleting (not reshaping), so a Rust field rename cannot silently
   * diverge it. Answers without a segment list (empty areas) project to
   * themselves and ARE cached — an uncacheable valid answer would make
   * the route recompute or fall through to `all` (gg finding 3).
   */
  static deriveSummary(full: string): string | null {
    let parsed: { segments?: unknown }
    try {
      parsed = JSON.parse(full) as { segments?: unknown }
    } catch {
      return null
    }
    if (parsed.segments !== undefined && !Array.isArray(parsed.segments)) return null
    delete parsed.segments
    return JSON.stringify(parsed)
  }

  /** Cache write on EVERY successful worker reply — including replies whose
   * client already left (their compute is still a valid future hit).
   * `unfiltered` ("show all") is deliberately never stored: 18 MB entries
   * would crowd out the point cache, and nothing reads that key. */
  private writePointCache(op: NoiseOnflyOp, lat: number, lng: number, full: string): void {
    if (op !== 'point') return
    const key = pointCacheKey(op, lat, lng)
    const summary = NoiseOnflySupervisor.deriveSummary(full)
    if (summary === null) return
    const bytes = Buffer.byteLength(full) + Buffer.byteLength(summary)
    const old = this.resultCache.get(key)
    if (old) this.resultCacheBytes -= old.bytes
    this.resultCache.delete(key)
    this.resultCache.set(key, { full, summary, bytes })
    this.resultCacheBytes += bytes
    while (
      (this.resultCache.size > RESULT_CACHE_MAX_ENTRIES
        || this.resultCacheBytes > RESULT_CACHE_MAX_BYTES)
      && this.resultCache.size > 0
    ) {
      const oldest = this.resultCache.keys().next().value!
      this.resultCacheBytes -= this.resultCache.get(oldest)!.bytes
      this.resultCache.delete(oldest)
    }
  }

  /** Summary view of a cached point answer, or null on miss. */
  cachedSummary(lat: number, lng: number): string | null {
    return this.cachedView(lat, lng, 'summary')
  }

  /** Full view of a cached point answer, or null on miss. */
  cachedFull(lat: number, lng: number): string | null {
    return this.cachedView(lat, lng, 'full')
  }

  private cachedView(lat: number, lng: number, view: 'summary' | 'full'): string | null {
    // Point-only: `unfiltered` answers are never stored, so no op key needed.
    const key = pointCacheKey('point', lat, lng)
    const hit = this.resultCache.get(key)
    if (!hit) return null
    this.resultCache.delete(key)
    this.resultCache.set(key, hit)
    return view === 'summary' ? hit.summary : hit.full
  }

  /** Obstacle footprints (as-used heights) in a bbox — the building-height
   *  debug overlay's data source; lat/lng = south-west, lat2/lng2 = north-east. */
  async queryObstacleFootprints(
    south: number, west: number, north: number, east: number, signal?: AbortSignal,
  ): Promise<string> {
    return this.enqueue(south, west, 'footprints', signal, north, east)
  }

  /** One vector obstacle containing a point, with its as-used height and type. */
  async queryBuildingAt(lat: number, lng: number, signal?: AbortSignal): Promise<string> {
    return this.enqueue(lat, lng, 'building-at', signal)
  }

  /** Read-only approximate outdoor layers; this op belongs to a separate small pool. */
  async querySurfaceCornerPreview(lat: number, lng: number, signal?: AbortSignal): Promise<string> {
    return this.enqueue(lat, lng, 'surface-preview', signal)
  }

  /**
   * Spawn one real pool worker and verify that it loaded the N-API addon and
   * completed sourceInit. The worker deliberately does not query a point, so
   * readiness cannot pre-cache point results before the first visitor query.
   */
  async checkReady(): Promise<void> {
    const response = JSON.parse(await this.enqueue(0, 0, 'ready')) as { ready?: unknown }
    if (response.ready !== true) {
      throw unavailableError('noise-onfly worker returned an invalid readiness response')
    }
  }

  private async enqueue(
    lat: number,
    lng: number,
    op: NoiseOnflyOp,
    signal?: AbortSignal,
    lat2?: number,
    lng2?: number,
    entryRef?: { id?: number },
  ): Promise<string> {
    if (this.closed) {
      throw unavailableError('noise-onfly supervisor is shutting down')
    }
    if (this.queue.length >= this.maxQueue) {
      this.log('warn', 'noise-onfly queue full', {
        queue_length: this.queue.length,
        active_requests: this.activeRequestIds(),
      })
      throw queueFullError()
    }

    return await new Promise<string>((resolve, reject) => {
      const entry: RequestEntry = {
        id: this.nextRequestId++,
        lat,
        lng,
        lat2,
        lng2,
        op,
        enqueuedAt: Date.now(),
        dispatchedAt: 0,
        resolve,
        reject,
        queueTimer: null,
        workTimer: null,
        worker: null,
        signal: undefined as unknown as AbortSignal,
        clientSettled: false,
        ctrl: new AbortController(),
        waiterTokens: new Set(),
        pinned: false,
      }
      entry.signal = entry.ctrl.signal

      if (signal?.aborted) {
        this.rejectClient(entry, abortError())
        return
      }

      entry.abortHandler = () => this.handleAbort(entry)
      entry.signal.addEventListener('abort', entry.abortHandler, { once: true })
      this.addWaiter(entry, signal)
      if (entryRef) entryRef.id = entry.id

      entry.queueTimer = setTimeout(() => {
        this.handleQueueTimeout(entry.id)
      }, this.queueTimeoutMs)

      this.queue.push(entry)
      this.log('info', 'noise-onfly queued request', {
        request_id: entry.id,
        queue_length: this.queue.length,
      })
      this.startNextIfPossible()
    })
  }

  warmWorkers(): void {
    if (this.closed) return
    for (const slot of this.slots) {
      if (slot.recycling) continue
      try { this.ensureWorker(slot) } catch (error) {
        this.log('warn', 'noise-onfly worker warmup failed', { error: toError(error).message })
      }
    }
  }

  async close(): Promise<void> {
    this.closed = true

    const shutdownError = unavailableError('noise-onfly supervisor is shutting down')
    while (this.queue.length > 0) {
      const entry = this.queue.shift()!
      this.clearQueueTimer(entry)
      this.detachAbortListener(entry)
      this.rejectClient(entry, shutdownError)
    }

    for (const slot of this.slots) {
      const active = slot.active
      slot.active = null
      if (active) {
        this.clearWorkTimer(active)
        active.worker = null
        this.detachAbortListener(active)
        this.rejectClient(active, shutdownError)
      }
      if (slot.recycling) {
        await slot.recycling
      }
      const current = slot.worker
      slot.worker = null
      if (current) {
        try {
          await current.terminate()
        } catch {
          // ignore terminate failures during shutdown
        }
      }
    }
  }

  private startNextIfPossible(): void {
    if (this.closed) {
      return
    }
    while (this.queue.length > 0) {
      const slot = this.slots.find((s) => !s.active && !s.recycling && !s.initializing)
      if (!slot) {
        return
      }

      try { this.ensureWorker(slot) } catch (error) {
        const failed = this.queue.shift()!
        this.clearQueueTimer(failed)
        this.rejectClient(failed, unavailableError(`noise-onfly worker spawn failed: ${toError(error).message}`))
        continue
      }
      if (slot.initializing) continue

      const entry = this.queue.shift()
      if (!entry) {
        return
      }
      if (entry.clientSettled) {
        // Aborted between enqueue and dispatch; try the next entry on this slot.
        continue
      }

      this.clearQueueTimer(entry)
      this.dispatchToSlot(slot, entry)
    }
  }

  private dispatchToSlot(slot: Slot, entry: RequestEntry): void {
    const worker = slot.worker!

    entry.worker = worker
    entry.dispatchedAt = Date.now()
    entry.workTimer = setTimeout(() => {
      void this.handleWorkTimeout(slot, entry.id)
    }, this.workTimeoutMs)
    slot.active = entry

    this.log('info', 'noise-onfly dispatched request', {
      request_id: entry.id,
      slot: slot.index,
      queue_length: this.queue.length,
    })

    try {
      worker.postMessage({ id: entry.id, lat: entry.lat, lng: entry.lng, lat2: entry.lat2, lng2: entry.lng2, op: entry.op })
    } catch (error) {
      this.finishActiveSlot(slot, entry)
      this.rejectClient(entry, unavailableError(`noise-onfly dispatch failed: ${toError(error).message}`))
      void this.recycleWorker(slot, worker, 'dispatch_failed')
    }
  }

  private ensureWorker(slot: Slot): NoiseOnflyWorker {
    if (slot.worker) {
      return slot.worker
    }

    const current = this.createWorker(slot.index)
    current.on('message', (message) => {
      if ('initialized' in message) return
      this.handleWorkerMessage(slot, current, message)
    })
    current.on('error', (err) => {
      void this.handleWorkerError(slot, current, err)
    })
    current.on('exit', (code) => {
      void this.handleWorkerExit(slot, current, code)
    })

    slot.worker = current
    slot.initializing = Boolean(current.ready)
    if (current.ready) {
      void current.ready.then(() => {
        if (this.closed || slot.worker !== current) return
        slot.initializing = false
        this.startNextIfPossible()
      }).catch((error) => this.handleWorkerError(slot, current, toError(error)))
    }
    this.log('info', 'noise-onfly worker spawned', { slot: slot.index })
    return current
  }

  private slotForActiveWorker(worker: NoiseOnflyWorker): Slot | null {
    return this.slots.find((s) => s.active?.worker === worker) ?? null
  }

  private handleWorkerMessage(
    slot: Slot,
    current: NoiseOnflyWorker,
    message: NoiseOnflyWorkerReply,
  ): void {
    if (slot.recyclingWorker === current) {
      return
    }

    const active = slot.active
    if (!active || active.worker !== current) {
      this.log('warn', 'noise-onfly received stray worker message', {
        request_id: message.id,
        slot: slot.index,
      })
      return
    }
    if (active.id !== message.id) {
      this.log('warn', 'noise-onfly worker reply id mismatch', {
        active_request_id: active.id,
        reply_request_id: message.id,
        slot: slot.index,
      })
      return
    }

    this.finishActiveSlot(slot, active)
    if (message.ok && message.resultJson !== undefined) {
      const now = Date.now()
      this.onTiming?.({
        op: active.op,
        queueMs: active.dispatchedAt - active.enqueuedAt,
        workMs: now - active.dispatchedAt,
        nativeMs: message.nativeMs,
        bytes: message.resultJson.length,
      })
      // Cache on every successful reply — even when the client already
      // left (resolveClient below is then a no-op, but the compute stays
      // a valid future hit).
      this.writePointCache(active.op, active.lat, active.lng, message.resultJson)
      this.resolveClient(active, message.resultJson)
    } else {
      this.rejectClient(
        active,
        new NoiseOnflyRequestError(
          message.error || 'noise-onfly worker failed',
          'NOISE_ONFLY_WORKER_FAILURE',
          500,
        ),
      )
    }
    queueMicrotask(() => this.startNextIfPossible())
  }

  private async handleWorkerError(
    slot: Slot,
    current: NoiseOnflyWorker,
    err: Error,
  ): Promise<void> {
    if (slot.recyclingWorker === current) {
      return
    }
    if (slot.worker !== current && slot.active?.worker !== current) {
      return
    }

    this.log('warn', 'noise-onfly worker error', {
      error: err.message,
      slot: slot.index,
      active_request_id: slot.active?.id ?? null,
    })

    const active = slot.active?.worker === current ? slot.active : null
    if (active) {
      this.finishActiveSlot(slot, active)
      this.rejectClient(active, unavailableError(`noise-onfly worker error: ${err.message}`))
    }

    await this.recycleWorker(slot, current, 'worker_error')
  }

  private async handleWorkerExit(
    slot: Slot,
    current: NoiseOnflyWorker,
    code: number,
  ): Promise<void> {
    if (slot.recyclingWorker === current) {
      return
    }
    if (slot.worker !== current && slot.active?.worker !== current) {
      return
    }

    this.log(code === 0 ? 'info' : 'warn', 'noise-onfly worker exited', {
      exit_code: code,
      slot: slot.index,
      active_request_id: slot.active?.id ?? null,
    })

    const active = slot.active?.worker === current ? slot.active : null
    if (active) {
      this.finishActiveSlot(slot, active)
      this.rejectClient(
        active,
        unavailableError(`noise-onfly worker exited with code ${code}`),
      )
    }

    await this.recycleWorker(slot, current, 'worker_exit', { skipTerminate: true })
  }

  private async handleWorkTimeout(slot: Slot, requestId: number): Promise<void> {
    const active = slot.active
    if (!active || active.id !== requestId) {
      return
    }
    const current = active.worker
    if (!current) {
      return
    }

    this.log('warn', 'noise-onfly request timed out', {
      request_id: active.id,
      slot: slot.index,
      queue_length: this.queue.length,
    })

    this.finishActiveSlot(slot, active)
    this.rejectClient(active, workTimeoutError(this.workTimeoutMs))
    await this.recycleWorker(slot, current, 'request_timeout')
  }

  private handleQueueTimeout(requestId: number): void {
    const queueIndex = this.queue.findIndex((entry) => entry.id === requestId)
    if (queueIndex === -1) {
      return
    }

    const [entry] = this.queue.splice(queueIndex, 1)
    this.clearQueueTimer(entry)
    this.detachAbortListener(entry)
    this.rejectClient(entry, queueTimeoutError(this.queueTimeoutMs))
    this.log('warn', 'noise-onfly queued request timed out', {
      request_id: requestId,
      queue_length: this.queue.length,
    })
  }

  private handleAbort(entry: RequestEntry): void {
    if (entry.worker) {
      const slot = this.slotForActiveWorker(entry.worker)
      if (slot && slot.active?.id === entry.id) {
        this.detachAbortListener(entry)
        this.rejectClient(entry, abortError())
        this.log('info', 'noise-onfly active request aborted', {
          request_id: entry.id,
          slot: slot.index,
        })
        return
      }
    }

    const queueIndex = this.queue.findIndex((candidate) => candidate.id === entry.id)
    if (queueIndex === -1) {
      return
    }

    this.queue.splice(queueIndex, 1)
    this.clearQueueTimer(entry)
    this.detachAbortListener(entry)
    this.rejectClient(entry, abortError())
    this.log('info', 'noise-onfly queued request aborted', {
      request_id: entry.id,
      queue_length: this.queue.length,
    })
  }

  private async recycleWorker(
    slot: Slot,
    current: NoiseOnflyWorker,
    reason: string,
    options: { skipTerminate?: boolean } = {},
  ): Promise<void> {
    if (slot.recycling) {
      return await slot.recycling
    }

    if (slot.worker === current) {
      slot.worker = null
      slot.initializing = false
    }
    slot.recyclingWorker = current
    this.log('warn', 'noise-onfly recycling worker', {
      reason,
      slot: slot.index,
      queue_length: this.queue.length,
    })

    slot.recycling = (async () => {
      if (!options.skipTerminate) {
        try {
          await current.terminate()
        } catch (error) {
          this.log('warn', 'noise-onfly worker terminate failed', {
            reason,
            slot: slot.index,
            error: toError(error).message,
          })
        }
      }
    })().finally(() => {
      if (slot.recyclingWorker === current) {
        slot.recyclingWorker = null
      }
      slot.recycling = null
      this.log('info', 'noise-onfly worker recycle complete', {
        slot: slot.index,
        queue_length: this.queue.length,
      })
      this.startNextIfPossible()
    })

    await slot.recycling
  }

  private finishActiveSlot(slot: Slot, entry: RequestEntry): void {
    if (slot.active?.id === entry.id) {
      slot.active = null
    }
    this.clearWorkTimer(entry)
    entry.worker = null
    this.detachAbortListener(entry)
  }

  private clearQueueTimer(entry: RequestEntry): void {
    if (entry.queueTimer) {
      clearTimeout(entry.queueTimer)
      entry.queueTimer = null
    }
  }

  private clearWorkTimer(entry: RequestEntry): void {
    if (entry.workTimer) {
      clearTimeout(entry.workTimer)
      entry.workTimer = null
    }
  }

  private detachAbortListener(entry: RequestEntry): void {
    if (entry.signal && entry.abortHandler) {
      entry.signal.removeEventListener('abort', entry.abortHandler)
      entry.abortHandler = undefined
    }
  }

  private resolveClient(entry: RequestEntry, resultJson: string): void {
    if (entry.clientSettled) {
      return
    }
    entry.clientSettled = true
    this.detachAbortListener(entry)
    entry.resolve(resultJson)
  }

  private rejectClient(entry: RequestEntry, err: Error): void {
    if (entry.clientSettled) {
      return
    }
    entry.clientSettled = true
    this.detachAbortListener(entry)
    entry.reject(err)
  }

  private activeRequestIds(): number[] {
    const ids: number[] = []
    for (const slot of this.slots) {
      if (slot.active) ids.push(slot.active.id)
    }
    return ids
  }

  private log(level: SupervisorLogLevel, message: string, meta?: Record<string, unknown>): void {
    this.logger?.(level, message, meta)
  }
}
