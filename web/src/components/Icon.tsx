import { ICONS, type IconName } from './icons'

export function Icon({ name }: { name: IconName }) {
  return (
    <svg className="icon" viewBox="0 -960 960 960" aria-hidden="true" focusable="false">
      <path d={ICONS[name]} />
    </svg>
  )
}
