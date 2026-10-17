import { useEffect, useRef } from 'react'
import { useI18n } from '../lib/i18n'
import { Icon } from './Icon'

type Props = {
  name: string
  stream: MediaStream | null
  showVideo: boolean
  micOn: boolean
  /** 지금 말하고 있다(받은 오디오 레벨 기준). 주황 테두리와 음성 막대를 보인다. */
  speaking?: boolean
  mirror?: boolean
  /** 내 타일은 음소거해 에코를 막는다. */
  muted?: boolean
  presenting?: boolean
  className?: string
}

export function VideoTile({ name, stream, showVideo, micOn, speaking, mirror, muted, presenting, className }: Props) {
  const { t } = useI18n()
  const ref = useRef<HTMLVideoElement>(null)

  useEffect(() => {
    if (ref.current && ref.current.srcObject !== stream) ref.current.srcObject = stream
  }, [stream])

  return (
    <div className={`tile${speaking ? ' speaking' : ''}${showVideo ? ' has-video' : ''} ${className ?? ''}`}>
      {/* 카메라가 꺼져도 상대 음성은 계속 들려야 하므로 video 요소는 숨기기만 한다. */}
      <video
        ref={ref}
        autoPlay
        playsInline
        muted={muted}
        className={mirror ? 'mirror' : undefined}
        style={{ visibility: showVideo ? 'visible' : 'hidden' }}
      />
      {/* 카메라가 꺼지면 회색 모노그램(이니셜)과 이름을 가운데에. 사진은 받지 않는다. */}
      {!showVideo && (
        <div className="monogram-wrap" aria-hidden="true">
          <span className="monogram">{[...name][0] ?? '?'}</span>
          <span className="levels">
            <i />
            <i />
            <i />
            <i />
          </span>
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
