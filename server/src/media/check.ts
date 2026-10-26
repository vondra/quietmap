// What an upload is, read from its bytes: a recording must be the WAV the map's recorder writes
// (frontend/src/lib/wav.ts: 16-bit PCM, one channel, 8 to 96 kHz, its header's sizes true, at most
// its most samples), a photo a JPEG, kept without its metadata; anything else is refused.
import type { MediaItem } from '../../../frontend/src/lib/media.ts'
import { RECORDING_SAMPLES_MAX } from '../../../frontend/src/lib/wav.ts'

export type MediaKind = MediaItem['kind']
export const MEDIA_KINDS: Record<MediaKind, { mime: string, extension: string }> = {
  sound: { mime: 'audio/wav', extension: '.wav' },
  photo: { mime: 'image/jpeg', extension: '.jpg' },
}

const ascii = (file: Buffer, at: number, length: number) => file.toString('latin1', at, at + length)

/** The length (s) of the recording a WAV file holds, or what is wrong with it. */
export function soundOf(file: Buffer): { durationS: number } | string {
  if (file.length < 44 || ascii(file, 0, 4) !== 'RIFF' || ascii(file, 8, 8) !== 'WAVEfmt ' || ascii(file, 36, 4) !== 'data') {
    return 'a recording must be a WAV file'
  }
  const [format, channels, sampleRate, byteRate, blockAlign, bits] = [file.readUInt16LE(20), file.readUInt16LE(22),
    file.readUInt32LE(24), file.readUInt32LE(28), file.readUInt16LE(32), file.readUInt16LE(34)]
  if (file.readUInt32LE(16) !== 16 || format !== 1 || channels !== 1 || bits !== 16 || blockAlign !== 2
    || sampleRate < 8_000 || sampleRate > 96_000 || byteRate !== sampleRate * 2) {
    return 'a recording must be 16-bit PCM, one channel, 8 to 96 kHz'
  }
  const dataBytes = file.readUInt32LE(40)
  if (dataBytes !== file.length - 44 || file.readUInt32LE(4) !== file.length - 8 || dataBytes === 0 || dataBytes % 2 !== 0) {
    return "the recording's sizes do not match its length"
  }
  if (dataBytes > RECORDING_SAMPLES_MAX * 2) return `a recording is at most ${RECORDING_SAMPLES_MAX} samples`
  return { durationS: dataBytes / 2 / sampleRate }
}

/** A segment the photo keeps: the image's own (tables, frame, scans) and, of the application
 *  segments, JFIF (APP0), the colour profile (APP2 ICC_PROFILE) and Adobe's colour transform
 *  (APP14). EXIF and XMP (APP1: the camera, the time, the position), IPTC (APP13), the rest and
 *  comments go. */
function kept(marker: number, segment: Buffer): boolean {
  if (marker === 0xfe) return false
  if (marker < 0xe0 || marker > 0xef) return true
  return marker === 0xe0 || marker === 0xee || (marker === 0xe2 && ascii(segment, 4, 12) === 'ICC_PROFILE\0')
}

/** A frame header (SOF0-15), which says the image's size and coding; 0xC4, 0xC8 and 0xCC are tables. */
const isFrame = (marker: number) => marker >= 0xc0 && marker <= 0xcf && marker !== 0xc4 && marker !== 0xc8 && marker !== 0xcc

/** A JPEG without its metadata, nor anything after its end, or what is wrong with it. */
export function photoOf(file: Buffer): Buffer | string {
  const wrong = 'a photo must be a JPEG'
  if (file.length < 4 || file[0] !== 0xff || file[1] !== 0xd8) return wrong
  const parts = [file.subarray(0, 2)]
  let framed = false
  let scanned = false
  for (let at = 2; at + 2 <= file.length;) {
    const marker = file[at + 1]
    if (file[at] !== 0xff) return wrong
    if (marker === 0xff) { // fill
      at++
      continue
    }
    if (marker === 0xd9) return scanned ? Buffer.concat([...parts, file.subarray(at, at + 2)]) : wrong
    // The markers without a length belong inside a scan's coded data only.
    if (marker === 0x01 || (marker >= 0xd0 && marker <= 0xd8) || at + 4 > file.length) return wrong
    let end = at + 2 + file.readUInt16BE(at + 2)
    if (end > file.length || end < at + 4) return wrong
    framed ||= isFrame(marker)
    if (marker === 0xda) {
      if (!framed) return wrong
      // A scan's coded data runs to the next marker: neither a stuffed 0xFF00 nor a restart.
      while (end + 1 < file.length && !(file[end] === 0xff && file[end + 1] !== 0 && (file[end + 1] < 0xd0 || file[end + 1] > 0xd7))) end++
      scanned = true
    }
    if (kept(marker, file.subarray(at, end))) parts.push(file.subarray(at, end))
    at = end
  }
  return wrong
}
