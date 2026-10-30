# plan-132 / health-design: agent health from live activity

Claude Design: https://claude.ai/artifact/EeRC818amzoCkyXEMPHjGR (Capstan design system, the Liquid Glass look of the Mac app design https://claude.ai/artifact/5oShuyLwYjRXojPY42K7z8, light and dark).
The user picked direction A on 2026-10-29. Artboard "A (safe)" is the design to build. Artboard "B (stretch)" is the direction that was not picked; it stays on the canvas for reference only.

Inputs: `AgentItem.activity` is `idle`, `working`, `blocked`, `done`, `unknown`, any other string (a newer host), or absent (an older host, or an agent never observed). `activitySince` is the ISO time the activity last changed, or absent.

## Screens and states

No new screen. Only screen 09 Agents changes, and only its inspector explanation:

- **Agents table**: unchanged. It shows the same health pill (word, SF Symbol, color) in the Health column. The health now comes from the rules below.
- **Agent inspector**: the explanation paragraph uses the new text for the health word. Under it, one line in 12 pt `label2` (the `basis` line, below) says what the health is based on.
- **Empty** (no agent selected), **loading**, **error**, **hover** and **focus**: unchanged from the approved screens 06 and 09. The basis line is plain text with no focus stop. VoiceOver reads it after the explanation.

## Rule order

The first rule that holds decides:

1. **Paused**: `paused`, or state `paused`.
2. **Lost**: state `lost`.
3. **Blocked**: state `blocked` (the agent told the controller).
4. **Stalled**: the project lists the agent under attention.
5. **Needs you**: a prompt from the agent waits in Waiting for you.
6. **Needs you**: activity `blocked`. It comes after rule 5, so a relayed prompt still shows Needs you with the prompt.
7. **Working**: activity `working`.
8. **Idle**: activity `idle`.
9. **Idle**: activity `done`.
10. **Fallback** (activity `unknown`, another string, or absent): today's task rule, marked as an estimate. Working if state is `active` and the agent has a task; otherwise Idle.

Rules 1 to 5 are today's order, unchanged. Rules 6 to 10 replace today's `active + task -> Working, else Idle`.

## Activity values

The rows apply only when rules 1 to 5 do not hold.

| activity | Health | Fallback it uses | Line under the explanation |
| --- | --- | --- | --- |
| `idle` | Idle | none | Host activity: idle[, for …] |
| `working` | Working | none | Host activity: working[, for …] |
| `blocked` | Needs you | none | Host activity: blocked[, for …] |
| `done` | Idle | none | Host activity: done[, for …] |
| `unknown` | Working or Idle | task rule (state `active` and a task -> Working, else Idle), an estimate | Estimate: the host reported activity unknown. |
| another string | Working or Idle | task rule, an estimate | Estimate: the host reported an activity this app does not know. |
| absent | Working or Idle | task rule, an estimate | Estimate: the host reported no activity. |

The raw string of an unrecognised activity is never shown. It is host text that may be long or untrusted, and the line names it only as "an activity this app does not know".

For rules 1 to 5 there is no line under the explanation. The explanation already names the field.

## How activitySince is shown

It is shown only in the inspector's line under the explanation, and only when the health came from rules 6 to 9. The table does not show it.

- Less than 1 minute: `for less than a minute`
- 1 to 59 minutes: `for 6 min`
- 1 to 24 hours: `for 2 h 5 min` (`for 3 h` when the minutes are 0)
- 24 hours or more: `since 27 Oct, 18:40` (the user's locale: abbreviated date and short time)
- Absent, unparseable, or in the future (clock skew): the clause is left out, e.g. `Host activity: working.`

The time is computed from the app's clock each time the inspector renders (on every poll). There is no ticking timer.

## Explanation texts

There is one text for each AgentHealth word. Each text names the field it is based on. None says the agent made progress.

- **Working**: "Working means the host sees the agent busy in its terminal (activity working). When the host gives no activity, or one this app does not know, Working is an estimate from state active and a task; it does not say the agent made progress."
- **Needs you**: "Needs you means a prompt from this agent waits for your answer in Waiting for you, or the host sees the agent stopped at a question in its terminal (activity blocked). If Waiting for you has no prompt from it, answer in the agent's terminal."
- **Stalled**: "Stalled means the controller flagged the agent for attention: it saw no progress for longer than its limit. The connection is fine. You can pause the agent while you look, or leave it to the supervisor."
- **Lost**: "Lost means the controller marked the agent lost (state lost): its pane is gone and the controller will not hear from it again."
- **Blocked**: "Blocked means the agent told the controller it cannot go on without something from outside (state blocked)."
- **Paused**: "Paused means the agent was paused (paused, or state paused). Resume it when you are ready."
- **Idle**: "Idle means the host sees the agent waiting at its prompt or finished with its turn (activity idle or done). When the host gives no activity, or one this app does not know, Idle is an estimate: the agent is not active or has no task."

These texts cover the fallback: an agent with a task whose activity is absent or not recognised shows Working, and the text and line say it is an estimate from state and task.

## Words, symbols and colors

None differ from today's AgentHealth. All seven words are kept: Working, Needs you, Stalled, Lost, Blocked, Paused, Idle. The SF Symbols are kept: `gearshape.fill`, `hand.raised.fill`, `hourglass`, `questionmark.circle.fill`, `xmark.octagon.fill`, `pause.circle.fill`, `moon.zzz.fill`. The `AgentTone` colors are kept except two: green Working; yellow Stalled; red Lost, Blocked; grey Idle. Needs you uses the accent and Paused grey. No new health word is added: activity `blocked` reuses Needs you, and `done` reuses Idle.

User decisions 2026-10-29 (relayed by the PM): Needs you uses the accent and Paused grey, as the approved mock draws them; activity blocked shows Needs you; no host-activity line when rules 1 to 5 decide; Need attention counts activity-blocked Needs you rows.

app-health changes only `Health.swift` (the rules, and a basis value for the line) and the AgentsScreen explanation (the texts and the line). No screen is added. `docs/design/mac-app.md` is not edited.

## Rule deviations

- **I1/I2 (icon style)**: the canvas draws the symbols as inline stroke SVGs that only approximate the SF Symbols. The build uses the real SF Symbols named above. Trade-off: SF Symbols cannot be embedded in an HTML mock.
- **A4 (not colour alone)**: passes. Every health has a word and a symbol. Estimates are told in words (the line), not by color.
- No other deviations. Type, spacing, radius and colors are those of the approved Mac app design.

## Design record changes

None. No DESIGN.md exists, and this package does not own one. The Capstan design system and `docs/design/mac-app.md` are unchanged.

## Open questions

1. The approved Mac app mock (screen 09) draws Needs you in accent and Paused in grey, but `AgentTone` in code uses yellow for both. This design keeps the code's colors, as the package asks. Should the mock or the code be aligned later?
2. When rules 1 to 5 decide (for example Stalled) and the host still reports activity `working`, should the inspector also show the host activity? This design shows no line in that case, to keep one source per health.
3. Should Need attention (the table filter) count activity-blocked Needs you rows? It counts by health word today, so they would be included.
