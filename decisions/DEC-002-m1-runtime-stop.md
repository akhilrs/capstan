# DEC-002: M1 runtime stop decision

## Status

**Stop before coordination-core implementation.**

## Evidence

- Herdr 0.9.0 client/server are endpoint-compatible.
- A dedicated pane `wG:p1S` started OMP 18.3.1 as agent `m1probe` with cwd `/home/akhil/Workspace/github.com/akhilrs/capstan` and argv `["omp"]`; Herdr reported `interactive_ready`.
- `herdr agent prompt` delivered `m1-probe-001`; the real OMP pane replied `ACK m1-probe-001`.
- Herdr's published API schema exposes terminal/pane/agent lifecycle and input surfaces. The exercised surface provides no controller-owned typed OMP command receipt, durable command record, replay identity, assignment capability, cgroup ownership, or filesystem authority revocation proof.
- A 30-second `herdr agent prompt` attempt for an interruption probe timed out without an observed running workload or durable receipt. It did not establish interruption, process-tree termination, reconnect, replacement, or authority revocation.

## Decision

Do not use terminal transcript scraping or Herdr prompt delivery as Capstan's runtime command authority. They can prove interactive delivery only, not PM-4's required durable and containment semantics.

The only permitted continuation is an explicit evaluation of a controller-owned OMP RPC/bridge that supplies: typed command/receipt IDs, durable append/replay/reconciliation, assignment-generation capability checks, controller-driven cancellation, container/cgroup lifecycle evidence, and revocation before replacement writes. If that bridge cannot supply every property, PM-4 remains stopped and M2–M4 must not start.

## Consequences

M1 acceptance criteria other than interactive readiness and correlated delivery remain unproven. No coordination core, terminal-scraping fallback, or second command authority is authorized by this result.
