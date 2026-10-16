import { startCapture, type Capture } from './capture'
import { FarEnd } from './farend'
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
  /** 상대들의 오디오 수신기(스피커로 나오는 소리) */
  farEnd: () => RTCRtpReceiver[]
  /** 상대 목소리가 마이크로 계속 들어올 때(1분에 3번 이상 막힘). 여러 번 불릴 수 있다. */
  onEcho: () => void
}

/** 올린 문턱에 막힌 소리가 1분 안에 이만큼 생기면 헤드폰을 권한다(가끔 작은 맞장구가 막히는 건 정상). */
const ECHO_HINT_AFTER = 3
const ECHO_HINT_WINDOW_MS = 60_000

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
  const farEnd = new FarEnd(o.farEnd)
  let blockedSeen = 0
  const blockedAt: number[] = []
  let mic = o.mic
  let capture: Capture | null = null
  let closed = false

  const onFrame = (frame: Int16Array) => {
    const now = performance.now()
    controller.vad(vad.push(frame, farEnd.active(now)))
    if (vad.echoBlocked === blockedSeen) return
    blockedSeen = vad.echoBlocked
    blockedAt.push(now)
    while (blockedAt[0] < now - ECHO_HINT_WINDOW_MS) blockedAt.shift()
    if (blockedAt.length >= ECHO_HINT_AFTER) {
      blockedAt.length = 0
      o.onEcho()
    }
  }

  startCapture(o.track, onFrame).then(
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
