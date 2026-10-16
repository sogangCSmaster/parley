import { currentLocale } from './i18n'

// 이름·언어를 다음 방문 때 미리 채우기 위한 편의 저장. 저장소가 막혀 있어도 앱은 동작해야 한다.
export type Prefs = { name: string; lang: string; partnerLang: string }

const KEY = 'parley.prefs'
// 처음 방문하면 내 언어는 화면 언어를 따른다.
const DEFAULT_PARTNER = { ko: 'ja', ja: 'ko', en: 'ko' } as const

export function loadPrefs(): Prefs {
  const locale = currentLocale()
  const DEFAULTS: Prefs = { name: '', lang: locale, partnerLang: DEFAULT_PARTNER[locale] }
  try {
    const raw = localStorage.getItem(KEY)
    return raw ? { ...DEFAULTS, ...JSON.parse(raw) } : DEFAULTS
  } catch {
    return DEFAULTS
  }
}

export function savePrefs(prefs: Prefs) {
  try {
    localStorage.setItem(KEY, JSON.stringify(prefs))
  } catch {
    // 저장 실패는 무시해도 기능에 영향 없음
  }
}
