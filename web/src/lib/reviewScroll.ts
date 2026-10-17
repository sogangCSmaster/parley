import { useLayoutEffect, useRef, useState } from 'react'

/** 맨 아래(최신)에서 이만큼 위로 올라가면 지난 대화를 읽는 중으로 본다. */
const REVIEW_PX = 48

/**
 * 쌓이는 자막 목록의 스크롤: 평소에는 최신 줄을 따라 내려가고, 사용자가 위로 올리면 멈춰 "읽는 중"이 된다.
 * 읽는 동안 새로 쌓인 줄 수를 센다. `changed`는 목록이 바뀔 때마다 달라지는 값(자막 배열)이다.
 */
export function useReviewScroll<T extends HTMLElement>(count: number, changed: unknown) {
  const ref = useRef<T>(null)
  const [reviewing, setReviewing] = useState(false)
  // 읽기 모드로 바뀔 때의 줄 수. 이보다 늘어난 만큼이 새 줄이다.
  const [seenCount, setSeenCount] = useState(0)

  useLayoutEffect(() => {
    const el = ref.current
    if (el && !reviewing) el.scrollTop = el.scrollHeight
  }, [changed, reviewing])

  function onScroll() {
    const el = ref.current
    if (!el) return
    const away = el.scrollHeight - el.scrollTop - el.clientHeight > REVIEW_PX
    if (away !== reviewing) {
      setReviewing(away)
      if (away) setSeenCount(count)
    }
  }

  function toLatest() {
    const el = ref.current
    setReviewing(false)
    if (el) el.scrollTo({ top: el.scrollHeight, behavior: matchMedia('(prefers-reduced-motion: reduce)').matches ? 'auto' : 'smooth' })
  }

  return { ref, reviewing, onScroll, toLatest, newCount: Math.max(0, count - seenCount) }
}
