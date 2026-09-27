# DEC-003: Accept Herdr caller risk for M2–M4

## Status

**Accepted 2026-09-25. M2–M4 may proceed under the operational assumption below.** This supersedes DEC-002's stop condition for the sole-authority gap; it does not erase the evidence or claim that the gap was technically fixed.

## Context

- An isolated Herdr 0.9.1 server accepted independent `agent.start` and `agent.prompt` calls. A separate OMP agent returned an exact external acknowledgement; a prior independent agent performed project actions outside controller dispatch.
- The graceful `/exit` → observe agent absent → close pane → restart with `resume_agents_on_restore=true` sequence prevented that closed pane from restoring in the tested case. It does not prevent a new independent caller.
- Herdr's same-user socket API does not enforce controller-exclusive assignment authority.

## Decision

The user accepts this as a residual operational risk: normal Capstan assignments are expected to use the Capstan workflow/controller, and independent Herdr callers are not expected to issue assignments during those runs. This is an operational assumption, **not** an enforced exclusivity guarantee. Do not claim that Herdr prevents independent clients from starting or prompting OMP agents.

Proceed with M2–M4 under that assumption. Keep the controller as the expected command authority in normal operation, and keep controller receipts/reconciliation as the semantic command record. No terminal transcript scraping, no second command authority, and no controller-owned OMP RPC runtime path are authorized by this decision.

## Consequences and revisit trigger

A same-user client can still start or prompt another OMP agent outside the controller's dispatch, journal, and recovery records. Such work could conflict with, duplicate, or supersede controller-managed work. This risk is accepted for the present operating model, not eliminated.

Stop and revisit this decision if independent Herdr activity is observed during a Capstan assignment, if the operating model adds concurrent Herdr clients, or if a future acceptance requirement again demands technically enforced sole authority. The tested close-before-restart sequence remains the recovery procedure for the closed pane; it is not a global caller fence.
