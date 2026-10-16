import { useEffect, useRef, useState } from 'react'

/**
 * 홈 화면의 "살아 있는" 제품 예시. 말하는 동안 원문이 타이핑되고, 번역이 한 박자 늦게 따라오며
 * 고쳐지다가 확정되는 이 제품의 핵심 동작을 그대로 재생한다. 예시 대화라 화면 언어로 번역하지 않는다.
 * 데모 상자는 높이가 고정이고 자막이 쌓이면 자막 영역 안에서 스크롤된다(페이지가 늘어나지 않음).
 * 의존성 없이 타이머 하나로 돈다. 움직임 줄이기 설정이면 완성된 장면을 정지 화면으로 보여 준다.
 */

type Who = 'sakura' | 'minjun'
type Lang = 'JA' | 'KO'

type Line = {
  who: Who
  time: string
  src: Lang
  tgt: Lang
  orig: string
  /** 원문 진행률(0~1)에 맞춰 보이는 중간 번역 */
  drafts: [number, string][]
  final: string
}

const PEOPLE: Record<Who, { name: string; initial: string; color: string }> = {
  sakura: { name: 'さくら', initial: 'さ', color: '#C2381A' },
  minjun: { name: '민준', initial: '민', color: '#0E7F6A' },
}

const SCRIPT: Line[] = [
  {
    who: 'sakura',
    time: '15:21',
    src: 'JA',
    tgt: 'KO',
    orig: '私たちの投資チームは4人のメンバーです。',
    drafts: [
      [0.3, '저희'],
      [0.55, '저희 투자 팀은'],
      [0.8, '저희 투자 팀은 네 명의 멤버'],
    ],
    final: '저희 투자팀은 4명의 멤버로 구성돼 있어요.',
  },
  {
    who: 'minjun',
    time: '15:22',
    src: 'KO',
    tgt: 'JA',
    orig: '주로 어떤 분야에 투자하고 계신가요?',
    drafts: [
      [0.35, '主に'],
      [0.65, '主にどの分野'],
      [0.9, '主にどの分野に投資を'],
    ],
    final: '主にどのような分野に投資されていますか？',
  },
  {
    who: 'sakura',
    time: '15:22',
    src: 'JA',
    tgt: 'KO',
    orig: 'AIと、最近だとディープテックに注力しています。',
    drafts: [
      [0.3, 'AI와'],
      [0.7, 'AI와 최근에는 딥테크'],
    ],
    final: 'AI, 그리고 최근에는 딥테크에 집중하고 있어요.',
  },
  {
    who: 'minjun',
    time: '15:23',
    src: 'KO',
    tgt: 'JA',
    orig: '다음 주에 데모를 보여드려도 될까요?',
    drafts: [
      [0.4, '来週'],
      [0.75, '来週デモを'],
    ],
    final: '来週、デモをお見せしてもよろしいですか？',
  },
]

type EntryView = Omit<Line, 'drafts' | 'final'> & { id: number; tr: string; typing: boolean; interim: boolean }
type Frame = { entries: EntryView[]; speaking: Who | null; delay: number }

const TYPE_MS = 90
const CHARS_PER_STEP = 2

/** 대본을 화면 프레임 목록으로 펼친다. */
function buildFrames(): Frame[] {
  const frames: Frame[] = []
  const done: EntryView[] = []
  const view = (line: Line, orig: string, tr: string, typing: boolean, interim: boolean): EntryView => ({
    id: SCRIPT.indexOf(line),
    who: line.who,
    time: line.time,
    src: line.src,
    tgt: line.tgt,
    orig,
    tr,
    typing,
    interim,
  })
  for (const line of SCRIPT) {
    const chars = [...line.orig]
    const steps = Math.ceil(chars.length / CHARS_PER_STEP)
    let draft = ''
    for (let k = 1; k <= steps; k++) {
      const progress = k / steps
      for (const [at, text] of line.drafts) if (progress >= at) draft = text
      const current = view(line, chars.slice(0, k * CHARS_PER_STEP).join(''), draft, true, true)
      frames.push({ entries: [...done, current], speaking: line.who, delay: TYPE_MS })
    }
    // 말이 끝난 뒤 잠깐 중간 번역 → 확정 번역으로 교체
    frames.push({ entries: [...done, view(line, line.orig, draft, false, true)], speaking: null, delay: 450 })
    done.push(view(line, line.orig, line.final, false, false))
    frames.push({ entries: [...done], speaking: null, delay: 1500 })
  }
  frames[frames.length - 1].delay = 2600
  return frames
}

// 대본은 고정이므로 모듈 로드 시 한 번만 펼친다.
const FRAMES = buildFrames()

function prefersReducedMotion() {
  return typeof matchMedia === 'function' && matchMedia('(prefers-reduced-motion: reduce)').matches
}

export function HeroDemo() {
  const [index, setIndex] = useState(() => (prefersReducedMotion() ? FRAMES.length - 1 : 0))
  const feedRef = useRef<HTMLDivElement>(null)

  useEffect(() => {
    if (prefersReducedMotion()) return
    const id = setTimeout(() => setIndex((i) => (i + 1) % FRAMES.length), FRAMES[index].delay)
    return () => clearTimeout(id)
  }, [index])

  // 새 자막이 들어오거나 글자가 늘면 맨 아래(최신 줄)로 따라간다. 처음부터 다시 돌면 맨 위로.
  useEffect(() => {
    const feed = feedRef.current
    if (feed) feed.scrollTop = feed.scrollHeight
  }, [index])

  const frame = FRAMES[index]

  return (
    <div className="demo" aria-hidden="true">
      <div className="demo-stage">
        {(Object.keys(PEOPLE) as Who[]).map((who) => {
          const p = PEOPLE[who]
          const speaking = frame.speaking === who
          return (
            <div key={who} className={`demo-tile ${speaking ? 'speaking' : ''}`}>
              <span className="demo-avatar" style={{ background: p.color }}>
                {p.initial}
              </span>
              <span className="demo-name">
                {speaking && (
                  <span className="demo-wave">
                    <i />
                    <i />
                    <i />
                  </span>
                )}
                {p.name}
              </span>
            </div>
          )
        })}
      </div>
      <div className="demo-panel">
        <div className="demo-feed" ref={feedRef}>
          {frame.entries.map((e) => (
            <div className="demo-entry" key={e.id}>
              <div className="meta">
                <b>{PEOPLE[e.who].name}</b>
                <time>{e.time}</time>
              </div>
              <p className="demo-orig">
                <span className="lang-tag">{e.src}</span>
                <span>
                  {e.orig}
                  {e.typing && <span className="demo-caret" />}
                </span>
              </p>
              <p className={`demo-tr ${e.interim ? 'interim' : ''}`}>
                <span className="lang-tag lang-tag-tr">{e.tgt}</span>
                <span>{e.tr || '…'}</span>
              </p>
            </div>
          ))}
        </div>
      </div>
    </div>
  )
}
