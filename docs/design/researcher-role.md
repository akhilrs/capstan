# Researcher role: web research with guarded tools

**Status:** proposal, for the user's review. Written beside `docs/design/architect-role.md` and in its style.
**Source of truth for the rules:** `src/researcher-policy.ts` (the allowlist, the deny lists and the checks). This document explains why they exist and does not repeat the lists; where the two differ, the code is right.

## 1. Summary

The Researcher is a role of kind `Developer`, designated by `[researcher] role`. Like the Architect and the Operator it adds no new agent kind: it reuses the seat, worktree, branch, report and review machinery. It answers a question from the PM using public web sources and commits one Markdown report, `<output_dir>/<slug>.md`, on its own branch. It reports with `cstan report` like a developer, so the report goes through the normal review and integrate path.

Prompt text (`src/prompts.ts`): the Researcher gets `RESEARCHER_REFERENCE` in place of the architect note; the PM gets `PM_RESEARCH_SECTION`. With `[researcher]` absent every prompt is byte-identical to before.

## 2. Threat model

The Researcher reads attacker-controlled text (web pages, posts, comments) and holds a shell with network access. Prompt injection from a page is assumed to succeed sometimes. The concern is what a hijacked or merely careless Researcher can do with the tools it has:

- **Exfiltration through upload.** `curl -d`, `-F`, `-T`, `--upload-file` or a non-GET method sends local files or secrets to a server.
- **Data in the query string.** A GET request to an attacker's host whose URL, header or cookie carries project content or a secret. A GET-only rule does not remove this; it is why the prompt says never to put project files, code or secrets in a URL, header or form, and why the allowlist is limited to the endpoints the work needs.
- **Pipe to shell.** `curl ... | sh`, `| bash`, or `| python` runs downloaded code with the agent's rights.
- **File writes.** curl output flags (`-o`, `-O`, `--output`) and shell redirection write fetched content anywhere the user can write: shell startup files, project files, hooks.
- **Browser uploads and sign-in.** A browser MCP server can fill and submit forms, upload files and use stored sessions. Those tools are left out of the allowlist except what reading needs.

## 3. Why `permission_mode` must be `default`

Allow and deny rules only gate a tool call when Claude Code asks for a decision on it. `acceptEdits` and `auto` approve calls without that check, and `bypassPermissions` skips it. The configuration loader therefore refuses any other mode for the Researcher role. A call that matches no allow rule is refused or prompts; a prompt the PM cannot answer under the prompt relay rules is a refusal in practice.

## 4. Allowlist and deny lists

The role's `allow` is a short allowlist: web search and fetch, `curl` against the endpoints the guidance names, `jq`, the browser read tools, and the writes and `git`/`cstan` commands needed to commit one report. Its `deny` list is required: configuration validation refuses a Researcher whose role omits the deny rules `src/researcher-policy.ts` names (the curl upload, method, output and pipe forms, interpreters and shells, `git push`, and the browser upload and sign-in tools). The prompt points at these rules and does not restate them, so the prompt cannot go stale against the policy.

Deny beats allow in Claude Code, so the deny list holds even if a later edit widens the allowlist.

## 5. Why `--strict-mcp-config`

The Researcher's browser is an MCP server defined in `[mcp_servers]` and named in `roles.<name>.mcp`. The launcher passes the generated MCP config with `--strict-mcp-config`, so Claude Code uses only those servers and ignores any other MCP configuration found in the user's home or in the project (which could add servers with their own tools and no deny rules). Without it the Researcher's tool surface would depend on files outside `capstan.toml`.

## 6. Residual risks

- **Guards, not a sandbox.** Deny rules match command text. A shell can often reach the same effect another way, so the rules lower the chance and do not give a guarantee. The README says so plainly.
- **Query-string leakage** is not blocked mechanically; it relies on the allowlist and the rule in the prompt.
- **Prompt injection** can still bend the content of the report (omit, mislead). The report is untrusted data to the PM and a reviewer reads it before merge.
- **Browser state.** The browser server may carry cookies or profile data from earlier sessions; use an isolated profile.
- **Third-party endpoints** (Reddit, Algolia) can change or rate-limit. Live result (see `docs/researcher-live-evidence.txt`): Reddit refuses unauthenticated `.json`, `old.reddit.com` and browser access (302 to /login or 403); only the `www.reddit.com` `.rss` feeds work, at about one request a minute. The prompt tells the Researcher to use those feeds at that pace, use WebSearch with `site:reddit.com` only as a last resort (when tried it returned no Reddit pages), and record any gap under Could not verify.
- **Browser output files.** The Playwright MCP server writes snapshot and console files to its output directory, which would otherwise be `.playwright-mcp/` inside the worktree. The starter config passes `--output-dir /tmp/capstan-playwright` (supported by `@playwright/mcp` 0.0.83) so the worktree stays clean; the directory is shared by all researchers on the machine and holds page text the researcher read, so clear it when the research is done. The prompt also stages only the report file.
- **Supply chain.** The browser server is started with `npx`; pin its version in `[mcp_servers]`.
