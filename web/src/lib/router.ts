import { useSyncExternalStore } from 'react'

const subscribe = (cb: () => void) => {
  addEventListener('popstate', cb)
  return () => removeEventListener('popstate', cb)
}

export function usePath(): string {
  return useSyncExternalStore(subscribe, () => location.pathname)
}

/** 경로와 쿼리 모두(쿼리만 바뀌어도 다시 그리려고). */
export function useLocationKey(): string {
  return useSyncExternalStore(subscribe, () => location.pathname + location.search)
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

/** 지금 주소의 쿼리 값(화면은 주소가 바뀌면 다시 그려지므로 그때그때 읽는다). */
export function queryParam(name: string): string | null {
  return new URLSearchParams(location.search).get(name)
}

/** 주소를 바꾸되 방문 기록은 남기지 않는다(쿼리의 일회용 값 지우기 등). */
export function replacePath(path: string) {
  history.replaceState(null, '', path)
  dispatchEvent(new PopStateEvent('popstate'))
}
