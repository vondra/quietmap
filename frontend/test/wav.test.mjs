// A recording's WAV file: the header a player and the server read, the samples in order, clipped.
import assert from 'node:assert/strict'
import test from 'node:test'

import { pcm16, wavFile } from '../src/lib/wav.ts'

test('a WAV of two blocks: RIFF, PCM, one channel, 16 bits at its rate, the samples in order', async () => {
  const blob = wavFile([pcm16(new Float32Array([0, 1, -1])), pcm16(new Float32Array([0.5, 2, -3]))], 48_000)
  assert.equal(blob.type, 'audio/wav')
  const file = await blob.arrayBuffer()
  const view = new DataView(file)
  const text = (at, length) => String.fromCharCode(...new Uint8Array(file, at, length))
  assert.equal(text(0, 4), 'RIFF')
  assert.equal(view.getUint32(4, true), file.byteLength - 8)
  assert.equal(text(8, 8), 'WAVEfmt ')
  assert.deepEqual([view.getUint16(20, true), view.getUint16(22, true), view.getUint32(24, true), view.getUint16(34, true)], [1, 1, 48_000, 16])
  assert.equal(view.getUint32(28, true), 96_000)
  assert.equal(text(36, 4), 'data')
  assert.equal(view.getUint32(40, true), 12)
  assert.deepEqual([...new Int16Array(file, 44)], [0, 32767, -32768, 16384, 32767, -32768])
})
