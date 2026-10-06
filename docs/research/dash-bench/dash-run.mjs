// Runs the dash UI against a socket without the CLI wrapper (no daemon start). usage: FAKE_SOCK=... FAKE_CRED=... node dash-run.mjs dash [--interval N] [--reduced-motion]
const flags = process.argv.slice(3);
const at = flags.indexOf("--interval");
const { runDash } = await import(new URL("../../../dist/src/dash/run.js", import.meta.url));
await runDash(
  { intervalSeconds: at >= 0 ? Number(flags[at + 1]) : 2, noColor: false, reducedMotion: flags.includes("--reduced-motion") },
  { socketPath: process.env.FAKE_SOCK, credential: process.env.FAKE_CRED ?? "x".repeat(40), workerLimit: 3 },
);
