/**
 * 로그인 상태 저장소. 앱이 뜨자마자(모듈 로드 시) 한 번 확인을 시작해 첫 화면과 병렬로 진행한다.
 */
import { useSyncExternalStore } from 'react'
import { fetchMe, logout as apiLogout } from './api'

export type AuthState = { status: 'loading' } | { status: 'guest' } | { status: 'host'; username: string }

let state: AuthState = { status: 'loading' }
const listeners = new Set<() => void>()

function set(next: AuthState) {
  state = next
  listeners.forEach((l) => l())
}

export function refreshAuth(): Promise<void> {
  return fetchMe()
    .then((username) => set(username ? { status: 'host', username } : { status: 'guest' }))
    .catch(() => set({ status: 'guest' }))
}

export async function logout() {
  await apiLogout().catch(() => undefined)
  set({ status: 'guest' })
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
