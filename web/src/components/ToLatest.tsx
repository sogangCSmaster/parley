import { useI18n } from '../lib/i18n'
import { Icon } from './Icon'

/** 지난 대화를 읽는 중에만 뜨는 "최신 대화로" 버튼. 새 줄이 있으면 그 수를 함께 보인다. */
export function ToLatest({ newCount, onClick }: { newCount: number; onClick: () => void }) {
  const { t } = useI18n()
  return (
    <button className="to-latest" onClick={onClick}>
      <Icon name="arrow_downward" />
      {t('history.latest')}
      {newCount > 0 && <span className="badge">{t('history.newCount', { n: String(newCount) })}</span>}
    </button>
  )
}
