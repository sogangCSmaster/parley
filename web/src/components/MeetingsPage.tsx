import { useEffect, useRef, useState, type FormEvent } from 'react'
import { api, createRoom, type Meeting } from '../lib/api'
import { useAuth } from '../lib/auth'
import { formatDate, formatDuration, formatTime, timeIn } from '../lib/format'
import { useI18n, type MessageKey } from '../lib/i18n'
import '../lib/i18nPages'
import { LANGUAGES, languageName } from '../lib/languages'
import { inviteText, meetingPath, meetingUrl } from '../lib/meetings'
import { loadPrefs } from '../lib/prefs'
import { navigate } from '../lib/router'
import { Header } from './Header'
import { Icon } from './Icon'
import { useModalDialog } from '../lib/modal'
import { Select } from './Select'

const LANG_OPTIONS = LANGUAGES.map((l) => ({ value: l.code as string, label: l.name }))

async function copy(text: string): Promise<boolean> {
  try {
    await navigator.clipboard.writeText(text)
    return true
  } catch {
    return false
  }
}

function useMeetings(): [{ upcoming: Meeting[]; past: Meeting[] } | null, () => void] {
  const [data, setData] = useState<{ upcoming: Meeting[]; past: Meeting[] } | null>(null)
  const [n, setN] = useState(0)
  useEffect(() => {
    let alive = true
    void api.get<{ upcoming: Meeting[]; past: Meeting[] }>('/api/meetings').then((r) => alive && r.ok && setData(r.data))
    return () => {
      alive = false
    }
  }, [n])
  return [data, () => setN((x) => x + 1)]
}

export function MeetingsPage() {
  const { t, locale } = useI18n()
  const auth = useAuth()
  const [data, reload] = useMeetings()
  const [tab, setTab] = useState<'upcoming' | 'past'>('upcoming')
  const [editing, setEditing] = useState<Meeting | 'new' | null>(null)
  const [done, setDone] = useState<Meeting | null>(null)
  const [toast, setToast] = useState<MessageKey | null>(null)
  const [error, setError] = useState<MessageKey | null>(null)
  const [starting, setStarting] = useState(false)

  useEffect(() => {
    if (auth.status === 'guest') navigate('/login')
  }, [auth.status])
  useEffect(() => {
    if (!toast) return
    const id = setTimeout(() => setToast(null), 2500)
    return () => clearTimeout(id)
  }, [toast])

  async function startNow() {
    setStarting(true)
    const r = await createRoom(true)
    setStarting(false)
    if (r.kind === 'ok') navigate(`/${r.code}`)
    else if (r.kind === 'plan') navigate('/checkout')
    else setError('home.createFailed')
  }

  async function copyLink(m: Meeting) {
    setToast((await copy(meetingUrl(m))) ? 'room.linkCopied' : 'account.failed')
  }

  /** `past`: 지난 회의 탭에서 지우면 '기록 삭제' 문구로 확인한다. */
  async function remove(m: Meeting, past: boolean) {
    // 이미 링크를 받은 손님이 있을 수 있으니 한 번 확인한다.
    const title = m.title || t('meetings.untitled')
    if (!window.confirm(t(past ? 'meetings.deletePastConfirm' : 'meetings.deleteConfirm', { title }))) return
    const r = await api.del(`/api/meetings/${m.id}`)
    if (r.ok) reload()
    else setError('account.failed')
  }

  const list = data ? data[tab] : []
  return (
    <div className="shell">
      <Header />
      <main className="page">
        <div className="page-head">
          <h1>{t('meetings.title')}</h1>
          <div className="row">
            <button className="btn btn-outline" onClick={() => setEditing('new')}>
              <Icon name="calendar_month" />
              {t('meetings.schedule')}
            </button>
            <button className="btn btn-primary" onClick={() => void startNow()} disabled={starting}>
              <Icon name="video_call" />
              {t('home.startNow')}
            </button>
          </div>
        </div>
        {error && (
          <p className="form-error" role="alert">
            {t(error)}
          </p>
        )}
        <div className="segmented tabs" role="tablist" aria-label={t('meetings.title')}>
          <button role="tab" aria-selected={tab === 'upcoming'} className={tab === 'upcoming' ? 'on' : ''} onClick={() => setTab('upcoming')}>
            {t('meetings.upcoming')} {data ? data.upcoming.length : ''}
          </button>
          <button role="tab" aria-selected={tab === 'past'} className={tab === 'past' ? 'on' : ''} onClick={() => setTab('past')}>
            {t('meetings.past')}
          </button>
        </div>
        <div className="table-card" role="tabpanel">
          {data && list.length === 0 && <p className="card-empty">{t(tab === 'upcoming' ? 'meetings.noUpcoming' : 'meetings.noPast')}</p>}
          {list.length > 0 && (
            <table className="meetings-table">
              <thead>
                <tr>
                  <th>{t('meetings.date')}</th>
                  <th>{t('meetings.meeting')}</th>
                  <th className="hide-sm">{tab === 'upcoming' ? t('meetings.languages') : t('meetings.length')}</th>
                  <th className="hide-sm">{tab === 'upcoming' ? t('meetings.invited') : t('meetings.people')}</th>
                  {tab === 'past' && <th className="hide-sm">{t('meetings.translated')}</th>}
                  <th aria-label={t('meetings.actions')} />
                </tr>
              </thead>
              <tbody>
                {list.map((m) => {
                  const at = (tab === 'upcoming' ? m.startsAt : (m.startedAt ?? m.startsAt)) ?? m.createdAt
                  return (
                    <tr key={m.id}>
                      <td>
                        <b>{formatDate(at, locale)}</b>
                        <small>{new Date(at * 1000).toLocaleDateString(locale, { weekday: 'short' })} {formatTime(at, locale)}</small>
                      </td>
                      <td>
                        <b className="meeting-title">{m.title || t('meetings.untitled')}</b>
                        {m.kind !== 'meeting' && <span className="badge-soft">{t(m.kind === 'live' ? 'newMeeting.live' : 'newMeeting.inPerson')}</span>}
                      </td>
                      {tab === 'upcoming' ? (
                        <>
                          <td className="hide-sm">{m.langA && m.langB ? `${languageName(m.langA)} ⇄ ${languageName(m.langB)}` : '—'}</td>
                          <td className="hide-sm">{m.guests.length ? t('meetings.peopleCount', { n: String(m.guests.length) }) : t('meetings.none')}</td>
                        </>
                      ) : (
                        <>
                          <td className="hide-sm">{formatDuration(m.lengthSec, t)}</td>
                          <td className="hide-sm">{m.peak ? t('meetings.peopleCount', { n: String(m.peak) }) : '—'}</td>
                          <td className="hide-sm">{formatDuration(m.translatedSec, t)}</td>
                        </>
                      )}
                      <td className="actions">
                        {tab === 'upcoming' && (
                          <>
                            <button className="btn btn-sm btn-outline hide-sm" onClick={() => void copyLink(m)}>
                              {t('meetings.copyLink')}
                            </button>
                            <RowMenu
                              items={[
                                [t('meetings.open'), () => navigate(meetingPath(m))],
                                // 좁은 화면에서는 줄의 링크 복사 버튼을 숨기므로 메뉴에서도 할 수 있게 한다.
                                [t('meetings.copyLink'), () => void copyLink(m)],
                                [t('meetings.edit'), () => setEditing(m)],
                                [t('meetings.inviteInfo'), () => setDone(m)],
                                [t('meetings.addToCalendar'), () => (location.href = `/api/meetings/${m.id}/ics`)],
                                [t('meetings.delete'), () => void remove(m, false), true],
                              ]}
                              label={t('meetings.actions')}
                            />
                          </>
                        )}
                        {/* 지난 회의 기록(이름·초대한 사람 주소 등)도 지울 수 있게(개인정보 처리방침과 같게). */}
                        {tab === 'past' && <RowMenu items={[[t('meetings.delete'), () => void remove(m, true), true]]} label={t('meetings.actions')} />}
                      </td>
                    </tr>
                  )
                })}
              </tbody>
            </table>
          )}
        </div>
        <p className="page-foot">{t('meetings.privacy')}</p>
      </main>
      {editing && (
        <ScheduleDialog
          meeting={editing === 'new' ? null : editing}
          onClose={() => setEditing(null)}
          onSaved={(m) => {
            setEditing(null)
            reload()
            setDone(m)
          }}
        />
      )}
      {done && <ScheduledDialog meeting={done} onClose={() => setDone(null)} onToast={setToast} />}
      {toast && (
        <div className="toast light" role="status">
          <Icon name="check_circle" />
          {t(toast)}
        </div>
      )}
    </div>
  )
}

/** "…" 메뉴 */
function RowMenu({ items, label }: { items: [string, () => void, boolean?][]; label: string }) {
  const [open, setOpen] = useState(false)
  const ref = useRef<HTMLDivElement>(null)
  useEffect(() => {
    if (!open) return
    const close = (e: PointerEvent) => !ref.current?.contains(e.target as Node) && setOpen(false)
    const esc = (e: KeyboardEvent) => e.key === 'Escape' && setOpen(false)
    document.addEventListener('pointerdown', close)
    document.addEventListener('keydown', esc)
    return () => {
      document.removeEventListener('pointerdown', close)
      document.removeEventListener('keydown', esc)
    }
  }, [open])
  return (
    <div className="row-menu" ref={ref}>
      <button className="icon-btn" aria-label={label} aria-expanded={open} onClick={() => setOpen((o) => !o)}>
        <Icon name="more_horiz" />
      </button>
      {open && (
        <div className="account-menu">
          {items.map(([text, fn, danger]) => (
            <button
              key={text}
              className={`account-item${danger ? ' danger' : ''}`}
              onClick={() => {
                setOpen(false)
                fn()
              }}
            >
              {text}
            </button>
          ))}
        </div>
      )}
    </div>
  )
}

function Dialog({ title, onClose, children, labelledBy = 'dialog-title' }: { title?: string; onClose: () => void; children: React.ReactNode; labelledBy?: string }) {
  const ref = useRef<HTMLDialogElement>(null)
  useModalDialog(ref)
  // Escape는 cancel 다음 close를 부르므로 close 하나에서만 닫는다.
  return (
    <dialog ref={ref} className="sheet" aria-labelledby={labelledBy} onClose={onClose}>
      {title && <h2 id={labelledBy}>{title}</h2>}
      {children}
    </dialog>
  )
}

const ZONES = ['Asia/Seoul', 'Asia/Tokyo', 'America/New_York', 'Europe/London'] as const

function toLocalInputs(sec: number): { date: string; time: string } {
  const d = new Date(sec * 1000)
  const pad = (n: number) => String(n).padStart(2, '0')
  return { date: `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}`, time: `${pad(d.getHours())}:${pad(d.getMinutes())}` }
}

function ScheduleDialog({ meeting, onClose, onSaved }: { meeting: Meeting | null; onClose: () => void; onSaved: (m: Meeting) => void }) {
  const { t, locale } = useI18n()
  const prefs = loadPrefs()
  // 기본값: 다음 정시 + 1시간
  const [initial] = useState(() => meeting?.startsAt ?? Math.ceil(Date.now() / 3_600_000) * 3600 + 3600)
  const [title, setTitle] = useState(meeting?.title ?? '')
  const [date, setDate] = useState(() => toLocalInputs(initial).date)
  const [time, setTime] = useState(() => toLocalInputs(initial).time)
  const [duration, setDuration] = useState(String(meeting?.durationMin ?? 60))
  const [kind, setKind] = useState<Meeting['kind']>(meeting?.kind ?? 'meeting')
  const [langA, setLangA] = useState(meeting?.langA || prefs.lang)
  const [langB, setLangB] = useState(meeting?.langB || prefs.partnerLang)
  const [guests, setGuests] = useState((meeting?.guests ?? []).join(', '))
  // 고칠 때는 기본으로 다시 보내지 않는다(제목만 고쳐도 손님 모두에게 메일이 가지 않게).
  const [sendMail, setSendMail] = useState(!meeting)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<MessageKey | null>(null)

  // 고칠 때 날짜·시각을 건드리지 않았으면 원래 값을 그대로 보낸다(서머타임 겹치는 시간에도 같은 순간이 되게).
  const original = toLocalInputs(initial)
  const startsAt =
    meeting?.startsAt != null && date === original.date && time === original.time
      ? meeting.startsAt
      : Math.floor(new Date(`${date}T${time}`).getTime() / 1000)
  const valid = Number.isFinite(startsAt)
  const myZone = Intl.DateTimeFormat().resolvedOptions().timeZone

  async function save(e: FormEvent) {
    e.preventDefault()
    if (!title.trim()) return setError('meetings.needTitle')
    if (!valid) return setError('meetings.badTime')
    setBusy(true)
    setError(null)
    const list = guests.split(/[,\s;]+/).map((g) => g.trim()).filter(Boolean)
    const body = { title: title.trim(), kind, startsAt, durationMin: Number(duration), langA, langB, guests: list, invite: sendMail && list.length > 0 }
    const r = meeting ? await api.patch<Meeting>(`/api/meetings/${meeting.id}`, body) : await api.post<Meeting>('/api/meetings', body)
    setBusy(false)
    if (r.ok) {
      // 저장하면서 초대 글을 바로 클립보드에(바로 붙여 넣을 수 있게).
      void copy(inviteText(r.data, t, locale))
      return onSaved(r.data)
    }
    if (r.status === 402) return navigate('/checkout')
    setError(r.code === 'starts_at' ? 'meetings.badTime' : r.code === 'guests' ? 'meetings.badGuests' : r.code === 'title' ? 'meetings.needTitle' : 'account.failed')
  }

  return (
    <Dialog title={t(meeting ? 'meetings.editTitle' : 'meetings.schedule')} onClose={onClose}>
      <form className="sheet-form" onSubmit={save}>
        <label className="field-label">
          {t('meetings.name')}
          <input className="input" value={title} maxLength={80} onChange={(e) => setTitle(e.target.value)} autoFocus placeholder={t('meetings.namePlaceholder')} />
        </label>
        <div className="two">
          <label className="field-label">
            {t('meetings.date')}
            <input className="input" type="date" value={date} onChange={(e) => setDate(e.target.value)} required />
          </label>
          <label className="field-label">
            {t('meetings.time')}
            <input className="input" type="time" value={time} step={300} onChange={(e) => setTime(e.target.value)} required />
          </label>
        </div>
        {valid && (
          <p className="form-note">
            {ZONES.filter((z) => z !== myZone)
              .slice(0, 3)
              .map((z) => `${t(`zone.${z.split('/')[1]}` as MessageKey)} ${timeIn(startsAt, locale, z)}`)
              .join(' · ')}
          </p>
        )}
        <label className="field-label">
          {t('meetings.duration')}
          <Select
            ariaLabel={t('meetings.duration')}
            value={duration}
            options={['30', '45', '60', '90', '120'].map((v) => ({ value: v, label: formatDuration(Number(v) * 60, t) }))}
            onChange={setDuration}
          />
        </label>
        <div className="field-label">
          {t('meetings.kind')}
          <div className="segmented" role="radiogroup" aria-label={t('meetings.kind')}>
            {(
              [
                ['meeting', 'meetings.kindMeeting'],
                ['live', 'newMeeting.live'],
                ['inPerson', 'newMeeting.inPerson'],
              ] as const
            ).map(([v, key]) => (
              <button key={v} type="button" role="radio" aria-checked={kind === v} className={kind === v ? 'on' : ''} onClick={() => setKind(v)}>
                {t(key)}
              </button>
            ))}
          </div>
        </div>
        <div className="two">
          <Select label={t('lang.mine')} ariaLabel={t('lang.mine')} value={langA} options={LANG_OPTIONS} onChange={setLangA} />
          <Select label={t('lang.partner')} ariaLabel={t('lang.partner')} value={langB} options={LANG_OPTIONS} onChange={setLangB} />
        </div>
        <label className="field-label">
          {t('meetings.guests')}
          <input className="input" value={guests} onChange={(e) => setGuests(e.target.value)} placeholder={t('meetings.guestsPlaceholder')} autoComplete="off" />
        </label>
        <label className="check-row">
          <input type="checkbox" checked={sendMail} onChange={(e) => setSendMail(e.target.checked)} />
          {t('meetings.sendMail')}
        </label>
        {error && (
          <p className="form-error" role="alert">
            {t(error)}
          </p>
        )}
        <div className="sheet-actions">
          <button type="button" className="btn btn-outline" onClick={onClose}>
            {t('room.cancel')}
          </button>
          <button className="btn btn-primary" disabled={busy}>
            {t(meeting ? 'account.save' : 'meetings.scheduleAndCopy')}
          </button>
        </div>
      </form>
    </Dialog>
  )
}

function ScheduledDialog({ meeting, onClose, onToast }: { meeting: Meeting; onClose: () => void; onToast: (k: MessageKey) => void }) {
  const { t, locale } = useI18n()
  const start = meeting.startsAt ?? 0
  const text = inviteText(meeting, t, locale)
  const myZone = Intl.DateTimeFormat().resolvedOptions().timeZone
  const mail = `mailto:${encodeURIComponent(meeting.guests.join(','))}?subject=${encodeURIComponent(meeting.title)}&body=${encodeURIComponent(text)}`
  return (
    <Dialog onClose={onClose} labelledBy="done-title">
      <div className="done-head">
        <span className="done-check" aria-hidden="true">
          <Icon name="check" />
        </span>
        <h2 id="done-title">{t('meetings.scheduled')}</h2>
      </div>
      <p className="lead left">{t('meetings.scheduledLead')}</p>
      <div className="invite-card">
        <b>{meeting.title}</b>
        <p>
          {formatDate(start, locale, true)} {formatTime(start, locale)} – {formatTime(start + meeting.durationMin * 60, locale)}
          <small>
            {ZONES.filter((z) => z !== myZone)
              .slice(0, 2)
              .map((z) => `${t(`zone.${z.split('/')[1]}` as MessageKey)} ${timeIn(start, locale, z)}`)
              .join(' · ')}
          </small>
        </p>
        <p>
          <small>{t('meetings.joinLink')}</small>
          <a href={meetingUrl(meeting)}>{meetingUrl(meeting).replace(/^https?:\/\//, '')}</a>
        </p>
        {meeting.langA && meeting.langB && (
          <small>
            {languageName(meeting.langA)} ⇄ {languageName(meeting.langB)} · {t('meetings.browserNote')}
          </small>
        )}
      </div>
      <button className="btn btn-primary btn-lg wide" onClick={async () => onToast((await copy(text)) ? 'meetings.copied' : 'account.failed')}>
        <Icon name="content_copy" />
        {t('meetings.copyInvite')}
      </button>
      <div className="three">
        <button className="btn btn-outline" onClick={async () => onToast((await copy(meetingUrl(meeting))) ? 'room.linkCopied' : 'account.failed')}>
          {t('meetings.copyLinkOnly')}
        </button>
        <a className="btn btn-outline" href={`/api/meetings/${meeting.id}/ics`}>
          {t('meetings.addToCalendar')}
        </a>
        <a className="btn btn-outline" href={mail}>
          {t('meetings.sendByMail')}
        </a>
      </div>
      {meeting.sent && meeting.sent.length > 0 && <p className="form-note">{t('meetings.mailedTo', { list: meeting.sent.join(', ') })}</p>}
      <div className="sheet-actions">
        <button className="btn btn-quiet" onClick={onClose}>
          {t('meetings.done')}
        </button>
      </div>
    </Dialog>
  )
}

export { ScheduleDialog, ScheduledDialog }
