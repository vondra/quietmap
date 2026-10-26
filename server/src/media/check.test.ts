// What an upload is: the recorder's WAV taken with its length; a stereo, float or 8-bit WAV, one
// whose header lies about its size, or anything else refused; a JPEG kept without its EXIF (the
// position in it), comments or what follows its end, every other segment as it was; no other image.
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import test from 'node:test'
import { pcm16, wavHeader } from '../../../frontend/src/lib/wav.ts'
import { photoOf, soundOf } from './check.ts'

const wav = (samples: number, rate = 48_000) => Buffer.concat([Buffer.from(wavHeader(samples * 2, rate)), Buffer.from(pcm16(new Float32Array(samples)).buffer)])

test("the recorder's WAV is taken with its length; any other WAV is refused", () => {
  assert.deepEqual(soundOf(wav(96_000)), { durationS: 2 })
  const edit = (at: number, value: number, bytes: 2 | 4) => {
    const file = wav(4_800)
    if (bytes === 2) file.writeUInt16LE(value, at)
    else file.writeUInt32LE(value, at)
    return file
  }
  for (const [what, file] of [
    ['stereo', edit(22, 2, 2)], ['float', edit(20, 3, 2)], ['8-bit', edit(34, 8, 2)], ['200 kHz', edit(24, 200_000, 4)],
    ['a longer data size', edit(40, 999_999, 4)], ['a cut file', wav(4_800).subarray(0, 1_000)], ['empty', wav(0)],
    ['a wrong byte rate', edit(28, 48_000, 4)], ['no block alignment', edit(32, 0, 2)],
    ['half a sample', Buffer.concat([Buffer.from(wavHeader(1, 48_000)), Buffer.from([0])])],
    ['not a WAV', Buffer.from('ID3 an mp3 file with tags and frames')],
  ] as const) {
    assert.equal(typeof soundOf(file), 'string', what)
  }
})

// 16 x 12 pixels, progressive (nine scans, tables between them), as a camera writes it: JFIF, EXIF
// with the maker and a GPS position (51°30'26" N, 0°7'39" W), a comment.
const PHOTO = readFileSync(new URL('./photo-with-gps.jpg', import.meta.url))

test('a JPEG is kept without its EXIF, comments and trailer, the image itself byte for byte', () => {
  const tables = PHOTO.indexOf(Buffer.from([0xff, 0xdb]))
  const kept = photoOf(Buffer.concat([PHOTO, Buffer.from('SEFH a phone maker trailer')]))
  assert.ok(Buffer.isBuffer(kept))
  // SOI and JFIF, then everything from the first quantization table to the end of the image.
  assert.deepEqual(kept, Buffer.concat([PHOTO.subarray(0, 20), PHOTO.subarray(tables)]))
  for (const gone of ['Exif', 'PhoneMaker', 'a comment', 'SEFH']) assert.equal(kept.indexOf(gone), -1, gone)
})

test('anything but a whole JPEG is refused', () => {
  for (const [what, file] of [
    ['cut before its end', PHOTO.subarray(0, 600)], ['without a scan', Buffer.from([0xff, 0xd8, 0xff, 0xd9])],
    ['a scan without a frame', Buffer.from([0xff, 0xd8, 0xff, 0xda, 0x00, 0x02, 0xff, 0xd9])],
    ['a PNG', Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0, 0])],
    ['a HEIC', Buffer.from('\x00\x00\x00\x18ftypheic', 'latin1')], ['an SVG', Buffer.from('<svg xmlns="http://www.w3.org/2000/svg"/>')],
    ['a segment longer than the file', Buffer.from([0xff, 0xd8, 0xff, 0xe1, 0x40, 0x00, 0, 0])],
  ] as const) {
    assert.equal(typeof photoOf(file), 'string', what)
  }
})
