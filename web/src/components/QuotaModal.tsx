import { useEffect, useRef, useState } from 'react'
import { api, type Billing } from '../lib/api'
import { formatDate } from '../lib/format'
import { useI18n } from '../lib/i18n'
import { useModalDialog } from '../lib/modal'

/**
 * 주최자의 이번 달 번역 시간이 떨어졌을 때(주최자에게만). 영상·음성은 계속되고 자막만 멈춘다.
 * 결제는 새 탭에서 한다(이 탭을 떠나면 회의가 끊기므로). 결제가 들어오면 서버가 알려 자막이 다시 켜진다.
 */
export function QuotaModal({ onClose }: { onClose: () => void }) {
  const { t, locale } = useI18n()
  const ref = useRef<HTMLDialogElement>(null)
  const [billing, setBilling] = useState<Billing | null>(null)
  useModalDialog(ref)
  useEffect(() => {
    void api.get<Billing>('/api/billing').then((r) => r.ok && setBilling(r.data))
  }, [])
  const hours = Math.round((billing?.includedSec ?? 0) / 3600)
  return (
    <dialog ref={ref} className="sheet quota" aria-labelledby="quota-title" onClose={onClose}>
      <span className="quota-ring" aria-hidden="true">
        {hours ? t('checkout.hours', { n: String(hours) }) : ''}
      </span>
      <h2 id="quota-title">{t('quota.title')}</h2>
      <p className="lead">{billing ? t('quota.body', { date: formatDate(billing.resetsAt, locale) }) : t('quota.bodyShort')}</p>
      <div className="quota-options">
        <a className="quota-option primary" href="/checkout?item=hours&q=1" target="_blank" rel="noreferrer">
          <span>{t('quota.addOne')}</span>
          <b>$3</b>
        </a>
        <a className="quota-option" href="/checkout?item=hours&q=5" target="_blank" rel="noreferrer">
          <span>{t('quota.addFive')}</span>
          <b>$15</b>
        </a>
        {billing?.plan === 'basic' && (
          <a className="quota-option" href="/checkout?item=pro" target="_blank" rel="noreferrer">
            <span>{t('quota.toPro')}</span>
            <b>{t('quota.proPrice')}</b>
          </a>
        )}
      </div>
      <button className="btn btn-quiet" onClick={() => ref.current?.close()}>
        {t('quota.continue')}
      </button>
    </dialog>
  )
}
