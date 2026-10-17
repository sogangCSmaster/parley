type Props = {
  checked: boolean
  onChange: (checked: boolean) => void
  label: string
  disabled?: boolean
  /** 이름표를 화면에서 숨기고 화면 낭독기에만 읽힌다(옆에 같은 제목이 이미 있을 때) */
  hideLabel?: boolean
}

export function Switch({ checked, onChange, label, disabled, hideLabel }: Props) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={checked}
      className="switch-row"
      disabled={disabled}
      onClick={() => onChange(!checked)}
    >
      <span className={hideLabel ? 'sr-only' : undefined}>{label}</span>
      <span className="switch" aria-hidden="true" />
    </button>
  )
}
