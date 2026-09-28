#!/usr/bin/env node
import { createHash } from "node:crypto";
import { chmodSync, lstatSync, mkdtempSync, mkdirSync, readdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { spawnSync } from "node:child_process";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const evidenceRoot = mkdtempSync(path.join(os.tmpdir(), "capstan-pm7-"));
chmodSync(evidenceRoot, 0o700);
const projectRoot = path.join(evidenceRoot, "project");
const logRoot = path.join(evidenceRoot, "logs");
const emptyTemplate = path.join(evidenceRoot, "empty-template");
mkdirSync(projectRoot, { mode: 0o700 });
mkdirSync(logRoot, { mode: 0o700 });
mkdirSync(emptyTemplate, { mode: 0o700 });
const manifestBytes = readFileSync(path.join(repoRoot, "fixtures/m0-fixtures.json"));
const manifest = JSON.parse(manifestBytes.toString("utf8"));
if (manifest.schema !== "capstan.m0-fixtures.v1") throw new Error("unexpected fixture manifest schema");
const task = manifest.tasks.find((entry) => entry.id === "jsonl-summary-feature");
if (!task) throw new Error("jsonl-summary-feature is absent from the authoritative fixture manifest");
const cstan = path.join(repoRoot, "dist/src/cli.js");
const env = { ...process.env, LC_ALL: "C", LANG: "C", TZ: "UTC" };
const gitEnv = {
  ...env,
  GIT_CONFIG_NOSYSTEM: "1",
  GIT_CONFIG_GLOBAL: "/dev/null",
  GIT_CONFIG_COUNT: "0",
  GIT_CONFIG_PARAMETERS: "",
};
let logIndex = 0;

function run(binary, args, { cwd, input, label, timeout = 3_700_000, childEnv = env } = {}) {
  const result = spawnSync(binary, args, { cwd, input, env: childEnv, encoding: null, timeout, maxBuffer: 32 * 1024 * 1024 });
  const record = {
    binary: path.basename(binary), args, status: result.status, signal: result.signal,
    error: result.error?.message, stdout: result.stdout?.toString("base64") ?? "",
    stderr: result.stderr?.toString("base64") ?? "",
  };
  writeFileSync(path.join(logRoot, `${String(logIndex++).padStart(3, "0")}-${label}.json`), JSON.stringify(record) + "\n", { mode: 0o600 });
  if (result.error) throw new Error(`${label} could not start (${result.error.code ?? "spawn error"})`);
  return result;
}
function gitAt(cwd, args, label, { allowFailure = false } = {}) {
  const result = run("git", ["-c", "core.fsmonitor=false", "-c", "core.hooksPath=/dev/null", ...args], {
    cwd, label, timeout: 120_000, childEnv: gitEnv,
  });
  if (!allowFailure && result.status !== 0) throw new Error(`${label} failed (details retained privately)`);
  return result;
}
function git(args, label) {
  return gitAt(projectRoot, args, label).stdout.toString("utf8").trim();
}
function findWorkspaceForCommit(directory, commitSha) {
  let entries;
  try {
    const stat = lstatSync(directory);
    if (!stat.isDirectory() || stat.isSymbolicLink()) return undefined;
    entries = readdirSync(directory, { withFileTypes: true });
  } catch {
    return undefined;
  }
  const gitEntry = entries.find((entry) => entry.name === ".git");
  if (gitEntry?.isDirectory()) {
    const probe = gitAt(directory, ["cat-file", "-e", `${commitSha}^{commit}`], "locate-accepted-commit", { allowFailure: true });
    if (probe.status === 0) return directory;
    return undefined;
  }
  for (const entry of entries) {
    if (!entry.isDirectory() || entry.isSymbolicLink() || entry.name === ".git") continue;
    const found = findWorkspaceForCommit(path.join(directory, entry.name), commitSha);
    if (found) return found;
  }
  return undefined;
}
function assert(condition, message) { if (!condition) throw new Error(message); }

try {
  assert(process.versions.node === "24.6.0", `Node 24.6.0 is required for fixture-compatible verification; found ${process.versions.node}`);
  assert(readFileSync(cstan, "utf8").startsWith("#!/usr/bin/env node"), "built cstan CLI missing; run npm run build first");
  const revisionInput = Buffer.concat(task.files
    .map((file) => ({ file, bytes: Buffer.from(file.base64, "base64") }))
    .sort((left, right) => Buffer.compare(Buffer.from(left.file.path), Buffer.from(right.file.path)))
    .flatMap(({ file, bytes }) => [
      Buffer.from(file.path, "ascii"), Buffer.from([0]), Buffer.from(file.mode, "ascii"),
      Buffer.from([0]), Buffer.from(String(bytes.length), "ascii"), Buffer.from([0]), bytes,
    ]));
  assert(createHash("sha256").update(revisionInput).digest("hex") === task.source_revision, "fixture files do not match the frozen source revision");
  for (const file of task.files) {
    const bytes = Buffer.from(file.base64, "base64");
    assert(bytes.toString("base64") === file.base64, `fixture base64 is noncanonical for ${file.path}`);
    const target = path.resolve(projectRoot, file.path);
    assert(target.startsWith(`${projectRoot}${path.sep}`), "fixture path escaped run root");
    mkdirSync(path.dirname(target), { recursive: true, mode: 0o700 });
    writeFileSync(target, bytes, { mode: Number.parseInt(file.mode, 8), flag: "wx" });
    chmodSync(target, Number.parseInt(file.mode, 8));
  }
  gitAt(projectRoot, ["init", "--template", emptyTemplate, "--quiet"], "git-init");
  git(["config", "user.name", "Capstan PM-7 reference runner"], "git-user-name");
  git(["config", "user.email", "pm7-reference-runner@invalid"], "git-user-email");
  git(["add", "--", ...task.files.map((file) => file.path)], "git-add-fixtures");
  git(["commit", "--quiet", "-m", "Frozen M0 fixture base"], "git-commit-base");
  const baseSha = git(["rev-parse", "HEAD"], "freeze-base");
  const brief = {
    schemaVersion: 1,
    taskId: task.id,
    objective: task.prompt,
    acceptanceCriteria: [
      "Implement all requirements in the frozen JSONL summary task prompt.",
      "Match every valid, malformed, and regression fixture case byte-for-byte in stdout/stderr and exit status.",
      "Keep changes within lib/summary.js and bin/summary.js and preserve the frozen fixture base.",
    ],
    limits: { maxSlices: 2, maxRunMs: 3_600_000, maxDispatches: 16 },
    slices: [
      {
        id: "summary-library", title: "Implement strict JSONL summary library",
        description: "Implement the fixture-defined strict JSONL parsing, validation, aggregation, ordering, and output behavior in lib/summary.js.",
        role: "Developer", dependsOn: [], writeScope: ["lib/summary.js"],
        acceptanceCriteria: ["Implement strict record validation and collision-safe exact-category aggregation."],
      },
      {
        id: "summary-command", title: "Implement summary CLI",
        description: "Implement the executable bin/summary.js command using the completed summary library; preserve stdin bytes and exact stdout/stderr/exit behavior.",
        role: "Developer", dependsOn: ["summary-library"], writeScope: ["bin/summary.js"],
        acceptanceCriteria: ["Implement all requirements in the frozen JSONL summary task prompt.", "Match every valid, malformed, and regression fixture case byte-for-byte in stdout/stderr and exit status.", "Keep changes within lib/summary.js and bin/summary.js and preserve the frozen fixture base.", "Run node bin/summary.js on stdin and emit the required exact result.", "Handle malformed input with no partial stdout and exact failure diagnostics."],
      },
    ],
  };
  const briefPath = path.join(evidenceRoot, "brief.json");
  writeFileSync(briefPath, JSON.stringify(brief, null, 2) + "\n", { mode: 0o600 });
  const initialized = run(process.execPath, [cstan, "init"], { cwd: projectRoot, label: "cstan-init" });
  assert(initialized.status === 0, "cstan init failed (details retained privately)");
  const workflow = run(process.execPath, [cstan, "run", "--brief", briefPath], { cwd: projectRoot, label: "cstan-run" });
  assert(workflow.status === 0, "cstan run did not accept the completed workflow (details retained privately)");
  let output;
  try { output = JSON.parse(workflow.stdout.toString("utf8")); }
  catch { throw new Error("cstan run produced no valid JSON result (details retained privately)"); }
  assert(output.state === "complete" && output.scheduler?.state === "complete", "workflow was not accepted as complete");
  assert(output.baseSha === baseSha, "cstan run base differs from the frozen fixture base");
  const statusResult = run(process.execPath, [cstan, "status", "--json"], { cwd: projectRoot, label: "read-accepted-status" });
  assert(statusResult.status === 0, "cstan status failed (details retained privately)");
  let status;
  try { status = JSON.parse(statusResult.stdout.toString("utf8")); }
  catch { throw new Error("cstan status produced no valid JSON (details retained privately)"); }
  assert(status.run?.state === "completed", "controller does not report a completed run");
  assert(Array.isArray(status.finalVerification) && status.finalVerification.length > 0, "controller has no accepted final-parent verification");
  assert(!status.findings?.some((finding) => finding.state !== "resolved"), "completed run retains a blocking finding");
  const sliceWorkIds = brief.slices.map((slice) => `wf-${createHash("sha256").update(`${task.id}:${slice.id}`).digest("hex").slice(0, 24)}`);
  assert(sliceWorkIds.every((workItemId) => status.work?.some((work) => work.workItemId === workItemId && work.state === "accepted")), "a reference-task slice is not accepted");
  assert(status.work?.every((work) => work.state === "accepted" || work.state === "canceled"), "controller retains unfinished work");
  const finalVerification = status.finalVerification.at(-1);
  const acceptedTip = finalVerification.commitSha;
  assert(/^(?:[a-f0-9]{40}|[a-f0-9]{64})$/.test(acceptedTip), "final verification has no full accepted commit SHA");
  assert(acceptedTip !== baseSha, "accepted composition has no implementation commit");
  const sourceWorkspace = findWorkspaceForCommit(path.join(projectRoot, ".capstan", "workspaces"), acceptedTip);
  assert(sourceWorkspace, "accepted composed commit is unavailable in a retained role workspace");
  const exerciseRoot = path.join(evidenceRoot, "exercise");
  gitAt(evidenceRoot, ["clone", "--quiet", "--no-checkout", "--no-local", "--template", emptyTemplate, "--", sourceWorkspace, exerciseRoot], "clone-accepted-tip");
  gitAt(exerciseRoot, ["checkout", "--quiet", "--detach", acceptedTip], "checkout-accepted-tip");
  assert(gitAt(exerciseRoot, ["rev-parse", "HEAD"], "verify-accepted-tip").stdout.toString("utf8").trim() === acceptedTip, "fresh test checkout is not the accepted composed SHA");
  const changedPaths = gitAt(exerciseRoot, ["diff", "--name-only", `${baseSha}..${acceptedTip}`], "check-composed-scope").stdout.toString("utf8").trim().split("\n").filter(Boolean).sort();
  assert(JSON.stringify(changedPaths) === JSON.stringify([...task.permitted_paths].sort()), "accepted composed change set differs from exact permitted paths");
  assert(gitAt(exerciseRoot, ["status", "--porcelain"], "check-worktree-clean").stdout.toString("utf8").trim() === "", "accepted test checkout contains uncommitted changes");
  assert(gitAt(exerciseRoot, ["rev-parse", `${baseSha}^{commit}`], "recheck-frozen-base").stdout.toString("utf8").trim() === baseSha, "frozen fixture base is unavailable");
  const cases = [
    { label: "valid", input: Buffer.from(task.valid_stdin), stdout: task.valid_stdout, stderr: task.valid_stderr, exit: task.valid_exit },
    { label: "malformed", input: Buffer.from(task.malformed_stdin), stdout: task.malformed_stdout, stderr: task.malformed_stderr, exit: task.malformed_exit },
    ...task.regression_cases.map((entry, index) => ({
      label: `regression-${String(index + 1).padStart(2, "0")}`,
      input: entry.stdin_base64 !== undefined ? Buffer.from(entry.stdin_base64, "base64") : Buffer.from(entry.stdin ?? "", "utf8"),
      stdout: entry.stdout, stderr: entry.stderr, exit: entry.exit, command: entry.command,
    })),
    ...(task.malformed_applicable ? task.malformed_regression_cases.map((entry, index) => ({
      label: `malformed-regression-${String(index + 1).padStart(2, "0")}`,
      input: entry.stdin_base64 !== undefined ? Buffer.from(entry.stdin_base64, "base64") : Buffer.from(entry.stdin ?? "", "utf8"),
      stdout: entry.stdout, stderr: entry.stderr, exit: entry.exit, command: entry.command,
    })) : []),
  ];
  for (const fixtureCase of cases) {
    assert(gitAt(exerciseRoot, ["rev-parse", "HEAD"], `before-${fixtureCase.label}`).stdout.toString("utf8").trim() === acceptedTip, "accepted tip changed before fixture execution");
    assert(gitAt(exerciseRoot, ["status", "--porcelain", "--untracked-files=all"], `clean-before-${fixtureCase.label}`).stdout.toString("utf8").trim() === "", "fixture checkout changed before case execution");
    const [binary, ...args] = fixtureCase.command ?? task.command;
    const result = run(binary, args, { cwd: exerciseRoot, input: fixtureCase.input, label: fixtureCase.label, timeout: 60_000 });
    assert(result.status === fixtureCase.exit && result.signal === null, `${fixtureCase.label}: unexpected exit status`);
    assert(result.stdout.equals(Buffer.from(fixtureCase.stdout, "utf8")), `${fixtureCase.label}: stdout differs from frozen fixture`);
    assert(result.stderr.equals(Buffer.from(fixtureCase.stderr, "utf8")), `${fixtureCase.label}: stderr differs from frozen fixture`);
    assert(gitAt(exerciseRoot, ["rev-parse", "HEAD"], `after-${fixtureCase.label}`).stdout.toString("utf8").trim() === acceptedTip, "accepted tip changed during fixture execution");
    assert(gitAt(exerciseRoot, ["status", "--porcelain", "--untracked-files=all"], `clean-after-${fixtureCase.label}`).stdout.toString("utf8").trim() === "", "fixture execution changed the accepted checkout");
  }
  assert(gitAt(exerciseRoot, ["rev-parse", "HEAD"], "final-accepted-tip-check").stdout.toString("utf8").trim() === acceptedTip, "accepted SHA changed during verification");
  assert(gitAt(exerciseRoot, ["rev-parse", `${baseSha}^{commit}`], "final-base-check").stdout.toString("utf8").trim() === baseSha, "frozen fixture base SHA changed");
  rmSync(evidenceRoot, { recursive: true, force: true });
  process.stdout.write(`PM-7 reference run passed: ${cases.length} fixture cases; base=${baseSha}; accepted=${acceptedTip}\n`);
} catch (error) {
  process.stderr.write(`PM-7 reference run failed; private evidence retained at ${evidenceRoot}\n${error instanceof Error ? error.message : "unknown failure"}\n`);
  process.exitCode = 1;
}
