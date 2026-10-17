import { useEffect, useState } from 'react'

/**
 * 지금 말하고 있는 상대들. 받은 RTP 패킷의 오디오 레벨(브라우저가 이미 계산한 값)만 읽어서
 * 원격 오디오를 따로 분석하지 않는다. 화면에 말하는 테두리를 그리는 용도라 200ms마다 한 번이면 충분하다.
 */
const LEVEL = 0.03
const HOLD_MS = 600
const POLL_MS = 200
const NONE: ReadonlySet<string> = new Set()
const supported =
  typeof RTCRtpReceiver !== 'undefined' && typeof RTCRtpReceiver.prototype.getSynchronizationSources === 'function'

export function useSpeakingPeers(receiversByPeer: () => [string, RTCRtpReceiver[]][], enabled: boolean): ReadonlySet<string> {
  const [speaking, setSpeaking] = useState<ReadonlySet<string>>(() => new Set())
  useEffect(() => {
    if (!enabled || !supported) return
    const lastLoud = new Map<string, number>()
    // 첫 확인은 항상 반영한다(꺼졌던 동안 남은 값을 지우도록).
    let current: string | null = null
    const id = setInterval(() => {
      const now = performance.now()
      for (const [peer, receivers] of receiversByPeer()) {
        for (const r of receivers) {
          const source = r.getSynchronizationSources()[0]
          if (source?.audioLevel === undefined || source.audioLevel < LEVEL) continue
          // 오래된 레벨(패킷이 끊긴 뒤 남은 값)은 무시한다. 표준은 timeOrigin 기준이지만 옛 구현은 epoch 기준이다.
          const age = source.timestamp > 1e12 ? Date.now() - source.timestamp : now - source.timestamp
          if (age < 500) lastLoud.set(peer, now)
        }
      }
      const active = [...lastLoud].filter(([, at]) => now - at < HOLD_MS).map(([peer]) => peer).sort()
      const key = active.join(',')
      // 바뀔 때만 다시 그린다.
      if (key !== current) {
        current = key
        setSpeaking(new Set(active))
      }
    }, POLL_MS)
    return () => clearInterval(id)
  }, [receiversByPeer, enabled])
  // 꺼져 있는 동안은 지난 값을 보이지 않는다. 다시 켜지면 첫 확인(200ms)에서 새로 채운다.
  return enabled && supported ? speaking : NONE
}
