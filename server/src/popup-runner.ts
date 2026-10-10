// Runs `qm-popup` for the clicks: a few at a time (each uses every core), a short queue behind
// them, and every child killed as soon as its request goes away or it runs too long.
import { spawn } from 'node:child_process'

export interface PopupRequest {
  year: string
  lat: number
  lon: number
  /** An opened row's sound path: the group ids of its parts (16 hex digits each). */
  source?: string[]
  /** Of that source, only its piece of this rank (loudest first) with every ray's ground. */
  piece?: number
}

export interface PopupRunHandlers {
  /** One streamed update: a complete line of the child's output, without its newline. */
  onLine: (line: string) => void
  /** The run is over: `null` after a final update and a clean exit, otherwise what failed. A
   *  failure is never followed by more lines; the answer so far is incomplete. */
  onEnd: (failure: PopupFailure | null) => void
}

export interface PopupFailure {
  /** For the visitor: no paths, no internals. */
  message: string
  /** For the log: the exit status and the tail of the child's stderr. */
  detail: string
}

export interface PopupRunnerOptions {
  popupBin: string
  preparedDir: string
  concurrency: number
  /** Clicks that may wait for a free slot; one more is refused. */
  queueLength: number
  /** A click still running after this is killed and fails. */
  timeoutMs: number
}

/** The benchmark's `--exact` never reaches a visitor: the arguments are built here. */
export function popupArguments(preparedDir: string, request: PopupRequest): string[] {
  return [
    '--prepared', preparedDir,
    '--year', request.year,
    '--lat', String(request.lat),
    '--lon', String(request.lon),
    ...(request.source ? ['--source', request.source.join(',')] : []),
    ...(request.piece !== undefined ? ['--piece', String(request.piece)] : []),
  ]
}

const STDERR_TAIL_BYTES = 2000

/** Whether a line is an update and whether it is the final one; `null` when it is no update. */
function updateFinality(line: string): 'partial' | 'final' | null {
  try {
    const update = JSON.parse(line) as { partial?: unknown } | null
    if (typeof update !== 'object' || update === null || typeof update.partial !== 'boolean') return null
    return update.partial ? 'partial' : 'final'
  } catch {
    return null
  }
}

export class PopupRunner {
  private readonly options: PopupRunnerOptions
  private running = 0
  private readonly queue: (() => void)[] = []

  constructor(options: PopupRunnerOptions) {
    this.options = options
  }

  /**
   * Starts the click now, or queues it while every slot is busy. Returns the function that
   * cancels it (queued: dropped; running: its child killed), or `null` when the queue is full.
   */
  submit(request: PopupRequest, handlers: PopupRunHandlers): (() => void) | null {
    let kill: (() => void) | null = null
    let cancelled = false
    const start = () => {
      if (!cancelled) kill = this.start(request, handlers)
    }
    if (this.running < this.options.concurrency) {
      start()
    } else if (this.queue.length < this.options.queueLength) {
      this.queue.push(start)
    } else {
      return null
    }
    return () => {
      if (cancelled) return
      cancelled = true
      const queued = this.queue.indexOf(start)
      if (queued >= 0) this.queue.splice(queued, 1)
      kill?.()
    }
  }

  private start(request: PopupRequest, handlers: PopupRunHandlers): () => void {
    this.running += 1
    const child = spawn(this.options.popupBin, popupArguments(this.options.preparedDir, request), {
      stdio: ['ignore', 'pipe', 'pipe'],
    })
    let pending = ''
    let last: 'partial' | 'final' | null = null
    let stderr = ''
    let failure: PopupFailure | null = null
    let finished = false
    const fail = (message: string, detail: string) => {
      failure ??= { message, detail }
      child.kill('SIGKILL')
    }
    const timer = setTimeout(
      () => fail('The noise computation took too long.', `killed after ${this.options.timeoutMs} ms`),
      this.options.timeoutMs,
    )
    const finish = (status: string) => {
      if (finished) return
      finished = true
      clearTimeout(timer)
      this.running -= 1
      if (!failure && pending !== '') failure = { message: 'The noise computation failed.', detail: 'output ended inside a line' }
      if (!failure && status !== 'exit code 0') failure = { message: 'The noise computation failed.', detail: status }
      if (!failure && last !== 'final') failure = { message: 'The noise computation failed.', detail: 'ended without a final update' }
      if (failure) failure.detail += stderr ? `; stderr: ${stderr.trim()}` : ''
      handlers.onEnd(failure)
      this.queue.shift()?.()
    }
    child.stdout.setEncoding('utf8')
    child.stdout.on('data', (chunk: string) => {
      if (failure) return
      pending += chunk
      for (let newline = pending.indexOf('\n'); newline >= 0; newline = pending.indexOf('\n')) {
        const line = pending.slice(0, newline)
        pending = pending.slice(newline + 1)
        last = updateFinality(line)
        if (last === null) {
          fail('The noise computation failed.', 'a line that is no update')
          return
        }
        handlers.onLine(line)
      }
    })
    child.stderr.setEncoding('utf8')
    child.stderr.on('data', (chunk: string) => {
      stderr = (stderr + chunk).slice(-STDERR_TAIL_BYTES)
    })
    child.on('error', (error) => {
      failure ??= { message: 'The noise computation failed.', detail: error.message }
      finish('not started')
    })
    child.on('close', (code, signal) => finish(signal ? `signal ${signal}` : `exit code ${code}`))
    return () => {
      failure ??= { message: 'cancelled', detail: 'the request went away' }
      child.kill('SIGKILL')
    }
  }
}
