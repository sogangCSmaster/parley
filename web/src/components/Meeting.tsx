import { useEffect, useState } from 'react'
import type { MediaState } from '../protocol/MediaState'
import type { RoomSettings } from '../protocol/RoomSettings'
import { fetchRoom } from '../lib/api'
import { useI18n } from '../lib/i18n'
import { acquireLocalMedia, type LocalMedia } from '../lib/media'
import { loadPrefs, savePrefs, type Prefs } from '../lib/prefs'
import { navigate } from '../lib/router'
import { Header } from './Header'
import { Lobby } from './Lobby'
import { Room } from './Room'

type Stage = 'lobby' | 'room' | 'left'

export function Meeting({ code, onRejoin }: { code: string; onRejoin: () => void }) {
  const { t } = useI18n()
  const [local, setLocal] = useState<LocalMedia | null>(null)
  const [prefs, setPrefs] = useState<Prefs>(loadPrefs)
  const [media, setMedia] = useState<MediaState>({ mic: true, cam: true, screen: false })
  const [stage, setStage] = useState<Stage>('lobby')
  // undefined: 확인 중, null: 없는 회의
  const [room, setRoom] = useState<RoomSettings | null | undefined>(undefined)

  // 회의 정보는 카메라 준비와 병렬로 가져온다(로비 표시 시간을 늘리지 않음).
  useEffect(() => {
    let cancelled = false
    fetchRoom(code)
      .then((r) => !cancelled && setRoom(r))
      // 일시적 오류면 입장 시도에서 다시 판정되므로 번역 켜짐으로 가정하고 진행한다.
      .catch(() => !cancelled && setRoom({ translation: true }))
    return () => {
      cancelled = true
    }
  }, [code])

  const captureNeeded = room !== null && stage !== 'left'
  useEffect(() => {
    if (!captureNeeded) return
    let cancelled = false
    let acquired: MediaStream | null = null
    acquireLocalMedia().then((result) => {
      if (cancelled) {
        result.stream.getTracks().forEach((t) => t.stop())
        return
      }
      acquired = result.stream
      setLocal(result)
    })
    return () => {
      cancelled = true
      acquired?.getTracks().forEach((t) => t.stop())
    }
  }, [captureNeeded])

  // 트랙의 enabled는 미디어 상태를 따라간다(로비에서 끈 채로 입장하는 경우 포함).
  useEffect(() => {
    local?.stream.getAudioTracks().forEach((t) => (t.enabled = media.mic))
    local?.stream.getVideoTracks().forEach((t) => (t.enabled = media.cam))
  }, [local, media.mic, media.cam])

  function updatePrefs(next: Prefs) {
    setPrefs(next)
    savePrefs(next)
  }

  function leave() {
    local?.stream.getTracks().forEach((t) => t.stop())
    setStage('left')
  }

  if (room === null) {
    return <Notice title={t('room.cannotJoin')} body={t('room.notFound')} onHome={() => navigate('/')} />
  }

  if (stage === 'left') {
    return (
      <div className="shell">
        <Header />
        <main className="left">
          <h1>{t('room.left')}</h1>
          <div className="row">
            <button className="btn btn-outline" onClick={onRejoin}>
              {t('room.rejoin')}
            </button>
            <button className="btn btn-primary" onClick={() => navigate('/')}>
              {t('room.home')}
            </button>
          </div>
        </main>
      </div>
    )
  }

  if (stage === 'lobby' || !local) {
    return (
      <Lobby
        code={code}
        local={local}
        room={room}
        prefs={prefs}
        media={media}
        onPrefsChange={updatePrefs}
        onMediaChange={setMedia}
        onJoin={() => setStage('room')}
      />
    )
  }

  return (
    <Room
      code={code}
      local={local.stream}
      prefs={prefs}
      media={media}
      onPrefsChange={updatePrefs}
      onMediaChange={setMedia}
      onLeave={leave}
      onRejoin={onRejoin}
    />
  )
}

export function Notice({ title, body, onHome }: { title: string; body: string; onHome: () => void }) {
  const { t } = useI18n()
  return (
    <div className="shell">
      <Header />
      <main className="left">
        <h1>{title}</h1>
        <p className="lead">{body}</p>
        <button className="btn btn-primary" onClick={onHome}>
          {t('room.home')}
        </button>
      </main>
    </div>
  )
}
