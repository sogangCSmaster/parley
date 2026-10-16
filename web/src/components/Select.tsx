import { useEffect, useId, useLayoutEffect, useRef, useState, type KeyboardEvent } from 'react'
import { Icon } from './Icon'
import type { IconName } from './icons'

export type SelectOption<T extends string> = { value: T; label: string }

type Props<T extends string> = {
  value: T
  options: readonly SelectOption<T>[]
  onChange: (value: T) => void
  /** 화면에 보이는 라벨. 없으면 ariaLabel을 쓴다. */
  label?: string
  ariaLabel?: string
  icon?: IconName
  variant?: 'field' | 'pill'
  /** 항상 아래로 펼친다(대면 모드의 180도 돌린 화면처럼 화면 좌표로 위아래를 판단하면 틀리는 곳). */
  alwaysDown?: boolean
}

/**
 * 네이티브 select 대신 쓰는 가벼운 드롭다운(listbox 패턴).
 * 키보드: ↑↓ 이동, Home/End, Enter·Space 선택, Esc 닫기, 글자 입력으로 해당 항목으로 이동.
 */
export function Select<T extends string>({
  value,
  options,
  onChange,
  label,
  ariaLabel,
  icon,
  variant = 'field',
  alwaysDown = false,
}: Props<T>) {
  const id = useId()
  const [open, setOpen] = useState(false)
  const [active, setActive] = useState(0)
  const [dropUp, setDropUp] = useState(false)
  const rootRef = useRef<HTMLDivElement>(null)
  const buttonRef = useRef<HTMLButtonElement>(null)
  const listRef = useRef<HTMLUListElement>(null)
  const selectedIndex = Math.max(
    0,
    options.findIndex((o) => o.value === value),
  )
  const selected = options[selectedIndex]

  function openList() {
    setActive(selectedIndex)
    setOpen(true)
  }

  function close(focusButton = true) {
    setOpen(false)
    if (focusButton) buttonRef.current?.focus()
  }

  function choose(index: number) {
    const option = options[index]
    if (option && option.value !== value) onChange(option.value)
    close()
  }

  // 기본은 아래로 펼친다. 아래 공간이 목록보다 좁고 위쪽이 더 넓을 때만 위로 펼친다. 그리고 목록에 포커스를 준다.
  useLayoutEffect(() => {
    if (!open || !buttonRef.current || !listRef.current) return
    const rect = buttonRef.current.getBoundingClientRect()
    const below = innerHeight - rect.bottom
    setDropUp(!alwaysDown && below < listRef.current.offsetHeight + 8 && rect.top > below)
    listRef.current.focus({ preventScroll: true })
  }, [open, alwaysDown])

  useEffect(() => {
    if (!open) return
    listRef.current?.querySelector(`[data-index="${active}"]`)?.scrollIntoView({ block: 'nearest' })
  }, [open, active])

  // 바깥을 누르면 닫는다.
  useEffect(() => {
    if (!open) return
    const onPointerDown = (e: PointerEvent) => {
      if (!rootRef.current?.contains(e.target as Node)) setOpen(false)
    }
    document.addEventListener('pointerdown', onPointerDown)
    return () => document.removeEventListener('pointerdown', onPointerDown)
  }, [open])

  function onButtonKeyDown(e: KeyboardEvent) {
    if (['ArrowDown', 'ArrowUp', 'Enter', ' '].includes(e.key)) {
      e.preventDefault()
      openList()
    }
  }

  function onListKeyDown(e: KeyboardEvent) {
    const last = options.length - 1
    switch (e.key) {
      case 'ArrowDown':
        setActive((i) => Math.min(last, i + 1))
        break
      case 'ArrowUp':
        setActive((i) => Math.max(0, i - 1))
        break
      case 'Home':
        setActive(0)
        break
      case 'End':
        setActive(last)
        break
      case 'Enter':
      case ' ':
        choose(active)
        break
      case 'Escape':
        close()
        break
      case 'Tab':
        close(false)
        return
      default: {
        // 첫 글자로 찾아가기
        if (e.key.length !== 1) return
        const key = e.key.toLowerCase()
        const start = active + 1
        const order = [...options.slice(start), ...options.slice(0, start)]
        const hit = order.find((o) => o.label.toLowerCase().startsWith(key))
        if (hit) setActive(options.indexOf(hit))
        return
      }
    }
    e.preventDefault()
  }

  const labelId = `${id}-label`
  return (
    <div className={`select select-${variant}`} ref={rootRef}>
      <span className={label ? 'select-label' : 'sr-only'} id={labelId}>
        {label ?? ariaLabel}
      </span>
      <button
        ref={buttonRef}
        type="button"
        className="select-trigger"
        aria-haspopup="listbox"
        aria-expanded={open}
        aria-controls={`${id}-list`}
        aria-labelledby={`${labelId} ${id}-value`}
        onClick={() => (open ? close() : openList())}
        onKeyDown={onButtonKeyDown}
      >
        {icon && <Icon name={icon} />}
        <span className="select-value" id={`${id}-value`}>
          {selected?.label}
        </span>
        <span className="select-chevron">
          <Icon name="expand_more" />
        </span>
      </button>
      {open && (
        <ul
          ref={listRef}
          id={`${id}-list`}
          className={`select-list ${dropUp ? 'up' : ''}`}
          role="listbox"
          tabIndex={-1}
          aria-labelledby={labelId}
          aria-activedescendant={`${id}-opt-${active}`}
          onKeyDown={onListKeyDown}
        >
          {options.map((o, i) => (
            <li
              key={o.value}
              id={`${id}-opt-${i}`}
              data-index={i}
              role="option"
              aria-selected={o.value === value}
              className={i === active ? 'active' : undefined}
              onPointerEnter={() => setActive(i)}
              onClick={() => choose(i)}
            >
              <span>{o.label}</span>
              {o.value === value && <Icon name="check" />}
            </li>
          ))}
        </ul>
      )}
    </div>
  )
}
