import { useEffect, useRef, useState } from 'react'
import { logout, useAuth } from '../lib/auth'
import { useI18n } from '../lib/i18n'
import { navigate, usePath } from '../lib/router'
import { BrandMark } from './BrandMark'
import { Icon } from './Icon'
import { LocaleSelect } from './LocaleSelect'

export function Header() {
  const { t } = useI18n()
  const auth = useAuth()
  const path = usePath()
  return (
    <header className="app-header">
      <a
        className="brand"
        href="/"
        onClick={(e) => {
          e.preventDefault()
          navigate('/')
        }}
      >
        <BrandMark />
        tolkmeet
      </a>
      <div className="header-actions">
        <LocaleSelect />
        {auth.status === 'guest' && path !== '/login' && (
          <button className="btn btn-quiet" onClick={() => navigate('/login')}>
            {t('auth.hostLogin')}
          </button>
        )}
        {auth.status === 'host' && <AccountMenu username={auth.username} />}
      </div>
    </header>
  )
}

/** 계정 버튼(사람 아이콘): 누르면 이름과 로그아웃이 펼쳐진다. 사진은 받지 않는다. */
function AccountMenu({ username }: { username: string }) {
  const { t } = useI18n()
  const [open, setOpen] = useState(false)
  const [loggingOut, setLoggingOut] = useState(false)
  const [failed, setFailed] = useState(false)
  const rootRef = useRef<HTMLDivElement>(null)
  const buttonRef = useRef<HTMLButtonElement>(null)

  // 바깥을 누르거나 Esc로 닫는다. Esc로 닫으면 키보드 포커스를 계정 버튼으로 돌려준다.
  useEffect(() => {
    if (!open) return
    const onDown = (e: PointerEvent) => {
      if (!rootRef.current?.contains(e.target as Node)) setOpen(false)
    }
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== 'Escape') return
      setOpen(false)
      buttonRef.current?.focus()
    }
    document.addEventListener('pointerdown', onDown)
    document.addEventListener('keydown', onKey)
    return () => {
      document.removeEventListener('pointerdown', onDown)
      document.removeEventListener('keydown', onKey)
    }
  }, [open])

  async function handleLogout() {
    if (loggingOut) return
    setLoggingOut(true)
    setFailed(false)
    const ok = await logout()
    setFailed(!ok)
    setLoggingOut(false)
  }

  return (
    <div className="account" ref={rootRef}>
      {/* 펼침 버튼(디스클로저): 메뉴 키보드 규칙 없이 Tab으로 안의 버튼에 닿는다. */}
      <button
        ref={buttonRef}
        className="account-btn"
        aria-label={t('auth.account')}
        aria-expanded={open}
        aria-controls="account-menu"
        onClick={() => setOpen((o) => !o)}
      >
        <Icon name="person" />
      </button>
      {open && (
        <div className="account-menu" id="account-menu">
          <p className="account-name">{username}</p>
          <button className="account-item" onClick={() => void handleLogout()} disabled={loggingOut}>
            <Icon name="logout" />
            {t('auth.logout')}
          </button>
          {failed && (
            <p className="form-error" role="alert">
              {t('auth.logoutFailed')}
            </p>
          )}
        </div>
      )}
    </div>
  )
}
