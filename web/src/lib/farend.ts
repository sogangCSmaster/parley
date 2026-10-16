/**
 * 상대 음성이 지금 스피커로 나오고 있는지. 받은 RTP 패킷의 오디오 레벨(브라우저가 이미 계산한 값,
 * `getSynchronizationSources().audioLevel`)만 읽으므로 원격 오디오를 따로 분석하지 않는다.
 */

/** 0..1 선형(1 = 0dBov). 0.02 ≈ -34dBov: 말소리는 넘고, 잡음 억제를 거친 배경 소음은 넘지 않는다. */
const ACTIVE_LEVEL = 0.02
/** 받은 시점보다 늦게 재생되고(지터 버퍼), 방에서 울리므로 상대 말이 끝난 뒤에도 잠시 유지한다. */
const HOLD_MS = 400
/** 패킷이 끊기면(망 정지·퇴장 직전) 마지막 레벨이 그대로 남는다. 그 값은 이 시간 뒤 버린다. */
const STALE_MS = 300
/** 수신기 조회는 메인 스레드에서 동기로 도는 호출이라 프레임(20ms)마다 하지 않는다. 유지 시간에 비해 짧다. */
const POLL_MS = 100
/** 말은 쉼마다 레벨이 떨어진다. 이만큼 한 번도 조용해지지 않은 상대는 말이 아니라 잡음으로 보고 무시한다. */
const NOISE_MS = 10_000

type Seen = { timestamp: number; at: number; quietAt: number }

export class FarEnd {
  private lastActive = -Infinity
  private polledAt = -Infinity
  private seen = new WeakMap<RTCRtpReceiver, Seen>()
  private receivers: () => RTCRtpReceiver[]
  // 이 API가 없는 브라우저에서는 상대 음성을 모르는 것으로 두고 VAD는 평소대로 동작한다.
  private supported =
    typeof RTCRtpReceiver !== 'undefined' && typeof RTCRtpReceiver.prototype.getSynchronizationSources === 'function'

  constructor(receivers: () => RTCRtpReceiver[]) {
    this.receivers = receivers
  }

  active(now: number): boolean {
    if (this.supported && now - this.polledAt >= POLL_MS) {
      this.polledAt = now
      this.poll(now)
    }
    return now - this.lastActive < HOLD_MS
  }

  private poll(now: number) {
    for (const receiver of this.receivers()) {
      const source = receiver.getSynchronizationSources()[0]
      if (source?.audioLevel === undefined) continue
      let seen = this.seen.get(receiver)
      if (!seen) {
        seen = { timestamp: source.timestamp, at: now, quietAt: now }
        this.seen.set(receiver, seen)
      } else if (seen.timestamp !== source.timestamp) {
        seen.timestamp = source.timestamp
        seen.at = now
      } else if (now - seen.at > STALE_MS) continue
      if (source.audioLevel < ACTIVE_LEVEL) seen.quietAt = now
      else if (now - seen.quietAt < NOISE_MS) this.lastActive = now
    }
  }
}
