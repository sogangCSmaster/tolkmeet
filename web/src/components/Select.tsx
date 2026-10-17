import { useEffect, useId, useLayoutEffect, useRef, useState, type KeyboardEvent } from 'react'
import { Icon } from './Icon'
import type { IconName } from './icons'

export type SelectOption<T extends string> = { value: T; label: string }

/** 아무리 좁아도 항목 두세 개는 보이게 */
const MIN_LIST = 112

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
  /**
   * 목록을 펼칠 자리. up: 버튼 위로. maxHeight: 자리가 좁을 때만 줄인 높이(없으면 CSS의 최대 높이).
   * top: 위아래 모두 좁을 때 버튼을 덮어 펼칠 위치(.select-box 기준).
   */
  const [place, setPlace] = useState<{ up?: boolean; maxHeight?: number; top?: number }>({})
  const rootRef = useRef<HTMLDivElement>(null)
  const boxRef = useRef<HTMLDivElement>(null)
  const buttonRef = useRef<HTMLButtonElement>(null)
  const listRef = useRef<HTMLUListElement>(null)
  const selectedIndex = Math.max(
    0,
    options.findIndex((o) => o.value === value),
  )
  const selected = options[selectedIndex]

  function openList() {
    setActive(selectedIndex)
    setPlace({})
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
  // 공간은 화면만이 아니라 내용을 잘라 내는 조상(회의 중 자막 패널처럼 overflow가 있는 상자) 안쪽으로 잰다.
  // 어느 쪽도 목록보다 좁으면 넓은 쪽에 맞춰 목록 높이를 줄인다(목록 안에서 스크롤).
  // 어느 쪽에도 항목 두세 개가 들어갈 자리가 없으면(아주 낮은 휴대폰의 자막 패널) 보이는 영역 안에서 버튼을 덮어 펼친다.
  useLayoutEffect(() => {
    if (!open || !boxRef.current || !buttonRef.current || !listRef.current) return
    const list = listRef.current
    if (!alwaysDown) {
      const rect = buttonRef.current.getBoundingClientRect()
      let top = 0
      let bottom = innerHeight
      for (let el = rootRef.current?.parentElement; el && el !== document.body; el = el.parentElement) {
        if (getComputedStyle(el).overflowY === 'visible') continue
        const r = el.getBoundingClientRect()
        top = Math.max(top, r.top)
        bottom = Math.min(bottom, r.bottom)
      }
      const below = bottom - rect.bottom - 8
      const above = rect.top - top - 8
      const need = list.offsetHeight
      const up = below < need && above > below
      const room = up ? above : below
      if (room >= Math.min(need, MIN_LIST)) setPlace({ up, maxHeight: room < need ? Math.floor(room) : undefined })
      else {
        const box = boxRef.current.getBoundingClientRect()
        setPlace({ top: Math.round(top + 8 - box.top), maxHeight: Math.min(need, Math.max(0, Math.floor(bottom - top - 16))) })
      }
    } else {
      // 항상 아래로 펼치는 목록(대면 대화의 뒤집힌 쪽)도 화면 밖으로 나가는 만큼 줄인다.
      // 돌려 놓은 상자도 getBoundingClientRect는 화면 기준이라 그대로 잰다.
      const r = list.getBoundingClientRect()
      const over = Math.max(0, 8 - r.top) + Math.max(0, r.bottom - (innerHeight - 8))
      if (over > 0) setPlace({ maxHeight: Math.max(MIN_LIST, Math.floor(list.offsetHeight - over)) })
    }
    list.focus({ preventScroll: true })
  }, [open, alwaysDown])

  // 지금 항목이 보이게 목록만 스크롤한다. scrollIntoView는 잘라 내는 조상(자막 패널)까지 스크롤해 머리줄을 밀어낸다.
  // 펼칠 자리를 정한 뒤(높이가 줄었을 수 있다)에도 다시 맞춘다.
  useEffect(() => {
    const list = listRef.current
    const item = list?.querySelector<HTMLElement>(`[data-index="${active}"]`)
    if (!open || !list || !item) return
    const top = item.offsetTop
    const bottom = top + item.offsetHeight
    if (top < list.scrollTop) list.scrollTop = top
    else if (bottom > list.scrollTop + list.clientHeight) list.scrollTop = bottom - list.clientHeight
  }, [open, active, place])

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
      {/* 목록은 라벨이 아니라 버튼에 붙인다: 위로 펼칠 때도 버튼 바로 위에서 펼쳐져 잰 공간과 맞는다. */}
      <div className="select-box" ref={boxRef}>
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
            className={`select-list ${place.up ? 'up' : ''}`}
            style={{ maxHeight: place.maxHeight, top: place.top }}
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
    </div>
  )
}
