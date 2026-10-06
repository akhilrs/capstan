import assert from "node:assert/strict";
import { spawnSync, spawn } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { after, test } from "node:test";
import { fileURLToPath } from "node:url";
import {
  NODE_DASH_HINT,
  candidatePaths,
  dashArgs,
  dashEnv,
  launchRust,
  resolveDash,
  safeToExecve,
  type LaunchDeps,
  type ResolveContext,
} from "../src/dash/launch.js";

const roots: string[] = [];
function tmp(): string {
  const dir = fs.realpathSync(
    fs.mkdtempSync(path.join(os.tmpdir(), "dash-launch-")),
  );
  roots.push(dir);
  return dir;
}
after(() => {
  for (const dir of roots) fs.rmSync(dir, { recursive: true, force: true });
});

/** A fake cstan-dash that records its arguments and the credential variables to <out>. */
function fake(file: string, out: string): string {
  fs.mkdirSync(path.dirname(file), { recursive: true });
  fs.writeFileSync(
    file,
    `#!/bin/sh
[ "$1" = --version ] && { echo "cstan-dash 0.0.0"; exit 0; }
{
  echo "args:$*"
  echo "credential:\${CSTAN_DASH_CREDENTIAL-unset}"
  echo "token:\${CAPSTAN_TOKEN-unset}"
  echo "socket:\${CAPSTAN_SOCKET-unset}"
  echo "keep:\${KEEP_ME-unset}"
} > '${out}'
exit 7
`,
    { mode: 0o755 },
  );
  return file;
}

interface Layout {
  readonly ctx: ResolveContext;
  readonly override: string;
  readonly beside: string;
  readonly repo: string;
  readonly installed: string;
  readonly onPath: string;
}

function layout(env: NodeJS.ProcessEnv = {}): Layout {
  const root = tmp();
  const out = path.join(root, "out");
  const seaDir = path.join(root, "sea-bin");
  fs.mkdirSync(seaDir, { recursive: true });
  const sea = path.join(seaDir, "cstan");
  fs.writeFileSync(sea, "");
  const home = path.join(root, "home");
  const pathDir = path.join(root, "pathdir");
  const moduleDir = path.join(root, "repo", "dist", "src", "dash");
  fs.mkdirSync(moduleDir, { recursive: true });
  const place = (file: string): string => file;
  const override = place(path.join(root, "override", "any-name"));
  const beside = path.join(seaDir, "cstan-dash");
  const repo = path.join(
    root,
    "repo",
    "dash",
    "target",
    "release",
    "cstan-dash",
  );
  const installed = path.join(
    home,
    ".local",
    "share",
    "capstan",
    "current",
    "bin",
    "cstan-dash",
  );
  const onPath = path.join(pathDir, "cstan-dash");
  void out;
  return {
    ctx: {
      env: {
        PATH: pathDir,
        CSTAN_DASH_BIN: override,
        ...env,
      },
      seaExecutable: sea,
      moduleDir,
      home,
    },
    override,
    beside,
    repo,
    installed,
    onPath,
  };
}

test("each location is picked in the stated order", () => {
  const l = layout();
  const out = path.join(tmp(), "o");
  assert.deepEqual(resolveDash(l.ctx), { kind: "none" });
  for (const file of [l.onPath, l.installed, l.repo, l.beside, l.override]) {
    fake(file, out);
    assert.deepEqual(resolveDash(l.ctx), { kind: "rust", bin: file });
  }
});

test("the repository build is only searched when running from dist/src", () => {
  const l = layout();
  fake(l.repo, "/dev/null");
  const elsewhere = {
    ...l.ctx,
    moduleDir: path.join(path.dirname(l.ctx.moduleDir), "other", "dash"),
  };
  assert.ok(!candidatePaths(elsewhere).includes(l.repo));
  assert.ok(candidatePaths(l.ctx).includes(l.repo));
});

test("a sea binary looks beside the realpath of the executable", () => {
  const l = layout();
  const real = path.join(tmp(), "real");
  fs.mkdirSync(real);
  fake(path.join(real, "cstan-dash"), "/dev/null");
  fs.writeFileSync(path.join(real, "cstan"), "");
  const link = path.join(path.dirname(l.beside), "linked-cstan");
  fs.symlinkSync(path.join(real, "cstan"), link);
  const resolved = resolveDash({ ...l.ctx, seaExecutable: link });
  assert.deepEqual(resolved, {
    kind: "rust",
    bin: path.join(real, "cstan-dash"),
  });
});

test("XDG_DATA_HOME moves the installed location", () => {
  const xdg = tmp();
  const l = layout({ XDG_DATA_HOME: xdg });
  const installed = path.join(xdg, "capstan", "current", "bin", "cstan-dash");
  fake(installed, "/dev/null");
  assert.deepEqual(resolveDash(l.ctx), { kind: "rust", bin: installed });
});

test("CSTAN_DASH=node ignores every binary; rust demands one", () => {
  const l = layout();
  fake(l.override, "/dev/null");
  assert.deepEqual(
    resolveDash({ ...l.ctx, env: { ...l.ctx.env, CSTAN_DASH: "node" } }),
    {
      kind: "node",
    },
  );
  const none = layout({ CSTAN_DASH: "rust" });
  assert.throws(() => resolveDash(none.ctx), /CSTAN_DASH=rust/);
  fake(none.onPath, "/dev/null");
  assert.equal(resolveDash(none.ctx).kind, "rust");
});

test("a relative or non-executable CSTAN_DASH_BIN is skipped", () => {
  const l = layout({ CSTAN_DASH_BIN: "relative/cstan-dash" });
  fake(l.onPath, "/dev/null");
  assert.deepEqual(resolveDash(l.ctx), { kind: "rust", bin: l.onPath });
  fs.mkdirSync(path.dirname(l.override), { recursive: true });
  fs.writeFileSync(l.override, "#!/bin/sh\n", { mode: 0o644 });
  const l2 = { ...l.ctx, env: { ...l.ctx.env, CSTAN_DASH_BIN: l.override } };
  assert.deepEqual(resolveDash(l2), { kind: "rust", bin: l.onPath });
});

const options = { intervalSeconds: 3, noColor: true, reducedMotion: true };
const runtime = {
  socketPath: "/tmp/s.sock",
  credential: "sekret",
  workerLimit: 4,
};

test("arguments are exact and optional ones are left out", () => {
  assert.deepEqual(dashArgs(options, runtime), [
    "--socket",
    "/tmp/s.sock",
    "--interval",
    "3",
    "--worker-limit",
    "4",
    "--no-color",
    "--reduced-motion",
  ]);
  assert.deepEqual(
    dashArgs(
      { intervalSeconds: 1, noColor: false, reducedMotion: false },
      { ...runtime, workerLimit: null },
    ),
    ["--socket", "/tmp/s.sock", "--interval", "1"],
  );
});

test("the environment drops the agent credentials and adds the operator one", () => {
  const env = dashEnv(
    { CAPSTAN_TOKEN: "t", CAPSTAN_SOCKET: "s", KEEP_ME: "yes", PATH: "/bin" },
    "sekret",
  );
  assert.deepEqual(env, {
    KEEP_ME: "yes",
    PATH: "/bin",
    CSTAN_DASH_CREDENTIAL: "sekret",
  });
});

const spawnOnly: LaunchDeps = {
  execve: undefined,
  spawnChild: spawn,
  onSignal: () => () => undefined,
};

test("spawn path: exact args, credential env, child exit code", async () => {
  const dir = tmp();
  const out = path.join(dir, "out");
  const bin = fake(path.join(dir, "cstan-dash"), out);
  const outcome = await launchRust(
    bin,
    options,
    runtime,
    {
      CAPSTAN_TOKEN: "t",
      CAPSTAN_SOCKET: "s",
      KEEP_ME: "yes",
      PATH: "/usr/bin:/bin",
    },
    spawnOnly,
  );
  assert.deepEqual(outcome, { kind: "exited", code: 7 });
  assert.equal(
    fs.readFileSync(out, "utf8"),
    [
      "args:--socket /tmp/s.sock --interval 3 --worker-limit 4 --no-color --reduced-motion",
      "credential:sekret",
      "token:unset",
      "socket:unset",
      "keep:yes",
      "",
    ].join("\n"),
  );
});

test("spawn path forwards SIGTERM and reports the signal as 128+n", async () => {
  const dir = tmp();
  const bin = path.join(dir, "cstan-dash");
  fs.writeFileSync(bin, "#!/bin/sh\nexec sleep 30\n", { mode: 0o755 });
  const handlers = new Map<string, () => void>();
  const pending = launchRust(
    bin,
    options,
    runtime,
    { PATH: "/usr/bin:/bin" },
    {
      ...spawnOnly,
      onSignal: (signal, handler) => {
        handlers.set(signal, handler);
        return () => void handlers.delete(signal);
      },
    },
  );
  await new Promise((resolve) => setTimeout(resolve, 200));
  handlers.get("SIGTERM")?.();
  assert.deepEqual(await pending, { kind: "exited", code: 143 });
  assert.equal(handlers.size, 0);
});

test("an unexecutable candidate is unusable, not an error", async () => {
  const dir = tmp();
  const noShebang = path.join(dir, "noshebang");
  fs.writeFileSync(noShebang, "echo hi\n", { mode: 0o755 });
  assert.deepEqual(
    await launchRust(noShebang, options, runtime, {}, spawnOnly),
    { kind: "unusable" },
  );
  const missing = path.join(dir, "missing");
  assert.deepEqual(await launchRust(missing, options, runtime, {}, spawnOnly), {
    kind: "unusable",
  });
  const noExec = path.join(dir, "noexec");
  fs.writeFileSync(noExec, "#!/bin/sh\n", { mode: 0o644 });
  assert.deepEqual(await launchRust(noExec, options, runtime, {}, spawnOnly), {
    kind: "unusable",
  });
});

test("execve is used when safe and replaced by spawn otherwise", async () => {
  const dir = tmp();
  const bin = fake(path.join(dir, "cstan-dash"), path.join(dir, "out"));
  const calls: string[][] = [];
  const execve: LaunchDeps["execve"] = (file, args, env) => {
    calls.push([file, ...args], Object.keys(env));
    throw Object.assign(new Error("boom"), { code: "ENOENT" });
  };
  const outcome = await launchRust(
    bin,
    options,
    runtime,
    { CAPSTAN_TOKEN: "t" },
    {
      ...spawnOnly,
      execve,
    },
  );
  assert.deepEqual(outcome, { kind: "unusable" });
  assert.deepEqual(calls[0], [
    bin,
    bin,
    "--socket",
    "/tmp/s.sock",
    "--interval",
    "3",
    "--worker-limit",
    "4",
    "--no-color",
    "--reduced-motion",
  ]);
  assert.deepEqual(calls[1], ["CSTAN_DASH_CREDENTIAL"]);

  // A script whose interpreter is missing must never reach execve (it would abort the process).
  const bad = path.join(dir, "bad");
  fs.writeFileSync(bad, "#!/no/such/interpreter\n", { mode: 0o755 });
  assert.equal(safeToExecve(bad), false);
  assert.equal(safeToExecve(bin), true);
  const elf = process.execPath;
  assert.equal(safeToExecve(elf), true);
});

test("a real execve replaces the process: args, credential, no agent env", () => {
  const dir = tmp();
  const out = path.join(dir, "out");
  const bin = fake(path.join(dir, "cstan-dash"), out);
  const launchModule = path.join(
    path.dirname(fileURLToPath(import.meta.url)),
    "..",
    "src",
    "dash",
    "launch.js",
  );
  const script = path.join(dir, "run.mjs");
  fs.writeFileSync(
    script,
    `import { launchRust, defaultLaunchDeps } from ${JSON.stringify(launchModule)};
await launchRust(${JSON.stringify(bin)}, ${JSON.stringify(options)}, ${JSON.stringify(runtime)}, process.env, defaultLaunchDeps());
console.log("still running");
`,
  );
  const result = spawnSync(process.execPath, [script], {
    env: {
      PATH: process.env.PATH ?? "",
      CAPSTAN_TOKEN: "t",
      CAPSTAN_SOCKET: "s",
      KEEP_ME: "yes",
    },
    encoding: "utf8",
  });
  assert.equal(result.status, 7);
  assert.ok(!result.stdout.includes("still running"));
  assert.match(
    fs.readFileSync(out, "utf8"),
    /credential:sekret\ntoken:unset\nsocket:unset\nkeep:yes\n/,
  );
});

test("the hint names the install routes", () => {
  assert.match(NODE_DASH_HINT, /install cstan-dash/);
  assert.match(NODE_DASH_HINT, /sh install\.sh/);
  assert.match(NODE_DASH_HINT, /npm run build:dash/);
  assert.ok(!NODE_DASH_HINT.includes("\n"));
});

test("a file with an ELF header that cannot run is skipped, not exec'd", () => {
  const l = layout();
  const out = path.join(tmp(), "o");
  for (const file of [l.override, l.beside]) {
    fs.mkdirSync(path.dirname(file), { recursive: true });
    fs.writeFileSync(file, Buffer.from("\x7fELF not really a program"), {
      mode: 0o755,
    });
  }
  assert.equal(safeToExecve(l.override), true);
  assert.deepEqual(resolveDash(l.ctx), { kind: "none" });
  fake(l.installed, out);
  assert.deepEqual(resolveDash(l.ctx), { kind: "rust", bin: l.installed });
});

test("a candidate that prints the wrong version text is unusable", () => {
  const l = layout();
  fs.mkdirSync(path.dirname(l.override), { recursive: true });
  fs.writeFileSync(l.override, "#!/bin/sh\necho something else\n", {
    mode: 0o755,
  });
  assert.deepEqual(resolveDash(l.ctx), { kind: "none" });
});

test("CSTAN_DASH=rust with an unusable candidate names the file", () => {
  const l = layout({ CSTAN_DASH: "rust" });
  fs.mkdirSync(path.dirname(l.override), { recursive: true });
  fs.writeFileSync(l.override, Buffer.from("\x7fELF broken"), { mode: 0o755 });
  assert.throws(
    () => resolveDash(l.ctx),
    (error: Error) => error.message.includes(l.override),
  );
});
