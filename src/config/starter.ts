/** The commented starter capstan.toml that `cstan init` writes. */

export const STARTER_CONFIG = `schema_version = 1

[limits]
max_workers = 3

[layout]
spawn = "pane"
pm_width_percent = 60

# While workers are active the controller keeps one Supervisor running and sends it a routine
# check. A Supervisor is a Claude session, so it uses usage; set enabled = false to turn it off.
[supervision]
enabled = true
check_seconds = 300

# An optional Architect plans normal and high-risk work, runs integration and signs it off. The
# user still merges to the main branch. While enabled = false, nothing about plans reaches an agent.
# To use it, remove the leading # from this table and from [roles.architect] below. The role must
# be a Developer role on a claude host; reviewer_role, if set, must be a Verifier role.
# [architect]
# enabled = true
# role = "architect"
# plan_review = "high_risk"     # "high_risk" | "always" | "never"
# reviewer_role = "reviewer"
# max_packages = 8              # 1 to 20
# count_toward_worker_limit = false
# high_risk_triggers = ["schema or migrations", "security or auth", "public contracts or wire formats", "cross-cutting changes"]

# An optional Operator runs shell commands for the PM, but only one proposal at a time and only after
# you approve it in the PM's picker; auto_approve lists exact read-only commands that skip the picker
# (the allowlist is in src/operator-policy.ts). While enabled = false, nothing about an Operator
# reaches an agent. To use it, remove the leading # from this table and from [roles.operator] below.
# The role must be a Developer role on a claude host, different from the architect role.
# [operator]
# enabled = true
# role = "operator"
# auto_approve = ["ls -l", "git rev-parse --short HEAD"]   # exact commands only
# auto_approve_prefix = []      # opt-in prefixes; only safe path arguments may follow
# timeout_seconds = 300
# max_timeout_seconds = 1800    # at most 3600
# output_tail_bytes = 8192      # at most 12288
# proposal_ttl_minutes = 60
# approval_ttl_minutes = 10
# max_pending_proposals = 5
# count_toward_worker_limit = false
# restart_health_timeout_seconds = 60
# restart_idle_wait_seconds = 120
# session_grant_max_minutes = 60    # how long "approve and allow again" grants last; at most 480
# full_auto_default_minutes = 30    # full auto: the PM may switch off every guard for this long, only after asking you
# full_auto_max_minutes = 120       # the longest full auto period; at most 480

# An optional prompt relay lets the PM show you a blocked worker's permission prompt and type the answer
# you pick. While enabled = false (the default), nothing about it reaches an agent and both
# cstan prompt subcommands are unavailable. To use it, remove the leading # from this table.
# [prompt_relay]
# enabled = true
# capture_ttl_seconds = 600     # how long a shown prompt can be answered; 60 to 3600

# An optional Researcher reads the web and writes findings under output_dir. It is a Developer role
# on a claude host that must use permission_mode "default", a strict allow list and the deny list in
# src/researcher-policy.ts; its MCP servers come from [mcp_servers.<name>] and the role's mcp key.
# The curl denies are guards, not a sandbox: permission_mode "default" plus the strict allow list is
# the real control. While enabled = false, nothing about a Researcher reaches an agent. To use it,
# remove the leading # from this table and from [roles.researcher] below; [mcp_servers.playwright] is
# already defined for the designer. The role must differ from the architect and operator roles.
# [researcher]
# enabled = true
# role = "researcher"
# output_dir = "docs/research"  # repo-relative; the only place the role may write
# user_agent = "capstan-researcher/1.0 (research bot; contact: project owner)"

# An MCP server a role may use (roles.<name>.mcp lists server names). The name becomes the
# mcp__<name>__ tool prefix. Pin the package version; an unpinned package (@latest or no @version) is warned about.
# The designer and the optional researcher share the playwright server below. To switch it off for the
# designer, remove its mcp line (its prompt then reports the Playwright checks as unverified).
[mcp_servers.playwright]
command = "npx"
args = ["-y", "@playwright/mcp@0.0.83", "--headless", "--isolated", "--output-dir", "/tmp/capstan-playwright"]

# Whether the PM mirrors work into Nexora. Policy only: connection details stay in .nexora.toml,
# which Capstan never reads. "ask" shows the PM's intake picker, "always" applies default_action
# without asking, "never" removes every Nexora instruction from the PM prompt.
# [nexora]
# track = "ask"                 # "ask" | "always" | "never"
# default_action = "create"     # what "always" does: "create" | "link" | "none"

# The model and permission mode a role gets when it sets none of its own, by role kind
# (PM, Supervisor, Developer, Verifier). [defaults] itself applies to every kind. A role's own
# model or permission_mode always wins. permission_mode = "auto" lets Claude Code's auto mode
# approve routine actions, so agents stall at permission prompts less often.
[defaults.PM]
model = "claude-opus-5-5"

[defaults.Supervisor]
model = "claude-opus-5-5"

[defaults.Developer]
model = "claude-sonnet-5-5"
# permission_mode = "auto"

[defaults.Verifier]
model = "claude-sonnet-5-5"

# Variables an agent needs beyond the basic ones (PATH, HOME, USER, LANG, TERM...) are copied
# from the environment where \`cstan start\` runs, never from your interactive shell file alone.
# Name them here (one-line values only); a name that is not set where the daemon starts is reported
# when agents launch.
# [env]
# pass = ["NEXORA_API_KEY"]

[hosts.claude]
kind = "claude"

# Optional Codex and OMP hosts for Developer and Verifier roles. Both run unattended with full access
# and without a sandbox: nothing stops a push or an edit outside the worktree, and \`cstan config check\`
# warns about every such role. PM, Supervisor, architect, operator and researcher roles stay on a claude host.
# [hosts.codex]
# kind = "codex"
# [hosts.omp]
# kind = "omp"

[roles.pm]
kind = "PM"
host = "claude"

[roles.developer]
kind = "Developer"
host = "claude"
permission_mode = "acceptEdits"
allow = ["Bash(git *)"]
deny = ["Bash(git push)", "Bash(git push *)"]
prompt = "You implement code changes. Work only in your own worktree and commit your work on your own branch in small commits. Never push and never merge. When you finish, tell the project manager the branch name, what you changed and what you could not verify."

[roles.designer]
kind = "Developer"
host = "claude"
permission_mode = "acceptEdits"
mcp = ["playwright"]
allow = ["Bash(git *)", "Skill", "Artifact", "DesignSync", "mcp__playwright", "Bash(python3 -m http.server *)"]
deny = ["Bash(git push)", "Bash(git push *)"]
# The designer's workflow lives in roles/designer.md, which \`cstan init\` writes. Edit that file to change it.
prompt_file = "roles/designer.md"

[roles.reviewer]
kind = "Verifier"
host = "claude"
permission_mode = "acceptEdits"
allow = ["Bash(git *)"]
deny = ["Write", "Edit", "NotebookEdit", "Agent", "Task", "Bash(git push)", "Bash(git push *)"]
prompt = "You review one commit when the controller asks. Read the change, do not edit any file and never push or merge. Judge correctness, tests and risk, and say plainly what you could not check."

[roles.tester]
kind = "Verifier"
host = "claude"
permission_mode = "acceptEdits"
allow = ["Bash(git *)"]
deny = ["Bash(git push)", "Bash(git push *)"]
prompt = "You test and verify behavior. Run the real checks, report exactly what passed and what failed, and add tests only when asked. Work only in your own worktree and commit any test changes on your own branch. Never push and never merge. When you finish, tell the project manager the branch name and the result."

[roles.supervisor]
kind = "Supervisor"
host = "claude"
deny = ["Write", "Edit", "NotebookEdit", "Agent", "Task", "Bash(git push)", "Bash(git push *)", "Bash(herdr *)", "Bash(tmux *)"]
prompt = "You watch the other agents and raise findings when one is stuck. You only read and report through cstan; you never edit files and never run project commands."

# A worker role on Codex or OMP (remove the leading # here and from the host above). It must set its
# own model, because the [defaults.Developer] model is a Claude model; it must set permission_mode
# to acceptEdits or auto; and it must have no allow or deny. Use any model your CLI accepts.
# [roles.codex-developer]
# kind = "Developer"
# host = "codex"
# model = "<a model name your codex CLI accepts>"
# permission_mode = "acceptEdits"
# prompt = "You implement code changes. Work only in your own worktree and commit your work on your own branch in small commits. Never push and never merge. When you finish, tell the project manager the branch name, what you changed and what you could not verify."

# [roles.architect]
# kind = "Developer"
# host = "claude"
# permission_mode = "acceptEdits"
# allow = ["Bash(git *)"]
# deny = ["Write", "Edit", "NotebookEdit", "Agent", "Task", "Bash(git push)", "Bash(git push *)", "Bash(git merge *)"]
# prompt = "You plan and integrate; you never edit or commit project files. Follow the project rules the PM gives you."

# [roles.operator]
# kind = "Developer"
# host = "claude"
# permission_mode = "default"
# allow = ["Bash(cstan *)"]
# deny = ["Write", "Edit", "NotebookEdit", "Agent", "Task", "Read", "Glob", "Grep"]
# prompt = "You run shell commands for the PM through cstan op propose, and nothing else."

# The Researcher role: web search and fetch, read-only curl, a headless browser through the
# playwright MCP server, and writes only under the researcher output_dir. See [researcher] above.
# [roles.researcher]
# kind = "Developer"
# host = "claude"
# permission_mode = "default"
# mcp = ["playwright"]
# allow = ["WebSearch", "WebFetch", "Bash(curl *)", "Bash(jq *)", "Bash(date*)", "Write(docs/research/**)", "Edit(docs/research/**)", "Bash(git status*)", "Bash(git diff*)", "Bash(git log*)", "Bash(git show *)", "Bash(git rev-parse *)", "Bash(git add *)", "Bash(git commit *)", "mcp__playwright__browser_navigate", "mcp__playwright__browser_navigate_back", "mcp__playwright__browser_snapshot", "mcp__playwright__browser_click", "mcp__playwright__browser_type", "mcp__playwright__browser_press_key", "mcp__playwright__browser_wait_for", "mcp__playwright__browser_tabs", "mcp__playwright__browser_close"]
# deny = ["Agent", "Task", "NotebookEdit", "Bash(git push)", "Bash(git push *)", "Bash(git merge *)", "Bash(git rebase *)", "Bash(git reset *)", "Bash(git remote *)", "Bash(git config *)", "Bash(git checkout *)", "Bash(git switch *)", "Bash(curl * -d*)", "Bash(curl * --data*)", "Bash(curl * -F*)", "Bash(curl * --form*)", "Bash(curl * -T*)", "Bash(curl * --upload-file*)", "Bash(curl * -X*)", "Bash(curl * --request*)", "Bash(curl * --json*)", "Bash(curl * -o*)", "Bash(curl * --output*)", "Bash(curl * -O*)", "Bash(curl * --remote-name*)", "Bash(curl * -K*)", "Bash(curl * --config*)", "Bash(curl * -u*)", "Bash(curl * --user*)", "Bash(curl * -c*)", "Bash(curl * --cookie-jar*)", "Bash(curl * -D*)", "Bash(curl * --dump-header*)", "Bash(curl * --trace*)", "Bash(curl * --stderr*)", "Bash(curl * --create-dirs*)", "Bash(curl * --libcurl*)", "Bash(curl * --hsts*)", "Bash(curl * --alt-svc*)", "Bash(curl * --etag-save*)", "Bash(curl * file:*)", "Bash(curl * @*)", "Bash(curl -d*)", "Bash(curl --data*)", "Bash(curl -F*)", "Bash(curl --form*)", "Bash(curl -T*)", "Bash(curl --upload-file*)", "Bash(curl -X*)", "Bash(curl --request*)", "Bash(curl --json*)", "Bash(curl -o*)", "Bash(curl --output*)", "Bash(curl -O*)", "Bash(curl --remote-name*)", "Bash(curl -K*)", "Bash(curl --config*)", "Bash(curl -u*)", "Bash(curl --user*)", "Bash(curl -c*)", "Bash(curl --cookie-jar*)", "Bash(curl -D*)", "Bash(curl --dump-header*)", "Bash(curl --trace*)", "Bash(curl --stderr*)", "Bash(curl --create-dirs*)", "Bash(curl --libcurl*)", "Bash(curl --hsts*)", "Bash(curl --alt-svc*)", "Bash(curl --etag-save*)", "Bash(curl *%output{*)", "Bash(curl file:*)", "Bash(curl @*)", "Bash(git * --output*)"]
# prompt = "You research questions on the web and write sourced findings as files under docs/research, then commit them on your own branch. Never push and never merge."
`;
