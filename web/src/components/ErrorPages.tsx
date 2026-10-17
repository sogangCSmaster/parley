import { useState } from 'react'
import { useAuth } from '../lib/auth'
import { useI18n } from '../lib/i18n'
import { chromeIntentUrl } from '../lib/inapp'
import { navigate } from '../lib/router'
import { Icon } from './Icon'
import { StatusPage } from './StatusPage'

/** 회의 링크 모양이 아닌 주소 */
export function NotFoundPage() {
  const { t } = useI18n()
  return (
    <StatusPage
      icon="brand"
      title={t('status.notFoundTitle')}
      body={t('status.notFoundBody')}
      actions={
        <button className="btn btn-primary" onClick={() => navigate('/')}>
          {t('room.home')}
        </button>
      }
    />
  )
}

/** 서버에 없는 회의(모두 나가고 30분이 지나 닫혔거나 잘못된 코드). 주최자는 바로 새 회의를 만들 수 있게 홈으로 보낸다. */
export function MeetingGonePage() {
  const { t } = useI18n()
  const auth = useAuth()
  return (
    <StatusPage
      icon="event_busy"
      title={t('status.goneTitle')}
      body={t('status.goneBody')}
      actions={
        <button className="btn btn-primary" onClick={() => navigate('/')}>
          {auth.status === 'host' ? t('home.newMeeting') : t('room.home')}
        </button>
      }
    />
  )
}

/** 마이크 권한이 막혔을 때. 허용 방법을 보여 주고, 듣기만 하고 들어갈 수도 있게 한다. */
export function MicBlockedPage({ onRetry, onListenOnly }: { onRetry: () => void; onListenOnly: () => void }) {
  const { t } = useI18n()
  return (
    <StatusPage
      icon="mic_off"
      title={t('mic.title')}
      body={t('mic.body')}
      actions={
        <>
          <button className="btn btn-outline" onClick={onListenOnly}>
            {t('mic.listenOnly')}
          </button>
          <button className="btn btn-primary" onClick={onRetry}>
            {t('room.retry')}
          </button>
        </>
      }
    >
      <ol className="status-card status-steps">
        <li>{t('mic.step1')}</li>
        <li>{t('mic.step2')}</li>
        <li>{t('mic.step3')}</li>
      </ol>
    </StatusPage>
  )
}

/** 메신저 앱 안의 브라우저. 안드로이드는 Chrome으로 바로 열고, 그 밖에는 링크를 복사해 다른 브라우저에 붙이게 한다. */
export function InAppBrowserPage() {
  const { t } = useI18n()
  const [copied, setCopied] = useState(false)
  const intent = chromeIntentUrl()

  async function copy() {
    try {
      await navigator.clipboard.writeText(location.href)
      setCopied(true)
    } catch {
      // 클립보드가 막힌 WebView도 있다. 주소를 선택해 직접 복사할 수 있게 보여 준다.
      window.prompt(t('room.copyLink'), location.href)
    }
  }

  return (
    <StatusPage
      icon="language"
      title={t('inapp.title')}
      body={t('inapp.body')}
      actions={
        <>
          {intent && (
            <a className="btn btn-outline" href={intent}>
              <Icon name="open_in_new" />
              {t('inapp.openChrome')}
            </a>
          )}
          <button className="btn btn-primary" onClick={copy}>
            <Icon name={copied ? 'check' : 'content_copy'} />
            {copied ? t('room.linkCopied') : t('room.copyLink')}
          </button>
        </>
      }
    >
      <ul className="status-chips">
        <li>Chrome</li>
        <li>Safari</li>
        <li>Edge</li>
      </ul>
    </StatusPage>
  )
}
