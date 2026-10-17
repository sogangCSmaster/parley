/** 날짜·시간·길이 표시(화면 언어의 형식으로). 시각은 서버의 유닉스 초. */
import type { Locale } from './i18n'

export function formatDate(sec: number, locale: Locale, withWeekday = false): string {
  return new Date(sec * 1000).toLocaleDateString(locale, { month: 'long', day: 'numeric', ...(withWeekday ? { weekday: 'short' } : {}) })
}

export function formatTime(sec: number, locale: Locale): string {
  return new Date(sec * 1000).toLocaleTimeString(locale, { hour: 'numeric', minute: '2-digit' })
}

export function formatDateTime(sec: number, locale: Locale): string {
  return `${formatDate(sec, locale, true)} ${formatTime(sec, locale)}`
}

/** "3일 전", "2 hours ago" */
export function relativeTime(sec: number, locale: Locale): string {
  const diff = sec - Date.now() / 1000
  const abs = Math.abs(diff)
  const rtf = new Intl.RelativeTimeFormat(locale, { numeric: 'auto' })
  if (abs < 60) return rtf.format(0, 'second')
  if (abs < 3600) return rtf.format(Math.round(diff / 60), 'minute')
  if (abs < 86_400) return rtf.format(Math.round(diff / 3600), 'hour')
  if (abs < 30 * 86_400) return rtf.format(Math.round(diff / 86_400), 'day')
  if (abs < 365 * 86_400) return rtf.format(Math.round(diff / (30 * 86_400)), 'month')
  return rtf.format(Math.round(diff / (365 * 86_400)), 'year')
}

/** 초 → "2시간 40분" / "2 h 40 min" / "2時間40分"(분 단위). 문구는 i18n의 ended.* 를 쓴다. */
export function formatDuration(sec: number, t: (key: 'ended.minutes' | 'ended.hoursMinutes', p: Record<string, string>) => string): string {
  const min = Math.max(0, Math.round(sec / 60))
  if (min < 60) return t('ended.minutes', { n: String(min) })
  return t('ended.hoursMinutes', { h: String(Math.floor(min / 60)), m: String(min % 60) })
}

/** 시간대 이름 없이 그 도시의 같은 시각(예약 화면의 "서울 오후 3:00 · 뉴욕 오전 2:00"). */
export function timeIn(sec: number, locale: Locale, timeZone: string): string {
  return new Date(sec * 1000).toLocaleTimeString(locale, { hour: 'numeric', minute: '2-digit', timeZone })
}

/** 사용자 에이전트 → "Mac · Chrome" */
export function deviceLabel(ua: string): string {
  const os = /iPhone/.test(ua) ? 'iPhone' : /iPad/.test(ua) ? 'iPad' : /Android/.test(ua) ? 'Android' : /Mac OS X|Macintosh/.test(ua) ? 'Mac' : /Windows/.test(ua) ? 'Windows' : /Linux/.test(ua) ? 'Linux' : ''
  const browser = /Edg\//.test(ua) ? 'Edge' : /OPR\//.test(ua) ? 'Opera' : /Firefox\//.test(ua) ? 'Firefox' : /Chrome\//.test(ua) ? 'Chrome' : /Safari\//.test(ua) ? 'Safari' : ''
  return [os, browser].filter(Boolean).join(' · ') || ua.slice(0, 40)
}
