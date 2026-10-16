/**
 * 로그인 상태 저장소. 앱이 뜨자마자(모듈 로드 시) 한 번 확인을 시작해 첫 화면과 병렬로 진행한다.
 */
import { useSyncExternalStore } from 'react'
import { fetchMe, logout as apiLogout } from './api'

export type AuthState = { status: 'loading' } | { status: 'guest' } | { status: 'host'; username: string }

let state: AuthState = { status: 'loading' }
let generation = 0
let refreshId = 0
const listeners = new Set<() => void>()

function set(next: AuthState) {
  state = next
  listeners.forEach((l) => l())
}

export function refreshAuth(): Promise<void> {
  const startedGeneration = generation
  const id = ++refreshId
  const isCurrent = () => generation === startedGeneration && refreshId === id
  return fetchMe()
    .then((username) => {
      if (isCurrent()) set(username ? { status: 'host', username } : { status: 'guest' })
    })
    .catch(() => {
      if (isCurrent()) set({ status: 'guest' })
    })
}

export async function logout(): Promise<boolean> {
  generation++
  try {
    await apiLogout()
  } catch {
    return false
  }
  // 로그아웃 도중 시작된 상태 조회도 세션 폐기 이전 응답일 수 있다.
  generation++
  set({ status: 'guest' })
  return true
}

void refreshAuth()

export function useAuth(): AuthState {
  return useSyncExternalStore(
    (cb) => {
      listeners.add(cb)
      return () => listeners.delete(cb)
    },
    () => state,
  )
}
