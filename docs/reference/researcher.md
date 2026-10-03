# Researcher reference

Back to the [README](../../README.md).

An optional **Researcher** agent looks things up on the web for the PM and writes the result as a report in your repository. Nothing about it exists while `[researcher]` is absent or `enabled = false`: no role, no prompt text, no browser server.

**What it is for.** Questions that need public sources: what people report about a tool, a comparison, a source check. The PM spawns it with `cstan spawn <role>`, sends the question and later gets a report on the Researcher's branch. The Researcher is a Developer-kind role: it writes exactly one file, `<output_dir>/<slug>.md`, with the sections Executive summary, Method, Findings (consensus kept apart from isolated opinion), Conflicts and disagreements, Sources (with dates) and Could not verify. It commits the file on its own branch and reports it with `cstan report`. You review and merge it like any other report.

**Enable it.**

1. In `capstan.toml`, uncomment the three blocks the starter file carries (the wording of the comments may differ; the keys are fixed):

```toml
[researcher]
enabled = true
role = "researcher"
output_dir = "docs/research"
user_agent = "capstan-researcher/1.0 (research bot; contact: project owner)"

[mcp_servers.playwright]
command = "npx"
args = ["-y", "@playwright/mcp@<pinned version>", "--headless", "--isolated", "--output-dir", "/tmp/capstan-playwright"]

[roles.researcher]
kind = "Developer"
host = "claude"
permission_mode = "default"
mcp = ["playwright"]
allow = ["WebSearch", "WebFetch", "Bash(curl *)", "Bash(jq *)", "Write(docs/research/**)", "Edit(docs/research/**)", "Bash(git add *)", "Bash(git commit *)", "mcp__playwright__browser_navigate", "mcp__playwright__browser_snapshot"] # abbreviated
deny = ["Agent", "Task", "Bash(git push)", "Bash(git push *)", "Bash(git * --output*)", "Bash(curl * -d*)", "Bash(curl -d*)", "Bash(curl * -o*)", "Bash(curl -X*)"] # abbreviated
prompt = "..."
```

The `allow` and `deny` lists above are abbreviated; the full required lists are in `src/researcher-policy.ts`, which is the source of truth. The example is abbreviated and cannot be pasted as is: copy the full deny list from the starter file. `output_dir` is repo-relative without `..` (default `docs/research`), `user_agent` is one line without quotes, and a server name matches `^[a-z][a-z0-9_-]{0,31}$` and becomes the `mcp__<name>__` tool prefix.

2. Install a browser once: `npx playwright install chromium`. The `--output-dir` argument (supported by `@playwright/mcp` 0.0.83) keeps the browser's snapshot and console files out of the researcher's worktree; without it a `.playwright-mcp/` directory appears there.
3. Make Node and `npx` available on the daemon's `PATH` (the browser server is started with `npx`).

`[mcp_servers]` and `roles.<name>.mcp` are general: any role on a `claude` host may list servers in `mcp`. The Researcher is the first user.

**What it may do.** Search and fetch public pages (WebSearch, WebFetch), call public JSON endpoints with `curl` (the Hacker News Algolia API; Reddit refuses unauthenticated JSON, `old.reddit.com` and browser access, so it reads only the `www.reddit.com` `.rss` feeds, about one request a minute, and records anything it could not read under Could not verify) and drive the browser server to read pages that need JavaScript. **What it may not do.** Anything but GET requests; send project files, code or secrets in a URL, header or form; pipe a download into a shell or interpreter; use curl output flags; sign in or fill in forms other than a site's search box; write outside `output_dir`; push, merge or start subagents. Its `permission_mode` must stay `default` so that every tool call is checked against the role's allow and deny rules. The authoritative rules are in `src/researcher-policy.ts`; see `docs/design/researcher-role.md` for the reasoning.

**The curl denies are guards, not a sandbox.** They refuse the obvious ways to send data out or run downloaded code. They do not stop a determined agent from using some other allowed tool in an unexpected way, and the Researcher reads untrusted pages that may try to steer it. Run it on a machine where you accept that.
