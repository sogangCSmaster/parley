import { useLayoutEffect, useRef } from 'react'
import type { Caption } from '../lib/captions'
import { useI18n } from '../lib/i18n'
import type { Prefs } from '../lib/prefs'
import type { SttStatus } from '../lib/stt'
import { Icon } from './Icon'
import { LanguageSelects } from './LanguageSelects'
import { Switch } from './Switch'

type Props = {
  prefs: Prefs
  translation: boolean
  isHost: boolean
  captions: Caption[]
  sttStatus: SttStatus | null
  speakerName: (id: string) => string
  onPrefsChange: (p: Prefs) => void
  onTranslationChange: (on: boolean) => void
  onClose: () => void
}

export function CaptionPanel({
  prefs,
  translation,
  isHost,
  captions,
  sttStatus,
  speakerName,
  onPrefsChange,
  onTranslationChange,
  onClose,
}: Props) {
  const { t } = useI18n()
  return (
    <aside className="panel" aria-label={t('captions.title')}>
      <div className="panel-head">
        <h2>{t('captions.title')}</h2>
        <button className="icon-btn" aria-label={t('captions.close')} onClick={onClose}>
          <Icon name="close" />
        </button>
      </div>
      {isHost && (
        <div className="panel-host">
          <Switch checked={translation} onChange={onTranslationChange} label={t('captions.translation')} />
        </div>
      )}
      {translation ? (
        <>
          <LanguageSelects prefs={prefs} onChange={onPrefsChange} />
          {(sttStatus === 'paused' || sttStatus === 'unavailable') && (
            <p className="stt-status" role="status">
              <Icon name="mic_off" />
              {t(sttStatus === 'paused' ? 'captions.sttPaused' : 'captions.sttUnavailable')}
            </p>
          )}
          <Feed captions={captions} speakerName={speakerName} />
        </>
      ) : (
        <div className="feed">
          <p className="feed-empty">
            {t('captions.off')}
            {isHost && (
              <>
                <br />
                {t('captions.offHost')}
              </>
            )}
          </p>
        </div>
      )}
    </aside>
  )
}

/** 사용자가 맨 아래를 보고 있을 때만 새 줄을 따라 내려간다. 위로 스크롤했으면 멈춘다. */
function Feed({ captions, speakerName }: { captions: Caption[]; speakerName: (id: string) => string }) {
  const { t, locale } = useI18n()
  const ref = useRef<HTMLDivElement>(null)
  const stick = useRef(true)
  // 취소 기록은 낮은 rev로 되살아나지 않도록 상태에 남기고 화면에서만 숨긴다.
  const visible = captions.filter((c) => !(c.final && c.src === ''))

  useLayoutEffect(() => {
    const el = ref.current
    if (el && stick.current) el.scrollTop = el.scrollHeight
  }, [captions])

  function onScroll() {
    const el = ref.current
    if (el) stick.current = el.scrollHeight - el.scrollTop - el.clientHeight < 24
  }

  return (
    <div className="feed" role="log" aria-live="polite" ref={ref} onScroll={onScroll}>
      {visible.length === 0 && <p className="feed-empty">{t('captions.empty')}</p>}
      {visible.map((c) => (
        // 확정 전 줄은 계속 바뀌므로 화면 낭독기에는 확정된 줄만 읽힌다.
        <div key={c.seg} className={c.final ? 'entry' : 'entry pending'} aria-hidden={!c.final}>
          <div className="meta">
            <b>{speakerName(c.speaker)}</b>
            <time>{new Date(c.at).toLocaleTimeString(locale, { hour: '2-digit', minute: '2-digit', hour12: false })}</time>
          </div>
          <p lang={c.src_lang}>{c.src}</p>
          {c.text != null ? (
            <p lang={c.tgt_lang}>{c.text}</p>
          ) : (
            !c.final && c.tgt_lang !== c.src_lang && <p>{t('captions.translating')}</p>
          )}
        </div>
      ))}
    </div>
  )
}
