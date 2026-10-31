# Capstan for iPhone

A SwiftUI client for Capstan hosts, with its own SSH client. It needs iOS 26 and Xcode 26. The plan the packages follow is `docs/plans/iphone-app-plan.md`; the design is `docs/design/iphone-app.md`.

| Folder | What it holds |
| --- | --- |
| `CapstanPhone.xcodeproj` | the app project (target and shared scheme `CapstanPhone`) |
| `CapstanPhone/` | the app sources; the project folder is synchronized, so a new file here is part of the build (`Info.plist` is kept out of the target's files) |
| `CapstanPhoneCore/` | local Swift package: the phone's key, Face ID rule, host trust, phone hosts, coordinator, lifecycle, settings |
| `CapstanSSH/` | local Swift package: SSH exec channels on swift-nio-ssh behind CapstanKit's `ProcessRunner` |
| `Spikes/NIOSSHSpike/` | the record of what swift-nio-ssh can do; nothing depends on it |
| `scripts/` | static checks |

The app also uses `../macos/CapstanKit` and `../macos/CapstanAppCore`, by relative path.

## Requirements

- Xcode 26 with the iOS platform (Xcode › Settings › Components › iOS) for the Simulator.
- An iPhone with iOS 26 for the device checks: the Secure Enclave, Face ID, the Local Network prompt and background refresh only run on a device.

## Build and run

0. Run `xcode-select -p`. If it prints a Command Line Tools path, switch to Xcode with `sudo xcode-select -s /Applications/Xcode.app/Contents/Developer`.
1. Open `apps/ios/CapstanPhone.xcodeproj` in Xcode 26.
2. Select the CapstanPhone target, then Signing & Capabilities: set your Team and a bundle identifier of your own (the project has `com.example.capstan.phone`). Do not add Push Notifications: the app has no push (plan decision D8).
3. Pick an iPhone Simulator and choose Run. The app opens on five empty tabs; with Face ID on (the default) it first shows the lock. In the Simulator, Features › Face ID › Enrolled, then Matching Face, unlocks it.

From a terminal (Nushell):

```nu
xcodebuild -project apps/ios/CapstanPhone.xcodeproj -scheme CapstanPhone -destination 'generic/platform=iOS Simulator' build
```

## Tests

```nu
swift test --package-path apps/ios/CapstanPhoneCore
swift test --sanitize=thread --package-path apps/ios/CapstanPhoneCore
swift test --package-path apps/ios/CapstanSSH
swift test --package-path apps/macos/CapstanKit
swift test --package-path apps/macos/CapstanAppCore
```

The shared scheme has no tests of its own; the package tests above are the test suites. `CapstanSSH`'s tests start an SSH server on `127.0.0.1` inside the test process.

## Static checks

These run anywhere with `bash`, `perl` and `python3` (no Xcode needed). They are guards, not proofs: only a build on a Mac shows that the code compiles.

```nu
apps/ios/scripts/check-static.sh                       # source rules; also runs the two checks below
apps/ios/scripts/check-static.sh --self-test
python3 apps/ios/scripts/check-xcodeproj.py            # project.pbxproj, Info.plist and the shared scheme
python3 apps/ios/scripts/check-xcodeproj.py --self-test
python3 apps/ios/scripts/check-shared-platform.py      # shared code that must build for iOS
python3 apps/ios/scripts/check-shared-platform.py --self-test
```

## If the project does not open

Create it again:

1. In Xcode choose File › New › Project › iOS › App, name it `CapstanPhone`, interface SwiftUI, language Swift, and save it in `apps/ios`.
2. Remove the template sources it generated and add the existing `CapstanPhone` folder as a synchronized folder; in its target membership, leave `Info.plist` out.
3. Add the four local packages (File › Add Package Dependencies › Add Local): `../macos/CapstanKit`, `../macos/CapstanAppCore`, `CapstanPhoneCore` and `CapstanSSH`, and link each product to the CapstanPhone target.
4. Set the deployment target to iOS 26.0, iPhone only, Swift language version 6, and Info.plist File to `CapstanPhone/Info.plist`.

Then run `python3 apps/ios/scripts/check-xcodeproj.py` to compare it with what the repository expects.

## Sending errors back

If a build fails, send the error lines: from `xcodebuild`, the lines that contain `error:`; from Xcode, the text of the entries in the Issue navigator. Include the Xcode version.
