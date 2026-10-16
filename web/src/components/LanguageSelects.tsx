import { useI18n } from '../lib/i18n'
import { LANGUAGES } from '../lib/languages'
import type { Prefs } from '../lib/prefs'
import { Icon } from './Icon'
import { Select } from './Select'

const OPTIONS = LANGUAGES.map((l) => ({ value: l.code as string, label: l.name }))

export function LanguageSelects({ prefs, onChange }: { prefs: Prefs; onChange: (p: Prefs) => void }) {
  const { t } = useI18n()
  return (
    <div className="langs">
      <Select
        label={t('lang.mine')}
        value={prefs.lang}
        options={OPTIONS}
        onChange={(lang) => onChange({ ...prefs, lang })}
      />
      <button
        type="button"
        className="icon-btn swap"
        aria-label={t('lang.swap')}
        onClick={() => onChange({ ...prefs, lang: prefs.partnerLang, partnerLang: prefs.lang })}
      >
        <Icon name="sync_alt" />
      </button>
      <Select
        label={t('lang.partner')}
        value={prefs.partnerLang}
        options={OPTIONS}
        onChange={(partnerLang) => onChange({ ...prefs, partnerLang })}
      />
    </div>
  )
}
