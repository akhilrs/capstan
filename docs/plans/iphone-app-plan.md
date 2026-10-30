# Capstan for iPhone: development plan for cloud sessions

Status: ready to hand out. Written 2026-10-30. Base: the approved design record `docs/design/iphone-app.md` (direction A, Glass Inbox) and the approved base-layer plan (plan-142, revision 5), carried into Phase 1 below.

Who reads this: a Claude Code cloud session on Linux, and the person who runs it (the user). The session has **no Mac, no Swift toolchain, no Xcode, no `cstan`, no Capstan controller**. It writes Swift it cannot compile. The user compiles and tests on their Mac (Xcode 26.6, swift-tools 6.2) and pastes the results back. Every package below is written for that loop.

## Contents

1. Goal and settled decisions
2. Architecture
3. Working rules for a cloud session
4. How a package is done (the loop with the user)
5. Phase 1: base layer (plan-142)
6. Phase 2: the app (PM-382)
7. Phase 3: glanceable surfaces and release (PM-380)
8. Mac test checklist per phase
9. Progress table
10. Open risks and questions
11. Appendix: facts the sessions need

---

## 1. Goal and settled decisions

**Goal.** A native iPhone app, Capstan for iPhone, with full parity with the Mac app (`docs/design/mac-app.md`): see what agents on your hosts wait for, read the exact text, answer or forward it, follow agents and plans, message the PM, and merge. It is a window onto `ssh <host> cstan events` and `cstan decide` (`docs/remote-protocol.md`, protocol v1). The app reads everything and acts only through request-bound decisions. A tap on a phone is never an approval: approvals and prompt answers are forwarded to the PM, who confirms with the person.

### 1.1 Decisions that are settled (do not reopen)

| # | Decision | Detail |
|---|---|---|
| D1 | Full parity with the Mac app | Same screens, adapted to iOS (design record, 17 boards). Differences are listed in the record's "What differs from the Mac" table. |
| D2 | Direct SSH, no relay | The phone opens SSH connections to the hosts itself, like the Mac. No server, no cloud, no push service. |
| D3 | Key in the Secure Enclave | One key per phone, used for every host: P-256 (`ecdsa-sha2-nistp256`), created in the Secure Enclave, never exportable. |
| D4 | ed25519 Keychain fallback | Offered for hosts that accept only ed25519, with a warning that it can be backed up and restored to another device. Never a silent fallback: if the enclave is unavailable (Simulator) the app shows an error. |
| D5 | Face ID on by default | A pure rule, `FaceIDRule` (section 1.2). Can be turned off in Settings. |
| D6 | Background refresh is read-only | It needs no Face ID, never sends a decision, and never makes a notification from agent text. |
| D7 | Live Activity and widgets stay | They show the last known count with an "as of" time. No server updates them: the app updates them while it runs and background refresh updates them briefly. |
| D8 | Push from the app is deferred | A merge from the phone does **not** push by default and there is **no push control anywhere** in the first release (no switch, no button, no "Push main…" result action). The design record shows a Push switch and a "Push main…" action on board 10; this plan supersedes the record there (see section 10, R1). |
| D9 | Untrusted host text rules as on the Mac | Agent text is plain text only: never markdown, never a link, never in a notification, Live Activity or widget. Design link is shown as text; opening it needs "Open in Safari…" after a confirmation. |
| D10 | Poll interval at least 1000 ms | Same minimum as the Mac. Default 2000 ms. |
| D11 | iOS 26 only, SwiftUI, Swift 6 language mode | Deployment target iOS 26.0. Packages declare `.iOS(.v26)` and `.macOS(.v14)` (the macOS platform is there so `swift test` runs on the Mac). |

### 1.2 FaceIDRule (the one Face ID rule)

Pure function in `CapstanPhoneCore`:

```
required = settingOn && (lastUnlock == nil || (backgroundedAt != nil && now - backgroundedAt > 60 s))
```

- `awayThreshold = 60 s`.
- Never required for background refresh.
- `lastUnlock == nil` means no unlock in this process (cold start included): required when the setting is on.
- `backgroundedAt == nil` means never backgrounded: not required once unlocked.
- `.inactive` (the Face ID sheet, the notification shade) is not background and records nothing.
- The app keeps `lastUnlock` and `backgroundedAt`: scene phase `.background` sets `backgroundedAt`; an `.active` check that passed or was not required sets `lastUnlock` (if passed) and clears `backgroundedAt`; `.inactive` changes nothing.
- The design record says "Face ID before connecting" (boards 01, 14 and the Mac comparison row). That wording **means FaceIDRule**: once per activation, 60 s away. Not per connection.

### 1.3 Out of scope

Push notifications from a server, any relay, an iPad or Watch layout, App Store submission (TestFlight readiness only), editing hosts' `authorized_keys` from the app, a password or keyboard-interactive SSH login, SSH agent forwarding, a pty, port forwarding.

---

## 2. Architecture

### 2.1 Layout

```
apps/macos/                         (exists; shared code stays here)
  CapstanKit/                       protocol v1 client, ssh argv quoting, stores, decide (Foundation only)
  CapstanAppCore/                   app logic with no SwiftUI (presentation models, outcomes, overview rows)
  Capstan/, Capstan.xcodeproj       the Mac app (never edited by iPhone work, except where a package says so)

apps/ios/                           (new)
  README.md                         build and test steps (package ios-shell writes it)
  CapstanPhone.xcodeproj/           the app project (iOS app target, widget extension later)  [ios-shell]
  CapstanPhone/                     app sources (synchronized folder)                          [ios-shell and screens]
    App/  Shared/  Waiting/  Detail/  Outcomes/  Agents/  Plans/  Overview/  Messages/  Merge/  Hosts/  Onboarding/  Settings/  System/
  CapstanWidgets/                   widget and Live Activity extension                         [Phase 3]
  CapstanPhoneCore/                 local package: iPhone logic, no SwiftUI, no NIO
    Sources/CapstanPhoneCore/{Keys,Gate,Trust,Hosts,Coordinator,Lifecycle,Settings,...}
    Tests/CapstanPhoneCoreTests/...
  CapstanSSH/                       local package: SSH exec channels on swift-nio-ssh
    Sources/CapstanSSH/{Runner,Tuning}
    Tests/CapstanSSHTests/{Runner,Tuning}
  Spikes/NIOSSHSpike/               standalone spike package, kept as a record
  scripts/
    check-static.sh                 source rules for the app (ios-shell)
    check-xcodeproj.py              project file guard (ios-shell)
    check-shared-platform.py        shared-code platform guard (kit-ios)
docs/design/iphone-app.md           design record (merged from docs/PM-379-iphone-app-design)
docs/design/iphone-app/             board PNGs
docs/plans/iphone-app-plan.md       this file
```

### 2.2 Package graph

```
swift-nio-ssh 0.15.0 (exact)
        │
CapstanSSH ───────────────► CapstanPhoneCore ──► CapstanKit      (apps/macos/CapstanKit, path ../../macos/CapstanKit)
        │                         │          └─► CapstanAppCore  (apps/macos/CapstanAppCore)
        └──────────────────────────────────────► CapstanKit
CapstanPhone (app)  ──► CapstanSSH, CapstanPhoneCore, CapstanKit, CapstanAppCore
CapstanWidgets      ──► CapstanPhoneCore (snapshot types only), CapstanKit (models only)
```

Rules that follow:

- `CapstanKit` and `CapstanAppCore` are reused **by relative path** from `apps/macos`. They are never copied.
- Only `CapstanSSH` and the spike import NIO. `CapstanPhoneCore` never imports `CapstanSSH` or NIO.
- Nothing depends on `CapstanSSH` except the app project.
- `CapstanKit` imports only Foundation and Dispatch; `CapstanAppCore` may also import `CapstanKit`; `CapstanPhoneCore` may import Foundation, CryptoKit, Security, LocalAuthentication, CapstanKit, CapstanAppCore. These are enforced by `check-static.sh` (Mac) and `check-shared-platform.py` (iOS).

### 2.3 The SSH seam

The Mac starts `/usr/bin/ssh <args>` through `ProcessRunner.start(_:_:)` (`CapstanKit/Stream/ProcessRunner.swift`). Callers (`Decider`, `HostSession`, `Probe`) build the argv with `RemoteCommand.sshArguments(...)`. They do not change.

On iOS there is no `ssh` binary. `SSHRunner` (in `CapstanSSH`) implements `ProcessRunner`:

1. `start(executable, argv)` requires `executable.path == RemoteCommand.sshPath` (`/usr/bin/ssh`) and `SSHInvocation`'s inverse `invocation(from: argv)` to parse; otherwise it throws.
2. `SSHInvocation {destination, identityFile: String?, remote}` is the exact inverse of `sshArguments`. On iOS `identityFile` is always `nil`; `SSHRunner` throws on a non-nil one.
3. The destination is an alias `capstan-<first 12 hex of the host id>` (`PhoneHost.alias(for:)`) because `SSHInvocation` has no port field. `PhoneHostRegistry` resolves the alias to `PhoneHost {id, hostname, port, user}`. The alias is never shown in the UI; the UI shows `hostname:port`.
4. `SSHRunner` opens (or reuses) one SSH connection per host, one **exec channel** per `start`, runs `remote` as the command, sends stdin EOF right after the exec request succeeds, splits stdout into lines with the public `LineSplitter`, keeps `stdoutAll` (max 1 MiB) and `stderrTail` (last 4096 bytes, lossy UTF-8), and maps exit status (`exit-status`, `128 + n` for `exit-signal`, `255` for any connection, auth, host-key or channel failure).
5. Host keys are pinned (trust on first use, block on change) in `HostKeyStore`. The runner emits `TransportEvent`s (host key unknown/changed, auth refused, unsupported algorithm, pins unreadable) on an `AsyncStream`; the app's coordinator turns them into UI.

### 2.4 The shared-platform check

`apps/ios/scripts/check-shared-platform.py` (written by `kit-ios`, `--self-test` included) fails when `CapstanKit` or `CapstanAppCore` outside `#if os(macOS)` and comments contain: `Process(`, `kill(`, `NSWorkspace`, `NSSound`, `AppKit`, or `/usr/bin/` (except `RemoteCommand.sshPath`); or when `CapstanPhoneCore` imports anything beyond Foundation, CryptoKit, Security, LocalAuthentication, CapstanKit, CapstanAppCore. Tokens match on word boundaries (`skill(`, `LiveProcessRunner(`, `ProcessRunner(` are not findings). Every package from `kit-ios` on runs it and lists it in its report.

### 2.5 Storage

Hosts (`HostsFile`), host-key pins and `PhoneHost`s live in the app's **Application Support**, not the App Group. The widget extension (Phase 3) reads only a small snapshot file in the App Group (counts, kinds, project names that are not agent text, "as of" time).

---

## 3. Working rules for a cloud session

1. **One package per session, one branch per package.** Branch name `feat/ios-<package>` (for example `feat/ios-phone-keys`). Docs-only packages use `docs/ios-<package>`. Create it from the latest `main` (or from the branch of a package this one depends on if that package is not merged yet; say so in the PR).
2. **Commits.** Conventional Commits 1.0.0: `<type>[(scope)][!]: <description>`; types `feat fix docs refactor perf test build ci chore style revert`; one space after the colon; blank line before a body. One commit per package where practical; small follow-up commits are fine. **No AI attribution**: no `Co-Authored-By` trailer naming Claude or any AI, no "Generated with" line, no session id, in commits or PR text.
3. **Never push to `main`, never merge.** Push your package branch and open a pull request for the user. The user merges. If the branch is behind `main`, rebase it; do not merge `main` into it.
4. **Only edit what the package owns.** Each package lists "Owns". Do not touch other files. **Never edit `apps/macos` except in `kit-ios`.** If you need a change outside your files, stop and put it in the PR as a question.
5. **Do not edit the design record** (`docs/design/iphone-app.md`, `docs/design/iphone-app/`). Report a problem in it in the PR.
6. **Keep the design tokens and forbidden moves** (section 11.3). Use system materials and controls (`.glassEffect`, `Material`, `presentationBackground`), SF Pro, SF Mono, SF Symbols. No custom blur, no second accent, no gradient except the system material.
7. **Static checks that run on Linux** (they need only `bash`, `perl`, `python3`). Run all that exist, with their `--self-test`, before every commit and list them in the PR:
   ```sh
   apps/macos/scripts/check-static.sh && apps/macos/scripts/check-static.sh --self-test
   python3 apps/macos/scripts/check-xcodeproj.py && python3 apps/macos/scripts/check-xcodeproj.py --self-test
   python3 apps/ios/scripts/check-shared-platform.py && python3 apps/ios/scripts/check-shared-platform.py --self-test   # after kit-ios
   apps/ios/scripts/check-static.sh && apps/ios/scripts/check-static.sh --self-test                                      # after ios-shell
   python3 apps/ios/scripts/check-xcodeproj.py && python3 apps/ios/scripts/check-xcodeproj.py --self-test               # after ios-shell
   ```
   They are guards, not proofs: only a Mac build shows the code compiles.
8. **You cannot build.** Write Swift carefully (Swift 6 strict concurrency: `Sendable`, `@MainActor`, no data races). Never claim "builds" or "tests pass"; write "not built here" in the PR and list the exact Mac commands (section 8). If the user pastes build errors, fix them in a follow-up commit on the same branch (do not amend after the PR is open).
9. **Tests come with the code.** Each package names its tests in "Acceptance". Write them in the same commit. Tests use fakes only (no network except `127.0.0.1` in `CapstanSSH`, no Keychain, no Secure Enclave, no Face ID). Device-only behaviour is a device check (section 8), not a unit test.
10. **Match the surrounding code**: naming, comment density, file layout. Folders under `Sources/<Target>/<Area>/` use default paths, so new files need no manifest edit. App sources are in a synchronized folder, so new files there need no project edit.
11. **Untrusted text.** Text from hosts is displayed only as plain text in an `UntrustedTextBlock`-style view with its header ("Command, untrusted text"). `check-static.sh` rules forbid markdown/link detection and non-literal `Text(...)`. Follow the same pattern as `apps/macos/Capstan/Shared/UntrustedTextBlock.swift`.
12. **When blocked** (an API you cannot verify, a design conflict, an ambiguity), do not invent. Write the question at the top of the PR under "Needs a decision" and stop at the safe point.

### 3.1 PR template (copy into every PR)

```
Package: <id>   Phase: <n>   Depends on: <ids>
What changed: <3 to 6 lines>
Files: <owned paths>
Not built here. Tests written: <names>
Static checks run (Linux): <list, each with result>
User must run on the Mac:
  <exact commands from the package>
Paste back: <what the package asks for>
Needs a decision: <questions or "none">
```

---

## 4. How a package is done

1. The user gives the session this file and the package id (for example: "Do package `phone-keys` from `docs/plans/iphone-app-plan.md`").
2. The session reads the package, its dependencies' code (already merged), and the sources named in section 11.1, then writes code and tests on `feat/ios-<package>`.
3. It runs the Linux static checks, commits, pushes the branch and opens the PR with the template.
4. The user pulls the branch on the Mac, runs the commands in the PR, and pastes back: `swift test` summary lines (or `error:` lines), the `xcodebuild` result (`** BUILD SUCCEEDED **` or `error:` lines), and anything the package asks for. Include the Xcode version if a build fails.
5. The session fixes failures with a follow-up commit. Repeat until the user's run is green. The user merges the PR and updates the progress table (section 9).
6. **Gates** (G1..G4, section 5.1) are user decisions recorded in the progress table. A package that depends on a gate must not start before the gate is open.

---

## 5. Phase 1: base layer (plan-142)

Phase 1 is the approved plan-142 carried over faithfully. It is the logic layer plus the SSH transport; no UI. Everything is verified by `swift test` on the Mac. The packages are `design-record`, `ssh-spike`, `kit-ios`, `phone-keys`, `phone-trust`, `ssh-transport`, `ssh-tuning`.

Integration order: design-record, ssh-spike, kit-ios, phone-keys, phone-trust, ssh-transport, ssh-tuning.

### 5.1 Gates

| Gate | Meaning | Gates what |
|---|---|---|
| G1 | The design record is merged (it is, with this plan) | assigning `phone-keys` |
| G2 | The user's builds named in `kit-ios`, `phone-keys`, `phone-trust` are green | merging each of those three |
| G3 | The user's run of `ssh-spike`'s tests, with the results table | assigning `ssh-transport` |
| G4 | `CapstanSSH`'s `swift test` and iOS Simulator build are green | merging `ssh-transport` and `ssh-tuning` |

A failed gate reopens the package whose code failed. "Depends on" means merged.

Precondition for `kit-ios`: both Mac packages move to swift-tools 6.2. The user checks that no open Mac work (other branches touching either `Package.swift`) is in flight; any such branch rebases on `kit-ios` afterwards. Tools 6.2 needs Xcode 26 (the Mac README already requires it).

### 5.2 Package `design-record`

| | |
|---|---|
| Goal | The approved iPhone design is in the repository. |
| Owns | `docs/design/iphone-app.md`, `docs/design/iphone-app/` |
| Depends on | nothing |
| Size | done by the merge that brought this plan; 1 h |
| Acceptance | The record says approved 2026-10-30 (revision 2); the Approval section lists the three answers; rows 01 and 14 name them; boards 01 and 14 PNGs were updated. |
| Mac steps | none |

Status: **done** (merged with this plan). Nothing to build. Gate G1 is open when this plan's PR is merged. This plan's settled rules supersede the record where they are more exact (the Face ID wording, section 1.2; the push control, section 1.1 D8). No package edits the record.

### 5.3 Package `ssh-spike`

| | |
|---|---|
| Goal | Prove what swift-nio-ssh 0.15.0 can do before building on it. |
| Owns | `apps/ios/Spikes/NIOSSHSpike/` |
| Depends on | nothing |
| Size | M (about 2 h) |

**Interfaces.** A standalone package `apps/ios/Spikes/NIOSSHSpike` (tools 6.2, macOS 14 and iOS 26) depending only on `.package(url: "https://github.com/apple/swift-nio-ssh.git", exact: "0.15.0")`, with a committed `Package.resolved`; no project code. Its tests run an in-process server on `127.0.0.1` and record, **one test per item**:

1. client userauth signed with a Secure Enclave key via `NIOSSHPrivateKey(secureEnclaveP256Key:)` (skipped with a clear message where no enclave exists);
2. an outbound signal request;
3. an `exit-signal` received;
4. stdin EOF after exec;
5. kex `curve25519-sha256`, `ecdh-sha2-nistp256`, `ecdh-sha2-nistp384`, `ecdh-sha2-nistp521`;
6. host keys `ssh-ed25519` and `ecdsa-sha2-nistp256/384/521`;
7. ciphers `aes128-gcm@openssh.com`, `aes256-gcm@openssh.com`;
8. whether the client's host-key algorithm order can be set (a probe: settable is "passed", fixed is recorded as "fixed", not failed).

The package stays in the repository as the record of what the library does; nothing depends on it.

**Acceptance.** The PR carries a table (item, test name, expected, passed/failed/skipped). Static: `grep -rn 'import Capstan' apps/ios/Spikes` finds nothing; `Package.swift` has the exact pin; tests bind only `127.0.0.1` (grep). G3 needs every item passed except the enclave item (may be skipped where `swift test` has no enclave) and the host-key-order probe.

**Outcomes and what each changes** (the user decides with the architect or this plan's owner):

| Result | Consequence |
|---|---|
| Enclave signing fails | re-plan: ed25519 becomes the default key kind (design and key screen change) |
| Signals missing | `ssh-tuning` terminate is close-only |
| `exit-signal` or extended data missing | exit code 255 and an empty tail |
| An algorithm missing | dropped from the `ssh-tuning` list |
| Host-key order fixed | a changed algorithm stays "changed" (possible false alarm, the safe side) |

**Mac steps and paste-back.**
```sh
swift test --package-path apps/ios/Spikes/NIOSSHSpike 2>&1 | tail -60
```
Paste the full test list with pass/fail/skip per test, and the `Package.resolved` hash if it changed.

### 5.4 Package `kit-ios`

| | |
|---|---|
| Goal | `CapstanKit` and `CapstanAppCore` build for iOS; `SSHInvocation` (the argv inverse); the shared-platform check. |
| Owns | `apps/macos/CapstanKit/Package.swift`, `apps/macos/CapstanAppCore/Package.swift`, `apps/macos/CapstanKit/Sources/CapstanKit/Stream/ProcessRunner.swift`, `apps/macos/CapstanKit/Sources/CapstanKit/Stream/RemoteCommand.swift`, `apps/macos/CapstanKit/Tests/CapstanKitTests/Stream/LiveProcessRunnerTests.swift`, `.../Stream/InvocationTests.swift` (new), `.../Stream/LineSplitterTests.swift` (new), `apps/ios/scripts/check-shared-platform.py` (new) |
| Depends on | nothing (needs the precondition in 5.1) |
| Size | M (about 3 h) |

This is the **only** package that edits `apps/macos`.

**Interfaces.**
- Both `Package.swift`: `// swift-tools-version: 6.2`, `platforms: [.macOS(.v14), .iOS(.v26)]`; nothing else changes.
- `ProcessRunner.swift`: `ProcessRunner` and `RunningProcess` unchanged. `LiveProcessRunner` and `LiveProcess` (`Foundation.Process`, `kill`) are wrapped in `#if os(macOS)`. `LineSplitter` becomes `public` with `public init()`, `public mutating func feed(_:) throws -> [Data]` and `public static let maxLineBytes` (= `LineDecoder.maxLineBytes - 1`; `LineDecodeError` is already public). `Latch` stays internal. Everything outside `#if os(macOS)` compiles for iOS. No flush: as on the Mac's `LiveProcess`, a final line without a newline is dropped at end of stream.
- `RemoteCommand.swift` adds `public struct SSHInvocation: Sendable, Equatable { destination: String; identityFile: String?; remote: String }` and `public static func invocation(from: [String]) -> SSHInvocation?`: the exact inverse of `sshArguments`; accepts only the options `sshArguments` writes, then `--`, the destination and one remote string; `nil` otherwise.
- `RemoteCommandTests.swift` needs no edit; round trips live in `InvocationTests.swift`.
- `LiveProcessRunnerTests` is wrapped in `#if os(macOS)`; other tests unchanged.
- `apps/ios/scripts/check-shared-platform.py` with `--self-test` (rules in 2.4). The self-test plants tokens in an `#else` of `#if os(macOS)` and in a nested `#if` and expects findings; negatives `skill(`, `LiveProcessRunner(`, `ProcessRunner(` give none.

**Acceptance (tests).**
- `InvocationTests`: `invocation(from: sshArguments(c, remote: r))` round-trips for configs with and without a key file and remotes with quotes, spaces and `sh -c` scripts (one with newlines and an embedded single quote, as `Probe`'s); `nil` for an extra, missing or reordered option, a missing `--`, a second remote word and an empty list. Also every argv a Mac caller passes to `runner.start` parses: `RemoteCommand.ssh` (events), `sshArguments` with `shScript` (probe), `DecideRequest.sshArguments` and `Probe`'s `'true'`. A grep finds no other `runner.start` call site in `CapstanKit` or `CapstanAppCore` than `Decider`, `HostSession` and `Probe`.
- `LineSplitterTests` (plain `import CapstanKit`, not `@testable`): chunks split into lines; a line of `maxLineBytes` passes and one byte more throws `LineDecodeError.tooLong`; an unterminated tail is not returned. It mirrors today's splitter, so Mac behaviour is unchanged.
- `check-shared-platform.py` and its self-test pass after the package's edits. `apps/macos/scripts/check-static.sh` and `check-xcodeproj.py` still pass.

**Mac steps (Gate G2) and paste-back.**
```sh
swift test --package-path apps/macos/CapstanKit
swift test --package-path apps/macos/CapstanAppCore
xcodebuild -project apps/macos/Capstan.xcodeproj -scheme Capstan -configuration Debug -destination 'platform=macOS' build
xcodebuild -scheme CapstanKit -destination 'generic/platform=iOS Simulator' build   # run inside apps/macos/CapstanKit
xcodebuild -scheme CapstanAppCore -destination 'generic/platform=iOS Simulator' build   # run inside apps/macos/CapstanAppCore
```
Paste: the `Executed N tests` lines (or `error:` lines) for the two `swift test`s, `BUILD SUCCEEDED` for the three builds, and the Xcode version. Tools 6.2 also changes how `Capstan.xcodeproj` resolves the local packages: a Mac app build failure is fixed inside this package before merge. If it fails G2 the whole phase waits (it blocks four packages).

### 5.5 Package `phone-keys`

| | |
|---|---|
| Goal | `CapstanPhoneCore` package; the iPhone's SSH key; the Face ID rule. |
| Owns | `apps/ios/CapstanPhoneCore/Package.swift`, `.../Sources/CapstanPhoneCore/Keys/`, `.../Tests/CapstanPhoneCoreTests/Keys/` (with `Fixtures/`), `.../Sources/CapstanPhoneCore/Gate/`, `.../Tests/CapstanPhoneCoreTests/Gate/` |
| Depends on | `design-record` (G1), `kit-ios` (merged) |
| Size | L (about 4 h) |

**Interfaces.**
- Package `CapstanPhoneCore` (tools 6.2; iOS 26 and macOS 14): depends on `../../macos/CapstanKit` and `../../macos/CapstanAppCore`. **One target with default paths**, so later directories need no manifest edit. The test target declares `resources: [.copy("Keys/Fixtures")]`; tests read fixtures through `Bundle.module`.
- Keys: `PhoneKeyKind {secureEnclaveP256, keychainEd25519}`; `PhoneKey` wraps `SecureEnclave.P256.Signing.PrivateKey` or `Curve25519.Signing.PrivateKey`; `protocol KeyVault { current, create, delete, replace(_ kind:) throws -> PhoneKey }`. `replace` creates the new key first and deletes the old one only after that succeeds, so a failure (enclave unavailable) leaves the old key. `KeychainKeyVault`: the enclave key uses `SecAccessControl` (`AfterFirstUnlockThisDeviceOnly`, `.privateKeyUsage`), **no biometry flag**, and its `dataRepresentation` is stored with the same accessibility; ed25519 uses `AfterFirstUnlock`, not synchronizable. `OpenSSHKey.publicLine` and `OpenSSHKey.fingerprint` print exactly as `ssh-keygen` does.
- Gate: `protocol BiometricGate { func unlock(reason:) async -> Bool }`; `FaceIDGate` (LocalAuthentication); and the pure `FaceIDRule.required(settingOn:, lastUnlock:, backgroundedAt:, now:) -> Bool` with `awayThreshold = 60 s` (section 1.2). PM-382's lifecycle planner calls it.
- iOS `HostConfig.identityFile` is always `nil`. `CapstanPhoneCore` never imports `CapstanSSH` or NIO.

**Acceptance (tests).**
- `KeysTests`: a fake vault covers create/current/delete for ed25519 and, for `secureEnclaveP256`, the fake's "unavailable" error (an enclave key cannot exist under `swift test`; enclave create and signing are device tests). `publicLine` and `fingerprint` match `ssh-keygen` fixtures made by the committed `Keys/Fixtures/gen-keys.sh`, which derives them from the committed private key files (`ssh-keygen -y` and `-lf`); the Swift tests read the committed outputs; `gen-keys.sh --check` re-derives and diffs them (run it on Linux and say so in the PR). A P-256 PEM is loaded with `P256.Signing.PrivateKey(pemRepresentation:)` (its public key type is the enclave key's); an ed25519 seed for ed25519.
- `KeysTests`: `replace` succeeds (new key, old gone); `replace` failing on create keeps the old key.
- `GateTests`: `FaceIDRule` at 59 s and 61 s away, never unlocked, cold start, never backgrounded, two background cycles, setting off.
- Fixtures are test-only keys named `*.test-key` with a README that says so.
- `check-shared-platform.py` passes.

**Mac steps (G2) and paste-back.**
```sh
swift test --package-path apps/ios/CapstanPhoneCore
```
Paste the summary lines. The enclave tests are not run here; they are device checks in Phase 2 (`device-checks`).

Risk to remember: an enclave key is device-only. After a restore or on a new phone `current()` is nil; the app shows "This iPhone needs a new key" with the new public line, and hosts answer `authRefused` until it is in `authorized_keys`. The Simulator has no Secure Enclave: the vault returns an error the UI shows (never a silent ed25519).

### 5.6 Package `phone-trust`

| | |
|---|---|
| Goal | Host-key pins, transport events, phone hosts and their stores. |
| Owns | `apps/ios/CapstanPhoneCore/Sources/CapstanPhoneCore/Trust/`, `.../Hosts/`, `.../Tests/CapstanPhoneCoreTests/Trust/`, `.../Hosts/` |
| Depends on | `kit-ios`, `phone-keys` (merged) |
| Size | L (about 5 h) |

**Interfaces.**
- Trust: `HostKeyPin {algorithm, blob, fingerprint}`, one pin per host. `TrustVerdict {trusted, unknown(pin), changed(pinned, offered), pinsUnreadable}`: the pinned key is trusted, any other key is `changed` and blocked, no pin is `unknown`. `trust()` sets a pin only on `unknown`; `forget` (Remove host) clears it. `HostKeyStore`: `Sendable final class`, JSON loaded at `init`, verdict from memory (no IO, safe on an event loop), writes on its own serial queue, `sweep(keeping:)` drops pins of unsaved hosts.
- Also in `Trust/`: `TransportEvent {hostKeyUnknown(hostId, pin), hostKeyChanged(hostId), authRefused(hostId), unsupported(hostId, UnsupportedReason {hostKeyType(String), noCommonAlgorithm}), pinsUnreadable}`; `PendingTrust {hostId, pin}`; `protocol SessionControl { restart(hostId:) }` (implemented by the PM-382 coordinator); `protocol TransportControl: Sendable { events: AsyncStream<TransportEvent>; latestEvent(hostId:); suspend() }` (implemented by `SSHRunner`). A host's latest event clears on its next successful start, `trust()` or `forget`.
- If the pin file exists but cannot be read, every verdict is `pinsUnreadable` (never `unknown`), the transport emits `TransportEvent.pinsUnreadable` and connects to nothing until the user resets all pins (`resetAll()`, then every host is `unknown` and asks for trust again).
- Hosts: `PhoneHost {id (the HostConfig id), hostname, port 1..65535 default 22, user}`; one key per phone, used for every host. `PhoneHostRegistry` (`init(store: HostKeyStore)`; `Sendable final class`, lock-protected, readable from any thread or event loop) resolves the alias to one `PhoneHost` or nil (`SSHRunner` then throws "unknown destination"). `registerCandidate(_:)` adds an in-memory entry for a host being checked; `commit(id)` keeps it when the host is added; `discard(id)` drops it and forgets its pins; uncommitted candidates vanish on relaunch and `HostKeyStore.sweep` at launch removes their pins. A host with the same user, port and hostname (lowercased, trailing dot removed) as a saved one is refused. A `resolve` that ran before a `discard` keeps its `PhoneHost` copy for that one connection. `PhoneHostRegistry.newHostId()` returns an id whose alias is unused.
- Alias: `PhoneHost.alias(for: id) = "capstan-<first 12 hex of the id>"`, valid under `HostConfig.validate`, never shown (UI and error tails use `hostname:port`). An alias already registered makes `addHost` draw a new id.
- `PhoneHostStore`: `load() -> [PhoneHost]` and `save(_:)` as JSON in Application Support beside `HostsFile`. Launch sequence (run by the PM-382 coordinator): load both, register every saved `PhoneHost`, `HostKeyStore.sweep(keeping: saved ids)`, then start sessions. `HostKeyStore.trust()` returns once the write is queued; a kill before it lands loses the pin and the user trusts again (accepted). `addHost` saves `PhoneHostStore` first, then `HostsFile`; start drops a `HostConfig` without a `PhoneHost` or the reverse (a kill between the writes) and saves the cleaned lists (a start-sequence helper in `Hosts/`).

**Acceptance (tests).**
- `TrustTests`: unknown; trusted after `trust()`; changed on another blob or another algorithm; `forget` and `sweep` clear; reload keeps pins; a corrupt file gives `pinsUnreadable` (never `unknown` or `trusted`) until `resetAll()`.
- `HostsTests`: validation; duplicate refused (case, trailing dot); alias lookup; two ports on one host; candidate resolve, commit, discard (pins forgotten).
- `RegistryRaceTests`: 10,000 resolves on 8 threads while commit and discard run; no crash; each resolve returns the old or the new state.
- `PhoneHostStoreTests`: save then load returns the same hosts; a missing file gives `[]`; a corrupt file gives `[]` and is not overwritten until the next save.
- Start-sequence helper: a `HostConfig` without a `PhoneHost`, or the reverse, is dropped and the cleaned lists are saved.
- `check-shared-platform.py` passes.

**Mac steps (G2) and paste-back.**
```sh
swift test --package-path apps/ios/CapstanPhoneCore
swift test --sanitize=thread --package-path apps/ios/CapstanPhoneCore
```
Paste both summaries (the thread-sanitizer run must be clean).

### 5.7 Package `ssh-transport`

| | |
|---|---|
| Goal | `CapstanSSH`: SSH exec channels on swift-nio-ssh behind `ProcessRunner`. |
| Owns | `apps/ios/CapstanSSH/Package.swift`, `apps/ios/CapstanSSH/Package.resolved`, `.../Sources/CapstanSSH/Runner/`, `.../Tests/CapstanSSHTests/Runner/` |
| Depends on | `kit-ios`, `phone-keys`, `phone-trust` (merged), `ssh-spike` (G3) |
| Size | XL (about 7 h). Split point if it slips: idle close and `suspend` may come as a second PR of this package before G4; exec, auth, host keys and events may not. |

**Interfaces.**
- Library: `apple/swift-nio-ssh`, `.package(exact: "0.15.0")` in `CapstanSSH/Package.swift`. Rejected: libssh2 (C, OpenSSL, no enclave signing), Citadel (extra layer), NMSSH (unmaintained). The app project's `Package.resolved` belongs to `ios-shell`.
- Package graph: `CapstanSSH` (tools 6.2) depends on `CapstanPhoneCore`, `CapstanKit` and swift-nio-ssh exactly 0.15.0; nothing depends on `CapstanSSH` but the app.
- `public final class SSHRunner: ProcessRunner, TransportControl` with `init(registry: PhoneHostRegistry, vault: KeyVault, trust: HostKeyStore, tuning: TransportTuning, clock: any SessionClock, hook: any TransportHook = DefaultHook())`.
  - `start`: `executable.path` must equal `RemoteCommand.sshPath` and `invocation(from:)` must parse, else it throws. One connection per host on demand, one exec channel per `start`, idle close 60 s after the last channel. Publickey auth with the vault's key only; no password, keyboard-interactive, agent, forwarding, pty or env.
  - `RunningProcess` on a channel behaves as the Mac's `LiveProcess`: lines through `LineSplitter`; `stdoutAll` keeps at most 1 MiB (1_048_576 bytes); `stderrTail` is the last 4096 bytes as lossy UTF-8; `terminate(grace)` closes the channel (`ssh-tuning` adds TERM first); `exitCode` = `exit-status`, `128 + n` for `exit-signal` (always mapped here; the spike only decides whether `ssh-tuning` can test it), `255` with a one-line tail for any connection, auth, host-key or channel failure.
  - stdin: EOF is sent right after the exec request succeeds (the Mac runs `ssh -T` and never writes stdin), so a remote command that reads stdin ends.
  - `suspend()` closes every connection at once (channels end 255); the app calls it after stopping sessions on background; the next `start` reconnects.
  - `TransportTuning {connectTimeout 10 s, idleClose 60 s, keepalive 15/15/3}`; `clock: SessionClock`; an internal `SocketOptionsObserver` lets tests read the applied options. `SSHRunner` owns its `MultiThreadedEventLoopGroup(1)`; the app calls `shutdown()` on termination.
  - `TransportEvent` stream: unbounded `AsyncStream` plus `latestEvent(hostId)`, so a late consumer loses nothing; the single consumer is the app's coordinator, subscribed before any start. One `SSHRunner` per app process, shared with background refresh; never suspended while a refresh task runs.
  - Host-key validation calls `HostKeyStore.verdict` on the event loop (no IO, no main-actor hop) and completes NIO's promise there.
  - Keys off the loop: `SSHRunner` reads `vault.current()` on its own task before connecting and holds the `PhoneKey` for that connection; NIO signs once per connection on the loop (one enclave call, accepted).
  - `BatchMode` and `ServerAlive` in the parsed argv are accepted and ignored; `TransportTuning` applies instead.
  - `protocol TransportHook: Sendable` (in `Runner/`, implemented in `Tuning/` by `ssh-tuning`): `channelOptions() -> [SocketOption]` (`SocketOption {level, name, value}`, applied at bootstrap); `algorithms(pinnedHostKey: String?) -> AlgorithmPreferences {hostKeys, kex, ciphers: [String]}` (the runner maps names it can set and ignores the rest); `terminate(_ channel: Channel, grace: TimeInterval, clock: any SessionClock) -> EventLoopFuture<Void>`. `DefaultHook`: no options, the library's algorithms, close-only terminate.
  - Unsupported: any negotiation failure emits `unsupported` (`hostKeyType(name)` when the server named only host-key types outside the list, else `noCommonAlgorithm`) and exits 255 with tail `unsupported: <reason>`.
  - The idle close is a task sleeping on the injected `SessionClock` (CapstanSSH's tests carry their own `TestClock` copy; CapstanKit's is in its test target); `suspend` needs no time; keepalive is socket options only, read back, never timed.
- Test fixture: `TestSSHServer` (`Tests/CapstanSSHTests/Runner/TestSSHServer.swift`, in-process, `127.0.0.1`) and `TestClock`; `ssh-tuning`'s tests use them and do not edit them.
- The algorithm list, signal handling and host-key order follow `ssh-spike`'s results. No fork of swift-nio-ssh.

**Acceptance (tests against the in-process server on 127.0.0.1).**
- exec `echo` gives the line and exit 0; `exit 3` gives 3; a 70000-byte line ends lines with `LineDecodeError.tooLong`; `stdoutAll` keeps 1 MiB; the stderr tail is the last 4096 bytes, lossy UTF-8.
- Host key: unknown emits `hostKeyUnknown` and exits 255 without running the command; after `trust()` the next start runs; a changed key emits `hostKeyChanged` and exits 255 every time. With `pinsUnreadable` it emits that event, exits 255 and connects to nothing.
- Auth: a server that refuses the key gives 255 and `authRefused`; no password attempt is made (server log).
- Two starts on one host share one connection; `terminate` closes only the channel; idle close after 60 s (test clock); `suspend()` ends open channels with 255 and the next start reconnects.
- `start` with a non-ssh executable, an unparsable argv or an unknown destination throws and opens nothing.
- `HostSession` and `checkHost` from CapstanKit run unchanged on `SSHRunner` against the test server with a fake `cstan` script that prints a protocol v1 snapshot.
- `suspend()` during a `Decider` call ends it with 255: the outcome is `notSent` with its `pendingId` and the record stays in `PendingStore` for retry (as on the Mac when ssh dies).
- stdin: a remote `cat` ends with exit 0 and no output.
- An event emitted before the consumer subscribes is still read, and `latestEvent` returns it.
- Unsupported: a server with only an RSA host key emits `unsupported(hostKeyType("ssh-rsa"))`; a test `TransportHook` restricting kex to `curve25519-sha256` against a server offering only `ecdh-sha2-nistp384` emits `unsupported(noCommonAlgorithm)`.
- `import NIO...` appears only under `apps/ios/CapstanSSH` and the spike (grep); `check-shared-platform.py` passes.

**Mac steps (G4) and paste-back.**
```sh
swift test --package-path apps/ios/CapstanSSH
xcodebuild -scheme CapstanSSH -destination 'generic/platform=iOS Simulator' build   # run inside apps/ios/CapstanSSH
```
Paste the test summary and `BUILD SUCCEEDED` (Darwin socket options and NIO on iOS are what the Simulator build checks).

Risk: if `ssh-spike`'s enclave signing fails, only the defaults change (ed25519 becomes the default kind; the key screen changes); `SSHRunner`'s interface does not depend on it.

### 5.8 Package `ssh-tuning`

| | |
|---|---|
| Goal | Signals, `exit-signal`, the algorithm list, host-key order and TCP keepalive. |
| Owns | `apps/ios/CapstanSSH/Sources/CapstanSSH/Tuning/`, `.../Tests/CapstanSSHTests/Tuning/` |
| Depends on | `ssh-transport` (merged or G4 open for its commit) |
| Size | M (about 4 h) |

**Interfaces.** Implements `TransportHook` in `Tuning/` only (the diff stays inside `Tuning/`).
- Liveness: `SO_KEEPALIVE` with `TCP_KEEPALIVE 15`, `TCP_KEEPINTVL 15`, `TCP_KEEPCNT 3` (Darwin) closes a dead link in about 60 s (channels end 255). `HostSession`'s heartbeat check stays the main detector; the gap to the Mac is accepted.
- Algorithms: kex `curve25519-sha256`, `ecdh-sha2-nistp256/384/521`; host keys `ssh-ed25519`, `ecdsa-sha2-nistp256/384/521`; ciphers `aes128-gcm@openssh.com`, `aes256-gcm@openssh.com`. Otherwise `TransportEvent.unsupported` and exit 255.
- Host-key algorithm order: when a host has a pin, its algorithm is offered first; if the library cannot order them (spike), a different algorithm stays "changed" (a possible false alarm, the safe side) and the PR says so.
- `channelOptions` are the keepalive options; `algorithms` gives the list with the pinned host key first; `terminate` sends signal `TERM` then closes after the grace (close-only if the spike showed signals missing; the PR says so). The runner (`ssh-transport`) applies all three and maps `exit-signal`.

**Acceptance (tests).**
- Algorithms: a server offering only `ecdsa-sha2-nistp521` passes; ordering puts the pinned host key first when settable; a pinned host with several key types resolves when ordering is possible.
- `terminate`: a remote that exits on TERM ends before the grace with `128 + 15`; one that ignores TERM is closed after the grace; an `exit-status` after the close request is still the exit code.
- Socket options read back via `SocketOptionsObserver`. Dead-link timing is not tested here (it is a device check in Phase 2, `device-checks`).

**Mac steps (G4) and paste-back.** Same two commands as `ssh-transport`; paste both results.

Risk: without host-key ordering a host with several key types may show a false "changed"; recovery is Remove host, add it again and trust the new key after comparing fingerprints (the Phase 2 host screens show this).

### 5.9 Phase 1 summary table

| Package | Owns (short) | Depends on | Size | Gate | Mac run |
|---|---|---|---|---|---|
| design-record | docs/design/iphone-app* | none | done | G1 | none |
| ssh-spike | Spikes/NIOSSHSpike | none | M | G3 | spike `swift test` |
| kit-ios | macos Package.swift x2, ProcessRunner, RemoteCommand, 3 tests, check-shared-platform.py | none | M | G2 | 2 swift tests, Mac build, 2 iOS builds |
| phone-keys | CapstanPhoneCore Keys, Gate | G1, kit-ios | L | G2 | swift test |
| phone-trust | CapstanPhoneCore Trust, Hosts | kit-ios, phone-keys | L | G2 | swift test + tsan |
| ssh-transport | CapstanSSH Runner | kit-ios, phone-keys, phone-trust, G3 | XL | G4 | swift test + iOS build |
| ssh-tuning | CapstanSSH Tuning | ssh-transport | M | G4 | swift test + iOS build |

Parallel start: `ssh-spike` and `kit-ios` can run at the same time. `phone-keys` then `phone-trust`. `ssh-transport` after all of them.

---

## 6. Phase 2: the app (PM-382)

Phase 2 turns the base layer into an app. It starts when G4 is open (`ssh-transport` and `ssh-tuning` merged) for the packages that need a transport, but the pure-logic packages (`ios-core`, the Xcode shell) may start after `phone-trust` and `kit-ios` since they use `ProcessRunner` and fakes only. The ordering below is the recommended one.

Every UI package follows the Mac equivalents (named in "Mirrors") and the design board (named in "Board"). The cloud session reads both. The PM-382 plan from the controller is not available to cloud sessions; this section replaces it. Package boundaries and file ownership were chosen so that **no two packages edit the same file** (screens own their own folder; the app model owns its own files; `ios-shell` owns the project file).

### 6.1 Package `ios-core`

| | |
|---|---|
| Goal | The pure logic that the app model needs: hosts coordinator, lifecycle planner, settings. No SwiftUI. |
| Owns | `apps/ios/CapstanPhoneCore/Sources/CapstanPhoneCore/Coordinator/`, `.../Lifecycle/`, `.../Settings/` and the matching `Tests/CapstanPhoneCoreTests/` folders |
| Depends on | `phone-trust`, `phone-keys`, `kit-ios` (merged) |
| Size | XL (about 8 h). Split if it slips: Settings (S) can be its own PR. |
| Mirrors | `apps/macos/Capstan/App/AppModel.swift` (start, `startHost`, `teardownHost`, `restart`, `addHost`, `updateHost`, `removeHost`, `check`), `apps/macos/CapstanAppCore/.../Hosts/`, `.../System/NotificationPlan.swift`, `.../System/Authorization.swift`, `.../System/Sounds.swift` |

**Interfaces.**
- `HostsCoordinator` (protocol-based, `Sendable`): takes `any ProcessRunner`, `PhoneHostRegistry`, `HostKeyStore`, `PhoneHostStore`, `HostsFile`, a `SessionClock` and a `TransportControl` (all injected so tests use fakes). Responsibilities, copied from the Mac's `AppModel`: launch sequence (load `HostsFile` and `PhoneHostStore`, drop orphans on either side, register hosts, `sweep`, start sessions); per-host `HostSession` start/teardown with generations; `restart(hostId:)` (it implements `SessionControl`); `addHost` (new id from the registry, candidate, check, commit or discard), `updateHost`, `removeHost` (forget pins, delete the host); `check(_:)` as an `AsyncStream<HostCheck>` using `CapstanKit.Probe`; it is the **only consumer** of `TransportControl.events`, subscribed before any start, and exposes `PendingTrust` (the trust sheet), changed-key, auth-refused, unsupported and pins-unreadable states per host; `trust(hostId:)`, `resetAllPins()`.
- `LifecyclePlanner`: pure function from (scene phase transitions, settings, `FaceIDRule` inputs, now) to actions: `.lockUI`, `.promptFaceID`, `.startSessions`, `.stopSessionsThenSuspend`, `.noop`. `.background` stops sessions and then calls `suspend()`; `.active` after a long absence prompts Face ID when `FaceIDRule.required`; `.inactive` does nothing. Background refresh (Phase 3) calls the rule with `settingOn: true` and **never requires** it.
- Settings model `PhoneSettings` (Codable, versioned JSON in Application Support): Face ID on (default **true**), poll interval ms (default 2000, minimum 1000, validation like the Mac's, stepper steps), heartbeat pulse on, notification kinds on/off, sound per kind, Time Sensitive per kind, Live Activity allowed (default off until the user allows). Pure `SettingsValidation`.
- Notification planning: reuse `CapstanAppCore.NotificationPlan` where possible; add `PhoneNotificationPlan` for iOS-only rules (category per kind; body = kind + project name only, **never agent text**; deep link `capstan://item/<host>/<projectId>/<requestId>`, `.../agent/...`, `.../plan/...`, `.../conversation/<projectId>`). `DeepLink` parse/format with strict validation.

**Acceptance (tests, `swift test` on the Mac).**
- `CoordinatorTests`: launch drops a `HostConfig` without a `PhoneHost` and the reverse and saves cleaned lists; add host flow (candidate, check pass, commit; check fail, discard forgets pins); duplicate refused; remove host forgets pins; a `hostKeyUnknown` event produces a `PendingTrust` and no session start; trusting restarts that host; `hostKeyChanged` blocks restart; `pinsUnreadable` blocks all until `resetAllPins`; events delivered before subscription are still seen (`latestEvent`).
- `LifecyclePlannerTests`: cold start with setting on → prompt; 59 s away → no prompt; 61 s away → prompt; `.inactive` records nothing; background stops sessions then suspends (order); setting off → never prompt; background refresh path never prompts.
- `SettingsTests`: defaults (Face ID true, 2000 ms); 999 ms rejected with the inline error text; 1000 accepted; JSON round trip; unknown future fields ignored.
- `DeepLinkTests`: round trip; malformed and oversize components rejected; no host text in the URL other than ids.
- `NotificationPlanTests`: body contains kind and project only; an agent text string injected in fixtures never appears in any output.
- `check-shared-platform.py` passes.

**Mac steps and paste-back.** `swift test --package-path apps/ios/CapstanPhoneCore` and the `--sanitize=thread` variant. Paste both summaries.

### 6.2 Package `ios-shell`

| | |
|---|---|
| Goal | The Xcode project, the app entry point, the static checks for the app, the README. An app that launches to an empty tab bar. |
| Owns | `apps/ios/CapstanPhone.xcodeproj/`, `apps/ios/CapstanPhone/App/CapstanPhoneApp.swift`, `apps/ios/CapstanPhone/App/RootTabView.swift`, `apps/ios/CapstanPhone/Shared/Tokens.swift` and the shared view kit (`Banner`, `Buttons`, `ConfirmSheet`, `StatePill`, `UntrustedTextBlock`, `KindGroupHeader`, `MetaList`), `apps/ios/scripts/check-static.sh`, `apps/ios/scripts/check-xcodeproj.py`, `apps/ios/README.md` |
| Depends on | `ssh-transport` (merged) and `ios-core` |
| Size | XL (about 8 h) |
| Mirrors | `apps/macos/Capstan/Shared/*`, `apps/macos/scripts/*`, `apps/macos/README.md` |

**Interfaces.**
- Project `CapstanPhone.xcodeproj`: one iOS app target `CapstanPhone` (bundle id placeholder the user changes; Signing set by the user), shared scheme `CapstanPhone`, deployment target iOS 26.0, Swift 6, synchronized folder `CapstanPhone/`. Local packages added by relative path: `../macos/CapstanKit`, `../macos/CapstanAppCore`, `CapstanPhoneCore`, `CapstanSSH`; products linked to the app target. App project `Package.resolved` for swift-nio-ssh exact 0.15.0. Info.plist keys: `NSLocalNetworkUsageDescription` (hosts on the LAN), `NSFaceIDUsageDescription`, `UIBackgroundModes` = `fetch`, URL scheme `capstan`. No push entitlement, no `aps-environment` (D8).
- `CapstanPhoneApp`: creates the single `SSHRunner`, `HostsCoordinator`, the `AppModel` placeholder (the real model is `app-model`), injects them, handles `scenePhase` through `LifecyclePlanner`, calls `SSHRunner.shutdown()` on termination, owns the Face ID lock overlay (a full-screen opaque view; content hidden until unlocked).
- `RootTabView`: five tabs with the design's symbols (tray, person.2, list.bullet.rectangle, bubble.left, server.rack), each tab a `NavigationStack` with an empty placeholder root view that the screen packages replace. Tab bar minimises on scroll; badges bound to model counts.
- Shared views port the Mac's `Shared/` views to iOS idioms, with the iOS tokens (design record "Tokens"): colour roles `label label2 sep fill fill2 accent on-accent red yellow green blue glass` plus `ground`, `cell`, and the Increase Contrast variants; type styles; 4 pt spacing; concentric radii; haptics helper.
- `apps/ios/scripts/check-static.sh` (+ `--self-test`): copy the Mac script's rules and adapt paths to `apps/ios`: (1) imports in `CapstanPhoneCore` limited as in 2.2, `CapstanSSH` may import NIO; (2) no markup, rich-text or link detection (`AttributedString(markdown:`, `NSDataDetector`, `dataDetectorTypes`, `LocalizedStringKey` from variables) in any Swift file; (3) links open only from the allowed file (`DesignLinkConfirm.swift`); (4) no `try!` or `fatalError` in the packages; (5) no user-home path literals outside tests; (6) user-visible text is a plain literal or `verbatim:` (Rule 6 program); **new**: (7) no `aps-environment`, `registerForRemoteNotifications` or the word "push" in a control title outside the merge result text that says "not pushed"; (8) `ActivityKit`, `WidgetKit` and notification content code may not reference types that carry agent text (a deny-list of `CapstanKit` text-carrying types is in the script header, kept in step with `UntrustedText`). Reuse `apps/macos/scripts/stubs.txt` style if stubs are used.
- `apps/ios/scripts/check-xcodeproj.py` (+ `--self-test`): same mutation-test style as the Mac's: target and scheme exist, deployment target 26.0, local package references are the four paths, products linked, synchronized folder present, App Sandbox not applicable but `ENABLE_APP_SANDBOX` unset, Info.plist keys above present, no push entitlement.
- `apps/ios/README.md`: requirements (Xcode 26, an iPhone with iOS 26 for device checks), build/run steps (set Team and bundle id), test commands (section 8), static checks, "if the project does not open" recreate steps, "sending errors back" as in the Mac README.

**Acceptance.** Linux: both new scripts pass and their `--self-test`s pass; `check-shared-platform.py` passes; the Mac's checks still pass. Mac (the user): the app builds for the Simulator and launches to five empty tabs; the Face ID overlay appears on cold start in the Simulator (Features → Face ID → Enrolled → Matching Face).
```sh
xcodebuild -project apps/ios/CapstanPhone.xcodeproj -scheme CapstanPhone -destination 'generic/platform=iOS Simulator' build
xcodebuild -project apps/ios/CapstanPhone.xcodeproj -scheme CapstanPhone -destination 'platform=iOS Simulator,name=iPhone 17 Pro' build
```
Paste `BUILD SUCCEEDED` or the `error:` lines, plus a screenshot of the tab bar.

### 6.3 Package `device-checks`

| | |
|---|---|
| Goal | A small developer-only "Device checks" screen and a written procedure that settle what no unit test can: enclave signing, Local Network permission, a real SSH login, dead-link timing. |
| Owns | `apps/ios/CapstanPhone/Settings/DeviceChecks/` (screen, hidden behind a long-press on the version row in Settings), `apps/ios/docs/device-checks.md` |
| Depends on | `ios-shell`, `ssh-tuning` (merged) |
| Size | M (about 3 h) |

**Behaviour.** Each check shows pass/fail and the error text (host-text rules apply: stderr tails are shown as plain text). Checks: (1) create a Secure Enclave key, sign a test payload, show the public line; (2) Face ID unlock via `FaceIDGate`; (3) the Local Network prompt appears on the first connection to a LAN host and a denial gives a readable error; (4) an SSH login to a user-entered host with host-key trust sheet; (5) run `cstan events` for 10 s and show line count and the last cursor; (6) kill the host's network (the user turns off Wi-Fi) and time how long until the channel ends (expected about 60 s with keepalive, `HostSession`'s heartbeat check about 3 intervals); (7) background the app for 30 s and for 90 s and show whether Face ID was required (expected: no, yes).

**Acceptance.** Static checks pass. The user runs the procedure on a physical iPhone and pastes the table of results (check, expected, observed). A failed enclave signing check triggers the ed25519-default re-plan (section 10, R2).

### 6.4 Package `app-model`

| | |
|---|---|
| Goal | The observable `AppModel` that feeds every screen. |
| Owns | `apps/ios/CapstanPhone/App/AppModel.swift`, `AppModel+Overview.swift`, `AppModel+Decide.swift`, `AppModel+Hosts.swift`, `apps/ios/CapstanPhone/App/PreviewSeed.swift`, `apps/ios/CapstanPhone/Shared/PreviewData.swift` |
| Depends on | `ios-core`, `ios-shell` |
| Size | XL (about 8 h) |
| Mirrors | `apps/macos/Capstan/App/AppModel.swift` (765 lines) and `AppModel+Overview.swift`, split into smaller files |

**Interfaces.** `@MainActor @Observable final class AppModel` exposing exactly what the Mac model exposes to screens, minus Mac-only things (windows, menu bar, sidebar selection), plus iPhone state: `inbox()`, `displayGroups()`, `inboxTotal()`, `item(_:)`, `detail(_:)`, `project(_:_:)`, `availability(_:row:option:)`, `decide(_:on:)`, `decideAgent`, `decideRun`, `retry(_:)`, `refresh(hostId:projectIds:)`, `restart(hostId:)`, `removeProject`, `addHost`, `updateHost`, `removeHost`, `check(_:)`, connection state per host (`ConnectionPresentation`), `PendingTrust`/transport problems per host from `HostsCoordinator`, `PhoneSettings`, key state (`KeyVault.current`), `navigation` (selected tab, one path per tab, presented sheet) and `open(deepLink:)`. Persistence of the last snapshot for the "as of" display uses `CapstanKit` `Persistence`. The Face ID lock state is the app's, kept by the scene-phase handler from `ios-shell`.

**Acceptance.** Pure parts are already tested in `CapstanAppCore`/`CapstanPhoneCore`; this package adds `apps/ios/CapstanPhoneCore`-side tests only if logic is added (put new pure logic in `CapstanPhoneCore`, not in the app, so it is testable). `PreviewSeed` gives fixtures for every board state so each screen package can use `#Preview`. Static checks pass. Mac: app builds; launches with the seed in the Simulator and shows tab placeholders bound to real counts.

### 6.5 Screen packages

Each screen package: **Owns** only its own folder under `apps/ios/CapstanPhone/` (listed), **Depends on** `app-model` and `ios-shell` (merged), takes pure presentation logic from `CapstanAppCore` where the Mac already has it (named under "Reuse"), and ships `#Preview`s for every state on its board (light, dark, AX3 where the board shows it). Common acceptance for all screen packages:

- Static checks pass (`check-static.sh` rule 6: plain-literal text; no agent text in rich form).
- Mac: builds for the Simulator; the user opens each `#Preview` state and compares to the board PNG (named per package); pastes `BUILD SUCCEEDED` and, for the board states, screenshots or "matches / differs because ...".
- Controls are at least 44 × 44 pt; all state is also a word or glyph, never colour alone; text styles only (no fixed point sizes); VoiceOver labels per the record.
- One primary action per screen in the bottom action bar; destructive actions confirm in a sheet with Cancel as default and initial focus.

| Package | Board | Owns | Reuse from `CapstanAppCore` / Mac | Size |
|---|---|---|---|---|
| `scr-onboarding` | 01 | `CapstanPhone/Onboarding/` | `Hosts/HostText`, `CheckStepRow`, `Probe` checks | XL |
| `scr-waiting` | 02, 06 | `CapstanPhone/Waiting/` | `Inbox/RowText`, `RowPills`, `Shell/Types`, `MenuList` rules | XL |
| `scr-detail` | 03, 04 | `CapstanPhone/Detail/` | `Inbox/InspectorMode`, `OptionRows`, `LinkCheck`, `IntegrationDetailRows` | XL |
| `scr-outcomes` | 05 | `CapstanPhone/Outcomes/` | `Shell/OutcomePresentation`, `Decide` | M |
| `scr-agents` | 07 | `CapstanPhone/Agents/` | `Workspaces/AgentRow`, `PauseControl` | L |
| `scr-plans` | 08 | `CapstanPhone/Plans/` | `Workspaces/PlanRow`, `PackageRow` | M |
| `scr-overview` | 09 | `CapstanPhone/Overview/` | `Workspaces/Overview`, `Store/OverviewController`, `RecentHistory` | L |
| `scr-hosts` | 12, 13 | `CapstanPhone/Hosts/` | `Hosts/ConnectionPresentation`, `HostText`, `ProjectRemovalPrompt`, `SilentDebounce` | XL |
| `scr-settings` | 14 | `CapstanPhone/Settings/` (not `DeviceChecks/`) | `System/Sounds`, `Authorization` | L |
| `scr-messages` | 11 | `CapstanPhone/Messages/` | Mac `MessagesScreen` logic | L, **blocked** (section 10, R3) |
| `scr-merge` | 10 | `CapstanPhone/Merge/` | `IntegrationDetailRows`, Mac merge inspector design (`docs/design/plan-137-merge-inspector.md`) | L, **blocked** (R3) |
| `scr-adapt` | 16, 17 | edits across screens' `Layout` helpers only: `CapstanPhone/Shared/Adaptive.swift` plus per-screen `+Adaptive.swift` files | n/a | L |

Order: `scr-waiting` and `scr-detail` first (the core loop), then `scr-outcomes`, `scr-hosts`, `scr-onboarding`, `scr-settings`, `scr-agents`, `scr-plans`, `scr-overview`, then `scr-messages` and `scr-merge` when unblocked, then `scr-adapt` last. `scr-waiting`, `scr-agents`, `scr-plans`, `scr-overview`, `scr-settings` can run in parallel once `app-model` is merged because they own disjoint folders; `scr-detail` and `scr-outcomes` coordinate through `app-model`'s `DecideOutcome`.

#### `scr-onboarding` (board 01)

Welcome (left-aligned, says what the app does and how it connects); "This iPhone's key" with the Secure Enclave key (public line, Copy and Share, the `authorized_keys` hint, Face ID on by default); "Use an ed25519 key instead…" opens the fallback sheet with the backup warning and **Keep the Secure Enclave key as the focused default**; Add a host form (name, hostname, port, user, cstan path, extra roots optional) with `HostConfig.validate` errors inline; host-key trust sheet (SHA256 fingerprint, the command to compare it: `ssh-keygen -lf /etc/ssh/ssh_host_<type>_key.pub`, Trust / Cancel with Cancel default); checks list (SSH, host key, Capstan version and protocol, projects found, first snapshot) from `HostCheck`; host key changed (blocked, no "trust anyway"; only Remove host and add again after comparing). Enclave unavailable (Simulator) shows an error, never a silent ed25519. A restored phone with no key shows "This iPhone needs a new key". Acceptance adds: Simulator walk-through with a test SSH host or the `CapstanSSH` test server running on the Mac (`127.0.0.1`), pasted as a short step list with results.

#### `scr-waiting` (boards 02, 06)

Large-title grouped list by kind with counts, the live status line, filter chips (All · Proposals · Prompts · Designs · Messages · Merges), swipe left (Deny… for proposals with full swipe off, Message PM), long-press preview and context menu (Open, Copy command, Message PM about this, Open project, Deny… last and destructive), forwarded pill, terminal-only pill, widening flag; states empty, first snapshot (skeleton rows shaped like real rows), reconnecting with items "as of 12:31" that cannot be answered, nothing reachable. Toolbar: Message PM. Landscape on Pro Max: `NavigationSplitView` (finished in `scr-adapt`). Approve is never a swipe or a menu item.

#### `scr-detail` (boards 03, 04)

Item details for proposal, prompt, design, message and merge-ready items: the untrusted block with header, the integrity-failed state and the "cut at 4096 bytes" state, exact option rows (widening flagged and terminal-only; text options terminal-only; all-terminal prompt), the bottom action bar, Confirm deny sheet at `.medium` (note field, Cancel default), Confirm a forward small-detent sheet, design link shown as text with "Open in Safari…" behind `DesignLinkConfirm` (open question 4 in the record decides whether the full URL shows first; **default: show it**), the PM message reply composer with disabled send. The Merge… button appears only when `scr-merge` exists (it navigates to that sheet); until then the button is absent.

#### `scr-outcomes` (board 05)

Sending state; forwarded toast with success haptic; "changed since opened" (`hash_mismatch`, warning haptic); every result from `OutcomePresentation`: applied, forwarded, `needs_terminal`, `not_waiting`, repeat, `no_active_pm`, `limit_reached`, exit 255 (error haptic). Pure text comes from `CapstanAppCore`; this package only draws it and fires haptics.

#### `scr-agents` (board 07)

Segmented filter; rows with health words and glyphs (Working, Needs you, Stalled, Lost, Blocked, Paused, Idle) and ages; detail with branch, commits ahead, last commit; Pause sheet at `.medium` (Cancel default, optional reason ≤ the protocol limit); paused state with Resume disabled when the project is paused (reason shown).

#### `scr-plans` (board 08)

Progress bars with a text equivalent; plan detail with packages in integration order, branch, review (passed, changes requested with round), verify, dependencies; not-started state.

#### `scr-overview` (board 09)

Quiet project (Moving, nothing waits, agents now, recent outcomes in host time) and busy project; the "…" pull-down menu (Message PM, Plans, Agents, Refresh, Pause project… destructive). Reached from Hosts.

#### `scr-hosts` (boards 12, 13)

Per host live/reconnecting; projects in every state (Ready, No key, Silent, Refused, Unsupported, Gone) as words with glyphs and the fix text; host detail with Remove host…; connection status lines (connecting, live, no heartbeat after 3 intervals, reconnecting with backoff, resync, stopped); reconnecting banner over stale items; unknown protocol version; SSH failure with exit codes 0, 1, 2, 255; transport problems from `HostsCoordinator`: host key changed (with the Remove-and-re-add recovery text), auth refused (show this phone's public line to add to `authorized_keys`), unsupported algorithm (the reason), pins unreadable (Reset all pins…). Toolbar: Add a host (opens onboarding's add flow), Settings. The `no_key` fix text: see R4.

#### `scr-settings` (board 14)

Hosts, key (public line, fingerprint, Replace…), Face ID switch (on by default), poll interval stepper with the 1000 ms minimum shown as an inline error and clamp, heartbeat pulse, notification kinds, a sound per kind with preview (system alert tones and tones the user added), Time Sensitive per kind. Settings → Notifications → sound per kind. **No push control.** A version row whose long press opens `DeviceChecks` (owned by `device-checks`; this package only links to it by a navigation destination `DeviceChecksScreen()` that `device-checks` defines; if `device-checks` is not merged yet, leave the row without the long press).

#### `scr-messages` (board 11) — blocked until R3

Conversations per project with unread badge; thread with Delivered, Sent (on the host, not picked up), Not sent with Retry; composer with a 500-byte counter; New message sheet at `.large` with the project picker. Uses `decide ... message reply`/the message-to-PM verb once the protocol defines it. Until then the package may build the screen against a fake `Decider` (previews and tests only) but the Messages tab shows an honest "not available yet" state and the toolbar Message PM entries are hidden.

#### `scr-merge` (board 10) — blocked until R3

Sheet at `.large`: branch, target, commit, review, verify; results: merged (not pushed), conflict with files named, refused (dirty tree, not reviewed on this commit), merged but not confirmed, failed. **No Push switch, no "Push main…" action, no "merged but push failed" result (D8).** Default focus on Cancel.

#### `scr-adapt` (boards 16, 17)

iPhone SE (375 × 667, no island), Pro Max (440 × 956), Pro Max landscape split view (`NavigationSplitView`); Dynamic Type through AX5 (rows stack via `dynamicTypeSize.isAccessibilitySize`, action bars stack with a solid background, tab labels give way to the large-content viewer); Increase Contrast and Reduce Transparency (glass becomes opaque, secondary text and accent switch to the Increase Contrast tokens); Reduce Motion (the heartbeat pulse stops; sheets cross-fade); VoiceOver labels and custom actions (Deny, Message PM); Full Keyboard Access order (back → More actions → options → secondary → primary) and the same shortcuts on a hardware keyboard. Acceptance: the user pastes screenshots of boards 16 and 17 states from the Simulator (SE, Pro Max, landscape, AX3, Increase Contrast) and an Accessibility Inspector pass.

### 6.6 Phase 2 summary table

| Package | Depends on | Size |
|---|---|---|
| ios-core | phone-trust, phone-keys, kit-ios | XL |
| ios-shell | ssh-transport, ios-core | XL |
| device-checks | ios-shell, ssh-tuning | M |
| app-model | ios-core, ios-shell | XL |
| scr-waiting | app-model | XL |
| scr-detail | app-model | XL |
| scr-outcomes | app-model | M |
| scr-hosts | app-model | XL |
| scr-onboarding | app-model | XL |
| scr-settings | app-model | L |
| scr-agents | app-model | L |
| scr-plans | app-model | M |
| scr-overview | app-model | L |
| scr-messages | app-model, protocol work (R3) | L |
| scr-merge | app-model, protocol work (R3) | L |
| scr-adapt | all other screens | L |

Size key: S ≤ 2 h, M 2 to 4 h, L 4 to 6 h, XL 6 to 8 h of a cloud session (rough; the user's Mac loop adds time). An XL package is already at the limit of one session: split it at the first natural boundary and say so in the PR.

---

## 7. Phase 3: glanceable surfaces and release (PM-380)

Phase 3 starts after the core loop screens (`scr-waiting`, `scr-detail`, `scr-outcomes`, `scr-hosts`, `scr-onboarding`, `scr-settings`) are merged.

### 7.1 Package `snapshot-store`

| | |
|---|---|
| Goal | The small, agent-text-free snapshot that widgets and the Live Activity read, plus the "as of" logic. |
| Owns | `apps/ios/CapstanPhoneCore/Sources/CapstanPhoneCore/Snapshot/`, `.../Tests/.../Snapshot/` |
| Depends on | `ios-core` |
| Size | M |

**Interfaces.** `GlanceSnapshot {asOf: Date, totalWaiting: Int, byKind: [WaitingKind: Int], projects: [GlanceProject {name (the project display name, treated as host text: length-capped, control characters stripped), count}], hostsReachable: Int, hostsTotal: Int}`; `GlanceWriter` (writes JSON atomically to the App Group container) and `GlanceReader`; `GlanceStaleness.label(asOf:now:)` ("as of 12:31" when older than 2 poll intervals + grace, otherwise live). **Contains no summary, command, reason, message body or option label** (a test enforces the type has no such field and that fixtures with hostile text produce a snapshot with none of it).
**Acceptance.** `GlanceSnapshotTests`: round trip; hostile text never present; stale label boundaries; atomic write survives a partial file (reader falls back to nil). **Mac:** `swift test --package-path apps/ios/CapstanPhoneCore`.

### 7.2 Package `live-activity`

| | |
|---|---|
| Goal | The "N waiting" Live Activity and Dynamic Island. |
| Owns | `apps/ios/CapstanWidgets/LiveActivity/`, `apps/ios/CapstanPhone/System/LiveActivityController.swift`, `ActivityAttributes` type in `apps/ios/CapstanPhoneCore/.../Snapshot/` only if shared (otherwise duplicated in the extension) |
| Depends on | `snapshot-store`, `ios-shell`; adds the widget extension target (so this package owns the project edit for the extension; `check-xcodeproj.py` is extended in the same PR) |
| Size | L |

**Behaviour.** Starts when the first item waits and the user allowed it in Settings; shows count and kind breakdown (kinds and counts only); ends when nothing waits; "as of" time shown when stale; updated by the app while it runs and by background refresh. Lock Screen layout and Dynamic Island expanded, compact and minimal per board 15. **No agent text, no project text beyond the project name** (and the name is optional in the island). No push token is requested (D8): `pushType: nil`.
**Acceptance.** Static checks (rule 8 forbids text-carrying types in the extension). Mac: build the app and extension for the Simulator; on a device or Simulator start/stop the activity from the Device checks screen and paste screenshots of Lock Screen and island states.
```sh
xcodebuild -project apps/ios/CapstanPhone.xcodeproj -scheme CapstanPhone -destination 'generic/platform=iOS Simulator' build
```

### 7.3 Package `widgets`

| | |
|---|---|
| Goal | Small (count) and medium (by kind with project names) widgets with an "as of" note when stale. |
| Owns | `apps/ios/CapstanWidgets/Widgets/` |
| Depends on | `snapshot-store`, `live-activity` (shares the extension target) |
| Size | M |

Empty ("nothing waits") and stale states per board 15; tapping opens the app on Waiting (deep link `capstan://item/...` only to the Waiting tab, never an approve/answer action: no interactive widget buttons). Timeline reload policy: after the app writes a snapshot (`WidgetCenter.reloadAllTimelines()`), plus a 15-minute fallback. **Acceptance:** previews for each state; user pastes screenshots of the widget gallery and the Home Screen.

### 7.4 Package `background-refresh`

| | |
|---|---|
| Goal | Read-only background refresh: brief reconnect to update the badge, the widget, the Live Activity, and local notifications. |
| Owns | `apps/ios/CapstanPhone/System/BackgroundRefresh.swift`, `apps/ios/CapstanPhoneCore/Sources/CapstanPhoneCore/Refresh/`, tests |
| Depends on | `snapshot-store`, `live-activity`, `ios-core`, `ssh-tuning` |
| Size | L |

**Interfaces.** A pure `RefreshPlanner` decides per task run: which hosts, a time budget (BGAppRefreshTask gives roughly 30 s), serial or limited-parallel fetch of one snapshot per host via `cstan events` (read the snapshot, then stop), the notifications to post (new waiting items since the last run, by kind and project name only; deduplicated by request id; never agent text), and the snapshot to write. Registration via `BGTaskScheduler` (identifier in Info.plist `BGTaskSchedulerPermittedIdentifiers`). **Never prompts Face ID, never calls `Decider`, never sends anything to a host except the read-only stream.** Uses the shared `SSHRunner` and never calls `suspend()` while a refresh task runs. It does not need the Secure Enclave key to be unlocked by biometry (the key has no biometry flag, which is why this is possible).
**Acceptance.** `RefreshPlannerTests`: budget respected; dedup; hostile text never in a planned notification; Face ID never requested (the planner has no `BiometricGate` dependency). Device check (user): simulate with Xcode's "Simulate Background Fetch" and `e -l objc -- (void)[[BGTaskScheduler sharedScheduler] _simulateLaunchForTaskWithIdentifier:@"<id>"]`, and observe a 24-hour real-world run; paste what updated. iOS decides when refresh runs; "less often than the Mac" is expected and documented (R5).

### 7.5 Package `device-pass`

| | |
|---|---|
| Goal | A full on-device test pass and fixes found by it. |
| Owns | `apps/ios/docs/device-pass.md` (the results) and fix commits in files the failing screen package owns, listed in the PR (this is the one package allowed to touch other packages' files, per failing item, in separate commits named `fix(<package>): ...`) |
| Depends on | everything except `scr-messages` and `scr-merge` if still blocked |
| Size | L |

The user runs the Mac test checklist for Phase 3 (section 8) on a physical iPhone and pastes the table; the session fixes failures.

### 7.6 Package `testflight-ready`

| | |
|---|---|
| Goal | Everything needed to archive and upload a TestFlight build, without doing it. |
| Owns | `apps/ios/docs/release.md`, `apps/ios/CapstanPhone/Assets.xcassets` (app icon placeholder from `docs/assets/` brand files), `PrivacyInfo.xcprivacy`, Info.plist final keys |
| Depends on | `device-pass` |
| Size | M |

**Contents.** Privacy manifest (required-reason API declarations: UserDefaults, file timestamp, if used); export compliance answer (the app uses SSH over standard algorithms: `ITSAppUsesNonExemptEncryption` per the user's decision, flagged in R6); signing steps (Team, bundle id, App Group, capabilities: Keychain, App Groups, Live Activities (`NSSupportsLiveActivities`), Background Modes fetch, Time Sensitive Notifications if the user wants it); archive and upload commands (`xcodebuild archive`, `xcodebuild -exportArchive`, or Xcode Organizer); TestFlight test information text; a pre-upload checklist (no debug seeds in Release, no `PreviewSeed` reachable, version and build numbers, the static checks green).
**Acceptance.** The user archives once and pastes `** ARCHIVE SUCCEEDED **`.
```sh
xcodebuild -project apps/ios/CapstanPhone.xcodeproj -scheme CapstanPhone -configuration Release -destination 'generic/platform=iOS' -archivePath build/CapstanPhone.xcarchive archive
```

### 7.7 Phase 3 summary table

| Package | Depends on | Size |
|---|---|---|
| snapshot-store | ios-core | M |
| live-activity | snapshot-store, ios-shell | L |
| widgets | snapshot-store, live-activity | M |
| background-refresh | snapshot-store, live-activity, ios-core, ssh-tuning | L |
| device-pass | all above | L |
| testflight-ready | device-pass | M |

---

## 8. Mac test checklist per phase

Run from the repository root on the Mac (Xcode 26.6). First: `xcode-select -p` must print an Xcode path, not `/Library/Developer/CommandLineTools` (otherwise `sudo xcode-select -s /Applications/Xcode.app/Contents/Developer`, or prefix commands with `DEVELOPER_DIR=/Applications/Xcode.app/Contents/Developer`). Pull the package branch first (`git fetch && git checkout feat/ios-<package>`). Paste back the summary lines, or the lines that contain `error:`, and the Xcode version if a build fails. Add `2>&1 | tail -60` to keep output short.

### 8.1 Phase 1

| After package | Commands | Expect |
|---|---|---|
| ssh-spike | `swift test --package-path apps/ios/Spikes/NIOSSHSpike` | per-item pass/fail/skip table (enclave item may skip) |
| kit-ios | `swift test --package-path apps/macos/CapstanKit` · `swift test --package-path apps/macos/CapstanAppCore` · `xcodebuild -project apps/macos/Capstan.xcodeproj -scheme Capstan -configuration Debug -destination 'platform=macOS' build` · `cd apps/macos/CapstanKit && xcodebuild -scheme CapstanKit -destination 'generic/platform=iOS Simulator' build` · `cd apps/macos/CapstanAppCore && xcodebuild -scheme CapstanAppCore -destination 'generic/platform=iOS Simulator' build` | all green; Mac tests unchanged in count except the new ones |
| phone-keys | `swift test --package-path apps/ios/CapstanPhoneCore` | green |
| phone-trust | `swift test --package-path apps/ios/CapstanPhoneCore` · `swift test --sanitize=thread --package-path apps/ios/CapstanPhoneCore` | green, no TSan reports |
| ssh-transport | `swift test --package-path apps/ios/CapstanSSH` · `cd apps/ios/CapstanSSH && xcodebuild -scheme CapstanSSH -destination 'generic/platform=iOS Simulator' build` | green |
| ssh-tuning | same two as ssh-transport | green |

Phase 1 exit: every package green, `G2`, `G3`, `G4` recorded in the progress table.

### 8.2 Phase 2

| After package | Commands | Expect |
|---|---|---|
| ios-core | `swift test --package-path apps/ios/CapstanPhoneCore` and `--sanitize=thread` | green |
| ios-shell | `xcodebuild -project apps/ios/CapstanPhone.xcodeproj -scheme CapstanPhone -destination 'generic/platform=iOS Simulator' build` · `python3 apps/ios/scripts/check-xcodeproj.py` · `apps/ios/scripts/check-static.sh` | builds; five empty tabs in the Simulator; Face ID overlay on cold start |
| device-checks | build as above, then on a **physical iPhone** (Xcode → Run, set Team and bundle id): the seven checks in 6.3 | table of expected/observed |
| app-model | build as above; run in the Simulator with the preview seed | tabs show real counts |
| each screen package | build as above; open each `#Preview` state in Xcode canvas; compare with the board PNG named in the package | states match or differences listed |
| scr-onboarding (extra) | run the `CapstanSSH` test server or a real host; walk Welcome → key → Add host → Trust → Checks in the Simulator (the Simulator has no enclave, so expect the "enclave unavailable" error unless on a device) | step list |
| scr-adapt | Simulator on iPhone SE (3rd gen), iPhone 17 Pro Max portrait and landscape; Settings → Accessibility → Larger Text (AX3, AX5), Increase Contrast, Reduce Transparency, Reduce Motion; Accessibility Inspector audit | screenshots |

Phase 2 exit: the user can add a real host on a device, trust its key, see the inbox, forward a proposal approval and see it arrive at the PM, deny a proposal, and Face ID behaves per `FaceIDRule`.

### 8.3 Phase 3

| After package | Commands / steps | Expect |
|---|---|---|
| snapshot-store | `swift test --package-path apps/ios/CapstanPhoneCore` | green |
| live-activity | build app + extension; device: Settings → allow Live Activity; make an item wait on a host | Lock Screen and island match board 15; ends when nothing waits |
| widgets | build; add small and medium widgets on the Home Screen; make the host unreachable | count with "as of" when stale |
| background-refresh | `swift test`; device: lock the phone for several hours; Xcode Debug → Simulate Background Fetch | widget and badge updated, no Face ID prompt, no agent text in any notification |
| device-pass | the whole checklist in `apps/ios/docs/device-pass.md` | table of results |
| testflight-ready | archive command in 7.6 | `ARCHIVE SUCCEEDED` |

Device-pass items: cold start prompts Face ID; 59 s away no prompt, 61 s away prompt; setting off never prompts; dead link detected within about a minute (keepalive) or 3 heartbeat intervals; airplane-mode and Wi-Fi to cellular transitions reconnect; a host key change blocks; a revoked key shows `authRefused` with the public line; restore to a new phone shows "needs a new key"; a hostile agent summary (host test fixture with markdown, a link, control characters, 4096-byte text) renders as plain text, never a link, never in a notification, Live Activity or widget.

---

## 9. Progress table

Fill in as work moves. Status: `todo` · `in progress` · `PR open` · `Mac green` · `merged` · `blocked`.

| Phase | Package | Branch | PR | Status | Gate / notes |
|---|---|---|---|---|---|
| 1 | design-record | (merged with this plan) | | merged | G1 |
| 1 | ssh-spike | feat/ios-ssh-spike | | todo | G3 |
| 1 | kit-ios | feat/ios-kit-ios | | todo | G2 |
| 1 | phone-keys | feat/ios-phone-keys | | todo | G2 |
| 1 | phone-trust | feat/ios-phone-trust | | todo | G2 |
| 1 | ssh-transport | feat/ios-ssh-transport | | todo | G4 |
| 1 | ssh-tuning | feat/ios-ssh-tuning | | todo | G4 |
| 2 | ios-core | feat/ios-ios-core | | todo | |
| 2 | ios-shell | feat/ios-ios-shell | | todo | |
| 2 | device-checks | feat/ios-device-checks | | todo | |
| 2 | app-model | feat/ios-app-model | | todo | |
| 2 | scr-waiting | feat/ios-scr-waiting | | todo | |
| 2 | scr-detail | feat/ios-scr-detail | | todo | |
| 2 | scr-outcomes | feat/ios-scr-outcomes | | todo | |
| 2 | scr-hosts | feat/ios-scr-hosts | | todo | |
| 2 | scr-onboarding | feat/ios-scr-onboarding | | todo | |
| 2 | scr-settings | feat/ios-scr-settings | | todo | |
| 2 | scr-agents | feat/ios-scr-agents | | todo | |
| 2 | scr-plans | feat/ios-scr-plans | | todo | |
| 2 | scr-overview | feat/ios-scr-overview | | todo | |
| 2 | scr-messages | feat/ios-scr-messages | | blocked | R3 protocol |
| 2 | scr-merge | feat/ios-scr-merge | | blocked | R3 protocol |
| 2 | scr-adapt | feat/ios-scr-adapt | | todo | |
| 3 | snapshot-store | feat/ios-snapshot-store | | todo | |
| 3 | live-activity | feat/ios-live-activity | | todo | |
| 3 | widgets | feat/ios-widgets | | todo | |
| 3 | background-refresh | feat/ios-background-refresh | | todo | |
| 3 | device-pass | feat/ios-device-pass | | todo | |
| 3 | testflight-ready | feat/ios-testflight-ready | | todo | |

Gate log (date, who, result): G1 ____ · G2 (kit-ios) ____ · G2 (phone-keys) ____ · G2 (phone-trust) ____ · G3 ____ · G4 ____.

---

## 10. Open risks and questions

| # | Risk or question | Effect | Proposed handling |
|---|---|---|---|
| R1 | The design record shows a Push switch (off by default) and "Push main…" / "merged but push failed" on board 10, and the Mac pushes by default. Decision D8 says no push control anywhere in the first release. | `scr-merge` differs from the board | `scr-merge` omits those controls; the record is not edited (the plan supersedes it). A later release adds an explicit `--push` flag in the protocol and the switch. |
| R2 | Secure Enclave signing through swift-nio-ssh may fail (`ssh-spike` item 1; also device check 1). | The default key kind changes | Re-plan with ed25519 default (key screen, Settings copy and onboarding change; `SSHRunner` does not). The enclave item may be skipped under `swift test`; the device check decides. |
| R3 | The merge verb (with an explicit `--push` flag) and the message-to-PM verb with Sent/Delivered are **not in protocol v1** (`docs/remote-protocol.md` section 11 lists only op, prompt, design, message reply, agent, run verbs; the Mac's merge/message screens rest on work not in the Kit's `DecideTypes`). Branch on agents, per-package review and verify are also listed by the record as protocol work. | `scr-messages`, `scr-merge` and parts of `scr-agents`/`scr-plans` cannot be real yet | Protocol work is a Capstan (Rust) task outside this plan. Until it lands, build against fakes behind the existing `Decider`/`CapstanKit` types and show honest "not available" states; check what `CapstanKit` and `CapstanAppCore` already provide before each screen (the Mac app has Agents, Plans and Messages screens, so the parts the Mac uses are available). |
| R4 | `no_key` fix text: the design does not name the host command that creates a project's operator key (record open question 3). | `scr-hosts` fix text | Show the project root and the state word, and the instruction "create the operator key for this project on the host"; the user supplies the command later. Do not invent a command. |
| R5 | Background following: iOS suspends SSH in the background; background refresh runs when iOS decides (record open question 2). | Notifications arrive late or not at all when the app is closed | Documented in Settings footnote and the device pass; read-only refresh; "as of" time everywhere. Acceptable for release; revisit if the user wants server push (needs a server, which is out of scope, D2/D8). |
| R6 | Export compliance and App Store review of an SSH client. | TestFlight upload questions | Settled in `testflight-ready` with the user. |
| R7 | swift-nio-ssh 0.15.0 was read in source on 2026-10-30, not built. | `ssh-spike` may contradict assumptions | G3 gates `ssh-transport`; outcomes table in 5.3. |
| R8 | Tools 6.2 for the two Mac packages changes how `Capstan.xcodeproj` resolves them. | Mac app build could fail at `kit-ios` | Fixed inside `kit-ios` before merge (G2 includes the Mac app build). Any other branch touching either `Package.swift` rebases on `kit-ios`. |
| R9 | Local Network permission on iOS 26 for hosts on the LAN; the first connection triggers the prompt. | A denied prompt looks like a timeout | `scr-hosts`/`scr-onboarding` show a readable message; device check 3. |
| R10 | Package size: XL packages (`ssh-transport`, `ios-core`, `ios-shell`, `app-model`, several screens) may exceed one session. | Sessions run out mid-package | Split at the named points and say so in the PR; the progress table gets extra rows. |
| R11 | A Linux session cannot compile Swift. | Compile errors found only on the Mac | The loop in section 4; keep packages small; prefer plain code over clever generics; the user pastes errors. |
| R12 | Open question 4 (open a design link): show the full URL in a confirmation first? | `scr-detail` | Default: yes, show it. Change only if the user says otherwise. |

Questions for the user (answer before the named package starts):
1. `ssh-spike`: any objection to the exact version pin 0.15.0 and to the tests binding only `127.0.0.1`?
2. `ios-shell`: bundle identifier, Team and App Group id to use (placeholders are fine until `testflight-ready`).
3. `scr-merge`: confirm D8 (no push anywhere in the first release).
4. `scr-hosts`: the host command for creating a project operator key (R4), if known.

---

## 11. Appendix: facts the sessions need

### 11.1 Sources to read (in the repository)

| Topic | Read |
|---|---|
| Protocol | `docs/remote-protocol.md` (sections 1 to 11: transport, line kinds, cursor, snapshot, items, errors, untrusted text, limits, decisions) |
| Design | `docs/design/iphone-app.md`, boards in `docs/design/iphone-app/screens/` (01 to 17, light row above dark row), `docs/design/iphone-app/directions/a-glass-inbox.png` |
| Mac design | `docs/design/mac-app.md`, `docs/design/project-overview.md`, `docs/design/plan-137-merge-inspector.md` |
| Shared code | `apps/macos/CapstanKit/Sources/CapstanKit/{Protocol,Store,Stream,Actions}`, `apps/macos/CapstanAppCore/Sources/CapstanAppCore/{Hosts,Inbox,Shell,System,Workspaces}` |
| Mac app | `apps/macos/Capstan/{App,Hosts,Inbox,Workspaces,System,Shared,Connection}`, `apps/macos/README.md` |
| Static checks | `apps/macos/scripts/check-static.sh`, `check-xcodeproj.py`, `stubs.txt` |

Design board index: 01 onboarding and add host · 02 waiting for you · 03 item details · 04 permission prompt and confirm · 05 decision outcomes · 06 inbox states · 07 agents · 08 plans · 09 project overview · 10 merge · 11 messages to PM · 12 hosts and projects · 13 connection states · 14 settings and sounds · 15 notifications, Live Activity, widgets · 16 sizes and landscape · 17 accessibility.

### 11.2 Key types a session will meet

| Type | Where | Role |
|---|---|---|
| `ProcessRunner`, `RunningProcess` | `CapstanKit/Stream/ProcessRunner.swift` | the seam: `start(_ executable: URL, _ arguments: [String])`; lines, `stdoutAll`, `stderrTail`, `terminate(grace:)`, `exitCode()` |
| `RemoteCommand`, `SSHInvocation` | `CapstanKit/Stream/RemoteCommand.swift` | builds and (after `kit-ios`) parses ssh argv |
| `HostConfig {id, displayName, destination, cstanPath, extraRoots, pollMs, identityFile, enabled}` | `CapstanKit/Stream/HostConfig.swift` | a saved host; `validate()` |
| `HostSession`, `Probe` | `CapstanKit/Stream/` | the event stream session and the host check |
| `Decider`, `DecideVerb`, `DecideOutcome` | `CapstanKit/Actions/` | `cstan decide` calls, `PendingStore` for retries |
| `HostStoreEngine`, `Reducer`, `Inbox`, `Health`, `Freshness` | `CapstanKit/Store/` | state, inbox grouping, health words, "as of" |
| `UntrustedText`, `Limits` | `CapstanKit/Protocol/` | text integrity and the byte limits |
| `OutcomePresentation`, `RowText`, `OptionRows`, `ConnectionPresentation`, `AgentRow`, `PlanRow`, `Overview` | `CapstanAppCore` | the Mac's presentation logic, reused unchanged |

### 11.3 Design tokens and forbidden moves (keep exactly)

Colour roles from the Mac, unchanged: `label label2 sep fill fill2 accent on-accent red yellow green blue glass`. Brand: rope amber `#e0a45e` (dark) and `#8a521c` (light, text-safe). Added for iOS:

| token | light | dark | role |
|---|---|---|---|
| ground | `#efefec` | `#101214` | grouped background |
| cell | `#ffffff` | `#1b1d20` | grouped cells, sheets with untrusted text, widgets |
| accent (Increase Contrast) | `#6e3f12` | `#f0bb7c` | accent under Increase Contrast |
| label2 (Increase Contrast) | `#3a4047` | `#c9ced3` | secondary text under Increase Contrast |

Type: large title 34/700, title2 22/700, headline 17/600, body 17/400, subheadline 15, footnote 13, mono 15 (13 in notes); SF Pro and SF Mono; tabular figures for counts and ages. Spacing: 4 pt scale (4, 8, 12, 16, 20, 24, 32); margins 16 pt (20 on Pro Max). Radii: display 55 (62 Pro Max), floating sheet 47 at an 8 pt inset, large sheet 38, grouped sections and option rows 22, untrusted blocks and fields 12, small controls 8, capsules at half their height (buttons 50 tall, chips 36, tab bar 62). Motion: 150 ms press, 250 ms state change, one 200 ms heartbeat pulse, system sheet and push animations. Depth: glass floats (nav bar buttons, tab bar, action bar buttons, menus, notifications, Live Activity); content is flat with tonal cells.

Liquid Glass rules: glass only on the floating layer; content scrolls under the nav bar and tab bar; shapes concentric with the display; one accent (rope amber) for the primary action, the selected chip and the active tab only; native controls and materials (`.glassEffect`, `Material`, `presentationBackground`), not custom blur; Reduce Transparency turns every glass surface opaque.

**Forbidden moves.** No glass on content rows or untrusted text. No second accent; state colours never fill a primary action. Agent text never in a notification, Live Activity, widget, link or rich text. Approve or answer never by swipe, notification action or widget. No gradient except the system material. No state by colour alone: every pill, dot and row state has a word or a glyph. **Added by this plan:** no push control anywhere (D8).

HIG items the sessions must follow: one primary action per screen in a bottom action bar; destructive actions always confirm in a sheet with Cancel as the default (`.keyboardShortcut(.cancelAction)` and initial focus); sheets that show untrusted text use an opaque `presentationBackground`; full swipe off for Deny; Dynamic Type via text styles only; targets at least 44 × 44 pt; haptics: `.success` applied/forwarded, `.warning` on `hash_mismatch`/`needs_terminal`/`limit_reached`, `.error` on SSH failure, `.selection` on chip/option changes, no haptic on heartbeats; Reduce Motion stops the pulse.

### 11.4 Content limits (from the protocol and design)

Commands up to 4096 bytes, reasons up to 1024, up to 32 prompt options of up to 256 bytes, up to 64 projects per stream, summaries up to 1024 bytes, messages to the PM up to 500 bytes; a stream line at most 65536 bytes including the newline; poll interval at least 1000 ms.

### 11.5 Reconciliations made while writing this plan

- The design record's "Face ID before connecting" is read as `FaceIDRule` (plan-142 says so).
- The record's push controls on board 10 are dropped (D8), because the settled decision is that push is deferred.
- The record's Verification section reports the lowest contrast as 4.88:1 (first pass) while the Approval section reports 5.26:1 for boards 01 and 14 after revision 2; these are different measures (all boards vs the two revised boards) and are not a conflict.
- plan-142 puts device checks in PM-382's first package; this plan places them in `device-checks`, after `ios-shell` (they need an app to run in), and keeps the background-refresh device check in Phase 3.
- plan-142 refers to PM-382's "ios-core and the app shell" as its first packages; here `ios-core` (pure logic) and `ios-shell` (project) are those two.
