import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { pathToFileURL } from "node:url";
import { parseCommitSubject } from "../src/conventions.js";

// The scripts are plain ESM that run unbuilt; load them from the source tree, not dist.
const scriptsDir = path.resolve(import.meta.dirname, "..", "..", "scripts");
const notes = (await import(
  pathToFileURL(path.join(scriptsDir, "release-notes.mjs")).href
)) as {
  parseCommit(message: string): { type: string; breaking: boolean } | null;
  nextVersion(
    current: string,
    commits: string[],
  ): { version: string; level: string } | null;
  renderChangelogSection(
    version: string,
    date: string,
    commits: { sha: string; message: string }[],
  ): string;
  lastReleaseTag(root: string): string | null;
};

test("level: fix and perf are patch, feat is minor, breaking is major", () => {
  assert.deepEqual(notes.nextVersion("1.2.3", ["fix: a"]), {
    version: "1.2.4",
    level: "patch",
  });
  assert.equal(notes.nextVersion("1.2.3", ["perf(x): a"])?.level, "patch");
  assert.deepEqual(notes.nextVersion("1.2.3", ["fix: a", "feat: b"]), {
    version: "1.3.0",
    level: "minor",
  });
  assert.deepEqual(notes.nextVersion("1.2.3", ["feat!: b", "feat: c"]), {
    version: "2.0.0",
    level: "major",
  });
  assert.equal(
    notes.nextVersion("1.2.3", ["fix: a\n\nBREAKING CHANGE: gone"])?.level,
    "major",
  );
});

test("level: breaking on 0.y.z is minor", () => {
  assert.deepEqual(notes.nextVersion("0.1.1", ["feat!: a"]), {
    version: "0.2.0",
    level: "minor",
  });
  assert.deepEqual(notes.nextVersion("0.1.1", ["feat: a"]), {
    version: "0.2.0",
    level: "minor",
  });
});

test("level: only docs/chore commits, merges and junk give null", () => {
  assert.equal(
    notes.nextVersion("1.0.0", ["docs: a", "chore: b", "test: c", "ci: d"]),
    null,
  );
  assert.equal(
    notes.nextVersion("1.0.0", ["Merge branch 'x'", "fixed stuff"]),
    null,
  );
  assert.equal(
    notes.nextVersion("1.0.0", ["Merge branch 'x'", "fix: real"])?.level,
    "patch",
  );
});

test("parseCommit reads scope, breaking footer and rejects junk", () => {
  assert.deepEqual(notes.parseCommit("feat(ui)!: big"), {
    type: "feat",
    scope: "ui",
    breaking: true,
    description: "big",
    note: null,
  });
  assert.equal(
    notes.parseCommit("fix: a\n\nBREAKING-CHANGE: x")?.breaking,
    true,
  );
  assert.equal(notes.parseCommit("Merge pull request #1"), null);
});

test("release-notes accepts and rejects the same subjects as src/conventions", () => {
  const subjects = [
    "feat: add",
    "fix(core): bug",
    "feat(ui)!: big",
    "revert: undo",
    "style: fmt",
    "Feat: caps",
    "feat:nospace",
    "feat:  two spaces",
    "feat(): empty",
    "feat(a b): space",
    "wip: nope",
    "feature: nope",
    "feat: ",
    "Merge branch 'main'",
    "",
  ];
  for (const subject of subjects)
    assert.equal(
      notes.parseCommit(subject) !== null,
      parseCommitSubject(subject).ok,
      JSON.stringify(subject),
    );
});

test("changelog section groups by type with scope and short sha", () => {
  const sha = "abcdef0123456789";
  const section = notes.renderChangelogSection("2.0.0", "2026-10-04", [
    { sha, message: "chore: tidy" },
    { sha, message: "feat(ui): shiny" },
    { sha, message: "revert: undo x" },
    { sha, message: "perf: quick" },
    { sha, message: "fix: bug" },
    { sha, message: "feat!: drop\n\nBREAKING CHANGE: old api removed" },
    { sha, message: "docs: words" },
  ]);
  assert.equal(
    section,
    [
      "## 2.0.0 (2026-10-04)",
      "",
      "### Breaking changes",
      "",
      "- old api removed (abcdef0)",
      "",
      "### Features",
      "",
      "- **ui:** shiny (abcdef0)",
      "",
      "### Bug fixes",
      "",
      "- bug (abcdef0)",
      "",
      "### Performance",
      "",
      "- quick (abcdef0)",
      "",
      "### Reverts",
      "",
      "- undo x (abcdef0)",
      "",
      "### Documentation",
      "",
      "- words (abcdef0)",
      "",
      "### Chores",
      "",
      "- tidy (abcdef0)",
      "",
    ].join("\n"),
  );
});

test("lastReleaseTag picks the highest vX.Y.Z reachable from HEAD", () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "cstan-tags-"));
  try {
    const git = (...args: string[]): string =>
      execFileSync(
        "git",
        ["-c", "user.email=t@t", "-c", "user.name=t", ...args],
        { cwd: dir, encoding: "utf8" },
      );
    git("init", "-q");
    assert.equal(notes.lastReleaseTag(dir), null);
    git("commit", "-q", "--allow-empty", "-m", "chore: a");
    git("tag", "v0.9.0");
    git("tag", "v0.10.0");
    git("tag", "vnext");
    git("tag", "v0.10.0-rc1");
    git("commit", "-q", "--allow-empty", "-m", "chore: b");
    git("tag", "v0.2.0");
    assert.equal(notes.lastReleaseTag(dir), "v0.10.0");
    git("checkout", "-q", "-b", "side", "HEAD~1");
    git("commit", "-q", "--allow-empty", "-m", "chore: c");
    git("tag", "v5.0.0");
    git("checkout", "-q", "-");
    assert.equal(notes.lastReleaseTag(dir), "v0.10.0");
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }
});
