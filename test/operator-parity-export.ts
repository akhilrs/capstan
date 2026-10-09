/**
 * Writes the fixtures capstan-operator (rust/crates/operator) is tested against: what the Node command policy
 * (src/operator-policy.ts) decides over a corpus of commands (normalise, tokenise, classify, hash, auto-approve rules,
 * session grants, the deny list) and what the restart coordinator (src/restart.ts) decides and words (busy snapshot,
 * refusals, run reports, notices).
 *
 * Run `npm run build && node dist/test/operator-parity-export.js` after an intended change and commit the result;
 * operator-parity.test.ts fails while the committed files differ from a fresh export.
 */
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import {
  OPERATOR_ALWAYS_APPROVAL,
  OPERATOR_AUTO_ALLOWLIST,
  OPERATOR_DANGEROUS_SHORT_LETTERS,
  autoApproveRuleProblem,
  autoDecision,
  classifyCommand,
  commandHash,
  hashPrefix,
  matchSessionGrant,
  normalizeCommand,
  normalizeReason,
  prefixGrantProblem,
  sessionRule,
  sessionRuleGrantId,
  tokenize,
  type GrantKind,
  type OperatorProposalKind,
} from "../src/operator-policy.js";
import {
  busySnapshot,
  createRestartCoordinator,
  knownGoodBuild,
  restartNoticeToPm,
  restartRunReport,
} from "../src/restart.js";
import type { RestartResult } from "../src/restart-helper.js";
import type { OperatorProposalRecord } from "../src/controller/types.js";

const root = path.resolve(import.meta.dirname, "..", "..");
export const PARITY_DIRECTORY = path.join(
  root,
  "rust",
  "crates",
  "operator",
  "tests",
  "parity",
);

type Json = null | boolean | number | string | Json[] | { [key: string]: Json };

const COMMANDS: readonly string[] = [
  "ls",
  "ls -l",
  "ls -la",
  "ls -a -h",
  "ls src",
  "ls src/foo.ts",
  "ls ../x",
  "ls /",
  "ls ~",
  "ls $HOME",
  "ls *.ts",
  "ls -R",
  "ls -fd",
  "ls -Fd",
  "ls --all",
  "ls --output=x",
  "ls ; rm x",
  "ls | cat",
  "ls > x",
  "ls `x`",
  "ls\nls",
  "ls  -l",
  "ls\t-l",
  " ls",
  "ls ",
  "ls -",
  "ls --",
  "Ls",
  "LS",
  "pwd",
  "whoami",
  "date",
  "date -u",
  "uname",
  "uname -a",
  "df",
  "df -h",
  "cstan",
  "cstan ping",
  "cstan status",
  "cstan status x",
  "cstan op propose x y",
  "git",
  "git status",
  "git status -s",
  "git statusx",
  "git rev-parse HEAD",
  "git rev-parse --abbrev-ref HEAD",
  "git rev-parse --short HEAD",
  "git rev-parse main",
  "git ls-files",
  "git ls-files src",
  "git push",
  "git push origin main",
  "git -C x status",
  "git status --force",
  "git log --output=x",
  "git config user.name x",
  "FOO=1 ls",
  "FOO=1",
  "RM -rf x",
  "/bin/rm x",
  "rm",
  "rm -rf x",
  "sudo ls",
  "kill 1",
  "docker ps",
  "npm test",
  "node -e 1",
  "curl x",
  "bash -c ls",
  "echo hi",
  "echo",
  "tar -xf a",
  "ls -l --force-with-lease",
  "ls --force",
  "ls --Force",
  "ls -D",
  "ls -x",
  "ls -c",
  "ls -e",
  "ls -o",
  "ls -O",
  "ls -r",
  "ls -R",
  "ls /usr/bin/rm",
  "ls ./rm",
  "ls a=b",
  "ls a,b",
  "ls a%b",
  "ls a@b",
  "ls a+b",
  "ls a:b",
  "ls -a-b",
  "ls a-b",
  "ls -- x",
  "ls\r\nls",
  "ls\u000bx",
  "ls x",
  "ls x",
  "é",
  "ls é",
  "ls \u0000",
  "ls \u007f",
  "ls ‮",
  "ls ​",
  "",
  "   ",
  "\n",
  "\t",
  "a".repeat(8192),
  "a".repeat(8193),
  "ls ".repeat(2730) + "l",
  "git ".repeat(3) + "status",
  "CHECKOUT x",
  "Push",
  "ls Push",
  "ls PUSH",
];

const KINDS: readonly OperatorProposalKind[] = ["command", "restart"];

const RULE_SETS: readonly {
  readonly exact: readonly string[];
  readonly prefix: readonly string[];
}[] = [
  { exact: [], prefix: [] },
  { exact: ["ls -la"], prefix: [] },
  { exact: [], prefix: ["ls"] },
  { exact: ["ls -la", "pwd"], prefix: ["ls", "git rev-parse"] },
  { exact: ["git status"], prefix: ["git rev-parse"] },
  { exact: [], prefix: ["cstan status"] },
  {
    exact: ["rm x", "ls ;", "ls  -l", " ls", "FOO=1 ls"],
    prefix: ["rm", "git"],
  },
  {
    exact: ["git ls-files", "git rev-parse HEAD", "uname -a"],
    prefix: ["git ls-files"],
  },
  { exact: [], prefix: ["ls -l", "uname", "df -h"] },
  { exact: ["ls src"], prefix: ["ls src"] },
];

const GRANT_SETS: readonly {
  readonly kind: GrantKind;
  readonly text: string;
  readonly grantId: string;
}[][] = [
  [],
  [{ grantId: "g1", kind: "exact", text: "git status" }],
  [{ grantId: "g2", kind: "prefix", text: "git status" }],
  [{ grantId: "g3", kind: "prefix", text: "ls" }],
  [
    { grantId: "g4", kind: "exact", text: "ls -la" },
    { grantId: "g5", kind: "prefix", text: "ls -l" },
    { grantId: "g6", kind: "prefix", text: "echo" },
  ],
  [{ grantId: "g7", kind: "exact", text: "rm x" }],
  [{ grantId: "g8", kind: "prefix", text: "rm" }],
];

const PREFIXES: readonly string[] = [
  "git",
  "cstan",
  "git status",
  "cstan op",
  "ls",
  "rm",
  "ls -la",
  "ls  -l",
  " ls",
  "ls ",
  "",
  " ",
  "FOO=1 ls",
  "ls ;",
  "ls\nls",
  "é",
  "echo hi",
  "git push",
  "a".repeat(8193),
];

function textResult(result: ReturnType<typeof normalizeCommand>): Json {
  return result.ok ? { ok: true } : { ok: false, code: result.code };
}

function policyCases(): Json {
  const commands = COMMANDS.map((command): Json => {
    const classification = classifyCommand(command);
    const tokens = tokenize(command);
    const hashes: Record<string, string> = {};
    for (const kind of ["command", "restart"])
      for (const force of [false, true])
        hashes[`${kind}:${force}`] = commandHash({
          kind,
          command,
          forceRestart: force,
        });
    return {
      command,
      normalize: textResult(normalizeCommand(command)),
      reason: textResult(normalizeReason(command)),
      simple: classification.simple,
      tokens: [...classification.tokens],
      alwaysApproval: [...classification.alwaysApproval],
      words: [...tokens.words],
      letters: [...tokens.letters],
      ruleProblem: autoApproveRuleProblem(command),
      hashes,
      hash12: hashPrefix(hashes["command:false"]!),
    };
  });
  const auto = RULE_SETS.map((rules): Json => ({
    exact: [...rules.exact],
    prefix: [...rules.prefix],
    results: COMMANDS.flatMap((command) =>
      KINDS.map((kind): Json => {
        const verdict = autoDecision(kind, command, rules.exact, rules.prefix);
        return {
          kind,
          command,
          auto: verdict.auto,
          rule: verdict.rule ?? null,
          reason: verdict.reason,
        };
      }),
    ),
  }));
  const grants = GRANT_SETS.map((set): Json => ({
    grants: set.map((grant) => ({ ...grant })),
    results: COMMANDS.flatMap((command) =>
      KINDS.map((kind): Json => {
        const verdict = matchSessionGrant(kind, command, set);
        return verdict.matched
          ? { kind, command, matched: true, grantId: verdict.grantId }
          : { kind, command, matched: false, reason: verdict.reason };
      }),
    ),
  }));
  return {
    alwaysApproval: [...OPERATOR_ALWAYS_APPROVAL],
    dangerousLetters: [...OPERATOR_DANGEROUS_SHORT_LETTERS],
    allowlist: OPERATOR_AUTO_ALLOWLIST.map((entry) => ({
      command: entry.command,
      subcommand: entry.subcommand ?? null,
      options: [...entry.options],
      positionals: Array.isArray(entry.positionals)
        ? [...(entry.positionals as readonly string[])]
        : (entry.positionals as string),
    })),
    commands,
    auto,
    grants,
    prefixProblems: PREFIXES.map((prefix): Json => ({
      prefix,
      problem: prefixGrantProblem(prefix),
    })),
    sessionRules: ["g1", "", "x:y"].map((id): Json => ({
      grantId: id,
      rule: sessionRule(id),
      back: sessionRuleGrantId(sessionRule(id)) ?? null,
    })),
    otherRules: ["full-auto", "ls", "session:", null].map((rule): Json => ({
      rule,
      grantId: sessionRuleGrantId(rule) ?? null,
    })),
  };
}

const RESULTS: readonly RestartResult[] = [
  { outcome: "ok" },
  { outcome: "ok", pid: 4242 },
  { outcome: "rolled_back" },
  {
    outcome: "rolled_back",
    reason: "the controller exited right after it started (exit code 3)",
  },
  {
    outcome: "rolled_back",
    reason: "did not answer ping within 60 seconds",
    failedLogTail: "boom\nstack line",
    ledgerRestored: true,
    depsChanged: ["package-lock.json"],
    pid: 7,
  },
  {
    outcome: "rolled_back",
    reason: "x",
    failedLogTail: "",
    ledgerRestored: false,
    depsChanged: [],
  },
  { outcome: "rolled_back", reason: "has ``` fence ````` inside" },
  { outcome: "rolled_back", reason: "r".repeat(1600) },
  { outcome: "rolled_back", reason: "z", failedLogTail: "t".repeat(7000) },
  { outcome: "down" },
  {
    outcome: "down",
    reason: "the old controller (pid 1) did not exit",
    manualRecovery: "Run these\n1. a\n2. b",
    depsChanged: ["package.json", "package-lock.json"],
  },
  {
    outcome: "down",
    reason: "q",
    manualRecovery: "m".repeat(2000),
    ledgerRestored: true,
  },
  { outcome: "down", reason: "no manual recovery" },
];

const BUSY_CASES: readonly {
  readonly startedReviews: number;
  readonly nonTerminalIntegrations: number;
  readonly unackedDeliveries: number;
  readonly inFlight: number | null;
}[] = [
  {
    startedReviews: 0,
    nonTerminalIntegrations: 0,
    unackedDeliveries: 0,
    inFlight: null,
  },
  {
    startedReviews: 0,
    nonTerminalIntegrations: 0,
    unackedDeliveries: 0,
    inFlight: 0,
  },
  {
    startedReviews: 1,
    nonTerminalIntegrations: 0,
    unackedDeliveries: 0,
    inFlight: null,
  },
  {
    startedReviews: 0,
    nonTerminalIntegrations: 2,
    unackedDeliveries: 0,
    inFlight: null,
  },
  {
    startedReviews: 0,
    nonTerminalIntegrations: 0,
    unackedDeliveries: 3,
    inFlight: null,
  },
  {
    startedReviews: 0,
    nonTerminalIntegrations: 0,
    unackedDeliveries: 0,
    inFlight: 4,
  },
  {
    startedReviews: 1,
    nonTerminalIntegrations: 2,
    unackedDeliveries: 3,
    inFlight: 4,
  },
];

async function restartCases(): Promise<Json> {
  const busy = BUSY_CASES.map((entry): Json => {
    const { inFlight, ...indicators } = entry;
    return {
      ...entry,
      busy: [
        ...busySnapshot(
          { busyIndicators: () => indicators },
          inFlight === null
            ? undefined
            : { inFlightOperations: () => inFlight },
        ),
      ],
    };
  });
  const reports = RESULTS.map((result): Json => ({
    result: JSON.parse(JSON.stringify(result)) as Json,
    report: { ...restartRunReport(result) } as unknown as Json,
    notice: restartNoticeToPm("p-1", result),
  }));
  // The coordinator's refusals, from a real coordinator over a scratch state directory.
  const scratch = mkdtempSync(path.join(tmpdir(), "capstan-operator-parity-"));
  try {
    const stateDir = path.join(scratch, "state");
    mkdirSync(stateDir, { recursive: true });
    const notices: string[] = [];
    const make = (busyList: readonly string[]) =>
      createRestartCoordinator({
        stateDir,
        projectRoot: scratch,
        distDir: path.join(scratch, "cstan"),
        helperSource: path.join(scratch, "helper.mjs"),
        binary: true,
        node: "node",
        argv: [],
        socketPath: path.join(stateDir, "control.sock"),
        pidPath: path.join(stateDir, "daemon.pid"),
        logPath: path.join(stateDir, "daemon.log"),
        credentialFile: path.join(scratch, "operator.key"),
        healthTimeoutSeconds: 1,
        idleWaitSeconds: 0,
        busy: () => busyList,
        notifyPm: (body, actionNeeded) =>
          notices.push(`${actionNeeded ? "action" : "info"}: ${body}`),
        requestStop: () => undefined,
      });
    const proposal = {
      proposalId: "p-1",
      forceRestart: false,
    } as OperatorProposalRecord;
    const refusals: Json[] = [];
    const attempt = async (
      name: string,
      action: () => unknown,
    ): Promise<void> => {
      notices.length = 0;
      try {
        await action();
        refusals.push({ name, refused: false, notices: [...notices] });
      } catch (error) {
        const refusal = error as { code?: string; message: string };
        refusals.push({
          name,
          refused: true,
          code: refusal.code ?? null,
          message: refusal.message,
          notices: [...notices],
        });
      }
    };
    await attempt("preflight without known-good", () => make([]).preflight());
    await attempt("run without known-good", () => make([]).run(proposal));
    const binary = path.join(scratch, "cstan");
    writeFileSync(binary, "binary");
    knownGoodBuild(stateDir, { binary: true }).snapshot(binary, scratch);
    await attempt("preflight with known-good", () => make([]).preflight());
    await attempt("run busy", () =>
      make([
        "1 review(s) in state started",
        "2 integration(s) not yet settled",
      ]).run(proposal),
    );
    return { busy, reports, refusals };
  } finally {
    rmSync(scratch, { recursive: true, force: true });
  }
}

/** Every fixture file by name, as the text the exporter writes. */
export async function exportFixtures(): Promise<Map<string, string>> {
  const files = new Map<string, string>();
  files.set("policy.json", JSON.stringify(policyCases()) + "\n");
  files.set("restart.json", JSON.stringify(await restartCases()) + "\n");
  return files;
}

if (import.meta.filename === process.argv[1]) {
  mkdirSync(PARITY_DIRECTORY, { recursive: true });
  const files = await exportFixtures();
  for (const [name, content] of files)
    writeFileSync(path.join(PARITY_DIRECTORY, name), content);
  process.stdout.write(`wrote ${files.size} fixtures to ${PARITY_DIRECTORY}\n`);
}
