type SupervisorLogLevel = 'info' | 'warn' | 'error'

export type NoiseOnflyWorkerReply = {
  id: number
  ok: boolean
  resultJson?: string
  error?: string
}

export type NoiseOnflyOp = 'point' | 'unfiltered' | 'ready' | 'footprints' | 'building-at'

export interface NoiseOnflyWorker {
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

type RequestEntry = {
  id: number
  lat: number
  lng: number
  /** bbox ops ('footprints'): north-east corner; lat/lng carry south-west. */
  lat2?: number
  lng2?: number
  op: NoiseOnflyOp
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
  recyclingWorker: NoiseOnflyWorker | null
  recycling: Promise<void> | null
}

export type NoiseOnflySupervisorConfig = {
  createWorker: NoiseOnflyWorkerFactory
  maxQueue: number
  queueTimeoutMs: number
  workTimeoutMs: number
  /**
   * Number of parallel NAPI workers. Default 1 (FIFO single-threaded —
   * preserves legacy behaviour and unit-test expectations). Set > 1 in
   * production to handle concurrent users without queueing — each worker
   * holds its own ~150 MB Rust state (R-trees per loaded R4) so memory
   * scales linearly with pool size; mmap'd Arrow + DEM rasters are
   * shared via OS page cache and don't duplicate.
   */
  poolSize?: number
  logger?: SupervisorLogger
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
  private readonly slots: Slot[]

  private readonly queue: RequestEntry[] = []
  private nextRequestId = 1
  private closed = false

  constructor(config: NoiseOnflySupervisorConfig) {
    this.createWorker = config.createWorker
    this.maxQueue = Math.max(0, config.maxQueue)
    this.queueTimeoutMs = Math.max(1, config.queueTimeoutMs)
    this.workTimeoutMs = Math.max(1, config.workTimeoutMs)
    this.logger = config.logger
    const poolSize = Math.max(1, config.poolSize ?? 1)
    this.slots = Array.from({ length: poolSize }, (_, index) => ({
      index,
      worker: null,
      active: null,
      recyclingWorker: null,
      recycling: null,
    }))
  }

  async queryNoiseAtPoint(lat: number, lng: number, signal?: AbortSignal): Promise<string> {
    return this.enqueue(lat, lng, 'point', signal)
  }

  async queryNoiseAtPointUnfiltered(lat: number, lng: number, signal?: AbortSignal): Promise<string> {
    return this.enqueue(lat, lng, 'unfiltered', signal)
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

  /**
   * Spawn one real pool worker and verify that it loaded the N-API addon and
   * completed sourceInit. The worker deliberately does not query a point, so
   * readiness cannot pre-cache mutable H3 cells during an enrichment repaint.
   */
  async checkReady(): Promise<void> {
    const response = JSON.parse(await this.enqueue(0, 0, 'ready')) as { ready?: unknown }
    if (response.ready !== true) {
      throw unavailableError('noise-onfly worker returned an invalid readiness response')
    }
  }

  private async enqueue(lat: number, lng: number, op: NoiseOnflyOp, signal?: AbortSignal, lat2?: number, lng2?: number): Promise<string> {
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
        resolve,
        reject,
        queueTimer: null,
        workTimer: null,
        worker: null,
        signal,
        clientSettled: false,
      }

      if (signal?.aborted) {
        this.rejectClient(entry, abortError())
        return
      }

      if (signal) {
        entry.abortHandler = () => this.handleAbort(entry)
        signal.addEventListener('abort', entry.abortHandler, { once: true })
      }

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
      const slot = this.slots.find((s) => !s.active && !s.recycling)
      if (!slot) {
        return
      }

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
    let worker: NoiseOnflyWorker
    try {
      worker = this.ensureWorker(slot)
    } catch (error) {
      this.rejectClient(entry, unavailableError(`noise-onfly worker spawn failed: ${toError(error).message}`))
      queueMicrotask(() => this.startNextIfPossible())
      return
    }

    entry.worker = worker
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
      this.handleWorkerMessage(slot, current, message)
    })
    current.on('error', (err) => {
      void this.handleWorkerError(slot, current, err)
    })
    current.on('exit', (code) => {
      void this.handleWorkerExit(slot, current, code)
    })

    slot.worker = current
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
