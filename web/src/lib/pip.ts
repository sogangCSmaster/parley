import { useCallback, useEffect, useState } from 'react'

/** Document Picture-in-Picture(Chrome·Edge 116+). 다른 앱 위에 항상 떠 있는 작은 문서 창을 연다. */
type DocumentPip = {
  requestWindow(options?: { width?: number; height?: number }): Promise<Window>
  window: Window | null
}

function api(): DocumentPip | null {
  return (window as unknown as { documentPictureInPicture?: DocumentPip }).documentPictureInPicture ?? null
}

/** 코드로 닫는 창. 이런 창의 pagehide는 사용자가 닫은 것으로 치지 않는다. */
const closingByCode = new WeakSet<Window>()

export const pipSupported = typeof window !== 'undefined' && api() !== null

/** 이 문서의 스타일을 새 창에 그대로 옮긴다(개발 중에는 <style>, 배포에서는 <link>). */
function copyStyles(target: Document) {
  for (const node of document.head.querySelectorAll('style, link[rel="stylesheet"]')) {
    target.head.appendChild(node.cloneNode(true))
  }
  target.documentElement.lang = document.documentElement.lang
}

/**
 * 자막 창 하나를 연다·닫는다. 여는 것은 사용자의 클릭 안에서만 된다(브라우저 규칙).
 * 사용자가 창을 직접 닫으면 `win`은 null, `closedByUser`는 true가 된다(다시 열면 false).
 */
export function usePipWindow(title: string) {
  const [win, setWin] = useState<Window | null>(null)
  const [closedByUser, setClosedByUser] = useState(false)

  /** `stillWanted`가 false를 돌려주면(여는 사이 공유가 끝남) 열린 창을 바로 닫는다. */
  const open = useCallback(
    async (stillWanted: () => boolean = () => true): Promise<boolean> => {
      const pip = api()
      if (!pip) return false
      let w = pip.window
      if (!w) {
        try {
          w = await pip.requestWindow({ width: 400, height: 190 })
        } catch {
          // 클릭 없이 부르면 브라우저가 거절한다. 그때는 버튼으로 다시 열게 한다.
          return false
        }
        copyStyles(w.document)
        w.document.title = title
      }
      if (!stillWanted()) {
        closingByCode.add(w)
        w.close()
        return false
      }
      const opened = w
      opened.addEventListener(
        'pagehide',
        () => {
          setWin((cur) => (cur === opened ? null : cur))
          if (!closingByCode.has(opened)) setClosedByUser(true)
        },
        { once: true },
      )
      setClosedByUser(false)
      setWin(opened)
      return true
    },
    [title],
  )

  const close = useCallback(() => {
    const w = api()?.window
    setWin(null)
    if (w) {
      closingByCode.add(w)
      w.close()
    }
  }, [])

  // 회의 화면을 떠나면 창도 닫는다.
  useEffect(
    () => () => {
      const w = api()?.window
      if (w) {
        closingByCode.add(w)
        w.close()
      }
    },
    [],
  )

  /** 새 공유를 시작할 때 지난 공유에서 사용자가 닫은 기록을 지운다. */
  const resetClosed = useCallback(() => setClosedByUser(false), [])

  return { win, open, close, closedByUser, resetClosed }
}
