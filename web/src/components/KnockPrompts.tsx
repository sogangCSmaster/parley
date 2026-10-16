import { useI18n } from '../lib/i18n'
import type { Knock } from '../protocol/Knock'
import { Icon } from './Icon'

type Props = { knocks: Knock[]; onAdmit: (id: string) => void; onDeny: (id: string) => void }

/** 주최자에게만 보이는 입장 요청 카드. 여러 명이면 쌓아서 보여준다. */
export function KnockPrompts({ knocks, onAdmit, onDeny }: Props) {
  const { t } = useI18n()
  if (knocks.length === 0) return null
  return (
    <div className="knocks" role="region" aria-live="assertive" aria-label={t('room.participants')}>
      {knocks.map((k) => (
        <div className="knock" key={k.id}>
          <span className="knock-icon" aria-hidden="true">
            <Icon name="person_add" />
          </span>
          <p>{t('knock.title', { name: k.name })}</p>
          <div className="knock-actions">
            <button className="btn btn-text" onClick={() => onDeny(k.id)}>
              {t('knock.deny')}
            </button>
            <button className="btn btn-primary" onClick={() => onAdmit(k.id)} autoFocus>
              {t('knock.admit')}
            </button>
          </div>
        </div>
      ))}
    </div>
  )
}
