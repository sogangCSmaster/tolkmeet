import { useRef } from 'react'

type Props = {
  value: string
  onChange: (v: string) => void
  /** 6자리를 다 채우면 */
  onComplete?: (v: string) => void
  label: string
  /** 확인 중에는 입력만 막는다(disabled로 막으면 포커스가 빠져 틀린 뒤 바로 다시 칠 수 없다). */
  disabled?: boolean
  autoFocus?: boolean
}

/**
 * 6자리 숫자 코드 칸. 칸은 보이는 모양일 뿐이고, 실제 입력은 숨긴 input 하나가 받는다
 * (붙여넣기·자동 채움(one-time-code)·화면 낭독기가 한 칸으로 다룬다).
 */
export function CodeInput({ value, onChange, onComplete, label, disabled, autoFocus }: Props) {
  const ref = useRef<HTMLInputElement>(null)
  const digits = value.padEnd(6, ' ').slice(0, 6).split('')
  const active = Math.min(value.length, 5)
  return (
    <div className="code-input" onClick={() => ref.current?.focus()}>
      <input
        ref={ref}
        className="code-input-real"
        inputMode="numeric"
        autoComplete="one-time-code"
        aria-label={label}
        value={value}
        readOnly={disabled}
        aria-busy={disabled || undefined}
        autoFocus={autoFocus}
        onChange={(e) => {
          const v = e.target.value.replace(/\D/g, '').slice(0, 6)
          onChange(v)
          if (v.length === 6) onComplete?.(v)
        }}
      />
      {digits.map((d, i) => (
        <span key={i} className={`code-box${i === active ? ' active' : ''}`} aria-hidden="true">
          {d.trim()}
        </span>
      ))}
    </div>
  )
}
