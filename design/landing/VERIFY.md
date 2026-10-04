# Capstan landing page: verification and self-critique

Build: `site/` (direction B, Control room). Design system: the Claude Design system "Capstan", source in `design/system/project/`.

Screenshots are kept locally in `design/landing/screens/` (git-ignored) and are not committed; the paths below refer to that local folder.

## Checks run (Playwright, Chromium, served with `python3 -m http.server`)

| Check | Result |
| --- | --- |
| Screenshots 375 / 768 / 1440, dark and light (`browser emulate_media`) | `screens/site/site-<width>-<theme>.jpg` |
| Horizontal overflow | none at any width or theme (scrollWidth equals the viewport) |
| WCAG AA text contrast, computed styles, every visible text element | 0 failures at 375, 768 and 1440 in both themes (77 to 78 elements each) |
| Non-text contrast (from tokens) | install bar outline 8.6:1 dark / 5.8:1 light; quiet button outline 6.5:1 / 5.3:1; focus ring 15.4:1 / 14.6:1 |
| Keyboard | Tab order: skip link, logo, How it works, Docs, GitHub, theme switch, dashboard scroll box, Copy, Read install.sh, Install reference, Releases, GitHub. Every stop has a 2px ink ring (`screens/site/focus-*.jpg`) |
| Copy success | clipboard holds the exact command; button "Copied", status announced (`state-copied-dark.jpg`) |
| Copy error | clipboard API and execCommand stubbed to fail; button "Copy again", command selected, status says which keys copy it (`state-error-dark.jpg`) |
| Theme switch | switches to light, label changes to "Switch to dark theme", choice survives a reload |
| Reduced motion | dashboard keeps its final state; transitions and the settle animation are off |
| Console and network | no errors, no failed requests |

## Anti-slop self-critique

Avoid:

- Purple-to-blue or default gradients: **pass**. There are no gradients.
- Glassmorphism: **pass**. No blur and no translucent surfaces.
- Emoji or stock-icon grids: **pass**. No icons; state is a ring or dot with a text label.
- Three identical feature cards: **pass**. No cards; the sequence is numbered rows.
- Centred hero with vague copy: **pass**. The hero is left aligned on a 5/7 split, and the copy says what happens (one PM, real sessions, which branch to merge).
- Everything rounded and soft-shadowed: **pass**. Radius tops out at 4px, and there are no shadows.
- Lorem ipsum or placeholder content: **pass**. All copy comes from the README; the dashboard is captioned as an example run.
- Generic Inter on white: **pass**. IBM Plex Sans and Mono, dark first.

Require:

- Type scale with at most two typefaces: **pass**. IBM Plex Sans and IBM Plex Mono, with seven sizes as tokens.
- One spacing scale, no ad-hoc values: **pass, with listed constants**. Every margin, padding and gap uses `space-*`. The remaining fixed values are structural: 1px hairlines, the 2px focus ring and offset, the 1px press, a 1280px container, a 30em lead measure, a 600px table minimum, a 15rem step-title column and the letter-spacing values.
- Restrained palette, one accent: **pass**. Ink and paper plus rope-amber.
- Deliberate hierarchy and asymmetry: **pass**. The hero splits 5/7 with the headline first, the install bar runs full width, and the sections split 4/8.
- Real content: **pass**.
- Motion only where it carries meaning: **pass**. Hover and press, copy feedback, and a single dashboard sequence (a report becomes verified, then review starts). All of it is off under reduced motion.
- Empty, loading, error, hover and focus states: **hover, focus and error pass. Empty and loading do not apply**: the page has no async data or collections, and copying resolves within one frame.

Against frontend-design's list of defaults, this direction sits near two of them: near-black with one accent, and mono labels. The user picked it. To soften that, uppercase is kept only for the terminal table headers, middle-dot meta strings and the kicker label were removed, and numbering is used only for the real five-step sequence.

## References

- Tailscale, plain technical copy: used throughout.
- Linear, restraint and tight hierarchy: used.
- Charm.sh, show the terminal UI as the hero: used as an illustrated `cstan dash`, not a real capture.
- Fly.io, the one-liner as the primary CTA with a copy button: used.
- Stripe Press: belonged to direction A, so it is not used.

## Not verified

- How the Claude Design system page renders: its compiled `tokens.css`, the component previews and the cover were published but not rendered or screenshotted.
- Browsers other than Chromium (Safari, Firefox).
- A real clipboard denial (the error state was forced with stubs).
- Screen reader output (only the semantics were checked: roles, labels and the live status).
