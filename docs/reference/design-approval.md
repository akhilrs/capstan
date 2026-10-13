# Design approval

UI work goes through a design that the user approves before anyone builds it. This page records how the prompts shape that flow. The commands and the gate come from the gate package (`"design": true` on a plan package, `[design] require_approval`, `cstan design approve|changes|show`); see `commands.md` and `configuration.md`.

## Reachability finding

The designer builds the design in Claude Design through the `Artifact` tool (action `quickstart`, intent `design`, the Design type under the chosen design system). `DesignSync` is read-only: it reads a design-system project and never makes a design. The starter `[roles.designer]` allows `Artifact` and `DesignSync`, so the tools are reachable from a role created by `cstan init`. Whether a session actually has them depends on the account and the host: the tool can be missing or denied. The designer finds out at the first `quickstart` call, not at configuration time, and that is why the prompt has a fallback. Reachability was not verified against a live Claude Design account when this page was written.

## The flow

1. The Architect puts UI or visual design in its own package: `"design": true`, the designer role, type `docs`, scope `design`, owning only `docs/design/<plan-id>-<package-id>*`. Implementation packages depend on it.
2. The designer creates the design in Claude Design and commits `docs/design/<plan-id>-<package-id>.md`: the artifact link, a short list of screens and states (empty, loading, error, hover, focus) and open questions. It reports that commit with the link and the screens in the report summary. It writes no implementation code.
3. The PM shows the user the link and the screens and asks with an AskUserQuestion picker: Approve or Changes needed. The PM never approves on its own judgement, and a passed review is not an approval.
4. Approve is recorded with `cstan design approve <plan>/<pkg> --link <link> --asked-user "<what the user said>"`. Changes needed is recorded with `cstan design changes <plan>/<pkg> --asked-user "<the requested changes>"`; the PM asks for the changes in free text and sends them to the designer, who amends the commit and reports again.
5. The gate: an implementation package that depends on the design package is not assigned until the design is approved. A `design_approval_missing` refusal means ask the user; `--early` is not the answer.
6. The implementation developer builds exactly the approved design.

## Fallback

When `Artifact` is missing or denied, the designer makes a static HTML mock and screenshots under `docs/design/<plan-id>-<package-id>/`, reports that path as the link and says Claude Design was unavailable. The approval flow is unchanged.

## The built-in designer prompt

The designer's workflow, including this design-package stage, is built into Capstan: a Developer role named `designer` with no `prompt` or `prompt_file` uses it, and it updates with Capstan. A project that ran `cstan init` before this still has `prompt_file = "roles/designer.md"` and keeps the old file, which has no design-package stage. To adopt the built-in prompt, remove the `prompt_file` line from `[roles.designer]`. To keep local edits, leave it, or set `prompt` or `prompt_file` to override the built-in text.
