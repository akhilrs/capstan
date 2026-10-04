# Agents table

An illustration of `cstan dash`: a terminal panel with a header bar, a table of agents and a footer of counts.

- Markup: `.cap-term` > `.cap-term__bar`, `.cap-scroll` (with `tabindex="0"`, `role="region"` and an `aria-label`, so keyboard users can scroll it) > `table.cap-table`, then `.cap-term__foot`.
- State cells use `.cap-state`: a ring for idle states, `.is-active` for a filled amber dot and amber text. The text label always says the state; colour never carries it alone.
- `.is-muted` cells hold secondary facts. Agent names, branches and commit ids stay in `fg`.
- Label it as an example in a caption. Use real role names and the real branch pattern `capstan/<agent>-g<n>`.
- The table keeps a 600px minimum and scrolls sideways inside its panel on phones; the page never scrolls sideways.
- Changing a cell may play `.cap-changed` (an opacity settle over `duration-base`) to show what changed; it is removed under reduced motion.
