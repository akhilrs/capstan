import { createHash } from "node:crypto";
import {
  chmodSync,
  existsSync,
  lstatSync,
  mkdirSync,
  readFileSync,
  realpathSync,
  writeFileSync,
  unlinkSync,
} from "node:fs";
import os from "node:os";
import path from "node:path";
import { spawnSync } from "node:child_process";
import type { Role } from "../controller/types.js";

const HERDR_VERSION = "0.9.0";
const HERDR_SHA256 =
  "4fa1a01158dd8043da92d31b270780b0dcc10603038d9b61cac4d81ab63fb71f";
const OMP_VERSION = "18.3.1";
const NODE_VERSION = "v24.6.0";
const IMAGE =
  "ubuntu@sha256:008173c23f95b170204355c12626cb5a965d779a7e1283b09e9cffbb1bf33ca3";
const ROLE_NAMES = new Set<Role>(["PM", "Developer", "Verifier", "Supervisor"]);

export interface RoleControllerPaths {
  readonly journalPath: string;
  /** Private, controller-owned directory containing receipt.sock. */
  readonly receiptSocketPath: string;
  /** Writable bridge directory; manager binds it at /bridge. */
  readonly bridgeSocketPath: string;
  /** Verifier-writable output, or Supervisor read-only access to the evidence root. */
  readonly evidenceDirectory?: string;
}

export interface RoleRuntimeManagerOptions {
  /** Private persistent root for staged tools and egress-helper state. */
  readonly stateRoot: string;
  /** Exact predeclared provider DNS host; never inferred from arbitrary URLs. */
  readonly providerHost: string;
  readonly providerPort?: number;
  readonly model?: string;
  readonly tokenProvider?: string;
  /** Qualification scripts are operational dependencies, not substitutes. */
  readonly qualificationScriptsDir?: string;
  readonly herdrBinary?: string;
  readonly ompBinary?: string;
  readonly ompNativeAddon?: string;
  readonly nodeBinary?: string;
}

export interface RoleRuntimeProvisionRequest {
  readonly role: Role;
  readonly seatId: string;
  /** The one assigned workspace. Supervisor receives a read-only mount. */
  readonly workspace: string;
  readonly controllerPaths: RoleControllerPaths;
}

export interface RoleRuntimeSession {
  readonly role: Role;
  readonly seatId: string;
  readonly sessionId: string;
  readonly containerId: string;
  readonly containerName: string;
  readonly networkName: string;
  readonly workspace: string;
  readonly bridgeSocketPath: string;
  readonly receiptSocketPath: string;
  readonly journalPath: string;
  readonly expectedOmpHostPid: number;
  /** Pinned helper passed to the controller receipt peer-auth policy. */
  readonly helperPath: string;
  readonly profile: string;
  readonly configPath: string;
  readonly state: "ready";
}

export interface RoleRuntimeContainmentProof {
  readonly contained: true;
  readonly sessionId: string;
  readonly containerId: string;
  readonly containerAbsent: true;
  readonly cgroupEmpty: true;
  readonly egressPolicyRemoved: true;
  readonly networkRemoved: true;
  readonly workspaceMount: string;
  readonly forcedKill: boolean;
  readonly exitCode: number;
}

interface RuntimeBinaries {
  readonly herdr: string;
  readonly omp: string;
  readonly addon: string;
  readonly node: string;
  readonly git: string;
  readonly gitExecPath: string;
  readonly peer: string;
  readonly egressKiller: string;
  readonly egressHelper: string;
}

function run(
  binary: string,
  args: readonly string[],
  env?: NodeJS.ProcessEnv,
): string {
  const result = spawnSync(binary, [...args], {
    encoding: "utf8",
    maxBuffer: 16 * 1024 * 1024,
    timeout: 60_000,
    ...(env ? { env } : {}),
  });
  if (result.error || result.status !== 0)
    throw new Error(
      `${binary} ${args.join(" ")} failed (${result.status ?? result.error}): ${(result.stderr ?? "").trim()}`,
    );
  return (result.stdout ?? "").trim();
}

function executable(file: string, label: string): string {
  if (!existsSync(file)) throw new Error(`${label} binary missing: ${file}`);
  return realpathSync(file);
}
function executableOnPath(name: string, label: string): string {
  const candidate = process.env.PATH?.split(path.delimiter)
    .filter((directory) => path.isAbsolute(directory))
    .map((directory) => path.join(directory, name))
    .find((file) => {
      if (!existsSync(file)) return false;
      const resolved = realpathSync(file);
      const stat = lstatSync(resolved);
      return stat.isFile() && (stat.mode & 0o111) !== 0;
    });
  if (!candidate) throw new Error(`${label} binary missing from PATH`);
  return realpathSync(candidate);
}

function validatePath(
  value: string,
  name: string,
  kind: "file" | "directory",
): string {
  if (!path.isAbsolute(value))
    throw new TypeError(`${name} must be an absolute path`);
  const stat = lstatSync(value);
  if (
    stat.isSymbolicLink() ||
    (kind === "file" ? !stat.isFile() : !stat.isDirectory())
  )
    throw new Error(`${name} must be a regular ${kind}`);
  if (
    (process.getuid && stat.uid !== process.getuid()) ||
    (stat.mode & 0o077) !== 0
  )
    throw new Error(`${name} must be owned by the current user and private`);
  return path.resolve(value);
}
function pathsOverlap(first: string, second: string): boolean {
  const a = path.resolve(first);
  const b = path.resolve(second);
  return (
    a === b ||
    a.startsWith(`${b}${path.sep}`) ||
    b.startsWith(`${a}${path.sep}`)
  );
}

function safeId(value: string, name: string): string {
  if (!/^[A-Za-z0-9][A-Za-z0-9._:-]{0,127}$/.test(value))
    throw new TypeError(`invalid ${name}`);
  return value;
}

function sha256(file: string): string {
  return createHash("sha256").update(readFileSync(file)).digest("hex");
}

/** Provision the qualification-selected Herdr/OMP container path; no host execution fallback exists. */
export class RoleRuntimeManager {
  readonly #options: RoleRuntimeManagerOptions;
  readonly #docker: string;
  readonly #scripts: string;
  readonly #binaries: RuntimeBinaries;
  readonly #token: string;
  readonly #sessions = new Map<string, RoleRuntimeSession>();
  readonly #roleJournal = new Map<Role, string>();
  readonly #bridgeSockets = new Map<
    string,
    { readonly dev: number; readonly ino: number }
  >();
  #counter = 0;

  constructor(options: RoleRuntimeManagerOptions) {
    if (process.version !== NODE_VERSION)
      throw new Error(
        `Qualification host Node must be ${NODE_VERSION}; got ${process.version}`,
      );
    if (!/^[a-z0-9.-]+$/i.test(options.providerHost))
      throw new TypeError(
        "providerHost must be the exact predeclared provider hostname",
      );
    const providerPort = options.providerPort ?? 443;
    if (
      !Number.isInteger(providerPort) ||
      providerPort < 1 ||
      providerPort > 65535
    )
      throw new TypeError("providerPort must be an exact TCP port");
    this.#options = options;
    this.#docker = "docker";
    this.#scripts = path.resolve(
      options.qualificationScriptsDir ??
        path.join(import.meta.dirname, "../../../scripts"),
    );
    if (
      !existsSync(path.join(this.#scripts, "m1-herdr-bridge.mjs")) ||
      !existsSync(path.join(this.#scripts, "m1-egress-helper.mjs")) ||
      !existsSync(path.join(this.#scripts, "m1-egress-tls.mjs")) ||
      !existsSync(path.join(this.#scripts, "m1-receipt-peer.c")) ||
      !existsSync(path.join(this.#scripts, "m1-egress-kill.c"))
    )
      throw new Error(
        `Selected M1 qualification scripts unavailable in ${this.#scripts}`,
      );
    const omp = executable(
      options.ompBinary ?? run("sh", ["-lc", "command -v omp"]),
      "OMP",
    );
    const help = run(omp, ["--help"]);
    if (help.split("\n", 1)[0] !== `omp v${OMP_VERSION}`)
      throw new Error(`Expected OMP ${OMP_VERSION}`);
    const addon = executable(
      options.ompNativeAddon ??
        path.join(
          os.homedir(),
          ".omp",
          "natives",
          OMP_VERSION,
          "pi_natives.linux-x64-baseline.node",
        ),
      "OMP native addon",
    );
    const tokenProvider = options.tokenProvider ?? "openai-codex";
    const token = run(omp, ["token", tokenProvider, "--raw"]);
    if (!token) throw new Error(`No OMP token for ${tokenProvider}`);
    run(this.#docker, ["version", "--format", "{{.Server.Version}}"]);
    const image = run(this.#docker, [
      "image",
      "inspect",
      IMAGE,
      "--format",
      "{{json .RepoDigests}}",
    ]);
    if (!image.includes(IMAGE.split("@")[1]!))
      throw new Error(`Pinned image digest unavailable: ${IMAGE}`);
    const herdr = executable(
      options.herdrBinary ?? path.join(this.#scripts, "herdr-linux-x86_64"),
      "Herdr",
    );
    if (sha256(herdr) !== HERDR_SHA256)
      throw new Error("Herdr 0.9.0 SHA-256 mismatch");
    if (
      run(herdr, ["--version"], { ...process.env, HERDR_ENV: "1" }) !==
      `herdr ${HERDR_VERSION}`
    )
      throw new Error(`Expected Herdr ${HERDR_VERSION}`);
    const git = executableOnPath("git", "Git");
    const gitExecPath = realpathSync(run(git, ["--exec-path"]));
    if (!lstatSync(gitExecPath).isDirectory())
      throw new Error("Git exec path is not a directory");
    const node = executable(options.nodeBinary ?? process.execPath, "Node");
    if (run(node, ["--version"]) !== NODE_VERSION)
      throw new Error(`Worker Node must be ${NODE_VERSION}`);
    mkdirSync(options.stateRoot, { recursive: true, mode: 0o700 });
    const stateRoot = validatePath(options.stateRoot, "stateRoot", "directory");
    const peer = path.join(stateRoot, "m1-receipt-peer");
    const egressKiller = path.join(stateRoot, "m1-egress-kill");
    run("cc", [
      "-std=c11",
      "-O2",
      "-Wall",
      "-Wextra",
      "-Werror",
      path.join(this.#scripts, "m1-receipt-peer.c"),
      "-o",
      peer,
    ]);
    chmodSync(peer, 0o755);
    run("cc", [
      "-std=c11",
      "-O2",
      "-Wall",
      "-Wextra",
      "-Werror",
      path.join(this.#scripts, "m1-egress-kill.c"),
      "-o",
      egressKiller,
    ]);
    chmodSync(egressKiller, 0o700);
    const egressHelper = path.join(stateRoot, "m1-egress-helper.mjs");
    writeFileSync(
      egressHelper,
      readFileSync(path.join(this.#scripts, "m1-egress-helper.mjs")),
      { mode: 0o600 },
    );
    writeFileSync(
      path.join(stateRoot, "m1-egress-tls.mjs"),
      readFileSync(path.join(this.#scripts, "m1-egress-tls.mjs")),
      { mode: 0o600 },
    );
    this.#binaries = {
      herdr,
      omp,
      addon,
      node,
      git,
      gitExecPath,
      peer,
      egressKiller,
      egressHelper,
    };
    this.#token = token;
  }

  async provision(
    role: Role,
    seatId: string,
    workspace: string,
    controllerPaths: RoleControllerPaths,
  ): Promise<RoleRuntimeSession> {
    const request = {
      role,
      seatId,
      workspace,
      controllerPaths,
    } satisfies RoleRuntimeProvisionRequest;
    if (!ROLE_NAMES.has(request.role))
      throw new TypeError("unsupported M1 role");
    const roleKey = `${role}:${request.seatId}`;
    if (this.#sessions.has(roleKey))
      throw new Error(`runtime already exists for seat ${request.seatId}`);
    safeId(request.seatId, "seatId");
    const paths = request.controllerPaths;
    const workspacePath = validatePath(
      request.workspace,
      "workspace",
      "directory",
    );
    this.#assertPrivateDirectory(
      path.join(workspacePath, ".home"),
      "workspace .home directory",
    );
    const bridgeSocketPath = path.resolve(paths.bridgeSocketPath);
    const receiptSocketPath = path.resolve(paths.receiptSocketPath);
    if (
      path.basename(bridgeSocketPath) !== "seat.sock" ||
      path.basename(receiptSocketPath) !== "receipt.sock"
    )
      throw new Error(
        "M1 selected runtime requires bridge seat.sock and receipt.sock endpoints",
      );
    const journalPath = validatePath(paths.journalPath, "journalPath", "file");
    const journal = this.#roleJournal.get(role);
    if (journal !== undefined && journal !== journalPath)
      throw new Error(
        `role ${role} journal path changed across seat replacement`,
      );
    for (const [existingRole, existingJournal] of this.#roleJournal)
      if (existingRole !== role && existingJournal === journalPath)
        throw new Error("M1 role journals must be separate persistent files");
    if (
      [...this.#sessions.values()].some(
        (session) =>
          session.bridgeSocketPath === bridgeSocketPath ||
          session.receiptSocketPath === receiptSocketPath,
      )
    )
      throw new Error(
        "bridge and receipt sockets must be unique to each active role runtime",
      );
    if (
      [...this.#sessions.values()].some(
        (session) => session.workspace === workspacePath,
      )
    )
      throw new Error("workspace is already mounted by an active role runtime");
    this.#assertPrivateDirectory(
      path.dirname(bridgeSocketPath),
      "bridge directory",
    );
    this.#assertPrivateDirectory(
      path.dirname(receiptSocketPath),
      "receipt directory",
    );
    const evidenceDirectory = paths.evidenceDirectory
      ? validatePath(paths.evidenceDirectory, "evidenceDirectory", "directory")
      : undefined;
    if (
      (role === "Verifier" || role === "Supervisor") !==
      (evidenceDirectory !== undefined)
    )
      throw new Error(
        "Verifier and Supervisor sessions require an evidence directory",
      );
    const evidenceRoot = path.resolve(this.#options.stateRoot, "evidence");
    if (
      evidenceDirectory &&
      ((role === "Supervisor"
        ? evidenceDirectory !== evidenceRoot
        : !evidenceDirectory.startsWith(`${evidenceRoot}${path.sep}`)) ||
        realpathSync(evidenceDirectory) !== evidenceDirectory)
    )
      throw new Error(
        "role evidence directory must be a real directory under the controller evidence root",
      );
    for (const protectedRoot of [
      path.resolve(this.#options.stateRoot),
      path.dirname(journalPath),
      path.dirname(receiptSocketPath),
      path.dirname(bridgeSocketPath),
    ]) {
      if (pathsOverlap(workspacePath, protectedRoot))
        throw new Error(
          `assigned workspace overlaps controller-managed state: ${protectedRoot}`,
        );
    }
    const ordinal = ++this.#counter;
    const safeRole = role.toLowerCase();
    const containerName = `capstan-m1-${process.pid}-${ordinal}-${safeRole}`;
    const networkName = `${containerName}-net`;
    const sessionId = `${request.seatId}:${ordinal}`;
    const profile = `m1-${safeRole}`;
    const env = {
      ...process.env,
      M1_EGRESS_KILLER: this.#binaries.egressKiller,
    };
    let networkCreated = false;
    let containerId: string | undefined;
    let bridgeIdentity:
      { readonly dev: number; readonly ino: number } | undefined;
    let policyId: string | undefined;
    try {
      run(this.#docker, [
        "network",
        "create",
        "--internal",
        "--label",
        "capstan.m1.runtime=true",
        networkName,
      ]);
      networkCreated = true;
      const token = run(this.#binaries.omp, [
        "token",
        this.#options.tokenProvider ?? "openai-codex",
        "--raw",
      ]);
      if (!token || token !== this.#token)
        throw new Error(
          "OMP provider token changed after runtime verification",
        );
      const args = [
        "run",
        "--detach",
        "--name",
        containerName,
        "--label",
        "capstan.m1.runtime=true",
        "--network",
        networkName,
        "--user",
        `${process.getuid?.() ?? 0}:${process.getgid?.() ?? 0}`,
        "--read-only",
        "--cap-drop",
        "ALL",
        "--security-opt",
        "no-new-privileges",
        "--pids-limit",
        "256",
        "--mount",
        `type=bind,src=${workspacePath},dst=/workspace${role === "Developer" ? "" : ",readonly"},bind-propagation=rprivate`,
        "--mount",
        `type=bind,src=${path.dirname(bridgeSocketPath)},dst=/bridge,bind-propagation=rprivate`,
        "--mount",
        `type=bind,src=${journalPath},dst=/workspace/.home/bridge.jsonl,readonly,bind-propagation=rprivate`,
        ...(evidenceDirectory
          ? [
              "--mount",
              `type=bind,src=${evidenceDirectory},dst=/evidence${role === "Supervisor" ? ",readonly" : ""},bind-propagation=rprivate`,
            ]
          : []),
        "--mount",
        `type=bind,src=${path.dirname(receiptSocketPath)},dst=/receipt,readonly,bind-propagation=rprivate`,
        "--tmpfs",
        `/home/worker:rw,nosuid,nodev,size=256m,uid=${process.getuid?.() ?? 0},gid=${process.getgid?.() ?? 0},mode=0700`,
        "--mount",
        `type=bind,src=${this.#binaries.herdr},dst=/usr/local/bin/herdr,readonly`,
        "--mount",
        `type=bind,src=${this.#binaries.omp},dst=/usr/local/bin/omp,readonly`,
        "--mount",
        `type=bind,src=${this.#binaries.addon},dst=/usr/local/bin/pi_natives.linux-x64-baseline.node,readonly`,
        "--mount",
        `type=bind,src=${this.#binaries.node},dst=/usr/local/bin/node,readonly`,
        "--mount",
        `type=bind,src=${path.join(this.#scripts, "m1-herdr-bridge.mjs")},dst=/usr/local/bin/m1-herdr-bridge.mjs,readonly`,
        "--mount",
        `type=bind,src=${this.#binaries.peer},dst=/usr/local/bin/m1-receipt-peer,readonly`,
        "--mount",
        `type=bind,src=${this.#binaries.git},dst=/usr/local/bin/git,readonly`,
        "--mount",
        `type=bind,src=${this.#binaries.gitExecPath},dst=/usr/lib/git-core,readonly`,
        "--tmpfs",
        `/tmp:rw,nosuid,nodev,noexec,size=256m,uid=${process.getuid?.() ?? 0},gid=${process.getgid?.() ?? 0},mode=0700`,
        "--tmpfs",
        `/run:rw,nosuid,nodev,noexec,size=64m,uid=${process.getuid?.() ?? 0},gid=${process.getgid?.() ?? 0},mode=0700`,
        "--env",
        "HOME=/home/worker",
        "--env",
        "PATH=/usr/local/bin:/usr/bin:/bin",
        "--env",
        "GIT_EXEC_PATH=/usr/lib/git-core",
        "--env",
        "LC_ALL=C",
        "--env",
        "LANG=C",
        "--env",
        "TZ=UTC",
        "--env",
        "TMPDIR=/tmp",
        "--env",
        "HERDR_ENV=1",
        "--env",
        "CAPSTAN_BRIDGE_SOCKET=/bridge/seat.sock",
        "--env",
        "CAPSTAN_BRIDGE_JOURNAL=/workspace/.home/bridge.jsonl",
        "--env",
        "CAPSTAN_BRIDGE_RECEIPT_SOCKET=/receipt/receipt.sock",
        "--env",
        "CAPSTAN_BRIDGE_PEER_HELPER=/usr/local/bin/m1-receipt-peer",
        "--env",
        "CAPSTAN_BRIDGE_CONTROLLER_PEER_PID=0",
        "--env",
        `CAPSTAN_BRIDGE_ROLE=${role}`,
        "--env",
        "OPENAI_CODEX_OAUTH_TOKEN",
        "--env",
        "HTTP_PROXY",
        "--env",
        "HTTPS_PROXY",
        "--env",
        "http_proxy",
        "--env",
        "https_proxy",
        "--env",
        "NODE_USE_ENV_PROXY=1",
        ...(evidenceDirectory
          ? ["--env", "CAPSTAN_EVIDENCE_DIR=/evidence"]
          : []),
        IMAGE,
        "sh",
        "-c",
        "umask 077; exec sleep infinity",
      ];
      const containerEnv = { ...env, OPENAI_CODEX_OAUTH_TOKEN: this.#token };
      containerId = run(this.#docker, args, containerEnv);
      const containerIp = run(this.#docker, [
        "inspect",
        "--format",
        "{{range .NetworkSettings.Networks}}{{.IPAddress}}{{end}}",
        containerId,
      ]);
      const prepared = JSON.parse(
        run(
          process.execPath,
          [
            this.#binaries.egressHelper,
            "prepare",
            "--container",
            containerId,
            "--container-ip",
            containerIp,
            "--provider-host",
            this.#options.providerHost,
            "--provider-port",
            String(this.#options.providerPort ?? 443),
            "--proxy-port",
            "0",
          ],
          env,
        ),
      ) as { proxy?: string; policyId?: string };
      if (
        typeof prepared.proxy !== "string" ||
        !/^http:\/\/[^/]+:\d+$/.test(prepared.proxy) ||
        typeof prepared.policyId !== "string"
      )
        throw new Error("M1 egress helper returned invalid policy identity");
      policyId = prepared.policyId;
      const proxyArgs = [
        "--env",
        `HTTP_PROXY=${prepared.proxy}`,
        "--env",
        `HTTPS_PROXY=${prepared.proxy}`,
        "--env",
        `http_proxy=${prepared.proxy}`,
        "--env",
        `https_proxy=${prepared.proxy}`,
        "--env",
        "NODE_USE_ENV_PROXY=1",
      ];
      const exec = (argv: string[]) =>
        run(
          this.#docker,
          [
            "exec",
            "--user",
            `${process.getuid?.() ?? 0}:${process.getgid?.() ?? 0}`,
            "--env",
            "HOME=/home/worker",
            ...proxyArgs,
            containerName,
            "/bin/sh",
            "-c",
            'umask 077; exec "$@"',
            "m1-runtime",
            ...argv,
          ],
          containerEnv,
        );
      const verified = JSON.parse(
        run(
          process.execPath,
          [
            this.#binaries.egressHelper,
            "verify",
            "--container",
            containerId,
            "--policy-id",
            policyId,
          ],
          env,
        ),
      ) as {
        firewall?: { forwarding?: string; hostInput?: string };
        providerConnections?: number;
      };
      if (
        verified.firewall?.forwarding !==
          "DOCKER-USER accept-proxy-then-reject" ||
        verified.firewall.hostInput !== "INPUT accept-proxy-then-reject" ||
        verified.providerConnections !== 0
      )
        throw new Error("M1 egress policy verification failed");
      const denied = JSON.parse(
        run(
          process.execPath,
          [
            this.#binaries.egressHelper,
            "probe-deny",
            "--container",
            containerId,
            "--policy-id",
            policyId,
          ],
          env,
        ),
      ) as { denied?: boolean; hostDenied?: boolean };
      if (denied.denied !== true || denied.hostDenied !== true)
        throw new Error("M1 direct egress denial probe failed");
      if (exec(["/usr/local/bin/node", "--version"]) !== NODE_VERSION)
        throw new Error("Container Node version mismatch");
      if (
        exec(["/usr/local/bin/herdr", "--version"]) !== `herdr ${HERDR_VERSION}`
      )
        throw new Error("Container Herdr version mismatch");
      if (
        !exec(["/usr/local/bin/omp", "--help"]).startsWith(
          `omp v${OMP_VERSION}\n`,
        )
      )
        throw new Error("Container OMP version mismatch");
      exec(["/bin/mkdir", "-p", "/home/worker/.omp/agent/extensions"]);
      run(
        this.#docker,
        [
          "exec",
          "--detach",
          "--user",
          `${process.getuid?.() ?? 0}:${process.getgid?.() ?? 0}`,
          "--env",
          "HOME=/home/worker",
          ...proxyArgs,
          containerName,
          "/usr/local/bin/herdr",
          "server",
        ],
        containerEnv,
      );
      const deadline = Date.now() + 15_000;
      while (Date.now() < deadline) {
        const status = spawnSync(
          this.#docker,
          [
            "exec",
            "--user",
            `${process.getuid?.() ?? 0}:${process.getgid?.() ?? 0}`,
            "--env",
            "HOME=/home/worker",
            containerName,
            "/usr/local/bin/herdr",
            "status",
            "server",
          ],
          { encoding: "utf8" },
        );
        if (status.status === 0 && status.stdout.includes("status: running"))
          break;
        await new Promise((resolve) => setTimeout(resolve, 100));
      }
      if (
        !spawnSync(
          this.#docker,
          [
            "exec",
            "--user",
            `${process.getuid?.() ?? 0}:${process.getgid?.() ?? 0}`,
            "--env",
            "HOME=/home/worker",
            containerName,
            "/usr/local/bin/herdr",
            "status",
            "server",
          ],
          { encoding: "utf8" },
        ).stdout?.includes("status: running")
      )
        throw new Error("Selected Herdr server did not become ready");
      exec(["/usr/local/bin/herdr", "integration", "install", "omp"]);
      exec([
        "/usr/local/bin/node",
        "-e",
        "const fs=require('node:fs'),path=require('node:path');const f=path.join('/home/worker/.omp/profiles',process.argv[1],'agent/config.yml');fs.mkdirSync(path.dirname(f),{recursive:true,mode:0o700});fs.writeFileSync(f,'setupVersion: 2\\n',{mode:0o600});",
        profile,
      ]);
      const created = JSON.parse(
        exec([
          "/usr/local/bin/herdr",
          "workspace",
          "create",
          "--cwd",
          "/workspace",
          "--no-focus",
        ]),
      ) as { result?: { root_pane?: { pane_id?: string } } };
      const pane = created.result?.root_pane?.pane_id;
      if (!pane || !/^w\d+:p\d+$/.test(pane))
        throw new Error("Herdr did not create the isolated OMP pane");
      const extension = "/usr/local/bin/m1-herdr-bridge.mjs";
      exec([
        "/usr/local/bin/herdr",
        "agent",
        "start",
        `m1_${request.seatId.replace(/[^A-Za-z0-9._-]/g, "-").slice(-28)}`,
        "--kind",
        "omp",
        "--pane",
        pane,
        "--",
        "--model",
        this.#options.model ?? "openai-codex/gpt-6-sol",
        "--profile",
        profile,
        "--cwd",
        "/workspace",
        "--extension",
        extension,
      ]);
      const pidDeadline = Date.now() + 30_000;
      let ompHostPid = 0;
      while (Date.now() < pidDeadline) {
        const rows = run(this.#docker, [
          "top",
          containerName,
          "-eo",
          "pid,args",
        ])
          .split("\n")
          .slice(1);
        const matching = rows.filter(
          (line) =>
            line.includes(extension) &&
            /\somp --model /.test(line) &&
            line.includes(`--profile ${profile}`),
        );
        if (matching.length === 1) {
          ompHostPid = Number(matching[0]!.trim().split(/\s+/, 1)[0]);
          if (Number.isSafeInteger(ompHostPid) && ompHostPid > 0) break;
          ompHostPid = 0;
        }
        await new Promise((resolve) => setTimeout(resolve, 100));
      }
      if (ompHostPid === 0)
        throw new Error(
          "selected OMP host PID did not appear before the deadline",
        );
      const socketDeadline = Date.now() + 15_000;
      while (Date.now() < socketDeadline) {
        try {
          const stat = lstatSync(bridgeSocketPath);
          if (!stat.isSocket())
            throw new Error("selected bridge endpoint is not a Unix socket");
          bridgeIdentity = { dev: stat.dev, ino: stat.ino };
          break;
        } catch (error) {
          if (
            !(error instanceof Error) ||
            !("code" in error) ||
            error.code !== "ENOENT"
          )
            throw error;
        }
        await new Promise((resolve) => setTimeout(resolve, 100));
      }
      if (!bridgeIdentity)
        throw new Error("selected OMP bridge socket did not appear");
      const session: RoleRuntimeSession = Object.freeze({
        role,
        seatId: request.seatId,
        sessionId,
        containerId,
        containerName,
        networkName,
        workspace: workspacePath,
        bridgeSocketPath,
        receiptSocketPath,
        journalPath,
        expectedOmpHostPid: ompHostPid,
        helperPath: this.#binaries.peer,
        profile,
        configPath: `/home/worker/.omp/profiles/${profile}/agent/config.yml`,
        state: "ready",
      });
      this.#bridgeSockets.set(sessionId, bridgeIdentity);
      this.#sessions.set(roleKey, session);
      this.#roleJournal.set(role, journalPath);
      return session;
    } catch (error) {
      if (containerId) {
        try {
          const state = JSON.parse(
            run(this.#docker, [
              "inspect",
              "--format",
              "{{json .State}}",
              containerId,
            ]),
          ) as { Running?: boolean; Paused?: boolean };
          if (state.Running && !state.Paused)
            run(this.#docker, ["pause", containerId]);
          run(
            process.execPath,
            [
              this.#binaries.egressHelper,
              "cleanup-container",
              "--container",
              containerId,
            ],
            env,
          );
          run(this.#docker, ["rm", "-f", containerId]);
          if (bridgeIdentity)
            this.#removeBridgeSocket(bridgeSocketPath, bridgeIdentity);
        } catch (cleanupError) {
          throw new AggregateError(
            [error, cleanupError],
            "Runtime provision and containment failed",
          );
        }
      }
      if (networkCreated) {
        try {
          run(this.#docker, ["network", "rm", networkName]);
        } catch (cleanupError) {
          throw new AggregateError(
            [error, cleanupError],
            "Runtime provision and network cleanup failed",
          );
        }
      }
      throw error;
    }
  }

  async stopAndContain(
    role: Role,
    sessionId: string,
  ): Promise<RoleRuntimeContainmentProof> {
    const pair = [...this.#sessions.entries()].find(
      ([, session]) => session.role === role && session.sessionId === sessionId,
    );
    if (!pair) throw new Error(`unknown runtime session ${sessionId}`);
    const [key, session] = pair;
    let forcedKill = false;
    try {
      const state = JSON.parse(
        run(this.#docker, [
          "inspect",
          "--format",
          "{{json .State}}",
          session.containerId,
        ]),
      ) as {
        Running: boolean;
        Paused: boolean;
        Pid: number;
        ExitCode: number;
      };
      const cgroup = state.Running
        ? this.#cgroupPath(session.containerId, state.Pid)
        : null;
      if (state.Running && !state.Paused) {
        try {
          run(this.#docker, ["pause", session.containerId]);
        } catch {
          forcedKill = true;
          run(this.#docker, ["kill", "--signal", "KILL", session.containerId]);
        }
      }
      const stopped = JSON.parse(
        run(this.#docker, [
          "inspect",
          "--format",
          "{{json .State}}",
          session.containerId,
        ]),
      ) as {
        Running: boolean;
        Paused: boolean;
        Pid: number;
        ExitCode: number;
      };
      if (stopped.Running && !stopped.Paused)
        throw new Error("worker remains live before egress policy removal");
      if (cgroup && !stopped.Paused)
        this.#assertCgroupEmpty(session.containerId, cgroup);
      if (stopped.ExitCode === 137) forcedKill = true;
      const egress = JSON.parse(
        run(
          process.execPath,
          [
            this.#binaries.egressHelper,
            "cleanup-container",
            "--container",
            session.containerId,
          ],
          {
            ...process.env,
            M1_EGRESS_KILLER: this.#binaries.egressKiller,
          },
        ),
      ) as { removed?: boolean };
      if (egress.removed !== true)
        throw new Error("M1 egress helper did not confirm policy removal");
      run(this.#docker, ["rm", "-f", session.containerId]);
      let absent = false;
      try {
        run(this.#docker, ["inspect", session.containerId]);
      } catch (error) {
        if (
          !(error instanceof Error) ||
          !/No such (?:object|container)/i.test(error.message)
        )
          throw error;
        absent = true;
      }
      if (!absent) throw new Error("Docker container remains after removal");
      const bridgeIdentity = this.#bridgeSockets.get(sessionId);
      if (!bridgeIdentity)
        throw new Error("missing observed bridge socket identity");
      this.#removeBridgeSocket(session.bridgeSocketPath, bridgeIdentity);
      this.#bridgeSockets.delete(sessionId);
      run(this.#docker, ["network", "rm", session.networkName]);
      if (cgroup) this.#assertCgroupEmpty(session.containerId, cgroup);
      this.#sessions.delete(key);
      return Object.freeze({
        contained: true,
        sessionId,
        containerId: session.containerId,
        containerAbsent: true,
        cgroupEmpty: true,
        egressPolicyRemoved: true,
        networkRemoved: true,
        workspaceMount: session.workspace,
        forcedKill,
        exitCode: stopped.ExitCode,
      });
    } catch (error) {
      throw new Error(
        `Unable to prove containment for ${session.sessionId}: ${String(error)}`,
        { cause: error },
      );
    }
  }

  async close(): Promise<readonly RoleRuntimeContainmentProof[]> {
    const proofs: RoleRuntimeContainmentProof[] = [];
    for (const session of [...this.#sessions.values()])
      proofs.push(await this.stopAndContain(session.role, session.sessionId));
    return Object.freeze(proofs);
  }

  #assertPrivateDirectory(directory: string, label: string): void {
    const stat = lstatSync(directory);
    if (
      !stat.isDirectory() ||
      stat.isSymbolicLink() ||
      (process.getuid && stat.uid !== process.getuid()) ||
      (stat.mode & 0o077) !== 0
    )
      throw new Error(
        `${label} must be a private directory owned by the current user`,
      );
  }
  #removeBridgeSocket(
    socketPath: string,
    identity: { readonly dev: number; readonly ino: number },
  ): void {
    let stat;
    try {
      stat = lstatSync(socketPath);
    } catch (error) {
      if (error instanceof Error && "code" in error && error.code === "ENOENT")
        return;
      throw error;
    }
    if (
      !stat.isSocket() ||
      stat.dev !== identity.dev ||
      stat.ino !== identity.ino
    )
      throw new Error("bridge socket identity changed before cleanup");
    unlinkSync(socketPath);
  }

  #cgroupPath(containerId: string, pid: number): string {
    if (!Number.isSafeInteger(pid) || pid <= 0)
      throw new Error(`No live container init PID for ${containerId}`);
    const rows = readFileSync(`/proc/${pid}/cgroup`, "utf8").trim().split("\n");
    const unified = rows.find((row) => row.startsWith("0::"));
    if (!unified)
      throw new Error(`Unified cgroup unavailable for ${containerId}`);
    const relative = unified.slice(3).replace(/^\/+/, "");
    const candidates = [
      path.join("/sys/fs/cgroup", relative),
      `/sys/fs/cgroup/docker/${containerId}`,
    ];
    const cgroup = candidates.find((candidate) =>
      existsSync(path.join(candidate, "cgroup.events")),
    );
    if (!cgroup) throw new Error(`Cannot inspect cgroup for ${containerId}`);
    return cgroup;
  }

  #assertCgroupEmpty(containerId: string, cgroup: string): void {
    if (!existsSync(path.join(cgroup, "cgroup.events"))) return;
    const events = readFileSync(path.join(cgroup, "cgroup.events"), "utf8");
    const pids = Number(
      readFileSync(path.join(cgroup, "pids.current"), "utf8").trim(),
    );
    if (pids !== 0 || !/^populated 0$/m.test(events))
      throw new Error(
        `Container cgroup remains populated (${pids}) for ${containerId}`,
      );
  }
}
