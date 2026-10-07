#!/bin/sh
# Builds the scratch project used to profile the daemon: a snapshot of the live ledger, a scratch
# credential, a herdr shim that answers empty and logs, and a git shim that refuses paths outside /tmp.
# Usage: setup.sh <live controller.sqlite> <capstan checkout> <scratch dir under /tmp>
# Nothing here writes to the live project; the live operator.key is never read or copied.
set -eu
live=$1 repo=$2 scratch=$3
case "$scratch" in /tmp/*) ;; *) echo "scratch must be under /tmp" >&2; exit 1;; esac
mkdir -m 700 "$scratch"
root=$scratch/root
mkdir -m 700 -p "$root/.capstan/state" "$scratch/shims"
node "$repo/scripts/copy-ledger.mjs" "$live" "$root/.capstan/state" >/dev/null 2>&1
# A fresh scratch credential replaces the operator's hash in the COPY only.
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
# The live capstan.toml minus [operator] (so no restart machinery or operator runs start).
live_root=$(dirname "$(dirname "$(dirname "$live")")")
awk '/^\[operator\]/{skip=1;next} /^\[/{skip=0} !skip' "$live_root/capstan.toml" > "$root/capstan.toml"
chmod 600 "$root/capstan.toml"
cp -r "$live_root/roles" "$root/roles"
cat > "$scratch/shims/herdr" <<SH
#!/bin/sh
echo "\$(date +%s.%N) \$*" >> "$scratch/herdr-calls.log"
exit 0
SH
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
