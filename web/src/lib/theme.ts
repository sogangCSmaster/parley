/**
 * 화면 테마(시스템·라이트·다크)와 자막 크기. 계정 설정에 저장하고, 첫 화면이 깜박이지 않게
 * 이 기기에도 같은 값을 남겨 둔다(로그인 확인보다 먼저 적용).
 */
import type { CaptionSize, Theme, User } from './api'
import { onUserChange } from './auth'
import { loadPrefs, savePrefs } from './prefs'

const THEME_KEY = 'tolkmeet.theme'
const SIZE_KEY = 'tolkmeet.captionSize'
const SCALE: Record<CaptionSize, string> = { s: '0.875', m: '1', l: '1.2' }

function read(key: string): string | null {
  try {
    return localStorage.getItem(key)
  } catch {
    return null
  }
}

function write(key: string, value: string) {
  try {
    localStorage.setItem(key, value)
  } catch {
    // 저장이 막혀도 이번 화면에는 적용된다
  }
}

export function applyTheme(theme: Theme) {
  const root = document.documentElement
  if (theme === 'system') root.removeAttribute('data-theme')
  else root.setAttribute('data-theme', theme)
  write(THEME_KEY, theme)
}

export function applyCaptionSize(size: CaptionSize) {
  document.documentElement.style.setProperty('--caption-scale', SCALE[size])
  write(SIZE_KEY, size)
}

export function currentTheme(): Theme {
  const t = read(THEME_KEY)
  return t === 'light' || t === 'dark' ? t : 'system'
}

export function currentCaptionSize(): CaptionSize {
  const s = read(SIZE_KEY)
  return s === 's' || s === 'l' ? s : 'm'
}

/** 계정의 기본 설정을 이 기기에 맞춘다(테마·자막 크기·내 언어·상대 언어). */
function adopt(user: User | null) {
  if (!user) return
  const p = user.prefs
  if (p.theme) applyTheme(p.theme)
  if (p.captionSize) applyCaptionSize(p.captionSize)
  if (p.lang || p.partnerLang) {
    const local = loadPrefs()
    savePrefs({ ...local, lang: p.lang ?? local.lang, partnerLang: p.partnerLang ?? local.partnerLang })
  }
}

applyTheme(currentTheme())
applyCaptionSize(currentCaptionSize())
onUserChange(adopt)
