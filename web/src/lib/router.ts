import { useSyncExternalStore } from 'react'

const subscribe = (cb: () => void) => {
  addEventListener('popstate', cb)
  return () => removeEventListener('popstate', cb)
}

export function usePath(): string {
  return useSyncExternalStore(subscribe, () => location.pathname)
}

export function navigate(path: string) {
  history.pushState(null, '', path)
  dispatchEvent(new PopStateEvent('popstate'))
}

const CODE = /^[a-km-np-z2-9]{4}-[a-km-np-z2-9]{4}-[a-km-np-z2-9]{4}$/

/** 코드 또는 회의 링크 전체를 받아 코드만 돌려준다. 형식이 틀리면 null. */
export function parseRoomCode(input: string): string | null {
  const last = input.trim().toLowerCase().split('/').filter(Boolean).pop() ?? ''
  return CODE.test(last) ? last : null
}
