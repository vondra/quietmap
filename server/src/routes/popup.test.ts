// The streamed popup route against a fake `qm-popup`: line order and flushing, validation, error
// lines, killed children on disconnect and timeout, and the full queue.
import assert from 'node:assert/strict'
import { mkdtemp, readFile, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { request as httpRequest, type IncomingMessage } from 'node:http'
import test, { after } from 'node:test'
import { buildApp, type AppConfig } from '../app.ts'

const FAKE_POPUP = join(import.meta.dirname, '..', 'test-fixtures', 'fake-qm-popup.ts')
const PREPARED = '/prepared-release'
const scratch = await mkdtemp(join(tmpdir(), 'popup-route-test-'))
after(() => rm(scratch, { recursive: true, force: true }))

function config(overrides: Partial<AppConfig> = {}): AppConfig {
  return {
    popupBin: FAKE_POPUP,
    preparedDir: PREPARED,
    years: ['2026', '2025'],
    popupConcurrency: 2,
    tilesDir: scratch,
    noIndex: false,
    photonUrl: 'http://127.0.0.1:9',
    ...overrides,
  }
}

/** A listening app (streaming needs a real socket, `inject` buffers the whole reply). Requests
 *  use one connection each and no pool, so an abort closes exactly its own connection. */
async function listen(t: test.TestContext, overrides: Partial<AppConfig> = {}, popupTimeoutMs?: number) {
  const app = await buildApp(config(overrides), { popupTimeoutMs })
  t.after(() => app.close())
  const address = await app.listen({ port: 0, host: '127.0.0.1' })
  return (query: string, signal?: AbortSignal) => new Promise<IncomingMessage>((resolve, reject) => {
    const request = httpRequest(`${address}/api/popup?${query}`, {
      agent: false,
      signal,
      headers: { 'accept-encoding': 'gzip, br' },
    }, resolve)
    request.on('error', reject)
    request.end()
  })
}

type Line = { at: number; value: Record<string, unknown> }

/** Reads the streamed lines, each with its arrival time. With `stopAfter` it returns once that
 *  many arrived and leaves the connection open (the click keeps computing). */
function lines(response: IncomingMessage, stopAfter = Infinity): Promise<Line[]> {
  return new Promise((resolve, reject) => {
    const read: Line[] = []
    let pending = ''
    let settled = false
    const settle = (error?: Error) => {
      if (settled) return
      settled = true
      if (error) reject(error)
      else resolve(read)
    }
    response.setEncoding('utf8')
    response.on('data', (chunk: string) => {
      pending += chunk
      for (let newline = pending.indexOf('\n'); newline >= 0; newline = pending.indexOf('\n')) {
        read.push({ at: performance.now(), value: JSON.parse(pending.slice(0, newline)) })
        pending = pending.slice(newline + 1)
      }
      if (read.length >= stopAfter) settle()
    })
    response.on('end', () => settle(pending === '' ? undefined : new Error('the stream ended inside a line')))
    response.on('error', (error) => settle(error))
  })
}

async function text(response: IncomingMessage): Promise<string> {
  response.setEncoding('utf8')
  let body = ''
  for await (const chunk of response) body += chunk
  return body
}

function running(pid: number): boolean {
  try {
    process.kill(pid, 0)
    return true
  } catch {
    return false
  }
}

async function eventually(condition: () => boolean, what: string): Promise<void> {
  const deadline = Date.now() + 3000
  while (!condition()) {
    if (Date.now() > deadline) assert.fail(`timed out waiting until ${what}`)
    await new Promise((resolve) => setTimeout(resolve, 20))
  }
}

test('each update is its own line, flushed the moment the child writes it, never compressed', async (t) => {
  const popup = await listen(t)
  const response = await popup('lat=1.5&lon=14.25')
  assert.equal(response.statusCode, 200)
  assert.equal(response.headers['content-type'], 'application/x-ndjson; charset=utf-8')
  assert.equal(response.headers['cache-control'], 'no-store, no-transform')
  assert.equal(response.headers['x-accel-buffering'], 'no')
  assert.equal(response.headers['content-encoding'], undefined)
  const [first, last, ...rest] = await lines(response)
  assert.deepEqual(rest, [])
  assert.deepEqual([first.value.seq, first.value.partial, last.value.seq, last.value.partial], [1, true, 2, false])
  // The child writes the final line 300 ms after the first: the first must not wait for it.
  assert.ok(last.at - first.at > 200, `the first line waited ${Math.round(last.at - first.at)} ms short of the last`)
  assert.deepEqual(first.value.argv, ['--prepared', PREPARED, '--year', '2026', '--lat', '1.5', '--lon', '14.25'])
})

test('a click may name a configured year; longitude wraps into -180..180', async (t) => {
  const popup = await listen(t)
  const cases: [string, string, string][] = [
    ['lat=1&lon=190&year=2025', '2025', '-170'],
    ['lat=1&lon=180', '2026', '-180'],
    ['lat=1&lon=-540.5', '2026', '179.5'],
  ]
  for (const [query, year, lon] of cases) {
    const [first] = await lines(await popup(query))
    assert.deepEqual((first.value.argv as string[]).slice(2), ['--year', year, '--lat', '1', '--lon', lon], query)
  }
})

test('the segments view asks the popup for its fixed count of pieces per layer', async (t) => {
  const popup = await listen(t)
  const [first] = await lines(await popup('lat=1&lon=14&segments=1'))
  assert.deepEqual((first.value.argv as string[]).slice(2), ['--year', '2026', '--lat', '1', '--lon', '14', '--pieces', '8'])
})

test('an invalid point or year, or a HEAD request, is refused before anything runs', async (t) => {
  const app = await buildApp(config())
  t.after(() => app.close())
  for (const [query, message] of [
    ['lon=14', /^lat must be/],
    ['lat=abc&lon=14', /^lat must be/],
    ['lat=&lon=14', /^lat must be/],
    ['lat=0x10&lon=14', /^lat must be/],
    ['lat=85.1&lon=14', /^lat must be a number within ±85.05$/],
    ['lat=-90&lon=14', /^lat must be/],
    ['lat=1e400&lon=14', /^lat must be/],
    ['lat=50', /^lon must be/],
    ['lat=50&lon=Infinity', /^lon must be/],
    ['lat=50&lon=14&year=1999', /^year must be one of 2026, 2025$/],
    ['lat=50&lon=14&year=', /^year must be one of/],
    ['lat=50&lon=14&segments=all', /^segments must be 1$/],
  ] as const) {
    const response = await app.inject(`/api/popup?${query}`)
    assert.equal(response.statusCode, 400, query)
    assert.match(response.json().error, message, query)
  }
  assert.equal((await app.inject({ method: 'HEAD', url: '/api/popup?lat=1&lon=14' })).statusCode, 404)
})

test('a failing child ends the stream with one error line that names no path', async (t) => {
  const popup = await listen(t)
  const body = await text(await popup('lat=2&lon=14'))
  const [first, error, ...rest] = body.trimEnd().split('\n').map((line) => JSON.parse(line))
  assert.equal(first.partial, true)
  // The visitor's line: the failure in words, never the child's stderr or a path.
  assert.deepEqual(error, { error: 'The noise computation failed.' })
  assert.deepEqual(rest, [])
})

test('a child that stops without a final update, or writes no update, fails', async (t) => {
  const popup = await listen(t)
  for (const query of ['lat=4&lon=14', 'lat=5&lon=14']) {
    const read = await lines(await popup(query))
    assert.deepEqual(read.at(-1)!.value, { error: 'The noise computation failed.' }, query)
    assert.ok(read.slice(0, -1).every((line) => line.value.partial === true), query)
  }
})

test('a binary that cannot start answers an error line', async (t) => {
  const popup = await listen(t, { popupBin: join(scratch, 'no-such-binary') })
  const read = await lines(await popup('lat=1&lon=14'))
  assert.deepEqual(read.map((line) => line.value), [{ error: 'The noise computation failed.' }])
})

test('a client that disconnects has its child killed', async (t) => {
  const popup = await listen(t)
  const controller = new AbortController()
  const [first] = await lines(await popup('lat=3&lon=14', controller.signal), 1)
  const pid = first.value.pid as number
  assert.ok(running(pid))
  controller.abort()
  await eventually(() => !running(pid), `child ${pid} is gone`)
})

test('a child running past the time limit is killed and answers an error', async (t) => {
  const popup = await listen(t, {}, 300)
  const read = await lines(await popup('lat=3&lon=14'))
  assert.equal(read.length, 2)
  assert.deepEqual(read[1].value, { error: 'The noise computation took too long.' })
  await eventually(() => !running(read[0].value.pid as number), 'the child is gone')
})

test('a full queue answers 503 at once; a click that leaves the queue never starts', async (t) => {
  const spawnLog = join(scratch, 'spawned')
  process.env.FAKE_POPUP_SPAWN_LOG = spawnLog
  t.after(() => { delete process.env.FAKE_POPUP_SPAWN_LOG })
  const popup = await listen(t, { popupConcurrency: 1 })
  const controllers = Array.from({ length: 3 }, () => new AbortController())
  // One click computes, two wait (two per slot).
  const running1 = await popup('lat=3.1&lon=14', controllers[0].signal)
  const [first] = await lines(running1, 1)
  const queued2 = popup('lat=3.2&lon=14', controllers[1].signal)
  const queued3 = popup('lat=3.3&lon=14', controllers[2].signal)
  await new Promise((resolve) => setTimeout(resolve, 100))
  const refused = await popup('lat=3.4&lon=14')
  assert.equal(refused.statusCode, 503)
  assert.equal(refused.headers['retry-after'], '1')
  assert.deepEqual(JSON.parse(await text(refused)), { error: 'The noise service is busy. Try again in a moment.' })

  // The third leaves while waiting; the first leaves while computing: the second takes the slot.
  controllers[2].abort()
  await assert.rejects(queued3)
  controllers[0].abort()
  await eventually(() => !running(first.value.pid as number), 'the first child is gone')
  const [second] = await lines(await queued2, 1)
  assert.deepEqual(second.value.center, [3.2, 14])
  controllers[1].abort()
  await eventually(() => !running(second.value.pid as number), 'the second child is gone')
  // Only the first two ever started: neither the refused nor the abandoned click.
  assert.deepEqual((await readFile(spawnLog, 'utf8')).trimEnd().split('\n'), ['3.1', '3.2'])
})
