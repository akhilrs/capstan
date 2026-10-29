# Design approval

UI work goes through a design that the user approves before anyone builds it. This page records how the prompts shape that flow. The commands and the gate come from the gate package (`"design": true` on a plan package, `[design] require_approval`, `cstan design approve|changes|show`); see `commands.md` and `configuration.md`.

## Reachability finding

The designer builds the design in Claude Design through the `Artifact` tool (action `quickstart`, intent `design`, the Design type under the chosen design system). `DesignSync` is read-only: it reads a design-system project and never makes a design. The starter `[roles.designer]` allows `Artifact` and `DesignSync`, so the tools are reachable from a role created by `cstan init`. Whether a session actually has them depends on the account and the host: the tool can be missing or denied. The designer finds out at the first `quickstart` call, not at configuration time, and that is why the prompt has a fallback. Reachability was not verified against a live Claude Design account when this page was written.

## The flow

1. The Architect puts UI or visual design in its own package: `"design": true`, the designer role, type `docs`, scope `design`, owning only `docs/design/<plan-id>-<package-id>*` (one exception: the design record, see Design record ownership). Implementation packages depend on it.
2. The designer creates the design in Claude Design and commits `docs/design/<plan-id>-<package-id>.md`: the artifact link, a short list of screens and states (empty, loading, error, hover, focus) and open questions. It reports that commit with the link and the screens in the report summary. It writes no implementation code.
3. The PM shows the user the link and the screens, and the design note's `## Rule deviations` and `## Design record changes` sections, and asks with an AskUserQuestion picker: Approve or Changes needed. The PM never approves on its own judgement, and a passed review is not an approval.
4. Approve is recorded with `cstan design approve <plan>/<pkg> --link <link> --asked-user "<what the user said>"`. Changes needed is recorded with `cstan design changes <plan>/<pkg> --asked-user "<the requested changes>"`; the PM asks for the changes in free text and sends them to the designer, who amends the commit and reports again.
5. The gate: an implementation package that depends on the design package is not assigned until the design is approved. A `design_approval_missing` refusal means ask the user; `--early` is not the answer.
6. The implementation developer builds exactly the approved design.

Changes needed is always the user's words: the PM records it only after asking the user. While a design is being revised, the PM tells the developers of already assigned packages that depend on it to stop building the affected parts until the revised design is approved.

## Built-in guidance

The built-in designer prompt ends with design guidance. The designer reads its topics with `cstan design guide <topic>`:

- `cstan design guide main`: precedence, the soft gate, the brief, directions, the design package, Build and Verify, the rule index and the catalog index.
- `cstan design guide rules`: the rule lines.
- `cstan design guide directions-product` and `cstan design guide directions-brand`: the direction catalog for products and for brands.
- `cstan design guide redesign`: work on existing screens (kept, moved, changed, removed).
- `cstan design guide proposal`: the template for a direction proposal.
- `cstan design guide record`: the `DESIGN.md` design record.

A project may add `docs/design/directions.md` and `docs/design/rules.md`; they extend or replace the built-in directions and rules by id. A reference collection ("like product X" files outside the repository) is read only when the PM names its path, and the designer records traits only, never values or tokens.

## The soft gate

A rule that fails is listed as a rule deviation with its id and the reason (brief, design system or an accepted trade-off) under `## Rule deviations` in the design note. A rule deviation never blocks the report, and it never blocks approval: the PM shows the list and the user decides.

## The design record

`DESIGN.md` at the repository root is the design record. It is the token source for Build: implementation packages read its tokens and build with them. Implementation packages never edit DESIGN.md; a developer who finds a problem in it reports the problem to the PM.

The Verify checklist of a `DESIGN.md` is text the designer runs through (Capstan does not run it):

1. token references resolve: every token a section names exists in the token section;
2. contrast pairs pass AA by a computed check, with the ratio recorded;
3. no unused tokens: every token is referenced at least once.

When the designer reports that the design record and the design system disagree, the PM asks the user which one wins and sends the answer to the designer.

## Design record ownership

This section applies only when a plan has a design package whose design needs the record. The design package that needs it owns the path `DESIGN.md` in addition to `docs/design/<plan-id>-<package-id>*`, and the design commit reported for approval holds the candidate `DESIGN.md` together with the design note.

- Per plan and per record path, exactly one design package of the plan owns the record path.
- Any other design package that needs record changes lists them under `## Design record changes` in its own design note and never edits the record. The owning package depends on those packages and applies their changes before it reports.
- No implementation package owns a record path. When an implementation developer reports a problem in the record, the PM does not record Changes needed on its own: it asks the user whether the design should change. If the user wants a change, the PM records their words with `cstan design changes <plan-id>/<package-id> --asked-user` on the design package that owns the record; otherwise it sends the problem to the architect to investigate or to plan a new design package.

A project can keep the record in other places: `design_record` takes one path per surface (up to 8), and `design_record_mode` is `maintain` (the owning design package keeps each path current), `read` (designers read the record and no package owns a path) or `off` (designers ignore it). The architect is told the paths and the mode when it starts, and the designer's prompt carries them; see [configuration](configuration.md#design).

Ownership is a prompt-level rule: the architect states it in the plan and the plan review reads the `owns` lists; the controller does not enforce it.

## Fallback

When `Artifact` is missing or denied, the designer makes a static HTML mock and screenshots under `docs/design/<plan-id>-<package-id>/`, reports that path as the link and says Claude Design was unavailable. The approval flow is unchanged.

## The built-in designer prompt

The designer's workflow, including this design-package stage, is built into Capstan: a Developer role named `designer` with no `prompt` or `prompt_file` uses it, and it updates with Capstan. A project that ran `cstan init` before this still has `prompt_file = "roles/designer.md"` and keeps the old file, which has no design-package stage. To adopt the built-in prompt, remove the `prompt_file` line from `[roles.designer]`. To keep local edits, leave it, or set `prompt` or `prompt_file` to override the built-in text.

## Remote decisions

`cstan decide design:<plan>/<package> ... approve` or `... changes` records the operator's design decision from outside the terminal. A remote decision is limited to what the operator may do: it is made with the operator key, approvals of commands and answers to prompts are forwarded to the PM as a message instead, and the `--nonce` makes a retry safe (a repeat returns the first answer and changes nothing). See [commands](commands.md).
