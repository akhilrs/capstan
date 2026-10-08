#!/bin/sh
# Builds the scratch project for the v0.3.0 profile from an ALREADY COPIED ledger (made with scripts/copy-ledger.mjs
# into a 0700 directory under /tmp). Nothing here reads or writes a live file.
# Usage: setup.sh <ledger copy dir (holds controller.sqlite)> <live capstan.toml copy source dir> <checkout> <scratch dir under /tmp>
#   argv2 is the live project root; only capstan.toml and roles/ are READ from it (listed in the access log).
# Adds to plan-23's setup: a Herdr REPLAY shim (see herdr-replay) that answers agent get / pane get / pane process-info /
# pane read for every ACTIVE agent of the ledger from test fixtures, over real scratch process trees.
set -eu
copy=$1 live_root=$2 repo=$3 scratch=$4
here=$(cd "$(dirname "$0")" && pwd)
case "$scratch" in /tmp/*) ;; *) echo "scratch must be under /tmp" >&2; exit 1;; esac
mkdir -m 700 "$scratch"
root=$scratch/root
mkdir -m 700 -p "$root" "$root/.capstan" "$root/.capstan/state"; chmod 700 "$root" "$root/.capstan" "$root/.capstan/state"; mkdir -m 700 -p "$scratch/shims" "$scratch/shims" "$scratch/home" "$scratch/tmp"
cp "$copy/controller.sqlite" "$root/.capstan/state/controller.sqlite"
node -e '
const fs = require("node:fs"), crypto = require("node:crypto");
const [root, repo] = process.argv.slice(1);
const key = crypto.randomBytes(32).toString("base64url");
fs.writeFileSync(root + "/.capstan/operator.key", key + "\n", { mode: 0o600 });
const { credentialHash } = require(repo + "/dist/src/controller/auth.js");
const sqlite = process.getBuiltinModule("node:sqlite");
const db = new sqlite.DatabaseSync(root + "/.capstan/state/controller.sqlite");
const project = db.prepare("SELECT project_id, name FROM projects").get();
const triggers = db.prepare("SELECT name, sql FROM sqlite_master WHERE type = ? AND tbl_name = ?").all("trigger", "actors");
for (const t of triggers) db.exec("DROP TRIGGER " + t.name);
db.prepare("UPDATE actors SET credential_hash = ? WHERE project_id = ? AND role = ?").run(credentialHash(key), project.project_id, "operator");
for (const t of triggers) db.exec(t.sql);
db.close();
fs.writeFileSync(root + "/.capstan/project.json", JSON.stringify({ schemaVersion: 1, projectId: project.project_id, name: project.name, stateDirectory: root + "/.capstan/state", maxSlices: 4, maxRunMs: 3600000, maxDispatches: 16 }, null, 2), { mode: 0o600 });
' "$root" "$repo" 2>/dev/null
cp "$root/.capstan/state/controller.sqlite" "$scratch/pristine.sqlite"
awk '/^\[operator\]/{skip=1;next} /^\[/{skip=0} !skip' "$live_root/capstan.toml" > "$root/capstan.toml"
chmod 600 "$root/capstan.toml"
[ -d "$live_root/roles" ] && cp -r "$live_root/roles" "$root/roles" || true
# One scratch process tree per active agent: shell(S) -> host(H) -> tool shell(T) -> sleep. Real /proc entries, so the
# process-activity probe scans the real process table exactly as it does live.
: > "$scratch/replay.map"
n=0
node -e '
const sqlite = process.getBuiltinModule("node:sqlite");
const db = new sqlite.DatabaseSync(process.argv[1], { readOnly: true });
for (const r of db.prepare("SELECT a.agent_id, a.kind, p.pane_id FROM agents a JOIN agent_panes p ON p.agent_id = a.agent_id WHERE a.state = ? ORDER BY a.agent_id").all("active"))
  console.log(r.agent_id, r.pane_id, r.kind);
' "$scratch/pristine.sqlite" 2>/dev/null > "$scratch/active.txt"
while read -r id pane kind; do
  n=$((n+1))
  # Workers are "working" (the process probe runs for them); PMs and the architect are idle.
  if [ "$kind" = PM ] || [ "$id" != "${id#architect}" ]; then status=idle; else status=working; fi
  setsid env -i PATH=/usr/bin:/bin sh -c 'sh -c "sh -c \"while :; do sleep 30; done\"" & wait' >/dev/null 2>&1 &
  sleep 0.3
  printf '%s %s %s %s %s\n' "$id" "$pane" "$status" "$!" "$kind" >> "$scratch/replay.map"
done < "$scratch/active.txt"
cp "$here/../../../test/fixtures/herdr-process-info.json" "$scratch/process-info.tpl" 2>/dev/null || cp "$repo/test/fixtures/herdr-process-info.json" "$scratch/process-info.tpl"
cp "$repo/test/fixtures/claude-idle-empty.ansi" "$scratch/screen.ansi"
cp "$here/herdr-replay" "$scratch/shims/herdr"
sed "s|@SCRATCH@|$scratch|g" "$here/herdr-replay" > "$scratch/shims/herdr"
cat > "$scratch/shims/git" <<SH
#!/bin/sh
for a in "\$@"; do
  case "\$a" in
    /*) case "\$a" in /tmp/*) ;; *) echo "git shim: refused \$a" >> "$scratch/git-refused.log"; echo "fatal: refused" >&2; exit 128;; esac;;
  esac
done
case "\$(pwd -P)" in /tmp/*) ;; *) echo "git shim: refused cwd \$(pwd -P)" >> "$scratch/git-refused.log"; echo "fatal: refused" >&2; exit 128;; esac
echo "\$(date +%s.%N) \$*" >> "$scratch/git-calls.log"
exec /usr/bin/git "\$@"
SH
chmod 755 "$scratch/shims/herdr" "$scratch/shims/git"
(cd "$root" && /usr/bin/git init -q . && /usr/bin/git -c user.name=s -c user.email=s@s commit -q --allow-empty -m init)
echo "$root"
