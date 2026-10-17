import { useEffect, useState } from 'react'
import type { MediaState } from '../protocol/MediaState'
import type { RoomSettings } from '../protocol/RoomSettings'
import { fetchRoom } from '../lib/api'
import { downloadText, transcriptFileName } from '../lib/transcript'
import { useI18n } from '../lib/i18n'
import { acquireLocalMedia, type LocalMedia } from '../lib/media'
import { loadPrefs, savePrefs, type Prefs } from '../lib/prefs'
import { navigate } from '../lib/router'
import { MeetingGonePage, MicBlockedPage } from './ErrorPages'
import { Lobby } from './Lobby'
import { Icon } from './Icon'
import { Room, type LeaveSummary } from './Room'
import { StatusPage } from './StatusPage'

type Stage = 'lobby' | 'room' | 'left'

export function Meeting({ code, onRejoin }: { code: string; onRejoin: () => void }) {
  const [local, setLocal] = useState<LocalMedia | null>(null)
  const [prefs, setPrefs] = useState<Prefs>(loadPrefs)
  const [media, setMedia] = useState<MediaState>({ mic: true, cam: true, screen: false })
  const [stage, setStage] = useState<Stage>('lobby')
  // 나간 뒤에도 자막을 저장할 수 있게 회의실에서 넘겨받는다(메모리에만, 다시 참여하면 사라진다).
  const [summary, setSummary] = useState<(LeaveSummary & { leftAt: number }) | null>(null)
  // 마이크 권한이 막힌 채로 듣기만 하고 들어가기로 했는지
  const [listenOnly, setListenOnly] = useState(false)
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

  // 입장에 끝내 실패하면(거절 등) 장치를 놓는다. "다시 시도"는 Meeting을 새로 마운트해 다시 잡는다.
  const [gaveUp, setGaveUp] = useState(false)
  const captureNeeded = room !== null && stage !== 'left' && !gaveUp
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

  function leave(next: LeaveSummary) {
    local?.stream.getTracks().forEach((t) => t.stop())
    setSummary({ ...next, leftAt: Date.now() })
    setStage('left')
  }

  if (room === null) return <MeetingGonePage />

  if (stage === 'left') {
    return <EndedPage code={code} summary={summary} onRejoin={onRejoin} />
  }

  const micBlocked = local?.error?.device === 'both' && local.error.kind === 'denied'
  if (stage === 'lobby' && micBlocked && !listenOnly) {
    return <MicBlockedPage onRetry={onRejoin} onListenOnly={() => setListenOnly(true)} />
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
      onGiveUp={() => setGaveUp(true)}
    />
  )
}

export function Notice({ title, body, onHome }: { title: string; body: string; onHome: () => void }) {
  const { t } = useI18n()
  return (
    <StatusPage
      title={title}
      body={body}
      actions={
        <button className="btn btn-primary" onClick={onHome}>
          {t('room.home')}
        </button>
      }
    />
  )
}

/** 나간 화면: 회의 시간·번역한 문장 수, 대화 기록 저장, 다시 참여·홈으로. */
function EndedPage({
  code,
  summary,
  onRejoin,
}: {
  code: string
  summary: (LeaveSummary & { leftAt: number }) | null
  onRejoin: () => void
}) {
  const { t } = useI18n()
  const exportTranscript = summary?.exportTranscript ?? null
  const minutes = summary?.joinedAt != null ? Math.max(1, Math.round((summary.leftAt - summary.joinedAt) / 60_000)) : null
  const duration =
    minutes == null
      ? null
      : minutes < 60
        ? t('ended.minutes', { n: String(minutes) })
        : t('ended.hoursMinutes', { h: String(Math.floor(minutes / 60)), m: String(minutes % 60) })
  return (
    <StatusPage
      eyebrow={t('lobby.meeting', { code })}
      title={t('room.left')}
      actions={
        <>
          <button className="btn btn-outline" onClick={onRejoin}>
            {t('room.rejoin')}
          </button>
          <button className="btn btn-primary" onClick={() => navigate('/')}>
            {t('room.home')}
          </button>
        </>
      }
    >
      {duration && (
        <dl className="ended-stats">
          <div className="status-card">
            <dt>{t('ended.duration')}</dt>
            <dd>{duration}</dd>
          </div>
          <div className="status-card">
            <dt>{t('ended.sentences')}</dt>
            <dd>{summary?.sentences ?? 0}</dd>
          </div>
        </dl>
      )}
      {exportTranscript && (
        <div className="status-card ended-save">
          <span className="ended-save-icon" aria-hidden="true">
            <Icon name="download" />
          </span>
          <div>
            <h2>{t('ended.saveTitle')}</h2>
            <p>{t('ended.saveBody')}</p>
          </div>
          <button className="btn btn-ink" onClick={() => downloadText(transcriptFileName(code, new Date()), exportTranscript())}>
            {t('ended.save')}
          </button>
        </div>
      )}
    </StatusPage>
  )
}
