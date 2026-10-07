# Changelog

All notable changes are listed here, newest release first. `npm run release` writes each section from the Conventional Commits since the last tag.

## 0.2.0 (2026-10-07)

### Features

- sign off a plan on a merged or confirmed integration (7360c06)
- **dash:** Rust dashboard, daemon status query fix and live-daemon guards (f897095)
- Show active tasks in cstan dash (9105c75)
- Dashboard cleanup and waiting-on-you strip (d587aeb)
- **cli:** require a git repository with a commit; add cstan init --git (43cd2cf)
- **messaging:** Reliable PM message delivery: pull all, unread notice, reliable (edde605)
- Paused golden regenerated (only the supervision header line (ea430ce)
- status/dash now show real supervision state (config enabled (73285e1)
- Git standards: Conventional Commits, SemVer releases, task-based (1977676)
- Untracked all review screenshots and git-ignored (286942c)
- Designer workflow: Claude Design first, design skills, anti-slop (3484432)
- README line 67 restored to amend wording (new commit on top, other (d9d4b51)

### Bug fixes

- collapse duplicate changelog entries in the release notes (37271bd)
- give a daemon start time to answer on a loaded machine (fd07672)
- **dash:** fold multi-line spawn titles; share the task row on short (dced0eb)
- **dash:** findings counter shows the visible range; caption says ended (5519c8a)
- **herdr:** mark restart and operator PM notices action-needed (09349e0)

### Documentation

- add a Dashboard (cstan-dash) install section to the README (d28f8d9)
- document the cstan-dash install and dashboard selection in the (432a7a4)
- measure dash and daemon CPU and memory use (0d610c0)

### Refactoring

- **controller:** Split oversized modules and remove the dormant legacy engine (bb534ae)

### Tests

- **dash:** end-to-end parity, redraw, key and performance checks of the (6c1159f)

### Chores

- remove obsolete plans, gate evidence and scratch files (0309406)
