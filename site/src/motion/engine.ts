// 움직임 엔진: 모든 값이 시간 t만의 함수라서 어느 순간이든 바로 그릴 수 있다(되감기·멈춘 화면 확인이 쉽다).
// 바꾸는 것은 transform·opacity·filter뿐이다. 레이아웃을 다시 하지 않아 120Hz 화면에서도 매끄럽다.

/** 용수철의 계단 응답: 0에서 출발해 1로 간다. w는 빠르기(고유 진동수), z는 감쇠비(1보다 작으면 살짝 넘쳤다 돌아온다). */
export function spring(t: number, w = 24, z = 0.74): number {
  if (t <= 0) return 0
  if (z >= 1) return 1 - Math.exp(-w * t) * (1 + w * t)
  const wd = w * Math.sqrt(1 - z * z)
  return 1 - Math.exp(-z * w * t) * (Math.cos(wd * t) + ((z * w) / wd) * Math.sin(wd * t))
}

/** [시각, 목표값, 빠르기?, 감쇠비?]. 첫 값이 출발점이다. */
export type Key = [number, number, number?, number?]

/** 목표가 바뀔 때마다 용수철로 따라가는 값. 앞의 움직임이 끝나기 전에 다음 목표가 와도 이어서 움직인다(겹침). */
export const track = (keys: Key[]) => (t: number) => {
  let v = keys[0][1]
  for (let i = 1; i < keys.length; i++) v += (keys[i][1] - keys[i - 1][1]) * spring(t - keys[i][0], keys[i][2], keys[i][3])
  return v
}

/** 여러 점을 지나는 경로(커서 등). 점마다 [출발 시각, x, y]. */
export function glide(points: [number, number, number][], w = 11, z = 0.9) {
  const x = track(points.map(([t, px]) => [t, px, w, z]))
  const y = track(points.map(([t, , py]) => [t, py, w, z]))
  return (t: number): [number, number] => [x(t), y(t)]
}

export const clamp = (x: number) => (x < 0 ? 0 : x > 1 ? 1 : x)
const easeOut = (x: number) => 1 - (1 - x) ** 3
/** 짧게 눌렀다 놓기: 0 → 1 → 0 */
export const bump = (t: number, d = 0.22) => (t > 0 && t < d ? Math.sin((t / d) * Math.PI) : 0)
/** 시각 a부터 d초 동안 0 → 1 */
export const ramp = (t: number, a: number, d: number) => easeOut(clamp((t - a) / d))

export type Look = { x?: number; y?: number; s?: number; sx?: number; sy?: number; o?: number; b?: number }
type Styled = Element & ElementCSSInlineStyle

const written = new WeakMap<Element, string>()
const n = (v: number, d = 2) => v.toFixed(d)

/** 요소 하나의 위치·크기·투명도·흐림을 쓴다. 지난 프레임과 같으면 건드리지 않는다. */
export function put(el: Styled, l: Look) {
  const { x = 0, y = 0, s = 1, o = 1, b = 0 } = l
  const tf = `translate(${n(x)}px,${n(y)}px) scale(${n((l.sx ?? 1) * s, 4)},${n((l.sy ?? 1) * s, 4)})`
  const op = n(clamp(o), 3)
  const fl = b > 0.05 && o > 0.001 ? `blur(${n(b)}px)` : 'none'
  const key = tf + op + fl
  if (written.get(el) === key) return
  written.set(el, key)
  el.style.transform = tf
  el.style.opacity = op
  el.style.filter = fl
}

/** [나타나는 시각, 사라지기 시작하는 시각] */
export type Win = [number, number]

/**
 * 나타남·사라짐 겉모습: 아래에서 흐릿하게 떠오르고(in), 위로 흐려지며 사라진다(out).
 * 여러 구간이면 지금 시각까지 시작한 마지막 구간을 따른다.
 */
export function show(el: Styled, t: number, wins: Win[], l: Look = {}, inD = 0.32, outD = 0.2) {
  let i = 0
  let o = 0
  for (const [a, b] of wins) {
    if (t < a) break
    i = easeOut(clamp((t - a) / inD))
    o = clamp((t - b) / outD) ** 2
  }
  const v = i * (1 - o)
  put(el, {
    ...l,
    o: v * (l.o ?? 1),
    b: 7 * (1 - i) + 5 * o,
    y: (l.y ?? 0) + 7 * (1 - i) - 5 * o,
    s: (l.s ?? 1) * (0.965 + 0.035 * i) * (1 - 0.025 * o),
  })
}

/**
 * 둥근 사각형을 조각 6개(가로·세로 띠 둘, 모서리 원 넷)로 만든다. 조각을 옮기고 늘리기만 해서 모양을 바꾸므로
 * 너비·높이·border-radius를 움직일 때와 달리 다시 그리지 않고, 모서리는 늘 둥근 그대로다.
 * 반환하는 함수에 (x, y, 너비, 높이, 반지름)을 주면 그 사각형을 그린다.
 */
export function plate(parent: HTMLElement, cls: string, maxR: number) {
  const box = document.createElement('div')
  box.className = cls
  const piece = (round: boolean) => {
    const e = document.createElement('i')
    e.className = round ? 'mo-pc mo-po' : 'mo-pc mo-pb'
    if (round) e.style.width = e.style.height = `${maxR * 2}px`
    box.append(e)
    return e
  }
  const [h, v] = [piece(false), piece(false)]
  const corners = [piece(true), piece(true), piece(true), piece(true)]
  parent.append(box)
  return {
    box,
    draw(x: number, y: number, w: number, ht: number, r = maxR) {
      w = Math.max(w, 0)
      ht = Math.max(ht, 0)
      r = Math.max(0, Math.min(r, maxR, w / 2, ht / 2))
      // 띠는 100px 정사각형을 늘린다. 조각끼리 조금 겹쳐 이음매가 보이지 않게 한다.
      put(h, { x, y: y + r - 0.25, sx: w / 100, sy: (ht - 2 * r + 0.5) / 100 })
      put(v, { x: x + r - 0.25, y, sx: (w - 2 * r + 0.5) / 100, sy: ht / 100 })
      const k = r / maxR
      const d = 2 * r
      put(corners[0], { x, y, s: k })
      put(corners[1], { x: x + w - d, y, s: k })
      put(corners[2], { x, y: y + ht - d, s: k })
      put(corners[3], { x: x + w - d, y: y + ht - d, s: k })
    },
  }
}
