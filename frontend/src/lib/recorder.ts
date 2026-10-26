// The microphone recorded for the map's later levels (research of 2026-10-25): the browser's voice
// processing asked off (on an iPhone Safari then drops its gain control and noise suppression too;
// Android keeps its gain control), the samples kept raw, 16-bit, at the rate the page's audio runs,
// which the WAV states (wav.ts). Audio starts only in a tap, so the recording's context is made and
// resumed in the tap that asks for it; a browser that still holds it back gets one more tap. The
// length is the samples taken, never a timer, and the take lasts while the page is seen: hidden,
// interrupted or without its microphone, it ends with what it has.
import workletUrl from './recorder-worklet.ts?worker&url'
import { RECORDING_SAMPLES_MAX, pcm16, wavFile } from './wav.ts'

/** The voice processing asked off: requests, which a browser may not grant; what it did is kept. */
const MICROPHONE = { echoCancellation: false, noiseSuppression: false, autoGainControl: false, channelCount: 1 }
/** Raised when what a take keeps, or how, changes. */
const RECORDER_VERSION = 1

/** Asking for the microphone; audio held back by the browser until a tap; recording. */
export type RecordingPhase = 'asking' | 'held' | 'recording'
/** Why a take ended. */
type TakeEnd = 'stopped' | 'limit' | 'hidden' | 'interrupted' | 'microphone'

export interface Take {
  wav: Blob
  durationS: number
  /** How it was recorded, for levels computed later: the browser, the screen, the microphone, the
   *  processing asked and the settings the browser reports (a missing one unknown), why it ended. */
  details: Record<string, unknown>
}

export interface Recorder {
  stop(): void
  /** Starts the audio the browser held back; called in a tap. */
  resume(): void
}

/** What the visitor is told when the microphone did not start. */
function problemOf(error: unknown): string {
  const name = error instanceof DOMException ? error.name : ''
  if (name === 'NotAllowedError') return "The microphone is not allowed for the map: allow it in the browser's settings."
  if (name === 'NotFoundError') return 'No microphone was found.'
  if (name === 'NotReadableError') return 'The microphone is in use by another app.'
  return 'This browser cannot record here.'
}

/** The phone's model and system version where the browser tells them (Chrome on Android); an
 *  iPhone never does. */
async function device(): Promise<Record<string, unknown> | null> {
  const data = (navigator as Navigator & { userAgentData?: { getHighEntropyValues(hints: string[]): Promise<Record<string, unknown>> } }).userAgentData
  return data ? data.getHighEntropyValues(['model', 'platformVersion']).catch(() => null) : null
}

/** Starts a recording, in the tap that asked for it. `onPhase` hears its phase and each second taken;
 *  `onEnd` gets the take, or null and what went wrong when nothing was taken. */
export function startRecording(
  onPhase: (phase: RecordingPhase, seconds: number) => void,
  onEnd: (take: Take | null, problem?: string) => void,
): Recorder {
  const context = new AudioContext()
  void context.resume()
  const rate = context.sampleRate
  const blocks: Int16Array<ArrayBuffer>[] = []
  let taken = 0
  let phase: RecordingPhase = 'asking'
  let stream: MediaStream | null = null
  // The graph's nodes, held here: a browser may collect a source no one holds, and its sound stops.
  let nodes: AudioNode[] = []
  let wakeLock: WakeLockSentinel | null = null
  let ended = false
  const details: Record<string, unknown> = {
    recorder: RECORDER_VERSION,
    startedAt: new Date().toISOString(),
    userAgent: navigator.userAgent,
    screen: { width: screen.width, height: screen.height, pixelRatio: devicePixelRatio },
    sampleRate: rate,
    asked: MICROPHONE,
  }
  void device().then(values => { details.device = values })
  // The screen kept on, asked in the tap (Safari may refuse a lock asked later): a phone that locks
  // itself would end the take.
  void navigator.wakeLock?.request('screen').then(lock => {
    if (ended) void lock.release()
    else wakeLock = lock
  }).catch(() => {})
  const show = () => onPhase(phase, Math.floor(taken / rate))

  const hidden = () => {
    if (document.visibilityState === 'hidden') end('hidden')
  }
  const left = () => end('hidden')
  document.addEventListener('visibilitychange', hidden)
  window.addEventListener('pagehide', left)

  function end(why: TakeEnd | null, problem?: string) {
    if (ended) return
    ended = true
    document.removeEventListener('visibilitychange', hidden)
    window.removeEventListener('pagehide', left)
    stream?.getTracks().forEach(track => track.stop())
    void wakeLock?.release().catch(() => {})
    void context.close().catch(() => {})
    if (why === null || taken === 0) return onEnd(null, problem)
    const wav = wavFile(blocks, rate)
    blocks.length = 0
    onEnd({ wav, durationS: taken / rate, details: { ...details, endedAt: new Date().toISOString(), ended: why } })
  }

  function begin() {
    if (ended) return
    phase = 'recording'
    show()
  }

  // A take runs until the visitor stops it, or until it holds what the server keeps (ten minutes at
  // 48 kHz), and is kept then.
  function keep(samples: Float32Array) {
    if (ended) return
    const kept = samples.subarray(0, RECORDING_SAMPLES_MAX - taken)
    const second = Math.floor(taken / rate)
    blocks.push(pcm16(kept))
    taken += kept.length
    if (taken >= RECORDING_SAMPLES_MAX) end('limit')
    else if (Math.floor(taken / rate) !== second) show()
  }

  void (async () => {
    try {
      stream = await navigator.mediaDevices.getUserMedia({ audio: MICROPHONE })
      if (ended) return stream.getTracks().forEach(track => track.stop())
      const [track] = stream.getAudioTracks()
      // The device's ids are random per site and say nothing of the sound.
      const settings: Record<string, unknown> = { ...track.getSettings() }
      delete settings.deviceId
      delete settings.groupId
      Object.assign(details, { microphone: track.label, settings })
      // A microphone paused (another app, Safari's own control) gives silence, not quiet: the take
      // ends with what it has.
      if (track.muted) return end(null, 'The microphone is paused or in use by another app.')
      for (const lost of ['ended', 'mute']) track.addEventListener(lost, () => end('microphone'))
      await context.audioWorklet.addModule(workletUrl)
      if (ended) return
      const node = new AudioWorkletNode(context, 'quietmap-recorder', { channelCount: 1, channelCountMode: 'explicit', channelInterpretation: 'speakers' })
      node.port.onmessage = (event: MessageEvent<Float32Array>) => keep(event.data)
      // Safari runs only nodes that reach the speakers; this one reaches them silent.
      const silence = context.createGain()
      silence.gain.value = 0
      nodes = [context.createMediaStreamSource(stream), node, silence]
      nodes.reduce((from, to) => from.connect(to)).connect(context.destination)
      context.addEventListener('statechange', () => {
        if (ended) return
        if (context.state === 'running' && phase === 'held') begin()
        else if (context.state !== 'running' && phase === 'recording') end('interrupted')
      })
      // A context the browser does not let start leaves resume() waiting for a tap: after a second,
      // the visitor is asked for it.
      await Promise.race([context.resume(), new Promise(resolve => setTimeout(resolve, 1000))]).catch(() => {})
      if (ended) return
      if (context.state === 'running') begin()
      else {
        phase = 'held'
        show()
      }
    } catch (error) {
      end(null, problemOf(error))
    }
  })()
  show()

  return {
    stop: () => end('stopped'),
    resume: () => void context.resume(),
  }
}
