import { type FormEvent } from 'react'
import type { MediaState } from '../protocol/MediaState'
import type { RoomSettings } from '../protocol/RoomSettings'
import { useI18n } from '../lib/i18n'
import { useAuth } from '../lib/auth'
import type { LocalMedia } from '../lib/media'
import type { Prefs } from '../lib/prefs'
import { Icon } from './Icon'
import { LanguageSelects } from './LanguageSelects'
import { Header } from './Header'
import { VideoTile } from './VideoTile'

type Props = {
  code: string
  local: LocalMedia | null
  /** undefined면 아직 확인 중 */
  room: RoomSettings | undefined
  prefs: Prefs
  media: MediaState
  onPrefsChange: (p: Prefs) => void
  onMediaChange: (m: MediaState) => void
  onJoin: () => void
}

export function Lobby({ code, local, room, prefs, media, onPrefsChange, onMediaChange, onJoin }: Props) {
  const { t } = useI18n()
  const auth = useAuth()
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
          <h1>{t('lobby.title')}</h1>
          <p className="lead">{code}</p>
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
          {room?.translation && (
            <>
              <LanguageSelects prefs={prefs} onChange={onPrefsChange} />
              <p className="lobby-note save-note">{t('captions.saveNotice')}</p>
            </>
          )}
          <button className="btn btn-primary btn-lg" disabled={!name || !local || !room}>
            {auth.status === 'host' ? t('lobby.joinNow') : t('lobby.askToJoin')}
          </button>
        </form>
      </main>
    </div>
  )
}
