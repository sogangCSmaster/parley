import { useLayoutEffect, useRef, useState } from 'react'

/** 맨 아래(최신)에서 이만큼 위로 올라가면 지난 대화를 읽는 중으로 본다. */
const REVIEW_PX = 48
/** 읽기 모드로 바뀌면 글자 크기가 바뀌어 목록 높이가 달라진다. 그 직후의 스크롤 이벤트로 바로 빠져나가지 않게 한다. */
const SETTLE_MS = 400

/**
 * 쌓이는 자막 목록의 스크롤: 평소에는 최신 줄을 따라 내려가고, 사용자가 위로 올리면 멈춰 "읽는 중"이 된다.
 * 읽는 동안 새로 쌓인 줄 수를 센다. 목록은 오래된 줄부터 잘리므로(최대 200줄) 길이가 아니라
 * 읽기 시작할 때의 마지막 줄 뒤에 몇 줄이 붙었는지로 센다. `changed`는 목록이 바뀔 때마다 달라지는 값이다.
 */
export function useReviewScroll<T extends HTMLElement>(keys: readonly string[], changed: unknown) {
  const ref = useRef<T>(null)
  const [reviewing, setReviewing] = useState(false)
  // 읽기 모드로 바뀔 때의 마지막 줄. 이 줄 뒤에 붙은 것이 새 줄이다.
  const [seenKey, setSeenKey] = useState<string | null>(null)
  const enteredAt = useRef(0)

  useLayoutEffect(() => {
    const el = ref.current
    if (el && !reviewing) el.scrollTop = el.scrollHeight
  }, [changed, reviewing])

  function onScroll(e: { timeStamp: number }) {
    const el = ref.current
    if (!el) return
    const away = el.scrollHeight - el.scrollTop - el.clientHeight > REVIEW_PX
    if (away && !reviewing) {
      enteredAt.current = e.timeStamp
      setSeenKey(keys.at(-1) ?? null)
      setReviewing(true)
    } else if (!away && reviewing && e.timeStamp - enteredAt.current > SETTLE_MS) {
      setReviewing(false)
    }
  }

  function toLatest() {
    // 읽기 모드를 끄면 레이아웃 효과가 맨 아래로 바로 옮긴다(부드러운 스크롤은 그 효과에 덮인다).
    setReviewing(false)
  }

  const at = seenKey === null ? -1 : keys.lastIndexOf(seenKey)
  const newCount = !reviewing ? 0 : at === -1 ? keys.length : keys.length - 1 - at
  return { ref, reviewing, onScroll, toLatest, newCount }
}
