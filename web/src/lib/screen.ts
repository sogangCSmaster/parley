import type { Mesh } from './mesh'

/** 캡처 요청이 늦게 끝나거나 송신자 일부가 실패해도 캡처 트랙의 소유권을 유지한다. */
export class ScreenShare {
  private track: MediaStreamTrack | null = null
  private busy = false
  private disposed = false
  private mesh: Pick<Mesh, 'setVideoTrack'>
  private camera: MediaStreamTrack | null
  private onTrack: (track: MediaStreamTrack | null) => void
  private onBusy: (busy: boolean) => void
  private onError: () => void

  constructor(
    mesh: Pick<Mesh, 'setVideoTrack'>,
    camera: MediaStreamTrack | null,
    onTrack: (track: MediaStreamTrack | null) => void,
    onBusy: (busy: boolean) => void,
    onError: () => void,
  ) {
    this.mesh = mesh
    this.camera = camera
    this.onTrack = onTrack
    this.onBusy = onBusy
    this.onError = onError
  }

  async toggle() {
    if (this.disposed || this.busy) return
    if (this.track) return this.stop()
    this.setBusy(true)
    let display: MediaStream
    try {
      display = await navigator.mediaDevices.getDisplayMedia({ video: true, audio: false })
    } catch (err) {
      if (!this.disposed && !(err instanceof DOMException && ['NotAllowedError', 'AbortError'].includes(err.name))) {
        this.onError()
      }
      this.setBusy(false)
      return
    }
    const track = display.getVideoTracks()[0]
    if (this.disposed || !track) {
      display.getTracks().forEach((t) => t.stop())
      this.setBusy(false)
      return
    }
    this.track = track
    try {
      await this.mesh.setVideoTrack(track)
      if (this.disposed) return
      // 브라우저에서 협상이 끝나기 전에 공유를 중지했을 수 있다.
      if (track.readyState === 'ended') {
        await this.stop()
        return
      }
      track.onended = () => { void this.stop() }
      this.onTrack(track)
    } catch {
      if (!this.disposed) {
        this.onError()
        await this.stop()
      }
    } finally {
      this.setBusy(false)
    }
  }

  private async stop() {
    const track = this.track
    if (!track || this.disposed) return
    this.track = null
    track.onended = null
    // 카메라 복원 성공 여부와 관계없이 화면 캡처부터 즉시 끝낸다.
    track.stop()
    this.onTrack(null)
    this.setBusy(true)
    try {
      await this.mesh.setVideoTrack(this.camera)
    } catch {
      if (!this.disposed) this.onError()
    } finally {
      this.setBusy(false)
    }
  }

  dispose() {
    this.disposed = true
    if (this.track) {
      this.track.onended = null
      this.track.stop()
      this.track = null
    }
  }

  private setBusy(busy: boolean) {
    this.busy = busy
    if (!this.disposed) this.onBusy(busy)
  }
}
