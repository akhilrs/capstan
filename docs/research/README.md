# Research artefacts

The measurement scripts here (`*.mjs`, some `*.sh` that call them) are **historical artefacts** of the Node implementation
(profiling, benchmarks, the cutover baseline). They need Node to rerun, and the Node tree they measured is deleted, so they
are kept for the record and are not maintained.

Because they are `.mjs` files, they are excluded from the tracked-file guard of `scripts/check.sh` through
[`scripts/node-allowlist`](../../scripts/node-allowlist), which both the guard and this note use: a tracked path starting with
a line of that file is allowed to be Node or TypeScript.
