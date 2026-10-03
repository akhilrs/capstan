/**
 * Pure rules for the Researcher role: which tool rules it may allow and which it must deny. The
 * allowlist is strict and `permission_mode = "default"` is the real control; the curl flag denies
 * are guards, not a sandbox (combined short options such as -sd, or a GET that leaks data in the
 * query string, can slip past prefix rules).
 */
import type {
  ResolvedResearcher,
  ResolvedRole,
} from "./config/capstan-config.js";

/** Tool rules the Researcher role must deny: subagents, every way to push or rewrite history, and the curl flags that send, save or read local files. */
export const RESEARCHER_REQUIRED_DENY: readonly string[] = [
  "Agent",
  "Task",
  "NotebookEdit",
  "Bash(git push)",
  "Bash(git push *)",
  "Bash(git merge *)",
  "Bash(git rebase *)",
  "Bash(git reset *)",
  "Bash(git remote *)",
  "Bash(git config *)",
  "Bash(git checkout *)",
  "Bash(git switch *)",
  "Bash(curl * -d*)",
  "Bash(curl * --data*)",
  "Bash(curl * -F*)",
  "Bash(curl * --form*)",
  "Bash(curl * -T*)",
  "Bash(curl * --upload-file*)",
  "Bash(curl * -X*)",
  "Bash(curl * --request*)",
  "Bash(curl * --json*)",
  "Bash(curl * -o*)",
  "Bash(curl * --output*)",
  "Bash(curl * -O*)",
  "Bash(curl * --remote-name*)",
  "Bash(curl * -K*)",
  "Bash(curl * --config*)",
  "Bash(curl * -u*)",
  "Bash(curl * --user*)",
  "Bash(curl * -c*)",
  "Bash(curl * --cookie-jar*)",
  "Bash(curl * -D*)",
  "Bash(curl * --dump-header*)",
  "Bash(curl * --trace*)",
  "Bash(curl * --stderr*)",
  "Bash(curl * --create-dirs*)",
  "Bash(curl * --libcurl*)",
  "Bash(curl * --hsts*)",
  "Bash(curl * --alt-svc*)",
  "Bash(curl * --etag-save*)",
  "Bash(curl * file:*)",
  "Bash(curl * @*)",
  "Bash(curl -d*)",
  "Bash(curl --data*)",
  "Bash(curl -F*)",
  "Bash(curl --form*)",
  "Bash(curl -T*)",
  "Bash(curl --upload-file*)",
  "Bash(curl -X*)",
  "Bash(curl --request*)",
  "Bash(curl --json*)",
  "Bash(curl -o*)",
  "Bash(curl --output*)",
  "Bash(curl -O*)",
  "Bash(curl --remote-name*)",
  "Bash(curl -K*)",
  "Bash(curl --config*)",
  "Bash(curl -u*)",
  "Bash(curl --user*)",
  "Bash(curl -c*)",
  "Bash(curl --cookie-jar*)",
  "Bash(curl -D*)",
  "Bash(curl --dump-header*)",
  "Bash(curl --trace*)",
  "Bash(curl --stderr*)",
  "Bash(curl --create-dirs*)",
  "Bash(curl --libcurl*)",
  "Bash(curl --hsts*)",
  "Bash(curl --alt-svc*)",
  "Bash(curl --etag-save*)",
  "Bash(curl *%output{*)",
  "Bash(curl file:*)",
  "Bash(curl @*)",
  "Bash(git * --output*)",
];

const FIXED_ALLOW: ReadonlySet<string> = new Set([
  "WebSearch",
  "WebFetch",
  "Bash(jq *)",
  "Bash(date*)",
  "Bash(git status*)",
  "Bash(git diff*)",
  "Bash(git log*)",
  "Bash(git show *)",
  "Bash(git rev-parse *)",
  "Bash(git add *)",
  "Bash(git commit *)",
  "Bash(cstan *)",
]);

const WEB_FETCH_DOMAIN = /^WebFetch\(domain:[A-Za-z0-9][A-Za-z0-9.-]*\)$/;
const CURL_RULE = /^Bash\(curl [^`$;&|<>\\()\n]*\)$/;
const MCP_RULE = /^mcp__([a-z][a-z0-9_-]*)__([A-Za-z0-9_-]+)$/;
const MCP_BARE = /^mcp__[a-z][a-z0-9_-]*$/;
const MCP_FORBIDDEN_TOOL = /upload|run_code|evaluate|install|file/i;

function allowProblem(
  rule: string,
  outputDir: string,
  servers: ReadonlySet<string>,
): string | null {
  if (
    FIXED_ALLOW.has(rule) ||
    WEB_FETCH_DOMAIN.test(rule) ||
    CURL_RULE.test(rule) ||
    rule === `Write(${outputDir}/**)` ||
    rule === `Edit(${outputDir}/**)`
  )
    return null;
  const mcp = MCP_RULE.exec(rule);
  if (mcp !== null) {
    const [, server, tool] = mcp as unknown as [string, string, string];
    if (!servers.has(server))
      return `names MCP server ${server}, which is not in this role's mcp list`;
    if (MCP_FORBIDDEN_TOOL.test(tool))
      return `must not allow the MCP tool ${tool}: tools that upload, run code, evaluate scripts, install or touch files are refused`;
    return null;
  }
  if (MCP_BARE.test(rule))
    return `must name one tool, not a whole server; the researcher role may not allow ${rule}`;
  return `is not on the researcher allowlist; the researcher role may not allow ${rule}`;
}

/** Readable problems with a role used as the Researcher; empty when it passes. The host and the other roles are checked by the config loader. */
export function researcherRuleProblems(
  role: ResolvedRole,
  researcher: ResolvedResearcher,
): string[] {
  const at = `roles.${role.name}`;
  const problems: string[] = [];
  if (role.kind !== "Developer")
    problems.push(
      `researcher.role "${role.name}" must be a Developer role; ${at}.kind is ${role.kind}`,
    );
  if (role.permissionMode !== "default")
    problems.push(
      `${at}.permission_mode must be "default" for the researcher role; it is "${role.permissionMode}"`,
    );
  const servers = new Set((role.mcp ?? []).map((server) => server.name));
  role.allow.forEach((rule, index) => {
    const problem = allowProblem(rule, researcher.outputDir, servers);
    if (problem !== null) problems.push(`${at}.allow[${index}] ${problem}`);
  });
  for (const required of RESEARCHER_REQUIRED_DENY)
    if (!role.deny.includes(required))
      problems.push(
        `${at}.deny must include ${required}: the researcher role may not start subagents, push, or send or save data with curl`,
      );
  return problems;
}
