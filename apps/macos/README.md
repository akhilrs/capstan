# Capstan for macOS

A SwiftUI client for Capstan hosts. It needs macOS 26 and Xcode 26.

| Folder | What it holds |
| --- | --- |
| `Capstan.xcodeproj` | the app project (target and shared scheme `Capstan`) |
| `Capstan/` | the app sources; the project folder is synchronized, so a new file here is part of the build |
| `CapstanKit/` | local Swift package: protocol v1 client, `ssh` argument quoting, text integrity |
| `CapstanAppCore/` | local Swift package: app logic that needs no SwiftUI |
| `scripts/` | static checks |

## Build and run

1. Open `apps/macos/Capstan.xcodeproj` in Xcode 26.
2. Select the Capstan target, then Signing & Capabilities: set your Team and a bundle identifier of your own. Notifications may not authorize until the app is signed.
3. Choose Run.

From a terminal:

```sh
xcodebuild -project apps/macos/Capstan.xcodeproj -scheme Capstan -configuration Debug -destination 'platform=macOS' build
```

## Tests

```sh
swift test --package-path apps/macos/CapstanKit
swift test --package-path apps/macos/CapstanAppCore
```

The shared scheme has no tests of its own; the package tests above are the test suites.

## Static checks

These run anywhere with `bash` and `python3` (no Xcode needed). They are guards, not proofs: only a build on a Mac shows that the code compiles.

```sh
apps/macos/scripts/check-static.sh              # source rules; also runs the project file check
apps/macos/scripts/check-static.sh --self-test
python3 apps/macos/scripts/check-xcodeproj.py   # project.pbxproj and the shared scheme
python3 apps/macos/scripts/check-xcodeproj.py --self-test
```

## Checking a host

The app reaches a host over `ssh` and runs `cstan` there. Before adding a host, check that it works without a prompt:

```sh
ssh -o BatchMode=yes <host> cstan help events
```

If that fails, fix it in a terminal first (key in your agent, host key accepted, `cstan` installed). If `cstan` is not on the remote `PATH`, set the host's cstan path in its settings: a plain command name or a path, nothing else. The app stores no key, passphrase or password; authentication is left to `ssh` and your agent.

## Why there is no sandbox

The app starts the system `ssh` and uses your `~/.ssh` configuration and agent. An App Sandbox would block both, so the project sets `ENABLE_APP_SANDBOX = NO` and has no entitlements file. Hardened Runtime stays on.

## Time Sensitive notifications (optional)

The app runs without the Time Sensitive entitlement, but then the setting "Allow during Focus" has no effect. To enable it, add the capability by hand: select the Capstan target, open Signing & Capabilities, choose + Capability, and add Time Sensitive Notifications. This needs a signed build with your Team.

## If the project does not open

Create it again:

1. In Xcode choose File › New › Project › macOS › App, name it `Capstan`, interface SwiftUI, language Swift, and save it in `apps/macos`.
2. Remove the template sources it generated and add the existing `Capstan` folder as a synchronized folder.
3. Add both local packages (File › Add Package Dependencies › Add Local): `CapstanKit` and `CapstanAppCore`, and link each product to the Capstan target.
4. Set the deployment target to macOS 26.0 and Swift language version to 6, and turn App Sandbox off.

Then run `python3 apps/macos/scripts/check-xcodeproj.py` to compare it with what the repository expects.

## Sending errors back

If a build fails, send the error lines: from `xcodebuild`, the lines that contain `error:`; from Xcode, the text of the entries in the Issue navigator. Include the Xcode version.
