# Design System

**References: Google Meet** (meeting screen layout, controls, colors) + a **simple caption panel** (white background, thin dividers, translation directly under the original).
No decoration: gradients, shimmer effects, per-speaker colors and the like are not used. **The font is Pretendard (JP build, which includes Korean, Japanese and Latin) throughout.** Tokens are in [tokens.css](tokens.css); see [preview.html](preview.html) for how it looks.

> **Next version:** the v2 direction (system font, glass surfaces, deep-orange buttons, recency-sized captions, account and billing screens) is mocked up in [v2/](v2/README.md). Screens move to it one at a time; until a screen is rebuilt, the rules below apply.

## Brand

- Name **tolkmeet** (always lowercase). Wordmark is Pretendard 800, letter-spacing -0.05em.
- Mark **"Pair"**: two offset rounded bars (original on top, translation below). Ink `--brand-ink` #16181D + accent `--brand-accent` #F2542D. Component: `web/src/components/BrandMark.tsx`; favicon: `web/public/favicon.svg`.
- UI color rules:
  - Primary buttons are ink (`--accent` = brand ink).
  - Red-orange `--brand-accent` is only 3.45:1 on white, so it is never used for text. It is only for non-text accents: switch on, focus bar, selected check, active toggles in the meeting room, and the speaking border.
  - Red-orange links and text buttons use `--accent-text` #C2381A (5.4:1).
  - End call / mic off uses crimson `--danger` #D7263D, distinct from the brand red-orange.
- **Inputs and selects**:
  - Borderless filled fields (`--field-bg`).
  - Hover makes the fill slightly darker.
  - Focus lifts the field to a white surface with one 1.5px ink border; no halo or double border.
  - The caret is red-orange.
  - The border is always held as 1.5px transparent, so the size doesn't shift between states.
- **Dropdown list**: a white card. The item under the pointer or keyboard gets the fill color. The selected item is ink-background, paper-color text, with a red-orange check. Exception: the display-language picker in the header is a button, so it stays an outlined pill.

## Screen structure

```
┌──────────────────────────────────────┬──────────────┐
│                                      │ Captions   ✕ │
│   ┌──────────┐  ┌──────────┐         │ My lang  ⇄ Their lang│
│   │  Partner │  │   Me     │         │──────────────│
│   └──────────┘  └──────────┘         │ Tanaka 15:21 │
│                                      │ 私たちの…    │
│      (screen share: share large,     │ Our team…    │
│       participants as small strip)   │──────────────│
│                                      │ Me 15:22     │
│                                      │ 어떤 분야…   │
│                                      │ どのような…  │
├──────────────────────────────────────┴──────────────┤
│ 15:23 | abc-defg-hij   🎤 📷 CC ⬆ ⋮ ☎︎      ⓘ 👥 文A  │
└─────────────────────────────────────────────────────┘
```

- Dark meeting area (`--stage-bg`) + a **white caption panel on the right** (`--panel-width`, radius 8px).
- On narrow screens (<760px): videos on top, caption panel underneath, control bar shows only the center buttons.

## Caption entry (core)

One utterance = one entry. Entries are separated by a 1px `--divider` line.

```
Speaker name  time                 ← 12px, name in --text / time in --text-tertiary
Original (speaker's language)      ← --caption-size, --text
▬ Translation (other language)     ← --caption-size, --text, 4px gap, small red-orange pill + indent
```

- If I speak Korean → Korean, then Japanese below. If the other person speaks Japanese → Japanese, then Korean below. **The original always comes first.**
- Original and translation use the same size and color (as in the reference photo). The translation line carries the brand touch: a short red-orange pill before it and a slight indent (`.tr-mark`), the lower bar of the "Pair" mark in miniature. The pill scales with the text (em/lh), so it looks the same in the caption panel, the live captions screen, in-person mode and the demos. While pending, the pill fades to 45%. In the caption panel the pill hangs in the left margin instead of indenting, so name, original and translation share one left edge.
- **Still being recognized/translated**: the whole entry turns `--text-secondary`, and the translation slot shows "번역 중…" ("Translating…"). Once finalized it returns to normal color. No other animation.
- New entries appear at the bottom; the panel auto-scrolls if the user is already at the bottom. If they scrolled up, it stops.
- Accessibility: the feed is `role="log"`. Only finalized entries are announced by screen readers; in-progress entries are `aria-hidden`.

## Language selection

Two selects at the top of the caption panel: **My language** ⇄ **Their language**. They map to the server model `speaks`/`reads`, and the other participant chooses the same way.

## Controls (same as Meet)

| Button | Default | State |
|---|---|---|
| Mic / camera | `--control-bg` circle | When off, `--danger` red + slashed icon |
| Captions (CC) | | When on, `--control-active-bg` red-orange |
| Screen share | | While sharing, red-orange |
| End call | `--danger` red pill | Always red |
| Right side (info · participants · captions panel) | No background | Icon is red-orange when its panel is open |

40px circles, icons are Material Symbols Rounded (inline SVG). Keyboard focus shows a 2px red-orange outline in the meeting room and an ink outline on light screens.

## Video tile

- `--tile-bg` background, radius 8px, name at bottom left.
- Camera off: colored circular avatar (initials).
- Speaking: 3px red-orange inset border.
- Mic off: small mic_off badge at top right.
- During screen share: shared screen is the main view, participants move to a small strip on the right.

## Face-to-face mode (phase 2)

Same caption entry, just larger (`--caption-size` 20px or more). The screen is split top/bottom with the top half rotated 180°, so the person across can read it.
