import { useEffect, useLayoutEffect, useRef, useState } from 'react'
import { useI18n } from '../lib/i18n'
import { languageName } from '../lib/languages'
import { DemoFace } from './DemoFaces'
import { Icon } from './Icon'

/**
 * 홈 화면의 "살아 있는" 제품 예시. 말하는 동안 원문이 타이핑되고, 번역이 한 박자 늦게 따라오며
 * 고쳐지다가 확정되는 이 제품의 핵심 동작을 그대로 재생한다. 대화 내용은 예시라 화면 언어로 바꾸지 않는다(이름표·제목만 t()).
 * 자막은 실제 회의 자막 패널(`CaptionPanel`)과 같은 모양·클래스로 그린다. 보는 사람은 민준(한국어, 상대 일본어)이다.
 * 데모 상자는 높이가 고정이고 자막이 쌓이면 자막 영역 안에서 스크롤된다(페이지가 늘어나지 않음).
 * 의존성 없이 타이머 하나로 돈다. 움직임 줄이기 설정이면 완성된 장면을 정지 화면으로 보여 준다.
 */

type Who = 'sakura' | 'minjun'

type Line = {
  who: Who
  time: string
  orig: string
  /** 원문 진행률(0~1)에 맞춰 보이는 중간 번역 */
  drafts: [number, string][]
  final: string
}

const NAMES: Record<Who, string> = { sakura: 'さくら', minjun: '민준' }
/** 화자가 말하는 언어와 그 말의 번역 언어 */
const LANGS: Record<Who, [string, string]> = { sakura: ['ja', 'ko'], minjun: ['ko', 'ja'] }
// 실제 회의처럼 상대가 왼쪽, 내가 오른쪽
const ORDER: Who[] = ['sakura', 'minjun']
const ME: Who = 'minjun'

const SCRIPT: Line[] = [
  {
    who: 'sakura',
    time: '15:21',
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
    orig: '다음 주에 데모를 보여드려도 될까요?',
    drafts: [
      [0.4, '来週'],
      [0.75, '来週デモを'],
    ],
    final: '来週、デモをお見せしてもよろしいですか？',
  },
]

type EntryView = { id: number; who: Who; time: string; orig: string; tr: string; typing: boolean; final: boolean }
type Frame = { entries: EntryView[]; speaking: Who | null; delay: number }

const TYPE_MS = 90
const CHARS_PER_STEP = 2

/** 대본을 화면 프레임 목록으로 펼친다. */
function buildFrames(): Frame[] {
  const frames: Frame[] = []
  const done: EntryView[] = []
  const view = (line: Line, orig: string, tr: string, typing: boolean, final: boolean): EntryView => ({
    id: SCRIPT.indexOf(line),
    who: line.who,
    time: line.time,
    orig,
    tr,
    typing,
    final,
  })
  for (const line of SCRIPT) {
    const chars = [...line.orig]
    const steps = Math.ceil(chars.length / CHARS_PER_STEP)
    let draft = ''
    for (let k = 1; k <= steps; k++) {
      const progress = k / steps
      for (const [at, text] of line.drafts) if (progress >= at) draft = text
      const current = view(line, chars.slice(0, k * CHARS_PER_STEP).join(''), draft, true, false)
      frames.push({ entries: [...done, current], speaking: line.who, delay: TYPE_MS })
    }
    // 말이 끝난 뒤 잠깐 중간 번역 → 확정 번역으로 교체
    frames.push({ entries: [...done, view(line, line.orig, draft, false, false)], speaking: null, delay: 450 })
    done.push(view(line, line.orig, line.final, false, true))
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
  const { t } = useI18n()
  const [index, setIndex] = useState(() => (prefersReducedMotion() ? FRAMES.length - 1 : 0))
  const feedRef = useRef<HTMLDivElement>(null)

  useEffect(() => {
    if (prefersReducedMotion()) return
    const id = setTimeout(() => setIndex((i) => (i + 1) % FRAMES.length), FRAMES[index].delay)
    return () => clearTimeout(id)
  }, [index])

  // 새 자막이 들어오거나 글자가 늘면 맨 아래(최신 줄)로 따라간다. 처음부터 다시 돌면 맨 위로.
  useLayoutEffect(() => {
    const feed = feedRef.current
    if (feed) feed.scrollTop = feed.scrollHeight
  }, [index])

  const frame = FRAMES[index]
  const name = (who: Who) => (who === ME ? t('room.you', { name: NAMES[who] }) : NAMES[who])

  return (
    <div className="demo" aria-hidden="true">
      <div className="demo-stage">
        {ORDER.map((who) => {
          const speaking = frame.speaking === who
          return (
            <div key={who} className={`demo-tile ${speaking ? 'speaking' : ''}`}>
              <DemoFace who={who} />
              <span className="demo-name">
                {speaking && (
                  <span className="demo-wave">
                    <i />
                    <i />
                    <i />
                  </span>
                )}
                {name(who)}
              </span>
            </div>
          )
        })}
      </div>
      <div className="demo-panel">
        <div className="demo-head">
          <b>{t('captions.title')}</b>
          <span className="demo-langs">
            {languageName('ko')}
            <Icon name="sync_alt" />
            {languageName('ja')}
          </span>
        </div>
        <div className="demo-feed" ref={feedRef}>
          {frame.entries.map((e) => (
            <div className={`entry demo-entry${e.final ? '' : ' pending'}`} key={e.id}>
              <div className="meta">
                <b>{name(e.who)}</b>
                <time>{e.time}</time>
              </div>
              <p lang={LANGS[e.who][0]}>
                {e.orig}
                {e.typing && <span className="demo-caret" />}
              </p>
              <p lang={e.tr ? LANGS[e.who][1] : undefined}>{e.tr || t('captions.translating')}</p>
            </div>
          ))}
        </div>
      </div>
    </div>
  )
}
