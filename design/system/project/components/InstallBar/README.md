# Install bar

The one primary action on a page: the install command, a Copy button and a live status line.

- Markup: `.cap-install` holding `code.cap-install__cmd` (with an `aria-hidden` `$ ` prompt in `.cap-install__prompt`) and a `button.cap-btn`. Put `p.cap-install__status` with `role="status"` and `aria-live="polite"` right after it, and point the button's `aria-describedby` at it.
- States are `data-state` on both the button and the status line: `idle` (label "Copy"), `copied` (label "Copied", ink fill, resets after 4s) and `error` (label "Copy again", outlined, the command is selected and the status says which keys copy it).
- The consumer provides the copy script: use `navigator.clipboard.writeText` in a secure context, fall back to `document.execCommand("copy")`, and on failure select the command text.
- Outline is `accent`, ground is `surface`, radius `radius-2`. Below 560px the button stacks under the command at full width.
- Use one per page. Do not put the install command in a plain code block elsewhere on the same screen.
