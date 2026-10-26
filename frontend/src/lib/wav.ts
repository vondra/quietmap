// A recording's samples as a WAV file: 16-bit PCM, one channel, at the rate they were taken, the one
// format the server takes (server/src/media/check.ts). 16 bits keep 96 dB below full scale, more than
// a phone's microphone, which hisses at about 30-35 dB(A).
const HEADER_BYTES = 44
/** The most samples a recording keeps: ten minutes at 48 kHz, 57.6 MB, the length statistical
 *  levels (L50, L90) are measured over. */
export const RECORDING_SAMPLES_MAX = 28_800_000

/** `samples` (each within ±1, beyond it clipped) as 16-bit PCM. */
export function pcm16(samples: Float32Array): Int16Array<ArrayBuffer> {
  const pcm = new Int16Array(samples.length)
  for (let k = 0; k < samples.length; k++) {
    const sample = Math.max(-1, Math.min(1, samples[k]))
    pcm[k] = Math.round(sample < 0 ? sample * 0x8000 : sample * 0x7fff)
  }
  return pcm
}

/** The header of a WAV file of `dataBytes` of 16-bit PCM, one channel, taken at `sampleRate`. */
export function wavHeader(dataBytes: number, sampleRate: number): ArrayBuffer {
  const header = new ArrayBuffer(HEADER_BYTES)
  const view = new DataView(header)
  const ascii = (at: number, text: string) => [...text].forEach((c, k) => view.setUint8(at + k, c.charCodeAt(0)))
  ascii(0, 'RIFF')
  view.setUint32(4, HEADER_BYTES - 8 + dataBytes, true)
  ascii(8, 'WAVE')
  ascii(12, 'fmt ')
  view.setUint32(16, 16, true)
  view.setUint16(20, 1, true) // PCM
  view.setUint16(22, 1, true) // one channel
  view.setUint32(24, sampleRate, true)
  view.setUint32(28, sampleRate * 2, true) // bytes a second
  view.setUint16(32, 2, true) // bytes a sample
  view.setUint16(34, 16, true)
  ascii(36, 'data')
  view.setUint32(40, dataBytes, true)
  return header
}

/** A WAV file of 16-bit `pcm` blocks taken at `sampleRate`, as a Blob, which copies the blocks once
 *  (ten minutes are 57.6 MB). Their bytes go in this machine's order: little-endian, as WAV's, on
 *  every phone and computer. */
export function wavFile(pcm: Int16Array<ArrayBuffer>[], sampleRate: number): Blob {
  const dataBytes = pcm.reduce((sum, block) => sum + block.byteLength, 0)
  return new Blob([wavHeader(dataBytes, sampleRate), ...pcm], { type: 'audio/wav' })
}
