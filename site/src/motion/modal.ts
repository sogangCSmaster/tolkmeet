// 기능 카드를 누르면 카드가 가운데의 대화 상자로 커지고(같은 용수철), 다 커지면 장면이 돈다. 닫으면 다시 카드로 줄어든다.
// 대화 상자는 네이티브 <dialog>(showModal)라 포커스가 안에 갇히고 Esc로 닫힌다. 바탕(배경판)은 조각 판(plate)이라
// 커지는 동안 모서리가 늘 둥글고 다시 그리지 않는다. 움직임 줄이기 설정이면 커지지 않고 멈춘 그림만 보인다.
import { clamp, plate, put, spring } from './engine'
import { build, type Scene } from './scenes'

type Rect = [x: number, y: number, w: number, h: number, r: number]
type Phase = 'idle' | 'grow' | 'play' | 'still' | 'shrink'
const CARD_R = 22
const DIALOG_R = 28
const GROW = 0.7
const SHRINK = 0.5

function setup(dlg: HTMLDialogElement) {
  const inner = dlg.querySelector<HTMLElement>('.mo-in')!
  const area = dlg.querySelector<HTMLElement>('.mo-area')!
  const title = dlg.querySelector<HTMLElement>('#mo-t')!
  const desc = dlg.querySelector<HTMLElement>('#mo-d')!
  const pause = dlg.querySelector<HTMLButtonElement>('.mo-pause')!
  const bg = plate(dlg.querySelector<HTMLElement>('.mo-plate')!, 'mo-bg', DIALOG_R)
  const reduce = matchMedia('(prefers-reduced-motion: reduce)')

  let phase: Phase = 'idle'
  let card: HTMLElement | null = null
  let scene: Scene | null = null
  let from: Rect = [0, 0, 0, 0, 0]
  let to: Rect = from
  let now: Rect = from
  let clock = 0 // 지금 단계(커짐·장면·줄어듦)가 시작된 뒤 흐른 초
  let sceneT = 0
  let paused = false
  let last = 0
  let raf = 0

  const lerp = (a: Rect, b: Rect, p: number) => a.map((v, i) => v + (b[i] - v) * p) as Rect
  const full = (): Rect => [0, 0, dlg.offsetWidth, dlg.offsetHeight, DIALOG_R]
  const cardRect = (): Rect => {
    const c = card!.getBoundingClientRect()
    const d = dlg.getBoundingClientRect()
    return [c.left - d.left, c.top - d.top, c.width, c.height, CARD_R]
  }
  // 무대(384×240)를 대화 상자 너비와 화면 높이에 맞춘다. 너무 커지지 않게 1.35배까지.
  const fit = () => dlg.style.setProperty('--s', Math.min(1.7, area.clientWidth / 384, (innerHeight * 0.56) / 240).toFixed(3))
  const run = () => {
    last = performance.now()
    raf ||= requestAnimationFrame(tick)
  }
  const stop = () => {
    cancelAnimationFrame(raf)
    raf = 0
  }

  function open(li: HTMLElement) {
    if (phase !== 'idle') return
    const source = li.querySelector<HTMLElement>('.mo-stage')
    if (!source) return
    card = li
    title.textContent = li.querySelector('h3')?.textContent ?? ''
    desc.textContent = li.dataset.desc ?? ''
    const stage = source.cloneNode(true) as HTMLElement
    area.replaceChildren(stage)
    paused = false
    pause.setAttribute('aria-pressed', 'false')
    dlg.showModal()
    fit()
    if (reduce.matches) {
      // 커지지도 돌지도 않는다: 카드 뒷면과 같은 멈춘 그림과 설명만 보인다.
      phase = 'still'
      dlg.classList.add('is-set', 'is-in')
      return
    }
    stage.classList.replace('mo-static', 'mo-live')
    scene = build(stage)
    scene.frame(0)
    sceneT = 0
    from = cardRect()
    to = full()
    growAt(0)
    li.classList.add('mo-away')
    phase = 'grow'
    clock = 0
    run()
  }

  function growAt(e: number) {
    now = lerp(from, to, spring(e, 12.5, 0.78))
    bg.draw(...now)
    const o = 1 - (1 - clamp((e - 0.14) / 0.32)) ** 3
    put(inner, { o, b: 6 * (1 - o), s: 0.97 + 0.03 * o })
    // 열린 다음 프레임에 바탕을 어둡게 해야 전환(CSS)이 보인다.
    if (e > 0) dlg.classList.add('is-in')
  }

  function settle() {
    phase = 'play'
    clock = 0
    put(inner, {})
    dlg.classList.add('is-set')
  }

  function shrinkAt(e: number) {
    now = lerp(from, to, spring(e, 20, 0.9))
    bg.draw(...now)
    const o = 1 - clamp(e / 0.14)
    put(inner, { o, b: 6 * (1 - o), s: 1 - 0.03 * (1 - o) })
  }

  function close() {
    if (phase === 'idle' || phase === 'shrink') return
    if (phase === 'still') return finish()
    from = phase === 'grow' ? now : full()
    bg.draw(...from)
    dlg.classList.remove('is-set', 'is-in')
    to = cardRect()
    phase = 'shrink'
    clock = 0
    run()
  }

  function finish() {
    stop()
    phase = 'idle'
    if (dlg.open) dlg.close()
    dlg.classList.remove('is-set', 'is-in')
    put(inner, {})
    area.replaceChildren()
    scene = null
    const li = card
    card = null
    if (li) {
      li.classList.remove('mo-away', 'is-flip')
      li.querySelector<HTMLElement>('.dt-btn')?.focus({ preventScroll: true })
    }
  }

  function tick(ts: number) {
    raf = 0
    const dt = Math.min(0.05, Math.max(0, (ts - last) / 1000))
    last = ts
    clock += dt
    if (phase === 'grow') {
      growAt(clock)
      if (clock >= GROW) settle()
    } else if (phase === 'shrink') {
      shrinkAt(clock)
      if (clock >= SHRINK) return finish()
    } else if (phase === 'play' && scene) {
      if (paused) return
      sceneT += dt
      scene.frame(sceneT % scene.dur)
    } else return
    raf = requestAnimationFrame(tick)
  }

  document.addEventListener('mo-open', (e) => open(e.target as HTMLElement))
  // Esc: 바로 닫지 않고 줄어들며 닫는다. 브라우저가 직접 닫아 버린 경우(close)에는 곧바로 정리한다.
  dlg.addEventListener('cancel', (e) => {
    e.preventDefault()
    close()
  })
  dlg.addEventListener('close', () => {
    if (phase !== 'idle') finish()
  })
  // 대화 상자 바깥(::backdrop)을 누르면 대상이 dialog 자신이다. 안쪽은 .mo-in이 덮고 있다.
  dlg.addEventListener('click', (e) => {
    if (e.target === dlg) close()
  })
  dlg.querySelector('.mo-x')?.addEventListener('click', close)
  pause.addEventListener('click', () => {
    paused = !paused
    pause.setAttribute('aria-pressed', String(paused))
    if (!paused && phase === 'play') run()
  })
  // 탭이 가려지면 멈췄다가 돌아오면 이어 간다.
  document.addEventListener('visibilitychange', () => {
    if (document.hidden) stop()
    else if (phase === 'grow' || phase === 'shrink' || (phase === 'play' && !paused)) run()
  })
  addEventListener('resize', () => {
    if (phase !== 'idle') fit()
  })

  // 개발 서버에서만: 화면 확인 스크립트가 원하는 순간을 멈춰 그린다(배포 빌드에는 들어가지 않는다).
  if (import.meta.env.DEV) {
    Object.assign(window, {
      __mo: {
        seek(t: number) {
          stop()
          if (phase === 'grow') settle()
          paused = true
          scene?.frame(t)
        },
        grow(e: number) {
          stop()
          growAt(e)
        },
      },
    })
  }
}

const dlg = document.querySelector<HTMLDialogElement>('.mo-dlg')
if (dlg) setup(dlg)
