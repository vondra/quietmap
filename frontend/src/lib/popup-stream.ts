/** Streams one click's answer from `/api/popup`: every line replaces the previous answer. */
import type { PopupError, PopupUpdate } from '../types/noise.ts'

export interface PopupStreamCallbacks {
  /** Each streamed update, the whole answer so far; `partial` stays true until the last one. */
  onUpdate: (update: PopupUpdate) => void
  /** The click failed: whatever was shown for it is incomplete. Never called after an abort. */
  onError: (message: string) => void
}

const HTTP_MESSAGES: Record<number, string> = {
  429: 'Too many requests. Try again in a moment.',
  503: 'The noise service is busy. Try again in a moment.',
}

/** The visitor's message for a refused request; a 4xx carries the server's own words (which
 *  input was wrong), a 5xx body is never shown. */
async function refusalMessage(response: Response): Promise<string> {
  const known = HTTP_MESSAGES[response.status]
  if (known) return known
  const body = await response.json().catch(() => null) as { error?: unknown } | null
  if (response.status < 500 && typeof body?.error === 'string') return body.error
  return `The noise service failed (HTTP ${response.status}).`
}

function isPopupError(line: PopupUpdate | PopupError): line is PopupError {
  return 'error' in line
}

/**
 * Requests the answer at `position` and reports every streamed line. The stream must end with a
 * final (non-partial) update; an error line, a broken connection or a stream that stops early is
 * an error, never a quieter answer. Aborting `signal` (a new click, closing the popup) ends the
 * request silently, and the server stops the computation. `source`, an opened row's parts, adds
 * their pieces and how they arrive to the final update; `piece`, the index of one of them in the
 * loudest-first list, that piece with every ray's own profile.
 */
export async function streamPopup(
  position: { lat: number; lng: number },
  signal: AbortSignal,
  callbacks: PopupStreamCallbacks,
  { source, piece }: { source?: string[]; piece?: number } = {},
): Promise<void> {
  const params = new URLSearchParams({ lat: String(position.lat), lon: String(position.lng) })
  if (source) params.set('source', source.join(','))
  if (piece != null) params.set('piece', String(piece))
  let last: PopupUpdate | null = null
  try {
    const response = await fetch(`/api/popup?${params}`, { signal })
    if (!response.ok || !response.body) {
      const message = await refusalMessage(response)
      if (!signal.aborted) callbacks.onError(message)
      return
    }
    const reader = response.body.pipeThrough(new TextDecoderStream()).getReader()
    let buffered = ''
    for (;;) {
      const { value, done } = await reader.read()
      if (done) break
      buffered += value
      for (let newline = buffered.indexOf('\n'); newline >= 0; newline = buffered.indexOf('\n')) {
        const text = buffered.slice(0, newline)
        buffered = buffered.slice(newline + 1)
        if (!text.trim()) continue
        // A line that does not parse throws into the catch below: an error, like a lost connection.
        const line = JSON.parse(text) as PopupUpdate | PopupError
        if (signal.aborted) return
        if (isPopupError(line)) {
          callbacks.onError(line.error)
          return
        }
        last = line
        callbacks.onUpdate(line)
      }
    }
  } catch {
    if (!signal.aborted) {
      callbacks.onError(last
        ? 'The connection was lost before the final answer.'
        : 'The noise service cannot be reached.')
    }
    return
  }
  if (!signal.aborted && (last === null || last.partial)) {
    callbacks.onError('The answer ended before it was complete.')
  }
}
