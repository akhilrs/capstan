# Divergences of the configuration replay

Overlays of `tests/config_parity.rs` (`capstan-parity-overlay`, schema in `rust/crates/parity-overlay/README.md`). The
fixtures are `config-edge-cases.json` and `config-corpus.json` (in `tests/parity`); `expected` has the shape of an entry of `config-expected.json`
(`{kind: "ok", sha256, bytes}` over the RoleConfig JSON with `$ROOT`, or `{kind: "error", message}`).

Three are the `[daemon]` table, retired at cutover: Rust is the only daemon (fixture `config-edge-cases.json`).

- `edge-daemon-node`: `implementation = "node"` is a configuration error that says the Node daemon was removed.
- `edge-daemon-rust`: loads, with a warning, and the RoleConfig JSON has no `daemon` member.
- `edge-daemon-empty`: loads, and the JSON has no `daemon` member.

The other `[daemon]` cases (bad value, wrong type, unknown key, not a table) give Node's text unchanged.

Two are the researcher role's permission mode (fixture `config-corpus.json`): Node refused every mode but `default`; Rust
accepts any, so both configs load.

- `captured-0513` (`researcher-mode-acceptEdits`): the researcher with `permission_mode = "acceptEdits"` loads.
- `captured-0514` (`researcher-mode-auto`): the researcher with `permission_mode = "auto"` loads.

## The PM prompt

The PM command reference (`src/prompts/pm_reference.txt`) is Rust-owned text: it has the `cstan cleanup` bullet that Node's
does not, and it changes with the product. `every_prompt_variant_equals_nodes_text` therefore does not compare a PM variant
(`golden-pm` and the `matrix-PM-*`, `matrix-pm-*`, `matrix-wait-*` and `matrix-summary-*` cases of
`tests/parity/prompts-expected.json`) with the frozen `bytes`, `sha256` or golden file; it checks that the PM prompt builds where
Node built it, names the agent and is under the size cap. The `bytes`, `sha256` and the golden file `prompts-off/pm.txt` of
those entries are no longer read. The developer, reviewer, architect, operator and supervisor prompts keep Node's bytes and
hashes.

The launcher's frozen sequences (`crates/launcher/tests/parity`) are handled by `normal` in
`crates/launcher/tests/common/mod.rs`; see `crates/launcher/tests/divergences/README.md`.

## Host-switchable roles

A role can move between hosts by changing only `host`: on a codex, omp or pi host `allow`, `deny` and `mcp` are ignored with a
warning (they resolve to `[]`) instead of refusing the config, and a role that writes no `permission_mode` takes `auto` there.
Seven corpus cases (fixture `config-corpus.json`) that Node refused now load:

- `captured-0202` (`codex-dev-allow-ignored`), `rust-pi-allow` (`pi-allow-ignored`): `allow` on a non-claude role.
- `captured-0203` (`omp-dev-deny-ignored`), `rust-pi-deny` (`pi-deny-ignored`): `deny` on a non-claude role.
- `captured-0533` (`omp-mcp-ignored`): `mcp` on an omp role.
- `captured-0204` (`codex-dev-default-mode-auto`), `captured-0230` (`codex-dev-defaults-model`): a codex role with no
  `permission_mode` resolves to `auto`.

`hooks = "inherit"` on a non-claude role only warns; its resolved value and the role hash are unchanged, so no overlay covers it.
