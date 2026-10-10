# capstan-parity-overlay

Test support for the replay harnesses (launcher, cstan, config). Each harness replays the Node fixtures; where Rust
deliberately differs from Node, an **overlay** replaces the expected output of that one case and says why. The Node
fixtures are never edited, so Node's own staleness tests keep passing.

## Overlay file

One JSON file per overlay, in the harness's `tests/divergences` directory:

```json
{
  "fixture": "pm.json",
  "case": "launch-pm",
  "reason": "the watch pane runs the cstan the daemon was started as, not node and the CLI file",
  "expected": { "...": "the whole expected output of the case, in the shape the harness compares" }
}
```

- `fixture`: the fixture file, a path relative to the harness's fixtures directory (no `..`, not absolute).
- `case`: the case name in that fixture, as the harness names it.
- `expected`: replaces the case's **entire** expected output (not a patch), so any divergence can be expressed.
- `reason`: required and non-empty.
- Unknown fields are refused.

## Reader

```rust
let overlays = capstan_parity_overlay::load(&divergences_dir, &fixtures_dir, &cases_of)?;
let expected = overlays.expected("pm.json", "launch-pm"); // Option<&Value>
```

`cases_of` lists the case names of one fixture file; the harness supplies it because fixture formats differ. A missing
`divergences_dir` means no overlays. `load` fails with a message naming the overlay file when an overlay names a fixture
or case that does not exist, has no reason, or replaces a case another overlay already replaces.
