import { useI18n } from '../lib/i18n'
import type { Prefs } from '../lib/prefs'
import { Icon } from './Icon'
import { LanguageSelects } from './LanguageSelects'
import { Switch } from './Switch'

type Props = {
  prefs: Prefs
  translation: boolean
  isHost: boolean
  onPrefsChange: (p: Prefs) => void
  onTranslationChange: (on: boolean) => void
  onClose: () => void
}

export function CaptionPanel({ prefs, translation, isHost, onPrefsChange, onTranslationChange, onClose }: Props) {
  const { t } = useI18n()
  return (
    <aside className="panel" aria-label={t('captions.title')}>
      <div className="panel-head">
        <h2>{t('captions.title')}</h2>
        <button className="icon-btn" aria-label={t('captions.close')} onClick={onClose}>
          <Icon name="close" />
        </button>
      </div>
      {isHost && (
        <div className="panel-host">
          <Switch checked={translation} onChange={onTranslationChange} label={t('captions.translation')} />
        </div>
      )}
      {translation ? (
        <>
          <LanguageSelects prefs={prefs} onChange={onPrefsChange} />
          <div className="feed" role="log" aria-live="polite">
            <p className="feed-empty">{t('captions.empty')}</p>
          </div>
        </>
      ) : (
        <div className="feed">
          <p className="feed-empty">
            {t('captions.off')}
            {isHost && (
              <>
                <br />
                {t('captions.offHost')}
              </>
            )}
          </p>
        </div>
      )}
    </aside>
  )
}
