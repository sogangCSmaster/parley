import { useState, type FormEvent } from 'react'
import { login, type LoginResult } from '../lib/api'
import { refreshAuth } from '../lib/auth'
import { useI18n, type MessageKey } from '../lib/i18n'
import { navigate } from '../lib/router'
import { Header } from './Header'

const ERRORS: Record<Exclude<LoginResult, 'ok'>, MessageKey> = {
  invalid: 'auth.invalid',
  locked: 'auth.locked',
  failed: 'auth.failed',
}

export function Login() {
  const { t } = useI18n()
  const [username, setUsername] = useState('')
  const [password, setPassword] = useState('')
  const [pending, setPending] = useState(false)
  const [error, setError] = useState<MessageKey | null>(null)

  async function submit(e: FormEvent) {
    e.preventDefault()
    setPending(true)
    setError(null)
    const result = await login(username.trim(), password)
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
          <h1>{t('auth.loginTitle')}</h1>
          <p className="lead">{t('auth.loginLead')}</p>
          <label className="field-label">
            {t('auth.username')}
            <input
              className="input"
              autoComplete="username"
              autoFocus
              required
              value={username}
              onChange={(e) => setUsername(e.target.value)}
            />
          </label>
          <label className="field-label">
            {t('auth.password')}
            <input
              className="input"
              type="password"
              autoComplete="current-password"
              required
              value={password}
              onChange={(e) => setPassword(e.target.value)}
            />
          </label>
          {error && (
            <p className="form-error" role="alert">
              {t(error)}
            </p>
          )}
          <button className="btn btn-primary btn-lg" disabled={pending || !username.trim() || !password}>
            {pending ? t('auth.loggingIn') : t('auth.submit')}
          </button>
        </form>
      </main>
    </div>
  )
}
