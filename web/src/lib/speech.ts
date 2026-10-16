import { startCapture, type Capture } from './capture'
import { LiveSession } from './live'
import { SttController, type Grant, type SegCounter, type SttStatus } from './stt'
import { Vad } from './vad'

export type Speech = {
  setMic: (on: boolean) => void
  setLang: (lang: string) => void
  close: () => void
}

type Options = {
  track: MediaStreamTrack
  lang: string
  mic: boolean
  seg: SegCounter
  requestToken: () => Promise<Grant>
  sendStt: (seg: number, text: string, final: boolean) => void
  onStatus: (status: SttStatus) => void
}

/** 마이크 캡처 + VAD + Gemini Live 세션 관리를 묶는다. 마이크가 꺼져 있으면 오디오 처리도 멈춘다. */
export function startSpeech(o: Options): Speech {
  const controller = new SttController(
    {
      requestToken: o.requestToken,
      openSession: (grant, events) => new LiveSession(grant.token, grant.model, events),
      sendStt: o.sendStt,
      onStatus: o.onStatus,
      now: () => performance.now(),
    },
    o.seg,
    o.lang,
  )
  const vad = new Vad()
  let mic = o.mic
  let capture: Capture | null = null
  let closed = false

  startCapture(o.track, (frame) => controller.vad(vad.push(frame))).then(
    (c) => {
      if (closed) return c.close()
      capture = c
      if (!mic) c.suspend()
    },
    () => {
      if (closed) return
      // AudioWorklet을 쓸 수 없는 브라우저
      controller.dispose()
      o.onStatus('unavailable')
    },
  )
  controller.setMic(mic)
  const timer = setInterval(() => controller.tick(), 1000)

  return {
    setMic(on) {
      if (on === mic) return
      mic = on
      if (on) capture?.resume()
      else {
        vad.reset()
        capture?.suspend()
      }
      controller.setMic(on)
    },
    setLang: (lang) => controller.setLang(lang),
    close() {
      closed = true
      clearInterval(timer)
      controller.dispose()
      capture?.close()
    },
  }
}
