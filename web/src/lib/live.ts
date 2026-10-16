/**
 * Gemini Live 음성 인식 세션 하나(WebSocket 하나). 서버가 발급한 1회용 토큰으로 연결하며,
 * 모델·언어·수동 활동 모드는 토큰에 잠겨 있다. 연결되기 전에 보낸 메시지는 모았다가 보낸다.
 */

const URL_BASE =
  'wss://generativelanguage.googleapis.com/ws/google.ai.generativelanguage.v1alpha.GenerativeService.BidiGenerateContentConstrained'
/** 연결 중 쌓아 두는 메시지 상한(오디오 100ms 조각 기준 약 15초) */
const MAX_QUEUE = 150
const SETUP_TIMEOUT_MS = 10_000

export type LiveEvents = {
  onReady: () => void
  onInterim: (text: string) => void
  onFinal: (text: string) => void
  /** 서버가 곧 세션을 끊겠다고 알림. 다음 안전한 시점에 교체한다. */
  onGoAway: () => void
  onClose: () => void
}

export interface Session {
  activityStart(): void
  audio(pcm: Int16Array): void
  activityEnd(): void
  close(): void
}

export class LiveSession implements Session {
  private ws: WebSocket
  private queue: string[] = []
  private ready = false
  private closed = false
  private setupTimer: ReturnType<typeof setTimeout>
  private decoder = new TextDecoder()
  private events: LiveEvents

  constructor(token: string, model: string, events: LiveEvents) {
    this.events = events
    this.ws = new WebSocket(`${URL_BASE}?access_token=${encodeURIComponent(token)}`)
    // Live API는 JSON을 바이너리 프레임으로 보낸다.
    this.ws.binaryType = 'arraybuffer'
    this.ws.onopen = () => this.ws.send(JSON.stringify({ setup: { model } }))
    this.ws.onmessage = (e) => this.onMessage(e.data)
    this.ws.onclose = () => this.finish()
    this.ws.onerror = () => this.finish()
    this.setupTimer = setTimeout(() => this.close(), SETUP_TIMEOUT_MS)
  }

  activityStart() {
    this.send({ realtimeInput: { activityStart: {} } })
  }

  audio(pcm: Int16Array) {
    this.send({ realtimeInput: { audio: { data: base64(pcm), mimeType: 'audio/pcm;rate=16000' } } })
  }

  activityEnd() {
    this.send({ realtimeInput: { activityEnd: {} } })
  }

  close() {
    if (this.closed) return
    this.ws.close()
    this.finish()
  }

  private send(msg: unknown) {
    if (this.closed) return
    const raw = JSON.stringify(msg)
    if (this.ready) this.ws.send(raw)
    else if (this.queue.length < MAX_QUEUE) this.queue.push(raw)
  }

  private onMessage(data: unknown) {
    if (this.closed) return
    let msg: LiveMessage
    try {
      msg = JSON.parse(typeof data === 'string' ? data : this.decoder.decode(data as ArrayBuffer))
    } catch {
      return
    }
    if (msg.setupComplete && !this.ready) {
      clearTimeout(this.setupTimer)
      this.ready = true
      for (const m of this.queue) this.ws.send(m)
      this.queue = []
      this.events.onReady()
    }
    const content = msg.serverContent
    const interim = content?.interimInputTranscription?.text
    if (typeof interim === 'string') this.events.onInterim(interim)
    const final = content?.inputTranscription?.text
    if (typeof final === 'string') this.events.onFinal(final)
    if (msg.goAway) this.events.onGoAway()
  }

  private finish() {
    if (this.closed) return
    this.closed = true
    clearTimeout(this.setupTimer)
    this.queue = []
    this.events.onClose()
  }
}

type LiveMessage = {
  setupComplete?: object
  goAway?: object
  serverContent?: {
    interimInputTranscription?: { text?: unknown }
    inputTranscription?: { text?: unknown }
  }
}

export function base64(pcm: Int16Array): string {
  const bytes = new Uint8Array(pcm.buffer, pcm.byteOffset, pcm.byteLength)
  let s = ''
  for (let i = 0; i < bytes.length; i += 0x8000) s += String.fromCharCode(...bytes.subarray(i, i + 0x8000))
  return btoa(s)
}
