import { LOCALES, setLocale, useI18n } from '../lib/i18n'
import { Select } from './Select'

const OPTIONS = LOCALES.map((l) => ({ value: l.code, label: l.name }))

export function LocaleSelect() {
  const { t, locale } = useI18n()
  return (
    <Select
      variant="pill"
      icon="language"
      ariaLabel={t('ui.language')}
      value={locale}
      options={OPTIONS}
      onChange={setLocale}
    />
  )
}
