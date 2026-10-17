import { useEffect, useState, type FormEvent } from 'react'
import { api, login } from '../lib/api'
import { refreshAuth, useFeatures } from '../lib/auth'
import { useI18n, type MessageKey } from '../lib/i18n'
import '../lib/i18nPages'
import { PRIVACY_URL, TERMS_URL } from '../lib/links'
import { loginWithPasskey, passkeySupported } from '../lib/passkey'
import { clearPendingSignup, pendingPassword, setPendingSignup } from '../lib/pendingSignup'
import { navigate, queryParam, replacePath } from '../lib/router'
import { AuthLayout, AuthSwitch, Divider, GoogleButton, InLink } from './AuthBits'
import { BrandMark } from './BrandMark'
import { CodeInput } from './CodeInput'
import { Icon } from './Icon'
import { Rich } from './Rich'

const EMAIL = /^[^\s@]+@[^\s@]+\.[^\s@]+$/

/** 서버 오류 코드 → 문구 */
function messageFor(code: string): MessageKey {
  switch (code) {
    case 'email':
      return 'auth.emailInvalid'
    case 'password':
      return 'auth.passwordShort'
    case 'breached':
      return 'auth.breached'
    case 'rate_limited':
      return 'auth.rateLimited'
    case 'locked':
      return 'auth.locked'
    default:
      return 'auth.failed'
  }
}

async function enterApp() {
  await refreshAuth()
  navigate('/')
}

// ── 로그인 ────────────────────────────────────────────

export function Login() {
  // Google 로그인 뒤 2차 인증이 필요하면 서버가 ?mfa=티켓으로 돌려보낸다.
  const [ticket, setTicket] = useState<string | null>(() => queryParam('mfa'))
  const googleFailed = queryParam('error') === 'google'
  useEffect(() => {
    if (queryParam('mfa') || queryParam('error')) replacePath('/login')
  }, [])
  return ticket ? <MfaStep ticket={ticket} onRestart={() => setTicket(null)} /> : <PasswordStep onMfa={setTicket} googleFailed={googleFailed} />
}

function PasswordStep({ onMfa, googleFailed }: { onMfa: (ticket: string) => void; googleFailed: boolean }) {
  const { t } = useI18n()
  const feat = useFeatures()
  const [email, setEmail] = useState('')
  const [password, setPassword] = useState('')
  const [pending, setPending] = useState(false)
  const [error, setError] = useState<MessageKey | null>(googleFailed ? 'auth.googleFailed' : null)

  async function submit(e: FormEvent) {
    e.preventDefault()
    setPending(true)
    setError(null)
    const r = await login(email.trim(), password)
    if (r.kind === 'ok') return enterApp()
    setPending(false)
    if (r.kind === 'mfa') return onMfa(r.ticket)
    setPassword('')
    setError(r.kind === 'invalid' || r.kind === 'unverified' ? 'auth.invalid' : r.kind === 'locked' ? 'auth.locked' : 'auth.failed')
  }

  async function passkey() {
    setError(null)
    const r = await loginWithPasskey()
    if (r === 'ok') return enterApp()
    if (r === 'failed') setError('auth.passkeyFailed')
  }

  return (
    <AuthLayout>
      <form className="auth-card" onSubmit={submit}>
        <BrandMark size={56} />
        <h1>{t('auth.loginTitle')}</h1>
        <div className="field-stack">
          <label>
            <span>{t('auth.email')}</span>
            <input
              autoComplete="username email webauthn"
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
            <input type="password" autoComplete="current-password" required value={password} onChange={(e) => setPassword(e.target.value)} />
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
        <InLink to="/forgot" className="auth-link">
          {t('auth.forgot')}
        </InLink>
        {(feat.google || passkeySupported) && <Divider label={t('auth.or')} />}
        {feat.google && <GoogleButton />}
        {passkeySupported && (
          <button type="button" className="btn btn-lg btn-plain" onClick={() => void passkey()}>
            <Icon name="passkey" />
            {t('auth.passkey')}
          </button>
        )}
        <AuthSwitch prompt={t('auth.noAccount')} to="/signup" label={t('auth.createAccount')} />
        <p className="auth-foot">{t('auth.guestNote')}</p>
      </form>
    </AuthLayout>
  )
}

function MfaStep({ ticket, onRestart }: { ticket: string; onRestart: () => void }) {
  const { t } = useI18n()
  const [code, setCode] = useState('')
  const [recovery, setRecovery] = useState(false)
  // 같이 쓰는 컴퓨터일 수 있으니 기본은 끈다.
  const [remember, setRemember] = useState(false)
  const [pending, setPending] = useState(false)
  const [error, setError] = useState<MessageKey | null>(null)

  async function submit(value = code) {
    if (pending) return
    setPending(true)
    setError(null)
    const r = await api.post('/api/login/mfa', { ticket, code: value, remember })
    if (r.ok) return enterApp()
    setPending(false)
    setCode('')
    // 시도 한도(429)는 기다리라고 알리고, 처음부터 다시 할 수 있게 버튼도 보인다.
    setError(r.status === 401 && r.code === 'expired' ? 'mfa.expired' : r.status === 429 ? 'auth.locked' : 'mfa.wrong')
  }

  async function passkey() {
    const r = await loginWithPasskey()
    if (r === 'ok') return enterApp()
    if (r === 'failed') setError('auth.passkeyFailed')
  }

  return (
    <AuthLayout>
      <form
        className="auth-card"
        onSubmit={(e) => {
          e.preventDefault()
          void submit()
        }}
      >
        <span className="status-icon" aria-hidden="true">
          <Icon name="phonelink_lock" />
        </span>
        <h1>{t('mfa.title')}</h1>
        <p className="lead">{t('mfa.lead')}</p>
        {recovery ? (
          <label className="field-label">
            {t('mfa.recoveryLabel')}
            <input
              className="input code-text"
              autoComplete="one-time-code"
              autoCapitalize="none"
              spellCheck={false}
              autoFocus
              placeholder="xxxxx-xxxxx"
              value={code}
              onChange={(e) => setCode(e.target.value)}
            />
          </label>
        ) : (
          <CodeInput value={code} onChange={setCode} onComplete={(v) => void submit(v)} label={t('verify.code')} disabled={pending} autoFocus />
        )}
        <label className="check-row">
          <input type="checkbox" checked={remember} onChange={(e) => setRemember(e.target.checked)} />
          {t('mfa.remember')}
        </label>
        {error && (
          <p className="form-error" role="alert">
            {t(error)}
          </p>
        )}
        {(recovery || error === 'mfa.wrong') && (
          <button className="btn btn-primary btn-lg" disabled={pending || code.trim().length < 6}>
            {t('mfa.submit')}
          </button>
        )}
        {passkeySupported && (
          <button type="button" className="btn btn-lg btn-plain" onClick={() => void passkey()}>
            <Icon name="passkey" />
            {t('mfa.usePasskey')}
          </button>
        )}
        <button
          type="button"
          className="auth-link"
          onClick={() => {
            setRecovery((r) => !r)
            setCode('')
            setError(null)
          }}
        >
          {recovery ? t('mfa.useApp') : t('mfa.useRecovery')}
        </button>
        {(error === 'mfa.expired' || error === 'auth.locked') && (
          <button type="button" className="btn btn-outline" onClick={onRestart}>
            {t('mfa.otherAccount')}
          </button>
        )}
      </form>
    </AuthLayout>
  )
}

// ── 가입 ─────────────────────────────────────────────

export function Signup() {
  const { t, locale } = useI18n()
  const feat = useFeatures()
  const [name, setName] = useState('')
  const [email, setEmail] = useState('')
  const [password, setPassword] = useState('')
  const [pending, setPending] = useState(false)
  const [error, setError] = useState<MessageKey | null>(null)

  async function submit(e: FormEvent) {
    e.preventDefault()
    const addr = email.trim().toLowerCase()
    if (!EMAIL.test(addr)) return setError('auth.emailInvalid')
    if ([...password].length < 8) return setError('auth.passwordShort')
    setPending(true)
    setError(null)
    const r = await api.post('/api/signup', { email: addr, password, name: name.trim(), locale })
    setPending(false)
    if (!r.ok) return setError(messageFor(r.code))
    setPendingSignup(addr, password)
    navigate(`/verify?email=${encodeURIComponent(addr)}`)
  }

  return (
    <AuthLayout>
      <form className="auth-card" onSubmit={submit}>
        <h1>{t('auth.signupTitle')}</h1>
        <p className="lead">{t('auth.signupLead')}</p>
        {feat.google && (
          <>
            <GoogleButton />
            <Divider label={t('auth.orEmail')} />
          </>
        )}
        <div className="field-stack">
          <label>
            <span>{t('auth.name')}</span>
            <input autoComplete="name" value={name} maxLength={60} onChange={(e) => setName(e.target.value)} />
          </label>
          <label>
            <span>{t('auth.email')}</span>
            <input
              type="email"
              autoComplete="email"
              autoCapitalize="none"
              spellCheck={false}
              required
              placeholder="name@company.com"
              value={email}
              onChange={(e) => setEmail(e.target.value)}
            />
          </label>
          <label>
            <span>{t('auth.password')}</span>
            <input
              type="password"
              autoComplete="new-password"
              required
              minLength={8}
              placeholder={t('auth.passwordHint')}
              value={password}
              onChange={(e) => setPassword(e.target.value)}
            />
          </label>
        </div>
        <p className="auth-foot left">
          <Rich
            text={t('auth.agree')}
            parts={{
              terms: (
                <a href={TERMS_URL} target="_blank" rel="noreferrer">
                  {t('auth.terms')}
                </a>
              ),
              privacy: (
                <a href={PRIVACY_URL} target="_blank" rel="noreferrer">
                  {t('auth.privacy')}
                </a>
              ),
            }}
          />
        </p>
        {error && (
          <p className="form-error" role="alert">
            {t(error)}
          </p>
        )}
        <button className="btn btn-primary btn-lg" disabled={pending}>
          {pending ? t('auth.creating') : t('auth.signupSubmit')}
        </button>
        <AuthSwitch prompt={t('auth.haveAccount')} to="/login" label={t('auth.hostLogin')} />
      </form>
    </AuthLayout>
  )
}

// ── 이메일 인증 ───────────────────────────────────────

const RESEND_WAIT = 60

export function Verify() {
  const { t } = useI18n()
  const email = (queryParam('email') ?? '').trim().toLowerCase()
  const [code, setCode] = useState(() => (queryParam('code') ?? '').replace(/\D/g, '').slice(0, 6))
  // 가입 화면에서 바로 왔으면 비밀번호를 메모리에 들고 있다. 메일 링크로 왔으면 한 번 더 묻는다.
  const remembered = pendingPassword(email)
  const [password, setPassword] = useState(remembered ?? '')
  const [pending, setPending] = useState(false)
  const [error, setError] = useState<MessageKey | null>(null)
  const [notice, setNotice] = useState<MessageKey | null>(null)
  const [wait, setWait] = useState(RESEND_WAIT)
  // 메일 링크의 코드가 주소창·기록에 남지 않게 지운다(값은 상태에 들고 있다).
  useEffect(() => {
    if (queryParam('code')) replacePath(`/verify?email=${encodeURIComponent(email)}`)
  }, [email])

  useEffect(() => {
    if (wait <= 0) return
    const id = setTimeout(() => setWait((w) => w - 1), 1000)
    return () => clearTimeout(id)
  }, [wait])

  async function submit(value = code) {
    if (pending || value.length !== 6 || !password) return
    setPending(true)
    setError(null)
    const r = await api.post('/api/verify', { email, code: value, password })
    if (r.ok) {
      clearPendingSignup()
      return enterApp()
    }
    setPending(false)
    setError(r.code === 'password' ? 'verify.wrongPassword' : 'verify.wrongCode')
    if (r.code !== 'password') setCode('')
  }

  async function resend() {
    setWait(RESEND_WAIT)
    setError(null)
    const r = await api.post('/api/verify/resend', { email })
    setNotice(r.ok ? 'verify.resent' : messageFor(r.code))
  }

  return (
    <AuthLayout>
      <form
        className="auth-card"
        onSubmit={(e) => {
          e.preventDefault()
          void submit()
        }}
      >
        <span className="status-icon dot" aria-hidden="true">
          <Icon name="mail" />
        </span>
        <h1>{t('verify.title')}</h1>
        <p className="lead">
          <Rich text={t('verify.sent')} parts={{ email: <b>{email}</b> }} />
        </p>
        <CodeInput value={code} onChange={setCode} onComplete={(v) => void submit(v)} label={t('verify.code')} disabled={pending} autoFocus={!!remembered} />
        {!remembered && (
          <label className="field-label">
            {t('verify.passwordPrompt')}
            <input className="input" type="password" autoComplete="current-password" value={password} onChange={(e) => setPassword(e.target.value)} autoFocus />
          </label>
        )}
        {error && (
          <p className="form-error" role="alert">
            {t(error)}
          </p>
        )}
        {notice && !error && (
          <p className="form-note" role="status">
            {t(notice)}
          </p>
        )}
        {(!remembered || error === 'verify.wrongPassword') && (
          <button className="btn btn-primary btn-lg" disabled={pending || code.length !== 6 || !password}>
            {t('verify.submit')}
          </button>
        )}
        <p className="hint-card">
          <Icon name="info" />
          {t('verify.spam')}
        </p>
        <p className="auth-foot">
          {t('verify.noMail')}{' '}
          {wait > 0 ? (
            <span>{t('verify.resendIn', { n: String(wait) })}</span>
          ) : (
            <button type="button" className="link-btn" onClick={() => void resend()}>
              {t('verify.resend')}
            </button>
          )}
          {' · '}
          <InLink to="/signup">{t('verify.changeEmail')}</InLink>
        </p>
      </form>
    </AuthLayout>
  )
}

// ── 비밀번호 재설정 ───────────────────────────────────

export function Forgot() {
  const { t } = useI18n()
  const [email, setEmail] = useState('')
  const [sent, setSent] = useState<string | null>(null)
  const [pending, setPending] = useState(false)
  const [error, setError] = useState<MessageKey | null>(null)

  async function submit(e: FormEvent) {
    e.preventDefault()
    const addr = email.trim().toLowerCase()
    if (!EMAIL.test(addr)) return setError('auth.emailInvalid')
    setPending(true)
    setError(null)
    const r = await api.post('/api/password/forgot', { email: addr })
    setPending(false)
    if (r.ok) setSent(addr)
    else setError(messageFor(r.code))
  }

  return (
    <AuthLayout>
      <form className="auth-card" onSubmit={submit}>
        <span className="status-icon" aria-hidden="true">
          <Icon name="lock_reset" />
        </span>
        <h1>{t('forgot.title')}</h1>
        {sent ? (
          <p className="lead" role="status">
            <Rich text={t('forgot.sent')} parts={{ email: <b>{sent}</b> }} />
          </p>
        ) : (
          <>
            <p className="lead">{t('forgot.lead')}</p>
            <label className="field-label">
              {t('auth.email')}
              <input className="input" type="email" autoComplete="email" autoFocus required value={email} onChange={(e) => setEmail(e.target.value)} />
            </label>
            {error && (
              <p className="form-error" role="alert">
                {t(error)}
              </p>
            )}
            <button className="btn btn-primary btn-lg" disabled={pending || !email.trim()}>
              {t('forgot.submit')}
            </button>
          </>
        )}
        <InLink to="/login" className="auth-link">
          {t('forgot.back')}
        </InLink>
      </form>
    </AuthLayout>
  )
}

export function Reset() {
  const { t } = useI18n()
  const [token] = useState(() => queryParam('token') ?? '')
  const [uid] = useState(() => Number(queryParam('uid') ?? '0'))
  const [password, setPassword] = useState('')
  const [again, setAgain] = useState('')
  const [pending, setPending] = useState(false)
  const [error, setError] = useState<MessageKey | null>(null)
  const [breached, setBreached] = useState<boolean | null>(null)
  // 링크의 토큰이 주소창·기록에 오래 남지 않게 지운다(값은 상태에 들고 있다).
  useEffect(() => replacePath('/reset'), [])

  const longEnough = [...password].length >= 8
  const same = password.length > 0 && password === again

  async function submit(e: FormEvent) {
    e.preventDefault()
    if (!longEnough || !same) return
    setError(null)
    if (!token || !Number.isSafeInteger(uid) || uid <= 0) return setError('reset.invalid')
    setPending(true)
    const r = await api.post<{ mfa?: string } | null>('/api/password/reset', { uid, token, password })
    setPending(false)
    if (r.ok) {
      if (r.data?.mfa) return navigate(`/login?mfa=${r.data.mfa}`)
      return enterApp()
    }
    if (r.code === 'breached') {
      setBreached(true)
      return setError('auth.breached')
    }
    setError(r.code === 'token' ? 'reset.invalid' : messageFor(r.code))
  }

  const rule = (ok: boolean | null, key: MessageKey) => (
    <li className={ok ? 'ok' : ok === false ? 'bad' : ''}>
      <Icon name={ok === false ? 'close' : 'check'} />
      {t(key)}
    </li>
  )

  return (
    <AuthLayout>
      <form className="auth-card" onSubmit={submit}>
        <h1>{t('reset.title')}</h1>
        <div className="field-stack">
          <label>
            <span>{t('reset.new')}</span>
            <input type="password" autoComplete="new-password" autoFocus value={password} onChange={(e) => { setPassword(e.target.value); setBreached(null) }} />
          </label>
          <label>
            <span>{t('reset.confirm')}</span>
            <input type="password" autoComplete="new-password" value={again} onChange={(e) => setAgain(e.target.value)} />
          </label>
        </div>
        <ul className="rules">
          {rule(password ? longEnough : null, 'reset.ruleLength')}
          {rule(again ? same : null, 'reset.ruleMatch')}
          {rule(breached === null ? null : !breached, 'reset.ruleBreach')}
        </ul>
        {error && (
          <p className="form-error" role="alert">
            {t(error)}
          </p>
        )}
        <button className="btn btn-primary btn-lg" disabled={pending || !longEnough || !same}>
          {t('reset.submit')}
        </button>
        <p className="auth-foot">{t('reset.note')}</p>
        {error === 'reset.invalid' && (
          <InLink to="/forgot" className="auth-link">
            {t('forgot.submit')}
          </InLink>
        )}
      </form>
    </AuthLayout>
  )
}
