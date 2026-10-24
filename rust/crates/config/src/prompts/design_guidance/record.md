# DESIGN.md: the project's design record

DESIGN.md is the long-lived record of the approved design system, kept in the repository so that Build, other hosts and reviewers can read it without Claude Design access. It is the project's own record: never import another product's file.

## Reading
- Read DESIGN.md at the repository root first, before the design system and before the catalog. If it exists it is the authority for tokens and rules; the catalog then influences only layout, density and emphasis. If it is absent, say so in the brief.
- If it and the Claude Design system disagree, report the difference to the PM. Do not choose silently.

## Writing
- Write or update DESIGN.md only when a UI change needs it and the package owns the file. An implementation package never edits it; it reports problems to the PM.
- Commit it together with the design note, in the one commit you report, so the approved commit already holds the record. List the changes under `## Design record changes` in the note.
- Write each value once, in the token section. Everything else refers to tokens by name. A rule stated as absolute has no exception elsewhere in the file.

## Section template
```
# <Project or surface> design record
Status: approved <date> by design package <plan-id>/<package-id>. Source of values: <repo tokens | Claude Design system | measured from live UI | proposed>.

## Tokens
(required) The one place for every value: colour roles, type styles, radii, spacing steps, motion durations. A machine-readable syntax is optional.

## 1. Character
Three mood words, who it is for, the catalog direction it started from, one sentence on what it is not.

## 2. Colour roles
One line per role, by token name: where it is used and where it must not be. Contrast pairs that pass AA, by token names, with the computed ratio.

## 3. Type
Families and their jobs, the scale and its ratio, weights, case rules, fallbacks and substitutes.

## 4. Space and layout
Base unit and scale, grid, container widths, density per surface.

## 5. Depth and shape
The one depth strategy, the radius family, border rules.

## 6. Components
Per component: purpose, variants, states (hover, focus, active, disabled, loading, empty, error), token references, one usage note.

## 7. Imagery, icons and motion
Roles, style rules, durations and easing, reduced-motion behaviour.

## 8. Do and don't
Three to six of each, every one testable by looking.

## 9. Responsive and accessibility
Breakpoints, collapse behaviour, minimum target size, focus style, contrast target.

## 10. Known gaps and change log
What is unspecified or inferred, and a dated list of changes with the design package behind each.
```

## Verify checklist
1. Token references resolve: every token named in a section exists in the token section.
2. Contrast pairs pass AA by computed check, with the ratio recorded.
3. No unused tokens: every token is referenced at least once.
