<!-- Generated from GATES.json by `claw8 gates`. Do not hand-edit — edit GATES.json instead. -->

# Gates

| Gate | Command |
|------|---------|
| m1-runtime-probe-syntax | `node --check scripts/m1-runtime-probe.mjs` |
| m1-controller-restart-probe-syntax | `node --check scripts/m1-controller-restart-probe.mjs` |
| m1-herdr-bridge-syntax | `node --check scripts/m1-herdr-bridge.mjs` |
| m1-herdr-bridge-dispatch-error-regression | `node scripts/m1-herdr-bridge.test.mjs` |
| m1-egress-helper-syntax | `node --check scripts/m1-egress-helper.mjs` |
| m1-egress-helper-boundary-regression | `node scripts/m1-egress-helper.test.mjs` |
| m1-herdr-qualification-syntax | `node --check scripts/m1-herdr-qualification.mjs` |
| m1-receipt-journal-regression | `node scripts/m1-receipt-journal.test.mjs` |
| m1-egress-tls-sni-regression | `node scripts/m1-egress-tls.test.mjs` |
| m1-receipt-frame-regression | `node scripts/m1-receipt-frame.test.mjs` |
| m1-egress-tls-syntax | `node --check scripts/m1-egress-tls.mjs` |
| m1-receipt-frame-syntax | `node --check scripts/m1-receipt-frame.mjs` |
| m1-native-helpers-syntax | `cc -std=c11 -O2 -Wall -Wextra -Werror -fsyntax-only scripts/m1-receipt-peer.c scripts/m1-egress-kill.c` |
| pm5-controller-quality-and-regression | `npm run check` |
| pm5-m1-adapter-integration | `node --test scripts/m1-controller-adapter.test.mjs` |
