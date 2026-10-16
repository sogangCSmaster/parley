import { useState } from 'react'
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
  const [loggingOut, setLoggingOut] = useState(false)
  const [logoutFailed, setLogoutFailed] = useState(false)

  async function handleLogout() {
    if (loggingOut) return
    setLoggingOut(true)
    setLogoutFailed(false)
    setLogoutFailed(!(await logout()))
    setLoggingOut(false)
  }
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
        parley
      </a>
      <div className="header-actions">
        {auth.status === 'guest' && path !== '/login' && (
          <button className="btn btn-text" onClick={() => navigate('/login')}>
            {t('auth.hostLogin')}
          </button>
        )}
        {auth.status === 'host' && (
          <>
            <span className="header-user">
              <Icon name="account_circle" />
              {auth.username}
            </span>
            <button className="btn btn-quiet" onClick={() => void handleLogout()} disabled={loggingOut}>
              {t('auth.logout')}
            </button>
            {logoutFailed && <span className="header-error form-error" role="alert">{t('auth.logoutFailed')}</span>}
          </>
        )}
        <LocaleSelect />
      </div>
    </header>
  )
}
