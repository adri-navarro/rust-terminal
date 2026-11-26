# Utility tab icons

## Intent
The five utility buttons (Commands, Docker, Monitor, Usage, Settings) used text
labels, e.g. "Commands (3)" and "Docker (2)", taking roughly 550-650px of the tab
bar. Terminal tabs had too little room. Make the buttons compact and icon-only so
terminal tabs get the room back.

## Approaches considered
- A. Icons in the tab bar (chosen): frees most of the room with no viewport or
  layout changes. The buttons, hit zones and views stay as they are.
- B. A right-side icon column (rejected): costs terminal width and touches every view.
- C. A "Tools" dropdown (rejected): hides everything one click deeper.

## Design
- Each button is 36px wide, icon only (18px glyph). Inactive: y=6, h=32. Active:
  y=5, h=34. Gap 6px, right margin 10px. Order, accent colours, fills, stroke and
  the 2px bottom accent bar are unchanged.
- Icon colour replaces the old text colour: `fg` when active, otherwise the
  per-button tint (Usage accent, Monitor info, Docker info when containers run,
  Commands success when commands run, otherwise `fg_muted`; Settings `fg_muted`).
- The status dot is dropped; its meaning lives in the icon tint and the badge.
- Count badge for Commands (running commands) and Docker (running stacks): only
  when N > 0, a `success` pill on the top-right corner, number in `bg`, "9+" above 9.
- Hover tooltip, drawn last with the toast style: "<Name> · <shortcut>", placed
  below the tab bar, right-aligned to the icon and clamped inside the window.
  No animation; cleared on click and when the cursor leaves the window.
- Terminal tabs keep `max_tabs_right = leftmost icon x - 46` and gain the freed width.

## Icon source and licence
Lucide 1.80.0 (ISC), a 2.8KB subset TTF with five glyphs: square-terminal,
container, activity, gauge, settings. Embedded with `include_bytes!` from
`assets/icons/lucide-subset.ttf`; the licence is shipped as
`assets/icons/LICENSE-lucide.txt`. Codepoints live in `ui::font::icon`.

## Non-goals
- No icon column, no viewport change, no change to the utility views, status bar,
  title-bar inset logic or terminal tab drawing.
