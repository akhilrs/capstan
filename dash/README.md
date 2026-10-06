# cstan-dash

The Rust build of the `cstan dash` operator dashboard (crate `cstan-dash`, a library plus the `cstan-dash` binary).

```
dash/src/model   status JSON -> DashModel, shared types, helpers, actions
dash/src/view    DashModel + ViewState -> Frame (styled lines), overlays
dash/src/app     terminal loop, polling, keys
dash/tests       parity tests against fixtures exported from the Node implementation
```

## Checks

`npm run check:dash` (also part of `npm run check`) runs `cargo fmt --check`, `cargo clippy --all-targets --locked -- -D warnings`
and `cargo test --locked` in this directory. It finds cargo on `PATH`, else in `$HOME/.cargo/bin`; export
`PATH="$HOME/.cargo/bin:$PATH"` first. Without cargo it fails; `CSTAN_SKIP_DASH_CHECK=1` turns that into a loud skip, for a
machine that cannot have rustup (the Rust dashboard is then not checked).

Build: `cargo build --release --locked` gives `target/release/cstan-dash`; `cstan-dash --version` prints `cstan-dash <version>`.

## Parity fixtures

`tests/parity/*.json` is what the Node implementation computes for the cases in `test/dash-golden-cases.ts`: the status, the
model, the helpers, and every frame and overlay. Regenerate after an intended change in Node:

```
npm run build && node dist/test/dash-parity-export.js
```

`test/dash-parity.test.ts` fails while the committed files differ from a fresh export. The Rust tests read them through
`tests/common`. Numbers are compared as `f64`; objects by key set; arrays in order.

## Shapes

Shared types derive `Clone, Debug, PartialEq, Serialize, Deserialize` with camelCase names. A field the Node model sets to
`null` is an `Option` that serialises as `null`; a field that is absent in Node (`undefined`) is an `Option` that is skipped.
