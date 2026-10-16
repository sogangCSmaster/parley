import { useEffect, useMemo, useRef, useState } from 'react'
import type { Knock } from '../protocol/Knock'
import type { MediaState } from '../protocol/MediaState'
import type { Peer } from '../protocol/Peer'
import type { RoomSettings } from '../protocol/RoomSettings'
import type { ServerMsg } from '../protocol/ServerMsg'
import type { JsonValue } from '../protocol/serde_json/JsonValue'
import { applyCaption, type Caption } from '../lib/captions'
import { useI18n, type Locale, type MessageKey } from '../lib/i18n'
import { Mesh, type SignalData } from '../lib/mesh'
import type { Prefs } from '../lib/prefs'
import { Signaling } from '../lib/signaling'
import { ScreenShare } from '../lib/screen'
import { CaptionPanel } from './CaptionPanel'
import { Header } from './Header'
import { KnockPrompts } from './KnockPrompts'
import { Icon } from './Icon'
import { VideoTile } from './VideoTile'

type Props = {
  code: string
  local: MediaStream
  prefs: Prefs
  media: MediaState
  onPrefsChange: (p: Prefs) => void
  onMediaChange: (m: MediaState) => void
  onLeave: () => void
  onRejoin: () => void
}

type Status =
  | { kind: 'connecting' }
  | { kind: 'waiting' }
  | { kind: 'joined'; selfId: string }
  | { kind: 'error'; message: MessageKey }

// 입장 전 오류는 화면 전체를 바꾸고, 입장 후 오류(예: 가득 찬 방에 수락)는 토스트로만 알린다.
const FATAL_ERRORS: Partial<Record<string, MessageKey>> = {
  room_full: 'room.full',
  room_not_found: 'room.notFound',
  too_many_waiting: 'room.queueFull',
}

export function Room({ code, local, prefs, media, onPrefsChange, onMediaChange, onLeave, onRejoin }: Props) {
  const { t, locale } = useI18n()
  const [status, setStatus] = useState<Status>({ kind: 'connecting' })
  const [peers, setPeers] = useState<Record<string, Peer>>({})
  const [streams, setStreams] = useState<Record<string, MediaStream>>({})
  const [screenTrack, setScreenTrack] = useState<MediaStreamTrack | null>(null)
  const [screenBusy, setScreenBusy] = useState(false)
  const [panelOpen, setPanelOpen] = useState(true)
  const [disconnected, setDisconnected] = useState(false)
  const [toast, setToast] = useState<{ key: MessageKey; params?: Record<string, string> } | null>(null)
  const [isHost, setIsHost] = useState(false)
  const [settings, setSettings] = useState<RoomSettings>({ translation: true })
  const [knocks, setKnocks] = useState<Knock[]>([])
  const [captions, setCaptions] = useState<Caption[]>([])
  // 나간 참가자의 지난 자막에도 이름이 보이도록 이름은 지우지 않고 모은다.
  const [names, setNames] = useState<Record<string, string>>({})

  const sigRef = useRef<Signaling | null>(null)
  const meshRef = useRef<Mesh | null>(null)
  const screenRef = useRef<ScreenShare | null>(null)
  const onMediaChangeRef = useRef(onMediaChange)
  useEffect(() => { onMediaChangeRef.current = onMediaChange }, [onMediaChange])
  // 입장 메시지는 마운트 시점 값으로 한 번만 보낸다. 이후 변경은 lang/media 메시지로 보낸다.
  const joinRef = useRef({ name: prefs.name.trim(), lang: prefs.lang, partnerLang: prefs.partnerLang, media })
  // 브라우저의 "공유 중지"처럼 나중에 호출되는 콜백이 최신 미디어 상태를 보도록 한다.
  const mediaRef = useRef(media)
  useEffect(() => {
    mediaRef.current = media
  }, [media])

  useEffect(() => {
    let disposed = false

    const onMessage = (msg: ServerMsg) => {
      if (disposed) return
      switch (msg.t) {
        case 'welcome': {
          const iceServers = iceRef.current
          const mesh = new Mesh(
            msg.you,
            local,
            iceServers,
            (to, data) => sig.send({ t: 'signal', to, data: data as unknown as JsonValue }),
            (id, stream) => setStreams((s) => ({ ...s, [id]: stream })),
          )
          meshRef.current = mesh
          screenRef.current = new ScreenShare(
            mesh,
            local.getVideoTracks()[0] ?? null,
            (track) => {
              setScreenTrack(track)
              const next = { ...mediaRef.current, screen: !!track }
              mediaRef.current = next
              onMediaChangeRef.current(next)
              sig.send({ t: 'media', media: next })
            },
            setScreenBusy,
            () => setToast({ key: 'room.screenFailed' }),
          )
          joinedRef.current = true
          setStatus({ kind: 'joined', selfId: msg.you })
          setIsHost(msg.host)
          setSettings(msg.settings)
          setKnocks(msg.knocks)
          setPeers(Object.fromEntries(msg.peers.map((p) => [p.id, p])))
          setNames((n) => ({ ...n, ...Object.fromEntries(msg.peers.map((p) => [p.id, p.name])) }))
          msg.peers.forEach((p) => mesh.connect(p.id))
          break
        }
        case 'peer_joined':
        case 'peer_updated':
          setPeers((ps) => ({ ...ps, [msg.peer.id]: msg.peer }))
          setNames((n) => ({ ...n, [msg.peer.id]: msg.peer.name }))
          break
        case 'peer_left':
          meshRef.current?.remove(msg.id)
          setPeers((ps) => without(ps, msg.id))
          setStreams((s) => without(s, msg.id))
          break
        case 'signal':
          meshRef.current?.handleSignal(msg.from, msg.data as unknown as SignalData)
          break
        case 'waiting':
          setStatus({ kind: 'waiting' })
          break
        case 'denied':
          setStatus({ kind: 'error', message: 'room.denied' })
          break
        case 'knock':
          setKnocks((ks) => [...ks.filter((k) => k.id !== msg.knock.id), msg.knock])
          break
        case 'knock_resolved':
          setKnocks((ks) => ks.filter((k) => k.id !== msg.id))
          break
        case 'settings':
          setSettings(msg.settings)
          break
        case 'caption':
          setCaptions((list) => applyCaption(list, msg, Date.now()))
          break
        case 'notice':
          if (msg.code === 'translation_failed') setToast({ key: 'captions.failed' })
          break
        case 'error': {
          const fatal = FATAL_ERRORS[msg.code]
          if (!joinedRef.current) setStatus({ kind: 'error', message: fatal ?? 'room.cannotJoin' })
          else if (msg.code === 'room_full') setToast({ key: 'room.full' })
          else console.warn('server error', msg.code, msg.message)
          break
        }
      }
    }

    const iceRef = { current: [] as RTCIceServer[] }
    const joinedRef = { current: false }
    const sig = new Signaling(onMessage, () => {
      if (disposed) return
      screenRef.current?.dispose()
      screenRef.current = null
      setScreenTrack(null)
      setScreenBusy(false)
      meshRef.current?.close()
      meshRef.current = null
      setStreams({})
      if (joinedRef.current) setDisconnected(true)
      else setStatus({ kind: 'error', message: 'room.disconnected' })
    })
    sigRef.current = sig

    fetch('/api/config')
      .then((r) => (r.ok ? r.json() : Promise.reject(new Error(`status ${r.status}`))))
      .then((cfg: { iceServers: RTCIceServer[] }) => {
        if (disposed) return
        iceRef.current = cfg.iceServers
        const j = joinRef.current
        sig.send({ t: 'join', room: code, name: j.name, lang: j.lang, partner_lang: j.partnerLang, media: j.media })
      })
      .catch(() => {
        if (!disposed) setStatus({ kind: 'error', message: 'room.serverDown' })
      })

    return () => {
      disposed = true
      sig.close()
      screenRef.current?.dispose()
      screenRef.current = null
      meshRef.current?.close()
      meshRef.current = null
      sigRef.current = null
    }
  }, [code, local])

  useEffect(() => {
    if (!toast) return
    const id = setTimeout(() => setToast(null), 3000)
    return () => clearTimeout(id)
  }, [toast])

  const joined = status.kind === 'joined'
  const showPanel = joined && panelOpen && (settings.translation || isHost)
  const selfStream = useMemo(() => (screenTrack ? new MediaStream([screenTrack]) : local), [screenTrack, local])

  function changeMedia(next: MediaState) {
    mediaRef.current = next
    onMediaChange(next)
    if (joined) sigRef.current?.send({ t: 'media', media: next })
  }

  function changePrefs(next: Prefs) {
    onPrefsChange(next)
    if (joined) sigRef.current?.send({ t: 'lang', lang: next.lang, partner_lang: next.partnerLang })
  }

  async function copyLink() {
    try {
      await navigator.clipboard.writeText(location.href)
      setToast({ key: 'room.linkCopied' })
    } catch {
      setToast({ key: 'room.linkFallback', params: { url: location.href } })
    }
  }

  function admit(id: string) {
    sigRef.current?.send({ t: 'admit', id })
  }

  function deny(id: string) {
    sigRef.current?.send({ t: 'deny', id })
  }

  function changeTranslation(translation: boolean) {
    sigRef.current?.send({ t: 'settings', settings: { translation } })
  }

  if (status.kind === 'waiting') {
    return (
      <div className="shell">
        <Header />
        <main className="left" aria-live="polite">
          <span className="spinner" aria-hidden="true" />
          <h1>{t('room.waitingTitle')}</h1>
          <p className="lead">{t('room.waitingBody')}</p>
          <button className="btn btn-outline" onClick={onLeave}>
            {t('room.cancel')}
          </button>
        </main>
      </div>
    )
  }

  if (status.kind === 'error') {
    return (
      <div className="shell">
        <Header />
        <main className="left">
          <h1>{t('room.cannotJoin')}</h1>
          <p className="lead">{t(status.message)}</p>
          <div className="row">
            <button className="btn btn-outline" onClick={onRejoin}>
              {t('room.retry')}
            </button>
            <button className="btn btn-primary" onClick={onLeave}>
              {t('room.leave')}
            </button>
          </div>
        </main>
      </div>
    )
  }

  const hasCam = local.getVideoTracks().length > 0
  const hasMic = local.getAudioTracks().length > 0
  const peerList = Object.values(peers)
  const presenterId = screenTrack ? 'self' : peerList.find((p) => p.media.screen)?.id

  const selfTile = (
    <VideoTile
      key="self"
      name={t('room.you', { name: prefs.name.trim() })}
      stream={selfStream}
      showVideo={!!screenTrack || (hasCam && media.cam)}
      micOn={hasMic && media.mic}
      mirror={!screenTrack}
      muted
      presenting={!!screenTrack}
      className={presenterId === 'self' ? 'main' : undefined}
    />
  )
  const peerTiles = peerList.map((p) => (
    <VideoTile
      key={p.id}
      name={p.name}
      stream={streams[p.id] ?? null}
      showVideo={!!streams[p.id]?.getVideoTracks().length && (p.media.cam || p.media.screen)}
      micOn={p.media.mic}
      presenting={p.media.screen}
      className={presenterId === p.id ? 'main' : undefined}
    />
  ))
  const tiles = [...peerTiles, selfTile]
  const mainTile = tiles.find((t) => t.key === (presenterId ?? ''))

  return (
    <div className="room">
      {disconnected && (
        <div className="banner" role="alert">
          {t('room.disconnected')}
          <button className="btn btn-text" onClick={onRejoin}>
            {t('room.rejoin')}
          </button>
        </div>
      )}
      <div className="room-main">
        {mainTile ? (
          <section className="stage presenting" aria-label={t('room.participants')}>
            {mainTile}
            <div className="strip">{tiles.filter((t) => t !== mainTile)}</div>
          </section>
        ) : (
          <section className={`stage count-${tiles.length}`} aria-label={t('room.participants')}>
            {tiles}
            {peerList.length === 0 && joined && (
              <div className="invite">
                <h2>{t('room.aloneTitle')}</h2>
                <p>{t('room.aloneBody')}</p>
                <code>{location.host + location.pathname}</code>
                <button className="btn btn-primary" onClick={copyLink}>
                  <Icon name="content_copy" />
                  {t('room.copyLink')}
                </button>
              </div>
            )}
          </section>
        )}
        {showPanel && (
          <CaptionPanel
            prefs={prefs}
            translation={settings.translation}
            isHost={isHost}
            captions={captions}
            speakerName={(id) => (joined && id === status.selfId ? t('room.you', { name: prefs.name.trim() }) : (names[id] ?? ''))}
            onPrefsChange={changePrefs}
            onTranslationChange={changeTranslation}
            onClose={() => setPanelOpen(false)}
          />
        )}
      </div>

      <footer className="bar">
        <div className="bar-left">
          <Clock locale={locale} />
          <span className="sep">|</span>
          {code}
        </div>
        <div className="controls">
          <button
            className={`ctl ${hasMic && media.mic ? '' : 'off'}`}
            disabled={!joined || disconnected || !hasMic}
            aria-pressed={media.mic}
            aria-label={media.mic ? t('ctl.micOff') : t('ctl.micOn')}
            onClick={() => changeMedia({ ...media, mic: !media.mic })}
          >
            <Icon name={hasMic && media.mic ? 'mic' : 'mic_off'} />
          </button>
          <button
            className={`ctl ${hasCam && media.cam ? '' : 'off'}`}
            disabled={!joined || disconnected || !hasCam}
            aria-pressed={media.cam}
            aria-label={media.cam ? t('ctl.camOff') : t('ctl.camOn')}
            onClick={() => changeMedia({ ...media, cam: !media.cam })}
          >
            <Icon name={hasCam && media.cam ? 'videocam' : 'videocam_off'} />
          </button>
          <button
            className={`ctl ${screenTrack ? 'on' : ''}`}
            disabled={!joined || disconnected || screenBusy || (!!presenterId && !screenTrack)}
            aria-pressed={!!screenTrack}
            aria-label={screenTrack ? t('ctl.stopShare') : t('ctl.share')}
            onClick={() => { void screenRef.current?.toggle() }}
          >
            <Icon name={screenTrack ? 'cancel_presentation' : 'present_to_all'} />
          </button>
          <button className="ctl leave" aria-label={t('ctl.leave')} onClick={onLeave}>
            <Icon name="call_end" />
          </button>
        </div>
        <div className="bar-right">
          <button className="ctl plain copy-link" aria-label={t('ctl.copyLink')} onClick={copyLink}>
            <Icon name="link" />
          </button>
          {/* 번역이 꺼지면 게스트에게는 자막 패널이 없다. 주최자는 다시 켤 수 있도록 남긴다. */}
          {joined && (settings.translation || isHost) && (
            <button
              className={`ctl plain ${showPanel ? 'on' : ''}`}
              aria-pressed={showPanel}
              aria-label={t('ctl.captions')}
              onClick={() => setPanelOpen((o) => !o)}
            >
              <Icon name="translate" />
            </button>
          )}
        </div>
      </footer>

      {isHost && <KnockPrompts knocks={knocks} onAdmit={admit} onDeny={deny} />}
      {toast && (
        <div className="toast" role="status">
          {t(toast.key, toast.params)}
        </div>
      )}
    </div>
  )
}

function Clock({ locale }: { locale: Locale }) {
  const [now, setNow] = useState(() => new Date())
  useEffect(() => {
    const id = setInterval(() => setNow(new Date()), 15_000)
    return () => clearInterval(id)
  }, [])
  return <span>{now.toLocaleTimeString(locale, { hour: '2-digit', minute: '2-digit', hour12: false })}</span>
}

function without<T>(record: Record<string, T>, key: string): Record<string, T> {
  const next = { ...record }
  delete next[key]
  return next
}
