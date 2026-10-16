import { useEffect, useRef, useState, type FormEvent } from 'react'
import { useI18n, type MessageKey } from '../lib/i18n'
import { Icon } from './Icon'
import { KindPreview } from './KindPreview'
import type { IconName } from './icons'

export type MeetingKind = 'global' | 'standard' | 'inPerson' | 'live'

const KIND_KEY = 'tolkmeet.meetingKind'

/** 지난번 선택을 기본값으로. 저장소가 막혀 있으면 글로벌 미팅. */
function lastMeetingKind(): MeetingKind {
  try {
    const saved = localStorage.getItem(KIND_KEY)
    return saved === 'standard' || saved === 'inPerson' || saved === 'live' ? saved : 'global'
  } catch {
    return 'global'
  }
}

function rememberKind(kind: MeetingKind) {
  try {
    localStorage.setItem(KIND_KEY, kind)
  } catch {
    // 기억 못 해도 동작에는 지장 없음
  }
}

const OPTIONS: { kind: MeetingKind; icon: IconName; title: MessageKey; body: MessageKey }[] = [
  { kind: 'global', icon: 'translate', title: 'newMeeting.global', body: 'newMeeting.globalBody' },
  { kind: 'standard', icon: 'video_call', title: 'newMeeting.standard', body: 'newMeeting.standardBody' },
  { kind: 'inPerson', icon: 'forum', title: 'newMeeting.inPerson', body: 'newMeeting.inPersonBody' },
  { kind: 'live', icon: 'subtitles', title: 'newMeeting.live', body: 'newMeeting.liveBody' },
]

type Props = {
  open: boolean
  creating: boolean
  error: MessageKey | null
  onCreate: (kind: MeetingKind) => void
  onClose: () => void
}

/** 브라우저 기본 <dialog>를 모달로 쓴다: Esc 닫기, 배경 클릭 차단, 포커스 이동이 기본 제공된다. */
export function NewMeetingDialog({ open, creating, error, onCreate, onClose }: Props) {
  const { t } = useI18n()
  const ref = useRef<HTMLDialogElement>(null)
  const [kind, setKind] = useState<MeetingKind>(lastMeetingKind)
  // 처음엔 좁게 열고, 종류를 고르면 넓어지며 오른쪽에 그 종류의 예시 화면을 보인다(좁은 화면에서는 보이지 않음).
  const [expanded, setExpanded] = useState(false)

  useEffect(() => {
    const dialog = ref.current
    if (!dialog) return
    if (open && !dialog.open) dialog.showModal()
    if (!open && dialog.open) dialog.close()
  }, [open])

  function choose(next: MeetingKind) {
    setKind(next)
    setExpanded(true)
  }

  function close() {
    setExpanded(false)
    onClose()
  }

  function submit(e: FormEvent) {
    e.preventDefault()
    rememberKind(kind)
    onCreate(kind)
  }

  return (
    <dialog
      ref={ref}
      className={`dialog${expanded ? ' dialog-wide' : ''}`}
      aria-labelledby="new-meeting-title"
      onClose={close}
      onCancel={(e) => {
        // 만드는 중에는 Esc로 닫히지 않게 한다(중복 생성 방지).
        if (creating) e.preventDefault()
      }}
    >
      <form className="dialog-body" onSubmit={submit}>
        <div className="dialog-main">
          <h2 id="new-meeting-title">{t('newMeeting.title')}</h2>
          <p className="lead">{t('newMeeting.lead')}</p>
          <div className="kind-options" role="radiogroup" aria-labelledby="new-meeting-title">
            {OPTIONS.map((o) => (
              <label key={o.kind} className="kind-option" onClick={() => setExpanded(true)}>
                <input
                  type="radio"
                  name="meeting-kind"
                  value={o.kind}
                  checked={kind === o.kind}
                  onChange={() => choose(o.kind)}
                />
                <span className="kind-icon" aria-hidden="true">
                  <Icon name={o.icon} />
                </span>
                <span className="kind-text">
                  <b>{t(o.title)}</b>
                  <span>{t(o.body)}</span>
                </span>
                <span className="kind-radio" aria-hidden="true" />
              </label>
            ))}
          </div>
          {/* 번역 켜고 끄기는 화상 회의에만 있다. 다른 종류에서도 자리는 남겨 고를 때 창 높이가 튀지 않게 한다. */}
          <p className="dialog-note" style={kind === 'global' || kind === 'standard' ? undefined : { visibility: 'hidden' }}>
            {t('newMeeting.note')}
          </p>
          {error && (
            <p className="form-error" role="alert">
              {t(error)}
            </p>
          )}
        </div>
        {expanded && (
          <div className="dialog-preview">
            <KindPreview kind={kind} />
          </div>
        )}
        <div className="dialog-actions">
          <button type="button" className="btn btn-quiet btn-lg" onClick={close} disabled={creating}>
            {t('room.cancel')}
          </button>
          <button type="submit" className="btn btn-primary btn-lg" disabled={creating}>
            {creating ? t('home.creating') : t('newMeeting.create')}
          </button>
        </div>
      </form>
    </dialog>
  )
}
