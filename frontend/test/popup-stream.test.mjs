// The streamed popup client: lines as they arrive, error lines, early ends, aborts, refused requests.
import assert from 'node:assert/strict'
import test from 'node:test'

import { streamPopup } from '../src/lib/popup-stream.ts'

const update = (seq, partial, lden = 60) => ({ seq, partial, total_lden: lden })

/** A streamed response whose body the test writes chunk by chunk. */
function controlledResponse(t, status = 200) {
  let controller
  const body = new ReadableStream({ start(c) { controller = c } })
  const encoder = new TextEncoder()
  t.mock.method(globalThis, 'fetch', async (url, init) => {
    init.signal.addEventListener('abort', () => controller.error(new DOMException('aborted', 'AbortError')))
    return new Response(body, { status, headers: { 'content-type': 'application/x-ndjson' } })
  })
  return {
    write: (text) => controller.enqueue(encoder.encode(text)),
    end: () => controller.close(),
  }
}

function run(signal = new AbortController().signal) {
  const events = []
  const done = streamPopup({ lat: 50, lng: 14 }, signal, {
    onUpdate: (u) => events.push(['update', u.seq, u.partial]),
    onError: (message) => events.push(['error', message]),
  })
  return { events, done }
}

const tick = () => new Promise((resolve) => setImmediate(resolve))

test('every line is reported as it arrives, split across chunks, and a final update ends it cleanly', async (t) => {
  const stream = controlledResponse(t)
  const { events, done } = run()
  const first = JSON.stringify(update(1, true))
  stream.write(first.slice(0, 7))
  await tick()
  assert.deepEqual(events, [])
  stream.write(`${first.slice(7)}\n`)
  await tick()
  assert.deepEqual(events, [['update', 1, true]])
  stream.write(`${JSON.stringify(update(2, false))}\n`)
  stream.end()
  await done
  assert.deepEqual(events, [['update', 1, true], ['update', 2, false]])
})

test('the request names the point, an opened row\'s parts and one of their pieces when asked for', async (t) => {
  const requested = []
  t.mock.method(globalThis, 'fetch', async (url) => {
    requested.push(url)
    return new Response(`${JSON.stringify(update(1, false))}\n`)
  })
  await run().done
  const callbacks = { onUpdate: () => {}, onError: () => {} }
  const source = ['98310618668d60db', 'a2d8108c83a87fed']
  await streamPopup({ lat: 50, lng: 14 }, new AbortController().signal, callbacks, { source })
  await streamPopup({ lat: 50, lng: 14 }, new AbortController().signal, callbacks, { source, piece: 0 })
  assert.deepEqual(requested, [
    '/api/popup?lat=50&lon=14',
    '/api/popup?lat=50&lon=14&source=98310618668d60db%2Ca2d8108c83a87fed',
    '/api/popup?lat=50&lon=14&source=98310618668d60db%2Ca2d8108c83a87fed&piece=0',
  ])
})

test('an error line is an error, after whatever came before it', async (t) => {
  const stream = controlledResponse(t)
  const { events, done } = run()
  stream.write(`${JSON.stringify(update(1, true))}\n{"error":"the noise computation failed"}\n`)
  stream.end()
  await done
  assert.deepEqual(events, [['update', 1, true], ['error', 'the noise computation failed']])
})

test('a stream that stops before its final update is an error, never a quieter answer', async (t) => {
  const stream = controlledResponse(t)
  const { events, done } = run()
  stream.write(`${JSON.stringify(update(1, true))}\n`)
  stream.end()
  await done
  assert.deepEqual(events, [['update', 1, true], ['error', 'The answer ended before it was complete.']])
})

test('an abort (a new click) ends the request silently', async (t) => {
  const stream = controlledResponse(t)
  const controller = new AbortController()
  const { events, done } = run(controller.signal)
  stream.write(`${JSON.stringify(update(1, true))}\n`)
  await tick()
  controller.abort()
  await done
  assert.deepEqual(events, [['update', 1, true]])
})

test('a refused request reports the server\'s words for a 4xx and a fixed sentence otherwise', async (t) => {
  const cases = [
    [400, { error: 'lat must be within ±85.05°' }, 'lat must be within ±85.05°'],
    [429, { error: 'Too Many Requests', message: 'Rate limit exceeded' }, 'Too many requests. Try again in a moment.'],
    [503, { error: 'busy' }, 'The noise service is busy. Try again in a moment.'],
    [500, { error: 'internal detail' }, 'The noise service failed (HTTP 500).'],
  ]
  for (const [status, body, expected] of cases) {
    t.mock.method(globalThis, 'fetch', async () => new Response(JSON.stringify(body), { status }))
    const { events, done } = run()
    await done
    assert.deepEqual(events, [['error', expected]])
    t.mock.restoreAll()
  }
})

test('an unreachable service is an error', async (t) => {
  t.mock.method(globalThis, 'fetch', async () => { throw new TypeError('fetch failed') })
  const { events, done } = run()
  await done
  assert.deepEqual(events, [['error', 'The noise service cannot be reached.']])
})
