import { useEffect, useMemo, useRef, useState } from 'react'
import type { Knock } from '../protocol/Knock'
import type { MediaState } from '../protocol/MediaState'
import type { Peer } from '../protocol/Peer'
import type { RoomSettings } from '../protocol/RoomSettings'
import type { ServerMsg } from '../protocol/ServerMsg'
import type { SignalData as WireSignal } from '../protocol/SignalData'
import { applyCaption, type Caption } from '../lib/captions'
import { useI18n, type Locale, type MessageKey } from '../lib/i18n'
import { Mesh } from '../lib/mesh'
import type { Prefs } from '../lib/prefs'
import { Signaling } from '../lib/signaling'
import { ScreenShare } from '../lib/screen'
import { startSpeech, type Speech } from '../lib/speech'
import { downloadText, formatTranscript, Transcript, transcriptFileName } from '../lib/transcript'
import type { Grant, SegCounter, SttStatus, TokenError } from '../lib/stt'
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
  /** 나갈 때 자막 기록을 텍스트로 만드는 함수를 넘긴다(없으면 null). 나간 화면에서 저장을 누를 때만 만든다. */
  onLeave: (exportTranscript: (() => string) | null) => void
  onRejoin: () => void
}

type Status =
  | { kind: 'connecting' }
  | { kind: 'waiting' }
  | { kind: 'joined'; selfId: string }
  | { kind: 'error'; message: MessageKey }

/** 다시 연결할 때 WebSocket이 열리기를 기다리는 시간. 넘으면 이 시도를 버리고 다음 시도로 넘어간다. */
const RECONNECT_TIMEOUT_MS = 5000
/** 처음 입장할 때는 재시도 없이 오류를 보이므로, 느린 모바일 망의 TLS 연결까지 기다리도록 넉넉히 둔다. */
const FIRST_CONNECT_TIMEOUT_MS = 10_000

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
  const [toast, setToast] = useState<{ key: MessageKey; params?: Record<string, string>; ms?: number } | null>(null)
  // 헤드폰 안내는 회의에 한 번만(재연결·번역 토글로 음성 인식이 다시 시작돼도).
  const echoHintedRef = useRef(false)
  const [isHost, setIsHost] = useState(false)
  const [settings, setSettings] = useState<RoomSettings>({ translation: true })
  const [knocks, setKnocks] = useState<Knock[]>([])
  const [captions, setCaptions] = useState<Caption[]>([])
  // 화면 목록(최근 200줄)과 별개로 회의 내내 확정된 자막을 모은다. 다시 연결해도 이어진다.
  const [transcript] = useState(() => new Transcript())
  const transcriptRef = useRef(transcript)
  const [hasTranscript, setHasTranscript] = useState(false)
  // 나간 참가자의 지난 자막에도 이름이 보이도록 이름은 지우지 않고 모은다.
  const [names, setNames] = useState<Record<string, string>>({})
  const [sttStatus, setSttStatus] = useState<SttStatus | null>(null)

  const sigRef = useRef<Signaling | null>(null)
  const meshRef = useRef<Mesh | null>(null)
  const screenRef = useRef<ScreenShare | null>(null)
  const speechRef = useRef<Speech | null>(null)
  // STT 구간 번호는 서버 연결 하나에서 계속 늘어나야 한다(음성 인식이 꺼졌다 켜져도).
  const segRef = useRef<SegCounter>({ next: 0 })
  // 발급은 비동기라 답 순서가 바뀔 수 있어 요청 id로 짝을 맞춘다. 남은 요청은 정리할 때 실패로 끝낸다.
  const tokensRef = useRef(new Map<number, { resolve: (g: Grant) => void; reject: (e: TokenError) => void }>())
  const tokenIdRef = useRef(0)
  const onMediaChangeRef = useRef(onMediaChange)
  useEffect(() => { onMediaChangeRef.current = onMediaChange }, [onMediaChange])
  // 이름은 입장할 때 정한 값으로 고정한다. 언어·미디어 상태는 다시 연결할 때 그때의 값을 보낸다.
  const nameRef = useRef(prefs.name.trim())
  const prefsRef = useRef(prefs)
  useEffect(() => {
    prefsRef.current = prefs
  }, [prefs])
  // 연결이 끊기면 같은 화면에서 다시 연결한다. 한 번 입장했으면 받은 재입장 토큰으로 수락 없이 돌아온다.
  const [attempt, setAttempt] = useState(0)
  const resumeRef = useRef<string | null>(null)
  const everJoinedRef = useRef(false)
  const retriesRef = useRef(0)
  // 대기 중인 재시도 타이머. 시도가 진행 중이면 null이다.
  const retryTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null)
  // 방이 없어졌거나 거절된 경우처럼 다시 연결해도 소용없을 때 true. 사용자가 직접 다시 시도한다.
  const stoppedRef = useRef(false)
  // 다시 연결하면 내 참가자 id가 바뀐다. 지난 id로 남은 내 자막도 "나"로 보이게 모은다.
  const selfIdsRef = useRef(new Set<string>())
  // 브라우저의 "공유 중지"처럼 나중에 호출되는 콜백이 최신 미디어 상태를 보도록 한다.
  const mediaRef = useRef(media)
  useEffect(() => {
    mediaRef.current = media
  }, [media])
  const langRef = useRef(prefs.lang)
  useEffect(() => {
    langRef.current = prefs.lang
  }, [prefs.lang])

  useEffect(() => {
    let disposed = false

    const onMessage = (msg: ServerMsg) => {
      if (disposed) return
      switch (msg.t) {
        case 'welcome': {
          const mesh = new Mesh(
            msg.you,
            local,
            msg.ice_servers,
            // 브라우저 toJSON() 형식과 같다(DOM 타입만 candidate를 선택 항목으로 둔다).
            (to, data) => sig.send({ t: 'signal', to, data: data as WireSignal }),
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
          everJoinedRef.current = true
          resumeRef.current = msg.resume
          selfIdsRef.current.add(msg.you)
          retriesRef.current = 0
          setDisconnected(false)
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
          meshRef.current?.handleSignal(msg.from, msg.data)
          break
        case 'waiting':
          // 토큰이 만료돼 다시 수락을 기다리는 경우: 이제부터는 처음 입장할 때처럼 다룬다.
          resumeRef.current = null
          everJoinedRef.current = false
          setDisconnected(false)
          setStatus({ kind: 'waiting' })
          break
        case 'denied':
          stoppedRef.current = true
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
        case 'caption': {
          const now = Date.now()
          setCaptions((list) => applyCaption(list, msg, now))
          transcriptRef.current.add(msg, now)
          setHasTranscript(transcriptRef.current.size > 0)
          break
        }
        case 'stt_token':
          tokensRef.current.get(msg.id)?.resolve({ token: msg.token, model: msg.model })
          tokensRef.current.delete(msg.id)
          break
        case 'stt_token_denied':
          tokensRef.current.get(msg.id)?.reject(msg.retry ? 'failed' : 'unavailable')
          tokensRef.current.delete(msg.id)
          break
        case 'notice':
          if (msg.code === 'translation_failed') setToast({ key: 'captions.failed' })
          break
        case 'error': {
          const fatal = FATAL_ERRORS[msg.code]
          if (!joinedRef.current) {
            stoppedRef.current = true
            resumeRef.current = null
            setStatus({ kind: 'error', message: fatal ?? 'room.cannotJoin' })
          }
          else if (msg.code === 'room_full') setToast({ key: 'room.full' })
          else console.warn('server error', msg.code, msg.message)
          break
        }
      }
    }

    const joinedRef = { current: false }
    const sig = new Signaling(onMessage, (opened) => {
      if (disposed) return
      // 이 시도는 끝났다. 연결 시간 제한이 같은 시도에 재시도를 한 번 더 걸지 않게 한다.
      clearTimeout(connectTimer)
      screenRef.current?.dispose()
      screenRef.current = null
      setScreenTrack(null)
      setScreenBusy(false)
      meshRef.current?.close()
      meshRef.current = null
      setStreams({})
      if (mediaRef.current.screen) {
        const next = { ...mediaRef.current, screen: false }
        mediaRef.current = next
        onMediaChangeRef.current(next)
      }
      if (stoppedRef.current) return
      if (!everJoinedRef.current) {
        setStatus({ kind: 'error', message: opened ? 'room.disconnected' : 'room.serverDown' })
        return
      }
      scheduleRetry()
    })
    // 입장한 적이 있으면 화면을 그대로 두고 1·2·4·8·10초… 간격으로 다시 연결한다.
    function scheduleRetry() {
      setDisconnected(true)
      if (retryTimerRef.current) clearTimeout(retryTimerRef.current)
      const delay = Math.min(10_000, 1000 * 2 ** retriesRef.current++)
      retryTimerRef.current = setTimeout(() => {
        retryTimerRef.current = null
        setAttempt((a) => a + 1)
      }, delay)
    }
    // 응답 없는 망(VPN 전환·캡티브 포털)에서는 연결 시도가 OS 시간 제한(1분 이상)까지 걸려 있을 수 있다.
    const connectTimer = setTimeout(() => {
      if (disposed || sig.hasOpened || stoppedRef.current) return
      sig.close()
      if (everJoinedRef.current) scheduleRetry()
      else setStatus({ kind: 'error', message: 'room.serverDown' })
    }, everJoinedRef.current ? RECONNECT_TIMEOUT_MS : FIRST_CONNECT_TIMEOUT_MS)
    sigRef.current = sig
    segRef.current = { next: 0 }

    // ICE 서버(TURN 자격 포함)는 welcome으로 온다. 열리기 전에 보낸 join은 큐에 모였다가 바로 나간다.
    const p = prefsRef.current
    sig.send({
      t: 'join',
      room: code,
      name: nameRef.current,
      lang: p.lang,
      partner_lang: p.partnerLang,
      media: mediaRef.current,
      ...(resumeRef.current ? { resume: resumeRef.current } : {}),
    })
    // 토큰은 welcome에서 새것으로 바뀔 때까지 지니고 있는다(이 연결이 열리지 못하면 다음 시도에 다시 쓴다).

    return () => {
      disposed = true
      clearTimeout(connectTimer)
      if (retryTimerRef.current) clearTimeout(retryTimerRef.current)
      sig.close()
      screenRef.current?.dispose()
      screenRef.current = null
      meshRef.current?.close()
      meshRef.current = null
      sigRef.current = null
    }
  }, [code, local, attempt])

  // 망이 돌아오면 기다리지 않고 바로 다시 연결한다.
  useEffect(() => {
    if (!disconnected) return
    const retryNow = () => {
      // 이미 진행 중인 시도는 건드리지 않는다(입장 직전에 끊으면 받은 토큰을 잃는다).
      if (!retryTimerRef.current) return
      clearTimeout(retryTimerRef.current)
      retryTimerRef.current = null
      setAttempt((a) => a + 1)
    }
    window.addEventListener('online', retryNow)
    return () => window.removeEventListener('online', retryNow)
  }, [disconnected])

  useEffect(() => {
    if (!toast) return
    const id = setTimeout(() => setToast(null), toast.ms ?? 3000)
    return () => clearTimeout(id)
  }, [toast])

  const joined = status.kind === 'joined'
  const micTrack = local.getAudioTracks()[0] ?? null
  // 내 말은 내 브라우저가 직접 인식한다. 번역이 켜진 방에 입장해 있는 동안만.
  const sttEnabled = joined && !disconnected && settings.translation && !!micTrack

  useEffect(() => {
    const sig = sigRef.current
    if (!sttEnabled || !micTrack || !sig) return
    const tokens = tokensRef.current
    const speech = startSpeech({
      track: micTrack,
      lang: langRef.current,
      mic: mediaRef.current.mic,
      seg: segRef.current,
      requestToken: () =>
        new Promise<Grant>((resolve, reject) => {
          const id = ++tokenIdRef.current
          tokens.set(id, { resolve, reject })
          sig.send({ t: 'stt_token', id })
          setTimeout(() => {
            if (!tokens.delete(id)) return
            reject('failed')
          }, 10_000)
        }),
      sendStt: (seg, text, final) => sig.send({ t: 'stt', seg, text, final }),
      onStatus: setSttStatus,
      farEnd: () => meshRef.current?.audioReceivers() ?? [],
      onEcho: () => {
        if (echoHintedRef.current) return
        echoHintedRef.current = true
        // 두 문장짜리 안내라 다른 알림보다 오래 보여 준다.
        setToast({ key: 'captions.echo', ms: 6000 })
      },
    })
    speechRef.current = speech
    return () => {
      speech.close()
      speechRef.current = null
      // 늦게 온 답은 위에서 id를 못 찾아 버려진다.
      for (const waiter of tokens.values()) waiter.reject('failed')
      tokens.clear()
      setSttStatus(null)
    }
  }, [sttEnabled, micTrack])

  useEffect(() => {
    speechRef.current?.setMic(media.mic)
  }, [media.mic, sttEnabled])

  useEffect(() => {
    speechRef.current?.setLang(prefs.lang)
  }, [prefs.lang, sttEnabled])
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

  function speakerName(id: string) {
    return selfIdsRef.current.has(id) ? t('room.you', { name: prefs.name.trim() }) : (names[id] ?? '')
  }

  function transcriptText(): string {
    const fileName = (id: string) => speakerName(id) || t('captions.participant')
    return formatTranscript(transcriptRef.current.entries(), {
      title: t('transcript.title'),
      code,
      savedAt: new Date(),
      locale,
      speakerName: fileName,
    })
  }

  function downloadTranscript() {
    downloadText(transcriptFileName(code, new Date()), transcriptText())
  }

  function leave() {
    // 나가는 클릭을 늦추지 않도록 텍스트는 저장을 누를 때 만든다.
    onLeave(transcriptRef.current.size > 0 ? transcriptText : null)
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
          <button className="btn btn-outline" onClick={leave}>
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
            <button className="btn btn-primary" onClick={leave}>
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
        <div className="banner" role="status">
          <span className="spinner spinner-sm" aria-hidden="true" />
          {t('room.reconnecting')}
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
            sttStatus={sttStatus}
            speakerName={speakerName}
            onDownload={hasTranscript ? downloadTranscript : null}
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
          <button className="ctl leave" aria-label={t('ctl.leave')} onClick={leave}>
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
