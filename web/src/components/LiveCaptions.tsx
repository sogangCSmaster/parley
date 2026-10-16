import { useEffect, useLayoutEffect, useRef, useState } from 'react'
import { useAuth } from '../lib/auth'
import { applyCaption, type Caption, type CaptionMsg } from '../lib/captions'
import { useI18n, type MessageKey } from '../lib/i18n'
import { canAutoDetect } from '../lib/langdetect'
import { LANGUAGES, languageName } from '../lib/languages'
import { loadPrefs, savePrefs } from '../lib/prefs'
import { navigate } from '../lib/router'
import type { SttStatus } from '../lib/stt'
import { AutoTalk } from '../lib/autotalk'
import type { TalkFailure } from '../lib/talk'
import { downloadText, formatTranscript, Transcript, transcriptFileName } from '../lib/transcript'
import { Icon } from './Icon'
import { Notice } from './Meeting'
import { Select } from './Select'

/**
 * 자막 화면: 대면 미팅 탁자 위 태블릿 한 대가 듣고, 누가 어떤 언어로 말하든 원문과 번역을 크게 쌓아 보여 준다.
 * 버튼을 누를 필요가 없다. 언어는 발화마다 글자로 가린다(`lib/langdetect.ts`).
 */
const LANG_OPTIONS = LANGUAGES.map((l) => ({ value: l.code as string, label: l.name }))

const FAILURES: Partial<Record<TalkFailure, MessageKey>> = {
  room_not_found: 'room.notFound',
  room_full: 'room.full',
  forbidden: 'talk.notYours',
  not_talk_room: 'talk.notTalkRoom',
  in_use: 'talk.inUse',
  disconnected: 'room.disconnected',
  server_down: 'room.serverDown',
}

export function LiveCaptions({ code }: { code: string }) {
  const { t } = useI18n()
  const auth = useAuth()
  if (auth.status === 'loading') return null
  if (auth.status !== 'host') {
    return <Notice title={t('live.title')} body={t('talk.hostOnly')} onHome={() => navigate('/')} />
  }
  return <Live code={code} />
}

function Live({ code }: { code: string }) {
  const { t, locale } = useI18n()
  const [langs, setLangs] = useState(() => {
    const p = loadPrefs()
    return { a: p.lang, b: p.partnerLang }
  })
  const [track, setTrack] = useState<MediaStreamTrack | null>(null)
  const [micError, setMicError] = useState(false)
  const [micAttempt, setMicAttempt] = useState(0)
  const [ids, setIds] = useState<{ a: string | null; b: string | null }>({ a: null, b: null })
  const [captions, setCaptions] = useState<Caption[]>([])
  const [stt, setStt] = useState<SttStatus | null>(null)
  const [listening, setListening] = useState(true)
  const [failure, setFailure] = useState<MessageKey | null>(null)
  const [transcript] = useState(() => new Transcript())
  const [hasTranscript, setHasTranscript] = useState(false)
  const talkRef = useRef<AutoTalk | null>(null)
  const langsRef = useRef(langs)
  const listeningRef = useRef(listening)
  // 실패하면 연결과 마이크를 모두 정리한다.
  const live = failure === null

  useEffect(() => {
    if (!live) return
    let cancelled = false
    let stream: MediaStream | null = null
    navigator.mediaDevices
      .getUserMedia({ audio: { echoCancellation: true, noiseSuppression: true, autoGainControl: true } })
      .then((s) => {
        if (cancelled) return s.getTracks().forEach((tr) => tr.stop())
        stream = s
        setTrack(s.getAudioTracks()[0] ?? null)
      })
      .catch(() => !cancelled && setMicError(true))
    return () => {
      cancelled = true
      stream?.getTracks().forEach((tr) => tr.stop())
      setTrack(null)
    }
  }, [live, micAttempt])

  useEffect(() => {
    if (!track || !live) return
    const talk = new AutoTalk(code, langsRef.current.a, langsRef.current.b, track, {
      onJoined: (side, id) => setIds((m) => ({ ...m, [side]: id })),
      onCaption: (msg: CaptionMsg) => {
        const now = Date.now()
        setCaptions((list) => applyCaption(list, msg, now))
        transcript.add(msg, now)
        setHasTranscript(transcript.size > 0)
      },
      onSttStatus: setStt,
      onFailed: (reason) => setFailure(FAILURES[reason] ?? 'room.cannotJoin'),
    })
    talk.setListening(listeningRef.current)
    talkRef.current = talk
    return () => {
      talk.close()
      talkRef.current = null
    }
  }, [code, track, transcript, live])

  // 같은 문자를 쓰는 두 언어는 가를 수 없으므로 듣지 않는다(엉뚱한 언어로 번역하지 않게).
  const pairOk = canAutoDetect(langs.a, langs.b)
  const hearing = listening && pairOk
  useEffect(() => {
    listeningRef.current = hearing
    talkRef.current?.setListening(hearing)
  }, [hearing, ids])

  function changeLang(side: 'a' | 'b', lang: string) {
    const next = { ...langs, [side]: lang }
    langsRef.current = next
    setLangs(next)
    talkRef.current?.setLangs(next.a, next.b)
    savePrefs({ ...loadPrefs(), lang: next.a, partnerLang: next.b })
  }

  function toggleListening() {
    setListening((on) => !on)
  }

  function speakerLabel(id: string) {
    if (id === ids.a) return languageName(langs.a)
    if (id === ids.b) return languageName(langs.b)
    return t('captions.participant')
  }

  function save() {
    const text = formatTranscript(transcript.entries(), {
      title: t('transcript.title'),
      code,
      savedAt: new Date(),
      locale,
      speakerName: speakerLabel,
    })
    downloadText(transcriptFileName(code, new Date()), text)
  }

  if (failure) {
    return (
      <div className="shell">
        <main className="left">
          <h1>{t('live.title')}</h1>
          <p className="lead">{t(failure)}</p>
          <div className="row">
            {hasTranscript && (
              <button className="btn btn-outline" onClick={save}>
                <Icon name="download" />
                {t('captions.download')}
              </button>
            )}
            <button className="btn btn-primary" onClick={() => navigate('/')}>
              {t('room.home')}
            </button>
          </div>
        </main>
      </div>
    )
  }

  const ready = !!ids.a && !!ids.b
  const status: MessageKey = micError
    ? 'talk.noMic'
    : !ready
      ? 'talk.connecting'
      : !hearing
        ? 'live.paused'
        : stt === 'unavailable'
          ? 'captions.sttUnavailable'
          : stt === 'paused'
            ? 'captions.sttPaused'
            : stt === 'live'
              ? 'live.listening'
              : 'talk.connecting'

  return (
    <div className="live">
      <header className="live-bar">
        <div className="live-langs">
          <Select ariaLabel={t('live.langA')} value={langs.a} options={LANG_OPTIONS} onChange={(l) => changeLang('a', l)} variant="pill" icon="language" />
          <Icon name="sync_alt" />
          <Select ariaLabel={t('live.langB')} value={langs.b} options={LANG_OPTIONS} onChange={(l) => changeLang('b', l)} variant="pill" icon="language" />
        </div>
        <p className={`live-status${hearing && stt === 'live' && !micError ? ' on' : ''}`} role="status">
          <span className="live-dot" aria-hidden="true" />
          {t(status)}
        </p>
        <div className="live-actions">
          {micError ? (
            <button
              className="btn btn-outline"
              onClick={() => {
                setMicError(false)
                setMicAttempt((n) => n + 1)
              }}
            >
              {t('room.retry')}
            </button>
          ) : (
            <button className="btn btn-outline" onClick={toggleListening} disabled={!ready || !pairOk}>
              <Icon name={listening ? 'mic_off' : 'mic'} />
              {t(listening ? 'live.pause' : 'live.resume')}
            </button>
          )}
          {hasTranscript && (
            <button className="icon-btn" aria-label={t('captions.download')} title={t('captions.download')} onClick={save}>
              <Icon name="download" />
            </button>
          )}
          <button className="btn btn-primary" onClick={() => navigate('/')}>
            {t('talk.end')}
          </button>
        </div>
      </header>
      {!pairOk && (
        <p className="live-warning" role="alert">
          {t('live.unsupportedPair')}
        </p>
      )}
      <LiveFeed captions={captions} />
    </div>
  )
}

/** 쌓여 가는 자막. 사용자가 위로 스크롤해 지난 말을 보는 중이면 따라 내려가지 않는다. */
function LiveFeed({ captions }: { captions: Caption[] }) {
  const { t, locale } = useI18n()
  const ref = useRef<HTMLDivElement>(null)
  const stick = useRef(true)
  const visible = captions.filter((c) => !(c.final && c.src === ''))
  useLayoutEffect(() => {
    const el = ref.current
    if (el && stick.current) el.scrollTop = el.scrollHeight
  }, [captions])

  return (
    <main
      className="live-feed"
      ref={ref}
      role="log"
      aria-live="polite"
      onScroll={() => {
        const el = ref.current
        if (el) stick.current = el.scrollHeight - el.scrollTop - el.clientHeight < 48
      }}
    >
      <div className="live-feed-inner">
        {visible.length === 0 && <p className="feed-empty">{t('live.empty')}</p>}
        {visible.map((c) => {
          const time = new Date(c.at).toLocaleTimeString(locale, { hour: '2-digit', minute: '2-digit', hour12: false })
          // 언어·시각은 원문 줄 앞에 붙여 줄 수를 아낀다. 번역이 없는 확정 줄(번역 실패 등)은 원문을 크게 보인다.
          const translated = c.tgt_lang !== c.src_lang && (c.text != null || !c.final)
          const meta = (
            <span className="live-meta">
              <b lang={c.src_lang}>{languageName(c.src_lang)}</b> {time}
            </span>
          )
          return (
            <article key={c.seg} className={`live-entry${c.final ? '' : ' pending'}`} aria-hidden={!c.final}>
              {translated ? (
                <>
                  <p className="live-src">
                    {meta}
                    <span lang={c.src_lang}>{c.src}</span>
                  </p>
                  <p className="live-tgt" lang={c.text != null ? c.tgt_lang : undefined}>
                    {c.text ?? t('captions.translating')}
                  </p>
                </>
              ) : (
                <>
                  <p className="live-src">{meta}</p>
                  <p className="live-tgt" lang={c.src_lang}>
                    {c.src}
                  </p>
                </>
              )}
            </article>
          )
        })}
      </div>
    </main>
  )
}
