import { useEffect, useState } from 'react'
import { api, createRoom, type Plan, type User } from '../lib/api'
import { features, refreshAuth, setUser, useAuth } from '../lib/auth'
import { useI18n, type MessageKey } from '../lib/i18n'
import '../lib/i18nPages'
import { legalUrl } from '../lib/links'
import { navigate, queryParam } from '../lib/router'
import { BrandMark } from './BrandMark'
import { Header } from './Header'
import { Icon } from './Icon'
import { Rich } from './Rich'

type Item = 'basic' | 'pro' | 'hours'

type CheckoutData = {
  clientToken: string
  sandbox: boolean
  priceId: string
  quantity: number
  email: string
  customData: Record<string, string>
}

type PaddleGlobal = {
  Environment: { set: (env: string) => void }
  Initialize: (o: { token: string; eventCallback?: (e: { name: string }) => void }) => void
  Checkout: { open: (o: object) => void }
}

/** Paddle.js는 결제 화면에서만 받는다(다른 화면의 첫 로드에 들어가지 않게). */
let paddleLoading: Promise<PaddleGlobal> | null = null
/** Paddle.js v2는 한 번만 초기화한다(결제 창을 닫았다 다시 열 때 또 부르지 않게). */
let paddleReady = false
function loadPaddle(): Promise<PaddleGlobal> {
  paddleLoading ??= new Promise((resolve, reject) => {
    const s = document.createElement('script')
    s.src = 'https://cdn.paddle.com/paddle/v2/paddle.js'
    s.async = true
    s.onload = () => {
      const p = (window as unknown as { Paddle?: PaddleGlobal }).Paddle
      if (p) resolve(p)
      else reject(new Error('paddle'))
    }
    s.onerror = () => {
      paddleLoading = null
      reject(new Error('paddle'))
    }
    document.head.appendChild(s)
  })
  return paddleLoading
}

/** 지금 화면이 어두운지(시스템 설정을 따르는 경우 포함) */
function isDark(): boolean {
  const chosen = document.documentElement.dataset.theme
  return chosen ? chosen === 'dark' : matchMedia('(prefers-color-scheme: dark)').matches
}

const PRICE: Record<Exclude<Item, 'hours'>, string> = { basic: '$19', pro: '$39' }
const HOURS = [1, 5, 10]

export function Checkout() {
  const { t, locale } = useI18n()
  const auth = useAuth()
  const [item, setItem] = useState<Item>(() => {
    const q = queryParam('item') ?? queryParam('plan')
    return q === 'pro' || q === 'hours' ? q : 'basic'
  })
  const [qty, setQty] = useState(() => {
    const q = Number(queryParam('q'))
    return HOURS.includes(q) ? q : 5
  })
  const [busy, setBusy] = useState(false)
  // 결제 정보는 판매 대행사 Paddle(영국)에 제공·국외 이전되므로 결제 전에 따로 동의를 받는다(개인정보 보호법 제17조·제28조의8).
  const [consentProvide, setConsentProvide] = useState(false)
  const [consentTransfer, setConsentTransfer] = useState(false)
  const consent = consentProvide && consentTransfer
  const [error, setError] = useState<MessageKey | null>(null)

  useEffect(() => {
    if (auth.status === 'guest') navigate(`/signup`)
  }, [auth.status])
  if (auth.status !== 'host') return <div className="shell" />
  const user = auth.user
  // 요금제가 없으면 시간 추가는 살 수 없다(구독부터).
  const sel: Item = item === 'hours' && user.plan === 'none' ? 'basic' : item
  // 이미 구독 중이면 다른 요금제는 "바꾸기"(남은 기간만큼 정산)로 처리한다.
  const changing = sel !== 'hours' && user.plan !== 'none' && user.plan !== sel
  const same = sel !== 'hours' && user.plan === sel

  async function pay() {
    setBusy(true)
    setError(null)
    if (changing) {
      const r = await api.post('/api/billing/change', { plan: sel })
      setBusy(false)
      if (!r.ok) return setError('account.billingFailed')
      return navigate(`/billing/done?item=${sel}`)
    }
    const r = await api.post<CheckoutData | { dev: true }>('/api/billing/checkout', {
      item: sel,
      quantity: sel === 'hours' ? qty : 1,
      consentProvide: consentProvide,
      consentTransfer: consentTransfer,
    })
    if (!r.ok) {
      setBusy(false)
      // 업데이트 전에 열어 둔 화면은 동의 값을 보내지 않으므로 새로고침을 안내한다.
      return setError(r.code === 'consent' ? 'checkout.consentNeeded' : 'account.billingFailed')
    }
    if ('dev' in r.data) {
      // 개발 모드: 결제 없이 켠다.
      const d = await api.post('/api/billing/dev', { item: sel, quantity: qty })
      setBusy(false)
      if (!d.ok) return setError('account.billingFailed')
      return navigate(`/billing/done?item=${sel}${sel === 'hours' ? `&q=${qty}` : ''}`)
    }
    const data = r.data
    try {
      const paddle = await loadPaddle()
      if (!paddleReady) {
        if (data.sandbox) paddle.Environment.set('sandbox')
        paddle.Initialize({ token: data.clientToken })
        paddleReady = true
      }
      paddle.Checkout.open({
        items: [{ priceId: data.priceId, quantity: data.quantity }],
        customer: { email: data.email },
        customData: data.customData,
        settings: {
          displayMode: 'overlay',
          theme: isDark() ? 'dark' : 'light',
          locale: document.documentElement.lang || 'en',
          successUrl: `${location.origin}/billing/done?item=${sel}${sel === 'hours' ? `&q=${qty}` : ''}`,
        },
      })
    } catch {
      setError('checkout.loadFailed')
    }
    setBusy(false)
  }

  const priceLabel = sel === 'hours' ? `$${qty * 3}` : PRICE[sel]
  return (
    <div className="shell">
      <Header />
      <main className="checkout">
        <section className="checkout-main" aria-labelledby="checkout-title">
          <h1 id="checkout-title">{t(sel === 'hours' ? 'checkout.hoursTitle' : sel === 'pro' ? 'checkout.proTitle' : 'checkout.basicTitle')}</h1>
          <div className="choice-grid" role="radiogroup" aria-label={t('account.plan')}>
            {(['basic', 'pro'] as const).map((p) => (
              <button key={p} type="button" role="radio" aria-checked={sel === p} className={`choice${sel === p ? ' on' : ''}`} onClick={() => setItem(p)}>
                <span className="choice-top">
                  {p === 'pro' ? 'Pro' : 'Basic'}
                  {user.plan === p && <span className="badge-soft">{t('checkout.current')}</span>}
                </span>
                <b>
                  {PRICE[p]}
                  <small> / {t('checkout.month')}</small>
                </b>
                <small>{t(p === 'pro' ? 'checkout.proHours' : 'checkout.basicHours')}</small>
              </button>
            ))}
            {user.plan !== 'none' && (
              <button type="button" role="radio" aria-checked={sel === 'hours'} className={`choice${sel === 'hours' ? ' on' : ''}`} onClick={() => setItem('hours')}>
                <span className="choice-top">{t('account.addHours')}</span>
                <b>
                  $3
                  <small> / {t('checkout.hour')}</small>
                </b>
                <small>{t('checkout.hoursNote')}</small>
              </button>
            )}
          </div>
          {sel === 'hours' && (
            <div className="segmented" role="radiogroup" aria-label={t('checkout.howMany')}>
              {HOURS.map((h) => (
                <button key={h} type="button" role="radio" aria-checked={qty === h} className={qty === h ? 'on' : ''} onClick={() => setQty(h)}>
                  {t('checkout.hours', { n: String(h) })}
                </button>
              ))}
            </div>
          )}
          <ul className="ticks">
            <li>{t(sel === 'pro' ? 'checkout.proHours' : sel === 'basic' ? 'checkout.basicHours' : 'checkout.hoursKeep')}</li>
            <li>{t('checkout.allModes')}</li>
            <li>{t('checkout.guestsFree')}</li>
            <li>{t('checkout.extra')}</li>
          </ul>
          <p className="fine">
            {t('checkout.fine')}{' '}
            <a href={legalUrl('refund', locale)} target="_blank" rel="noreferrer">
              {t('checkout.refund')}
            </a>
          </p>
        </section>
        <section className="checkout-pay" aria-labelledby="pay-title">
          <h2 id="pay-title">{t('checkout.summary')}</h2>
          <div className="pay-line">
            <span>{t('auth.email')}</span>
            <span title={user.email}>{user.email}</span>
          </div>
          <div className="pay-line">
            <span>{sel === 'hours' ? t('checkout.hours', { n: String(qty) }) : `${sel === 'pro' ? 'Pro' : 'Basic'} · ${t('account.monthly')}`}</span>
            <span>{priceLabel}</span>
          </div>
          {changing ? (
            // 요금제 바꾸기는 결제창 없이 등록된 결제 수단으로 남은 기간만큼 바로 정산된다.
            <p className="form-note">{t(sel === 'basic' ? 'checkout.downgradeNote' : 'checkout.changeNote')}</p>
          ) : (
            <div className="pay-line muted">
              <span>{t('checkout.tax')}</span>
              <span>{t('checkout.taxLater')}</span>
            </div>
          )}
          {error && (
            <p className="form-error" role="alert">
              {t(error)}
            </p>
          )}
          {!same && !changing && (
            <div className="consent-box" id="paddle-consent">
              <label className="check-row">
                <input type="checkbox" checked={consentProvide} onChange={(e) => setConsentProvide(e.target.checked)} />
                <span>{t('checkout.consentProvide')}</span>
              </label>
              <label className="check-row">
                <input type="checkbox" checked={consentTransfer} onChange={(e) => setConsentTransfer(e.target.checked)} />
                <span>{t('checkout.consentTransfer')}</span>
              </label>
              <p className="fine">
                <Rich
                  text={t('checkout.consentNote')}
                  parts={{
                    privacy: (
                      <a href={legalUrl('privacy', locale)} target="_blank" rel="noreferrer">
                        {t('checkout.privacyLink')}
                      </a>
                    ),
                  }}
                />
              </p>
            </div>
          )}
          {same ? (
            <p className="form-note">{t('checkout.already')}</p>
          ) : (
            <button
              className="btn btn-primary btn-lg"
              disabled={busy || (!changing && !consent) || (!features().billing && !features().devBilling)}
              aria-describedby={!changing && !consent ? 'paddle-consent' : undefined}
              onClick={() => void pay()}
            >
              {changing ? t('checkout.change', { plan: sel === 'pro' ? 'Pro' : 'Basic' }) : t('checkout.pay', { price: priceLabel })}
            </button>
          )}
          {!changing && (
            <p className="fine center">
              <Icon name="lock_reset" />
              {t('checkout.secure')}
            </p>
          )}
        </section>
      </main>
    </div>
  )
}

export function PaidDone() {
  const { t } = useI18n()
  const auth = useAuth()
  const item = queryParam('item')
  const hours = Number(queryParam('q') ?? '0')
  const [starting, setStarting] = useState(false)
  const user: User | null = auth.status === 'host' ? auth.user : null
  const [round, setRound] = useState(0)
  // 요금제를 샀거나 바꿨으면 그 요금제가 될 때까지, 시간만 샀으면 요금제가 있을 때까지 확인한다.
  const want = item === 'pro' || item === 'basic' ? item : null
  const waiting = (plan: string | undefined) => (want ? plan !== want : plan === 'none')
  const [stalled, setStalled] = useState(false)

  // 결제 결과는 Paddle 웹훅으로 조금 늦게 들어올 수 있다. 요금제가 켜질 때까지 잠깐 다시 확인한다.
  useEffect(() => {
    let tries = 0
    let stop = false
    async function poll() {
      const r = await api.get<{ user: User | null }>('/api/me')
      if (stop) return
      if (r.ok && r.data.user) setUser(r.data.user)
      if (!r.ok || waiting(r.data.user?.plan)) {
        // 15초쯤 지나도 안 들어오면 멈추고, 다시 확인할 수 있게 알린다.
        if (tries++ < 10) setTimeout(() => void poll(), 1500)
        else setStalled(true)
      }
    }
    void poll()
    return () => {
      stop = true
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps -- want은 주소에서 한 번 정해진다
  }, [round])

  async function start() {
    setStarting(true)
    const r = await createRoom(true)
    if (r.kind === 'ok') navigate(`/${r.code}`)
    else {
      setStarting(false)
      void refreshAuth()
    }
  }

  const plan: Plan | null = item === 'pro' || item === 'basic' ? item : null
  return (
    <div className="shell">
      <Header />
      <main className="status-page">
        <span className="status-brand big" aria-hidden="true">
          <BrandMark size={96} />
        </span>
        <div className="status-text">
          <h1>{plan ? t('paid.title', { plan: plan === 'pro' ? 'Pro' : 'Basic' }) : t('paid.hoursTitle', { n: String(hours || 1) })}</h1>
          <p className="lead">{user ? t(plan ? 'paid.lead' : 'paid.hoursLead', { email: user.email, hours: plan === 'pro' ? '30' : '10' }) : ''}</p>
        </div>
        {plan && (
          <ol className="steps3">
            <li>
              <b>{t('paid.step1')}</b>
              <span>{t('paid.step1Body')}</span>
            </li>
            <li>
              <b>{t('paid.step2')}</b>
              <span>{t('paid.step2Body')}</span>
            </li>
            <li>
              <b>{t('paid.step3')}</b>
              <span>{t('paid.step3Body')}</span>
            </li>
          </ol>
        )}
        {stalled && waiting(user?.plan) && (
          <p className="form-note" role="status">
            {t('paid.pending')}{' '}
            <button
              className="link-btn"
              onClick={() => {
                setStalled(false)
                setRound((n) => n + 1)
              }}
            >
              {t('paid.checkAgain')}
            </button>
          </p>
        )}
        <div className="row status-actions">
          <button className="btn btn-primary btn-lg" onClick={() => void start()} disabled={starting || user?.plan === 'none'}>
            {t(plan ? 'paid.firstMeeting' : 'home.startNow')}
          </button>
          <button className="btn btn-outline btn-lg" onClick={() => navigate('/')}>
            {t('room.home')}
          </button>
        </div>
      </main>
    </div>
  )
}
