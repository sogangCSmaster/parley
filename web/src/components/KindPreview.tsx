import { useI18n } from '../lib/i18n'
import type { MeetingKind } from './NewMeetingDialog'

/**
 * "새 회의"에서 고른 종류가 어떤 화면인지 보여 주는 작은 예시. 이미지 없이 HTML·CSS로만 그린다(추가 요청 없음).
 * 홈 데모처럼 예시 대화는 화면 언어로 번역하지 않는다(화면 이름표만 t()로). 장식이라 화면 낭독기에는 숨긴다(설명은 선택지 글에 있다).
 */
export function KindPreview({ kind }: { kind: MeetingKind }) {
  return (
    <div className={`kp kp-${kind}`} aria-hidden="true">
      {kind === 'global' && <Global />}
      {kind === 'standard' && <Standard />}
      {kind === 'inPerson' && <InPersonMock />}
      {kind === 'live' && <LiveMock />}
    </div>
  )
}

function Tile({ initial, name, color, speaking }: { initial: string; name: string; color: string; speaking?: boolean }) {
  return (
    <div className={`kp-tile${speaking ? ' speaking' : ''}`}>
      <span className="kp-avatar" style={{ background: color }}>
        {initial}
      </span>
      <span className="kp-name">{name}</span>
    </div>
  )
}

function Controls() {
  return (
    <div className="kp-controls">
      <i />
      <i />
      <i />
      <i className="end" />
    </div>
  )
}

function Global() {
  const { t } = useI18n()
  return (
    <div className="kp-stage kp-split">
      <div className="kp-tiles">
        <Tile initial="さ" name="さくら" color="#C2381A" speaking />
        <Tile initial="민" name="민준" color="#0E7F6A" />
        <Controls />
      </div>
      <div className="kp-panel">
        <b className="kp-panel-title">{t('captions.title')}</b>
        <div className="kp-entry">
          <small>さくら</small>
          <p>
            <em>JA</em>私たちの投資チームは4人です。
          </p>
          <p className="tr">
            <em>KO</em>저희 투자팀은 4명이에요.
          </p>
        </div>
        <div className="kp-entry">
          <small>민준</small>
          <p>
            <em>KO</em>어떤 분야에 투자하세요?
          </p>
          <p className="tr">
            <em>JA</em>どの分野に投資していますか？
          </p>
        </div>
      </div>
    </div>
  )
}

function Standard() {
  return (
    <div className="kp-stage">
      <div className="kp-tiles two">
        <Tile initial="민" name="민준" color="#0E7F6A" speaking />
        <Tile initial="지" name="지은" color="#5B5F68" />
      </div>
      <Controls />
    </div>
  )
}

// 아래는 일본어 화자(기기 주인), 위는 한국어 화자(돌려 보임). 내 말은 원문을 흐리게, 상대 말은 내 언어 번역을 크게.
function InPersonMock() {
  return (
    <div className="kp-device kp-talk">
      <div className="kp-half far">
        <p className="big">네, 감사합니다.</p>
        <p className="small">はい、ありがとうございます。</p>
        <p className="big mine">예산에 대해 확인하고 싶어요.</p>
        <p className="small">予算について確認したいです。</p>
        <span className="kp-btn">누르고 말하세요</span>
      </div>
      <div className="kp-bar" />
      <div className="kp-half">
        <p className="big mine">はい、ありがとうございます。</p>
        <p className="small">네, 감사합니다.</p>
        <p className="big">予算について確認したいです。</p>
        <p className="small">예산에 대해 확인하고 싶어요.</p>
        <span className="kp-btn on">聞き取り中…</span>
      </div>
    </div>
  )
}

const LIVE_LINES: [string, string, string, boolean?][] = [
  ['日本語', '本日はお時間をいただきありがとうございます。', '오늘 시간 내 주셔서 감사합니다.'],
  ['한국어', '저희 서비스를 먼저 소개해 드릴게요.', 'まず弊社のサービスをご紹介します。'],
  ['日本語', '予算について確認したいです。', '예산에 대해 확인하고 싶습니다.'],
  ['한국어', '네, 어떤 부분이 궁금하세요?', 'はい、どの部分が気になりますか？'],
  ['日本語', '販売代理店の条件について', '판매 대리점 조건에 대해', true],
]

function LiveMock() {
  const { t } = useI18n()
  return (
    <div className="kp-device kp-live">
      <div className="kp-live-bar">
        <span>한국어 ⇄ 日本語</span>
        <span className="kp-dot">{t('live.listening')}</span>
      </div>
      {LIVE_LINES.map(([lang, src, tgt, pending]) => (
        <div key={src} className={`kp-line${pending ? ' pending' : ''}`}>
          <small>
            <b>{lang}</b> {src}
          </small>
          <p>{tgt}</p>
        </div>
      ))}
    </div>
  )
}
