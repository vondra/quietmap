import assert from 'node:assert/strict'
import test from 'node:test'
import { fetchNoiseDetail } from '../src/lib/fetch-noise-detail.ts'

const position = { lat: 50, lng: 14 }
const preview = { status: 'provisional', receiver: 'outdoor', accuracy: 'unmeasured', center: [50, 14], layers: [] }
const exact = { total_lden: 53 }

function setup(t) {
  const pending = new Map()
  t.mock.method(globalThis, 'fetch', url => new Promise(resolve => pending.set(url.split('?')[0], resolve)))
  const events = []
  const controller = new AbortController()
  let current = true
  const done = fetchNoiseDetail(position, controller.signal, {
    isCurrent: () => current,
    onData: data => events.push(['data', data]),
    onPreview: data => events.push(['preview', data]),
    onError: error => events.push(['error', error]),
  })
  const resolve = async (path, data, status = 200) => {
    pending.get(path)(new Response(JSON.stringify(data), { status }))
    await new Promise(resolve => setImmediate(resolve))
  }
  return { events, controller, done, resolve, invalidate: () => { current = false } }
}

test('provisional layers arrive first and exact data replaces them; a late preview cannot downgrade exact data', async t => {
  for (const previewFirst of [true, false]) {
    const run = setup(t)
    if (previewFirst) await run.resolve('/api/noise-preview', preview)
    await run.resolve('/api/noise-onfly-v2', exact)
    if (!previewFirst) await run.resolve('/api/noise-preview', preview)
    await run.done
    assert.deepEqual(run.events, [...(previewFirst ? [['preview', preview]] : []), ['data', exact]])
    t.mock.restoreAll()
  }
})

test('closing or moving the point suppresses responses even if the transport ignores abort', async t => {
  const run = setup(t)
  run.controller.abort()
  await run.resolve('/api/noise-onfly-v2', exact)
  await run.resolve('/api/noise-preview', preview)
  await run.done
  assert.deepEqual(run.events, [])
})

test('failed exact query retains the clearly provisional result; unavailable or mismatched preview cannot affect exact', async t => {
  const failed = setup(t)
  await failed.resolve('/api/noise-preview', preview)
  await failed.resolve('/api/noise-onfly-v2', {}, 503)
  await failed.done
  assert.deepEqual(failed.events, [['preview', preview], ['error', 'API 503']])
  t.mock.restoreAll()
  for (const response of [null, { ...preview, center: [51, 14] }]) {
    const run = setup(t)
    await run.resolve('/api/noise-preview', response)
    await run.resolve('/api/noise-onfly-v2', exact)
    await run.done
    assert.deepEqual(run.events, [['data', exact]])
    t.mock.restoreAll()
  }
})


test('a new click invalidates old data, preview and error before the effect has aborted the old request', async t => {
  for (const status of [200, 503]) {
    const run = setup(t)
    run.invalidate()
    assert.equal(run.controller.signal.aborted, false)
    await run.resolve('/api/noise-preview', preview)
    await run.resolve('/api/noise-onfly-v2', exact, status)
    await run.done
    assert.deepEqual(run.events, [])
    t.mock.restoreAll()
  }
})
