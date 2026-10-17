import { useEffect, type RefObject } from 'react'

/**
 * <dialog>를 모달로 연다. 닫을 때 컴포넌트를 내리는 방식이라(dialog.close()를 거치지 않아)
 * 브라우저가 포커스를 돌려주지 않으므로, 열기 전에 포커스가 있던 곳으로 직접 돌려준다.
 */
export function useModalDialog(ref: RefObject<HTMLDialogElement | null>) {
  useEffect(() => {
    const before = document.activeElement instanceof HTMLElement ? document.activeElement : null
    const d = ref.current
    if (d && !d.open) d.showModal()
    return () => {
      if (before?.isConnected) before.focus()
    }
  }, [ref])
}
