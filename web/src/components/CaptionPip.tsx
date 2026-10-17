import { useEffect, useRef } from 'react'
import { createRoot, type Root } from 'react-dom/client'
import type { Caption } from '../lib/captions'
import { useI18n } from '../lib/i18n'
import { Icon } from './Icon'

type Props = {
  win: Window
  captions: Caption[]
  speakerName: (id: string) => string
  micOn: boolean
  hasMic: boolean
  onMic: () => void
  onStop: () => void
}

/**
 * 화면 공유 중에 띄우는 자막 창. 다른 문서(PiP 창)에 그리므로 그 문서에 따로 React 루트를 만든다
 * (포털로는 다른 문서의 클릭이 회의 화면의 React까지 오지 않는다).
 */
export function CaptionPip({ win, ...props }: Props) {
  const rootRef = useRef<Root | null>(null)

  useEffect(() => {
    const mounted = mountIn(win)
    rootRef.current = mounted.root
    return () => {
      rootRef.current = null
      mounted.dispose()
    }
  }, [win])

  useEffect(() => {
    rootRef.current?.render(<PipContent {...props} />)
  })

  return null
}

/** PiP 문서에 그릴 자리를 만들고 React 루트를 연다. */
function mountIn(win: Window): { root: Root; dispose: () => void } {
  const doc = win.document
  const host = doc.createElement('div')
  doc.body.className = 'pip-body'
  doc.body.appendChild(host)
  const root = createRoot(host)
  return {
    root,
    dispose: () => {
      // 그리는 도중 해제하지 않도록 다음 틱에 정리한다.
      queueMicrotask(() => root.unmount())
      host.remove()
    },
  }
}

function PipContent({ captions, speakerName, micOn, hasMic, onMic, onStop }: Omit<Props, 'win'>) {
  const { t } = useI18n()
  // 마지막 두 줄만: 지난 줄은 작고 흐리게, 방금 줄은 크게.
  const visible = captions.filter((c) => !(c.final && c.src === '')).slice(-2)
  return (
    <div className="pip">
      <div className="pip-head">
        <span className="pip-live">
          <i aria-hidden="true" />
          {t('pip.sharing')}
        </span>
        {hasMic && (
          <button
            className={`pip-mic${micOn ? '' : ' off'}`}
            aria-pressed={micOn}
            aria-label={micOn ? t('ctl.micOff') : t('ctl.micOn')}
            onClick={onMic}
          >
            <Icon name={micOn ? 'mic' : 'mic_off'} />
          </button>
        )}
        <button className="pip-stop" onClick={onStop}>
          {t('pip.stop')}
        </button>
      </div>
      <div className="pip-lines" aria-live="polite">
        {visible.length === 0 && <p className="pip-empty">{t('pip.empty')}</p>}
        {visible.map((c, i) => {
          const latest = i === visible.length - 1
          const translated = c.tgt_lang !== c.src_lang && (c.text != null || !c.final)
          const name = speakerName(c.speaker)
          return (
            <div key={c.seg} className={`pip-line${latest ? ' latest' : ''}${c.final ? '' : ' pending'}`}>
              {latest && (
                <span className="pip-avatar" aria-hidden="true">
                  {[...name][0] ?? '?'}
                </span>
              )}
              <div className="pip-text">
                <p className="pip-src" lang={c.src_lang}>
                  <b>{name}</b> {translated ? c.src : ''}
                </p>
                <p className="pip-tgt tr-mark" lang={translated ? c.tgt_lang : c.src_lang}>
                  {translated ? (c.text ?? t('captions.translating')) : c.src}
                </p>
              </div>
            </div>
          )
        })}
      </div>
    </div>
  )
}
