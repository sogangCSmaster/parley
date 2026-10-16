import type { CaptionMsg } from './captions'

/**
 * 회의 자막 기록. 화면의 자막 목록(최근 200줄)과 따로, 확정된 줄을 회의 내내 모은다.
 * 서버에는 저장하지 않고 이 브라우저 메모리에만 있다가, 사용자가 내려받을 때 텍스트 파일이 된다.
 */

/** 약 3시간 분량. 넘으면 가장 오래된 줄부터 버린다(메모리를 일정하게). */
export const MAX_TRANSCRIPT = 5000

export type TranscriptLine = {
  seg: string
  speaker: string
  at: number
  src_lang: string
  src: string
  tgt_lang: string
  text: string | null
}

export class Transcript {
  private lines = new Map<string, TranscriptLine & { rev: number }>()
  private firstSeen = new Map<string, number>()

  /** 받은 자막을 모두 넘긴다. 확정된 것만 남기고, 같은 줄은 더 큰 rev로 바꾸며, 빈 원문의 확정은 철회다. */
  add(msg: CaptionMsg, now: number) {
    if (!this.firstSeen.has(msg.seg)) {
      this.firstSeen.set(msg.seg, now)
      if (this.firstSeen.size > MAX_TRANSCRIPT * 2) this.firstSeen.delete(this.firstSeen.keys().next().value!)
    }
    if (!msg.final) return
    const prev = this.lines.get(msg.seg)
    if (prev && prev.rev >= msg.rev) return
    if (msg.src === '') {
      this.lines.delete(msg.seg)
      return
    }
    const line = {
      seg: msg.seg,
      speaker: msg.speaker,
      at: prev?.at ?? this.firstSeen.get(msg.seg) ?? now,
      src_lang: msg.src_lang,
      src: msg.src,
      tgt_lang: msg.tgt_lang,
      text: msg.text,
      rev: msg.rev,
    }
    this.lines.set(msg.seg, line)
    if (this.lines.size > MAX_TRANSCRIPT) this.lines.delete(this.lines.keys().next().value!)
  }

  get size() {
    return this.lines.size
  }

  /** 말을 시작한 순서(동시에 말하면 먼저 끝난 쪽이 아니라 먼저 시작한 쪽이 앞). */
  entries(): TranscriptLine[] {
    return [...this.lines.values()].sort((a, b) => a.at - b.at)
  }
}

type FormatOptions = {
  title: string
  code: string
  savedAt: Date
  locale: string
  speakerName: (id: string) => string
}

export function formatTranscript(lines: TranscriptLine[], o: FormatOptions): string {
  // 줄마다 toLocaleTimeString을 부르면 포매터를 매번 새로 만들어 5000줄에 100ms가 넘는다. 하나를 재사용한다.
  const time = new Intl.DateTimeFormat(o.locale, { hour: '2-digit', minute: '2-digit', hour12: false })
  const date = new Intl.DateTimeFormat(o.locale, { month: 'short', day: 'numeric' })
  let day = o.savedAt.toDateString()
  const head = `${o.title} · ${o.code}\n${o.savedAt.toLocaleString(o.locale, { hour12: false })}`
  const body = lines.map((l) => {
    // 자정을 넘긴 회의에서 시각이 거꾸로 가 보이지 않게, 날짜가 바뀐 줄에는 날짜를 붙인다.
    const at = new Date(l.at)
    const stamp = at.toDateString() === day ? time.format(at) : `${date.format(at)} ${time.format(at)}`
    day = at.toDateString()
    const rows = [`[${stamp}] ${o.speakerName(l.speaker)}`, `(${l.src_lang}) ${l.src}`]
    if (l.text != null && l.tgt_lang !== l.src_lang) rows.push(`(${l.tgt_lang}) ${l.text}`)
    return rows.join('\n')
  })
  return [head, ...body].join('\n\n') + '\n'
}

/** parley-abcd-efgh-ijkm-20261016-2231.txt (로컬 시각) */
export function transcriptFileName(code: string, at: Date): string {
  const p = (n: number) => String(n).padStart(2, '0')
  const stamp = `${at.getFullYear()}${p(at.getMonth() + 1)}${p(at.getDate())}-${p(at.getHours())}${p(at.getMinutes())}`
  return `parley-${code}-${stamp}.txt`
}

/** 브라우저에서 바로 파일로 내려받는다(서버 왕복 없음). */
export function downloadText(name: string, text: string) {
  // BOM: 오래된 Windows 메모장·Excel이 한국어·일본어를 깨지지 않게 UTF-8로 읽도록.
  const url = URL.createObjectURL(new Blob(['\uFEFF', text], { type: 'text/plain;charset=utf-8' }))
  const a = document.createElement('a')
  a.href = url
  a.download = name
  a.click()
  // Safari는 클릭 직후 해제하면 내려받기가 비거나 취소된다. 넉넉히 기다렸다 해제한다.
  setTimeout(() => URL.revokeObjectURL(url), 10_000)
}
