import { useEffect, useRef } from 'react'
import { useI18n } from '../lib/i18n'
import { Icon } from './Icon'

type Props = {
  name: string
  stream: MediaStream | null
  showVideo: boolean
  micOn: boolean
  mirror?: boolean
  /** 내 타일은 음소거해 에코를 막는다. */
  muted?: boolean
  presenting?: boolean
  className?: string
}

export function VideoTile({ name, stream, showVideo, micOn, mirror, muted, presenting, className }: Props) {
  const { t } = useI18n()
  const ref = useRef<HTMLVideoElement>(null)

  useEffect(() => {
    if (ref.current && ref.current.srcObject !== stream) ref.current.srcObject = stream
  }, [stream])

  return (
    <div className={`tile ${className ?? ''}`}>
      {/* 카메라가 꺼져도 상대 음성은 계속 들려야 하므로 video 요소는 숨기기만 한다. */}
      <video
        ref={ref}
        autoPlay
        playsInline
        muted={muted}
        className={mirror ? 'mirror' : undefined}
        style={{ visibility: showVideo ? 'visible' : 'hidden' }}
      />
      {!showVideo && (
        <div className="avatar" style={{ background: avatarColor(name) }} aria-hidden="true">
          {[...name][0] ?? '?'}
        </div>
      )}
      <span className="name">
        {presenting && <Icon name="present_to_all" />}
        {name}
      </span>
      {!micOn && (
        <span className="tile-status" aria-label={t('room.micMuted')}>
          <Icon name="mic_off" />
        </span>
      )}
    </div>
  )
}

// 흰 이니셜 대비 4.5:1 이상인 브랜드 계열 색
const AVATAR_COLORS = ['#C2381A', '#2459D6', '#0E7F6A', '#6D44D8', '#A45A12', '#B02F86']

function avatarColor(name: string): string {
  let h = 0
  for (const ch of name) h = (h * 31 + ch.codePointAt(0)!) >>> 0
  return AVATAR_COLORS[h % AVATAR_COLORS.length]
}
