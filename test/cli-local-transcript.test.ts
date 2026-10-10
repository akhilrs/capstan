import assert from "node:assert/strict";
import {
  cpSync,
  existsSync,
  readdirSync,
  readFileSync,
  realpathSync,
  symlinkSync,
  writeFileSync,
} from "node:fs";
import path from "node:path";
import { test } from "node:test";
import {
  LOCAL_TRANSCRIPT_DIRECTORY,
  exportLocalTranscripts,
} from "./cli-local-transcript-export.js";
import {
  TRANSCRIPT_DIRECTORY,
  exportTranscripts,
} from "./cli-transcript-export.js";
import { removeTempDir, tempDir } from "./tmp.js";
import { ROUTES } from "../src/daemon.js";

const REGENERATE =
  "run `npm run build && node dist/test/cli-local-transcript-export.js` and commit rust/crates/cstan/tests/local-transcripts";
const root = path.resolve(import.meta.dirname, "..", "..");

function committed(directory: string): Map<string, string> {
  return new Map(
    readdirSync(directory)
      .filter((name) => name.endsWith(".json"))
      .map((name) => [name, readFileSync(path.join(directory, name), "utf8")]),
  );
}

test("the committed local CLI transcripts equal a fresh export from the Node CLI", async () => {
  const fresh = await exportLocalTranscripts();
  const stale: string[] = [];
  for (const [name, text] of fresh) {
    const file = path.join(LOCAL_TRANSCRIPT_DIRECTORY, name);
    if (!existsSync(file) || readFileSync(file, "utf8") !== text)
      stale.push(name);
  }
  assert.deepEqual(stale, [], `stale or missing transcripts; ${REGENERATE}`);
  const extra = [...committed(LOCAL_TRANSCRIPT_DIRECTORY).keys()].filter(
    (name) => !fresh.has(name),
  );
  assert.deepEqual(extra, [], `unexpected transcripts; ${REGENERATE}`);
});

test("every ledger a local transcript copies is committed", () => {
  const missing: string[] = [];
  for (const [name, text] of committed(LOCAL_TRANSCRIPT_DIRECTORY)) {
    const layout = (
      JSON.parse(text) as { layout: { copy?: Record<string, string> } }
    ).layout;
    for (const source of Object.values(layout.copy ?? {}))
      if (!existsSync(path.join(LOCAL_TRANSCRIPT_DIRECTORY, source)))
        missing.push(`${name}: ${source}`);
  }
  assert.deepEqual(missing, []);
});

/** The ledgers the offline status and inspect transcripts read are at three or more different migration versions. */
test("the offline status transcripts read ledgers of at least three migration versions", () => {
  const sources = new Set<string>();
  for (const [name, text] of committed(LOCAL_TRANSCRIPT_DIRECTORY))
    if (name.startsWith("status-offline-")) {
      const copy = (JSON.parse(text) as { layout: { copy?: object } }).layout
        .copy;
      for (const source of Object.values(copy ?? {})) sources.add(source);
    }
  assert.ok(sources.size >= 3, `ledgers: ${[...sources].join(", ")}`);
});

/** Commands the CLI never sends to a daemon by name; a usage error is their transcript. */
const NOT_REACHABLE = new Set(["launch", "peek", "shutdown"]);

interface Recorded {
  readonly argv: readonly string[];
  readonly env: Readonly<Record<string, string>>;
  readonly node: {
    readonly stderr: string;
    readonly exit: number | null;
  } | null;
}

function commandOf(argv: readonly string[]): string | undefined {
  const [first] = argv;
  if (first === "pm")
    return argv.filter((arg) => arg !== "--json")[1] === "restart"
      ? "pm-restart"
      : undefined;
  return first;
}

function transcripts(): Recorded[] {
  return [TRANSCRIPT_DIRECTORY, LOCAL_TRANSCRIPT_DIRECTORY].flatMap(
    (directory) =>
      [...committed(directory).values()].map(
        (text) => JSON.parse(text) as Recorded,
      ),
  );
}

test("every daemon command has a transcript in the operator environment and, where its access allows, in the agent environment", () => {
  const seen = new Map<string, Set<"operator" | "agent">>();
  for (const recorded of transcripts()) {
    const command = commandOf(recorded.argv);
    if (command === undefined || recorded.node === null) continue;
    const environment = recorded.env.CAPSTAN_TOKEN ? "agent" : "operator";
    const set = seen.get(command) ?? new Set();
    set.add(environment);
    seen.set(command, set);
  }
  const missing: string[] = [];
  for (const [command, route] of Object.entries(ROUTES)) {
    const have = seen.get(command) ?? new Set();
    if (!have.has("operator")) missing.push(`${command}: operator`);
    if (route.access !== "operator" && !have.has("agent"))
      missing.push(`${command}: agent`);
  }
  assert.deepEqual(
    missing,
    [],
    "a ROUTES command without a transcript has no reference once Node is gone; add a case to test/cli-local-transcript-export.ts",
  );
});

test("the commands the CLI does not send to a daemon end in a usage error", () => {
  for (const command of NOT_REACHABLE) {
    assert.ok(Object.hasOwn(ROUTES, command), `${command} is not in ROUTES`);
    const usage = transcripts().filter(
      (recorded) =>
        commandOf(recorded.argv) === command &&
        !recorded.env.CAPSTAN_TOKEN &&
        recorded.node !== null,
    );
    assert.ok(usage.length > 0, `no operator transcript for ${command}`);
    for (const recorded of usage) {
      assert.equal(recorded.node?.exit, 2, command);
      assert.match(recorded.node?.stderr ?? "", /^usage: /, command);
    }
  }
});

/** The transcripts that hold the package version. */
const LOCAL_VERSION_NAMES = new Set([
  "version",
  "version-flag",
  "version-short",
]);
const CLI_VERSION_NAMES = new Set([
  "fallback-version",
  "fallback-version-word",
  "front-version",
  "front-version-in-agent-env",
]);

test("the version transcripts do not change when the package version does", async () => {
  const scratch = realpathSync(tempDir("capstan-freeze-corpus-version-"));
  try {
    // The CLI reads package.json two levels above dist/src/cli.js, so a copy of the build with its own package.json prints its own version.
    cpSync(path.join(root, "dist"), path.join(scratch, "dist"), {
      recursive: true,
    });
    symlinkSync(
      path.join(root, "node_modules"),
      path.join(scratch, "node_modules"),
    );
    const manifest = JSON.parse(
      readFileSync(path.join(root, "package.json"), "utf8"),
    ) as { version: string };
    const bumped = "98.76.54";
    assert.notEqual(manifest.version, bumped);
    writeFileSync(
      path.join(scratch, "package.json"),
      JSON.stringify({ ...manifest, version: bumped }),
    );
    const options = {
      cli: path.join(scratch, "dist", "src", "cli.js"),
      version: bumped,
    };
    const local = await exportLocalTranscripts({
      ...options,
      only: (name) => LOCAL_VERSION_NAMES.has(name),
    });
    const cli = await exportTranscripts({
      ...options,
      only: (name) => CLI_VERSION_NAMES.has(name),
    });
    assert.equal(local.size, LOCAL_VERSION_NAMES.size);
    assert.equal(cli.size, CLI_VERSION_NAMES.size);
    for (const [directory, fresh] of [
      [LOCAL_TRANSCRIPT_DIRECTORY, local],
      [TRANSCRIPT_DIRECTORY, cli],
    ] as const)
      for (const [name, text] of fresh) {
        assert.equal(
          text,
          readFileSync(path.join(directory, name), "utf8"),
          `${name} depends on the package version`,
        );
        assert.ok(text.includes("<VERSION>"), `${name} has no <VERSION>`);
        assert.ok(!text.includes(bumped), `${name} holds the version`);
      }
  } finally {
    removeTempDir(scratch);
  }
});
