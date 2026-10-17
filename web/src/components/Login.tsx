import { useState, type FormEvent } from 'react'
import { login, type LoginResult } from '../lib/api'
import { refreshAuth } from '../lib/auth'
import { useI18n, type MessageKey } from '../lib/i18n'
import { navigate } from '../lib/router'
import { BrandMark } from './BrandMark'
import { Header } from './Header'

const ERRORS: Record<Exclude<LoginResult, 'ok'>, MessageKey> = {
  invalid: 'auth.invalid',
  locked: 'auth.locked',
  failed: 'auth.failed',
}

export function Login() {
  const { t } = useI18n()
  const [email, setEmail] = useState('')
  const [password, setPassword] = useState('')
  const [pending, setPending] = useState(false)
  const [error, setError] = useState<MessageKey | null>(null)

  async function submit(e: FormEvent) {
    e.preventDefault()
    setPending(true)
    setError(null)
    const result = await login(email.trim(), password)
    if (result === 'ok') {
      await refreshAuth()
      navigate('/')
      return
    }
    setPassword('')
    setError(ERRORS[result])
    setPending(false)
  }

  return (
    <div className="shell">
      <Header />
      <main className="auth">
        <form className="auth-card" onSubmit={submit}>
          <BrandMark size={56} />
          <h1>{t('auth.loginTitle')}</h1>
          {/* 이메일·비밀번호를 한 덩어리 칸으로: 칸마다 위에 작은 이름표 */}
          <div className="field-stack">
            <label>
              <span>{t('auth.email')}</span>
              <input
                autoComplete="username email"
                inputMode="email"
                autoCapitalize="none"
                spellCheck={false}
                autoFocus
                required
                value={email}
                onChange={(e) => setEmail(e.target.value)}
              />
            </label>
            <label>
              <span>{t('auth.password')}</span>
              <input
                type="password"
                autoComplete="current-password"
                required
                value={password}
                onChange={(e) => setPassword(e.target.value)}
              />
            </label>
          </div>
          {error && (
            <p className="form-error" role="alert">
              {t(error)}
            </p>
          )}
          <button className="btn btn-primary btn-lg" disabled={pending || !email.trim() || !password}>
            {pending ? t('auth.loggingIn') : t('auth.submit')}
          </button>
          <p className="auth-foot">{t('auth.guestNote')}</p>
        </form>
      </main>
    </div>
  )
}
