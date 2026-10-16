import { FRAME_SAMPLES, SAMPLE_RATE } from './vad'

/**
 * 마이크 트랙 → 16kHz mono PCM16, 20ms 프레임. AudioWorklet 소스는 문자열로 번들에 넣어
 * Blob URL로 올린다(추가 네트워크 요청 없음). 컨텍스트는 기기 기본 샘플레이트로 열고 워클릿이
 * 평균 내림으로 16kHz로 줄인다(Firefox는 다른 샘플레이트 컨텍스트에 마이크를 연결하지 못한다).
 */
const WORKLET = `
class ParleyCapture extends AudioWorkletProcessor {
  constructor() {
    super()
    this.step = Math.max(1, sampleRate / ${SAMPLE_RATE})
    this.pos = 0; this.acc = 0; this.n = 0; this.i = 0
    this.out = new Int16Array(${FRAME_SAMPLES})
  }
  process(inputs) {
    const ch = inputs[0] && inputs[0][0]
    if (!ch) return true
    for (let k = 0; k < ch.length; k++) {
      this.acc += ch[k]; this.n++; this.pos++
      if (this.pos < this.step) continue
      this.pos -= this.step
      const v = Math.max(-1, Math.min(1, this.acc / this.n))
      this.acc = 0; this.n = 0
      this.out[this.i++] = v * 32767
      if (this.i === this.out.length) {
        this.port.postMessage(this.out, [this.out.buffer])
        this.out = new Int16Array(${FRAME_SAMPLES}); this.i = 0
      }
    }
    return true
  }
}
registerProcessor('parley-capture', ParleyCapture)
`

export type Capture = {
  suspend(): void
  resume(): void
  close(): void
}

export async function startCapture(track: MediaStreamTrack, onFrame: (frame: Int16Array) => void): Promise<Capture> {
  const ctx = new AudioContext({ latencyHint: 'interactive' })
  const url = URL.createObjectURL(new Blob([WORKLET], { type: 'text/javascript' }))
  try {
    await ctx.audioWorklet.addModule(url)
  } catch (err) {
    void ctx.close()
    throw err
  } finally {
    URL.revokeObjectURL(url)
  }
  const source = ctx.createMediaStreamSource(new MediaStream([track]))
  const node = new AudioWorkletNode(ctx, 'parley-capture', { numberOfInputs: 1, numberOfOutputs: 1, outputChannelCount: [1] })
  node.port.onmessage = (e: MessageEvent<Int16Array>) => onFrame(e.data)
  source.connect(node)
  // 출력은 쓰지 않지만(무음), 목적지에 연결해야 모든 브라우저가 노드를 처리한다.
  node.connect(ctx.destination)
  if (ctx.state === 'suspended') void ctx.resume()
  return {
    suspend: () => void ctx.suspend(),
    resume: () => void ctx.resume(),
    close: () => {
      node.port.onmessage = null
      source.disconnect()
      node.disconnect()
      void ctx.close()
    },
  }
}
