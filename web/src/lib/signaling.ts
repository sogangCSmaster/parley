import type { ClientMsg } from '../protocol/ClientMsg'
import type { ServerMsg } from '../protocol/ServerMsg'

/** 서버 `/ws` 연결. 열리기 전에 보낸 메시지는 큐에 모았다가 보낸다. */
export class Signaling {
  private ws: WebSocket
  private queue: string[] = []
  private closedByUs = false

  constructor(onMessage: (msg: ServerMsg) => void, onDisconnect: () => void) {
    const proto = location.protocol === 'https:' ? 'wss:' : 'ws:'
    this.ws = new WebSocket(`${proto}//${location.host}/ws`)
    this.ws.onopen = () => {
      for (const m of this.queue) this.ws.send(m)
      this.queue = []
    }
    this.ws.onmessage = (e) => onMessage(JSON.parse(e.data) as ServerMsg)
    this.ws.onclose = () => {
      if (!this.closedByUs) onDisconnect()
    }
  }

  send(msg: ClientMsg) {
    const raw = JSON.stringify(msg)
    if (this.ws.readyState === WebSocket.OPEN) this.ws.send(raw)
    else this.queue.push(raw)
  }

  close() {
    this.closedByUs = true
    this.ws.close()
  }
}
