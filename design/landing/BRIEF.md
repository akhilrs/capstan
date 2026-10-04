# Capstan landing page — design brief

Scratch task from pm-1 (designer workflow live test). Directions only; no final build.

## Users
Developers who already run Claude Code or Codex in a terminal, often several sessions at once,
and have felt the pain of babysitting them. Comfortable with `curl | sh`, git worktrees and tmux-like
panes. Skeptical of AI hype; they read the install script before running it.

## Purpose
In ten seconds: "Capstan is a controller that turns a few AI coding agents into a team — you talk to
one PM agent, it runs the crew, and nothing reaches main unless you merge it." Then one action: copy
the curl one-liner. Secondary: GitHub / docs.

## Tone
Plain, exact, a little nautical (the capstan: one hand on the bars moves a heavy anchor). Confident
through specifics (commit ids, branches, ledger) rather than adjectives. No hype words.

## Reference products
- **Tailscale.com** — plain-spoken developer copy and warm, illustrated (not glossy) brand; take the
  hand-drawn warmth paired with dense technical honesty.
- **Linear.app (changelog/method pages)** — tight type scale and disciplined hierarchy; take the restraint.
- **Charm.sh (Bubble Tea / Gum)** — terminal UI shown as the hero itself; take "show the real TUI" for
  `cstan dash`.
- **Stripe Press** — editorial serif on paper with ink illustration; take the book-like page for the
  ink-logo direction.
- **Fly.io docs/landing** — install one-liner as the primary CTA with copy button; take the CTA shape.

## Constraints
- Platform: static single page, responsive 375 → 1440 px, light and dark (the logo ships light+dark SVGs).
- Brand: existing pen-and-ink logo (`docs/assets/capstan-logo-{light,dark}.svg`); ink #22303c, rope
  amber #a3672a (dark: parchment #e4ded0, amber #e0a45e). Amber is the only accent.
- No Capstan design system exists in Claude Design (proposed to PM, not created); tokens are declared
  per direction.
- Accessibility: WCAG AA contrast, visible focus, keyboard-reachable copy button, reduced motion.
- Content: real copy from README only (commands, roles, branch names). No invented testimonials or stats.
- Deadline: scratch test; stop after Directions.
