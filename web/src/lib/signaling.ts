import type { ClientMsg } from '../protocol/ClientMsg'
import type { ServerMsg } from '../protocol/ServerMsg'

/** 서버 `/ws` 연결. 열리기 전에 보낸 메시지는 큐에 모았다가 보낸다. */
export class Signaling {
  private ws: WebSocket
  private queue: string[] = []
  private closedByUs = false
  private opened = false

  /** `onDisconnect(opened)`: opened가 false면 서버에 연결조차 못 한 것이다. */
  constructor(onMessage: (msg: ServerMsg) => void, onDisconnect: (opened: boolean) => void) {
    const proto = location.protocol === 'https:' ? 'wss:' : 'ws:'
    this.ws = new WebSocket(`${proto}//${location.host}/ws`)
    this.ws.onopen = () => {
      if (this.closedByUs) return
      this.opened = true
      for (const m of this.queue) this.ws.send(m)
      this.queue = []
    }
    this.ws.onmessage = (e) => {
      if (!this.closedByUs) onMessage(JSON.parse(e.data) as ServerMsg)
    }
    this.ws.onclose = () => {
      if (!this.closedByUs) onDisconnect(this.opened)
    }
  }

  /** 서버와 연결된 적이 있는지(지금 열려 있는지와는 다르다) */
  get hasOpened() {
    return this.opened
  }

  send(msg: ClientMsg) {
    if (this.closedByUs) return
    const raw = JSON.stringify(msg)
    if (this.ws.readyState === WebSocket.OPEN) this.ws.send(raw)
    else if (this.ws.readyState === WebSocket.CONNECTING) this.queue.push(raw)
  }

  close() {
    this.closedByUs = true
    this.queue = []
    this.ws.close()
  }
}
