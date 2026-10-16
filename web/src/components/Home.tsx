import { useState, type FormEvent } from 'react'
import { createRoom } from '../lib/api'
import { refreshAuth, useAuth } from '../lib/auth'
import { useI18n, type MessageKey } from '../lib/i18n'
import { navigate, parseRoomCode } from '../lib/router'
import { Header } from './Header'
import { HeroDemo } from './HeroDemo'
import { Icon } from './Icon'
import { NewMeetingDialog, type MeetingKind } from './NewMeetingDialog'

/**
 * AGPL-3.0 §13: 수정한 버전을 서비스로 운영하면 사용자에게 그 소스를 제공해야 한다.
 * 포크해서 운영한다면 이 주소를 자기 저장소로 바꾼다.
 */
const SOURCE_URL = 'https://github.com/sogangCSmaster/parley'

export function Home({ invalidPath }: { invalidPath: boolean }) {
  const { t } = useI18n()
  const [input, setInput] = useState('')
  // 메시지 키를 저장해 두면 화면 언어를 바꿔도 오류 문구가 함께 바뀐다.
  const [error, setError] = useState<MessageKey | null>(invalidPath ? 'home.invalidLink' : null)
  const [creating, setCreating] = useState(false)
  const [dialogOpen, setDialogOpen] = useState(false)
  const [createError, setCreateError] = useState<MessageKey | null>(null)
  const auth = useAuth()
  const isHost = auth.status === 'host'

  async function createMeeting(kind: MeetingKind) {
    setCreating(true)
    setCreateError(null)
    try {
      navigate(`/${await createRoom({ translation: kind === 'global' })}`)
    } catch {
      // 세션이 만료됐을 수 있으니 로그인 상태를 다시 확인한다.
      void refreshAuth()
      setCreateError('home.createFailed')
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
              <>
                <div className="action-row">
                  <button
                    className="btn btn-primary btn-lg"
                    aria-haspopup="dialog"
                    onClick={() => {
                      setCreateError(null)
                      setDialogOpen(true)
                    }}
                  >
                    <Icon name="video_call" />
                    {t('home.newMeeting')}
                  </button>
                </div>
                <div className="or" aria-hidden="true">
                  {t('home.or')}
                </div>
              </>
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
              <button className={`btn btn-lg ${isHost ? 'btn-outline' : 'btn-primary'}`} disabled={!input.trim()}>
                {t('home.join')}
              </button>
            </form>
          </div>
          {error && (
            <p className="form-error" role="alert">
              {t(error)}
            </p>
          )}
          <p className="hint">{isHost ? t('home.hint') : t('home.guestHint')}</p>
          <p className="source-note">
            {t('home.openSource')}{' '}
            <a href={SOURCE_URL} target="_blank" rel="noopener noreferrer">
              {t('home.sourceCode')}
            </a>
          </p>
          {isHost && (
            <NewMeetingDialog
              open={dialogOpen}
              creating={creating}
              error={createError}
              onCreate={(kind) => void createMeeting(kind)}
              onClose={() => setDialogOpen(false)}
            />
          )}
        </section>
        <HeroDemo />
      </main>
    </div>
  )
}
