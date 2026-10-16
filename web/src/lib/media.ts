export type MediaError = {
  kind: 'denied' | 'notFound' | 'busy' | 'unknown'
  device: 'camera' | 'both'
}
export type LocalMedia = { stream: MediaStream; error: MediaError | null }

/** 카메라+마이크를 요청하고, 실패하면 마이크만, 그래도 실패하면 빈 스트림으로 입장할 수 있게 한다. */
export async function acquireLocalMedia(): Promise<LocalMedia> {
  const audio = { echoCancellation: true, noiseSuppression: true, autoGainControl: true }
  try {
    const stream = await navigator.mediaDevices.getUserMedia({ audio, video: { width: 1280, height: 720 } })
    return { stream, error: null }
  } catch (videoErr) {
    try {
      const stream = await navigator.mediaDevices.getUserMedia({ audio })
      return { stream, error: { kind: classify(videoErr), device: 'camera' } }
    } catch (audioErr) {
      return { stream: new MediaStream(), error: { kind: classify(audioErr), device: 'both' } }
    }
  }
}

function classify(err: unknown): MediaError['kind'] {
  const name = err instanceof DOMException ? err.name : ''
  if (name === 'NotAllowedError') return 'denied'
  if (name === 'NotFoundError') return 'notFound'
  if (name === 'NotReadableError') return 'busy'
  return 'unknown'
}
