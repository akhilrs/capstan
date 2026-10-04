# Capstan

Capstan is an open-source controller for a small team of AI coding agents. This system is the look of its web pages: a dark control room drawn in ink, with one coil of rope-amber. It is built from the hand-drawn ink logo (`docs/assets/` in the repository) and the landing page in `site/`.

## Content fundamentals

- **Say what happens, with the real nouns.** Commit ids, branch names, `cstan` commands, roles (PM, Architect, Developer, Reviewer, Supervisor). "The controller checks the commit is on that branch", never "seamless verification".
- **Plain, exact, unhurried.** Active voice, sentence case, no hype words, no exclamation marks. Short sentences that a skeptical developer can check against the README.
- **Anything a person could type is mono.** Commands, paths, ids, branch names, flags.
- **Example data is labelled as an example.** The dashboard on the landing page says "An example run".
- **Errors direct.** "The browser blocked the clipboard. The command is selected: press Ctrl+C to copy it." No apologies, no vagueness.

## Visual foundations

**Colour.** Ink and paper, dark first. `bg` is the ground, `surface` the one raised step (terminal panels, the install bar, code). `fg` and `fg-muted` carry all text. `line` is a hairline for dividers only. `accent` (rope amber) is the only accent: the primary action, its outline, prompts (`$`) and active states. Spend it on fewer than five things a screen. `logo-ink` and `logo-rope` belong to the logo, not to UI. Every text pair meets WCAG AA in both themes; the light accent is deepened to `#8a521c` because the logo's `#a3672a` is 4.0:1 on light ground.

**Type.** Two families, IBM Plex Sans for prose and headings and IBM Plex Mono for anything typed. Weights 400 and 600, with 500 for small mono labels. One `display` headline per page. Uppercase is allowed only on table headers that imitate the terminal UI.

**Spacing.** A 4px scale, `space-1` (4px) to `space-9` (96px). Page gutters are `space-7` on desktop, `space-5` on tablet and `space-4` on phones. Sections are separated by `space-8` and a `line` rule, not by cards.

**Shape.** Nearly square: `radius-2` (4px) for buttons, panels and focus rings, `radius-1` for inline code, `radius-0` for rules and rows. No shadows, no gradients, no glass.

**Motion.** `duration-fast` (150ms) for hover and press, `duration-base` (250ms) for a state that changes in answer to something, both on the ease-out curve `cubic-bezier(0.2, 0.7, 0.2, 1)`. Motion only confirms or shows a change of state; under `prefers-reduced-motion` it is removed.

**Focus.** A 2px `focus` ring (ink, so it shows on amber) with a 2px offset on every control. Never removed.

**Layout.** A 12-column grid with asymmetric splits (5/7 hero, 4/8 sections). Content is left aligned. On phones the install bar comes before illustrations.

## Iconography

None. Capstan uses words and the logo. State is shown with a small ring (idle) or a filled amber dot (active) next to a text label, never colour alone. Do not add emoji or icon grids.

## Components

- **Install bar**: the one primary action. Command, Copy button, and a live status line with idle, copied and error states.
- **Agents table**: the `cstan dash` illustration inside a terminal panel; scrolls sideways in its own box on phones.
- **Step rows**: numbered rows for a real sequence only (the five steps of a task).
- **Buttons**: primary (amber) and quiet (outlined); hover, active, focus and disabled states.
- **Code block**: commands with an amber `$` prompt and muted comments.

The CSS is `components/bundle.css`; it reads only the tokens. The repository's `site/components.css` is the same file.
