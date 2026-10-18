// 기능 카드 여섯 장의 움직임 장면. 마크업과 문구는 components/Scene.astro에 있고, 여기서는 시간표만 정한다.
// 무대는 384×240, 원점은 가운데. 가운데의 판(surface) 하나가 알약 → 카드 → 알림으로 모양을 바꾸고, 그 위의 층이
// 흐림과 함께 나타났다 사라진다. 장면은 처음 상태로 돌아와 끝나서 이어 돌려도 끊기지 않는다.
import { bump, clamp, glide, plate, put, ramp, show, spring, track, type Key } from './engine'

export type Scene = { dur: number; frame: (t: number) => void }

type Size = [number, number]
type Kit = {
  q: (name: string) => HTMLElement
  qa: (name: string) => HTMLElement[]
  size: (layer: string) => Size
  at: (el: Element) => Size
  /** 가운데 판: 너비, 높이, 어두운 정도(0 흰색 → 1 검정), 눌림 배율, 세로 어긋남 */
  surface: (w: number, h: number, dark?: number, k?: number, oy?: number) => void
  /** 경로를 따라 움직이고 clicks 시각마다 누르는 커서 */
  pointer: (path: (t: number) => Size, clicks: number[]) => (t: number) => void
  /** 글자가 왼쪽부터 드러나는 줄(덮개를 밀어 낸다). p: 0 → 1 */
  typer: (el: HTMLElement) => (p: number) => void
}

const REST: Size = [150, 92]

/** 판의 크기 변화: [시각, 크기] 목록 → 너비·높이 두 값 */
function morph(steps: [number, Size][]) {
  const w = track(steps.map(([t, s]) => [t, s[0]] as Key))
  const h = track(steps.map(([t, s]) => [t, s[1]] as Key))
  return (t: number) => [w(t), h(t)] as Size
}
/** 판의 밝기 변화(넘치지 않게 감쇠비 1) */
const shade = (steps: [number, number][]) => track(steps.map(([t, v]) => [t, v, 20, 1] as Key))
/** 눌린 쪽으로 커서 끝이 조금 들어가게 */
const aim = ([x, y]: Size, dx = 8, dy = 6): Size => [x + dx, y + dy]
/** 소리 막대 하나의 높이(0~1). 막대마다 박자가 달라 말소리처럼 보인다. */
const wave = (t: number, i: number) => Math.abs(Math.sin(t * (7 + (i % 5) * 1.9) + i * 1.3) * (0.6 + 0.4 * Math.sin(t * 3.1 + i)))

const scenes: Record<string, (k: Kit) => Scene> = {
  // 9개 언어: 알약 → 언어 칩 → 日本語 고르기 → 알림 → 알약
  lang(k) {
    const pill = k.q('pill'), chips = k.q('chips'), toast = k.q('toast'), cap = k.q('cap'), on = k.q('on')
    const pick = on.parentElement as HTMLElement
    const chip = k.qa('chip')
    const P = k.size('pill'), C = k.size('chips'), T = k.size('toast')
    const size = morph([[0, P], [1.02, C], [3.05, T], [5.35, P]])
    const dark = shade([[0, 0], [3.05, 1], [5.35, 0]])
    const ptr = k.pointer(glide([[0, ...REST], [0.3, ...aim(k.at(pill), 14)], [1.55, ...aim(k.at(pick), 4, 5)], [3.15, ...REST]]), [0.95, 2.45])
    return {
      dur: 7.2,
      frame(t) {
        const press = 1 - 0.045 * bump(t - 0.95, 0.24)
        k.surface(...size(t), dark(t), press)
        show(pill, t, [[-1, 0.98], [5.5, 99]], { s: press })
        put(chips, { o: t > 1 && t < 3.3 ? 1 : 0 })
        show(cap, t, [[1.08, 2.9]])
        chip.forEach((c, i) => show(c, t, [[1.12 + i * 0.032, 2.86 + i * 0.012]], { s: c === pick ? 1 - 0.08 * bump(t - 2.45, 0.24) : 1 }))
        put(on, { o: ramp(t, 2.45, 0.14) })
        show(toast, t, [[3.15, 5.25]])
        ptr(t)
      },
    }
  },

  // 가입 없이: 링크 복사 → 복사 알림 → 입장 요청 카드 → 수락 → 들어옴 알림 → 알약
  join(k) {
    const pill = k.q('pill'), copied = k.q('copied'), knock = k.q('knock'), joined = k.q('joined')
    const av = k.q('av'), kt = k.q('kt'), kb = k.q('kb'), admit = k.q('admit')
    const size = morph([[0, k.size('pill')], [1.0, k.size('copied')], [2.55, k.size('knock')], [4.3, k.size('joined')], [6.0, k.size('pill')]])
    const dark = shade([[0, 0], [1.0, 1], [2.55, 0], [4.3, 1], [6.0, 0]])
    const ptr = k.pointer(glide([[0, ...REST], [0.3, ...aim(k.at(pill), 14)], [1.35, 128, 74], [2.95, ...aim(k.at(admit), 4, 5)], [4.5, ...REST]]), [0.92, 3.75])
    return {
      dur: 7.6,
      frame(t) {
        const press = 1 - 0.045 * bump(t - 0.92, 0.24)
        k.surface(...size(t), dark(t), press)
        show(pill, t, [[-1, 0.95], [6.12, 99]], { s: press })
        show(copied, t, [[1.12, 2.45]])
        put(knock, { o: t > 2.6 && t < 4.4 ? 1 : 0 })
        show(av, t, [[2.66, 4.16]], { s: 0.6 + 0.4 * spring(t - 2.66, 16, 0.55) })
        show(kt, t, [[2.74, 4.16]])
        show(kb, t, [[2.84, 4.16]])
        put(admit, { s: 1 - 0.08 * bump(t - 3.75, 0.24) })
        show(joined, t, [[4.42, 5.9]])
        ptr(t)
      },
    }
  },

  // 발음 그대로: 말하는 마이크 알약 → 자막 카드 → 알약. 중간 글자가 써지는 동안 번역도 흐릿하게 함께 나오고,
  // 확정되는 순간 둘 다 진해진다(말하는 동안 번역이 나온다).
  accent(k) {
    const mic = k.q('mic'), card = k.q('card'), interim = k.q('interim'), final = k.q('final'), tr = k.q('tr')
    const bars = k.qa('bar'), head = k.qa('hb')
    // 번역 줄이 나올 때 카드가 아래로 자란다: 그 전에는 번역 줄만큼 짧고 위로 올라가 있다.
    const M = k.size('mic'), C = k.size('card'), gap = tr.offsetHeight + 6
    const size = morph([[0, M], [1.35, [C[0], C[1] - gap]], [2.0, C], [6.45, M]])
    const oy = track([[0, 0], [1.35, -gap / 2], [2.0, 0]])
    const type = k.typer(interim)
    const chars = [...tr.querySelectorAll<HTMLElement>('.mo-ch')]
    return {
      dur: 8,
      frame(t) {
        const loud = ramp(t, 0.35, 0.3) * (1 - ramp(t, 3.2, 0.35))
        k.surface(...size(t), 0, 1, oy(t))
        show(mic, t, [[-1, 1.3], [6.6, 99]])
        bars.forEach((b, i) => put(b, { sy: 0.18 + 0.82 * loud * wave(t, i) }))
        show(card, t, [[1.45, 6.3]])
        head.forEach((b, i) => put(b, { sy: 0.35 + 0.65 * loud * wave(t, i + 3) }))
        show(interim, t, [[1.5, 3.2]], {}, 0.05)
        type(clamp((t - 1.6) / 1.4))
        show(final, t, [[3.25, 99]])
        show(tr, t, [[2.05, 99]], { o: 0.55 + 0.45 * ramp(t, 3.25, 0.3) }, 0.25)
        // 번역 글자가 중간 글자(1.6~3.0초)를 조금 늦게 따라가며 차례로 나타난다
        const p = clamp((t - 2.05) / 1.35) * chars.length
        chars.forEach((c, i) => put(c, { o: clamp(p - i) }))
      },
    }
  },

  // .txt로 저장: 대화 기록 카드 → 저장 누르기 → 파일 → 저장 알림 → 대화 기록
  txt(k) {
    const list = k.q('list'), save = k.q('save'), file = k.q('file'), saved = k.q('saved'), doc = k.q('doc')
    const lines = k.qa('line')
    const L = k.size('list')
    const size = morph([[0, L], [2.3, k.size('file')], [3.95, k.size('saved')], [5.6, L]])
    const dark = shade([[0, 0], [3.95, 1], [5.6, 0]])
    const ptr = k.pointer(glide([[0, ...REST], [0.5, ...aim(k.at(save), 12)], [2.55, ...REST]]), [1.95])
    return {
      dur: 7.6,
      frame(t) {
        const press = 1 - 0.03 * bump(t - 1.95, 0.24)
        k.surface(...size(t), dark(t), press)
        put(list, { o: t < 2.5 || t > 5.6 ? 1 : 0, s: press })
        lines.forEach((l, i) => show(l, t, [[-1, 2.12 + i * 0.04], [5.72 + i * 0.06, 99]]))
        show(save, t, [[-1, 2.2], [5.98, 99]], { s: 1 - 0.08 * bump(t - 1.95, 0.24) })
        show(file, t, [[2.45, 3.85]])
        put(doc, { s: 0.6 + 0.4 * spring(t - 2.5, 16, 0.55) })
        show(saved, t, [[4.08, 5.5]])
        ptr(t)
      },
    }
  },

  // 화면 공유: 알약 → 발표 화면 → 모서리 위로 자막 창이 미끄러져 와 새 줄로 바뀜 → 알약
  share(k) {
    const pill = k.q('pill'), slide = k.q('slide'), win = k.q('win')
    const parts = k.qa('sh'), lines = k.qa('wl')
    const P = k.size('pill')
    const size = morph([[0, P], [1.0, k.size('slide')], [6.35, P]])
    const ptr = k.pointer(glide([[0, ...REST], [0.3, ...aim(k.at(pill), 14)], [1.4, -150, 100], [6.4, ...REST]]), [0.92])
    return {
      dur: 8.2,
      frame(t) {
        const press = 1 - 0.045 * bump(t - 0.92, 0.24)
        k.surface(...size(t), 0, press)
        show(pill, t, [[-1, 0.95], [6.5, 99]], { s: press })
        put(slide, { o: t > 1 && t < 6.4 ? 1 : 0 })
        parts.forEach((p, i) => show(p, t, [[1.12 + i * 0.07, 6.2 - i * 0.03]]))
        const x = 70 * (1 - spring(t - 2.1, 15, 0.78)) + 70 * spring(t - 5.85, 18, 1)
        show(win, t, [[2.1, 5.95]], { x })
        show(lines[0], t, [[2.3, 4.25]])
        show(lines[1], t, [[4.4, 99]])
        ptr(t)
      },
    }
  },

  // 오픈 소스: 알약 → 코드 카드(줄마다 써짐) → AGPL-3.0 배지 → GitHub 알약 누르기 → 알약
  oss(k) {
    const pill = k.q('pill'), code = k.q('code'), gh = k.q('gh'), badge = k.q('badge')
    const lines = k.qa('ln'), typers = lines.map(k.typer)
    const P = k.size('pill')
    const size = morph([[0, P], [1.0, k.size('code')], [4.5, k.size('gh')], [6.35, P]])
    const dark = shade([[0, 0], [1.0, 1], [6.35, 0]])
    const ptr = k.pointer(glide([[0, ...REST], [0.3, ...aim(k.at(pill), 14)], [1.4, ...REST], [4.85, ...aim(k.at(gh), 20)], [6.0, ...REST]]), [0.92, 5.5])
    return {
      dur: 7.6,
      frame(t) {
        const press = 1 - 0.045 * (bump(t - 0.92, 0.24) + bump(t - 5.5, 0.24))
        k.surface(...size(t), dark(t), press)
        show(pill, t, [[-1, 0.95], [6.48, 99]], { s: press })
        show(code, t, [[1.1, 4.35]])
        typers.forEach((type, i) => type(clamp((t - 1.3 - i * 0.42) / 0.4)))
        show(badge, t, [[3.0, 4.2]], { s: 0.4 + 0.6 * spring(t - 3.0, 17, 0.5) })
        show(gh, t, [[4.62, 6.25]], { s: press })
        ptr(t)
      },
    }
  },
}

/** 대화 상자에 복제한 무대(.mo-live)에 판과 커서를 붙이고, 층의 크기·위치를 재서 장면을 만든다. */
export function build(stage: HTMLElement): Scene {
  const surf = document.createElement('div')
  surf.className = 'mo-surf'
  stage.prepend(surf)
  const light = plate(surf, 'mo-pl', 22)
  const dim = plate(surf, 'mo-pl mo-pd', 22)

  const cur = document.createElement('div')
  cur.className = 'mo-cur'
  cur.innerHTML = '<i class="mo-ring"></i><svg viewBox="0 0 20 22" width="20" height="22"><path d="M3 2v15l4.2-3.8 2.8 6.2 2.6-1.2-2.7-6h5.7z" /></svg>'
  stage.append(cur)
  const ring = cur.firstElementChild as HTMLElement

  const box = stage.getBoundingClientRect()
  const ux = box.width / 384
  const uy = box.height / 240
  const all = (name: string) => [...stage.querySelectorAll<HTMLElement>(`[data-l="${name}"],[data-a="${name}"]`)]
  const kit: Kit = {
    q: (name) => all(name)[0],
    qa: all,
    size: (name) => {
      const el = all(name)[0]
      return [el.offsetWidth, el.offsetHeight]
    },
    at: (el) => {
      const r = el.getBoundingClientRect()
      return [(r.left + r.width / 2 - box.left - box.width / 2) / ux, (r.top + r.height / 2 - box.top - box.height / 2) / uy]
    },
    surface(w, h, d = 0, k = 1, oy = 0) {
      light.draw(-w / 2, oy - h / 2, w, h)
      dim.draw(-w / 2, oy - h / 2, w, h)
      put(dim.box, { o: d })
      put(surf, { s: k })
    },
    pointer: (path, clicks) => (t) => {
      const [x, y] = path(t)
      let press = 0
      let r = -1
      for (const c of clicks) {
        press = Math.max(press, bump(t - c, 0.2))
        if (t >= c && t < c + 0.5) r = (t - c) / 0.5
      }
      put(cur, { x, y, s: 1 - 0.16 * press })
      put(ring, { s: 0.3 + 1.2 * (1 - (1 - Math.max(r, 0)) ** 3), o: r < 0 ? 0 : 0.5 * (1 - r) })
    },
    typer: (el) => {
      const cover = el.querySelector<HTMLElement>('.mo-cover')!
      const w = el.offsetWidth + 24
      return (p) => put(cover, { x: p * w })
    },
  }
  // 커서는 처음에 투명하다(CSS). 커서를 쓰지 않는 장면(발음)에서는 그대로 보이지 않는다.
  return (scenes[stage.dataset.scene ?? ''] ?? scenes.lang)(kit)
}
