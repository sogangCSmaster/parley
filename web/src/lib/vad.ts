/**
 * 의존성 없는 음성 구간 감지(VAD). 20ms 프레임 에너지를 적응형 잡음 바닥과 비교한다.
 * 설계와 수치 근거는 docs/ARCHITECTURE.md의 "음성 구간 감지(VAD)와 침묵 비용".
 *
 * - 바닥의 `RATIO`배(그리고 `MIN_RMS`)를 `START_FRAMES` 연속 넘으면 말 시작
 * - 시작 시 직전 `PREROLL_MS`를 함께 내보낸다(첫 음절 잘림 방지)
 * - 조용해져도 `HANGOVER_MS`는 더 보낸 뒤 끝(문장 중간 쉼에서 끊기지 않게)
 */

export const SAMPLE_RATE = 16000
export const FRAME_MS = 20
export const FRAME_SAMPLES = (SAMPLE_RATE * FRAME_MS) / 1000

const PREROLL_MS = 300
const HANGOVER_MS = 600
const START_FRAMES = 2
/** 잡음 바닥 대비 약 10dB */
const RATIO = 3.2
/** 아주 조용한 방에서 숨소리·키보드에 반응하지 않도록 하는 절대 하한(약 -47dBFS) */
const MIN_RMS = 150
const FLOOR_INIT = 100
const FLOOR_MIN = 10
/**
 * 바닥은 조용해지면 빨리 내려가고, 시끄러워지면 천천히 따라 올라간다(조용할 때 약 2초, 말하는 중 약 10초).
 * 실제 말은 음절 사이마다 에너지가 떨어져 바닥이 다시 내려가므로 끊기지 않고, 끊김 없는 잡음(선풍기 등)은
 * 몇 초 안에 바닥이 되어 말로 보지 않는다.
 */
const FLOOR_DOWN = 0.2
const FLOOR_UP = 0.01
const FLOOR_UP_SPEAKING = 0.002
/** 처음 100ms는 바닥만 잰다(입장 직후 방 소음을 말로 보지 않게). */
const WARMUP_FRAMES = 5

export type VadResult =
  | { kind: 'start'; frames: Int16Array[] }
  | { kind: 'speech'; frames: Int16Array[] }
  | { kind: 'end'; frames: Int16Array[] }
  | null

export function rms(frame: Int16Array): number {
  let sum = 0
  for (let i = 0; i < frame.length; i++) sum += frame[i] * frame[i]
  return Math.sqrt(sum / Math.max(1, frame.length))
}

export class Vad {
  private floor = FLOOR_INIT
  private frames = 0
  private speaking = false
  private loud = 0
  private quiet = 0
  private preroll: Int16Array[] = []

  get active() {
    return this.speaking
  }

  /** 프레임 하나를 넣고, 말하는 중이면 내보낼 프레임을 돌려준다. */
  push(frame: Int16Array): VadResult {
    const energy = rms(frame)
    if (this.frames < WARMUP_FRAMES) {
      this.floor = this.frames++ === 0 ? energy : this.floor + (energy - this.floor) * 0.5
      this.floor = Math.max(FLOOR_MIN, this.floor)
      this.preroll.push(frame)
      return null
    }
    const isLoud = energy > Math.max(MIN_RMS, this.floor * RATIO)
    const up = this.speaking ? FLOOR_UP_SPEAKING : FLOOR_UP
    this.floor = Math.max(FLOOR_MIN, this.floor + (energy - this.floor) * (energy < this.floor ? FLOOR_DOWN : up))
    if (this.speaking) {
      this.quiet = isLoud ? 0 : this.quiet + 1
      if (this.quiet * FRAME_MS >= HANGOVER_MS) {
        this.speaking = false
        this.loud = 0
        return { kind: 'end', frames: [frame] }
      }
      return { kind: 'speech', frames: [frame] }
    }

    this.preroll.push(frame)
    if (this.preroll.length > PREROLL_MS / FRAME_MS) this.preroll.shift()
    this.loud = isLoud ? this.loud + 1 : 0
    if (this.loud >= START_FRAMES) {
      this.speaking = true
      this.quiet = 0
      const frames = this.preroll
      this.preroll = []
      return { kind: 'start', frames }
    }
    return null
  }

  /** 마이크를 끌 때: 말하는 중이었으면 true(호출한 쪽이 즉시 끝을 알린다). */
  reset(): boolean {
    const was = this.speaking
    this.speaking = false
    this.loud = 0
    this.quiet = 0
    this.preroll = []
    return was
  }
}
