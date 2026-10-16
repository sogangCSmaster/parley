import type { ServerMsg } from '../protocol/ServerMsg'

export type CaptionMsg = Extract<ServerMsg, { t: 'caption' }>

/** 자막 한 줄. `at`은 처음 받은 시각(표시용). */
export type Caption = CaptionMsg & { at: number }

/** 메모리와 렌더 비용을 일정하게 두기 위해 오래된 줄은 버린다. */
const MAX_CAPTIONS = 200

/** 같은 `seg`는 같은 줄을 갱신한다. 늦게 도착한 낮은 `rev`는 버린다. */
export function applyCaption(list: Caption[], msg: CaptionMsg, now: number): Caption[] {
  for (let i = list.length - 1; i >= 0; i--) {
    if (list[i].seg !== msg.seg) continue
    if (msg.rev <= list[i].rev) return list
    const next = list.slice()
    next[i] = { ...msg, at: list[i].at }
    return next
  }
  const next = list.length >= MAX_CAPTIONS ? list.slice(list.length - MAX_CAPTIONS + 1) : list.slice()
  next.push({ ...msg, at: now })
  return next
}
