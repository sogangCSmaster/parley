import { useState, type FormEvent } from 'react'
import { createRoom } from '../lib/api'
import { refreshAuth, useAuth } from '../lib/auth'
import { useI18n, type MessageKey } from '../lib/i18n'
import { navigate, parseRoomCode } from '../lib/router'
import { Header } from './Header'
import { Icon } from './Icon'
import { Switch } from './Switch'

export function Home({ invalidPath }: { invalidPath: boolean }) {
  const { t } = useI18n()
  const [input, setInput] = useState('')
  // 메시지 키를 저장해 두면 화면 언어를 바꿔도 오류 문구가 함께 바뀐다.
  const [error, setError] = useState<MessageKey | null>(invalidPath ? 'home.invalidLink' : null)
  const [creating, setCreating] = useState(false)
  const [translation, setTranslation] = useState(true)
  const auth = useAuth()
  const isHost = auth.status === 'host'

  async function createMeeting() {
    setCreating(true)
    setError(null)
    try {
      navigate(`/${await createRoom({ translation })}`)
    } catch {
      // 세션이 만료됐을 수 있으니 로그인 상태를 다시 확인한다.
      void refreshAuth()
      setError('home.createFailed')
      setCreating(false)
    }
  }

  function join(e: FormEvent) {
    e.preventDefault()
    const code = parseRoomCode(input)
    if (code) navigate(`/${code}`)
    else setError('home.invalidCode')
  }

  return (
    <div className="shell">
      <Header />
      <main className="home">
        <section className="home-copy">
          <h1>{t('home.title')}</h1>
          <p className="lead">{t('home.lead')}</p>
          <div className="home-actions">
            {isHost && (
              <button className="btn btn-primary btn-lg" onClick={createMeeting} disabled={creating}>
                <Icon name="video_call" />
                {creating ? t('home.creating') : t('home.newMeeting')}
              </button>
            )}
            <form className="code-form" onSubmit={join}>
              <label className="field">
                <Icon name="keyboard" />
                <input
                  value={input}
                  onChange={(e) => setInput(e.target.value)}
                  placeholder={t('home.codePlaceholder')}
                  aria-label={t('home.codeLabel')}
                />
              </label>
              <button className={`btn btn-lg ${isHost ? 'btn-text' : 'btn-primary'}`} disabled={!input.trim()}>
                {t('home.join')}
              </button>
            </form>
          </div>
          {isHost && (
            <div className="home-options">
              <Switch checked={translation} onChange={setTranslation} label={t('home.translation')} />
            </div>
          )}
          {error && (
            <p className="form-error" role="alert">
              {t(error)}
            </p>
          )}
          <p className="hint">{isHost ? t('home.hint') : t('home.guestHint')}</p>
        </section>
        <Demo />
      </main>
    </div>
  )
}

/** 제품이 실제로 어떻게 보이는지 보여주는 정적 예시. 예시 대화라 번역 대상이 아니다. */
function Demo() {
  return (
    <div className="demo" aria-hidden="true">
      <div className="demo-stage">
        <div className="demo-tile">
          <span className="demo-avatar" style={{ background: '#C2381A' }}>
            さ
          </span>
          <span className="demo-name">さくら</span>
        </div>
        <div className="demo-tile">
          <span className="demo-avatar" style={{ background: '#0E7F6A' }}>
            민
          </span>
          <span className="demo-name">민준</span>
        </div>
      </div>
      <div className="demo-panel">
        <div className="entry">
          <div className="meta">
            <b>さくら</b>
            <time>15:21</time>
          </div>
          <p>私たちの投資チームは4人のメンバーです。</p>
          <p>저희 투자팀은 4명의 멤버로 구성돼 있어요.</p>
        </div>
        <div className="entry">
          <div className="meta">
            <b>민준</b>
            <time>15:22</time>
          </div>
          <p>주로 어떤 분야에 투자하고 계신가요?</p>
          <p>主にどのような分野に投資されていますか？</p>
        </div>
      </div>
    </div>
  )
}
