#!/bin/sh
# Makes a fresh in-process project from the pristine copy: inproc.sh <dir under /tmp>; prints the root for stmts.mjs.
S=${SCRATCH:-/tmp/capstan-prof}
d=$1
case "$d" in /tmp/*) ;; *) echo "dir must be under /tmp" >&2; exit 1;; esac
rm -rf "$d"; mkdir -m 700 "$d" "$d/root" "$d/root/.capstan" "$d/root/.capstan/state"
cp "$S/root/.capstan/operator.key" "$d/root/.capstan/"
sed "s|$S/root|$d/root|" "$S/root/.capstan/project.json" > "$d/root/.capstan/project.json"
cp "$S/root/capstan.toml" "$d/root/"; cp -r "$S/root/roles" "$d/root/roles"
cp "$S/pristine.sqlite" "$d/root/.capstan/state/controller.sqlite"
chmod 600 "$d/root/.capstan/operator.key" "$d/root/.capstan/project.json" "$d/root/capstan.toml" "$d/root/.capstan/state/controller.sqlite"
echo "$d/root"
