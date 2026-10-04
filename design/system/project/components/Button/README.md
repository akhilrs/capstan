# Button

Two buttons, primary and quiet, and the focus ring that every control shares.

- `button.cap-btn` is primary: `accent` fill, `on-accent` text, mono 600 at `text-code`, at least 44px tall, `radius-2`. Hover goes to `accent-hover` over `duration-fast`; press moves it down 1px.
- `.cap-btn--quiet` is for secondary actions such as the theme switch: transparent with a `fg-muted` outline; hover fills with `surface`.
- `:disabled` drops to 45% opacity with a not-allowed cursor. Use a real `disabled` attribute.
- Focus: every control gets `outline: 2px solid var(--focus)` with a 2px offset on `:focus-visible`. `focus` is ink (`fg`), so the ring shows on the amber fill. Never remove it.
- Name a button by what it does ("Copy", "Copy again"), with no trailing arrows.
