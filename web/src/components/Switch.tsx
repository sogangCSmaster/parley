type Props = { checked: boolean; onChange: (checked: boolean) => void; label: string; disabled?: boolean }

export function Switch({ checked, onChange, label, disabled }: Props) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={checked}
      className="switch-row"
      disabled={disabled}
      onClick={() => onChange(!checked)}
    >
      <span>{label}</span>
      <span className="switch" aria-hidden="true" />
    </button>
  )
}
