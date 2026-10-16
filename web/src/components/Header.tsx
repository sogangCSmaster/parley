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
            <button className="btn btn-text" onClick={() => void logout()}>
              {t('auth.logout')}
            </button>
          </>
        )}
        <LocaleSelect />
      </div>
    </header>
  )
}
