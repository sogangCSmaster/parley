import { useI18n } from '../lib/i18n'
import { languageName } from '../lib/languages'
import type { Knock } from '../protocol/Knock'
import { Icon } from './Icon'

type Props = { knocks: Knock[]; onAdmit: (id: string) => void; onDeny: (id: string) => void }

/**
 * 주최자에게만 보이는 입장 요청 카드(화면 위 가운데, 유리 면). 먼저 온 사람부터 한 명씩 보이고,
 * 뒤에 기다리는 사람이 있으면 그 수를 함께 적는다. 말하는 언어를 보여 누구인지 가늠하게 한다.
 */
export function KnockPrompts({ knocks, onAdmit, onDeny }: Props) {
  const { t } = useI18n()
  const k = knocks[0]
  if (!k) return null
  return (
    <div className="knocks" role="region" aria-live="assertive" aria-label={t('knock.title', { name: k.name })}>
      <div className="knock" key={k.id}>
        <span className="knock-avatar" aria-hidden="true">
          {[...k.name][0] ?? '?'}
        </span>
        <div className="knock-text">
          <p>{t('knock.title', { name: k.name })}</p>
          <small>
            {k.lang && t('knock.speaks', { lang: languageName(k.lang) })}
            {knocks.length > 1 && (
              <>
                {k.lang && ' · '}
                {t('knock.waiting', { n: String(knocks.length - 1) })}
              </>
            )}
          </small>
        </div>
        <div className="knock-actions">
          {/* 주황(빨강 계열)은 "나가기·취소"처럼 읽혀서, 수락은 흰 버튼 + 체크, 거절은 회색 유리 + X로 확실히 가른다. */}
          <button className="btn btn-glass" onClick={() => onDeny(k.id)}>
            <Icon name="close" />
            {t('knock.deny')}
          </button>
          <button className="btn btn-admit" onClick={() => onAdmit(k.id)} autoFocus>
            <Icon name="check" />
            {t('knock.admit')}
          </button>
        </div>
      </div>
    </div>
  )
}
