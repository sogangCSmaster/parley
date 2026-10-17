import { useState, type FormEvent } from 'react'
import { createRoom } from '../lib/api'
import { refreshAuth, useAuth } from '../lib/auth'
import { useI18n, type MessageKey } from '../lib/i18n'
import { navigate, parseRoomCode } from '../lib/router'
import { BrandMark } from './BrandMark'
import { Header } from './Header'
import { Icon } from './Icon'

type Kind = 'meeting' | 'inPerson' | 'live'

export function Home() {
  const auth = useAuth()
  if (auth.status === 'loading') return null
  return (
    <div className="shell">
      <Header />
      {auth.status === 'host' ? <HostHome name={auth.username} /> : <GuestHome />}
    </div>
  )
}

/** 받은 링크나 코드를 붙여 넣어 들어가는 칸. 오류 문구는 키로 두어 화면 언어를 바꾸면 함께 바뀐다. */
function JoinForm({ primary }: { primary?: boolean }) {
  const { t } = useI18n()
  const [input, setInput] = useState('')
  const [error, setError] = useState<MessageKey | null>(null)

  function join(e: FormEvent) {
    e.preventDefault()
    const code = parseRoomCode(input)
    if (code) navigate(`/${code}`)
    else setError('home.invalidCode')
  }

  return (
    <form className="join-form" onSubmit={join}>
      <div className="join-row">
        <label className="field">
          <Icon name="link" />
          <input
            value={input}
            onChange={(e) => {
              setInput(e.target.value)
              setError(null)
            }}
            placeholder={t('home.codePlaceholder')}
            aria-label={t('home.codeLabel')}
          />
        </label>
        <button className={`btn ${primary ? 'btn-primary btn-lg' : 'btn-ink'}`} disabled={!input.trim()}>
          {t('home.join')}
        </button>
      </div>
      {error && (
        <p className="form-error" role="alert">
          {t(error)}
        </p>
      )}
    </form>
  )
}

/** 로그인하지 않은 사람: 받은 링크로 들어오는 곳. 회의를 여는 사람은 로그인으로 안내한다. */
function GuestHome() {
  const { t } = useI18n()
  return (
    <main className="guest-home">
      <BrandMark size={64} />
      <h1>{t('home.guestTitle')}</h1>
      <p className="lead">{t('home.guestHint')}</p>
      <JoinForm primary />
      <p className="guest-foot">
        {t('home.hostPrompt')}{' '}
        <a
          href="/login"
          onClick={(e) => {
            e.preventDefault()
            navigate('/login')
          }}
        >
          {t('auth.hostLogin')}
        </a>
      </p>
    </main>
  )
}

/** 로그인한 주최자: 인사, 지금 회의 시작, 이 기기에서 바로(대면 대화·자막 화면), 코드로 참여. */
function HostHome({ name }: { name: string }) {
  const { t, locale } = useI18n()
  const [creating, setCreating] = useState<Kind | null>(null)
  const [error, setError] = useState<MessageKey | null>(null)
  const today = new Date().toLocaleDateString(locale, { month: 'long', day: 'numeric', weekday: 'long' })

  async function create(kind: Kind) {
    if (creating) return
    setCreating(kind)
    setError(null)
    try {
      // 대면 대화·자막 화면도 회의실 하나를 쓴다(번역 켜짐). 번역은 회의 중에 주최자가 끌 수 있다.
      const code = await createRoom({ translation: true })
      navigate(kind === 'inPerson' ? `/talk/${code}` : kind === 'live' ? `/live/${code}` : `/${code}`)
    } catch {
      // 세션이 만료됐을 수 있으니 로그인 상태를 다시 확인한다.
      void refreshAuth()
      setError('home.createFailed')
      setCreating(null)
    }
  }

  return (
    <main className="host-home">
      <div className="host-top">
        <div>
          <p className="status-eyebrow">{today}</p>
          <h1>{t('home.hello', { name })}</h1>
        </div>
        <JoinForm />
      </div>
      {error && (
        <p className="form-error" role="alert">
          {t(error)}
        </p>
      )}
      <div className="host-grid">
        <section className="start-card" aria-labelledby="start-title">
          <BrandMark size={44} />
          <h2 id="start-title">{t('home.startTitle')}</h2>
          <p>{t('home.startBody')}</p>
          <button className="btn btn-primary btn-lg" onClick={() => void create('meeting')} disabled={!!creating}>
            <Icon name="video_call" />
            {creating === 'meeting' ? t('home.creating') : t('home.startNow')}
          </button>
        </section>
        <section className="device-card" aria-labelledby="device-title">
          <h2 id="device-title">{t('home.onThisDevice')}</h2>
          <div className="device-options">
            <button className="device-option" onClick={() => void create('inPerson')} disabled={!!creating}>
              <Icon name="forum" />
              <b>{creating === 'inPerson' ? t('home.creating') : t('newMeeting.inPerson')}</b>
              <span>{t('home.inPersonShort')}</span>
            </button>
            <button className="device-option" onClick={() => void create('live')} disabled={!!creating}>
              <Icon name="subtitles" />
              <b>{creating === 'live' ? t('home.creating') : t('newMeeting.live')}</b>
              <span>{t('home.liveShort')}</span>
            </button>
          </div>
        </section>
      </div>
    </main>
  )
}
