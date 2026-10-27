# Research: a macOS desktop app that manages Capstan on remote Linux hosts (PM-285)

## Executive summary

- **Transport:** use the system `ssh` binary (OpenSSH) with ControlMaster/ControlPersist and keepalives, and run a remote command that streams JSON lines. Docker does the same: its SSH contexts run `docker system dial-stdio` on the remote and forward stdio. This reuses `~/.ssh/config`, the agent, Keychain and ProxyJump. Pure-library SSH (Citadel, SwiftNIO SSH, russh) is a fallback, not a starting point.
- **App technology:** native SwiftUI (MenuBarExtra plus UserNotifications) fits a Mac-first app best. Tauri 2's notification plugin documents that notification *actions* are mobile-only, which weakens its case for approve/deny from a notification. A Tauri front end can avoid Node (Trunk with Leptos is documented), but the repo's no-Node rule would still need a separate repo or directory decision.
- **Capstan must add:** a machine-wide, sequence-numbered event stream command with replay from a cursor, so a reconnecting client gets at-least-once delivery. No such command was verified here (not examined; this report is web research only).
- **Security:** one-tap approvals are the push-MFA-fatigue pattern. Notifications should be read-only or "open the app" in stage 1; acting needs context shown, binding to a specific request id, and device-auth/number-match style friction.
- **Staged MVP:** (1) read-only list plus notifications, (2) actions with confirmation, (3) terminal jump (open Terminal/iTerm via ssh, only later an embedded SwiftTerm).

Caveat: web search results were thin in places. Items marked "unverified" are from my own background knowledge or were not confirmed; see "Could not verify".

## Method

Date of research and access: 2026-10-27. Tools: WebSearch (standard mode) and WebFetch for vendor docs and repositories. Queries covered: Docker context over SSH; VS Code Remote-SSH and ControlMaster; Tauri 2 with Rust front ends, notification plugin, macOS signing; UNUserNotificationCenter actions; OpenSSH streamlocal forwarding; Sparkle; iTerm2 tmux integration and Warp SSH; Tailscale macOS variants; MFA fatigue; ssh_config man page; Citadel, swift-nio-ssh, russh, SwiftTerm repositories. Reddit and Hacker News were not queried (the question is mostly answerable from docs); this is a gap, not a finding. Page content was treated as untrusted data.

## Findings

### 1. How comparable apps handle remote targets and notifications

**Consensus**
- *Docker contexts (SSH).* The client runs SSH and executes `docker system dial-stdio` on the remote to forward the API over stdio; this avoids hard-coding the remote socket path ([OneUptime](https://oneuptime.com/blog/post/2026-01-16-docker-contexts-remote-management/view), [pkg.go.dev knative func/ssh](https://pkg.go.dev/knative.dev/func/pkg/ssh)). Hosts can be aliases in `~/.ssh/config`. Failures reported are mostly auth (`Permission denied (publickey)`) and remote user lacking rights ([Docker forum](https://forums.docker.com/t/docker-compose-through-ssh-failing-and-referring-to-docker-example-com/115165)). Docker's model has no event replay or notifications of its own.
- *VS Code Remote-SSH.* Installs a server on the remote and tunnels all communication over SSH ([VS Code docs](https://code.visualstudio.com/docs/remote/ssh)). The official tips recommend ControlMaster/ControlPersist so extra connections multiplex over one authenticated master ([VS Code blog 2019](https://code.visualstudio.com/blogs/2019/10/03/remote-ssh-tips-and-tricks), [MIT ORCD](https://orcd-docs.mit.edu/accessing-orcd/control-channels/)). A CERN guide notes multiplexing does not work with the extension's remote-command mode ([CERN ATLAS docs](https://atlas-software.docs.cern.ch/athena/ide/ssh)).
- *iTerm2 / Warp.* iTerm2 tmux control mode (`tmux -CC`) keeps sessions alive when SSH drops and reattaches with `tmux -CC attach` ([iTerm2 docs](https://iterm2.com/documentation-tmux-integration.html)). Warp's legacy SSH wrapper restricts remote shells, does not support tmux and fails when `RemoteCommand` is set ([Warp docs](https://docs.warp.dev/terminal/warpify/ssh-legacy)). Lesson: wrappers around ssh that depend on remote shell state are brittle; keeping state in a remote durable process (tmux, here Herdr/controller) and treating the client as disposable works.
- *Tailscale.* The Mac app ships in three variants (App Store, standalone with system extension, open-source CLI daemon) and surfaces problems as warnings in the menu-bar item ([Tailscale docs](https://tailscale.com/docs/concepts/macos-variants)). Useful pattern: a menu-bar status item carries "needs attention" states. Its client-to-daemon protocol was not verified.

**Isolated / not found**
- No example of a menu-bar tool watching remote jobs over SSH was found in this research (not verified either way).

### 2. Transport from a Mac to a remote Unix socket

**Consensus (from OpenSSH docs and mailing-list sources)**
- OpenSSH supports Unix-socket forwarding through `direct-streamlocal@openssh.com` and `streamlocal-forward@openssh.com` ([OpenSSH dev list, 2006 patch](https://lists.mindrot.org/pipermail/openssh-unix-dev/2006-April/024201.html)). This is `ssh -L local.sock:remote.sock`.
- Stale socket files break re-forwarding; `StreamLocalBindUnlink yes` removes an existing socket ([OpenSSH dev list](https://lists.mindrot.org/pipermail/openssh-unix-dev/2006-April/024201.html), [ssh_config man page](https://man.openbsd.org/ssh_config)).
- Server policy interacts: disabling TCP forwarding also disables streamlocal forwards, and `PermitOpen` does not restrict socket paths like hosts ([OpenSSH dev list 2016](https://lists.mindrot.org/pipermail/openssh-unix-dev/2016-November/035498.html)). Single-source observation; check against the VM's sshd.
- ssh_config: `ControlMaster` shares sessions over one connection; `ControlPersist` keeps the master in background; `ServerAliveInterval` defaults to 0 (disabled), `ServerAliveCountMax` defaults to 3; `ExitOnForwardFailure` defaults to no ([man.openbsd.org](https://man.openbsd.org/ssh_config)).

**Options compared (analysis, mine)**
| Option | Pros | Cons |
|---|---|---|
| Remote command streaming JSON lines over `ssh host cstan events --follow` | Same as Docker's model; no socket path or credential leaves the host; needs no forwarding permission; the credential stays on the VM | Needs a new Capstan command; one process per project unless the command is machine-wide |
| `ssh -L local.sock:remote.sock` | Reuses existing controller protocol and credential | Credential must be copied or handled by the Mac; stale sockets; per-project forwards; blocked if forwarding disabled |
| ControlMaster multiplexing | One auth, cheap extra channels, survives short blips if master lives | Master can die silently on sleep; needs keepalives and a health check |
| Library (below) | No subprocess, in-process reconnect | Agent/Keychain/ProxyJump/`ssh_config` reimplementation |

**Libraries**
- *Citadel* (Swift, MIT): high-level API over NIOSSH; exec, PTY, SFTP, jump hosts, TCP port forwarding. Unix sockets not mentioned; no releases listed; RSA only in a NIOSSH fork ([GitHub](https://github.com/orlandos-nl/Citadel)).
- *swift-nio-ssh* (Apache-2.0): exec/shell channels, TCP forwarding, password and public-key auth, Ed25519 and ECDSA only; agent and Unix-socket forwarding not mentioned; the README says it does not ship production-ready clients ([GitHub](https://github.com/apple/swift-nio-ssh)).
- *russh* (Rust, Apache-2.0, v0.63): client and server, `direct-streamlocal` client-side and `forward-streamlocal`, OpenSSH agent forwarding channels listed ([GitHub](https://github.com/Eugeny/russh)).
- libssh2 was not researched (unverified).

**Reliability (analysis, partly unverified)**: treat the SSH channel as lossy. Enable `ServerAliveInterval`/`ServerAliveCountMax` so a dead link after sleep is detected, reconnect with backoff on wake, and make the stream resumable by cursor (see section 5). Mac sleep/wake behaviour of ControlMaster was not sourced.

**Keys and agent**: using the system `ssh` inherits the user's agent, Keychain-backed keys (`AddKeysToAgent`/`UseKeychain` in ssh_config; not sourced here) and `ProxyJump`. Libraries above do not document agent support, so they would need their own key handling.

### 3. App technology

**SwiftUI / AppKit**
- `MenuBarExtra` with `.menu` and `.window` styles, macOS 13+ (from background knowledge; the Apple page returned no content, unverified).
- Actionable notifications: actions grouped into a `UNNotificationCategory`, registered at launch; one delegate method handles taps and the app has a short time to act ([Cocoacasts](https://courses.cocoacasts.com/actionable-notifications-with-the-user-notifications-framework), [Hacking with Swift](https://www.hackingwithswift.com/read/21/3/acting-on-responses)). Notification center works only for bundled apps ([par-term source](https://docs.rs/crate/par-term/0.38.0/source/src/platform/notify_macos.rs)).
- *SwiftTerm* (MIT-style Swift library, not license-verified): AppKit `TerminalView` and `LocalProcessTerminalView` that runs a command on a local pty; SSH is not built in ([GitHub](https://github.com/migueldeicaza/SwiftTerm)). Embedded terminal can run `ssh -t host ...` locally.
- Auto-update: Sparkle, with EdDSA signatures in the appcast plus Apple code signing ([Sparkle EdDSA docs](https://sparkle-project.org/documentation/eddsa-migration)).

**Tauri 2**
- Frontend can be Rust: the Leptos guide uses Trunk with `beforeDevCommand: "trunk serve"` and `beforeBuildCommand: "trunk build"` and never mentions Node, though Trunk's own prerequisites weren't checked ([Tauri Leptos guide](https://v2.tauri.app/start/frontend/leptos/)). Yew and Dioxus were not verified; the Dioxus docs URL returned 404.
- Notification plugin: "The Actions API is only available on mobile platforms." So there are no action buttons on macOS desktop through the official plugin ([Tauri notification plugin](https://v2.tauri.app/plugin/notification/)). A native bridge via custom Rust/objc code is possible (my inference, not sourced).
- Signing and notarization: Developer ID certificate plus notarization via `APPLE_API_*` or `APPLE_ID`/`APPLE_PASSWORD`/`APPLE_TEAM_ID`; a free Apple account cannot notarize ([Tauri macOS signing](https://v2.tauri.app/distribute/sign/macos/)). The same Apple requirement applies to a SwiftUI app.
- Updater: official plugin exists (from background knowledge; unverified).

**Comparison (analysis, mine)**
| | SwiftUI | Tauri 2 (Rust UI) |
|---|---|---|
| Notification actions on macOS | Native, full | Not in official plugin |
| Menu bar | MenuBarExtra | Tray (not researched) |
| Embedded terminal | SwiftTerm | Web terminal (xterm.js, needs JS) or shell out |
| Cross-platform later | No | Yes (Windows/Linux) |
| No-Node rule | Fine; Xcode toolchain instead | Possible with Trunk; adds Rust UI stack |
| Footprint | Small, native | Webview, larger |
Other options named, not researched: Electron (conflicts with no-Node), Flutter, Dioxus desktop, a TUI/remote web UI (a web UI served by the controller would avoid a Mac app but loses native notifications).

### 4. Security of acting on approvals from notifications

**Consensus**: push approvals invite approval fatigue; attackers trigger repeated prompts until the user taps approve; fixes are number matching, rate limiting, showing application and context, and phishing-resistant methods ([SecureW2](https://securew2.com/blog/what-is-mfa-fatigue-and-how-do-attackers-exploit-it), [Beyond Identity](https://www.beyondidentity.com/resource/using-push-notifications-for-mfa-is-a-security-liability), [Protectimus](https://www.protectimus.com/blog/mfa-fatigue-attacks/)). Microsoft made number matching mandatory for Authenticator push in 2023 (reported by the search summary; the underlying Microsoft page was not opened).

**Applied to Capstan (analysis)**: the threat is mostly accidental taps and stale prompts, not external attackers, since the channel is the user's own SSH. Mitigations: approvals bound to a request id and content hash so a stale or replaced proposal can't be approved; show the exact command or diff in the app, not just the notification; no destructive approvals directly from the banner (open the app first); require unlock/Touch ID in the app (Apple `UNNotificationActionOptions.authenticationRequired` is believed to require device unlock, but Apple's page returned no content so this is unverified; Airship's docs describe the iOS flag this way); idempotent server-side decisions so a retried tap after reconnect cannot double-apply; audit log of who approved from where.

### 5. Recommendation

**Architecture**: native SwiftUI menu-bar app. Per host, a supervised `ssh` child with ControlMaster and keepalives runs a Capstan machine-wide event stream; the app keeps a cursor per host, de-duplicates by event id, and reconnects with backoff, including on wake. Notifications are derived from "waiting on user" events. Actions later go through a separate short-lived `ssh host cstan ...` call, not the event stream.

**Staged MVP**
1. Read-only: add host (an ssh alias), list projects and agents, show action-needed items, native notifications that open the app. No credentials leave the VM.
2. Actions: approve/deny/answer/pause with request-id binding, confirmation in the app, unlock required, and idempotent server handling.
3. Terminal: first "open in Terminal/iTerm" running `ssh -t` into the PM's Herdr pane; embedded SwiftTerm later.

**What Capstan must add** (suggestions; the codebase was not inspected): `cstan events --follow --since <cursor>` machine-wide across projects, one JSON object per line with a monotonic per-host sequence id, a stable event type for each waiting-on-user item, replay from a cursor with a "gap/reset" marker if the cursor is too old, heartbeats, a snapshot command for current state, and a decision command taking a request id and expected content hash. It should run as the user, reading each controller's socket locally so the credential stays on the VM.

## Conflicts and disagreements

- Tauri's Leptos guide does not mention Node, but whether Trunk or the Tauri CLI needs Node in practice was not confirmed; treat "no Node" for Tauri as plausible, not proven.
- Libraries differ on Unix-socket forwarding: russh lists streamlocal; Citadel and swift-nio-ssh do not mention it. Absence on a README is not proof of absence.
- VS Code recommends ControlMaster, but CERN reports it does not work with the extension's remote-command mode; for a `ssh host cmd` stream design this needs a quick test.
- Warp's SSH wrapper docs are mixed legacy/new; its current tmux status was not verified.

## Sources

Accessed 2026-10-27 unless noted. Publication dates are shown where the page gave one; "n/d" = not displayed.

1. OneUptime, "How to Set Up Docker Contexts for Remote Container Management", https://oneuptime.com/blog/post/2026-01-16-docker-contexts-remote-management/view, 2026-01-16 (from URL).
2. knative func, ssh package, https://pkg.go.dev/knative.dev/func/pkg/ssh, n/d.
3. Docker forums, "Docker Compose through SSH failing...", https://forums.docker.com/t/docker-compose-through-ssh-failing-and-referring-to-docker-example-com/115165, n/d.
4. VS Code, "Remote Development using SSH", https://code.visualstudio.com/docs/remote/ssh, n/d.
5. VS Code blog, "Remote SSH tips and tricks", https://code.visualstudio.com/blogs/2019/10/03/remote-ssh-tips-and-tricks, 2019-10-03.
6. MIT ORCD, "Control channels", https://orcd-docs.mit.edu/accessing-orcd/control-channels/, n/d.
7. CERN ATLAS software docs, SSH, https://atlas-software.docs.cern.ch/athena/ide/ssh, n/d.
8. iTerm2, tmux integration, https://iterm2.com/documentation-tmux-integration.html, n/d.
9. Warp docs, Legacy SSH wrapper, https://docs.warp.dev/terminal/warpify/ssh-legacy, n/d.
10. Tailscale, "Three ways to run Tailscale on macOS", https://tailscale.com/docs/concepts/macos-variants, n/d.
11. OpenSSH dev list, "[Patch] Unix Domain Socket Forwarding", https://lists.mindrot.org/pipermail/openssh-unix-dev/2006-April/024201.html, 2006-04.
12. OpenSSH dev list, "Allow UNIX socket forwarding, but disallow TCP forwarding?", https://lists.mindrot.org/pipermail/openssh-unix-dev/2016-November/035498.html, 2016-11.
13. OpenBSD, ssh_config(5), https://man.openbsd.org/ssh_config, n/d.
14. Citadel, https://github.com/orlandos-nl/Citadel, n/d.
15. apple/swift-nio-ssh, https://github.com/apple/swift-nio-ssh, n/d.
16. Eugeny/russh, https://github.com/Eugeny/russh, n/d.
17. migueldeicaza/SwiftTerm, https://github.com/migueldeicaza/SwiftTerm, n/d.
18. Cocoacasts, "Actionable notifications with the User Notifications framework", https://courses.cocoacasts.com/actionable-notifications-with-the-user-notifications-framework, n/d.
19. Hacking with Swift, "Acting on responses", https://www.hackingwithswift.com/read/21/3/acting-on-responses, n/d.
20. par-term source (notify_macos.rs), https://docs.rs/crate/par-term/0.38.0/source/src/platform/notify_macos.rs, n/d.
21. Sparkle, EdDSA migration, https://sparkle-project.org/documentation/eddsa-migration, n/d.
22. Tauri, Leptos guide, https://v2.tauri.app/start/frontend/leptos/, n/d.
23. Tauri, Notification plugin, https://v2.tauri.app/plugin/notification/, n/d.
24. Tauri, macOS code signing, https://v2.tauri.app/distribute/sign/macos/, n/d.
25. SecureW2, "What Is MFA Fatigue...", https://securew2.com/blog/what-is-mfa-fatigue-and-how-do-attackers-exploit-it, n/d.
26. Beyond Identity, "Using push notifications for MFA is a security liability", https://www.beyondidentity.com/resource/using-push-notifications-for-mfa-is-a-security-liability, n/d.
27. Protectimus, "MFA Fatigue Attacks", https://www.protectimus.com/blog/mfa-fatigue-attacks/, n/d.

## Could not verify

- Reddit and Hacker News were not queried at all; no community experience reports on menu-bar remote-job watchers, Remote-SSH reconnect failures or Tauri-on-macOS notification problems are included.
- Apple's pages for `MenuBarExtra` and `UNNotificationActionOptions.authenticationRequired` returned no body text; macOS 13+ and the unlock requirement are from background knowledge and secondary sources (Airship).
- The Dioxus guide URL returned 404; Yew and Dioxus were not assessed. Whether Trunk or the Tauri CLI requires Node was not confirmed.
- Tauri updater and tray behaviour, libssh2, Docker Desktop's own UI, Tailscale's GUI-to-daemon protocol, and VS Code's reconnect behaviour and `remote.SSH` settings were not sourced.
- Mac sleep/wake behaviour of ControlMaster and keepalives was not sourced.
- Whether Capstan already has any event-stream command was not checked; section 5's "must add" list is a design suggestion.
- The Microsoft number-matching claim comes from a search summary; the Microsoft page was not opened.
- Some search results (e.g. one Tauri query) returned unrelated pages and were discarded.
