import { useCallback, useEffect, useState } from 'react'

/** Document Picture-in-Picture(Chrome·Edge 116+). 다른 앱 위에 항상 떠 있는 작은 문서 창을 연다. */
type DocumentPip = {
  requestWindow(options?: { width?: number; height?: number }): Promise<Window>
  window: Window | null
}

function api(): DocumentPip | null {
  return (window as unknown as { documentPictureInPicture?: DocumentPip }).documentPictureInPicture ?? null
}

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
 * 사용자가 창을 직접 닫으면 null로 돌아간다.
 */
export function usePipWindow(title: string) {
  const [win, setWin] = useState<Window | null>(null)

  const open = useCallback(async (): Promise<boolean> => {
    const pip = api()
    if (!pip) return false
    if (pip.window) {
      setWin(pip.window)
      return true
    }
    try {
      const w = await pip.requestWindow({ width: 400, height: 190 })
      copyStyles(w.document)
      w.document.title = title
      w.addEventListener('pagehide', () => setWin(null), { once: true })
      setWin(w)
      return true
    } catch {
      // 클릭 없이 부르면 브라우저가 거절한다. 그때는 버튼으로 다시 열게 한다.
      return false
    }
  }, [title])

  const close = useCallback(() => {
    api()?.window?.close()
    setWin(null)
  }, [])

  // 회의 화면을 떠나면 창도 닫는다.
  useEffect(() => () => api()?.window?.close(), [])

  return { win, open, close }
}
