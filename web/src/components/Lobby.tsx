import { useEffect, useRef, type FormEvent } from 'react'
import type { MediaState } from '../protocol/MediaState'
import type { RoomInfo } from '../lib/api'
import { useI18n } from '../lib/i18n'
import { useAuth, userName } from '../lib/auth'
import type { LocalMedia } from '../lib/media'
import type { Prefs } from '../lib/prefs'
import { legalUrl } from '../lib/links'
import { Icon } from './Icon'
import { LanguageSelects } from './LanguageSelects'
import { Rich } from './Rich'
import { Header } from './Header'
import { VideoTile } from './VideoTile'

type Props = {
  code: string
  local: LocalMedia | null
  /** undefined면 아직 확인 중 */
  room: RoomInfo | undefined
  prefs: Prefs
  media: MediaState
  onPrefsChange: (p: Prefs) => void
  onMediaChange: (m: MediaState) => void
  onJoin: () => void
}

export function Lobby({ code, local, room, prefs, media, onPrefsChange, onMediaChange, onJoin }: Props) {
  const { t, locale } = useI18n()
  const auth = useAuth()
  // 로그인한 주최자는 처음 한 번만 계정의 "회의에서 보이는 이름"을 채워 둔다(지우고 새로 써도 되돌아오지 않게).
  const filled = useRef(false)
  useEffect(() => {
    if (filled.current || auth.status === 'loading') return
    filled.current = true
    if (!prefs.name && auth.status === 'host') onPrefsChange({ ...prefs, name: userName(auth.user) })
  }, [auth, prefs, onPrefsChange])
  const hasMic = !!local?.stream.getAudioTracks().length
  const hasCam = !!local?.stream.getVideoTracks().length
  const name = prefs.name.trim()

  function submit(e: FormEvent) {
    e.preventDefault()
    if (name && local && room) onJoin()
  }

  return (
    <div className="shell">
      <Header />
      <main className="lobby">
        <div className="lobby-preview">
          <VideoTile
            name={name || t('lobby.me')}
            stream={local?.stream ?? null}
            showVideo={hasCam && media.cam}
            micOn={hasMic && media.mic}
            mirror
            muted
            offLabel={local && hasCam ? t('lobby.camOff') : undefined}
          />
          <div className="lobby-controls">
            <button
              className={`ctl ${hasMic && media.mic ? '' : 'off'}`}
              disabled={!hasMic}
              aria-pressed={media.mic}
              aria-label={media.mic ? t('ctl.micOff') : t('ctl.micOn')}
              onClick={() => onMediaChange({ ...media, mic: !media.mic })}
            >
              <Icon name={hasMic && media.mic ? 'mic' : 'mic_off'} />
            </button>
            <button
              className={`ctl ${hasCam && media.cam ? '' : 'off'}`}
              disabled={!hasCam}
              aria-pressed={media.cam}
              aria-label={media.cam ? t('ctl.camOff') : t('ctl.camOn')}
              onClick={() => onMediaChange({ ...media, cam: !media.cam })}
            >
              <Icon name={hasCam && media.cam ? 'videocam' : 'videocam_off'} />
            </button>
          </div>
          {!local && <p className="lobby-note">{t('lobby.preparing')}</p>}
          {local?.error && (
            <p className="form-error" role="alert">
              {t(`media.${local.error.kind}`, { device: t(`media.${local.error.device}`) })}
            </p>
          )}
        </div>

        <form className="lobby-form" onSubmit={submit}>
          <div>
            {/* 주최자 이름(회의에서 보이는 이름)을 알면 함께, 없으면 회의 코드를 보인다. */}
            <p className="status-eyebrow">{room?.host ? t('lobby.hostMeeting', { host: room.host }) : t('lobby.meeting', { code })}</p>
            <h1>{room?.title || t('lobby.title')}</h1>
          </div>
          <label className="field-label">
            {t('lobby.name')}
            <input
              className="input"
              value={prefs.name}
              maxLength={40}
              autoFocus
              onChange={(e) => onPrefsChange({ ...prefs, name: e.target.value })}
              placeholder={t('lobby.namePlaceholder')}
            />
          </label>
          {room?.translation && <LanguageSelects prefs={prefs} onChange={onPrefsChange} />}
          <button className="btn btn-primary btn-lg" disabled={!name || !local || !room}>
            {auth.status === 'host' ? t('lobby.joinNow') : t('lobby.askToJoin')}
          </button>
          {(auth.status !== 'host' || room?.translation) && (
            <p className="lobby-note save-note">
              {auth.status !== 'host' && t('lobby.guestNote')} {room?.translation && t('captions.saveNotice')}
            </p>
          )}
          {/* 손님도 입장 전에 약관·처리방침을 보고, 번역이 켜진 회의면 음성이 Gemini로 간다는 것도 본다. */}
          <p className="lobby-note">
            <Rich
              text={t('lobby.agree')}
              parts={{
                terms: (
                  <a href={legalUrl('terms', locale)} target="_blank" rel="noreferrer">
                    {t('lobby.terms')}
                  </a>
                ),
                privacy: (
                  <a href={legalUrl('privacy', locale)} target="_blank" rel="noreferrer">
                    {t('lobby.privacy')}
                  </a>
                ),
              }}
            />
            {room?.translation && <> {t('lobby.geminiNote')}</>}
          </p>
        </form>
      </main>
    </div>
  )
}
