// The recorder's audio thread, an AudioWorklet that recorder.ts loads from this file of its own (Safari
// loads no worklet from a blob): the microphone's samples, mixed to one channel by its node, sent to
// the page a tenth of a second at a time, where a message for each 128-frame quantum would be 375 a
// second. The part block when the take ends, under a tenth of a second, the tap on Stop, is not sent.
export {}

// The worklet's scope, which TypeScript's DOM library does not describe.
declare const sampleRate: number
declare abstract class AudioWorkletProcessor {
  readonly port: MessagePort
}
declare function registerProcessor(name: string, processor: new () => AudioWorkletProcessor): void

class RecorderProcessor extends AudioWorkletProcessor {
  #block = new Float32Array(Math.round(sampleRate / 10))
  #filled = 0

  process(inputs: Float32Array[][]): boolean {
    const samples = inputs[0]?.[0] ?? []
    for (const sample of samples) {
      this.#block[this.#filled++] = sample
      if (this.#filled === this.#block.length) {
        // The block is handed over, which leaves it empty here: the next is made first.
        const full = this.#block
        this.#block = new Float32Array(full.length)
        this.#filled = 0
        this.port.postMessage(full, [full.buffer])
      }
    }
    // Kept running: false would end it after this quantum.
    return true
  }
}

registerProcessor('quietmap-recorder', RecorderProcessor)
