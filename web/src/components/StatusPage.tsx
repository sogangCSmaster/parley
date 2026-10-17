import type { ReactNode } from 'react'
import type { IconName } from './icons'
import { BrandMark } from './BrandMark'
import { Header } from './Header'
import { Icon } from './Icon'

type Props = {
  /** 제목 위 흰 원 안의 아이콘. 'spinner'는 진행 중, 'brand'는 기울어진 로고(없는 페이지). */
  icon?: IconName | 'spinner' | 'brand'
  eyebrow?: string
  title: string
  body?: ReactNode
  /** 본문 아래 카드(안내 단계·통계 등) */
  children?: ReactNode
  /** 맨 아래 버튼 줄 */
  actions?: ReactNode
  /** 화면 낭독기에 바뀐 상태를 알린다(대기 화면 등). */
  live?: boolean
}

/** 대기·종료·오류처럼 한 가지 상태를 알리는 가운데 정렬 화면. */
export function StatusPage({ icon, eyebrow, title, body, children, actions, live }: Props) {
  return (
    <div className="shell">
      <Header />
      <main className="status-page" aria-live={live ? 'polite' : undefined}>
        {icon === 'spinner' ? (
          <span className="spinner" aria-hidden="true" />
        ) : icon === 'brand' ? (
          <span className="status-brand" aria-hidden="true">
            <BrandMark size={96} />
          </span>
        ) : icon ? (
          <span className="status-icon" aria-hidden="true">
            <Icon name={icon} />
          </span>
        ) : null}
        <div className="status-text">
          {eyebrow && <p className="status-eyebrow">{eyebrow}</p>}
          <h1>{title}</h1>
          {body && <p className="lead">{body}</p>}
        </div>
        {children}
        {actions && <div className="row status-actions">{actions}</div>}
      </main>
    </div>
  )
}
